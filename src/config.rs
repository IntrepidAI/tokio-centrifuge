use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Handle;

#[derive(Debug, Clone)]
pub struct Config {
	pub token: String,
	pub name: String,
	pub version: String,
	pub runtime: Option<Handle>,
	pub read_timeout: Duration,
	pub reconnect_strategy: Arc<dyn ReconnectStrategy>,
}

impl Default for Config {
	fn default() -> Self {
		Config {
			token: String::new(),
			name: String::from(env!("CARGO_PKG_NAME")),
			version: String::new(),
			runtime: None,
			read_timeout: Duration::from_secs(5),
			reconnect_strategy: Arc::new(BackoffReconnect::default()),
		}
	}
}

impl Config {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn with_token(mut self, token: impl Into<String>) -> Self {
		self.token = token.into();
		self
	}

	pub fn with_name(mut self, name: impl Into<String>) -> Self {
		self.name = name.into();
		self
	}

	pub fn with_version(mut self, version: impl Into<String>) -> Self {
		self.version = version.into();
		self
	}

	pub fn with_runtime(mut self, runtime: tokio::runtime::Handle) -> Self {
		self.runtime = Some(runtime);
		self
	}

	pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
		self.read_timeout = timeout;
		self
	}

	pub fn with_reconnect_strategy(mut self, strategy: impl ReconnectStrategy) -> Self {
		self.reconnect_strategy = Arc::new(strategy);
		self
	}
}

pub trait ReconnectStrategy: std::fmt::Debug + Send + Sync + 'static {
	fn time_before_next_attempt(&self, attempt: u32) -> Duration;
}

#[derive(Debug, Clone)]
pub struct BackoffReconnect {
	pub factor: f64,
	pub min_delay: Duration,
	pub max_delay: Duration,
}

impl ReconnectStrategy for BackoffReconnect {
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
	fn default() -> Self {
		BackoffReconnect {
			factor: 2.0,
			min_delay: Duration::from_millis(200),
			max_delay: Duration::from_secs(20),
		}
	}
}
