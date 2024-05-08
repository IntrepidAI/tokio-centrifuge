use thiserror::Error;

#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveSubscriptionError {
    #[error("subscription must be unsubscribed to be removed")]
    NotUnsubscribed,
}
