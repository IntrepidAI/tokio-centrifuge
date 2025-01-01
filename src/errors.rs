use std::fmt::Display;

use thiserror::Error;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveSubscriptionError {
    #[error("subscription must be unsubscribed to be removed")]
    NotUnsubscribed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientError {
    pub code: ClientErrorCode,
    pub message: String,
}

impl From<anyhow::Error> for ClientError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            code: ClientErrorCode::Internal,
            message: err.to_string(),
        }
    }
}

impl From<ClientError> for crate::protocol::Error {
    fn from(err: ClientError) -> Self {
        Self {
            code: err.code.0.into(),
            message: err.message,
            temporary: err.code.is_temporary(),
        }
    }
}

impl ClientError {
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: ClientErrorCode::Internal,
            message: message.into(),
        }
    }
}

impl From<ClientErrorCode> for ClientError {
    fn from(code: ClientErrorCode) -> Self {
        Self {
            code,
            message: code.to_string(),
        }
    }
}

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientErrorCode(pub u16);

#[allow(non_upper_case_globals)]
impl ClientErrorCode {
    pub const Internal:              Self = Self(100);
    pub const Unauthorized:          Self = Self(101);
    pub const UnknownChannel:        Self = Self(102);
    pub const PermissionDenied:      Self = Self(103);
    pub const MethodNotFound:        Self = Self(104);
    pub const AlreadySubscribed:     Self = Self(105);
    pub const LimitExceeded:         Self = Self(106);
    pub const BadRequest:            Self = Self(107);
    pub const NotAvailable:          Self = Self(108);
    pub const TokenExpired:          Self = Self(109);
    pub const Expired:               Self = Self(110);
    pub const TooManyRequests:       Self = Self(111);
    pub const UnrecoverablePosition: Self = Self(112);

    pub fn is_temporary(self) -> bool {
        matches!(self, Self::Internal | Self::TooManyRequests)
    }
}

impl Display for ClientErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            100 => write!(f, "internal server error"),
            101 => write!(f, "unauthorized"),
            102 => write!(f, "unknown channel"),
            103 => write!(f, "permission denied"),
            104 => write!(f, "method not found"),
            105 => write!(f, "already subscribed"),
            106 => write!(f, "limit exceeded"),
            107 => write!(f, "bad request"),
            108 => write!(f, "not available"),
            109 => write!(f, "token expired"),
            110 => write!(f, "expired"),
            111 => write!(f, "too many requests"),
            112 => write!(f, "unrecoverable position"),
            _ => write!(f, "unknown code {}", self.0),
        }
    }
}

impl From<ClientErrorCode> for u16 {
    fn from(code: ClientErrorCode) -> Self {
        code.0
    }
}

impl From<u16> for ClientErrorCode {
    fn from(code: u16) -> Self {
        Self(code)
    }
}

impl From<ClientErrorCode> for crate::protocol::Error {
    fn from(code: ClientErrorCode) -> Self {
        Self {
            code: code.0.into(),
            message: code.to_string(),
            temporary: code.is_temporary(),
        }
    }
}

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisconnectErrorCode(pub u16);

#[allow(non_upper_case_globals)]
impl DisconnectErrorCode {
    pub const ConnectionClosed:       Self = Self(3000);
    pub const Shutdown:               Self = Self(3001);
    pub const ServerError:            Self = Self(3004);
    pub const Expired:                Self = Self(3005);
    pub const SubExpired:             Self = Self(3006);
    pub const Slow:                   Self = Self(3008);
    pub const WriteError:             Self = Self(3009);
    pub const InsufficientState:      Self = Self(3010);
    pub const ForceReconnect:         Self = Self(3011);
    pub const NoPong:                 Self = Self(3012);
    pub const TooManyRequests:        Self = Self(3013);
    pub const InvalidToken:           Self = Self(3500);
    pub const BadRequest:             Self = Self(3501);
    pub const Stale:                  Self = Self(3502);
    pub const ForceNoReconnect:       Self = Self(3503);
    pub const ConnectionLimit:        Self = Self(3504);
    pub const ChannelLimit:           Self = Self(3505);
    pub const InappropriateProtocol:  Self = Self(3506);
    pub const PermissionDenied:       Self = Self(3507);
    pub const NotAvailable:           Self = Self(3508);
    pub const TooManyErrors:          Self = Self(3509);

    pub fn should_reconnect(self) -> bool {
        !matches!(self.0, 3500..=3999 | 4500..=4999)
    }
}

impl Display for DisconnectErrorCode {
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
    fn from(code: DisconnectErrorCode) -> Self {
        code.0
    }
}

impl From<u16> for DisconnectErrorCode {
    fn from(code: u16) -> Self {
        Self(code)
    }
}

impl From<DisconnectErrorCode> for Option<CloseFrame> {
    fn from(code: DisconnectErrorCode) -> Self {
        Some(CloseFrame {
            code: code.0.into(),
            reason: code.to_string().into(),
        })
    }
}
