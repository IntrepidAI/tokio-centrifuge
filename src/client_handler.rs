use std::collections::HashMap;
use std::io::BufRead;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use async_tungstenite::tokio::ConnectStream;
use async_tungstenite::tungstenite::Message;
use async_tungstenite::WebSocketStream;
use futures::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

use crate::config::Protocol;
use crate::protocol::{Command, RawCommand, RawReply, Reply};

#[derive(Error, Debug)]
pub enum ReplyError {
    #[error("request timed out after {0:?}")]
    Timeout(Duration),
    #[error("connection closed")]
    Closed,
}

#[allow(clippy::option_map_unit_fn)]
#[allow(clippy::type_complexity)]
pub async fn websocket_handler(
    rt: tokio::runtime::Handle,
    stream: WebSocketStream<ConnectStream>,
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
                        Message::Text(text) => text.into_bytes(),
                        Message::Binary(bin) => bin,
                        Message::Close(close_frame) => {
                            if let Some(close_frame) = close_frame {
                                let code: u16 = close_frame.code.into();
                                let reason = close_frame.reason;
                                let reconnect = !matches!(code, 3500..=3999 | 4500..=4999);
                                log::debug!("connection closed by remote, code={code}, reason={reason}");
                                break 'outer reconnect;
                            }
                            break 'outer true;
                        }
                        _ => continue 'outer,
                    };

                    let handle_frame = |result| {
                        let (frame, frame_id) = match result {
                            Ok(data) => data,
                            Err(err) => {
                                log::debug!("failed to parse frame: {}", err);
                                on_error(err);
                                return;
                            }
                        };

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
                    };

                    match protocol {
                        Protocol::Json => {
                            for line in data.lines() {
                                let line = match line {
                                    Ok(line) => line,
                                    Err(err) => {
                                        log::debug!("failed to read line: {}", err);
                                        on_error(anyhow!(err));
                                        continue;
                                    }
                                };

                                log::trace!("<-- {:?}", line);

                                let result = (|| -> Result<(Reply, u32), anyhow::Error> {
                                    let frame: RawReply = serde_json::from_str(&line)?;
                                    let frame_id = frame.id;
                                    let frame: Reply = frame.into();
                                    Ok((frame, frame_id))
                                })();

                                handle_frame(result);
                            }
                        }
                        Protocol::Protobuf => {
                            let mut data = &data[..];

                            while !data.is_empty() {
                                let Ok(len) = prost::decode_length_delimiter(data) else {
                                    break;
                                };
                                let len_delimiter_len = prost::length_delimiter_len(len);
                                log::trace!("<-- {}", format_protobuf(&data[..len_delimiter_len + len]));
                                data = &data[len_delimiter_len..];

                                let result = (|| -> Result<(Reply, u32), anyhow::Error> {
                                    let frame = RawReply::decode(&data[..len])?;
                                    let frame_id = frame.id;
                                    let frame: Reply = frame.into();
                                    Ok((frame, frame_id))
                                })();

                                data = &data[len..];
                                handle_frame(result);
                            }
                        }
                    }
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
                        let message = encode_commands(&[RawCommand::from(Command::Empty)], protocol);
                        match write_ws.send(message).await {
                            Ok(()) => (),
                            Err(err) => {
                                on_error(anyhow!(err));
                                break 'outer;
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
                            let _ = reply_ch.send(Err(ReplyError::Timeout(timeout)));
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
                                        let _ = ch.send(Err(ReplyError::Timeout(timeout)));
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
                        let message = encode_commands(&commands, protocol);
                        commands.clear();

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
        let reconnect = true;
        log::debug!("websocket connection aborted, reconnect={}", reconnect);
        reconnect
    }
}

fn format_protobuf(buf: &[u8]) -> String {
    fn buf_to_hex(buf: &[u8]) -> String {
        buf.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join("")
    }

    let Ok(len) = prost::decode_length_delimiter(buf) else {
        return buf_to_hex(buf);
    };
    let len_delimiter_len = prost::length_delimiter_len(len);

    format!("{} {}", buf_to_hex(&buf[..len_delimiter_len]), buf_to_hex(&buf[len_delimiter_len..]))
}

fn encode_commands(commands: &[RawCommand], protocol: Protocol) -> Message {
    match protocol {
        Protocol::Json => {
            let lines = commands.iter().map(|command| {
                let line = serde_json::to_string(command).unwrap();
                log::trace!("--> {}", &line);
                line
            }).collect::<Vec<_>>();

            Message::Text(lines.join("\n"))
        }
        Protocol::Protobuf => {
            let mut buf = Vec::new();
            for command in commands.iter() {
                let buf_len = buf.len();
                command.encode_length_delimited(&mut buf).unwrap();
                log::trace!("--> {}", format_protobuf(&buf[buf_len..]));
            }
            Message::Binary(buf)
        }
    }
}
