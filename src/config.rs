use std::sync::Arc;
use std::time::Duration;

use tokio::runtime::Handle;

#[derive(Debug, Default, Clone)]
pub struct Config {
	pub(crate) runtime: Option<Handle>,
	pub(crate) reconnect_strategy: Option<Arc<dyn ReconnectStrategy>>,
}

impl Config {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn runtime(mut self, runtime: tokio::runtime::Handle) -> Self {
		self.runtime = Some(runtime);
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
