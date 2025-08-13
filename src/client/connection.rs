//! Connection management and reconnection logic.
//!
//! This module handles the low-level connection operations including
//! establishing WebSocket connections, managing reconnection attempts,
//! and handling connection state transitions.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::client::types::State;

use super::inner::ClientInner;

/// Manages connection establishment and reconnection logic.
///
/// This struct provides methods for connecting to WebSocket servers,
/// handling connection failures, and implementing reconnection strategies.
pub(crate) struct ConnectionManager;

impl ConnectionManager {
    /// Calculates delay and waits before the next reconnection attempt.
    ///
    /// This function implements the reconnection strategy by calculating
    /// the appropriate delay based on the number of previous attempts.
    /// It also handles interruption by user disconnect requests.
    ///
    /// # Arguments
    ///
    /// * `client` - Reference to the client instance
    /// * `closer_read` - Receiver for close signals
    /// * `reconnect_attempts` - Number of previous reconnection attempts
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the delay completed successfully,
    /// or `Err(false)` if interrupted by user.
    pub(crate) fn do_delay<'a>(
        client: &Arc<Mutex<ClientInner>>,
        closer_read: &'a mut mpsc::Receiver<bool>,
        reconnect_attempts: u32,
    ) -> impl Future<Output = Result<(), bool>> + 'a {
        let delay = {
            let inner = client.lock().unwrap();
            if reconnect_attempts > 0 {
                inner
                    .reconnect_strategy
                    .time_before_next_attempt(reconnect_attempts)
            } else {
                Duration::ZERO
            }
        };

        async move {
            let task = async {
                if reconnect_attempts > 0 {
                    log::debug!(
                        "reconnecting attempt {}, delay={:?}",
                        reconnect_attempts,
                        delay
                    );
                }
                tokio::time::sleep(delay).await;
                Ok(())
            };

            tokio::select! {
                biased;
                _ = closer_read.recv() => {
                    log::debug!("reconnect interrupted by user");
                    Err(false)
                }
                result = task => result
            }
        }
    }

    /// Establishes a WebSocket connection to the server.
    ///
    /// Attempts to connect to the WebSocket server using the URL from the client configuration.
    /// Handles connection errors and determines whether reconnection should be attempted.
    ///
    /// # Arguments
    ///
    /// * `client` - Reference to the client instance
    /// * `closer_read` - Receiver for close signals
    ///
    /// # Returns
    ///
    /// Returns the WebSocket stream on successful connection,
    /// or an error indicating whether reconnection should be attempted.
    pub(crate) fn do_connect<'a>(
        client: &Arc<Mutex<ClientInner>>,
        closer_read: &'a mut mpsc::Receiver<bool>,
    ) -> impl Future<Output = Result<WebSocketStream<MaybeTlsStream<TcpStream>>, bool>> + 'a {
        let url = {
            let inner = client.lock().unwrap();
            inner.url.clone()
        };

        let client = client.clone();
        async move {
            let task = async {
                log::debug!("connecting to {}", &url);
                match tokio_tungstenite::connect_async(&*url).await {
                    Ok((stream, _)) => Ok(stream),
                    Err(err) => {
                        log::debug!("{err}");
                        let mut inner = client.lock().unwrap();
                        if inner.state != State::Connecting {
                            return Err(false);
                        }

                        let do_reconnect = match err {
                            tokio_tungstenite::tungstenite::Error::Url(_) => {
                                // invalid url, don't reconnect
                                false
                            }
                            _ => true,
                        };

                        if let Some(ref mut on_error) = inner.on_error {
                            on_error(err.into());
                        }
                        Err(do_reconnect)
                    }
                }
            };

            tokio::select! {
                biased;
                _ = closer_read.recv() => {
                    log::debug!("reconnect interrupted by user");
                    Err(false)
                }
                result = task => result
            }
        }
    }

    /// Verifies that the client is in the expected state.
    ///
    /// This function checks if the client's current state matches the expected state.
    /// It's used to ensure proper state transitions during connection operations.
    ///
    /// # Arguments
    ///
    /// * `client` - Reference to the client instance
    /// * `expected` - The expected state
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the state matches, or `Err(false)` otherwise.
    pub(crate) fn do_check_state(
        client: &Arc<Mutex<ClientInner>>,
        expected: State,
    ) -> Result<(), bool> {
        let inner = client.lock().unwrap();
        if inner.state != expected {
            return Err(false);
        }
        Ok(())
    }
}
