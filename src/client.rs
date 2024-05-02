use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_tungstenite::tokio::ConnectStream;
use async_tungstenite::WebSocketStream;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};

use crate::client_handler::ReplyError;
use crate::config::{Config, ReconnectStrategy};
use crate::protocol::{Command, Reply};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Connecting,
    Connected,
}

#[allow(clippy::type_complexity)]
struct CentrifugeInner {
    rt: Handle,
    join_set: tokio::task::JoinSet<()>,
    url: Arc<str>,
    state: State,
    reconnect_attempts: u32,
    reconnect_strategy: Arc<dyn ReconnectStrategy>,
    closer_write: Option<mpsc::Sender<()>>,
    control_write: Option<mpsc::Sender<(Command, oneshot::Sender<Result<Reply, ReplyError>>, Duration)>>,
    on_connecting: Option<Box<dyn FnMut() + Send + 'static>>,
    on_connected: Option<Box<dyn FnMut() + Send + 'static>>,
    on_disconnected: Option<Box<dyn FnMut() + Send + 'static>>,
    on_error: Option<Box<dyn FnMut(anyhow::Error) + Send + 'static>>,
}

impl CentrifugeInner {
    // Disconnected, Connected -> Connecting
    //  - from Disconnected: `.connect()` called
    //  - from Connected: temporary connection loss / reconnect advice
    fn move_to_connecting(&mut self, outer: Arc<Mutex<Self>>) {
        debug_assert_ne!(self.state, State::Connecting);
        self.set_state(State::Connecting);
        self.start_reconnecting(outer);

        if let Some(ref mut on_connecting) = self.on_connecting {
            on_connecting();
        }
    }

    // Connecting -> Connected
    //  - from Connecting: successful connect
    fn move_to_connected(
        &mut self,
        outer: Arc<Mutex<Self>>,
        closer: mpsc::Receiver<()>,
        stream: WebSocketStream<ConnectStream>,
    ) {
        assert_eq!(self.state, State::Connecting);
        self.set_state(State::Connected);
        self.reconnect_attempts = 0;

        let (control_write, control_read) = mpsc::channel(32);
        self.control_write = Some(control_write);

        // self.control_write.as_mut().unwrap().try_send((Command::Connect(ConnectRequest {
        //     token: "".to_string(),
        //     data: vec![],
        //     subs: std::collections::HashMap::new(),
        //     name: "".to_string(),
        //     version: "".to_string(),
        // }), oneshot::channel().0, Duration::from_secs(5))).unwrap();

        let client = outer;
        self.join_set.spawn_on(async move {
            let client1 = client.clone();
            crate::client_handler::websocket_handler(stream, control_read, closer, |_push| {
                // dbg!(push);
            }, move |err| {
                let mut inner = client1.lock().unwrap();
                if let Some(ref mut on_error) = inner.on_error {
                    on_error(err);
                }
            }).await;

            {
                let outer = client.clone();
                let mut inner = client.lock().unwrap();
                if inner.state != State::Connected { return; }
                inner.move_to_connecting(outer);
            }
        }, &self.rt);

        if let Some(ref mut on_connected) = self.on_connected {
            on_connected();
        }
    }

    // Connecting, Connected -> Disconnected
    //  - from Connecting: `.disconnect()` called or terminal condition met
    //  - from Connected: `.disconnect()` called or terminal condition met
    fn move_to_disconnected(&mut self) {
        assert_ne!(self.state, State::Disconnected);
        self.set_state(State::Disconnected);
        self.reconnect_attempts = 0;
        self.closer_write = None;
        self.control_write = None;

        if let Some(ref mut on_disconnected) = self.on_disconnected {
            on_disconnected();
        }
    }

    fn start_reconnecting(&mut self, outer: Arc<Mutex<Self>>) {
        assert_eq!(self.state, State::Connecting);

        let (closer_write, mut closer_read) = mpsc::channel(1);
        self.closer_write = Some(closer_write);

        let client = outer;
        self.join_set.spawn_on(async move {
            let task = async {
                let (url, delay) = {
                    let inner = client.lock().unwrap();
                    if inner.state != State::Connecting { return Err(()); }
                    let delay = if inner.reconnect_attempts > 0 {
                        let delay = inner.reconnect_strategy.time_before_next_attempt(inner.reconnect_attempts);
                        log::debug!("reconnecting attempt {}, delay={:?}", inner.reconnect_attempts, delay);
                        delay
                    } else {
                        Duration::ZERO
                    };
                    (inner.url.clone(), delay)
                };

                if delay > Duration::ZERO {
                    tokio::time::sleep(delay).await;

                    let inner = client.lock().unwrap();
                    if inner.state != State::Connecting { return Err(()); }
                }

                log::debug!("connecting to {}", &url);
                let stream = match async_tungstenite::tokio::connect_async(&*url).await {
                    Ok((stream, _)) => stream,
                    Err(err) => {
                        log::debug!("{err}");
                        let outer = client.clone();
                        let mut inner = client.lock().unwrap();
                        if inner.state != State::Connecting { return Err(()); }
                        if let Some(ref mut on_error) = inner.on_error {
                            on_error(err.into());
                        }
                        inner.reconnect_attempts += 1;
                        inner.start_reconnecting(outer);
                        return Err(());
                    }
                };

                Ok(stream)
            };

            tokio::select! {
                biased;
                _ = closer_read.recv() => {
                    log::debug!("reconnect interrupted by user");
                }
                result = task => {
                    if let Ok(stream) = result {
                        let outer = client.clone();
                        let mut inner = client.lock().unwrap();
                        if inner.state != State::Connecting { return; }
                        inner.move_to_connected(outer, closer_read, stream);
                    }
                }
            }
        }, &self.rt);
    }

    fn set_state(&mut self, state: State) {
        log::debug!("state: {:?} -> {:?}", self.state, state);
        self.state = state;
    }
}

pub struct Centrifuge(Arc<Mutex<CentrifugeInner>>);

impl Centrifuge {
    pub fn new(url: &str, config: Config) -> Self {
        let rt = config.runtime.unwrap_or_else(|| tokio::runtime::Handle::current());

        Self(Arc::new(Mutex::new(CentrifugeInner {
            rt,
            join_set: tokio::task::JoinSet::new(),
            url: url.into(),
            state: State::Disconnected,
            reconnect_attempts: 0,
            reconnect_strategy: config.reconnect_strategy.unwrap_or_else(|| {
                Arc::new(crate::config::BackoffReconnect::default())
            }),
            closer_write: None,
            control_write: None,
            on_connecting: None,
            on_connected: None,
            on_disconnected: None,
            on_error: None,
        })))
    }

    pub fn connect(&self) {
        let mut inner = self.0.lock().unwrap();
        if inner.state == State::Disconnected {
            inner.move_to_connecting(self.0.clone());
        }
    }

    pub fn disconnect(&self) {
        let mut inner = self.0.lock().unwrap();
        if inner.state != State::Disconnected {
            inner.move_to_disconnected();
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
}
