//! Channel subscription management and lifecycle.
//!
//! This module provides the subscription functionality for the client,
//! allowing users to subscribe to channels, receive publications,
//! and manage subscription state transitions.
//!
//! # Example
//!
//! ```rust
//! use tokio_centrifuge::client::Client;
//! use tokio_centrifuge::subscription::State;
//! use tokio_centrifuge::config::Config;
//! 
//! // Note: In a real application, you would use #[tokio::main] and await
//! // #[tokio::main]
//! // async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!     let config = Config::new().use_json();
//!     // let client = Client::new("ws://localhost:8000", config);
//!     // let subscription = client.new_subscription("news");
//!     
//!     // Set up publication handler
//!     // subscription.on_publication(|pub_data| {
//!     //     println!("Received: {:?}", pub_data);
//!     // });
//!     
//!     // Subscribe to channel
//!     // let result = subscription.subscribe().await;
//!     // match result {
//!     //     Ok(()) => println!("Successfully subscribed"),
//!     //     Err(()) => println!("Failed to subscribe"),
//!     // }
//!     
//!     // Check state
//!     // assert_eq!(subscription.state(), State::Subscribed);
//!     
//!     // Unsubscribe when done
//!     // subscription.unsubscribe().await;
//!     
//!     // Ok(())
//! // }
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::Future;
use slotmap::new_key_type;
use tokio::sync::oneshot;

use crate::client::types::MessageStore;
use crate::client::{Client, FutureResult, RequestError};
use crate::client_handler::ReplyError;
use crate::protocol::{Command, Publication, PublishRequest, Reply};

new_key_type! { pub(crate) struct SubscriptionId; }

/// Represents the current state of a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Subscription is not active and not attempting to connect
    Unsubscribed,
    /// Subscription is in the process of being established
    Subscribing,
    /// Subscription is active and receiving publications
    Subscribed,
}

/// Internal subscription implementation that manages state and callbacks.
///
/// This struct contains the core subscription logic including state management,
/// callback handling, and message storage for publishing.
pub(crate) struct SubscriptionInner {
    /// The channel name this subscription is for
    pub(crate) channel: Arc<str>,
    /// Current subscription state
    pub(crate) state: State,
    /// Callback for when subscription starts
    on_subscribing: Option<Box<dyn FnMut() + Send + 'static>>,
    /// Callback for when subscription is established
    on_subscribed: Option<Box<dyn FnMut() + Send + 'static>>,
    /// Callback for when subscription is removed
    on_unsubscribed: Option<Box<dyn FnMut() + Send + 'static>>,
    /// Callback for incoming publications
    pub(crate) on_publication: Option<Box<dyn FnMut(Publication) + Send + 'static>>,
    /// Callback for subscription errors
    on_error: Option<Box<dyn FnMut(anyhow::Error) + Send + 'static>>,
    /// Channels waiting for subscription completion
    pub(crate) on_subscribed_ch: Vec<oneshot::Sender<Result<(), ()>>>,
    /// Channels waiting for unsubscription completion
    pub(crate) on_unsubscribed_ch: Vec<oneshot::Sender<()>>,
    /// Message store for publishing to this subscription
    pub(crate) pub_ch_write: Option<MessageStore>,
    /// Timeout for read operations
    read_timeout: Duration,
}

impl SubscriptionInner {
    /// Creates a new subscription for the specified channel.
    ///
    /// # Arguments
    ///
    /// * `channel` - The channel name to subscribe to
    /// * `read_timeout` - Timeout for read operations
    pub fn new(channel: &str, read_timeout: Duration) -> Self {
        SubscriptionInner {
            channel: channel.into(),
            state: State::Unsubscribed,
            on_subscribing: None,
            on_subscribed: None,
            on_unsubscribed: None,
            on_publication: None,
            on_error: None,
            on_subscribed_ch: Vec::new(),
            on_unsubscribed_ch: Vec::new(),
            pub_ch_write: None,
            read_timeout,
        }
    }

    /// Transitions the subscription to the subscribing state.
    ///
    /// This method sets up the message store for publishing and changes
    /// the state to indicate that subscription is in progress.
    pub fn move_to_subscribing(&mut self) {
        if self.pub_ch_write.is_none() {
            let (pub_ch_write, _) = MessageStore::new(self.read_timeout);
            self.pub_ch_write = Some(pub_ch_write);
        }
        self._set_state(State::Subscribing);
    }

    /// Transitions the subscription to the subscribed state.
    ///
    /// This method indicates that the subscription has been successfully
    /// established and is ready to receive publications.
    pub fn move_to_subscribed(&mut self) {
        self._set_state(State::Subscribed);
    }

    /// Transitions the subscription to the unsubscribed state.
    ///
    /// This method cleans up the message store and indicates that
    /// the subscription is no longer active.
    pub fn move_to_unsubscribed(&mut self) {
        self.pub_ch_write = None;
        self._set_state(State::Unsubscribed);
    }

