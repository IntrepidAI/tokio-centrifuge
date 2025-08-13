//! # Client Handler Module
//!
//! This module handles the WebSocket communication layer for client connections.
//! It manages message reading/writing, reply tracking, and connection lifecycle
//! for client connections to Centrifugo servers.
//!
//! ## Core Functionality
//!
//! - **WebSocket Handler**: Main function for managing client WebSocket connections
//! - **Reply Tracking**: Maps request IDs to response channels
//! - **Message Processing**: Handles incoming replies and push notifications
//! - **Connection Health**: Manages ping/pong and connection monitoring
//!
//! ## Architecture
//!
//! The WebSocket handler is split into two main tasks:
//! - **Reader Task**: Processes incoming WebSocket messages
//! - **Writer Task**: Sends outgoing commands and manages reply tracking

use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use futures::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::config::Protocol;
use crate::protocol::{Command, RawCommand, RawReply, Reply};
use crate::utils::{decode_frames, encode_frames};

/// Errors that can occur when processing replies
///
/// This enum covers various failure scenarios in reply handling,
/// including timeouts, connection issues, and protocol violations.
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyError {
    /// The request timed out waiting for a response
    #[error("request timed out")]
    Timeout,
    /// The connection was closed before receiving a reply
    #[error("connection closed")]
    Closed,
    /// The protocol used is inappropriate for the request
    #[error("inappropriate protocol")]
    InappropriateProtocol,
}

