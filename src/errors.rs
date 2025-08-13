//! # Errors Module
//!
//! This module provides comprehensive error handling for the tokio-centrifuge library.
//! It includes error types for client operations, server operations, and connection
//! management, following the Centrifugo protocol specification.
//!
//! ## Error Categories
//!
//! - **ClientError**: Errors that occur during client operations
//! - **DisconnectErrorCode**: Connection-level error codes
//! - **RemoveSubscriptionError**: Subscription management errors
//!
//! ## Error Codes
//!
//! The library follows the Centrifugo protocol error code specification:
//! - **100-199**: Client operation errors
//! - **3000-3999**: Connection-level errors
//! - **3500-3999**: Authentication and permission errors

use std::borrow::Cow;
use std::fmt::Display;

use thiserror::Error;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

/// Error that occurs when trying to remove a subscription
///
/// This error is raised when attempting to remove a subscription
/// that hasn't been properly unsubscribed first.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::errors::RemoveSubscriptionError;
///
/// // In a real implementation, you would have a subscription object
/// // let subscription = get_subscription();
///
/// // match subscription.remove() {
/// //     Ok(()) => println!("Subscription removed"),
/// //     Err(RemoveSubscriptionError::NotUnsubscribed) => {
/// //         println!("Must unsubscribe before removing");
/// //     }
/// // }
/// ```
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveSubscriptionError {
    /// Subscription must be unsubscribed before removal
    #[error("subscription must be unsubscribed to be removed")]
    NotUnsubscribed,
}

/// Client operation error with code and message
///
/// This struct represents errors that occur during client operations
/// such as RPC calls, publications, or subscriptions.
///
/// ## Fields
///
/// - **code**: Numeric error code following Centrifugo specification
/// - **message**: Human-readable error description
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::errors::{ClientError, ClientErrorCode};
///
/// let error = ClientError::new(
///     ClientErrorCode::Unauthorized,
///     "Invalid authentication token"
/// );
///
/// assert_eq!(error.code, ClientErrorCode::Unauthorized);
/// assert_eq!(error.message, "Invalid authentication token");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientError {
    /// Numeric error code
    pub code: ClientErrorCode,
    /// Human-readable error message
    pub message: Cow<'static, str>,
}

impl From<anyhow::Error> for ClientError {
    /// Converts anyhow errors to internal client errors
    fn from(err: anyhow::Error) -> Self {
        Self::internal(err.to_string())
    }
}

impl From<ClientError> for crate::protocol::Error {
    /// Converts client errors to protocol errors
    fn from(err: ClientError) -> Self {
        Self {
            code: err.code.0.into(),
            message: err.message.into_owned(),
            temporary: err.code.is_temporary(),
        }
    }
}

