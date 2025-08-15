//! # Configuration Module
//!
//! This module provides configuration types and settings for the tokio-centrifuge library.
//! It includes protocol selection, connection parameters, and reconnection strategies.
//!
//! ## Core Types
//!
//! - **Protocol**: Supported protocol encoding formats
//! - **Config**: Main configuration struct for client connections
//! - **ReconnectStrategy**: Trait for custom reconnection behavior
//! - **BackoffReconnect**: Exponential backoff reconnection strategy

use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Handle;

/// Supported protocol encoding formats
///
/// The library supports two main protocol formats for communication
/// with Centrifugo servers.
///
/// ## Variants
///
/// - **Json**: JSON encoding (human-readable, good for debugging)
/// - **Protobuf**: Protocol Buffer encoding (binary, more efficient)
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::config::Protocol;
///
/// let json_protocol = Protocol::Json;
/// let protobuf_protocol = Protocol::Protobuf;
///
/// assert_ne!(json_protocol, protobuf_protocol);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// JSON encoding format
    ///
    /// Human-readable format that's easy to debug and inspect.
    /// Good for development and testing environments.
    Json,

    /// Protocol Buffer encoding format
    ///
    /// Binary format that's more efficient in terms of size
    /// and parsing speed. Recommended for production use.
    Protobuf,
}

/// Main configuration struct for client connections
///
/// This struct contains all the configuration options needed to
/// establish and maintain connections to Centrifugo servers.
///
/// ## Configuration Options
///
/// - **Connection**: Token, name, version, and protocol
/// - **Runtime**: Custom tokio runtime handle
/// - **Timeouts**: Read timeout settings
/// - **Reconnection**: Reconnection strategy and behavior
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::config::{Config, Protocol};
/// use std::time::Duration;
///
/// let config = Config::new()
///     .with_token("your-auth-token")
///     .with_name("my-client")
///     .with_version("1.0.0")
///     .use_json()
///     .with_read_timeout(Duration::from_secs(10));
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Authentication token for the connection
    ///
    /// This token is sent during the connection handshake
    /// and is used by the server for authentication.
    pub token: String,

    /// Client name identifier
    ///
    /// Human-readable name for the client, useful for
    /// server-side logging and monitoring.
    pub name: String,

    /// Client version string
    ///
    /// Version information for the client, useful for
    /// protocol compatibility checking.
    pub version: String,

    /// Protocol encoding format to use
    ///
    /// Determines how messages are serialized and deserialized.
    pub protocol: Protocol,

    /// Optional custom tokio runtime handle
    ///
    /// If provided, the client will use this runtime instead
    /// of the default one. Useful for integration with existing
    /// tokio applications.
    pub runtime: Option<Handle>,

    /// Read timeout for WebSocket operations
    ///
    /// Maximum time to wait for incoming messages before
    /// considering the connection stale.
    pub read_timeout: Duration,

    /// Reconnection strategy for handling disconnections
    ///
    /// Determines how and when the client attempts to reconnect
    /// after a connection failure or disconnection.
    pub reconnect_strategy: Arc<dyn ReconnectStrategy>,
}

impl Default for Config {
    /// Creates default configuration with sensible defaults
    ///
    /// Defaults:
    /// - Empty token (no authentication)
    /// - Package name from Cargo.toml
    /// - Empty version
    /// - JSON protocol
    /// - No custom runtime
    /// - 5 second read timeout
    /// - Exponential backoff reconnection
    fn default() -> Self {
        Config {
            token: String::new(),
            name: String::from(env!("CARGO_PKG_NAME")),
            version: String::new(),
            protocol: Protocol::Json,
            runtime: None,
            read_timeout: Duration::from_secs(5),
            reconnect_strategy: Arc::new(BackoffReconnect::default()),
        }
    }
}

impl Config {
    /// Creates a new configuration with default values
    ///
    /// This is equivalent to `Config::default()`.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new();
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the authentication token
    ///
    /// ## Arguments
    ///
    /// * `token` - Authentication token string
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().with_token("secret-token");
    /// assert_eq!(config.token, "secret-token");
    /// ```
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = token.into();
        self
    }

    /// Sets the client name
    ///
    /// ## Arguments
    ///
    /// * `name` - Client name string
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().with_name("my-app");
    /// assert_eq!(config.name, "my-app");
    /// ```
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Sets the client version
    ///
    /// ## Arguments
    ///
    /// * `version` - Version string
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::Config;
    ///
    /// let config = Config::new().with_version("2.1.0");
    /// assert_eq!(config.version, "2.1.0");
    /// ```
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Sets a custom tokio runtime handle
    ///
    /// ## Arguments
    ///
    /// * `runtime` - Tokio runtime handle
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::Config;
    /// use tokio::runtime::Runtime;
    ///
    /// let runtime = Runtime::new().unwrap();
    /// let config = Config::new().with_runtime(runtime.handle().clone());
    /// ```
    pub fn with_runtime(mut self, runtime: tokio::runtime::Handle) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Sets the read timeout duration
    ///
    /// ## Arguments
    ///
    /// * `timeout` - Read timeout duration
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::Config;
    /// use std::time::Duration;
    ///
    /// let config = Config::new().with_read_timeout(Duration::from_secs(30));
    /// assert_eq!(config.read_timeout, Duration::from_secs(30));
    /// ```
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Sets the reconnection strategy
    ///
    /// ## Arguments
    ///
    /// * `strategy` - Reconnection strategy implementation
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::{Config, BackoffReconnect};
    /// use std::time::Duration;
    ///
    /// let strategy = BackoffReconnect {
    ///     factor: 1.5,
    ///     min_delay: Duration::from_millis(100),
    ///     max_delay: Duration::from_secs(10),
    /// };
    ///
    /// let config = Config::new().with_reconnect_strategy(strategy);
    /// ```
    pub fn with_reconnect_strategy(mut self, strategy: impl ReconnectStrategy) -> Self {
        self.reconnect_strategy = Arc::new(strategy);
        self
    }

    /// Sets the protocol to JSON encoding
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::{Config, Protocol};
    ///
    /// let config = Config::new().use_json();
    /// assert_eq!(config.protocol, Protocol::Json);
    /// ```
    pub fn use_json(mut self) -> Self {
        self.protocol = Protocol::Json;
        self
    }

    /// Sets the protocol to Protobuf encoding
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::{Config, Protocol};
    ///
    /// let config = Config::new().use_protobuf();
    /// assert_eq!(config.protocol, Protocol::Protobuf);
    /// ```
    pub fn use_protobuf(mut self) -> Self {
        self.protocol = Protocol::Protobuf;
        self
    }
}

