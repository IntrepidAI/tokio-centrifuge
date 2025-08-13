//! # Session Module
//!
//! This module handles client session management, including connection state,
//! command processing, and subscription management.
//!
//! ## Core Components
//!
//! - **ServerSession**: Main session handler for client connections
//! - **SessionState**: Tracks the current state of a client session
//! - **Subscription**: Manages individual channel subscriptions
//!
//! ## Session Lifecycle
//!
//! 1. **Initial**: Client connects, authentication occurs
//! 2. **Connected**: Client is authenticated and can send commands
//! 3. **Terminated**: Client disconnects (future state)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tokio::task::{AbortHandle, JoinSet};

use crate::errors::{ClientErrorCode, DisconnectErrorCode};
use crate::protocol::{
    Command, ConnectResult, Publication, PublishResult, Push, PushData, Reply, RpcResult,
    SubscribeResult, Unsubscribe, UnsubscribeResult,
};
use crate::server::callbacks::{ChannelFn, OnConnectFn, OnDisconnectFn, PublishFn, RpcMethodFn};
use crate::server::types::{
    ConnectContext, DisconnectContext, PublishContext, RpcContext, ServeParams, SubContext,
};
use matchit::Router;

/// Represents the current state of a client session
///
/// Sessions progress through different states as clients connect,
/// authenticate, and eventually disconnect.
///
/// ## State Transitions
///
/// - **Initial** → **Connected**: After successful authentication
/// - **Connected** → **Terminated**: When client disconnects
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Client has connected but not yet authenticated
    Initial,
    /// Client is authenticated and can send commands
    Connected,
    // /// Client has disconnected (future state)
    // Terminated,
}

/// Manages a single channel subscription
///
/// This struct tracks an active subscription and ensures proper cleanup
/// when the subscription is dropped or cancelled.
///
/// ## Drop Behavior
///
/// When dropped, the subscription automatically aborts any running tasks
/// to prevent resource leaks.
struct Subscription {
    /// Handle to abort the subscription task
    abort_handle: AbortHandle,
}