    /// Updates the subscription state and triggers appropriate callbacks.
    ///
    /// This method changes the internal state and calls the appropriate
    /// callback functions based on the new state.
    ///
    /// # Arguments
    ///
    /// * `state` - The new state to transition to
    fn _set_state(&mut self, state: State) {
        log::debug!(
            "state: {:?} -> {:?}, channel={}",
            self.state,
            state,
            self.channel
        );
        self.state = state;

        match state {
            State::Unsubscribed => {
                if let Some(ref mut on_unsubscribed) = self.on_unsubscribed {
                    on_unsubscribed();
                }
            }
            State::Subscribing => {
                if let Some(ref mut on_subscribing) = self.on_subscribing {
                    on_subscribing();
                }
            }
            State::Subscribed => {
                if let Some(ref mut on_subscribed) = self.on_subscribed {
                    on_subscribed();
                }
            }
        }
    }
}

/// High-level subscription interface for managing channel subscriptions.
///
/// The `Subscription` provides methods for subscribing to channels,
/// receiving publications, and managing the subscription lifecycle.
/// It acts as a handle to the internal subscription implementation.
#[derive(Clone)]
pub struct Subscription {
    /// Unique identifier for this subscription
    pub(crate) id: SubscriptionId,
    /// Reference to the client that owns this subscription
    client: Client,
}

impl Subscription {
    /// Creates a new subscription instance.
    ///
    /// This method is called internally by the client when creating
    /// new subscriptions.
    ///
    /// # Arguments
    ///
    /// * `client` - Reference to the client that owns this subscription
    /// * `key` - Unique identifier for the subscription
    pub(crate) fn new(client: &Client, key: SubscriptionId) -> Self {
        Subscription {
            id: key,
            client: client.clone(),
        }
    }