impl ClientError {
    /// Creates a new client error with the specified code and message
    ///
    /// ## Arguments
    ///
    /// * `code` - Error code from ClientErrorCode
    /// * `message` - Human-readable error message
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::{ClientError, ClientErrorCode};
    ///
    /// let error = ClientError::new(
    ///     ClientErrorCode::MethodNotFound,
    ///     "RPC method 'unknown' not found"
    /// );
    /// ```
    pub fn new(code: ClientErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Creates an internal server error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the internal error
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::internal("Database connection failed");
    /// ```
    pub fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::Internal, message)
    }

    /// Creates an unauthorized access error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the authorization failure
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::unauthorized("Invalid API key");
    /// ```
    pub fn unauthorized(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::Unauthorized, message)
    }

    /// Creates an unknown channel error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the channel issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::unknown_channel("Channel 'private' does not exist");
    /// ```
    pub fn unknown_channel(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::UnknownChannel, message)
    }

    /// Creates a permission denied error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the permission issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::permission_denied("Insufficient permissions for admin channel");
    /// ```
    pub fn permission_denied(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::PermissionDenied, message)
    }

    /// Creates a method not found error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the method issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::method_not_found("RPC method 'calculate' not implemented");
    /// ```
    pub fn method_not_found(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::MethodNotFound, message)
    }

    /// Creates an already subscribed error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the subscription issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::already_subscribed("Already subscribed to 'news' channel");
    /// ```
    pub fn already_subscribed(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::AlreadySubscribed, message)
    }

    /// Creates a limit exceeded error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the limit issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::limit_exceeded("Maximum 10 subscriptions allowed");
    /// ```
    pub fn limit_exceeded(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::LimitExceeded, message)
    }

    /// Creates a bad request error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the request issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::bad_request("Invalid JSON payload");
    /// ```
    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::BadRequest, message)
    }

    /// Creates a not available error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the availability issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::not_available("Service temporarily unavailable");
    /// ```
    pub fn not_available(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::NotAvailable, message)
    }

    /// Creates a token expired error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the token issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::token_expired("Authentication token has expired");
    /// ```
    pub fn token_expired(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::TokenExpired, message)
    }

    /// Creates an expired error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the expiration issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::expired("Subscription has expired");
    /// ```
    pub fn expired(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::Expired, message)
    }

    /// Creates a too many requests error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the rate limit issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::too_many_requests("Rate limit exceeded: 100 requests per minute");
    /// ```
    pub fn too_many_requests(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::TooManyRequests, message)
    }

    /// Creates an unrecoverable position error
    ///
    /// ## Arguments
    ///
    /// * `message` - Error message describing the position issue
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientError;
    ///
    /// let error = ClientError::unrecoverable_position("Stream position is unrecoverable");
    /// ```
    pub fn unrecoverable_position(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ClientErrorCode::UnrecoverablePosition, message)
    }
}

impl From<ClientErrorCode> for ClientError {
    /// Converts error codes to client errors with default messages
    fn from(code: ClientErrorCode) -> Self {
        Self::new(code, code.to_str())
    }
}

/// Numeric error codes for client operations
///
/// This struct represents the standard error codes defined by the
/// Centrifugo protocol for client-side operations.
///
/// ## Error Code Ranges
///
/// - **100-199**: Client operation errors
/// - **100**: Internal server error
/// - **101**: Unauthorized access
/// - **102**: Unknown channel
/// - **103**: Permission denied
/// - **104**: Method not found
/// - **105**: Already subscribed
/// - **106**: Limit exceeded
/// - **107**: Bad request
/// - **108**: Not available
/// - **109**: Token expired
/// - **110**: Expired
/// - **111**: Too many requests
/// - **112**: Unrecoverable position
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::errors::ClientErrorCode;
///
/// let code = ClientErrorCode::Unauthorized;
/// assert_eq!(code.0, 101);
/// assert_eq!(code.to_string(), "unauthorized");
/// ```
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientErrorCode(pub u16);

#[allow(non_upper_case_globals)]
impl ClientErrorCode {
    /// Internal server error (100)
    pub const Internal: Self = Self(100);
    /// Unauthorized access (101)
    pub const Unauthorized: Self = Self(101);
    /// Unknown channel (102)
    pub const UnknownChannel: Self = Self(102);
    /// Permission denied (103)
    pub const PermissionDenied: Self = Self(103);
    /// Method not found (104)
    pub const MethodNotFound: Self = Self(104);
    /// Already subscribed (105)
    pub const AlreadySubscribed: Self = Self(105);
    /// Limit exceeded (106)
    pub const LimitExceeded: Self = Self(106);
    /// Bad request (107)
    pub const BadRequest: Self = Self(107);
    /// Not available (108)
    pub const NotAvailable: Self = Self(108);
    /// Token expired (109)
    pub const TokenExpired: Self = Self(109);
    /// Expired (110)
    pub const Expired: Self = Self(110);
    /// Too many requests (111)
    pub const TooManyRequests: Self = Self(111);
    /// Unrecoverable position (112)
    pub const UnrecoverablePosition: Self = Self(112);

