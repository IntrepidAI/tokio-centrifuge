use std::collections::HashMap;
use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use matchit::{InsertError, Router};
use tokio::sync::Semaphore;
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::config::Protocol;
use crate::errors::{ClientError, ClientErrorCode, DisconnectErrorCode};
use crate::protocol::{
    Command, ConnectResult, Publication, PublishResult, Push, PushData, RawCommand, RawReply,
    Reply, RpcResult, SubscribeResult, Unsubscribe, UnsubscribeResult,
};
use crate::utils::{decode_frames, encode_frames};

#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ClientId(uuid::Uuid);

impl ClientId {
    pub fn new() -> Self {
        Default::default()
    }
}

impl Default for ClientId {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ServeParams {
    pub client_id: Option<ClientId>,
    pub encoding: Protocol,
    pub ping_interval: Option<Duration>,
    pub pong_timeout: Option<Duration>,
}

impl ServeParams {
    pub fn json() -> Self {
        Self {
            encoding: Protocol::Json,
            ..Default::default()
        }
    }

    pub fn protobuf() -> Self {
        Self {
            encoding: Protocol::Protobuf,
            ..Default::default()
        }
    }

    pub fn with_client_id(self, client_id: ClientId) -> Self {
        Self {
            client_id: Some(client_id),
            ..self
        }
    }

    pub fn without_ping(self) -> Self {
        Self {
            ping_interval: None,
            pong_timeout: None,
            ..self
        }
    }
}

impl Default for ServeParams {
    fn default() -> Self {
        Self {
            client_id: None,
            encoding: Protocol::Json,
            ping_interval: Some(Duration::from_secs(25)),
            pong_timeout: Some(Duration::from_secs(10)),
        }
    }
}

impl From<Protocol> for ServeParams {
    fn from(protocol: Protocol) -> Self {
        Self {
            encoding: protocol,
            ..Default::default()
        }
    }
}

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
    params: ServeParams,
    state: SessionState,
    on_connect: OnConnectFn,
    on_disconnect: Option<OnDisconnectFn>,
    rpc_semaphore: Arc<Semaphore>,
    rpc_methods: Arc<Mutex<Router<RpcMethodFn>>>,
    sub_channels: Arc<Mutex<Router<ChannelFn>>>,
    pub_channels: Arc<Mutex<Router<PublishFn>>>,
    subscriptions: Arc<Mutex<HashMap<String, Subscription>>>,
    tasks: JoinSet<()>,
    reply_ch: tokio::sync::mpsc::Sender<Result<(u32, Reply), DisconnectErrorCode>>,
}

