//! # Protocol Module
//!
//! This module defines the core protocol types and structures used for
//! communication between clients and servers in the Centrifugo protocol.
//!
//! ## Protocol Overview
//!
//! The Centrifugo protocol is a bidirectional protocol that supports:
//! - **Client Commands**: Connect, subscribe, publish, RPC calls, etc.
//! - **Server Replies**: Responses to commands, push notifications, errors
//! - **Push Data**: Real-time updates, join/leave events, messages
//!
//! ## Core Types
//!
//! - **Command**: Client-to-server commands
//! - **Reply**: Server-to-client responses
//! - **Push**: Server-initiated messages
//! - **PushData**: Various types of push notifications
//!
//! ## Example
//!
//! ```rust
//! use tokio_centrifuge::protocol::{Command, Reply, ConnectRequest, ConnectResult};
//!
//! // Client sends connect command
//! let connect_cmd = Command::Connect(ConnectRequest {
//!     token: "auth-token".to_string(),
//!     data: vec![],
//!     subs: std::collections::HashMap::new(),
//!     name: "client".to_string(),
//!     version: "1.0.0".to_string(),
//! });
//!
//! // Server responds with connect result
//! let connect_reply = Reply::Connect(ConnectResult {
//!     client: "client-id".to_string(),
//!     version: "1.0.0".to_string(),
//!     expires: false,
//!     ttl: 0,
//!     data: vec![],
//!     node: "node-1".to_string(),
//!     ping: 25000,
//!     pong: false,
//!     session: "session-id".to_string(),
//!     subs: std::collections::HashMap::new(),
//! });
//! ```

#[allow(clippy::module_inception)]
mod protocol;

pub use protocol::*;
use serde::{Serialize, Serializer};

/// Client-to-server commands
///
/// This enum represents all the commands that clients can send to the server.
/// Each variant contains the specific request data for that command type.
#[derive(Debug, Clone)]
pub enum Command {
    /// Establish connection with authentication
    Connect(ConnectRequest),
    /// Subscribe to a channel
    Subscribe(SubscribeRequest),
    /// Unsubscribe from a channel
    Unsubscribe(UnsubscribeRequest),
    /// Publish data to a channel
    Publish(PublishRequest),
    /// Request presence information for a channel
    Presence(PresenceRequest),
    /// Request presence statistics for a channel
    PresenceStats(PresenceStatsRequest),
    /// Request message history for a channel
    History(HistoryRequest),
    /// Ping the server (keep-alive)
    Ping(PingRequest),
    /// Send a message to a channel
    Send(SendRequest),
    /// Make an RPC call to a method
    Rpc(RpcRequest),
    /// Refresh the connection token
    Refresh(RefreshRequest),
    /// Refresh a subscription token
    SubRefresh(SubRefreshRequest),
    /// Empty command (no-op)
    Empty,
}

/// Server-to-client replies
///
/// This enum represents all the responses that servers can send to clients.
/// Each variant contains the specific result data for that reply type.
#[derive(Debug, Clone)]
pub enum Reply {
    /// Server-initiated push notification
    Push(Push),
    /// Error response
    Error(Error),
    /// Connection establishment result
    Connect(ConnectResult),
    /// Subscription result
    Subscribe(SubscribeResult),
    /// Unsubscription result
    Unsubscribe(UnsubscribeResult),
    /// Publication result
    Publish(PublishResult),
    /// Presence information result
    Presence(PresenceResult),
    /// Presence statistics result
    PresenceStats(PresenceStatsResult),
    /// Message history result
    History(HistoryResult),
    /// Ping response
    Ping(PingResult),
    /// RPC call result
    Rpc(RpcResult),
    /// Token refresh result
    Refresh(RefreshResult),
    /// Subscription refresh result
    SubRefresh(SubRefreshResult),
    /// Empty reply (no-op)
    Empty,
}