    /// Checks if this error is temporary and retryable
    ///
    /// Temporary errors include:
    /// - Internal server errors
    /// - Too many requests (rate limiting)
    ///
    /// ## Returns
    ///
    /// `true` if the error is temporary and retryable
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientErrorCode;
    ///
    /// assert!(ClientErrorCode::Internal.is_temporary());
    /// assert!(ClientErrorCode::TooManyRequests.is_temporary());
    /// assert!(!ClientErrorCode::Unauthorized.is_temporary());
    /// ```
    pub fn is_temporary(self) -> bool {
        matches!(self, Self::Internal | Self::TooManyRequests)
    }

    /// Converts the error code to a human-readable string
    ///
    /// ## Returns
    ///
    /// A string representation of the error code
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::ClientErrorCode;
    ///
    /// assert_eq!(ClientErrorCode::Unauthorized.to_str(), "unauthorized");
    /// assert_eq!(ClientErrorCode::MethodNotFound.to_str(), "method not found");
    /// ```
    pub fn to_str(&self) -> Cow<'static, str> {
        match self.0 {
            100 => Cow::Borrowed("internal server error"),
            101 => Cow::Borrowed("unauthorized"),
            102 => Cow::Borrowed("unknown channel"),
            103 => Cow::Borrowed("permission denied"),
            104 => Cow::Borrowed("method not found"),
            105 => Cow::Borrowed("already subscribed"),
            106 => Cow::Borrowed("limit exceeded"),
            107 => Cow::Borrowed("bad request"),
            108 => Cow::Borrowed("not available"),
            109 => Cow::Borrowed("token expired"),
            110 => Cow::Borrowed("expired"),
            111 => Cow::Borrowed("too many requests"),
            112 => Cow::Borrowed("unrecoverable position"),
            _ => Cow::Owned(format!("unknown code {}", self.0)),
        }
    }
}

impl Display for ClientErrorCode {
    /// Formats the error code as a human-readable string
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

impl From<ClientErrorCode> for u16 {
    /// Extracts the numeric error code
    fn from(code: ClientErrorCode) -> Self {
        code.0
    }
}

impl From<u16> for ClientErrorCode {
    /// Creates an error code from a numeric value
    fn from(code: u16) -> Self {
        Self(code)
    }
}

impl From<ClientErrorCode> for crate::protocol::Error {
    /// Converts client error codes to protocol errors
    fn from(code: ClientErrorCode) -> Self {
        Self {
            code: code.0.into(),
            message: code.to_string(),
            temporary: code.is_temporary(),
        }
    }
}

/// Numeric error codes for connection-level operations
///
/// This struct represents the standard error codes defined by the
/// Centrifugo protocol for connection-level operations.
///
/// ## Error Code Ranges
///
/// - **3000-3999**: Connection-level errors
/// - **3500-3999**: Authentication and permission errors
/// - **4500-4999**: Additional error codes
///
/// ## Reconnection Behavior
///
/// Error codes in the ranges 3500-3999 and 4500-4999 indicate
/// that the client should not attempt to reconnect.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::errors::DisconnectErrorCode;
///
/// let code = DisconnectErrorCode::InvalidToken;
/// assert_eq!(code.0, 3500);
/// assert_eq!(code.to_string(), "invalid token");
/// assert!(!code.should_reconnect());
/// ```
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisconnectErrorCode(pub u16);

#[allow(non_upper_case_globals)]
impl DisconnectErrorCode {
    /// Connection closed (3000)
    pub const ConnectionClosed: Self = Self(3000);
    /// Server shutdown (3001)
    pub const Shutdown: Self = Self(3001);
    /// Internal server error (3004)
    pub const ServerError: Self = Self(3004);
    /// Connection expired (3005)
    pub const Expired: Self = Self(3005);
    /// Subscription expired (3006)
    pub const SubExpired: Self = Self(3006);
    /// Slow connection (3008)
    pub const Slow: Self = Self(3008);
    /// Write error (3009)
    pub const WriteError: Self = Self(3009);
    /// Insufficient state (3010)
    pub const InsufficientState: Self = Self(3010);
    /// Force reconnect (3011)
    pub const ForceReconnect: Self = Self(3011);
    /// No pong received (3012)
    pub const NoPong: Self = Self(3012);
    /// Too many requests (3013)
    pub const TooManyRequests: Self = Self(3013);
    /// Invalid token (3500)
    pub const InvalidToken: Self = Self(3500);
    /// Bad request (3501)
    pub const BadRequest: Self = Self(3501);
    /// Stale connection (3502)
    pub const Stale: Self = Self(3502);
    /// Force no reconnect (3503)
    pub const ForceNoReconnect: Self = Self(3503);
    /// Connection limit exceeded (3504)
    pub const ConnectionLimit: Self = Self(3504);
    /// Channel limit exceeded (3505)
    pub const ChannelLimit: Self = Self(3505);
    /// Inappropriate protocol (3506)
    pub const InappropriateProtocol: Self = Self(3506);
    /// Permission denied (3507)
    pub const PermissionDenied: Self = Self(3507);
    /// Not available (3508)
    pub const NotAvailable: Self = Self(3508);
    /// Too many errors (3509)
    pub const TooManyErrors: Self = Self(3509);