impl ServerSession {
    #[allow(clippy::too_many_arguments)]
    fn new(
        params: ServeParams,
        reply_ch: tokio::sync::mpsc::Sender<Result<(u32, Reply), DisconnectErrorCode>>,
        on_connect: OnConnectFn,
        on_disconnect: Option<OnDisconnectFn>,
        rpc_methods: Arc<Mutex<Router<RpcMethodFn>>>,
        sub_channels: Arc<Mutex<Router<ChannelFn>>>,
        pub_channels: Arc<Mutex<Router<PublishFn>>>,
    ) -> Self {
        const MAX_CONCURRENT_REQUESTS: usize = 10;

        Self {
            params,
            on_connect,
            on_disconnect,
            state: SessionState::Initial,
            rpc_semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
            rpc_methods,
            sub_channels,
            pub_channels,
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
                            client_id: self.params.client_id.unwrap(),
                            client_name: connect.name,
                            client_version: connect.version,
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
                            client: self.params.client_id.unwrap().to_string(),
                            ping: self.params.ping_interval.unwrap_or(Duration::ZERO).as_secs() as u32,
                            pong: self.params.pong_timeout.is_some(),
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
                        let client_id = self.params.client_id.unwrap();
                        let encoding = self.params.encoding;
                        self.tasks.spawn(async move {
                            let (match_params, f) = match_;
                            let result = f(RpcContext {
                                encoding,
                                client_id,
                                params: match_params,
                            }, rpc_request.data).await;
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
                    Command::Publish(pub_request) => {
                        let permit = self.rpc_semaphore.clone().acquire_owned().await;

                        let Some(match_) = ({
                            let methods = self.pub_channels.lock().unwrap();
                            match methods.at(&pub_request.channel) {
                                Ok(match_) => {
                                    let params: HashMap<String, String> = match_.params.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
                                    Some((params, match_.value.clone()))
                                }
                                Err(_err) => {
                                    None
                                }
                            }
                        }) else {
                            let _ = self.reply_ch.send(Ok((id, Reply::Error(ClientErrorCode::UnknownChannel.into())))).await;
                            return;
                        };

                        let reply_ch = self.reply_ch.clone();
                        let client_id = self.params.client_id.unwrap();
                        let encoding = self.params.encoding;
                        self.tasks.spawn(async move {
                            let (match_params, f) = match_;
                            let result = f(PublishContext {
                                encoding,
                                client_id,
                                params: match_params,
                            }, pub_request.data).await;
                            match result {
                                Ok(()) => {
                                    let _ = reply_ch.send(Ok((id, Reply::Publish(PublishResult {})))).await;
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
                            let client_id = self.params.client_id.unwrap();
                            let encoding = self.params.encoding;
                            let subscriptions_ = self.subscriptions.clone();

                            let abort_handle = self.tasks.spawn(async move {
                                let (match_params, f) = match_;
                                let mut stream = match f(SubContext {
                                    encoding,
                                    client_id,
                                    params: match_params,
                                    data: sub_request.data,
                                }).await {
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
                            subscriptions.insert(sub_request.channel, Subscription {
                                abort_handle,
                            });
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

impl Drop for ServerSession {
    fn drop(&mut self) {
        if self.state == SessionState::Connected {
            if let Some(on_disconnect) = &self.on_disconnect {
                on_disconnect(DisconnectContext { client_id: self.params.client_id.unwrap() });
            }
        }
    }
}

#[derive(Debug)]
pub struct ConnectContext {
    pub client_id: ClientId,
    pub client_name: String,
    pub client_version: String,
    pub token: String,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct DisconnectContext {
    pub client_id: ClientId,
}

#[derive(Debug)]
pub struct RpcContext {
    pub encoding: Protocol,
    pub client_id: ClientId,
    pub params: HashMap<String, String>,
}

#[derive(Debug)]
pub struct SubContext {
    pub encoding: Protocol,
    pub client_id: ClientId,
    pub params: HashMap<String, String>,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct PublishContext {
    pub encoding: Protocol,
    pub client_id: ClientId,
    pub params: HashMap<String, String>,
}

type OnConnectFn = Arc<
    dyn Fn(ConnectContext) ->
        Pin<Box<dyn Future<Output = Result<(), DisconnectErrorCode>> + Send>>
    + Send + Sync
>;

type PublishFn = Arc<
    dyn Fn(PublishContext, Vec<u8>) ->
        Pin<Box<dyn Future<Output = Result<(), ClientError>> + Send>>
    + Send + Sync
>;

type OnDisconnectFn = Arc<
    dyn Fn(DisconnectContext) + Send + Sync
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
    on_disconnect: Option<OnDisconnectFn>,
    rpc_methods: Arc<Mutex<Router<RpcMethodFn>>>,
    sub_channels: Arc<Mutex<Router<ChannelFn>>>,
    pub_channels: Arc<Mutex<Router<PublishFn>>>,
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

    pub fn on_disconnect(&mut self, f: impl Fn(DisconnectContext) + Send + Sync + 'static) {
        self.on_disconnect = Some(Arc::new(f));
    }

    pub fn add_rpc_method<Fut>(
        &self,
        name: &str,
        f: impl Fn(RpcContext, Vec<u8>) -> Fut + Send + Sync + 'static,
    ) -> Result<(), InsertError>
    where
        Fut: std::future::Future<Output = Result<Vec<u8>, ClientError>> + Send + 'static,
    {
        let wrap_f: RpcMethodFn = Arc::new(move |ctx: RpcContext, data: Vec<u8>| {
            Box::pin(f(ctx, data))
        });
        self.rpc_methods.lock().unwrap().insert(name.to_string(), wrap_f)
    }

    pub fn on_publication<Fut>(
        &self,
        name: &str,
        f: impl Fn(PublishContext, Vec<u8>) -> Fut + Send + Sync + 'static,
    ) -> Result<(), InsertError>
    where
        Fut: std::future::Future<Output = Result<(), ClientError>> + Send + 'static,
    {
        let wrap_f: PublishFn = Arc::new(move |ctx: PublishContext, data: Vec<u8>| {
            Box::pin(f(ctx, data))
        });
        self.pub_channels.lock().unwrap().insert(name.to_string(), wrap_f)
    }

    pub fn add_channel<Fut, St>(
        &self,
        name: &str,
        f: impl Fn(SubContext) -> Fut + Send + Sync + 'static,
    ) -> Result<(), InsertError>
    where
        Fut: std::future::Future<Output = Result<St, ClientError>> + Send + 'static,
        St: Stream<Item = Vec<u8>> + Send + 'static,
    {
        let f = Arc::new(f);
        let wrap_f: ChannelFn = Arc::new(move |ctx: SubContext| {
            let f = f.clone();
            Box::pin(async move {
                Ok(Box::pin(f(ctx).await?) as Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>)
            })
        });
        self.sub_channels.lock().unwrap().insert(name.to_string(), wrap_f)
    }

    pub async fn serve<S>(&self, stream: S, mut params: ServeParams)
    where
        S: Stream<Item = Result<Message, WsError>> + Sink<Message> + Send + Unpin + 'static,
    {
        params.client_id = Some(params.client_id.unwrap_or_default());

        let (mut write_ws, mut read_ws) = stream.split();
        let (closer_tx, mut closer_rx) = tokio::sync::mpsc::channel::<()>(1);

        let mut ping_timer = tokio::time::interval(params.ping_interval.unwrap_or(Duration::MAX));
        ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let connect_fn = self.on_connect.0.clone();
        let disconnect_fn = self.on_disconnect.clone();
        let rpc_methods = self.rpc_methods.clone();
        let sub_channels = self.sub_channels.clone();
        let pub_channels = self.pub_channels.clone();
        let (reply_ch_tx, mut reply_ch_rx) = tokio::sync::mpsc::channel(64);

        let reader_task = tokio::spawn(async move {
            let mut timer_send_ping = Box::pin(tokio::time::sleep(Duration::MAX));
            let mut timer_expect_pong = Box::pin(tokio::time::sleep(Duration::MAX));

            let mut session = ServerSession::new(
                params,
                reply_ch_tx.clone(),
                connect_fn,
                disconnect_fn,
                rpc_methods,
                sub_channels,
                pub_channels,
            );

            let error_code = 'outer: loop {
                tokio::select! {
                    biased;

                    _ = closer_rx.recv() => {
                        break 'outer DisconnectErrorCode::ConnectionClosed;
                    }

                    _ = &mut timer_send_ping => {
                        if session.state() == SessionState::Connected {
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

                        let _ = decode_frames(&data, params.encoding, |result| {
                            let result: RawCommand = result?;
                            let frame_id = result.id;
                            let frame: Command = result.into();
                            commands.push((frame_id, frame));
                            Ok(())
                        });

                        for (frame_id, frame) in commands {
                            if session.state() == SessionState::Initial {
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

                let data = encode_frames(&replies, params.encoding, |_id| {
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

// see tokio Instant impl
fn far_future() -> Instant {
    // Roughly 30 years from now.
    // API does not provide a way to obtain max `Instant`
    // or convert specific date in the future to instant.
    // 1000 years overflows on macOS, 100 years overflows on FreeBSD.
    Instant::now() + Duration::from_secs(86400 * 365 * 30)
}
