use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_tungstenite::tungstenite::{Error as WsError, Message};
use futures::{Sink, SinkExt, Stream, StreamExt};
use tokio::sync::Semaphore;
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::Instant;

use crate::config::Protocol;
use crate::errors::{ClientErrorCode, DisconnectErrorCode};
use crate::protocol::{
    Command, ConnectResult, Publication, Push, PushData, RawCommand, RawReply, Reply, RpcResult,
    SubscribeResult, UnsubscribeResult,
};
use crate::utils::{decode_frames, encode_frames};

const PING_INTERVAL: Duration = Duration::from_secs(25);
const PING_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Initial,
    Connected,
    //Terminated,
}

struct ServerSession {
    state: SessionState,
    rpc_semaphore: Arc<Semaphore>,
    rpc_methods: Arc<Mutex<HashMap<String, RpcMethodFn>>>,
    sub_channels: Arc<Mutex<HashMap<String, ChannelFn>>>,
    subscriptions: HashMap<String, AbortHandle>,
    tasks: JoinSet<()>,
    reply_ch: tokio::sync::mpsc::Sender<Result<(u32, Reply), DisconnectErrorCode>>,
}

impl ServerSession {
    fn new(
        reply_ch: tokio::sync::mpsc::Sender<Result<(u32, Reply), DisconnectErrorCode>>,
        rpc_methods: Arc<Mutex<HashMap<String, RpcMethodFn>>>,
        sub_channels: Arc<Mutex<HashMap<String, ChannelFn>>>,
    ) -> Self {
        const MAX_CONCURRENT_REQUESTS: usize = 10;

        Self {
            state: SessionState::Initial,
            rpc_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            rpc_methods,
            sub_channels,
            subscriptions: HashMap::new(),
            tasks: JoinSet::new(),
            reply_ch,
        }
    }

    async fn process_command(&mut self, id: u32, command: Command) {
        const MAX_SUBSCRIPTION_COUNT: usize = 128;

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
                        let _ = self.reply_ch.send(Err(DisconnectErrorCode::BadRequest)).await;
                    }
                }
            }
            SessionState::Connected => {
                match command {
                    Command::Connect(_) => {
                        log::debug!("client already authenticated");
                        let _ = self.reply_ch.send(Err(DisconnectErrorCode::BadRequest)).await;
                    }
                    Command::Rpc(rpc_request) => {
                        let f = {
                            let methods = self.rpc_methods.lock().unwrap();
                            methods.get(&rpc_request.method).cloned()
                        };

                        let Some(f) = f else {
                            let _ = self.reply_ch.send(Ok((id, Reply::Error(ClientErrorCode::MethodNotFound.into())))).await;
                            return;
                        };

                        let permit = self.rpc_semaphore.clone().acquire_owned().await;
                        let reply_ch = self.reply_ch.clone();
                        self.tasks.spawn(async move {
                            let result = f(rpc_request.data).await;
                            match result {
                                Ok(data) => {
                                    let _ = reply_ch.send(Ok((id, Reply::Rpc(RpcResult { data })))).await;
                                }
                                Err((code, message)) => {
                                    let _ = reply_ch.send(Ok((id, Reply::Error(crate::protocol::Error {
                                        code: code.0.into(),
                                        message,
                                        temporary: code.is_temporary(),
                                    })))).await;
                                }
                            }
                            drop(permit);
                        });
                    }
                    Command::Subscribe(sub_request) => {
                        if self.subscriptions.contains_key(&sub_request.channel) {
                            let _ = self.reply_ch.send(Ok((id, Reply::Error(ClientErrorCode::AlreadySubscribed.into())))).await;
                            return;
                        }

                        if self.subscriptions.len() >= MAX_SUBSCRIPTION_COUNT {
                            log::warn!("subscription limit exceeded");
                            let _ = self.reply_ch.send(Ok((id, Reply::Error(ClientErrorCode::LimitExceeded.into())))).await;
                            return;
                        }

                        let f = {
                            let methods = self.sub_channels.lock().unwrap();
                            methods.get(&sub_request.channel).cloned()
                        };

                        let Some(f) = f else {
                            let _ = self.reply_ch.send(Ok((id, Reply::Error(ClientErrorCode::UnknownChannel.into())))).await;
                            return;
                        };

                        let reply_ch = self.reply_ch.clone();
                        let channel_name = sub_request.channel.clone();

                        let abort_handle = self.tasks.spawn(async move {
                            let mut stream = f();

                            while let Some(data) = stream.next().await {
                                let _ = reply_ch.send(Ok((0, Reply::Push(Push {
                                    channel: channel_name.clone(),
                                    data: PushData::Publication(Publication {
                                        data,
                                        ..Default::default()
                                    }),
                                })))).await;
                            }
                        });
                        self.subscriptions.insert(sub_request.channel, abort_handle);

                        let _ = self.reply_ch.send(Ok((id, Reply::Subscribe(SubscribeResult::default())))).await;
                    }
                    Command::Unsubscribe(unsub_request) => {
                        if let Some(abort_handle) = self.subscriptions.remove(&unsub_request.channel) {
                            abort_handle.abort();
                        }

                        let _ = self.reply_ch.send(Ok((id, Reply::Unsubscribe(UnsubscribeResult::default())))).await;
                    }
                    _ => {
                        let _ = self.reply_ch.send(Ok((id, Reply::Error(ClientErrorCode::NotAvailable.into())))).await;
                    }
                }
            }
            // SessionState::Terminated => {}
        }
    }

    fn state(&self) -> SessionState {
        self.state
    }
}

