#[allow(clippy::module_inception)]
mod protocol;

pub use protocol::*;
use serde::{Serialize, Serializer};

#[derive(Debug, Clone)]
pub enum Command {
    Connect(ConnectRequest),
    Subscribe(SubscribeRequest),
    Unsubscribe(UnsubscribeRequest),
    Publish(PublishRequest),
    Presence(PresenceRequest),
    PresenceStats(PresenceStatsRequest),
    History(HistoryRequest),
    Ping(PingRequest),
    Send(SendRequest),
    Rpc(RpcRequest),
    Refresh(RefreshRequest),
    SubRefresh(SubRefreshRequest),
    Empty,
}

#[derive(Debug, Clone)]
pub enum Reply {
    Push(Push),
    Error(Error),
    Connect(ConnectResult),
    Subscribe(SubscribeResult),
    Unsubscribe(UnsubscribeResult),
    Publish(PublishResult),
    Presence(PresenceResult),
    PresenceStats(PresenceStatsResult),
    History(HistoryResult),
    Ping(PingResult),
    Rpc(RpcResult),
    Refresh(RefreshResult),
    SubRefresh(SubRefreshResult),
    Empty,
}

#[derive(Debug, Clone)]
pub struct Push {
    pub channel: String,
    pub data: PushData,
}

#[derive(Debug, Clone)]
pub enum PushData {
    Publication(Publication),
    Join(Join),
    Leave(Leave),
    Unsubscribe(Unsubscribe),
    Message(Message),
    Subscribe(Subscribe),
    Connect(Box<Connect>),
    Disconnect(Disconnect),
    Refresh(Refresh),
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
    let value: serde_json::Value = serde_json::from_slice(v.as_ref())
        .map_err(|e| serde::ser::Error::custom(
            format!("unable to serialize to json: {e}, consider using protobuf instead")
        ))?;
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