impl Drop for Subscription {
    /// Automatically aborts the subscription when dropped
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

/// Main session handler for a client connection
///
/// This struct manages the entire lifecycle of a client connection,
/// including authentication, command processing, and resource cleanup.
///
/// ## Features
///
/// - **State Management**: Tracks connection state and enforces protocol rules
/// - **Command Processing**: Handles all client commands (RPC, publish, subscribe, etc.)
/// - **Resource Management**: Manages subscriptions, tasks, and semaphores
/// - **Error Handling**: Provides appropriate error responses to clients
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::session::ServerSession;
/// use tokio_centrifuge::server::types::ServeParams;
///
/// // In a real implementation, you would have these handlers
/// // let params = ServeParams::json();
/// // let (reply_tx, reply_rx) = tokio::sync::mpsc::channel(64);
///
/// // let mut session = ServerSession::new(
/// //     params,
/// //     reply_tx,
/// //     on_connect_fn,
/// //     Some(on_disconnect_fn),
/// //     rpc_methods,
/// //     sub_channels,
/// //     pub_channels,
/// // );
///
/// // Process client commands (in async context)
/// // session.process_command(1, Command::Connect(connect_data)).await;
/// ```
pub struct ServerSession {
    /// Server configuration parameters
    params: ServeParams,
    /// Current session state
    state: SessionState,
    /// Connection handler function
    on_connect: OnConnectFn,
    /// Optional disconnect handler function
    on_disconnect: Option<OnDisconnectFn>,
    /// Semaphore limiting concurrent RPC requests
    rpc_semaphore: Arc<Semaphore>,
    /// Registered RPC methods
    rpc_methods: Arc<Mutex<Router<RpcMethodFn>>>,
    /// Registered subscription channels
    sub_channels: Arc<Mutex<Router<ChannelFn>>>,
    /// Registered publication channels
    pub_channels: Arc<Mutex<Router<PublishFn>>>,
    /// Active subscriptions for this client
    subscriptions: Arc<Mutex<HashMap<String, Subscription>>>,
    /// Background tasks for this session
    tasks: JoinSet<()>,
    /// Channel for sending replies to the client
    reply_ch: tokio::sync::mpsc::Sender<Result<(u32, Reply), DisconnectErrorCode>>,
}

impl ServerSession {
    /// Creates a new server session
    ///
    /// ## Arguments
    ///
    /// * `params` - Server configuration parameters
    /// * `reply_ch` - Channel for sending replies to the client
    /// * `on_connect` - Connection handler function
    /// * `on_disconnect` - Optional disconnect handler function
    /// * `rpc_methods` - Registered RPC methods
    /// * `sub_channels` - Registered subscription channels
    /// * `pub_channels` - Registered publication channels
    ///
    /// ## Configuration
    ///
    /// The session is configured with:
    /// - Maximum 10 concurrent RPC requests
    /// - Initial state (unauthenticated)
    /// - Empty subscription list
    /// - Empty task list
    #[allow(clippy::too_many_arguments)]
    pub fn new(
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

    /// Processes a command from the client
    ///
    /// This method handles all client commands based on the current session state.
    /// It enforces protocol rules and routes commands to appropriate handlers.
    ///
    /// ## Arguments
    ///
    /// * `id` - Command ID for response correlation
    /// * `command` - The command to process
    ///
    /// ## State-Based Processing
    ///
    /// - **Initial State**: Only accepts `Connect` commands
    /// - **Connected State**: Accepts all commands except `Connect`
    ///
    /// ## Commands Handled
    ///
    /// - **Connect**: Client authentication
    /// - **RPC**: Remote procedure calls
    /// - **Publish**: Data publication to channels
    /// - **Subscribe**: Channel subscription requests
    /// - **Unsubscribe**: Channel unsubscription requests
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::protocol::Command;
    ///
    /// // Process a connect command (in async context)
    /// // session.process_command(1, Command::Connect(connect_data)).await;
    ///
    /// // Process an RPC call (in async context)
    /// // session.process_command(2, Command::Rpc(rpc_data)).await;
    /// ```
    pub async fn process_command(&mut self, id: u32, command: Command) {
        const MAX_SUBSCRIPTION_COUNT: usize = 128;

        match self.state {
            SessionState::Initial => match command {
                Command::Connect(connect) => {
                    log::debug!(
                        "connection established with name={}, version={}",
                        connect.name,
                        connect.version
                    );
                    match (self.on_connect)(ConnectContext {
                        client_id: self.params.client_id.unwrap(),
                        client_name: connect.name,
                        client_version: connect.version,
                        token: connect.token,
                        data: connect.data,
                    })
                    .await
                    {
                        Ok(()) => (),
                        Err(err) => {
                            log::debug!("authentication failed: {:?}", err);
                            let _ = self.reply_ch.send(Err(err)).await;
                            return;
                        }
                    }

                    self.state = SessionState::Connected;
                    let _ = self
                        .reply_ch
                        .send(Ok((
                            id,
                            Reply::Connect(ConnectResult {
                                client: self.params.client_id.unwrap().to_string(),
                                ping: self
                                    .params
                                    .ping_interval
                                    .unwrap_or(std::time::Duration::ZERO)
                                    .as_secs() as u32,
                                pong: self.params.pong_timeout.is_some(),
                                ..Default::default()
                            }),
                        )))
                        .await;
                }
                _ => {
                    log::debug!("expected connect request, got: {:?}", command);
                    let _ = self
                        .reply_ch
                        .send(Err(DisconnectErrorCode::BadRequest))
                        .await;
                }
            },
            SessionState::Connected => match command {
                Command::Connect(_) => {
                    log::debug!("client already authenticated");
                    let _ = self
                        .reply_ch
                        .send(Err(DisconnectErrorCode::BadRequest))
                        .await;
                }
                Command::Rpc(rpc_request) => {
                    let permit = self.rpc_semaphore.clone().acquire_owned().await;

                    let Some(match_) = ({
                        let methods = self.rpc_methods.lock().unwrap();
                        match methods.at(&rpc_request.method) {
                            Ok(match_) => {
                                let params: HashMap<String, String> = match_
                                    .params
                                    .iter()
                                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                                    .collect();
                                Some((params, match_.value.clone()))
                            }
                            Err(_err) => None,
                        }
                    }) else {
                        let _ = self
                            .reply_ch
                            .send(Ok((
                                id,
                                Reply::Error(ClientErrorCode::MethodNotFound.into()),
                            )))
                            .await;
                        return;
                    };

                    let reply_ch = self.reply_ch.clone();
                    let client_id = self.params.client_id.unwrap();
                    let encoding = self.params.encoding;
                    self.tasks.spawn(async move {
                        let (match_params, f) = match_;
                        let result = f(
                            RpcContext {
                                encoding,
                                client_id,
                                params: match_params,
                            },
                            rpc_request.data,
                        )
                        .await;
                        match result {
                            Ok(data) => {
                                let _ = reply_ch
                                    .send(Ok((id, Reply::Rpc(RpcResult { data }))))
                                    .await;
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
                                let params: HashMap<String, String> = match_
                                    .params
                                    .iter()
                                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                                    .collect();
                                Some((params, match_.value.clone()))
                            }
                            Err(_err) => None,
                        }
                    }) else {
                        let _ = self
                            .reply_ch
                            .send(Ok((
                                id,
                                Reply::Error(ClientErrorCode::UnknownChannel.into()),
                            )))
                            .await;
                        return;
                    };

                    let reply_ch = self.reply_ch.clone();
                    let client_id = self.params.client_id.unwrap();
                    let encoding = self.params.encoding;
                    self.tasks.spawn(async move {
                        let (match_params, f) = match_;
                        let result = f(
                            PublishContext {
                                encoding,
                                client_id,
                                params: match_params,
                            },
                            pub_request.data,
                        )
                        .await;
                        match result {
                            Ok(()) => {
                                let _ = reply_ch
                                    .send(Ok((id, Reply::Publish(PublishResult {}))))
                                    .await;
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
                                    let params: HashMap<String, String> = match_
                                        .params
                                        .iter()
                                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                                        .collect();
                                    Some((params, match_.value.clone()))
                                }
                                Err(_err) => None,
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
                            })
                            .await
                            {
                                Ok(stream) => stream,
                                Err(err) => {
                                    subscriptions_.lock().unwrap().remove(&channel_name);
                                    let _ = reply_ch.send(Ok((id, Reply::Error(err.into())))).await;
                                    return;
                                }
                            };

                            let _ = reply_ch
                                .send(Ok((id, Reply::Subscribe(SubscribeResult::default()))))
                                .await;
                            drop(permit);

                            use futures::StreamExt;
                            while let Some(data) = stream.next().await {
                                let _ = reply_ch
                                    .send(Ok((
                                        0,
                                        Reply::Push(Push {
                                            channel: channel_name.clone(),
                                            data: PushData::Publication(Publication {
                                                data,
                                                ..Default::default()
                                            }),
                                        }),
                                    )))
                                    .await;
                            }

                            subscriptions_.lock().unwrap().remove(&channel_name);

                            let _ = reply_ch
                                .send(Ok((
                                    0,
                                    Reply::Push(Push {
                                        channel: channel_name.clone(),
                                        data: PushData::Unsubscribe(Unsubscribe {
                                            code: 2500,
                                            reason: "channel closed".to_string(),
                                        }),
                                    }),
                                )))
                                .await;
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
                    let _ = self
                        .reply_ch
                        .send(Ok((id, Reply::Error(ClientErrorCode::NotAvailable.into()))))
                        .await;
                }
            }, // SessionState::Terminated => {}
        }
    }

    /// Returns the current session state
    ///
    /// This method is used by external code to check the current
    /// state of the session without direct access to the field.
    ///
    /// ## Returns
    ///
    /// The current `SessionState` of the session
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::session::SessionState;
    ///
    /// // In a real implementation, you would have a session object
    /// // let session = get_session();
    ///
    /// // match session.state() {
    /// //     SessionState::Initial => println!("Client not yet authenticated"),
    /// //     SessionState::Connected => println!("Client is authenticated"),
    /// // }
    /// ```
    pub fn state(&self) -> SessionState {
        self.state
    }
}

impl Drop for ServerSession {
    /// Cleans up the session when dropped
    ///
    /// If the client was connected, this calls the disconnect handler
    /// to allow for proper cleanup and logging.
    fn drop(&mut self) {
        if self.state == SessionState::Connected {
            if let Some(on_disconnect) = &self.on_disconnect {
                on_disconnect(DisconnectContext {
                    client_id: self.params.client_id.unwrap(),
                });
            }
        }
    }
}
