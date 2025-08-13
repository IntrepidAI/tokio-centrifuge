//! Internal client implementation and connection lifecycle management.
//!
//! This module contains the core client implementation that manages
//! the connection lifecycle, state transitions, and internal operations.
//! It coordinates between the various specialized modules to provide
//! a unified client experience.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::Future;
use slotmap::SlotMap;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

use crate::client_handler::ReplyError;
use crate::config::{Protocol, ReconnectStrategy};
use crate::protocol::Command;
use crate::subscription::{SubscriptionId, SubscriptionInner};
use crate::{
    client::types::{MessageStore, State},
    subscription,
};

use super::connection::ConnectionManager;
use super::handshake::HandshakeManager;
use super::subscription_handler::SubscriptionHandler;

/// Internal client implementation that manages the connection lifecycle.
///
/// This struct contains all the internal state and logic for managing
/// the client's connection to the server, including state transitions,
/// subscription management, and error handling.
pub(crate) struct ClientInner {
    /// Tokio runtime handle for spawning tasks
    pub(crate) rt: Handle,
    /// WebSocket server URL
    pub(crate) url: Arc<str>,
    /// Current connection state
    pub(crate) state: State,
    /// Authentication token for the server
    pub(crate) token: String,
    /// Client name identifier
    pub(crate) name: String,
    /// Client version string
    pub(crate) version: String,
    /// Protocol configuration (JSON/Protobuf)
    pub(crate) protocol: Protocol,
    /// Strategy for handling reconnection attempts
    pub(crate) reconnect_strategy: Arc<dyn ReconnectStrategy>,
    /// Timeout for read operations
    pub(crate) read_timeout: Duration,
    /// Channel for sending close signals
    pub(crate) closer_write: Option<mpsc::Sender<bool>>,
    /// Callback for connection start events
    pub(crate) on_connecting: Option<Box<dyn FnMut() + Send + 'static>>,
    /// Callback for successful connection events
    pub(crate) on_connected: Option<Box<dyn FnMut() + Send + 'static>>,
    /// Channels waiting for connection completion
    pub(crate) on_connected_ch: Vec<oneshot::Sender<Result<(), ()>>>,
    /// Callback for disconnection events
    pub(crate) on_disconnected: Option<Box<dyn FnMut() + Send + 'static>>,
    /// Channels waiting for disconnection completion
    pub(crate) on_disconnected_ch: Vec<oneshot::Sender<()>>,
    /// Callback for error events
    pub(crate) on_error: Option<Box<dyn FnMut(anyhow::Error) + Send + 'static>>,
    /// Active subscriptions
    pub(crate) subscriptions: SlotMap<SubscriptionId, SubscriptionInner>,
    /// Mapping from channel names to subscription IDs
    pub(crate) sub_name_to_id: HashMap<String, SubscriptionId>,
    /// Message store for publishing
    pub(crate) pub_ch_write: Option<MessageStore>,
    /// Channel for subscription management
    pub(crate) sub_ch_write: Option<mpsc::UnboundedSender<SubscriptionId>>,
    /// Number of active background tasks
    pub(crate) active_tasks: usize,
}

impl ClientInner {
    /// Transitions the client to the connecting state.
    ///
    /// This method is called when starting a new connection attempt.
    /// It sets up the message store and starts the connection process.
    ///
    /// # Arguments
    ///
    /// * `outer` - Arc reference to self for spawning tasks
    pub(crate) fn move_to_connecting(&mut self, outer: Arc<Mutex<Self>>) {
        debug_assert_ne!(self.state, State::Connecting);
        if self.pub_ch_write.is_none() {
            let (pub_ch_write, _) = MessageStore::new(self.read_timeout);
            self.pub_ch_write = Some(pub_ch_write);
        }
        self._set_state(State::Connecting);
        self.start_connecting(outer);
    }

    /// Transitions the client to the connected state.
    ///
    /// This method is called when a connection is successfully established.
    /// It should only be called from the connecting state.
    pub(crate) fn move_to_connected(&mut self) {
        assert_eq!(self.state, State::Connecting);
        self._set_state(State::Connected);
    }

    /// Transitions the client to the disconnected state.
    ///
    /// This method is called when a connection is lost or explicitly closed.
    /// It cleans up connection-related resources.
    pub(crate) fn move_to_disconnected(&mut self) {
        assert_ne!(self.state, State::Disconnected);
        self.closer_write = None;
        self.pub_ch_write = None;
        self._set_state(State::Disconnected);
    }

