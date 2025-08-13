//! Client types and state management.
//!
//! This module contains the core types used by the client implementation,
//! including connection states and message storage.

use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

use crate::client_handler::ReplyError;
use crate::protocol::Command;

/// Represents the current connection state of the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Client is not connected to any server
    Disconnected,
    /// Client is attempting to establish a connection
    Connecting,
    /// Client has successfully connected to the server
    Connected,
}

/// Internal structure for storing pending messages with their replies.
///
/// Each message item contains the command to be sent, a channel for receiving
/// the reply, and a deadline for timeout handling.
pub(crate) struct MessageStoreItem {
    /// The command to be sent to the server
    pub(crate) command: Command,
    /// Channel sender for receiving the reply
    pub(crate) reply: oneshot::Sender<Result<crate::protocol::Reply, ReplyError>>,
    /// When this message expires
    pub(crate) deadline: Instant,
}

impl MessageStoreItem {
    /// Checks if the message has expired and handles timeout if needed.
    ///
    /// Returns `Some(self)` if the message is still valid, or `None` if expired.
    /// If expired, sends a timeout error through the reply channel.
    fn check_expiration(self, now: Instant) -> Option<Self> {
        if self.deadline > now {
            Some(self)
        } else {
            let _ = self.reply.send(Err(ReplyError::Timeout));
            None
        }
    }
}

/// Manages a queue of pending messages with timeout handling.
///
/// The message store maintains a queue of commands that are waiting for replies
/// from the server. It automatically handles message expiration and provides
/// activity notifications when new messages are added.
pub(crate) struct MessageStore {
    /// Timeout duration for messages
    timeout: Duration,
    /// Channel for notifying about new message activity
    activity: mpsc::Sender<()>,
    /// Queue of pending messages
    messages: VecDeque<MessageStoreItem>,
}

impl MessageStore {
    /// Creates a new message store with the specified timeout.
    ///
    /// Returns a tuple of (store, activity_receiver) where the receiver
    /// can be used to wait for new message activity.
    pub fn new(timeout: Duration) -> (Self, mpsc::Receiver<()>) {
        let (activity_tx, activity_rx) = mpsc::channel(1);
        let store = Self {
            timeout,
            activity: activity_tx,
            messages: VecDeque::new(),
        };
        (store, activity_rx)
    }

    /// Adds a new command to the message store and returns a receiver for the reply.
    ///
    /// The command will be stored with a deadline based on the store's timeout.
    /// Returns a receiver that will receive the server's reply or an error.
    pub fn send(
        &mut self,
        command: Command,
    ) -> oneshot::Receiver<Result<crate::protocol::Reply, ReplyError>> {
        let (tx, rx) = oneshot::channel();
        let now = Instant::now();
        let deadline = now + self.timeout;
        self.messages.push_back(MessageStoreItem {
            command,
            reply: tx,
            deadline,
        });
        while let Some(item) = self.messages.pop_front() {
            if let Some(item) = item.check_expiration(now) {
                self.messages.push_front(item);
                break;
            }
        }
        let _ = self.activity.try_send(());
        rx
    }

    /// Resets the activity channel and returns the new receiver.
    ///
    /// This is used when the message store needs to be recreated,
    /// typically during reconnection scenarios.
    pub(crate) fn reset_channel(&mut self) -> mpsc::Receiver<()> {
        let (activity_tx, activity_rx) = mpsc::channel(1);
        self.activity = activity_tx;
        activity_rx
    }

    /// Retrieves the next valid message from the queue.
    ///
    /// Returns the next message that hasn't expired, or `None` if the queue is empty.
    /// Expired messages are automatically removed during this operation.
    pub(crate) fn get_next(&mut self, time: Instant) -> Option<MessageStoreItem> {
        loop {
            let item = self.messages.pop_front()?;
            if let Some(item) = item.check_expiration(time) {
                return Some(item);
            }
        }
    }
}
