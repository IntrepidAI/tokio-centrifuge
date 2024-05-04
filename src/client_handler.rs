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
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

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
    mut control_ch: mpsc::Receiver<(Command, oneshot::Sender<Result<Reply, ReplyError>>, Duration)>,
    mut closer_ch: mpsc::Receiver<bool>,
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
                                #[allow(clippy::manual_range_contains)]
                                let reconnect = code < 3500 || code >= 5000 || (code >= 4000 && code < 4500);
                                log::debug!("connection closed by remote, code={code}, reason={reason}");
                                break 'outer reconnect;
                            }
                            break 'outer true;
                        }
                        _ => continue 'outer,
                    };

                    for line in data.lines() {
                        let line = match line {
                            Ok(line) => line,
                            Err(err) => {
                                log::debug!("failed to read line: {}", err);
                                on_error(anyhow!(err));
                                continue;
                            }
                        };

                        println!("<-- {}", line);

                        let result = (|| -> Result<(Reply, u32), anyhow::Error> {
                            let frame: RawReply = serde_json::from_str(&line)?;
                            let frame_id = frame.id;
                            let frame: Reply = frame.into();
                            Ok((frame, frame_id))
                        })();

                        let (frame, frame_id) = match result {
                            Ok(data) => data,
                            Err(err) => {
                                log::debug!("failed to parse frame: {}", err);
                                log::debug!("{}", line);
                                on_error(err);
                                continue;
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
        'outer: loop {
            tokio::select! {
                biased;

                ping = ping_read.recv() => {
                    if ping.is_some() {
                        let message = Message::Text("{}".to_string());
                        println!("--> {}", message.to_text().unwrap());
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

                control_msg = control_ch.recv() => {
                    let (data, reply_ch, timeout) = match control_msg {
                        Some(data) => data,
                        None => break 'outer,
                    };

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
                    let message = Message::Text(serde_json::to_string(&command).unwrap());
                    println!("--> {}", message.to_text().unwrap());

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
