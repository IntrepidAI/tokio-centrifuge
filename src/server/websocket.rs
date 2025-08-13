//! # WebSocket Module
//!
//! This module handles WebSocket connection management, including message
//! reading/writing, ping/pong handling, and coordination between reader
//! and writer tasks.
//!
//! ## Core Functionality
//!
//! - **Connection Management**: Splits WebSocket streams into read/write halves
//! - **Message Processing**: Handles incoming commands and outgoing replies
//! - **Ping/Pong**: Manages connection health with configurable intervals
//! - **Task Coordination**: Coordinates reader and writer tasks for clean shutdown
//!
//! ## Architecture
//!
//! The WebSocket handling is split into two main tasks:
//! - **Reader Task**: Processes incoming messages and manages session state
//! - **Writer Task**: Sends replies and manages outgoing message batching

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};
use matchit::Router;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::protocol::{Command, RawCommand, RawReply, Reply};
use crate::server::callbacks::{ChannelFn, OnConnectFn, OnDisconnectFn, PublishFn, RpcMethodFn};
use crate::server::session::ServerSession;
use crate::server::types::ServeParams;
use crate::utils::{decode_frames, encode_frames};

/// Serves WebSocket connections with full protocol support
///
/// This function handles the complete WebSocket connection lifecycle,
/// including authentication, message processing, and connection cleanup.
///
/// ## Arguments
///
/// * `stream` - WebSocket stream to serve connections on
/// * `params` - Server configuration parameters
/// * `on_connect` - Connection handler function
/// * `on_disconnect` - Optional disconnect handler function
/// * `rpc_methods` - Registered RPC methods
/// * `sub_channels` - Registered subscription channels
/// * `pub_channels` - Registered publication channels
///
/// ## Features
///
/// - **Bidirectional Communication**: Handles both incoming and outgoing messages
/// - **Connection Health**: Manages ping/pong for connection monitoring
/// - **Error Handling**: Graceful error handling and connection cleanup
/// - **Resource Management**: Proper cleanup of tasks and resources
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::websocket::serve_websocket;
/// use tokio_centrifuge::server::types::ServeParams;
/// use tokio_tungstenite::accept_async;
///
/// // In a real implementation, you would have these handlers
/// // let params = ServeParams::json();
/// // let (ws_stream, _) = accept_async(request).await?;
///
/// // Create server session (in async context)
/// // serve_websocket(
/// //     ws_stream,
/// //     params,
/// //     on_connect_fn,
/// //     Some(on_disconnect_fn),
/// //     rpc_methods,
/// //     sub_channels,
/// //     pub_channels,
/// // ).await;
/// ```
///
/// ## Task Architecture
///
/// The function spawns two main tasks:
///
/// 1. **Reader Task**: Processes incoming WebSocket messages
///    - Handles client commands
///    - Manages session state
///    - Sends replies through internal channel
///
/// 2. **Writer Task**: Sends outgoing WebSocket messages
///    - Receives replies from internal channel
///    - Batches messages for efficiency
///    - Handles WebSocket write operations
///
/// ## Connection Lifecycle
///
/// 1. **Setup**: Split stream, create channels, spawn tasks
/// 2. **Processing**: Handle messages, manage session, coordinate tasks
/// 3. **Cleanup**: Join tasks, close stream, cleanup resources
pub async fn serve_websocket<S>(
    stream: S,
    mut params: ServeParams,
    on_connect: OnConnectFn,
    on_disconnect: Option<OnDisconnectFn>,
    rpc_methods: Arc<Mutex<Router<RpcMethodFn>>>,
    sub_channels: Arc<Mutex<Router<ChannelFn>>>,
    pub_channels: Arc<Mutex<Router<PublishFn>>>,
) where
    S: Stream<Item = Result<Message, WsError>> + Sink<Message> + Send + Unpin + 'static,
{
    params.client_id = Some(params.client_id.unwrap_or_default());

    let (mut write_ws, mut read_ws) = stream.split();
    let (closer_tx, mut closer_rx) = tokio::sync::mpsc::channel::<()>(1);

    let mut ping_timer = tokio::time::interval(params.ping_interval.unwrap_or(Duration::MAX));
    ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let (reply_ch_tx, mut reply_ch_rx) = tokio::sync::mpsc::channel(64);

    let reader_task = tokio::spawn(async move {
        let mut timer_send_ping = Box::pin(tokio::time::sleep(Duration::MAX));
        let mut timer_expect_pong = Box::pin(tokio::time::sleep(Duration::MAX));

        let mut session = ServerSession::new(
            params.clone(),
            reply_ch_tx.clone(),
            on_connect,
            on_disconnect,
            rpc_methods,
            sub_channels,
            pub_channels,
        );

        let error_code = 'outer: loop {
            tokio::select! {
                biased;

                _ = closer_rx.recv() => {
                    break 'outer crate::errors::DisconnectErrorCode::ConnectionClosed;
                }

                _ = &mut timer_send_ping => {
                    if session.state() == crate::server::SessionState::Connected {
                        let _ = reply_ch_tx.send(Ok((0, Reply::Empty))).await;
                    }
                    timer_send_ping.as_mut().reset(
                        Instant::now()
                            .checked_add(params.ping_interval.unwrap_or(Duration::MAX))
                            .unwrap_or(far_future())
                            .checked_add(params.pong_timeout.unwrap_or(Duration::MAX))
                            .unwrap_or(far_future())
                    );
                }

                remote_msg = read_ws.next() => {
                    let message = match remote_msg {
                        Some(Ok(message)) => message,
                        _ => break 'outer crate::errors::DisconnectErrorCode::ConnectionClosed,
                    };

                    let data = match message {
                        Message::Text(text) => text.into(),
                        Message::Binary(bin) => bin,
                        Message::Close(_close_frame) => {
                            break 'outer crate::errors::DisconnectErrorCode::ConnectionClosed;
                        }
                        _ => continue,
                    };

                    let mut commands = Vec::new();

                    let _ = decode_frames(&data, params.encoding, |result| {
                        let result: RawCommand = result?;
                        let frame_id = result.id;
                        let frame: Command = result.into();
                        commands.push((frame_id, frame));
                        Ok(())
                    });

                    for (frame_id, frame) in commands {
                        if session.state() == crate::server::SessionState::Initial {
                            if let Command::Connect(_) = frame {
                                timer_send_ping.as_mut().reset(
                                    Instant::now()
                                        .checked_add(params.ping_interval.unwrap_or(Duration::MAX))
                                        .unwrap_or(far_future())
                                );
                                timer_expect_pong.as_mut().reset(
                                    Instant::now()
                                        .checked_add(params.ping_interval.unwrap_or(Duration::MAX))
                                        .unwrap_or(far_future())
                                        .checked_add(params.pong_timeout.unwrap_or(Duration::MAX))
                                        .unwrap_or(far_future())
                                );
                            }
                        }

                        if let Command::Empty = frame {
                            timer_send_ping.as_mut().reset(
                                Instant::now()
                                    .checked_add(params.ping_interval.unwrap_or(Duration::MAX))
                                    .unwrap_or(far_future())
                            );
                            timer_expect_pong.as_mut().reset(
                                Instant::now()
                                    .checked_add(params.ping_interval.unwrap_or(Duration::MAX))
                                    .unwrap_or(far_future())
                                    .checked_add(params.pong_timeout.unwrap_or(Duration::MAX))
                                    .unwrap_or(far_future())
                            );
                            continue;
                        }

                        session.process_command(frame_id, frame).await;
                    }
                }

                _ = timer_expect_pong.as_mut() => {
                    if session.state() == crate::server::SessionState::Initial {
                        break 'outer crate::errors::DisconnectErrorCode::Stale;
                    } else {
                        break 'outer crate::errors::DisconnectErrorCode::NoPong;
                    }
                }
            }
        };

        let _ = reply_ch_tx.send(Err(error_code)).await;
        read_ws
    });

    let writer_task = tokio::spawn(async move {
        let mut batch = Vec::new();

        let error_code = 'outer: loop {
            let control_msgs = reply_ch_rx.recv_many(&mut batch, 32).await;
            if control_msgs == 0 {
                break 'outer crate::errors::DisconnectErrorCode::ConnectionClosed;
            }

            let mut replies = Vec::with_capacity(batch.len());
            let mut error = None;

            for message in batch.drain(..) {
                match message {
                    Ok((frame_id, reply)) => {
                        let mut reply = RawReply::from(reply);
                        reply.id = frame_id;
                        replies.push(reply);
                    }
                    Err(err) => {
                        error = Some(err);
                        break;
                    }
                }
            }

            let data = encode_frames(&replies, params.encoding, |_id| {
                error = Some(crate::errors::DisconnectErrorCode::InappropriateProtocol);
            });
            if let Some(data) = data {
                match write_ws.send(data).await {
                    Ok(()) => (),
                    Err(_err) => {
                        break 'outer crate::errors::DisconnectErrorCode::WriteError;
                    }
                }
            }

            if let Some(err) = error {
                break 'outer err;
            }
        };

        let _ = closer_tx.try_send(());
        let _ = write_ws.send(Message::Close(error_code.into())).await;
        log::debug!(
            "client disconnected, code={}, reason={}",
            error_code.0,
            error_code
        );

        write_ws
    });

    let (read_ws, write_ws) = tokio::join!(reader_task, writer_task);

    if let (Ok(read_ws), Ok(write_ws)) = (read_ws, write_ws) {
        let mut stream = read_ws.reunite(write_ws).unwrap();
        let _ = stream.close().await;
    } else {
        log::debug!("failed to join reader and writer tasks");
    }
}

/// Creates a timestamp far in the future for timer calculations
///
/// This function is used to set timers to a distant future time
/// when no immediate timeout is needed. It's approximately 30 years
/// in the future to avoid overflow issues on different platforms.
///
/// ## Platform Considerations
///
/// - **macOS**: 1000 years overflows
/// - **FreeBSD**: 100 years overflows
/// - **30 years**: Safe on all supported platforms
///
/// ## Returns
///
/// An `Instant` representing a time far in the future
///
/// ## Implementation Note
///
/// This is based on tokio's Instant implementation and provides
/// a safe way to set timers without immediate expiration.
fn far_future() -> Instant {
    // Roughly 30 years from now.
    // API does not provide a way to obtain max `Instant`
    // or convert specific date in the future to instant.
    // 1000 years overflows on macOS, 100 years overflows on FreeBSD.
    Instant::now() + Duration::from_secs(86400 * 365 * 30)
}