type RpcMethodFn = Arc<
    dyn Fn(Vec<u8>) ->
        Pin<Box<dyn Future<Output = Result<Vec<u8>, (ClientErrorCode, String)>> + Send>>
    + Send + Sync
>;

type ChannelFn = Arc<dyn Fn() -> Pin<Box<dyn Stream<Item = Vec<u8>> + Send>> + Send + Sync>;

#[derive(Default, Clone)]
pub struct Server {
    rpc_methods: Arc<Mutex<HashMap<String, RpcMethodFn>>>,
    sub_channels: Arc<Mutex<HashMap<String, ChannelFn>>>,
}

impl Server {
    pub fn new() -> Self {
        Self::default()
    }

    // pub fn add_rpc_method<T, R>(&self, name: &str, f: impl Fn(T) -> anyhow::Result<R> + Send + Sync + 'static)
    pub fn add_rpc_method<In, Out, Fut>(&self, name: &str, f: impl Fn(In) -> Fut + Send + Sync + 'static)
    where
        In: serde::de::DeserializeOwned,
        Out: serde::Serialize,
        Fut: std::future::Future<Output = anyhow::Result<Out>> + Send + 'static,
    {
        let f = Arc::new(f);
        let wrap_f: RpcMethodFn = Arc::new(move |data: Vec<u8>| {
            let f = f.clone();
            Box::pin(async move {
                let data = match serde_json::from_slice(&data) {
                    Ok(data) => data,
                    Err(err) => {
                        return Err((ClientErrorCode::BadRequest, err.to_string()));
                    }
                };
                let result = match f(data).await {
                    Ok(result) => result,
                    Err(err) => {
                        return Err((ClientErrorCode::Internal, err.to_string()));
                    }
                };
                let bytes = match serde_json::to_vec(&result) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        return Err((ClientErrorCode::Internal, err.to_string()));
                    }
                };
                Ok(bytes)
            })
        });
        self.rpc_methods.lock().unwrap().insert(name.to_string(), wrap_f);
    }

    pub fn add_channel<St, T>(&self, name: &str, f: impl Fn() -> St + Send + Sync + 'static)
    where
        St: Stream<Item = T> + Send + 'static,
        T: serde::Serialize + Send + 'static,
    {
        let f = Arc::new(f);
        let wrap_f: ChannelFn = Arc::new(move || {
            let stream = f();
            let result = stream.filter_map(|item| async move {
                let data = match serde_json::to_vec(&item) {
                    Ok(data) => data,
                    Err(err) => {
                        log::error!("failed to serialize channel item: {}", err);
                        return None;
                    }
                };
                Some(data)
            });
            Box::pin(result)
        });
        self.sub_channels.lock().unwrap().insert(name.to_string(), wrap_f);
    }

    pub async fn serve<S>(&self, stream: S, protocol: Protocol)
    where
        S: Stream<Item = Result<Message, WsError>> + Sink<Message> + Send + Unpin + 'static,
    {
        let (mut write_ws, mut read_ws) = stream.split();
        let (closer_tx, mut closer_rx) = tokio::sync::mpsc::channel::<()>(1);

        let mut ping_timer = tokio::time::interval(PING_INTERVAL);
        ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let rpc_methods = self.rpc_methods.clone();
        let sub_channels = self.sub_channels.clone();
        let (reply_ch_tx, mut reply_ch_rx) = tokio::sync::mpsc::channel(64);

        let reader_task = tokio::spawn(async move {
            let mut timer_send_ping = Box::pin(tokio::time::sleep(PING_INTERVAL + PING_TIMEOUT));
            let mut timer_expect_pong = Box::pin(tokio::time::sleep(PING_TIMEOUT));

            let mut session = ServerSession::new(
                reply_ch_tx.clone(),
                rpc_methods,
                sub_channels,
            );

            let error_code = 'outer: loop {
                tokio::select! {
                    biased;

                    _ = closer_rx.recv() => {
                        break 'outer DisconnectErrorCode::ConnectionClosed;
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
                            _ => break 'outer DisconnectErrorCode::ConnectionClosed,
                        };

                        let data = match message {
                            Message::Text(text) => text.into_bytes(),
                            Message::Binary(bin) => bin,
                            Message::Close(_close_frame) => {
                                break 'outer DisconnectErrorCode::ConnectionClosed;
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
                            break 'outer DisconnectErrorCode::Stale;
                        } else {
                            break 'outer DisconnectErrorCode::NoPong;
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
                    break 'outer DisconnectErrorCode::ConnectionClosed;
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

                let data = encode_frames(&replies, protocol, |_id| {
                    error = Some(DisconnectErrorCode::InappropriateProtocol);
                });
                if let Some(data) = data {
                    match write_ws.send(data).await {
                        Ok(()) => (),
                        Err(_err) => {
                            break 'outer DisconnectErrorCode::WriteError;
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

        if let (Ok(read_ws), Ok(write_ws)) = (read_ws, write_ws) {
            let mut stream = read_ws.reunite(write_ws).unwrap();
            let _ = stream.close().await;
        } else {
            log::debug!("failed to join reader and writer tasks");
        }
    }
}