/// Server-initiated message
///
/// This struct represents messages that the server sends to clients
/// without a corresponding request, such as real-time updates.
#[derive(Debug, Clone)]
pub struct Push {
    /// The channel this push message is for
    pub channel: String,
    /// The push data content
    pub data: PushData,
}

/// Types of push notifications
///
/// This enum defines the various types of push data that can be sent
/// from server to client, including publications, join/leave events,
/// and other real-time updates.
#[derive(Debug, Clone)]
pub enum PushData {
    /// New publication in a channel
    Publication(Publication),
    /// Client joined a channel
    Join(Join),
    /// Client left a channel
    Leave(Leave),
    /// Client unsubscribed from a channel
    Unsubscribe(Unsubscribe),
    /// General message
    Message(Message),
    /// Subscription event
    Subscribe(Subscribe),
    /// Connection event
    Connect(Box<Connect>),
    /// Disconnection event
    Disconnect(Disconnect),
    /// Token refresh event
    Refresh(Refresh),
    /// Empty push data
    Empty,
}

impl From<Command> for RawCommand {
    fn from(value: Command) -> Self {
        let mut result = RawCommand::default();

        match value {
            Command::Connect(v) => {
                result.connect = Some(v);
            }
            Command::Subscribe(v) => {
                result.subscribe = Some(v);
            }
            Command::Unsubscribe(v) => {
                result.unsubscribe = Some(v);
            }
            Command::Publish(v) => {
                result.publish = Some(v);
            }
            Command::Presence(v) => {
                result.presence = Some(v);
            }
            Command::PresenceStats(v) => {
                result.presence_stats = Some(v);
            }
            Command::History(v) => {
                result.history = Some(v);
            }
            Command::Ping(v) => {
                result.ping = Some(v);
            }
            Command::Send(v) => {
                result.send = Some(v);
            }
            Command::Rpc(v) => {
                result.rpc = Some(v);
            }
            Command::Refresh(v) => {
                result.refresh = Some(v);
            }
            Command::SubRefresh(v) => {
                result.sub_refresh = Some(v);
            }
            Command::Empty => {}
        }

        result
    }
}

impl From<Reply> for RawReply {
    fn from(value: Reply) -> Self {
        let mut result = RawReply::default();

        match value {
            Reply::Error(v) => {
                result.error = Some(v);
            }
            Reply::Push(v) => {
                result.push = Some(v.into());
            }
            Reply::Connect(v) => {
                result.connect = Some(v);
            }
            Reply::Subscribe(v) => {
                result.subscribe = Some(v);
            }
            Reply::Unsubscribe(v) => {
                result.unsubscribe = Some(v);
            }
            Reply::Publish(v) => {
                result.publish = Some(v);
            }
            Reply::Presence(v) => {
                result.presence = Some(v);
            }
            Reply::PresenceStats(v) => {
                result.presence_stats = Some(v);
            }
            Reply::History(v) => {
                result.history = Some(v);
            }
            Reply::Ping(v) => {
                result.ping = Some(v);
            }
            Reply::Rpc(v) => {
                result.rpc = Some(v);
            }
            Reply::Refresh(v) => {
                result.refresh = Some(v);
            }
            Reply::SubRefresh(v) => {
                result.sub_refresh = Some(v);
            }
            Reply::Empty => {}
        }

        result
    }
}

impl From<Push> for RawPush {
    fn from(value: Push) -> Self {
        let mut result = RawPush {
            channel: value.channel,
            ..Default::default()
        };

        match value.data {
            PushData::Publication(v) => {
                result.publication = Some(v);
            }
            PushData::Join(v) => {
                result.join = Some(v);
            }
            PushData::Leave(v) => {
                result.leave = Some(v);
            }
            PushData::Unsubscribe(v) => {
                result.unsubscribe = Some(v);
            }
            PushData::Message(v) => {
                result.message = Some(v);
            }
            PushData::Subscribe(v) => {
                result.subscribe = Some(v);
            }
            PushData::Connect(v) => {
                result.connect = Some(*v);
            }
            PushData::Disconnect(v) => {
                result.disconnect = Some(v);
            }
            PushData::Refresh(v) => {
                result.refresh = Some(v);
            }
            PushData::Empty => {}
        }

        result
    }
}

