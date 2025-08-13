//! Subscription management and message handling.
//!
//! This module handles the lifecycle of channel subscriptions including
//! subscribing to channels, unsubscribing, and processing incoming
//! publications from subscribed channels.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

use crate::client::types::MessageStore;
use crate::client_handler::ReplyError;
use crate::protocol::{Command, SubscribeRequest, UnsubscribeRequest};
use crate::subscription::SubscriptionId;

use super::inner::ClientInner;

/// Manages channel subscriptions and their lifecycle.
///
/// This struct handles the subscription process including:
/// - Subscribing to channels when requested
/// - Unsubscribing from channels when needed
/// - Managing subscription state transitions
/// - Processing incoming publications
pub(crate) struct SubscriptionHandler;

impl SubscriptionHandler {
    /// Handles subscription lifecycle management.
    ///
    /// This function runs in a separate task and manages all subscription-related
    /// operations. It monitors the subscription channel for new subscription
    /// requests and handles the subscribe/unsubscribe logic.
    ///
    /// # Arguments
    ///
    /// * `client` - The client instance for accessing subscriptions
    /// * `sub_ch_read` - Receiver for subscription channel IDs
    /// * `control_write` - Sender for control commands to the server
    /// * `rt` - Tokio runtime handle for spawning tasks
    pub(crate) async fn handle_subscriptions(
        client: Arc<Mutex<ClientInner>>,
        mut sub_ch_read: mpsc::UnboundedReceiver<SubscriptionId>,
        control_write: mpsc::Sender<(
            Command,
            oneshot::Sender<Result<crate::protocol::Reply, ReplyError>>,
            Duration,
        )>,
        rt: tokio::runtime::Handle,
    ) {
        let mut subscribed_channels: HashSet<SubscriptionId> = HashSet::new();
        let mut join_set = JoinSet::new();

        /// Result types for managing task lifecycle in the join set.
        #[allow(clippy::type_complexity)]
        enum JoinSetResult {
            /// A new task that needs to be spawned
            NewTask(
                (
                    Option<Pin<Box<dyn Future<Output = JoinSetResult> + Send>>>,
                    mpsc::Receiver<Pin<Box<dyn Future<Output = JoinSetResult> + Send>>>,
                ),
            ),
            /// Task completed successfully but no further action needed
            Irrelevant,
        }

        let (new_tasks_tx, new_tasks_rx) = mpsc::channel(1);

        join_set.spawn_on(
            async move { JoinSetResult::NewTask((None, new_tasks_rx)) },
            &rt,
        );

        /// Task for handling message publishing for a specific subscription.
        ///
        /// This task manages the message queue for a subscription and sends
        /// messages to the server when the subscription is active.
        #[allow(clippy::type_complexity)]
        async fn publish_task(
            client: Arc<Mutex<ClientInner>>,
            control_write: mpsc::Sender<(
                Command,
                oneshot::Sender<Result<crate::protocol::Reply, ReplyError>>,
                Duration,
            )>,
            get_store: impl Fn(&mut ClientInner) -> Option<&mut MessageStore>,
        ) -> JoinSetResult {
            let mut pub_ch_read = {
                let mut inner = client.lock().unwrap();
                let Some(store) = get_store(&mut inner) else {
                    return JoinSetResult::Irrelevant;
                };
                store.reset_channel()
            };

            const MAX_CAPACITY: usize = 32;
            let mut buffer = Vec::new();
            loop {
                {
                    // lock mutex and fill our buffer
                    let mut inner = client.lock().unwrap();
                    let Some(pub_ch_write) = get_store(&mut inner) else {
                        break;
                    };
                    let now = std::time::Instant::now();
                    for _ in 0..MAX_CAPACITY {
                        if let Some(item) = pub_ch_write.get_next(now) {
                            buffer.push(item);
                        } else {
                            break;
                        }
                    }
                }
                if buffer.is_empty() {
                    // wait for activity
                    let Some(()) = pub_ch_read.recv().await else {
                        break;
                    };
                } else {
                    // send messages
                    for item in buffer.drain(..) {
                        let timeout = item
                            .deadline
                            .saturating_duration_since(std::time::Instant::now());
                        let _ = control_write
                            .send((item.command, item.reply, timeout))
                            .await;
                    }
                }
            }

            JoinSetResult::Irrelevant
        }

        let control_write1 = control_write;
        join_set.spawn_on(
            async move {
                loop {
                    let mut buf = Vec::new();
                    let count = sub_ch_read.recv_many(&mut buf, 32).await;
                    if count == 0 {
                        break;
                    }

                    let mut inner = client.lock().unwrap();
                    let rt = inner.rt.clone();
                    let timeout = inner.read_timeout;

                    for sub_id in buf.drain(..count) {
                        let Some(sub) = inner.subscriptions.get_mut(sub_id) else {
                            continue;
                        };

                        if sub.state != crate::subscription::State::Unsubscribed
                            && !subscribed_channels.contains(&sub_id)
                        {
                            // subscribe
                            let (tx, rx) = oneshot::channel();
                            let command = Command::Subscribe(SubscribeRequest {
                                channel: (*sub.channel).into(),
                                ..Default::default()
                            });
                            let control_write = control_write1.clone();
                            subscribed_channels.insert(sub_id);
                            let client = client.clone();
                            let new_tasks_tx = new_tasks_tx.clone();
                            rt.spawn(async move {
                                {
                                    let _ = control_write.send((command, tx, timeout)).await;
                                    let result = rx.await;
                                    let mut inner = client.lock().unwrap();
                                    if let Some(sub) = inner.subscriptions.get_mut(sub_id) {
                                        let msg =
                                            if let Ok(Ok(crate::protocol::Reply::Subscribe(_))) =
                                                result
                                            {
                                                sub.move_to_subscribed();
                                                Ok(())
                                            } else {
                                                Err(())
                                            };
                                        for ch in sub.on_subscribed_ch.drain(..) {
                                            let _ = ch.send(msg);
                                        }
                                    }
                                }

                                let _ = new_tasks_tx
                                    .send(Box::pin(publish_task(
                                        client.clone(),
                                        control_write.clone(),
                                        move |inner| {
                                            inner
                                                .subscriptions
                                                .get_mut(sub_id)
                                                .and_then(|sub| sub.pub_ch_write.as_mut())
                                        },
                                    )))
                                    .await;
                            });
                        }

                        if sub.state == crate::subscription::State::Unsubscribed
                            && subscribed_channels.contains(&sub_id)
                        {
                            // unsubscribe
                            let (tx, rx) = oneshot::channel();
                            let command = Command::Unsubscribe(UnsubscribeRequest {
                                channel: (*sub.channel).into(),
                            });
                            let control_write = control_write1.clone();
                            subscribed_channels.remove(&sub_id);
                            let client = client.clone();
                            rt.spawn(async move {
                                let _ = control_write.send((command, tx, timeout)).await;
                                let _ = rx.await;
                                let mut inner = client.lock().unwrap();
                                if let Some(sub) = inner.subscriptions.get_mut(sub_id) {
                                    for ch in sub.on_unsubscribed_ch.drain(..) {
                                        let _ = ch.send(());
                                    }
                                }
                            });
                        }
                    }
                }

                JoinSetResult::Irrelevant
            },
            &rt,
        );

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(JoinSetResult::NewTask((task, mut new_tasks_rx))) => {
                    join_set.spawn_on(
                        async move {
                            if let Some(task) = new_tasks_rx.recv().await {
                                JoinSetResult::NewTask((Some(task), new_tasks_rx))
                            } else {
                                JoinSetResult::Irrelevant
                            }
                        },
                        &rt,
                    );
                    if let Some(task) = task {
                        join_set.spawn_on(task, &rt);
                    }
                }
                Ok(JoinSetResult::Irrelevant) => {}
                Err(err) => {
                    log::debug!("task failed: {:?}", err);
                    break;
                }
            }
        }
    }
}
