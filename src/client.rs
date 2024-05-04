use std::future::IntoFuture;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_tungstenite::tokio::ConnectStream;
use async_tungstenite::WebSocketStream;
use futures::Future;
use serde::Serialize;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};

use crate::client_handler::ReplyError;
use crate::config::{Config, ReconnectStrategy};
use crate::protocol::{Command, ConnectRequest, PublishRequest, Reply};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Connecting,
    Connected,
}

type PublishMessage = (String, Vec<u8>, oneshot::Sender<Result<Reply, ReplyError>>, Instant);

#[allow(clippy::type_complexity)]
struct CentrifugeInner {
    rt: Handle,
    url: Arc<str>,
    state: State,
    token: String,
    name: String,
    version: String,
    reconnect_strategy: Arc<dyn ReconnectStrategy>,
    read_timeout: Duration,
    closer_write: Option<mpsc::Sender<bool>>,
    on_connecting: Option<Box<dyn FnMut() + Send + 'static>>,
    on_connected: Option<Box<dyn FnMut() + Send + 'static>>,
    on_connected_ch: Vec<oneshot::Sender<Result<(), ()>>>,
    on_disconnected: Option<Box<dyn FnMut() + Send + 'static>>,
    on_disconnected_ch: Vec<oneshot::Sender<()>>,
    on_error: Option<Box<dyn FnMut(anyhow::Error) + Send + 'static>>,
    push_ch_write: Option<mpsc::UnboundedSender<PublishMessage>>,
    active_tasks: usize,
}

impl CentrifugeInner {
    // Disconnected, Connected -> Connecting
    //  - from Disconnected: `.connect()` called
    //  - from Connected: temporary connection loss / reconnect advice
    fn move_to_connecting(&mut self, outer: Arc<Mutex<Self>>) {
        debug_assert_ne!(self.state, State::Connecting);
        self.set_state(State::Connecting);

        let (push_ch_write, push_ch_read) = mpsc::unbounded_channel();
        self.push_ch_write = Some(push_ch_write);
        self.start_connecting(outer, push_ch_read);
    }

    // Connecting -> Connected
    //  - from Connecting: successful connect
    fn move_to_connected(&mut self) {
        assert_eq!(self.state, State::Connecting);
        self.set_state(State::Connected);
    }

    // Connecting, Connected -> Disconnected
    //  - from Connecting: `.disconnect()` called or terminal condition met
    //  - from Connected: `.disconnect()` called or terminal condition met
    fn move_to_disconnected(&mut self) {
        assert_ne!(self.state, State::Disconnected);
        self.set_state(State::Disconnected);
        self.closer_write = None;
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
    ) -> impl Future<Output = Result<WebSocketStream<ConnectStream>, bool>> + 'a {
        let url = {
            let inner = client.lock().unwrap();
            inner.url.clone()
        };

        let client = client.clone();
        async move {
            let task = async {
                log::debug!("connecting to {}", &url);
                match async_tungstenite::tokio::connect_async(&*url).await {
                    Ok((stream, _)) => Ok(stream),
                    Err(err) => {
                        log::debug!("{err}");
                        let mut inner = client.lock().unwrap();
                        if inner.state != State::Connecting { return Err(false); }
                        if let Some(ref mut on_error) = inner.on_error {
                            on_error(err.into());
                        }
                        Err(true)
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

    fn do_handshake(
        client: &Arc<Mutex<Self>>,
        closer_write: mpsc::Sender<bool>,
        closer_read: mpsc::Receiver<bool>,
        stream: WebSocketStream<ConnectStream>,
    ) -> impl Future<Output = Result<(
        Pin<Box<dyn Future<Output = bool> + Send>>,
        mpsc::Sender<(Command, oneshot::Sender<Result<Reply, ReplyError>>, Duration)>,
    ), bool>> + '_ {
        let (control_write, control_read) = mpsc::channel(32);

        let (rt, connect_rx) = {
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
            (rt, connect_rx)
        };

        async move {
            //
            // Spawn tokio task to handle reads and writes to websocket
            //
            let client1 = client.clone();
            let mut handler_future: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(crate::client_handler::websocket_handler(rt, stream, control_read, closer_read, |_push| {
                // dbg!(push);
            }, move |err| {
                let mut inner = client1.lock().unwrap();
                if let Some(ref mut on_error) = inner.on_error {
                    on_error(err);
                }
            }));

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
                            if let Reply::Connect(connect) = reply {
                                // handshake completed
                                log::debug!("connection established with {} {}", connect.client, connect.version);
                                Ok((handler_future, control_write))
                            } else {
                                log::debug!("unexpected reply: {:?}", reply);
                                // close connection, don't reconnect
                                let _ = closer_write.try_send(false);
                                Err(handler_future.await)
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
    async fn do_connection_cycle(
        client: Arc<Mutex<Self>>,
        mut push_ch_read: mpsc::UnboundedReceiver<PublishMessage>,
    ) {
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
            let (mut future, control_write) = loop {
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

            {
                let mut inner = client.lock().unwrap();
                inner.move_to_connected();
            }

            loop {
                let task = async {
                    let (channel, data, reply_ch, deadline) = push_ch_read.recv().await?;
                    let timeout = deadline.saturating_duration_since(Instant::now());
                    let _ = control_write.send((Command::Publish(PublishRequest {
                        channel,
                        data,
                    }), reply_ch, timeout)).await;
                    Some(())
                };

                tokio::select! {
                    biased;
                    need_reconnect = &mut future => {
                        return need_reconnect;
                    }
                    result = task => {
                        if result.is_none() {
                            break;
                        }
                    }
                }
            }
            future.await
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

    fn start_connecting(&mut self, client: Arc<Mutex<Self>>, push_ch_read: mpsc::UnboundedReceiver<PublishMessage>) {
        self.active_tasks += 1;

        self.rt.spawn(async move {
            Self::do_connection_cycle(client.clone(), push_ch_read).await;
            let mut inner = client.lock().unwrap();
            inner.active_tasks -= 1;
        });
    }

    fn set_state(&mut self, state: State) {
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

pub struct FutureResult<T>(T);

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

pub struct Centrifuge(Arc<Mutex<CentrifugeInner>>);

impl Centrifuge {
    pub fn new(url: &str, config: Config) -> Self {
        let rt = config.runtime.unwrap_or_else(|| tokio::runtime::Handle::current());

        Self(Arc::new(Mutex::new(CentrifugeInner {
            rt,
            url: url.into(),
            state: State::Disconnected,
            token: config.token,
            name: config.name,
            version: config.version,
            reconnect_strategy: config.reconnect_strategy,
            read_timeout: config.read_timeout,
            closer_write: None,
            on_connecting: None,
            on_connected: None,
            on_connected_ch: Vec::new(),
            on_disconnected: None,
            on_disconnected_ch: Vec::new(),
            on_error: None,
            push_ch_write: None,
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

    pub fn publish<T: ?Sized + Serialize>(
        &self,
        channel: &str,
        data: &T,
    ) -> FutureResult<impl Future<Output = Result<(), ()>>> {
        let (tx, rx) = oneshot::channel();
        let inner = self.0.lock().unwrap();
        if let Some(ref push_ch_write) = inner.push_ch_write {
            let _ = push_ch_write.send(
                (channel.to_string(), serde_json::to_vec(data).unwrap(), tx, Instant::now() + inner.read_timeout)
            );
        } else {
            let _ = tx.send(Err(ReplyError::Closed));
        }
        FutureResult(async {
            match rx.await {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(_)) => Err(()),
                Err(_) => Err(()),
            }
        })
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
}