    /// Subscribes to the channel.
    ///
    /// This method initiates the subscription process and returns a future
    /// that resolves when the subscription is established or fails.
    ///
    /// # Returns
    ///
    /// A `FutureResult` containing a future that resolves to `Ok(())` on
    /// successful subscription or `Err(())` on failure.
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
    ///     // let result = subscription.subscribe().await;
    ///     // match result {
    ///     //     Ok(()) => println!("Subscribed to news channel"),
    ///     //     Err(()) => println!("Failed to subscribe"),
    ///     // }
    ///     // Ok(())
    /// // }
    /// ```
    pub fn subscribe(&self) -> FutureResult<impl Future<Output = Result<(), ()>>> {
        let (tx, rx) = oneshot::channel();
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            if sub.state != State::Unsubscribed {
                let _ = tx.send(Err(()));
            } else {
                sub.on_subscribed_ch.push(tx);
                sub.move_to_subscribing();
                if let Some(channel) = inner.sub_ch_write.as_ref() {
                    let _ = channel.send(self.id);
                }
            }
        } else {
            let _ = tx.send(Err(()));
        }
        FutureResult(async {
            match rx.await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(())) => Err(()),
                Err(_) => Err(()),
            }
        })
    }

    /// Unsubscribes from the channel.
    ///
    /// This method removes the subscription and returns a future that
    /// resolves when the unsubscription is complete.
    ///
    /// # Returns
    ///
    /// A `FutureResult` containing a future that resolves when unsubscribed.
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
    ///     // subscription.unsubscribe().await; // Wait for unsubscription
    ///     // Ok(())
    /// // }
    /// ```
    pub fn unsubscribe(&self) -> FutureResult<impl Future<Output = ()>> {
        let (tx, rx) = oneshot::channel();
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            if sub.state == State::Unsubscribed {
                let _ = tx.send(());
            } else {
                sub.on_unsubscribed_ch.push(tx);
                sub.move_to_unsubscribed();
                if let Some(channel) = inner.sub_ch_write.as_ref() {
                    let _ = channel.send(self.id);
                }
            }
        } else {
            let _ = tx.send(());
        }
        FutureResult(async {
            let _ = rx.await;
        })
    }

    /// Publishes a message to the subscribed channel.
    ///
    /// This method sends a message to the channel that this subscription
    /// is listening to. The message will be queued if not connected and
    /// sent when the connection is established.
    ///
    /// # Arguments
    ///
    /// * `data` - The message data as bytes
    ///
    /// # Returns
    ///
    /// A `FutureResult` containing a future that resolves to `Ok(())` on
    /// successful publication or an error on failure.
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
    ///     // let publish_future = subscription.publish(b"Hello, channel!".to_vec());
    ///     // let result = publish_future.await;
    ///     // match result {
    ///     //     Ok(()) => println!("Message published successfully"),
    ///     //     Err(err) => println!("Failed to publish: {}", err),
    ///     // }
    ///     // Ok(())
    /// // }
    /// ```
    pub fn publish(
        &self,
        data: Vec<u8>,
    ) -> FutureResult<impl Future<Output = Result<(), RequestError>>> {
        let mut inner = self.client.0.lock().unwrap();
        let read_timeout = inner.read_timeout;
        let deadline = Instant::now() + read_timeout;
        let rx = if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            if let Some(ref mut pub_ch_write) = sub.pub_ch_write {
                pub_ch_write.send(Command::Publish(PublishRequest {
                    channel: (*sub.channel).to_owned(),
                    data,
                }))
            } else {
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(Err(ReplyError::Closed));
                rx
            }
        } else {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Err(ReplyError::Closed));
            rx
        };
        FutureResult(async move {
            let result = tokio::time::timeout_at(deadline.into(), rx).await;
            match result {
                Ok(Ok(Ok(Reply::Publish(_)))) => Ok(()),
                Ok(Ok(Ok(Reply::Error(err)))) => Err(RequestError::ErrorResponse(err)),
                Ok(Ok(Ok(reply))) => Err(RequestError::UnexpectedReply(reply)),
                Ok(Ok(Err(err))) => Err(err.into()),
                Ok(Err(err)) => Err(err.into()),
                Err(err) => Err(err.into()),
            }
        })
    }

    /// Sets a callback for when the subscription starts.
    ///
    /// This callback is called whenever the subscription begins the
    /// subscription process.
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
    /// // let subscription = client.new_subscription("news");
    ///
    /// // subscription.on_subscribing(|| {
    /// //     println!("Starting subscription to news channel");
    /// // });
    /// ```
    pub fn on_subscribing(&self, func: impl FnMut() + Send + 'static) {
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            sub.on_subscribing = Some(Box::new(func));
        }
    }

    /// Sets a callback for when the subscription is established.
    ///
    /// This callback is called whenever the subscription is successfully
    /// established and is ready to receive publications.
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
    /// // let subscription = client.new_subscription("news");
    ///
    /// // subscription.on_subscribed(|| {
    /// //     println!("Successfully subscribed to news channel");
    /// // });
    /// ```
    pub fn on_subscribed(&self, func: impl FnMut() + Send + 'static) {
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            sub.on_subscribed = Some(Box::new(func));
        }
    }

    /// Sets a callback for when the subscription is removed.
    ///
    /// This callback is called whenever the subscription is unsubscribed
    /// or removed from the client.
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
    /// // let subscription = client.new_subscription("news");
    /// // subscription.on_unsubscribed(|| {
    /// //     println!("Unsubscribed from news channel");
    /// // });
    /// ```
    pub fn on_unsubscribed(&self, func: impl FnMut() + Send + 'static) {
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            sub.on_unsubscribed = Some(Box::new(func));
        }
    }

    /// Sets a callback for incoming publications.
    ///
    /// This callback is called whenever a new publication is received
    /// on the subscribed channel.
    ///
    /// # Arguments
    ///
    /// * `func` - The callback function to execute with the publication data
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
    /// // subscription.on_publication(|pub_data| {
    /// //     println!("Received publication: {:?}", pub_data);
    /// // });
    /// ```
    pub fn on_publication(&self, func: impl FnMut(Publication) + Send + 'static) {
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            sub.on_publication = Some(Box::new(func));
        }
    }

    /// Sets a callback for subscription errors.
    ///
    /// This callback is called whenever an error occurs during
    /// subscription operations.
    ///
    /// # Arguments
    ///
    /// * `func` - The callback function to execute with the error
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
    /// // subscription.on_error(|err| {
    /// //     eprintln!("Subscription error: {}", err);
    /// // });
    /// ```
    pub fn on_error(&self, func: impl FnMut(anyhow::Error) + Send + 'static) {
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            sub.on_error = Some(Box::new(func));
        }
    }

    /// Returns the current state of the subscription.
    ///
    /// # Returns
    ///
    /// The current `State` of the subscription.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tokio_centrifuge::client::Client;
    /// use tokio_centrifuge::config::Config;
    /// use tokio_centrifuge::subscription::State;
    /// 
    /// let config = Config::new().use_json();
    /// // let client = Client::new("ws://localhost:8000/connection/websocket", config);
    /// 
    /// // let subscription = client.new_subscription("news");
    /// // match subscription.state() {
    /// //     State::Unsubscribed => println!("Not subscribed"),
    /// //     State::Subscribing => println!("Subscribing..."),
    /// //     State::Subscribed => println!("Subscribed!"),
    /// // }
    /// ```
    pub fn state(&self) -> State {
        let inner = self.client.0.lock().unwrap();
        inner
            .subscriptions
            .get(self.id)
            .map(|s| s.state)
            .unwrap_or(State::Unsubscribed)
    }
}