impl From<RawCommand> for Command {
    fn from(value: RawCommand) -> Self {
        if let Some(v) = value.connect {
            Self::Connect(v)
        } else if let Some(v) = value.subscribe {
            Self::Subscribe(v)
        } else if let Some(v) = value.unsubscribe {
            Self::Unsubscribe(v)
        } else if let Some(v) = value.publish {
            Self::Publish(v)
        } else if let Some(v) = value.presence {
            Self::Presence(v)
        } else if let Some(v) = value.presence_stats {
            Self::PresenceStats(v)
        } else if let Some(v) = value.history {
            Self::History(v)
        } else if let Some(v) = value.ping {
            Self::Ping(v)
        } else if let Some(v) = value.send {
            Self::Send(v)
        } else if let Some(v) = value.rpc {
            Self::Rpc(v)
        } else if let Some(v) = value.refresh {
            Self::Refresh(v)
        } else if let Some(v) = value.sub_refresh {
            Self::SubRefresh(v)
        } else {
            Self::Empty
        }
    }
}

impl From<RawReply> for Reply {
    fn from(value: RawReply) -> Self {
        if let Some(v) = value.error {
            Self::Error(v)
        } else if let Some(v) = value.push {
            Self::Push(v.into())
        } else if let Some(v) = value.connect {
            Self::Connect(v)
        } else if let Some(v) = value.subscribe {
            Self::Subscribe(v)
        } else if let Some(v) = value.unsubscribe {
            Self::Unsubscribe(v)
        } else if let Some(v) = value.publish {
            Self::Publish(v)
        } else if let Some(v) = value.presence {
            Self::Presence(v)
        } else if let Some(v) = value.presence_stats {
            Self::PresenceStats(v)
        } else if let Some(v) = value.history {
            Self::History(v)
        } else if let Some(v) = value.ping {
            Self::Ping(v)
        } else if let Some(v) = value.rpc {
            Self::Rpc(v)
        } else if let Some(v) = value.refresh {
            Self::Refresh(v)
        } else if let Some(v) = value.sub_refresh {
            Self::SubRefresh(v)
        } else {
            Self::Empty
        }
    }
}

impl From<RawPush> for Push {
    fn from(value: RawPush) -> Self {
        let data = if let Some(v) = value.publication {
            PushData::Publication(v)
        } else if let Some(v) = value.join {
            PushData::Join(v)
        } else if let Some(v) = value.leave {
            PushData::Leave(v)
        } else if let Some(v) = value.unsubscribe {
            PushData::Unsubscribe(v)
        } else if let Some(v) = value.message {
            PushData::Message(v)
        } else if let Some(v) = value.subscribe {
            PushData::Subscribe(v)
        } else if let Some(v) = value.connect {
            PushData::Connect(Box::new(v))
        } else if let Some(v) = value.disconnect {
            PushData::Disconnect(v)
        } else if let Some(v) = value.refresh {
            PushData::Refresh(v)
        } else {
            PushData::Empty
        };

        Self {
            channel: value.channel,
            data,
        }
    }
}

fn serialize_json<T: AsRef<[u8]>, S: Serializer>(v: &T, serializer: S) -> Result<S::Ok, S::Error> {
    let value: serde_json::Value = serde_json::from_slice(v.as_ref()).map_err(|e| {
        serde::ser::Error::custom(format!(
            "unable to serialize to json: {e}, consider using protobuf instead"
        ))
    })?;
    value.serialize(serializer)
}

fn deserialize_json<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    let value: serde_json::Value = serde::de::Deserialize::deserialize(deserializer)?;
    serde_json::to_vec(&value).map_err(serde::de::Error::custom)
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}
