use std::borrow::Cow;
use std::fmt::Display;

use async_tungstenite::tungstenite::protocol::CloseFrame;
use thiserror::Error;

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveSubscriptionError {
    #[error("subscription must be unsubscribed to be removed")]
    NotUnsubscribed,
}

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCode(pub u16);

#[allow(non_upper_case_globals)]
impl ErrorCode {
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

    pub fn to_str(self) -> Cow<'static, str> {
        match self.0 {
            3000 => Cow::Borrowed("connection closed"),
            3001 => Cow::Borrowed("shutdown"),
            3004 => Cow::Borrowed("internal server error"),
            3005 => Cow::Borrowed("connection expired"),
            3006 => Cow::Borrowed("subscription expired"),
            3008 => Cow::Borrowed("slow"),
            3009 => Cow::Borrowed("write error"),
            3010 => Cow::Borrowed("insufficient state"),
            3011 => Cow::Borrowed("force reconnect"),
            3012 => Cow::Borrowed("no pong"),
            3013 => Cow::Borrowed("too many requests"),
            3500 => Cow::Borrowed("invalid token"),
            3501 => Cow::Borrowed("bad request"),
            3502 => Cow::Borrowed("stale"),
            3503 => Cow::Borrowed("force disconnect"),
            3504 => Cow::Borrowed("connection limit"),
            3505 => Cow::Borrowed("channel limit"),
            3506 => Cow::Borrowed("inappropriate protocol"),
            3507 => Cow::Borrowed("permission denied"),
            3508 => Cow::Borrowed("not available"),
            3509 => Cow::Borrowed("too many errors"),
            _ => Cow::Owned(format!("unknown code {}", self.0)),
        }
    }
}

impl Display for ErrorCode {
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

impl From<ErrorCode> for u16 {
    fn from(code: ErrorCode) -> Self {
        code.0
    }
}

impl From<u16> for ErrorCode {
    fn from(code: u16) -> Self {
        Self(code)
    }
}

impl From<ErrorCode> for Option<CloseFrame<'_>> {
    fn from(code: ErrorCode) -> Self {
        Some(CloseFrame {
            code: code.0.into(),
            reason: code.to_string().into(),
        })
    }
}