    /// Determines if the client should attempt to reconnect
    ///
    /// ## Reconnection Rules
    ///
    /// - **Should reconnect**: Error codes 3000-3499 and 4000-4499
    /// - **Should not reconnect**: Error codes 3500-3999 and 4500-4999
    ///
    /// ## Returns
    ///
    /// `true` if the client should attempt to reconnect
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::errors::DisconnectErrorCode;
    ///
    /// // Should reconnect
    /// assert!(DisconnectErrorCode::ConnectionClosed.should_reconnect());
    /// assert!(DisconnectErrorCode::ServerError.should_reconnect());
    ///
    /// // Should not reconnect
    /// assert!(!DisconnectErrorCode::InvalidToken.should_reconnect());
    /// assert!(!DisconnectErrorCode::BadRequest.should_reconnect());
    /// ```
    pub fn should_reconnect(self) -> bool {
        !matches!(self.0, 3500..=3999 | 4500..=4999)
    }
}

impl Display for DisconnectErrorCode {
    /// Formats the error code as a human-readable string
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            3000 => write!(f, "connection closed"),
            3001 => write!(f, "shutdown"),
            3004 => write!(f, "internal server error"),
            3005 => write!(f, "connection expired"),
            3006 => write!(f, "subscription expired"),
            3008 => write!(f, "slow"),
            3009 => write!(f, "write error"),
            3010 => write!(f, "insufficient state"),
            3011 => write!(f, "force reconnect"),
            3012 => write!(f, "no pong"),
            3013 => write!(f, "too many requests"),
            3500 => write!(f, "invalid token"),
            3501 => write!(f, "bad request"),
            3502 => write!(f, "stale"),
            3503 => write!(f, "force disconnect"),
            3504 => write!(f, "connection limit"),
            3505 => write!(f, "channel limit"),
            3506 => write!(f, "inappropriate protocol"),
            3507 => write!(f, "permission denied"),
            3508 => write!(f, "not available"),
            3509 => write!(f, "too many errors"),
            _ => write!(f, "unknown code {}", self.0),
        }
    }
}

impl From<DisconnectErrorCode> for u16 {
    /// Extracts the numeric error code
    fn from(code: DisconnectErrorCode) -> Self {
        code.0
    }
}

impl From<u16> for DisconnectErrorCode {
    /// Creates an error code from a numeric value
    fn from(code: u16) -> Self {
        Self(code)
    }
}

impl From<DisconnectErrorCode> for Option<CloseFrame> {
    /// Converts disconnect error codes to WebSocket close frames
    ///
    /// This is useful for properly closing WebSocket connections
    /// with appropriate error codes and reasons.
    fn from(code: DisconnectErrorCode) -> Self {
        Some(CloseFrame {
            code: code.0.into(),
            reason: code.to_string().into(),
        })
    }
}
