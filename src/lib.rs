//! # tokio-centrifuge
//!
//! Unofficial Rust Client SDK for Centrifugo - a scalable real-time messaging server.
//!
//! ## Overview
//!
//! `tokio-centrifuge` provides a comprehensive Rust implementation for building
//! real-time applications using the Centrifugo protocol. It supports both client
//! and server implementations with WebSocket transport.
//!
//! ## Features
//!
//! - **WebSocket Transport**: Full WebSocket support with ping/pong health checks
//! - **Protocol Support**: JSON and Protobuf encoding formats
//! - **Client SDK**: Easy-to-use client for connecting to Centrifugo servers
//! - **Server Implementation**: Full server implementation for custom real-time services
//! - **RPC Methods**: Support for custom RPC method registration
//! - **Pub/Sub Channels**: Publication and subscription channel support
//! - **Authentication**: Configurable connection authentication
//! - **Async/Await**: Built on tokio for high-performance async operations
//!
//! ## Quick Start
//!
//! ### Client Usage
//!
//! ```rust
//! use tokio_centrifuge::client::Client;
//! use tokio_centrifuge::config::Protocol;
//!
//! // Create client configuration
//! let config = tokio_centrifuge::config::Config::new().use_json();
//! // let mut client = Client::new("ws://localhost:8000/connection/websocket", config);
//!
//! // Connect and subscribe (in async context)
//! // let mut subscription = client.subscribe("news").await?;
//! ```
//!
//! ### Server Usage
//!
//! ```rust
//! use tokio_centrifuge::server::{Server, types::ConnectContext};
//! use tokio_centrifuge::server::types::ServeParams;
//!
//! let mut server = Server::new();
//!
//! // Handle connections
//! server.on_connect(|ctx: ConnectContext| async move {
//!     println!("Client connected: {}", ctx.client_id);
//!     Ok(())
//! });
//!
//! // Add RPC method
//! server.add_rpc_method("hello", |ctx, data| async move {
//!     Ok(b"Hello, World!".to_vec())
//! }).unwrap();
//! ```
//!
//! ## Architecture
//!
//! The library is organized into several key modules:
//!
//! - **`client`**: Client implementation for connecting to Centrifugo servers
//! - **`server`**: Server implementation for building custom real-time services
//! - **`protocol`**: Protocol definitions and message handling
//! - **`config`**: Configuration types and protocol settings
//! - **`errors`**: Error types and error handling
//! - **`utils`**: Utility functions for encoding/decoding
//!
//! ## Protocol Support
//!
//! `tokio-centrifuge` implements the Centrifugo protocol, which includes:
//!
//! - **Connection Management**: Connect, disconnect, and ping/pong
//! - **RPC Calls**: Remote procedure calls with custom methods
//! - **Publications**: Publishing data to channels
//! - **Subscriptions**: Subscribing to channels for real-time updates
//! - **Error Handling**: Comprehensive error codes and messages
//!
//! ## Examples
//!
//! Check the `examples/` directory for complete working examples:
//!
//! - **`pubsub`**: Basic pub/sub functionality demonstration
//! - **`server`**: Custom server implementation example
//!
//! ## Dependencies
//!
//! - **tokio**: Async runtime
//! - **tokio-tungstenite**: WebSocket implementation
//! - **serde**: Serialization/deserialization
//! - **futures**: Async stream utilities
//! - **uuid**: Client identification
//!
//! ## License
//!
//! This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
//!
//! ## Contributing
//!
//! Contributions are welcome! Please feel free to submit a Pull Request.

pub mod client;
pub mod client_handler;
pub mod config;
pub mod errors;
pub mod protocol;
pub mod server;
pub mod subscription;
pub mod utils;

// Server::serve requires tungstenite::Message, so we should
// re-export it to make sure user has the same version.
pub use tokio_tungstenite::tungstenite;
