//! Client error types and result handling.
//!
//! This module defines the error types used by the client implementation
//! and provides utilities for handling asynchronous results.

use std::future::Future;
use std::future::IntoFuture;
use thiserror::Error;

/// A wrapper around futures that can be polled to get results.
///
/// This is a regular future which you can poll to get result,
/// but it's totally fine to drop it if you don't need results.
///
/// # Example
///
/// ```rust
/// use tokio_centrifuge::client::{Client, FutureResult};
/// use tokio_centrifuge::config::Config;
/// 
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     let config = Config::new().use_json();
///     // let client = Client::new("ws://localhost:8000/connection/websocket", config);
///     
///     // let future_result = client.connect();
///     // You can either await it directly
///     // let result = future_result.await;
///     Ok(())
/// }
/// ```
/// 
/// Or convert to a future and await later:
/// 
/// ```rust
/// use tokio_centrifuge::client::{Client, FutureResult};
/// use tokio_centrifuge::config::Config;
/// 
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///     let config = Config::new().use_json();
///     // let client = Client::new("ws://localhost:8000/connection/websocket", config);
///     
///     // let future_result = client.connect();
///     // Convert to a future and await later
///     // let future = future_result.into_future();
///     // let result = future.await;
///     Ok(())
/// }
/// ```
pub struct FutureResult<T>(pub(crate) T);

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

/// Errors that can occur when making requests to the server.
///
/// This enum covers various failure scenarios including server errors,
/// network issues, and protocol violations.
#[derive(Error, Debug)]
pub enum RequestError {
    /// The server returned an error response
    ErrorResponse(crate::protocol::Error),

    /// The server returned an unexpected reply type
    UnexpectedReply(crate::protocol::Reply),

    /// An error occurred while processing the reply
    ReplyError(#[from] crate::client_handler::ReplyError),

    /// The request timed out
    Timeout(#[from] tokio::time::error::Elapsed),

    /// The request was cancelled
    Cancelled(#[from] tokio::sync::oneshot::error::RecvError),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::ErrorResponse(err) => {
                write!(f, "Server error: {} {}", err.code, err.message)
            }
            RequestError::UnexpectedReply(_) => write!(f, "Unexpected reply from server"),
            RequestError::ReplyError(err) => write!(f, "Reply processing error: {}", err),
            RequestError::Timeout(err) => write!(f, "Request timed out: {}", err),
            RequestError::Cancelled(_) => write!(f, "Request was cancelled"),
        }
    }
}