    /// Manages the complete connection lifecycle.
    ///
    /// This function handles the entire connection process including:
    /// - Establishing initial connection
    /// - Managing reconnection attempts
    /// - Processing the connection until it's lost
    /// - Determining whether to reconnect
    ///
    /// # Arguments
    ///
    /// * `client` - Arc reference to the client instance
    async fn do_connection_cycle(client: Arc<Mutex<Self>>) {
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
                    ConnectionManager::do_check_state(&client, State::Connecting)?;
                    ConnectionManager::do_delay(&client, &mut closer_read, reconnect_attempt - 1)
                        .await?;

                    ConnectionManager::do_check_state(&client, State::Connecting)?;
                    let stream = ConnectionManager::do_connect(&client, &mut closer_read).await?;

                    ConnectionManager::do_check_state(&client, State::Connecting)?;
                    let result =
                        HandshakeManager::do_handshake(&client, closer_write, closer_read, stream)
                            .await?;
                    Ok(result)
                }
                .await;

                let mut inner = client.lock().unwrap();
                if inner.state != State::Connecting {
                    return false;
                }
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

            let (sub_ch_write, sub_ch_read) = mpsc::unbounded_channel();
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

            let mut join_set = JoinSet::new();

            #[allow(clippy::type_complexity)]
            enum JoinSetResult {
                MainTask(bool),
                NewTask(
                    (
                        Option<Pin<Box<dyn Future<Output = JoinSetResult> + Send>>>,
                        mpsc::Receiver<Pin<Box<dyn Future<Output = JoinSetResult> + Send>>>,
                    ),
                ),
                Irrelevant,
            }

            join_set.spawn_on(async { JoinSetResult::MainTask(future.await) }, &rt);

            let (_new_tasks_tx, new_tasks_rx) = mpsc::channel(1);

            join_set.spawn_on(
                async move { JoinSetResult::NewTask((None, new_tasks_rx)) },
                &rt,
            );

            /// Task for handling message publishing from the main message store.
            #[allow(clippy::type_complexity)]
            async fn publish_task(
                client: Arc<Mutex<ClientInner>>,
                control_write: mpsc::Sender<(
                    Command,
                    oneshot::Sender<Result<crate::protocol::Reply, ReplyError>>,
                    Duration,
                )>,
                get_store: impl Fn(&mut ClientInner) -> Option<&mut MessageStore>,
            ) -> JoinSetResult {
                let mut pub_ch_read = {
                    let mut inner = client.lock().unwrap();
                    let Some(store) = get_store(&mut inner) else {
                        return JoinSetResult::Irrelevant;
                    };
                    store.reset_channel()
                };

                const MAX_CAPACITY: usize = 32;
                let mut buffer = Vec::new();
                loop {
                    {
                        // lock mutex and fill our buffer
                        let mut inner = client.lock().unwrap();
                        let Some(pub_ch_write) = get_store(&mut inner) else {
                            break;
                        };
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
                        let Some(()) = pub_ch_read.recv().await else {
                            break;
                        };
                    } else {
                        // send messages
                        for item in buffer.drain(..) {
                            let timeout = item.deadline.saturating_duration_since(Instant::now());
                            let _ = control_write
                                .send((item.command, item.reply, timeout))
                                .await;
                        }
                    }
                }

                JoinSetResult::Irrelevant
            }

            join_set.spawn_on(
                publish_task(client.clone(), control_write.clone(), |inner| {
                    inner.pub_ch_write.as_mut()
                }),
                &rt,
            );

            // Handle subscriptions in a separate task
            let client_clone = client.clone();
            let control_write_clone = control_write.clone();
            let rt_clone = rt.clone();
            tokio::spawn(async move {
                SubscriptionHandler::handle_subscriptions(
                    client_clone,
                    sub_ch_read,
                    control_write_clone,
                    rt_clone,
                )
                .await;
            });

            while let Some(result) = join_set.join_next().await {
                match result {
                    Ok(JoinSetResult::MainTask(need_reconnect)) => {
                        return need_reconnect;
                    }
                    Ok(JoinSetResult::NewTask((task, mut new_tasks_rx))) => {
                        join_set.spawn_on(
                            async move {
                                if let Some(task) = new_tasks_rx.recv().await {
                                    JoinSetResult::NewTask((Some(task), new_tasks_rx))
                                } else {
                                    JoinSetResult::Irrelevant
                                }
                            },
                            &rt,
                        );
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
        }
        .await;

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

    /// Starts the connection process in a background task.
    ///
    /// This method spawns a new task to handle the connection lifecycle
    /// and increments the active task counter.
    ///
    /// # Arguments
    ///
    /// * `client` - Arc reference to self for the spawned task
    fn start_connecting(&mut self, client: Arc<Mutex<Self>>) {
        self.active_tasks += 1;

        self.rt.spawn(async move {
            Self::do_connection_cycle(client.clone()).await;
            let mut inner = client.lock().unwrap();
            inner.active_tasks -= 1;
        });
    }

    /// Updates the client state and triggers appropriate callbacks.
    ///
    /// This method changes the internal state and calls the appropriate
    /// callback functions based on the new state.
    ///
    /// # Arguments
    ///
    /// * `state` - The new state to transition to
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