/// Main WebSocket handler for client connections
///
/// This function manages the complete WebSocket connection lifecycle for clients,
/// including message reading/writing, reply tracking, and connection health monitoring.
///
/// ## Arguments
///
/// * `rt` - Tokio runtime handle for spawning tasks
/// * `stream` - WebSocket stream to handle
/// * `control_ch` - Channel for sending commands to the server
/// * `closer_ch` - Channel for receiving close signals
/// * `protocol` - Protocol encoding to use (JSON/Protobuf)
/// * `on_push` - Callback for handling push notifications
/// * `on_error` - Callback for handling errors
///
/// ## Returns
///
/// Returns `true` if the connection should be reconnected, `false` otherwise.
///
/// ## Features
///
/// - **Bidirectional Communication**: Handles both incoming and outgoing messages
/// - **Reply Tracking**: Maps request IDs to response channels for correlation
/// - **Push Notifications**: Processes server-initiated messages
/// - **Connection Health**: Manages ping/pong for connection monitoring
/// - **Error Handling**: Graceful error handling and connection cleanup
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::client_handler::websocket_handler;
/// use tokio_centrifuge::config::Protocol;
///
/// // In a real implementation, you would have these channels and handlers
/// // let (control_tx, control_rx) = mpsc::channel(64);
/// // let (closer_tx, closer_rx) = mpsc::channel(1);
///
/// // let should_reconnect = websocket_handler(
/// //     rt,
/// //     ws_stream,
/// //     control_rx,
/// //     closer_rx,
/// //     Protocol::Json,
/// //     |push| { /* handle push */ },
/// //     |err| { /* handle error */ },
/// // ).await;
/// ```
///
/// ## Task Architecture
///
/// The function spawns two main tasks:
///
/// 1. **Reader Task**: Processes incoming WebSocket messages
///    - Handles server replies and push notifications
///    - Manages reply mapping and correlation
///    - Monitors connection health
///
/// 2. **Writer Task**: Sends outgoing WebSocket messages
///    - Processes commands from control channel
///    - Manages reply tracking and timeouts
///    - Handles WebSocket write operations
#[allow(clippy::option_map_unit_fn)]
#[allow(clippy::type_complexity)]
pub async fn websocket_handler(
    rt: tokio::runtime::Handle,
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    mut control_ch: mpsc::Receiver<(
        Command,
        oneshot::Sender<Result<Reply, ReplyError>>,
        Duration,
    )>,
    mut closer_ch: mpsc::Receiver<bool>,
    protocol: Protocol,
    on_push: impl Fn(Reply) + Send + Sync + 'static,
    on_error: impl Fn(anyhow::Error) + Send + Sync + 'static,
) -> bool {
    struct ReplyMap<T> {
        id: AtomicU32,
        map: Mutex<HashMap<u32, (oneshot::Sender<T>, Option<AbortHandle>)>>,
    }
    let reply_map_arc = Arc::new(ReplyMap::<Result<Reply, ReplyError>> {
        id: AtomicU32::new(1),
        map: Mutex::new(HashMap::new()),
    });

    let (mut write_ws, mut read_ws) = stream.split();
    let (ping_write, mut ping_read) = mpsc::channel::<()>(1);
    let on_error_arc = Arc::new(on_error);

    let on_error = on_error_arc.clone();
    let reply_map = reply_map_arc.clone();
    let reader_task = rt.spawn(async move {
        let do_reconnect = 'outer: loop {
            tokio::select! {
                biased;

                do_reconnect = closer_ch.recv() => {
                    break 'outer do_reconnect.unwrap_or(false);
                }

                remote_msg = read_ws.next() => {
                    let message = match remote_msg {
                        Some(Ok(message)) => message,
                        Some(Err(err)) => {
                            log::debug!("failed to read message: {}", err);
                            on_error(anyhow!(err));
                            break 'outer true;
                        }
                        None => break 'outer true,
                    };

                    let data = match message {
                        Message::Text(text) => text.into(),
                        Message::Binary(bin) => bin,
                        Message::Close(close_frame) => {
                            if let Some(close_frame) = close_frame {
                                let code: u16 = close_frame.code.into();
                                let reason = close_frame.reason;
                                let reconnect = crate::errors::DisconnectErrorCode(code).should_reconnect();
                                log::debug!("connection closed by remote, code={code}, reason={reason}");
                                break 'outer reconnect;
                            }
                            break 'outer true;
                        }
                        _ => continue 'outer,
                    };

                    let _ = decode_frames(&data, protocol, |result| {
                        let raw_frame: RawReply = match result {
                            Ok(data) => data,
                            Err(err) => {
                                on_error(err);
                                return Ok(());
                            }
                        };

                        let frame_id = raw_frame.id;
                        let frame: Reply = raw_frame.into();

                        if let Reply::Empty = frame {
                            let _ = ping_write.try_send(());
                        } else if frame_id == 0 {
                            on_push(frame);
                        } else {
                            let mut map = reply_map.map.lock().unwrap();
                            match map.remove(&frame_id) {
                                Some((reply_ch, abort_handle)) => {
                                    let _ = reply_ch.send(Ok(frame));
                                    abort_handle.map(|h| h.abort());
                                }
                                None => {
                                    log::debug!("unknown reply id={}", frame_id);
                                    on_error(anyhow!("unknown reply id={}", frame_id))
                                }
                            }
                        }

                        Ok(())
                    });
                }
            }
        };

        drop(ping_write);
        (read_ws, do_reconnect)
    });

    let on_error = on_error_arc;
    let reply_map = reply_map_arc.clone();
    let writer_task = rt.spawn(async move {
        let mut batch = Vec::new();
        let mut commands = Vec::new();

        'outer: loop {
            tokio::select! {
                biased;

                ping = ping_read.recv() => {
                    if ping.is_some() {
                        let message = encode_frames(&[RawCommand::from(Command::Empty)], protocol, |_| {});
                        if let Some(message) = message {
                            match write_ws.send(message).await {
                                Ok(()) => (),
                                Err(err) => {
                                    on_error(anyhow!(err));
                                    break 'outer;
                                }
                            }
                        }
                    } else {
                        break 'outer;
                    }
                }

                control_msgs = control_ch.recv_many(&mut batch, 32) => {
                    if control_msgs == 0 {
                        break 'outer;
                    }

                    for control_msg in batch.drain(..) {
                        let (data, reply_ch, timeout) = control_msg;

                        if timeout == Duration::ZERO {
                            let _ = reply_ch.send(Err(ReplyError::Timeout));
                            continue 'outer;
                        }

                        let id = loop {
                            let id = reply_map.id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if id != 0 {
                                break id;
                            }
                        };

                        let abort_handle = {
                            if timeout == Duration::MAX {
                                None
                            } else {
                                let reply_map = reply_map.clone();
                                Some(tokio::spawn(async move {
                                    tokio::time::sleep(timeout).await;
                                    let mut map = reply_map.map.lock().unwrap();
                                    if let Some((ch, _)) = map.remove(&id) {
                                        let _ = ch.send(Err(ReplyError::Timeout));
                                    }
                                }).abort_handle())
                            }
                        };

                        {
                            let mut map = reply_map.map.lock().unwrap();
                            map.insert(id, (reply_ch, abort_handle));
                        }

                        let mut command = RawCommand::from(data);
                        command.id = id;
                        commands.push(command);
                    }

                    if !commands.is_empty() {
                        let message = encode_frames(&commands, protocol, |id| {
                            let mut map = reply_map.map.lock().unwrap();
                            let Some(id) = commands.get(id).map(|c| c.id) else { return; };
                            if let Some((ch, abort_handle)) = map.remove(&id) {
                                let _ = ch.send(Err(ReplyError::InappropriateProtocol));
                                abort_handle.map(|h| h.abort());
                            }
                        });
                        commands.clear();

                        if let Some(message) = message {
                            match write_ws.send(message).await {
                                Ok(()) => (),
                                Err(err) => {
                                    on_error(anyhow!(err));
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
        }

        write_ws
    });

    let (read_ws, write_ws) = tokio::join!(reader_task, writer_task);

    {
        let mut reply_map = reply_map_arc.map.lock().unwrap();
        for (_, (sender, abort_handle)) in reply_map.drain() {
            let _ = sender.send(Err(ReplyError::Closed));
            abort_handle.map(|h| h.abort());
        }
    }

    if let (Ok((read_ws, reconnect)), Ok(write_ws)) = (read_ws, write_ws) {
        let mut stream = read_ws.reunite(write_ws).unwrap();
        let _ = stream.close(None).await;
        log::debug!("websocket connection closed, reconnect={}", reconnect);
        reconnect
    } else {
        // don't reconnect in case of panic, because it can cause infinite reconnects
        let reconnect = false;
        log::debug!("websocket connection aborted, reconnect={}", reconnect);
        reconnect
    }
}
