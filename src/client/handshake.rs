//! WebSocket handshake and connection establishment.
//!
//! This module handles the initial handshake process when establishing
//! a connection to the Centrifugo server, including authentication
//! and protocol negotiation.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::client_handler::ReplyError;

use crate::protocol::{Command, ConnectRequest};
use crate::subscription;

use super::inner::ClientInner;

/// Manages the WebSocket handshake process.
///
/// This struct handles the initial connection handshake with the server,
/// including sending authentication credentials and establishing the
/// communication protocol.
pub(crate) struct HandshakeManager;

impl HandshakeManager {
    /// Performs the initial handshake with the server.
    ///
    /// This function establishes the connection by:
    /// 1. Creating control channels for communication
    /// 2. Sending a connect request with authentication
    /// 3. Setting up the WebSocket handler
    /// 4. Waiting for the server's response
    ///
    /// # Arguments
    ///
    /// * `client` - Reference to the client instance
    /// * `closer_write` - Sender for close signals
    /// * `closer_read` - Receiver for close signals
    /// * `stream` - The established WebSocket stream
    ///
    /// # Returns
    ///
    /// Returns a tuple containing:
    /// - The handler future for managing the connection
    /// - The control channel for sending commands
    ///
    /// Or an error if the handshake failed.
    #[allow(clippy::type_complexity)]
    pub(crate) fn do_handshake(
        client: &Arc<Mutex<ClientInner>>,
        closer_write: mpsc::Sender<bool>,
        closer_read: mpsc::Receiver<bool>,
        stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> impl Future<
        Output = Result<
            (
                Pin<Box<dyn Future<Output = bool> + Send>>,
                mpsc::Sender<(
                    Command,
                    oneshot::Sender<Result<crate::protocol::Reply, ReplyError>>,
                    Duration,
                )>,
            ),
            bool,
        >,
    > + '_ {
        let (control_write, control_read) = mpsc::channel(32);

        let (rt, connect_rx, protocol) = {
            let inner = client.lock().unwrap();

            let (connect_tx, connect_rx) = oneshot::channel();

            let command = Command::Connect(ConnectRequest {
                token: inner.token.clone(),
                data: vec![],
                subs: std::collections::HashMap::new(),
                name: inner.name.clone(),
                version: inner.version.clone(),
            });
            let timeout = inner.read_timeout;

            control_write
                .try_send((command, connect_tx, timeout))
                .unwrap();

            let rt = inner.rt.clone();
            (rt, connect_rx, inner.protocol)
        };

        async move {
            //
            // Spawn tokio task to handle reads and writes to websocket
            //
            let client1 = client.clone();
            let client2 = client.clone();
            let mut handler_future: Pin<Box<dyn Future<Output = bool> + Send>> =
                Box::pin(crate::client_handler::websocket_handler(
                    rt,
                    stream,
                    control_read,
                    closer_read,
                    protocol,
                    move |push| {
                        if let crate::protocol::Reply::Push(push) = push {
                            if let crate::protocol::PushData::Publication(publication) = push.data {
                                let mut inner = client1.lock().unwrap();
                                if let Some(sub_id) =
                                    inner.sub_name_to_id.get(&push.channel).copied()
                                {
                                    if let Some(sub) = inner.subscriptions.get_mut(sub_id) {
                                        if sub.state == subscription::State::Subscribed {
                                            if let Some(ref mut on_publication) = sub.on_publication
                                            {
                                                on_publication(publication);
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            log::debug!("unexpected push: {:?}", push);
                        }
                    },
                    move |err| {
                        let mut inner = client2.lock().unwrap();
                        if let Some(ref mut on_error) = inner.on_error {
                            on_error(err);
                        }
                    },
                ));

            //
            // Send handshake request and wait for reply
            //
            tokio::select! {
                biased;

                do_reconnect = &mut handler_future => {
                    Err(do_reconnect)
                }

                result = connect_rx => {
                    match result {
                        Ok(Ok(reply)) => {
                            match reply {
                                crate::protocol::Reply::Connect(connect) => {
                                    // handshake completed
                                    log::debug!("connection established with {} {}", connect.client, connect.version);
                                    Ok((handler_future, control_write))
                                }
                                crate::protocol::Reply::Error(err) => {
                                    log::debug!("handshake failed: {}", &err.message);
                                    // close connection, don't reconnect
                                    let _ = closer_write.try_send(err.temporary);
                                    Err(handler_future.await)
                                }
                                _ => {
                                    log::debug!("unexpected reply: {:?}", reply);
                                    // close connection, don't reconnect
                                    let _ = closer_write.try_send(false);
                                    Err(handler_future.await)
                                }
                            }
                        }
                        Ok(Err(err)) => {
                            // connection closed, or timeout, or smth like that,
                            // we close connection and then reconnect (if timeout)
                            log::debug!("handshake failed: {:?}", err);
                            let _ = closer_write.try_send(true);
                            Err(handler_future.await)
                        }
                        Err(err) => {
                            // same as above
                            log::debug!("handshake failed: {:?}", err);
                            let _ = closer_write.try_send(true);
                            Err(handler_future.await)
                        }
                    }
                }
            }
        }
    }
}
