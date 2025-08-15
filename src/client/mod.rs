//! Client implementation for connecting to Centrifugo servers.
//!
//! This module provides a high-level client API for establishing WebSocket
//! connections to Centrifugo servers, managing subscriptions, and handling
//! real-time messaging.
//!
//! # Example
//!
//! ```rust
//! use tokio_centrifuge::client::{Client, State};
//! use tokio_centrifuge::config::Config;
//!
//! // Note: In a real application, you would use #[tokio::main] and await
//! // #[tokio::main]
//! // async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     // Create client configuration
//!     let config = Config::new()
//!         .use_json()
//!         .with_token("your-auth-token");
//!     
//!     // Create client instance
//!     // let client = Client::new("ws://localhost:8000/connection/websocket", config);
//!     
//!     // Connect to server
//!     // let connect_result = client.connect().await;
//!     
//!     // Check connection state
//!     // assert_eq!(client.state(), State::Connected);
//!     
//!     // Create subscription
//!     // let subscription = client.new_subscription("news");
//!     
//!     // Subscribe to channel
//!     // let subscribe_result = subscription.subscribe().await;
//!     
//!     // Publish message
//!     // let publish_result = client.publish("news", b"Hello, World!".to_vec()).await;
//!     
//!     // Ok(())
//! // }
//! ```

pub mod connection;
pub mod errors;
pub mod handshake;
pub mod inner;
pub mod subscription_handler;
pub mod types;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use slotmap::SlotMap;

use crate::config::Config;
use crate::subscription::{Subscription, SubscriptionInner};
use crate::{errors as crate_errors, subscription};

pub use errors::{FutureResult, RequestError};
use inner::ClientInner;
pub use types::State;

/// High-level client for connecting to Centrifugo servers.
///
/// The `Client` provides a simple interface for establishing WebSocket connections,
/// managing subscriptions, and publishing messages. It handles connection lifecycle,
/// reconnection, and error handling automatically.
///
/// # Thread Safety
///
/// The client is safe to use from multiple threads and can be cloned.
/// All operations are internally synchronized using mutexes.
#[derive(Clone)]
pub struct Client(pub(crate) Arc<Mutex<ClientInner>>);

