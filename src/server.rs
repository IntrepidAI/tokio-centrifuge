use std::time::Duration;

use async_tungstenite::tungstenite::Message;
use futures::{AsyncRead, AsyncWrite, SinkExt, StreamExt};
use tokio::time::Instant;

use crate::config::Protocol;
use crate::errors::ErrorCode;
use crate::protocol::{Command, ConnectResult, RawCommand, RawReply, Reply};
use crate::utils::{decode_frames, encode_frames};

const PING_INTERVAL: Duration = Duration::from_secs(25);
const PING_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Initial,
    Connected,
    Terminated,
}

struct ServerSession {
    state: SessionState,
    reply_ch: tokio::sync::mpsc::Sender<Result<(u32, Reply), ErrorCode>>,
}

impl ServerSession {
    fn new(reply_ch: tokio::sync::mpsc::Sender<Result<(u32, Reply), ErrorCode>>) -> Self {
        Self {
            state: SessionState::Initial,
            reply_ch,
        }
    }

    async fn process_command(&mut self, id: u32, command: Command) {
        match self.state {
            SessionState::Initial => {
                match command {
                    Command::Connect(connect) => {
                        log::debug!("connection established with name={}, version={}", connect.name, connect.version);
                        self.state = SessionState::Connected;
                        let _ = self.reply_ch.send(Ok((id, Reply::Connect(ConnectResult {
                            client: uuid::Uuid::new_v4().to_string(),
                            ping: PING_INTERVAL.as_secs() as u32,
                            pong: true,
                            ..Default::default()
                        })))).await;
                    }
                    _ => {
                        log::debug!("expected connect request, got: {:?}", command);
                        let _ = self.reply_ch.send(Err(ErrorCode::BadRequest)).await;
                    }
                }
            }
            SessionState::Connected => {
                log::debug!("received message: {:?}", command);
            }
            SessionState::Terminated => {}
        }
    }

    fn state(&self) -> SessionState {
        self.state
    }
}

pub struct Server;

impl Server {
    pub async fn accept_async<S>(stream: S, protocol: Protocol) -> anyhow::Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let websocket = async_tungstenite::accept_async(stream).await?;
        let (mut write_ws, mut read_ws) = websocket.split();
        let (closer_tx, mut closer_rx) = tokio::sync::mpsc::channel::<()>(1);

        let mut ping_timer = tokio::time::interval(PING_INTERVAL);
        ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let (reply_ch_tx, mut reply_ch_rx) = tokio::sync::mpsc::channel(64);

        let reader_task = tokio::spawn(async move {
            let mut timer_send_ping = Box::pin(tokio::time::sleep(PING_INTERVAL + PING_TIMEOUT));
            let mut timer_expect_pong = Box::pin(tokio::time::sleep(PING_TIMEOUT));

            let mut session = ServerSession::new(reply_ch_tx.clone());

            let error_code = 'outer: loop {
                tokio::select! {
                    biased;

                    _ = closer_rx.recv() => {
                        break 'outer ErrorCode::ConnectionClosed;
                    }

                    _ = timer_send_ping.as_mut() => {
                        if session.state() == SessionState::Connected {
                            let _ = reply_ch_tx.send(Ok((0, Reply::Empty))).await;
                        }
                        timer_send_ping.as_mut().reset(Instant::now() + PING_INTERVAL + PING_TIMEOUT);
                    }

                    remote_msg = read_ws.next() => {
                        let message = match remote_msg {
                            Some(Ok(message)) => message,
                            _ => break 'outer ErrorCode::ConnectionClosed,
                        };

                        let data = match message {
                            Message::Text(text) => text.into_bytes(),
                            Message::Binary(bin) => bin,
                            Message::Close(_close_frame) => {
                                break 'outer ErrorCode::ConnectionClosed;
                            }
                            _ => continue,
                        };

                        let mut commands = Vec::new();

                        let _ = decode_frames(&data, protocol, |result| {
                            let result: RawCommand = result?;
                            let frame_id = result.id;
                            let frame: Command = result.into();
                            commands.push((frame_id, frame));
                            Ok(())
                        });

                        for (frame_id, frame) in commands {
                            if session.state() == SessionState::Initial {
                                if let Command::Connect(_) = frame {
                                    timer_send_ping.as_mut().reset(Instant::now() + PING_INTERVAL);
                                    timer_expect_pong.as_mut().reset(Instant::now() + PING_INTERVAL + PING_TIMEOUT);
                                }
                            }

                            if let Command::Empty = frame {
                                timer_send_ping.as_mut().reset(Instant::now() + PING_INTERVAL);
                                timer_expect_pong.as_mut().reset(Instant::now() + PING_INTERVAL + PING_TIMEOUT);
                                continue;
                            }

                            session.process_command(frame_id, frame).await;
                        }
                    }

                    _ = timer_expect_pong.as_mut() => {
                        if session.state() == SessionState::Initial {
                            break 'outer ErrorCode::Stale;
                        } else {
                            break 'outer ErrorCode::NoPong;
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
                    break 'outer ErrorCode::ConnectionClosed;
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

                let data = encode_frames(&replies, protocol, |_, _| ());
                if let Some(data) = data {
                    match write_ws.send(data).await {
                        Ok(()) => (),
                        Err(_err) => {
                            break 'outer ErrorCode::WriteError;
                        }
                    }
                }

                if let Some(err) = error {
                    break 'outer err;
                }
            };

            let _ = closer_tx.try_send(());
            let _ = write_ws.send(Message::Close(error_code.into())).await;
            log::debug!("client disconnected, code={}, reason={}", error_code.0, error_code);

            write_ws
        });

        let (read_ws, write_ws) = tokio::join!(reader_task, writer_task);

        Ok(())
    }
}