/// Trait for implementing custom reconnection strategies
///
/// This trait allows users to implement their own reconnection
/// logic, such as custom backoff algorithms or rate limiting.
///
/// ## Required Methods
///
/// * `time_before_next_attempt` - Returns delay before next reconnection attempt
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::config::ReconnectStrategy;
/// use std::time::Duration;
///
/// #[derive(Debug)]
/// struct LinearReconnect {
///     base_delay: Duration,
/// }
///
/// impl ReconnectStrategy for LinearReconnect {
///     fn time_before_next_attempt(&self, attempt: u32) -> Duration {
///         Duration::from_millis(self.base_delay.as_millis() as u64 * attempt as u64)
///     }
/// }
/// ```
pub trait ReconnectStrategy: std::fmt::Debug + Send + Sync + 'static {
    /// Calculates the delay before the next reconnection attempt
    ///
    /// ## Arguments
    ///
    /// * `attempt` - The attempt number (1-based)
    ///
    /// ## Returns
    ///
    /// Duration to wait before attempting to reconnect
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::{ReconnectStrategy, BackoffReconnect};
    /// use std::time::Duration;
    ///
    /// let strategy = BackoffReconnect::default();
    /// let delay = strategy.time_before_next_attempt(3);
    /// assert!(delay > Duration::from_millis(0));
    /// ```
    fn time_before_next_attempt(&self, attempt: u32) -> Duration;
}

/// Exponential backoff reconnection strategy
///
/// This strategy implements exponential backoff with configurable
/// parameters for handling reconnection attempts.
///
/// ## Algorithm
///
/// The delay is calculated as: `min_delay * factor^attempt`
/// and then clamped between `min_delay` and `max_delay`.
///
/// ## Example
///
/// ```rust
/// use tokio_centrifuge::config::BackoffReconnect;
/// use std::time::Duration;
///
/// let strategy = BackoffReconnect {
///     factor: 2.0,
///     min_delay: Duration::from_millis(100),
///     max_delay: Duration::from_secs(5),
/// };
///
/// // First attempt: 100ms
/// // Second attempt: 200ms
/// // Third attempt: 400ms
/// // Fourth attempt: 800ms
/// // Fifth attempt: 1600ms
/// // Sixth attempt: 3200ms (clamped to 5000ms)
/// ```
#[derive(Debug, Clone)]
pub struct BackoffReconnect {
    /// Exponential factor for backoff calculation
    ///
    /// Each attempt multiplies the delay by this factor.
    /// Common values are 2.0 (doubling) or 1.5 (50% increase).
    pub factor: f64,

    /// Minimum delay between attempts
    ///
    /// The calculated delay will never be less than this value.
    pub min_delay: Duration,

    /// Maximum delay between attempts
    ///
    /// The calculated delay will never exceed this value.
    pub max_delay: Duration,
}

impl ReconnectStrategy for BackoffReconnect {
    /// Calculates exponential backoff delay
    ///
    /// ## Implementation Details
    ///
    /// 1. Calculates `min_delay * factor^attempt`
    /// 2. Clamps result between `min_delay` and `max_delay`
    /// 3. Handles edge case where `min_delay > max_delay`
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::{BackoffReconnect, ReconnectStrategy};
    /// use std::time::Duration;
    ///
    /// let strategy = BackoffReconnect {
    ///     factor: 2.0,
    ///     min_delay: Duration::from_millis(100),
    ///     max_delay: Duration::from_secs(1),
    /// };
    ///
    /// let delay1 = strategy.time_before_next_attempt(1);
    /// let delay2 = strategy.time_before_next_attempt(2);
    /// let delay3 = strategy.time_before_next_attempt(3);
    ///
    /// assert!(delay1 < delay2);
    /// assert!(delay2 < delay3);
    /// ```
    fn time_before_next_attempt(&self, attempt: u32) -> Duration {
        if self.min_delay > self.max_delay {
            return self.max_delay;
        }

        let time = self.min_delay.as_secs_f64() * self.factor.powi(attempt as i32);
        let time = time.clamp(self.min_delay.as_secs_f64(), self.max_delay.as_secs_f64());
        Duration::from_secs_f64(time)
    }
}

impl Default for BackoffReconnect {
    /// Creates default exponential backoff strategy
    ///
    /// Defaults:
    /// - **factor**: 2.0 (doubling each attempt)
    /// - **min_delay**: 200ms
    /// - **max_delay**: 20 seconds
    ///
    /// ## Example
    ///
    /// ```rust
    /// use tokio_centrifuge::config::BackoffReconnect;
    ///
    /// let strategy = BackoffReconnect::default();
    /// assert_eq!(strategy.factor, 2.0);
    /// ```
    fn default() -> Self {
        BackoffReconnect {
            factor: 2.0,
            min_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(20),
        }
    }
}