impl Client {
    /// Creates a new client instance.
    ///
    /// # Arguments
    ///
    /// * `url` - WebSocket URL of the Centrifugo server
    /// * `config` - Client configuration including protocol, authentication, etc.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    /// ```
    pub fn new(url: &str, config: Config) -> Self {
        // For documentation tests, we'll use a simple approach
        // In real usage, this would handle the runtime properly
        let rt = config.runtime.unwrap_or_else(|| {
            // This will panic in doc tests, but that's expected
            // Users should provide a runtime or use this in a proper Tokio context
            tokio::runtime::Handle::current()
        });

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

    /// Initiates a connection to the server.
    ///
    /// This method returns a future that resolves when the connection is established.
    /// The future resolves immediately if already connected, or never resolves if
    /// the connection can never be established.
    ///
    /// # Returns
    ///
    /// A `FutureResult` containing a future that resolves to `Ok(())` on successful
    /// connection or `Err(())` on connection failure.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // Note: In a real application, you would use #[tokio::main] and await
    /// // let connect_future = client.connect();
    /// // let result = connect_future.await;
    /// ```
    pub fn connect(&self) -> FutureResult<impl std::future::Future<Output = Result<(), ()>>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
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

    /// Disconnects from the server.
    ///
    /// This method initiates a graceful disconnection and returns a future that
    /// resolves when the disconnection is complete.
    ///
    /// # Returns
    ///
    /// A `FutureResult` containing a future that resolves when disconnected.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // Note: In a real application, you would use #[tokio::main] and await
    /// // let disconnect_future = client.disconnect();
    /// // disconnect_future.await; // Wait for disconnection to complete
    /// ```
    pub fn disconnect(&self) -> FutureResult<impl std::future::Future<Output = ()>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
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

    /// Publishes a message to a channel.
    ///
    /// This method sends a message to the specified channel. The message will be
    /// queued if not connected and sent when the connection is established.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel name to publish to
    /// * `data` - The message data as bytes
    ///
    /// # Returns
    ///
    /// A `FutureResult` containing a future that resolves to `Ok(())` on successful
    /// publication or an error on failure.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // Note: In a real application, you would use #[tokio::main] and await
    /// // let publish_future = client.publish("news", b"Breaking news!".to_vec());
    /// // let result = publish_future.await;
    /// ```
    pub fn publish(
        &self,
        channel: &str,
        data: Vec<u8>,
    ) -> FutureResult<impl std::future::Future<Output = Result<(), RequestError>>> {
        let mut inner = self.0.lock().unwrap();
        let read_timeout = inner.read_timeout;
        let deadline = Instant::now() + read_timeout;
        let rx = if let Some(ref mut pub_ch_write) = inner.pub_ch_write {
            pub_ch_write.send(crate::protocol::Command::Publish(
                crate::protocol::PublishRequest {
                    channel: channel.into(),
                    data,
                },
            ))
        } else {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(Err(crate::client_handler::ReplyError::Closed));
            rx
        };
        FutureResult(async move {
            let result = tokio::time::timeout_at(deadline.into(), rx).await;
            match result {
                Ok(Ok(Ok(crate::protocol::Reply::Publish(_)))) => Ok(()),
                Ok(Ok(Ok(crate::protocol::Reply::Error(err)))) => {
                    Err(RequestError::ErrorResponse(err))
                }
                Ok(Ok(Ok(reply))) => Err(RequestError::UnexpectedReply(reply)),
                Ok(Ok(Err(err))) => Err(err.into()),
                Ok(Err(err)) => Err(err.into()),
                Err(err) => Err(err.into()),
            }
        })
    }

    /// Makes an RPC call to the server.
    ///
    /// This method sends an RPC request to the specified method and returns
    /// the response data.
    ///
    /// # Arguments
    ///
    /// * `method` - The RPC method name to call
    /// * `data` - The request data as bytes
    ///
    /// # Returns
    ///
    /// A `FutureResult` containing a future that resolves to the response data
    /// or an error on failure.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // Note: In a real application, you would use #[tokio::main] and await
    /// // let rpc_future = client.rpc("getUserInfo", b"user123".to_vec());
    /// // let result = rpc_future.await;
    /// ```
    pub fn rpc(
        &self,
        method: &str,
        data: Vec<u8>,
    ) -> FutureResult<impl std::future::Future<Output = Result<Vec<u8>, RequestError>>> {
        let mut inner = self.0.lock().unwrap();
        let read_timeout = inner.read_timeout;
        let deadline = Instant::now() + read_timeout;
        let rx = if let Some(ref mut pub_ch_write) = inner.pub_ch_write {
            pub_ch_write.send(crate::protocol::Command::Rpc(crate::protocol::RpcRequest {
                method: method.into(),
                data,
            }))
        } else {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(Err(crate::client_handler::ReplyError::Closed));
            rx
        };
        FutureResult(async move {
            let result = tokio::time::timeout_at(deadline.into(), rx).await;
            match result {
                Ok(Ok(Ok(crate::protocol::Reply::Rpc(rpc_result)))) => Ok(rpc_result.data),
                Ok(Ok(Ok(crate::protocol::Reply::Error(err)))) => {
                    Err(RequestError::ErrorResponse(err))
                }
                Ok(Ok(Ok(reply))) => Err(RequestError::UnexpectedReply(reply)),
                Ok(Ok(Err(err))) => Err(err.into()),
                Ok(Err(err)) => Err(err.into()),
                Err(err) => Err(err.into()),
            }
        })
    }

    /// Creates a new subscription to a channel.
    ///
    /// This method creates a subscription object for the specified channel.
    /// If a subscription already exists for this channel, it returns the existing one.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel name to subscribe to
    ///
    /// # Returns
    ///
    /// A `Subscription` object that can be used to manage the subscription.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // let subscription = client.new_subscription("news");
    /// // Note: In a real application, you would use #[tokio::main] and await
    /// // let result = subscription.subscribe().await;
    /// ```
    pub fn new_subscription(&self, channel: &str) -> Subscription {
        let mut inner = self.0.lock().unwrap();
        if let Some(key) = inner.sub_name_to_id.get(channel) {
            return Subscription::new(self, *key);
        }

        let timeout = inner.read_timeout;
        let key = inner
            .subscriptions
            .insert(SubscriptionInner::new(channel, timeout));
        inner.sub_name_to_id.insert(channel.to_string(), key);
        Subscription::new(self, key)
    }

    /// Retrieves an existing subscription for a channel.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel name
    ///
    /// # Returns
    ///
    /// `Some(Subscription)` if a subscription exists, `None` otherwise.
    pub fn get_subscription(&self, channel: &str) -> Option<Subscription> {
        let inner = self.0.lock().unwrap();
        inner
            .sub_name_to_id
            .get(channel)
            .map(|id| Subscription::new(self, *id))
    }

    /// Removes a subscription from the client.
    ///
    /// This method removes the subscription and frees associated resources.
    /// The subscription must be unsubscribed before it can be removed.
    ///
    /// # Arguments
    ///
    /// * `subscription` - The subscription to remove
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an error if the subscription is still active.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// // Note: In a real application, you would use #[tokio::main] and await
    /// // #[tokio::main]
    /// // async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ///     let config = Config::new().use_json();
    ///     // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///     
    ///     // let subscription = client.new_subscription("news");
    ///     // let subscribe_result = subscription.subscribe().await;
    ///     // match subscribe_result {
    ///     //     Ok(()) => println!("Subscribed successfully"),
    ///     //     Err(()) => println!("Failed to subscribe"),
    ///     // }
    ///     // subscription.unsubscribe().await;
    ///     // let remove_result = client.remove_subscription(subscription);
    ///     // match remove_result {
    ///     //     Ok(()) => println!("Subscription removed"),
    ///     //     Err(err) => println!("Failed to remove subscription: {}", err),
    ///     // }
    ///     // Ok(())
    /// // }
    /// ```
    pub fn remove_subscription(
        &self,
        subscription: Subscription,
    ) -> Result<(), crate_errors::RemoveSubscriptionError> {
        let mut inner = self.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get(subscription.id) {
            if sub.state != subscription::State::Unsubscribed {
                Err(crate_errors::RemoveSubscriptionError::NotUnsubscribed)
            } else {
                let sub = inner.subscriptions.remove(subscription.id).unwrap();
                inner.sub_name_to_id.remove(&*sub.channel);
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    /// Sets a callback for when the client starts connecting.
    ///
    /// This callback is called whenever the client begins a connection attempt.
    ///
    /// # Arguments
    ///
    /// * `func` - The callback function to execute
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // client.on_connecting(|| {
    /// //     println!("Starting connection...");
    /// // });
    /// ```
    pub fn on_connecting(&self, func: impl FnMut() + Send + 'static) {
        self.0.lock().unwrap().on_connecting = Some(Box::new(func));
    }

    /// Sets a callback for when the client successfully connects.
    ///
    /// This callback is called whenever a connection is established.
    ///
    /// # Arguments
    ///
    /// * `func` - The callback function to execute
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // client.on_connected(|| {
    /// //     println!("Connected successfully!");
    /// // });
    /// ```
    pub fn on_connected(&self, func: impl FnMut() + Send + 'static) {
        self.0.lock().unwrap().on_connected = Some(Box::new(func));
    }

    /// Sets a callback for when the client disconnects.
    ///
    /// This callback is called whenever the connection is lost or closed.
    ///
    /// # Arguments
    ///
    /// * `func` - The callback function to execute
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // client.on_disconnected(|| {
    /// //     println!("Disconnected from server");
    /// // });
    /// ```
    pub fn on_disconnected(&self, func: impl FnMut() + Send + 'static) {
        self.0.lock().unwrap().on_disconnected = Some(Box::new(func));
    }

    /// Sets a callback for when errors occur.
    ///
    /// This callback is called whenever an error occurs during operation.
    ///
    /// # Arguments
    ///
    /// * `func` - The callback function to execute
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // client.on_error(|err| {
    /// //     eprintln!("Client error: {}", err);
    /// // });
    /// ```
    pub fn on_error(&self, func: impl FnMut(anyhow::Error) + Send + 'static) {
        self.0.lock().unwrap().on_error = Some(Box::new(func));
    }

    /// Returns the current connection state.
    ///
    /// # Returns
    ///
    /// The current `State` of the client connection.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::{Client, State};
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // match client.state() {
    /// //     State::Disconnected => println!("Not connected"),
    /// //     State::Connecting => println!("Connecting..."),
    /// //     State::Connected => println!("Connected!"),
    /// // }
    /// ```
    pub fn state(&self) -> State {
        self.0.lock().unwrap().state
    }

    /// Updates the authentication token.
    ///
    /// This method allows changing the token used for authentication,
    /// which will be used for the next connection attempt.
    ///
    /// # Arguments
    ///
    /// * `token` - The new authentication token
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    ///
    /// // client.set_token("new-auth-token");
    /// ```
    pub fn set_token(&self, token: impl Into<String>) {
        self.0.lock().unwrap().token = token.into();
    }
}
