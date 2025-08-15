//! # Server Module
//!
//! This module provides the core server functionality for tokio-centrifuge.
//! It includes WebSocket server implementation, session management, and
//! various callback mechanisms for handling client connections.
//!
//! ## Features
//!
//! - **WebSocket Server**: Full WebSocket server implementation with ping/pong support
//! - **Session Management**: Client session handling with state management
//! - **RPC Methods**: Support for custom RPC method registration
//! - **Channels**: Pub/sub channel support with streaming capabilities
//! - **Authentication**: Configurable connection authentication
//!
//! ## Example
//!
//! ```rust
//! use tokio_centrifuge::server::{Server, types::ConnectContext};
//!
//! let mut server = Server::new();
//!
//! server.on_connect(async move |ctx: ConnectContext| {
//!     // Handle client connection
//!     Ok(())
//! });
//!
//! server.add_rpc_method("hello", async move |ctx, data| {
//!     // Handle RPC calls
//!     Ok(b"Hello, World!".to_vec())
//! });
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures::Sink;
use futures::Stream;
use matchit::{InsertError, Router};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::server::callbacks::{
    ChannelFn, ConnectFnWrapper, OnDisconnectFn, PublishFn, RpcMethodFn,
};
use crate::server::types::ServeParams;
use crate::server::websocket::serve_websocket;

pub mod callbacks;
pub mod session;
pub mod types;
pub mod websocket;

// Re-export commonly used types
pub use session::SessionState;

/// Main server instance for handling WebSocket connections
///
/// This struct provides methods to configure and start a WebSocket server
/// that can handle Centrifugo protocol connections.
///
/// ## Features
///
/// - **Connection Callbacks**: Handle client connect/disconnect events
/// - **RPC Methods**: Register custom RPC methods with URL parameters
/// - **Publication Channels**: Handle client publications to channels
/// - **Subscription Channels**: Create streaming channels for client subscriptions
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::Server;
///
/// let mut server = Server::new();
///
/// // Configure connection handling
/// server.on_connect(|ctx| async move {
///     println!("Client connected: {}", ctx.client_id);
///     Ok(())
/// });
///
/// // Add RPC method
/// server.add_rpc_method("hello/{name}", |ctx, data| async move {
///     let name = ctx.params.get("name").map_or("World", |v| v);
///     Ok(format!("Hello, {}!", name).into_bytes())
/// });
/// ```
#[derive(Default, Clone)]
pub struct Server {
    on_connect: ConnectFnWrapper,
    on_disconnect: Option<OnDisconnectFn>,
    rpc_methods: Arc<Mutex<Router<RpcMethodFn>>>,
    sub_channels: Arc<Mutex<Router<ChannelFn>>>,
    pub_channels: Arc<Mutex<Router<PublishFn>>>,
}

impl Server {
    /// Creates a new server instance with default configuration
    ///
    /// The server starts with:
    /// - Default connection handler (accepts all connections)
    /// - No disconnect handler
    /// - Empty RPC methods, publication channels, and subscription channels
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::Server;
    ///
    /// let server = Server::new();
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the connection handler for new client connections
    ///
    /// This handler is called when a client attempts to connect.
    /// It can perform authentication, validation, or any other
    /// connection-time logic.
    ///
    /// ## Arguments
    ///
    /// * `f` - Async function that handles connection requests
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::{Server, types::ConnectContext};
    /// use tokio_centrifuge::errors::DisconnectErrorCode;
    ///
    /// let mut server = Server::new();
    ///
    /// server.on_connect(|ctx: ConnectContext| async move {
    ///     if ctx.token.is_empty() {
    ///         return Err(DisconnectErrorCode::InvalidToken);
    ///     }
    ///     Ok(())
    /// });
    /// ```
    pub fn on_connect<Fut>(
        &mut self,
        f: impl Fn(crate::server::types::ConnectContext) -> Fut + Send + Sync + 'static,
    ) where
        Fut: std::future::Future<Output = Result<(), crate::errors::DisconnectErrorCode>>
            + Send
            + 'static,
    {
        self.on_connect = ConnectFnWrapper(Arc::new(
            move |ctx: crate::server::types::ConnectContext| {
                Box::pin(f(ctx))
                    as Pin<
                        Box<
                            dyn Future<Output = Result<(), crate::errors::DisconnectErrorCode>>
                                + Send,
                        >,
                    >
            },
        ));
    }

    /// Sets the disconnect handler for client disconnections
    ///
    /// This handler is called when a client disconnects from the server.
    /// It can be used for cleanup, logging, or analytics.
    ///
    /// ## Arguments
    ///
    /// * `f` - Function that handles client disconnection
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::{Server, types::DisconnectContext};
    ///
    /// let mut server = Server::new();
    ///
    /// server.on_disconnect(|ctx: DisconnectContext| {
    ///     println!("Client disconnected: {}", ctx.client_id);
    /// });
    /// ```
    pub fn on_disconnect(
        &mut self,
        f: impl Fn(crate::server::types::DisconnectContext) + Send + Sync + 'static,
    ) {
        self.on_disconnect = Some(Arc::new(f));
    }

