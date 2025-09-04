use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Display;
use std::future::IntoFuture;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::Future;
use slotmap::SlotMap;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::client_handler::ReplyError;
use crate::config::{Config, Protocol, ReconnectStrategy};
use crate::protocol::{
    Command, ConnectRequest, PublishRequest, PushData, Reply, RpcRequest, SubscribeRequest, UnsubscribeRequest
};
use crate::subscription::{Subscription, SubscriptionId, SubscriptionInner};
use crate::{errors, subscription};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Connecting,
    Connected,
}

pub(crate) struct MessageStoreItem {
    command: Command,
    reply: oneshot::Sender<Result<Reply, ReplyError>>,
    deadline: Instant,
}

impl MessageStoreItem {
    fn check_expiration(self, now: Instant) -> Option<Self> {
        if self.deadline > now {
            Some(self)
        } else {
            let _ = self.reply.send(Err(ReplyError::Timeout));
            None
        }
    }
}

pub(crate) struct MessageStore {
    timeout: Duration,
    activity: mpsc::Sender<()>,
    messages: VecDeque<MessageStoreItem>,
}

impl MessageStore {
    pub fn new(timeout: Duration) -> (Self, mpsc::Receiver<()>) {
        let (activity_tx, activity_rx) = mpsc::channel(1);
        let store = Self {
            timeout,
            activity: activity_tx,
            messages: VecDeque::new(),
        };
        (store, activity_rx)
    }

    pub fn send(&mut self, command: Command) -> oneshot::Receiver<Result<Reply, ReplyError>> {
        let (tx, rx) = oneshot::channel();
        let now = Instant::now();
        let deadline = now + self.timeout;
        self.messages.push_back(MessageStoreItem {
            command,
            reply: tx,
            deadline,
        });
        while let Some(item) = self.messages.pop_front() {
            if let Some(item) = item.check_expiration(now) {
                self.messages.push_front(item);
                break;
            }
        }
        let _ = self.activity.try_send(());
        rx
    }

    fn reset_channel(&mut self) -> mpsc::Receiver<()> {
        let (activity_tx, activity_rx) = mpsc::channel(1);
        self.activity = activity_tx;
        activity_rx
    }

    fn get_next(&mut self, time: Instant) -> Option<MessageStoreItem> {
        loop {
            let item = self.messages.pop_front()?;
            if let Some(item) = item.check_expiration(time) {
                return Some(item);
            }
        }
    }
}

pub(crate) struct ClientInner {
    rt: Handle,
    url: Arc<str>,
    state: State,
    token: String,
    name: String,
    version: String,
    protocol: Protocol,
    reconnect_strategy: Arc<dyn ReconnectStrategy>,
    pub(crate) read_timeout: Duration,
    closer_write: Option<mpsc::Sender<bool>>,
    on_connecting: Option<Box<dyn FnMut() + Send + 'static>>,
    on_connected: Option<Box<dyn FnMut() + Send + 'static>>,
    on_connected_ch: Vec<oneshot::Sender<Result<(), ()>>>,
    on_disconnected: Option<Box<dyn FnMut() + Send + 'static>>,
    on_disconnected_ch: Vec<oneshot::Sender<()>>,
    on_error: Option<Box<dyn FnMut(anyhow::Error) + Send + 'static>>,
    pub(crate) subscriptions: SlotMap<SubscriptionId, SubscriptionInner>,
    pub(crate) sub_name_to_id: HashMap<String, SubscriptionId>,
    pub(crate) pub_ch_write: Option<MessageStore>,
    pub(crate) sub_ch_write: Option<mpsc::UnboundedSender<SubscriptionId>>,
    active_tasks: usize,
}

impl ClientInner {
    // Disconnected, Connected -> Connecting
    //  - from Disconnected: `.connect()` called
    //  - from Connected: temporary connection loss / reconnect advice
    fn move_to_connecting(&mut self, outer: Arc<Mutex<Self>>) {
        debug_assert_ne!(self.state, State::Connecting);
        if self.pub_ch_write.is_none() {
            let (pub_ch_write, _) = MessageStore::new(self.read_timeout);
            self.pub_ch_write = Some(pub_ch_write);
        }
        self._set_state(State::Connecting);
        self.start_connecting(outer);
    }

