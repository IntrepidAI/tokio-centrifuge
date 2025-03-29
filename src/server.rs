use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};
use matchit::{InsertError, Router};
use tokio::sync::Semaphore;
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::config::Protocol;
use crate::errors::{ClientError, ClientErrorCode, DisconnectErrorCode};
use crate::protocol::{
    Command, ConnectResult, Publication, Push, PushData, RawCommand, RawReply, Reply, RpcResult,
    SubscribeResult, Unsubscribe, UnsubscribeResult,
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

struct Subscription {
    abort_handle: AbortHandle,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

struct ServerSession {
    client_id: uuid::Uuid,
    state: SessionState,
    on_connect: OnConnectFn,
    rpc_semaphore: Arc<Semaphore>,
    rpc_methods: Arc<Mutex<Router<RpcMethodFn>>>,
    sub_channels: Arc<Mutex<Router<ChannelFn>>>,
    subscriptions: Arc<Mutex<HashMap<String, Subscription>>>,
    tasks: JoinSet<()>,
    reply_ch: tokio::sync::mpsc::Sender<Result<(u32, Reply), DisconnectErrorCode>>,
}

impl ServerSession {
    fn new(
        client_id: uuid::Uuid,
        reply_ch: tokio::sync::mpsc::Sender<Result<(u32, Reply), DisconnectErrorCode>>,
        auth_fn: OnConnectFn,
        rpc_methods: Arc<Mutex<Router<RpcMethodFn>>>,
        sub_channels: Arc<Mutex<Router<ChannelFn>>>,
    ) -> Self {
        const MAX_CONCURRENT_REQUESTS: usize = 10;

        Self {
            on_connect: auth_fn,
            client_id,
            state: SessionState::Initial,
            rpc_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            rpc_methods,
            sub_channels,
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
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
                        match (self.on_connect)(ConnectContext {
                            client_id: self.client_id,
                            token: connect.token,
                            data: connect.data,
                        }).await {
                            Ok(()) => (),
                            Err(err) => {
                                log::debug!("authentication failed: {:?}", err);
                                let _ = self.reply_ch.send(Err(err)).await;
                                return;
                            }
                        }

                        self.state = SessionState::Connected;
                        let _ = self.reply_ch.send(Ok((id, Reply::Connect(ConnectResult {
                            client: self.client_id.as_hyphenated().to_string(),
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
                        let permit = self.rpc_semaphore.clone().acquire_owned().await;

                        let Some(match_) = ({
                            let methods = self.rpc_methods.lock().unwrap();
                            match methods.at(&rpc_request.method) {
                                Ok(match_) => {
                                    let params: HashMap<String, String> = match_.params.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
                                    Some((params, match_.value.clone()))
                                }
                                Err(_err) => {
                                    None
                                }
                            }
                        }) else {
                            let _ = self.reply_ch.send(Ok((id, Reply::Error(ClientErrorCode::MethodNotFound.into())))).await;
                            return;
                        };

                        let reply_ch = self.reply_ch.clone();
                        let client_id = self.client_id;
                        self.tasks.spawn(async move {
                            let (match_params, f) = match_;
                            let result = f(RpcContext { client_id, params: match_params }, rpc_request.data).await;
                            match result {
                                Ok(data) => {
                                    let _ = reply_ch.send(Ok((id, Reply::Rpc(RpcResult { data })))).await;
                                }
                                Err(err) => {
                                    let _ = reply_ch.send(Ok((id, Reply::Error(err.into())))).await;
                                }
                            }
                            drop(permit);
                        });
                    }
                    Command::Subscribe(sub_request) => {
                        let permit = self.rpc_semaphore.clone().acquire_owned().await;

                        let response = (|| {
                            let mut subscriptions = self.subscriptions.lock().unwrap();

                            if subscriptions.contains_key(&sub_request.channel) {
                                return Some(Reply::Error(ClientErrorCode::AlreadySubscribed.into()));
                            }

                            if subscriptions.len() >= MAX_SUBSCRIPTION_COUNT {
                                log::warn!("subscription limit exceeded");
                                return Some(Reply::Error(ClientErrorCode::LimitExceeded.into()));
                            }

                            let Some(match_) = ({
                                let methods = self.sub_channels.lock().unwrap();
                                match methods.at(&sub_request.channel) {
                                    Ok(match_) => {
                                        let params: HashMap<String, String> = match_.params.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
                                        Some((params, match_.value.clone()))
                                    }
                                    Err(_err) => {
                                        None
                                    }
                                }
                            }) else {
                                return Some(Reply::Error(ClientErrorCode::UnknownChannel.into()));
                            };

                            let reply_ch = self.reply_ch.clone();
                            let channel_name = sub_request.channel.clone();
                            let client_id = self.client_id;

                            let subscriptions_ = self.subscriptions.clone();
                            let abort_handle = self.tasks.spawn(async move {
                                let (match_params, f) = match_;
                                let mut stream = match f(SubContext { client_id, params: match_params, data: sub_request.data }).await {
                                    Ok(stream) => stream,
                                    Err(err) => {
                                        subscriptions_.lock().unwrap().remove(&channel_name);
                                        let _ = reply_ch.send(Ok((id, Reply::Error(err.into())))).await;
                                        return;
                                    }
                                };

                                let _ = reply_ch.send(Ok((id, Reply::Subscribe(SubscribeResult::default())))).await;
                                drop(permit);

                                while let Some(data) = stream.next().await {
                                    let _ = reply_ch.send(Ok((0, Reply::Push(Push {
                                        channel: channel_name.clone(),
                                        data: PushData::Publication(Publication {
                                            data,
                                            ..Default::default()
                                        }),
                                    })))).await;
                                }

                                subscriptions_.lock().unwrap().remove(&channel_name);

                                let _ = reply_ch.send(Ok((0, Reply::Push(Push {
                                    channel: channel_name.clone(),
                                    data: PushData::Unsubscribe(Unsubscribe {
                                        code: 2500,
                                        reason: "channel closed".to_string(),
                                    }),
                                })))).await;
                            });
                            subscriptions.insert(sub_request.channel, Subscription { abort_handle });
                            None
                        })();

                        if let Some(response) = response {
                            let _ = self.reply_ch.send(Ok((id, response))).await;
                        }
                    }
                    Command::Unsubscribe(unsub_request) => {
                        let reply = {
                            let mut subscriptions = self.subscriptions.lock().unwrap();
                            if let Some(sub) = subscriptions.remove(&unsub_request.channel) {
                                sub.abort_handle.abort();
                            }
                            Reply::Unsubscribe(UnsubscribeResult::default())
                        };

                        let _ = self.reply_ch.send(Ok((id, reply))).await;
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

#[derive(Debug, Clone)]
pub struct ConnectContext {
    pub client_id: uuid::Uuid,
    pub token: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RpcContext {
    pub client_id: uuid::Uuid,
    pub params: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct SubContext {
    pub client_id: uuid::Uuid,
    pub params: HashMap<String, String>,
    pub data: Vec<u8>,
}

type OnConnectFn = Arc<
    dyn Fn(ConnectContext) ->
        Pin<Box<dyn Future<Output = Result<(), DisconnectErrorCode>> + Send>>
    + Send + Sync
>;

type RpcMethodFn = Arc<
    dyn Fn(RpcContext, Vec<u8>) ->
        Pin<Box<dyn Future<Output = Result<Vec<u8>, ClientError>> + Send>>
    + Send + Sync
>;

type ChannelFn = Arc<
    dyn Fn(SubContext) ->
        Pin<Box<dyn Future<Output = Result<
            Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>,
            ClientError
        >> + Send>>
    + Send + Sync
>;

#[derive(Clone)]
struct ConnectFnWrapper(OnConnectFn);

impl Default for ConnectFnWrapper {
    fn default() -> Self {
        fn default_connect_fn(ctx: ConnectContext) -> Pin<Box<dyn Future<Output = Result<(), DisconnectErrorCode>> + Send>> {
            Box::pin(async move {
                if !ctx.token.is_empty() {
                    return Err(DisconnectErrorCode::InvalidToken);
                }
                Ok(())
            })
        }

        Self(Arc::new(default_connect_fn))
    }
}

#[derive(Default, Clone)]
pub struct Server {
    on_connect: ConnectFnWrapper,
    rpc_methods: Arc<Mutex<Router<RpcMethodFn>>>,
    sub_channels: Arc<Mutex<Router<ChannelFn>>>,
}

impl Server {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_connect<Fut>(&mut self, f: impl Fn(ConnectContext) -> Fut + Send + Sync + 'static)
    where
        Fut: std::future::Future<Output = Result<(), DisconnectErrorCode>> + Send + 'static,
    {
        self.on_connect = ConnectFnWrapper(Arc::new(move |ctx: ConnectContext| {
            Box::pin(f(ctx)) as Pin<Box<dyn Future<Output = Result<(), DisconnectErrorCode>> + Send>>
        }));
    }

    // pub fn add_rpc_method<T, R>(&self, name: &str, f: impl Fn(T) -> anyhow::Result<R> + Send + Sync + 'static)
    pub fn add_rpc_method<In, Out, Fut>(
        &self,
        name: &str,
        f: impl Fn(RpcContext, In) -> Fut + Send + Sync + 'static,
    ) -> Result<(), InsertError>
    where
        In: serde::de::DeserializeOwned,
        Out: serde::Serialize,
        Fut: std::future::Future<Output = Result<Out, ClientError>> + Send + 'static,
    {
        let f = Arc::new(f);
        let wrap_f: RpcMethodFn = Arc::new(move |ctx: RpcContext, data: Vec<u8>| {
            let f = f.clone();
            Box::pin(async move {
                let data = crate::utils::deserialize(&data)?;
                let result = f(ctx, data).await?;
                let bytes = match serde_json::to_vec(&result) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        return Err(ClientError::internal(err.to_string()));
                    }
                };
                Ok(bytes)
            })
        });
        self.rpc_methods.lock().unwrap().insert(name.to_string(), wrap_f)
    }

    pub fn add_channel<Out, Fut, St>(
        &self,
        name: &str,
        f: impl Fn(SubContext) -> Fut + Send + Sync + 'static,
    ) -> Result<(), InsertError>
    where
        Out: serde::Serialize + Send + 'static,
        Fut: std::future::Future<Output = Result<St, ClientError>> + Send + 'static,
        St: Stream<Item = Out> + Send + 'static,
    {
        let f = Arc::new(f);
        let wrap_f: ChannelFn = Arc::new(move |ctx: SubContext| {
            let f = f.clone();
            Box::pin(async move {
                let stream = f(ctx).await?;
                let result = stream.filter_map(async move |item| {
                    let data = match serde_json::to_vec(&item) {
                        Ok(data) => data,
                        Err(err) => {
                            log::error!("failed to serialize channel item: {}", err);
                            return None;
                        }
                    };
                    Some(data)
                });
                Ok(Box::pin(result) as Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>)
            })
        });
        self.sub_channels.lock().unwrap().insert(name.to_string(), wrap_f)
    }

    pub async fn serve<S>(&self, stream: S, protocol: Protocol)
    where
        S: Stream<Item = Result<Message, WsError>> + Sink<Message> + Send + Unpin + 'static,
    {
        let client_id = uuid::Uuid::new_v4();
        let (mut write_ws, mut read_ws) = stream.split();
        let (closer_tx, mut closer_rx) = tokio::sync::mpsc::channel::<()>(1);

        let mut ping_timer = tokio::time::interval(PING_INTERVAL);
        ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let auth_fn = self.on_connect.0.clone();
        let rpc_methods = self.rpc_methods.clone();
        let sub_channels = self.sub_channels.clone();
        let (reply_ch_tx, mut reply_ch_rx) = tokio::sync::mpsc::channel(64);

        let reader_task = tokio::spawn(async move {
            let mut timer_send_ping = Box::pin(tokio::time::sleep(PING_INTERVAL + PING_TIMEOUT));
            let mut timer_expect_pong = Box::pin(tokio::time::sleep(PING_TIMEOUT));

            let mut session = ServerSession::new(
                client_id,
                reply_ch_tx.clone(),
                auth_fn,
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
                            Message::Text(text) => text.into(),
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
