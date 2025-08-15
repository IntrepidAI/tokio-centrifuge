//! # Callbacks Module
//!
//! This module defines the callback function types used throughout the server.
//! These types provide a flexible way to handle various server events and
//! customize server behavior.
//!
//! ## Callback Types
//!
//! - **OnConnectFn**: Handles new client connections
//! - **OnDisconnectFn**: Handles client disconnections
//! - **RpcMethodFn**: Handles RPC method calls
//! - **PublishFn**: Handles client publications
//! - **ChannelFn**: Creates subscription streams

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::errors::{ClientError, DisconnectErrorCode};
use crate::server::types::{
    ConnectContext, DisconnectContext, PublishContext, RpcContext, SubContext,
};
use futures::Stream;

/// Function type for handling client connection requests
///
/// This callback is called when a client attempts to connect to the server.
/// It can perform authentication, validation, or any other connection-time logic.
///
/// ## Arguments
///
/// * `ctx` - Connection context containing client information
///
/// ## Returns
///
/// * `Ok(())` - Connection is allowed
/// * `Err(DisconnectErrorCode)` - Connection is rejected with specific error
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::callbacks::OnConnectFn;
/// use tokio_centrifuge::server::types::ConnectContext;
/// use std::sync::Arc;
///
/// let connect_handler: OnConnectFn = Arc::new(|ctx: ConnectContext| {
///     Box::pin(async move {
///         if ctx.token.is_empty() {
///             return Err(tokio_centrifuge::errors::DisconnectErrorCode::InvalidToken);
///         }
///         Ok(())
///     })
/// });
/// ```
pub type OnConnectFn = Arc<
    dyn Fn(ConnectContext) -> Pin<Box<dyn Future<Output = Result<(), DisconnectErrorCode>> + Send>>
        + Send
        + Sync,
>;

/// Function type for handling client publications to channels
///
/// This callback is called when a client publishes data to a channel.
/// It can process the data, store it, or forward it to other clients.
///
/// ## Arguments
///
/// * `ctx` - Publication context containing client and channel information
/// * `data` - Raw data published by the client
///
/// ## Returns
///
/// * `Ok(())` - Publication was processed successfully
/// * `Err(ClientError)` - Publication failed with specific error
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::callbacks::PublishFn;
/// use tokio_centrifuge::server::types::PublishContext;
/// use std::sync::Arc;
///
/// let publish_handler: PublishFn = Arc::new(|ctx: PublishContext, data: Vec<u8>| {
///     Box::pin(async move {
///         println!("Client {} published to channel: {:?}", ctx.client_id, data);
///         Ok(())
///     })
/// });
/// ```
pub type PublishFn = Arc<
    dyn Fn(PublishContext, Vec<u8>) -> Pin<Box<dyn Future<Output = Result<(), ClientError>> + Send>>
        + Send
        + Sync,
>;

/// Function type for handling client disconnections
///
/// This callback is called when a client disconnects from the server.
/// It can be used for cleanup, logging, analytics, or resource management.
///
/// ## Arguments
///
/// * `ctx` - Disconnection context containing client information
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::callbacks::OnDisconnectFn;
/// use tokio_centrifuge::server::types::DisconnectContext;
/// use std::sync::Arc;
///
/// let disconnect_handler: OnDisconnectFn = Arc::new(|ctx: DisconnectContext| {
///     println!("Client {} disconnected", ctx.client_id);
///     // Clean up resources, log analytics, etc.
/// });
/// ```
pub type OnDisconnectFn = Arc<dyn Fn(DisconnectContext) + Send + Sync>;

/// Function type for handling RPC method calls
///
/// This callback is called when a client invokes an RPC method.
/// It can perform any server-side logic and return data to the client.
///
/// ## Arguments
///
/// * `ctx` - RPC context containing client and method information
/// * `data` - Raw data sent with the RPC call
///
/// ## Returns
///
/// * `Ok(Vec<u8>)` - RPC response data
/// * `Err(ClientError)` - RPC call failed with specific error
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::callbacks::RpcMethodFn;
/// use tokio_centrifuge::server::types::RpcContext;
/// use std::sync::Arc;
///
/// let rpc_handler: RpcMethodFn = Arc::new(|ctx: RpcContext, data: Vec<u8>| {
///     Box::pin(async move {
///         let user_id = ctx.params.get("user_id");
///         println!("RPC call from client {} for user {:?}", ctx.client_id, user_id);
///         
///         // Process RPC call and return response
///         Ok(b"RPC response".to_vec())
///     })
/// });
/// ```
pub type RpcMethodFn = Arc<
    dyn Fn(
            RpcContext,
            Vec<u8>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ClientError>> + Send>>
        + Send
        + Sync,
>;

/// Function type for creating subscription streams
///
/// This callback is called when a client subscribes to a channel.
/// It returns a stream that the client will receive data from.
///
/// ## Arguments
///
/// * `ctx` - Subscription context containing client and channel information
///
/// ## Returns
///
/// * `Ok(Stream)` - Stream of data for the subscription
/// * `Err(ClientError)` - Subscription failed with specific error
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::callbacks::ChannelFn;
/// use tokio_centrifuge::server::types::SubContext;
/// use futures::stream::Stream;
/// use std::sync::Arc;
/// use std::pin::Pin;
///
/// let channel_handler: ChannelFn = Arc::new(|ctx: SubContext| {
///     Box::pin(async move {
///         let room_id = ctx.params.get("room_id");
///         println!("Subscription request from client {} for room {:?}",
///                  ctx.client_id, room_id);
///         
///         // Create and return stream for the channel (implementation would go here)
///         // let room_stream = create_room_stream(room_id);
///         
///         Err(tokio_centrifuge::errors::ClientError::internal("Stream not implemented"))
///     })
/// });
/// ```
pub type ChannelFn = Arc<
    dyn Fn(
            SubContext,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>, ClientError>,
                    > + Send,
            >,
        > + Send
        + Sync,
>;

/// Wrapper for connection handler functions
///
/// This struct provides a default connection handler that accepts
/// all connections without authentication. It can be replaced with
/// a custom handler using the `Server::on_connect` method.
///
/// ## Default Behavior
///
/// The default handler accepts all connections where the token is empty.
/// This is useful for development and testing, but should be replaced
/// with proper authentication in production.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::callbacks::ConnectFnWrapper;
///
/// let wrapper = ConnectFnWrapper::default();
/// // Use wrapper.0 to access the underlying function
/// ```
#[derive(Clone)]
pub struct ConnectFnWrapper(pub OnConnectFn);

impl Default for ConnectFnWrapper {
    /// Creates a default connection handler
    ///
    /// The default handler accepts all connections where the token is empty.
    /// This is equivalent to no authentication.
    fn default() -> Self {
        fn default_connect_fn(
            ctx: ConnectContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), DisconnectErrorCode>> + Send>> {
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