    // Connecting -> Connected
    //  - from Connecting: successful connect
    fn move_to_connected(&mut self) {
        assert_eq!(self.state, State::Connecting);
        self._set_state(State::Connected);
    }

    // Connecting, Connected -> Disconnected
    //  - from Connecting: `.disconnect()` called or terminal condition met
    //  - from Connected: `.disconnect()` called or terminal condition met
    fn move_to_disconnected(&mut self) {
        assert_ne!(self.state, State::Disconnected);
        self.closer_write = None;
        self.pub_ch_write = None;
        self._set_state(State::Disconnected);
    }

    fn do_check_state(client: &Arc<Mutex<Self>>, expected: State) -> Result<(), bool> {
        let inner = client.lock().unwrap();
        if inner.state != expected {
            return Err(false);
        }
        Ok(())
    }

    // Calculate delay and wait before next reconnect attempt
    fn do_delay<'a>(
        client: &Arc<Mutex<Self>>,
        closer_read: &'a mut mpsc::Receiver<bool>,
        reconnect_attempts: u32,
    ) -> impl Future<Output = Result<(), bool>> + 'a {
        let delay = {
            let inner = client.lock().unwrap();
            if reconnect_attempts > 0 {
                inner.reconnect_strategy.time_before_next_attempt(reconnect_attempts)
            } else {
                Duration::ZERO
            }
        };

        async move {
            let task = async {
                if reconnect_attempts > 0 {
                    log::debug!("reconnecting attempt {}, delay={:?}", reconnect_attempts, delay);
                }
                tokio::time::sleep(delay).await;
                Ok(())
            };

            tokio::select! {
                biased;
                _ = closer_read.recv() => {
                    log::debug!("reconnect interrupted by user");
                    Err(false)
                }
                result = task => result
            }
        }
    }

    // Connect to websocket server
    fn do_connect<'a>(
        client: &Arc<Mutex<Self>>,
        closer_read: &'a mut mpsc::Receiver<bool>,
    ) -> impl Future<Output = Result<WebSocketStream<MaybeTlsStream<TcpStream>>, bool>> + 'a {
        let url = {
            let inner = client.lock().unwrap();
            inner.url.clone()
        };

        let client = client.clone();
        async move {
            let task = async {
                log::debug!("connecting to {}", &url);
                match tokio_tungstenite::connect_async(&*url).await {
                    Ok((stream, _)) => Ok(stream),
                    Err(err) => {
                        log::debug!("{err}");
                        let mut inner = client.lock().unwrap();
                        if inner.state != State::Connecting { return Err(false); }

                        let do_reconnect = match err {
                            tokio_tungstenite::tungstenite::Error::Url(_) => {
                                // invalid url, don't reconnect
                                false
                            }
                            _ => true
                        };

                        if let Some(ref mut on_error) = inner.on_error {
                            on_error(err.into());
                        }
                        Err(do_reconnect)
                    }
                }
            };

            tokio::select! {
                biased;
                _ = closer_read.recv() => {
                    log::debug!("reconnect interrupted by user");
                    Err(false)
                }
                result = task => result
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn do_handshake(
        client: &Arc<Mutex<Self>>,
        closer_write: mpsc::Sender<bool>,
        closer_read: mpsc::Receiver<bool>,
        stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> impl Future<Output = Result<(
        Pin<Box<dyn Future<Output = bool> + Send>>,
        mpsc::Sender<(Command, oneshot::Sender<Result<Reply, ReplyError>>, Duration)>,
    ), bool>> + '_ {
        let (control_write, control_read) = mpsc::channel(32);

        let (rt, connect_rx, protocol) = {
            let inner = client.lock().unwrap();
            //inner.control_write = Some(control_write);

            let (connect_tx, connect_rx) = oneshot::channel();

            let command = Command::Connect(ConnectRequest {
                token: inner.token.clone(),
                data: vec![],
                subs: std::collections::HashMap::new(),
                name: inner.name.clone(),
                version: inner.version.clone(),
            });
            let timeout = inner.read_timeout;

            control_write.try_send((command, connect_tx, timeout)).unwrap();

            let rt = inner.rt.clone();
            (rt, connect_rx, inner.protocol)
        };

        async move {
            //
            // Spawn tokio task to handle reads and writes to websocket
            //
            let client1 = client.clone();
            let client2 = client.clone();
            let mut handler_future: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(
                crate::client_handler::websocket_handler(rt, stream, control_read, closer_read, protocol, move |push| {
                    if let Reply::Push(push) = push {
                        if let PushData::Publication(publication) = push.data {
                            let mut inner = client1.lock().unwrap();
                            if let Some(sub_id) = inner.sub_name_to_id.get(&push.channel).copied() {
                                if let Some(sub) = inner.subscriptions.get_mut(sub_id) {
                                    if sub.state == subscription::State::Subscribed {
                                        if let Some(ref mut on_publication) = sub.on_publication {
                                            on_publication(publication);
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        log::debug!("unexpected push: {:?}", push);
                    }
                }, move |err| {
                    let mut inner = client2.lock().unwrap();
                    if let Some(ref mut on_error) = inner.on_error {
                        on_error(err);
                    }
                })
            );

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
                                Reply::Connect(connect) => {
                                    // handshake completed
                                    log::debug!("connection established with {} {}", connect.client, connect.version);
                                    Ok((handler_future, control_write))
                                }
                                Reply::Error(err) => {
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

    // this function does the following
    //  - connects to the server (Connecting state)
    //  - reconnects to the server as many times as it takes
    //  - processes connection (Connected state)
    //  - terminates, returning whether it should reconnect
    async fn do_connection_cycle(client: Arc<Mutex<Self>>) {
        // okay, this is bloody complicated, because there're many loops here
        //  1. inner loop is read-write into websocket
        //  2. on top of that, there's reconnect loop (which tries to connect once)
        //  3. on top of that, there's a connection cycle (which connects until success,
        //     resulting in one valid connection each time)
        //  4. on top of that, there's an outer loop (which reconnects if already
        //     previously established connection is lost)
        // -----

        // this channel may buffer publishes while connection is being established
        let client1 = client.clone();
        let need_reconnect = async move {
            let mut reconnect_attempt = 0;
            let (future, control_write) = loop {
                let (closer_write, mut closer_read) = {
                    let mut inner = client.lock().unwrap();
                    let (closer_write, closer_read) = mpsc::channel::<bool>(1);
                    inner.closer_write = Some(closer_write.clone());
                    (closer_write, closer_read)
                };

                let result: Result<_, bool> = async {
                    reconnect_attempt += 1;
                    Self::do_check_state(&client, State::Connecting)?;
                    Self::do_delay(&client, &mut closer_read, reconnect_attempt - 1).await?;

                    Self::do_check_state(&client, State::Connecting)?;
                    let stream = Self::do_connect(&client, &mut closer_read).await?;

                    Self::do_check_state(&client, State::Connecting)?;
                    let result = Self::do_handshake(&client, closer_write, closer_read, stream).await?;
                    Ok(result)
                }.await;

                let mut inner = client.lock().unwrap();
                if inner.state != State::Connecting { return false; }
                for ch in inner.on_connected_ch.drain(..) {
                    let _ = ch.send(match result.as_ref() {
                        Ok(_) => Ok(()),
                        Err(_) => Err(()),
                    });
                }

                match result {
                    Ok(any) => {
                        // connection successful
                        break any;
                    }
                    Err(true) => {
                        // reconnect
                        continue;
                    }
                    Err(false) => {
                        // interrupted
                        return false;
                    }
                }
            };

            let (sub_ch_write, mut sub_ch_read) = mpsc::unbounded_channel();
            let rt = {
                let mut inner = client.lock().unwrap();
                inner.move_to_connected();
                for (sub_id, sub) in inner.subscriptions.iter() {
                    if sub.state != subscription::State::Unsubscribed {
                        let _ = sub_ch_write.send(sub_id);
                    }
                }
                inner.sub_ch_write = Some(sub_ch_write);
                inner.rt.clone()
            };

            let mut subscribed_channels: HashSet<SubscriptionId> = HashSet::new();
            let mut join_set = JoinSet::new();

            #[allow(clippy::type_complexity)]
            enum JoinSetResult {
                MainTask(bool),
                NewTask((
                    Option<Pin<Box<dyn Future<Output = JoinSetResult> + Send>>>,
                    mpsc::Receiver<Pin<Box<dyn Future<Output = JoinSetResult> + Send>>>,
                )),
                Irrelevant,
            }

            join_set.spawn_on(async {
                JoinSetResult::MainTask(future.await)
            }, &rt);

            let (new_tasks_tx, new_tasks_rx) = mpsc::channel(1);

            join_set.spawn_on(async move {
                JoinSetResult::NewTask((None, new_tasks_rx))
            }, &rt);

            #[allow(clippy::type_complexity)]
            async fn publish_task(
                client: Arc<Mutex<ClientInner>>,
                control_write: mpsc::Sender<(Command, oneshot::Sender<Result<Reply, ReplyError>>, Duration)>,
                get_store: impl Fn(&mut ClientInner) -> Option<&mut MessageStore>,
            ) -> JoinSetResult {
                let mut pub_ch_read = {
                    let mut inner = client.lock().unwrap();
                    let Some(store) = get_store(&mut inner) else { return JoinSetResult::Irrelevant; };
                    store.reset_channel()
                };

                const MAX_CAPACITY: usize = 32;
                let mut buffer = Vec::new();
                loop {
                    {
                        // lock mutex and fill our buffer
                        let mut inner = client.lock().unwrap();
                        let Some(pub_ch_write) = get_store(&mut inner) else { break; };
                        let now = Instant::now();
                        for _ in 0..MAX_CAPACITY {
                            if let Some(item) = pub_ch_write.get_next(now) {
                                buffer.push(item);
                            } else {
                                break;
                            }
                        }
                    }
                    if buffer.is_empty() {
                        // wait for activity
                        let Some(()) = pub_ch_read.recv().await else { break; };
                    } else {
                        // send messages
                        for item in buffer.drain(..) {
                            let timeout = item.deadline.saturating_duration_since(Instant::now());
                            let _ = control_write.send((item.command, item.reply, timeout)).await;
                        }
                    }
                }

                JoinSetResult::Irrelevant
            }

            join_set.spawn_on(publish_task(client.clone(), control_write.clone(), |inner| {
                inner.pub_ch_write.as_mut()
            }), &rt);

            let control_write1 = control_write;
            join_set.spawn_on(async move {
                loop {
                    let mut buf = Vec::new();
                    let count = sub_ch_read.recv_many(&mut buf, 32).await;
                    if count == 0 {
                        break;
                    }

                    let mut inner = client.lock().unwrap();
                    let rt = inner.rt.clone();
                    let timeout = inner.read_timeout;

                    for sub_id in buf.drain(..count) {
                        let Some(sub) = inner.subscriptions.get_mut(sub_id) else { continue; };

                        if sub.state != subscription::State::Unsubscribed && !subscribed_channels.contains(&sub_id) {
                            // subscribe
                            let (tx, rx) = oneshot::channel();
                            let command = Command::Subscribe(SubscribeRequest {
                                channel: (*sub.channel).into(),
                                ..Default::default()
                            });
                            let control_write = control_write1.clone();
                            subscribed_channels.insert(sub_id);
                            let client = client.clone();
                            let new_tasks_tx = new_tasks_tx.clone();
                            rt.spawn(async move {
                                {
                                    let _ = control_write.send((command, tx, timeout)).await;
                                    let result = rx.await;
                                    let mut inner = client.lock().unwrap();
                                    if let Some(sub) = inner.subscriptions.get_mut(sub_id) {
                                        let msg = if let Ok(Ok(Reply::Subscribe(sub_result))) = result {
                                            sub.move_to_subscribed(sub_result.data);
                                            Ok(())
                                        } else {
                                            let (code, reason) = match result {
                                                Ok(Ok(Reply::Error(err))) => (err.code, err.message.into()),
                                                Ok(Ok(_)) => (0, "unexpected reply".into()),
                                                Ok(Err(err)) => (0, err.to_string().into()),
                                                Err(err) => (0, err.to_string().into()),
                                            };
                                            sub.move_to_unsubscribed(code, reason);
                                            Err(())
                                        };
                                        for ch in sub.on_subscribed_ch.drain(..) {
                                            let _ = ch.send(msg);
                                        }
                                    }
                                }

                                let _ = new_tasks_tx.send(Box::pin(publish_task(
                                    client.clone(),
                                    control_write.clone(),
                                    move |inner| {
                                        inner.subscriptions.get_mut(sub_id).and_then(|sub| sub.pub_ch_write.as_mut())
                                    }
                                ))).await;
                            });
                        }

                        if sub.state == subscription::State::Unsubscribed && subscribed_channels.contains(&sub_id) {
                            // unsubscribe
                            let (tx, rx) = oneshot::channel();
                            let command = Command::Unsubscribe(UnsubscribeRequest {
                                channel: (*sub.channel).into(),
                            });
                            let control_write = control_write1.clone();
                            subscribed_channels.remove(&sub_id);
                            let client = client.clone();
                            rt.spawn(async move {
                                let _ = control_write.send((command, tx, timeout)).await;
                                let _ = rx.await;
                                let mut inner = client.lock().unwrap();
                                if let Some(sub) = inner.subscriptions.get_mut(sub_id) {
                                    for ch in sub.on_unsubscribed_ch.drain(..) {
                                        let _ = ch.send(());
                                    }
                                }
                            });
                        }
                    }
                }

                JoinSetResult::Irrelevant
            }, &rt);

            while let Some(result) = join_set.join_next().await {
                match result {
                    Ok(JoinSetResult::MainTask(need_reconnect)) => {
                        return need_reconnect;
                    }
                    Ok(JoinSetResult::NewTask((task, mut new_tasks_rx))) => {
                        join_set.spawn_on(async move {
                            if let Some(task) = new_tasks_rx.recv().await {
                                JoinSetResult::NewTask((Some(task), new_tasks_rx))
                            } else {
                                JoinSetResult::Irrelevant
                            }
                        }, &rt);
                        if let Some(task) = task {
                            join_set.spawn_on(task, &rt);
                        }
                    }
                    Ok(JoinSetResult::Irrelevant) => {}
                    Err(err) => {
                        log::debug!("task failed: {:?}", err);
                        return false;
                    }
                }
            }

            // there's always a main task, so this should never happen
            unreachable!()
        }.await;

        {
            // this should only happen when state=Connected, but connection is lost
            let mut inner = client1.lock().unwrap();
            if need_reconnect {
                if inner.state == State::Connected {
                    inner.move_to_connecting(client1.clone());
                }
            } else {
                if inner.state == State::Connected {
                    inner.move_to_disconnected();
                }
                for ch in inner.on_disconnected_ch.drain(..) {
                    let _ = ch.send(());
                }
            }
        }
    }

    fn start_connecting(&mut self, client: Arc<Mutex<Self>>) {
        self.active_tasks += 1;

        self.rt.spawn(async move {
            Self::do_connection_cycle(client.clone()).await;
            let mut inner = client.lock().unwrap();
            inner.active_tasks -= 1;
        });
    }

    fn _set_state(&mut self, state: State) {
        log::debug!("state: {:?} -> {:?}", self.state, state);
        self.state = state;

        match state {
            State::Disconnected => {
                if let Some(ref mut on_disconnected) = self.on_disconnected {
                    on_disconnected();
                }
            }
            State::Connecting => {
                if let Some(ref mut on_connecting) = self.on_connecting {
                    on_connecting();
                }
            }
            State::Connected => {
                if let Some(ref mut on_connected) = self.on_connected {
                    on_connected();
                }
            }
        }
    }
}

// this is a regular future which you can poll to get result,
// but it's totally fine to drop it if you don't need results
pub struct FutureResult<T>(pub(crate) T);

impl<T, R> IntoFuture for FutureResult<T>
where
    T: Future<Output = R>,
{
    type Output = R;
    type IntoFuture = T;

    fn into_future(self) -> Self::IntoFuture {
        self.0
    }
}

#[derive(Error, Debug)]
pub enum RequestError {
    ErrorResponse(crate::protocol::Error),
    UnexpectedReply(crate::protocol::Reply),
    ReplyError(#[from] crate::client_handler::ReplyError),
    Timeout(#[from] tokio::time::error::Elapsed),
    Cancelled(#[from] tokio::sync::oneshot::error::RecvError),
}

impl Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::ErrorResponse(err) => write!(f, "{} {}", err.code, err.message),
            RequestError::UnexpectedReply(_) => write!(f, "unexpected reply"),
            RequestError::ReplyError(err) => write!(f, "{}", err),
            RequestError::Timeout(err) => write!(f, "{}", err),
            RequestError::Cancelled(_) => write!(f, "operation cancelled"),
        }
    }
}

#[derive(Clone)]
pub struct Client(pub(crate) Arc<Mutex<ClientInner>>);

impl Client {
    pub fn new(url: &str, config: Config) -> Self {
        let rt = config.runtime.unwrap_or_else(|| tokio::runtime::Handle::current());

        Self(Arc::new(Mutex::new(ClientInner {
            rt,
            url: url.into(),
            state: State::Disconnected,
            token: config.token,
            name: config.name,
            version: config.version,
            protocol: config.protocol,
            reconnect_strategy: config.reconnect_strategy,
            read_timeout: config.read_timeout,
            closer_write: None,
            on_connecting: None,
            on_connected: None,
            on_connected_ch: Vec::new(),
            on_disconnected: None,
            on_disconnected_ch: Vec::new(),
            on_error: None,
            subscriptions: SlotMap::with_key(),
            sub_name_to_id: HashMap::new(),
            sub_ch_write: None,
            pub_ch_write: None,
            active_tasks: 0,
        })))
    }

    // future resolves when connection is established, with few caveats:
    //  - if connection is already established, future resolves immediately
    //  - if connection is never established, future never resolves
    pub fn connect(&self) -> FutureResult<impl Future<Output = Result<(), ()>>> {
        let (tx, rx) = oneshot::channel();
        let mut inner = self.0.lock().unwrap();
        if inner.state == State::Disconnected {
            inner.on_connected_ch.push(tx);
            inner.move_to_connecting(self.0.clone());
        } else {
            let _ = tx.send(Ok(()));
        }
        FutureResult(async {
            match rx.await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(())) => Err(()),
                Err(_) => Err(()),
            }
        })
    }

    pub fn disconnect(&self) -> FutureResult<impl Future<Output = ()>> {
        let (tx, rx) = oneshot::channel();
        let mut inner = self.0.lock().unwrap();
        if inner.state != State::Disconnected {
            inner.on_disconnected_ch.push(tx);
            inner.move_to_disconnected();
        } else {
            let _ = tx.send(());
        }
        FutureResult(async {
            let _ = rx.await;
        })
    }

    pub fn publish(
        &self,
        channel: &str,
        data: Vec<u8>,
    ) -> FutureResult<impl Future<Output = Result<(), RequestError>>> {
        let mut inner = self.0.lock().unwrap();
        let read_timeout = inner.read_timeout;
        let deadline = Instant::now() + read_timeout;
        let rx = if let Some(ref mut pub_ch_write) = inner.pub_ch_write {
            pub_ch_write.send(Command::Publish(PublishRequest {
                channel: channel.into(),
                data,
            }))
        } else {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Err(ReplyError::Closed));
            rx
        };
        FutureResult(async move {
            let result = tokio::time::timeout_at(deadline.into(), rx).await;
            match result {
                Ok(Ok(Ok(Reply::Publish(_)))) => {
                    Ok(())
                }
                Ok(Ok(Ok(Reply::Error(err)))) => {
                    Err(RequestError::ErrorResponse(err))
                }
                Ok(Ok(Ok(reply))) => {
                    Err(RequestError::UnexpectedReply(reply))
                }
                Ok(Ok(Err(err))) => Err(err.into()),
                Ok(Err(err)) => Err(err.into()),
                Err(err) => Err(err.into()),
            }
        })
    }

    pub fn rpc(
        &self,
        method: &str,
        data: Vec<u8>,
    ) -> FutureResult<impl Future<Output = Result<Vec<u8>, RequestError>>> {
        let mut inner = self.0.lock().unwrap();
        let read_timeout = inner.read_timeout;
        let deadline = Instant::now() + read_timeout;
        let rx = if let Some(ref mut pub_ch_write) = inner.pub_ch_write {
            pub_ch_write.send(Command::Rpc(RpcRequest {
                method: method.into(),
                data,
            }))
        } else {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Err(ReplyError::Closed));
            rx
        };
        FutureResult(async move {
            let result = tokio::time::timeout_at(deadline.into(), rx).await;
            match result {
                Ok(Ok(Ok(Reply::Rpc(rpc_result)))) => {
                    Ok(rpc_result.data)
                }
                Ok(Ok(Ok(Reply::Error(err)))) => {
                    Err(RequestError::ErrorResponse(err))
                }
                Ok(Ok(Ok(reply))) => {
                    Err(RequestError::UnexpectedReply(reply))
                }
                Ok(Ok(Err(err))) => Err(err.into()),
                Ok(Err(err)) => Err(err.into()),
                Err(err) => Err(err.into()),
            }
        })
    }

    pub fn new_subscription(&self, channel: &str) -> Subscription {
        let mut inner = self.0.lock().unwrap();
        if let Some(key) = inner.sub_name_to_id.get(channel) {
            return Subscription::new(self, *key);
        }

        let timeout = inner.read_timeout;
        let key = inner.subscriptions.insert(SubscriptionInner::new(channel, timeout));
        inner.sub_name_to_id.insert(channel.to_string(), key);
        Subscription::new(self, key)
    }

    pub fn get_subscription(&self, channel: &str) -> Option<Subscription> {
        let inner = self.0.lock().unwrap();
        inner.sub_name_to_id.get(channel).map(|id| Subscription::new(self, *id))
    }

    pub fn remove_subscription(&self, subscription: Subscription) -> Result<(), errors::RemoveSubscriptionError> {
        let mut inner = self.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get(subscription.id) {
            if sub.state != subscription::State::Unsubscribed {
                Err(errors::RemoveSubscriptionError::NotUnsubscribed)
            } else {
                let sub = inner.subscriptions.remove(subscription.id).unwrap();
                inner.sub_name_to_id.remove(&*sub.channel);
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    pub fn on_connecting(&self, func: impl FnMut() + Send + 'static) {
        self.0.lock().unwrap().on_connecting = Some(Box::new(func));
    }

    pub fn on_connected(&self, func: impl FnMut() + Send + 'static) {
        self.0.lock().unwrap().on_connected = Some(Box::new(func));
    }

    pub fn on_disconnected(&self, func: impl FnMut() + Send + 'static) {
        self.0.lock().unwrap().on_disconnected = Some(Box::new(func));
    }

    pub fn on_error(&self, func: impl FnMut(anyhow::Error) + Send + 'static) {
        self.0.lock().unwrap().on_error = Some(Box::new(func));
    }

    pub fn state(&self) -> State {
        self.0.lock().unwrap().state
    }

    pub fn set_token(&self, token: impl Into<String>) {
        self.0.lock().unwrap().token = token.into();
    }
}