    /// Adds a custom RPC method that clients can call
    ///
    /// RPC methods support URL parameters in the format `{param_name}`.
    /// These parameters are extracted and passed to the handler function.
    ///
    /// ## Arguments
    ///
    /// * `name` - Method name with optional URL parameters (e.g., "user/{id}")
    /// * `f` - Async function that handles RPC calls
    ///
    /// ## Returns
    ///
    /// `Result<(), InsertError>` - Success or error if method name is invalid
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::Server;
    ///
    /// let server = Server::new();
    ///
    /// server.add_rpc_method("user/{id}", |ctx, data| async move {
    ///     let user_id = ctx.params.get("id").map_or("unknown", |v| v);
    ///     Ok(format!("User ID: {}", user_id).into_bytes())
    /// }).unwrap();
    /// ```
    pub fn add_rpc_method<Fut>(
        &self,
        name: &str,
        f: impl Fn(crate::server::types::RpcContext, Vec<u8>) -> Fut + Send + Sync + 'static,
    ) -> Result<(), InsertError>
    where
        Fut: std::future::Future<Output = Result<Vec<u8>, crate::errors::ClientError>>
            + Send
            + 'static,
    {
        let wrap_f: RpcMethodFn = Arc::new(
            move |ctx: crate::server::types::RpcContext, data: Vec<u8>| Box::pin(f(ctx, data)),
        );
        self.rpc_methods
            .lock()
            .unwrap()
            .insert(name.to_string(), wrap_f)
    }

    /// Registers a publication channel handler
    ///
    /// Publication channels allow clients to publish data to the server.
    /// The handler receives the published data and can process it accordingly.
    ///
    /// ## Arguments
    ///
    /// * `name` - Channel name with optional URL parameters
    /// * `f` - Async function that handles publications
    ///
    /// ## Returns
    ///
    /// `Result<(), InsertError>` - Success or error if channel name is invalid
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::Server;
    ///
    /// let server = Server::new();
    ///
    /// server.on_publication("chat/{room}", |ctx, data| async move {
    ///     let room = ctx.params.get("room").map_or("general", |v| v);
    ///     println!("Message in room {}: {:?}", room, data);
    ///     Ok(())
    /// }).unwrap();
    /// ```
    pub fn on_publication<Fut>(
        &self,
        name: &str,
        f: impl Fn(crate::server::types::PublishContext, Vec<u8>) -> Fut + Send + Sync + 'static,
    ) -> Result<(), InsertError>
    where
        Fut: std::future::Future<Output = Result<(), crate::errors::ClientError>> + Send + 'static,
    {
        let wrap_f: PublishFn = Arc::new(
            move |ctx: crate::server::types::PublishContext, data: Vec<u8>| Box::pin(f(ctx, data)),
        );
        self.pub_channels
            .lock()
            .unwrap()
            .insert(name.to_string(), wrap_f)
    }

    /// Adds a subscription channel that clients can subscribe to
    ///
    /// Subscription channels provide streaming data to clients.
    /// The handler returns a stream that clients receive data from.
    ///
    /// ## Arguments
    ///
    /// * `name` - Channel name with optional URL parameters
    /// * `f` - Async function that returns a stream for the channel
    ///
    /// ## Returns
    ///
    /// `Result<(), InsertError>` - Success or error if channel name is invalid
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::Server;
    /// use futures::stream::Stream;
    ///
    /// let server = Server::new();
    ///
    /// // Example would require a concrete stream type
    /// // server.add_channel("updates/{user_id}", |ctx| async move {
    /// //     // Return a stream of updates for the user (implementation would go here)
    /// //     // let updates_stream = create_updates_stream(ctx.user_id);
    /// //     
    /// //     Ok(updates_stream)
    /// // }).unwrap();
    /// ```
    pub fn add_channel<Fut, St>(
        &self,
        name: &str,
        f: impl Fn(crate::server::types::SubContext) -> Fut + Send + Sync + 'static,
    ) -> Result<(), InsertError>
    where
        Fut: std::future::Future<Output = Result<St, crate::errors::ClientError>> + Send + 'static,
        St: Stream<Item = Vec<u8>> + Send + 'static,
    {
        let f = Arc::new(f);
        let wrap_f: ChannelFn = Arc::new(move |ctx: crate::server::types::SubContext| {
            let f = f.clone();
            Box::pin(async move {
                Ok(Box::pin(f(ctx).await?) as Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>)
            })
        });
        self.sub_channels
            .lock()
            .unwrap()
            .insert(name.to_string(), wrap_f)
    }

    /// Starts serving WebSocket connections on the given stream
    ///
    /// This method takes ownership of the stream and starts handling
    /// WebSocket connections according to the server configuration.
    ///
    /// ## Arguments
    ///
    /// * `stream` - WebSocket stream to serve connections on
    /// * `params` - Server parameters including encoding and ping settings
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::{Server, types::ServeParams};
    /// use tokio_tungstenite::accept_async;
    ///
    /// // In a real implementation, you would have these handlers
    /// // let server = Server::new();
    /// // let params = ServeParams::json();
    ///
    /// // Accept WebSocket connection (in async context)
    /// // let (ws_stream, _) = accept_async(request).await?;
    ///
    /// // Serve the connection (in async context)
    /// // server.serve(ws_stream, params).await;
    /// ```
    pub async fn serve<S>(&self, stream: S, params: ServeParams)
    where
        S: Stream<Item = Result<Message, WsError>> + Sink<Message> + Send + Unpin + 'static,
    {
        serve_websocket(
            stream,
            params,
            self.on_connect.0.clone(),
            self.on_disconnect.clone(),
            self.rpc_methods.clone(),
            self.sub_channels.clone(),
            self.pub_channels.clone(),
        )
        .await;
    }
}
