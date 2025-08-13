//! # Types Module
//!
//! This module contains all the core types and structures used by the server.
//! It provides client identification, connection parameters, and various
//! context structures for different operations.
//!
//! ## Core Types
//!
//! - **ClientId**: Unique identifier for each client connection
//! - **ServeParams**: Configuration parameters for server behavior
//! - **Context Structs**: Information passed to various handlers

use std::fmt::Display;
use uuid::Uuid;

use crate::config::Protocol;

/// Unique identifier for a client connection
///
/// This is a wrapper around a UUID that provides a human-readable
/// string representation and implements common traits for easy use.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::types::ClientId;
///
/// let client_id = ClientId::new();
/// println!("Client ID: {}", client_id);
///
/// // Use as HashMap key
/// let mut clients = std::collections::HashMap::new();
/// clients.insert(client_id, "client_data");
/// ```
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ClientId(Uuid);

impl ClientId {
    /// Creates a new random client ID
    ///
    /// This generates a new UUID v4 for the client.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::types::ClientId;
    ///
    /// let id = ClientId::new();
    /// assert_ne!(id, ClientId::new()); // Each ID is unique
    /// ```
    pub fn new() -> Self {
        Default::default()
    }
}

impl Default for ClientId {
    /// Creates a default client ID (same as `new()`)
    fn default() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Display for ClientId {
    /// Formats the client ID as a hyphenated UUID string
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::types::ClientId;
    ///
    /// let id = ClientId::new();
    /// let formatted = format!("{}", id);
    /// assert!(formatted.contains('-'));
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

/// Configuration parameters for server behavior
///
/// This struct controls various aspects of how the server operates,
/// including encoding format, ping intervals, and client identification.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::types::ServeParams;
/// use tokio_centrifuge::config::Protocol;
/// use std::time::Duration;
///
/// // Create JSON server with custom ping settings
/// let params = ServeParams::json()
///     .with_client_id(tokio_centrifuge::server::types::ClientId::new())
///     .without_ping();
///
/// // Create Protobuf server with default settings
/// let params = ServeParams::protobuf();
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ServeParams {
    /// Optional client ID to assign to this connection
    ///
    /// If `None`, a new random ID will be generated.
    pub client_id: Option<ClientId>,

    /// Protocol encoding format for messages
    pub encoding: Protocol,

    /// Interval between ping messages (None to disable)
    ///
    /// Default is 25 seconds.
    pub ping_interval: Option<std::time::Duration>,

    /// Timeout for pong responses (None to disable)
    ///
    /// Default is 10 seconds.
    pub pong_timeout: Option<std::time::Duration>,
}

impl ServeParams {
    /// Creates server parameters configured for JSON encoding
    ///
    /// Uses default ping/pong settings and no specific client ID.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::types::ServeParams;
    /// use tokio_centrifuge::config::Protocol;
    ///
    /// let params = ServeParams::json();
    /// assert_eq!(params.encoding, Protocol::Json);
    /// ```
    pub fn json() -> Self {
        Self {
            encoding: Protocol::Json,
            ..Default::default()
        }
    }

    /// Creates server parameters configured for Protobuf encoding
    ///
    /// Uses default ping/pong settings and no specific client ID.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::types::ServeParams;
    /// use tokio_centrifuge::config::Protocol;
    ///
    /// let params = ServeParams::protobuf();
    /// assert_eq!(params.encoding, Protocol::Protobuf);
    /// ```
    pub fn protobuf() -> Self {
        Self {
            encoding: Protocol::Protobuf,
            ..Default::default()
        }
    }

    /// Sets a specific client ID for this connection
    ///
    /// ## Arguments
    ///
    /// * `client_id` - The client ID to assign
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::types::{ServeParams, ClientId};
    ///
    /// let client_id = ClientId::new();
    /// let params = ServeParams::json().with_client_id(client_id);
    /// assert_eq!(params.client_id, Some(client_id));
    /// ```
    pub fn with_client_id(self, client_id: ClientId) -> Self {
        Self {
            client_id: Some(client_id),
            ..self
        }
    }

    /// Disables ping/pong functionality for this connection
    ///
    /// This is useful for testing or when ping/pong is not needed.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::types::ServeParams;
    ///
    /// let params = ServeParams::json().without_ping();
    /// assert_eq!(params.ping_interval, None);
    /// assert_eq!(params.pong_timeout, None);
    /// ```
    pub fn without_ping(self) -> Self {
        Self {
            ping_interval: None,
            pong_timeout: None,
            ..self
        }
    }
}

impl Default for ServeParams {
    /// Creates default server parameters
    ///
    /// Defaults:
    /// - JSON encoding
    /// - 25 second ping interval
    /// - 10 second pong timeout
    /// - No specific client ID
    fn default() -> Self {
        Self {
            client_id: None,
            encoding: Protocol::Json,
            ping_interval: Some(std::time::Duration::from_secs(25)),
            pong_timeout: Some(std::time::Duration::from_secs(10)),
        }
    }
}

impl From<Protocol> for ServeParams {
    /// Creates server parameters from a protocol
    ///
    /// Uses the specified protocol with default ping/pong settings.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::server::types::ServeParams;
    /// use tokio_centrifuge::config::Protocol;
    ///
    /// let params: ServeParams = Protocol::Protobuf.into();
    /// assert_eq!(params.encoding, Protocol::Protobuf);
    /// ```
    fn from(protocol: Protocol) -> Self {
        Self {
            encoding: protocol,
            ..Default::default()
        }
    }
}

/// Context information for client connection events
///
/// This struct contains all the information available when a client
/// attempts to connect to the server.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::types::ConnectContext;
///
/// async fn handle_connect(ctx: ConnectContext) -> Result<(), ()> {
///     println!("Client {} connecting with version {}",
///              ctx.client_id, ctx.client_version);
///     
///     if !ctx.token.is_empty() {
///         // Validate token
///         return Ok(());
///     }
///     
///     Err(())
/// }
/// ```
#[derive(Debug)]
pub struct ConnectContext {
    /// Unique identifier for the client
    pub client_id: ClientId,

    /// Human-readable name of the client
    pub client_name: String,

    /// Version string of the client
    pub client_version: String,

    /// Authentication token provided by the client
    pub token: String,

    /// Additional connection data
    pub data: Vec<u8>,
}

/// Context information for client disconnection events
///
/// This struct contains information about a client that has
/// disconnected from the server.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::types::DisconnectContext;
///
/// fn handle_disconnect(ctx: DisconnectContext) {
///     println!("Client {} disconnected", ctx.client_id);
///     // Clean up resources, log analytics, etc.
/// }
/// ```
#[derive(Debug)]
pub struct DisconnectContext {
    /// ID of the disconnected client
    pub client_id: ClientId,
}

/// Context information for RPC method calls
///
/// This struct contains information about an RPC method call,
/// including the client ID, encoding format, and URL parameters.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::types::RpcContext;
///
/// async fn handle_rpc(ctx: RpcContext, data: Vec<u8>) -> Result<Vec<u8>, ()> {
///     let user_id = ctx.params.get("user_id");
///     println!("RPC call from client {} for user {:?}", ctx.client_id, user_id);
///     
///     // Process RPC call
///     Ok(b"RPC response".to_vec())
/// }
/// ```
#[derive(Debug)]
pub struct RpcContext {
    /// Protocol encoding format for this request
    pub encoding: Protocol,

    /// ID of the client making the request
    pub client_id: ClientId,

    /// URL parameters extracted from the method name
    pub params: std::collections::HashMap<String, String>,
}

/// Context information for subscription requests
///
/// This struct contains information about a client's subscription
/// request to a channel, including the subscription data.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::types::SubContext;
///
/// async fn handle_subscription(ctx: SubContext) -> Result<Box<dyn futures::Stream<Item = Vec<u8>> + Send>, ()> {
///     let room_id = ctx.params.get("room_id");
///     println!("Subscription request from client {} for room {:?}",
///              ctx.client_id, room_id);
///     
///     // Create and return stream for the channel
///     // let room_stream = create_room_stream(room_id);
///     
///     Err(()) // Placeholder - would return actual stream
/// }
/// ```
#[derive(Debug)]
pub struct SubContext {
    /// Protocol encoding format for this subscription
    pub encoding: Protocol,

    /// ID of the client requesting the subscription
    pub client_id: ClientId,

    /// URL parameters extracted from the channel name
    pub params: std::collections::HashMap<String, String>,

    /// Subscription data provided by the client
    pub data: Vec<u8>,
}

/// Context information for publication requests
///
/// This struct contains information about a client's publication
/// to a channel.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::server::types::PublishContext;
///
/// async fn handle_publication(ctx: PublishContext, data: Vec<u8>) -> Result<(), ()> {
///     let room_id = ctx.params.get("room_id");
///     println!("Publication from client {} to room {:?}: {:?}",
///              ctx.client_id, room_id, data);
///     
///     // Process the publication
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct PublishContext {
    /// Protocol encoding format for this publication
    pub encoding: Protocol,

    /// ID of the client making the publication
    pub client_id: ClientId,

    /// URL parameters extracted from the channel name
    pub params: std::collections::HashMap<String, String>,
}
