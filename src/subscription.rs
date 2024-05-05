use std::{sync::Arc, time::{Duration, Instant}};

use futures::Future;
use slotmap::new_key_type;
use tokio::sync::oneshot;

use crate::{client::{Client, FutureResult, MessageStore}, client_handler::ReplyError, protocol::Reply};

new_key_type! { pub(crate) struct SubscriptionId; }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Unsubscribed,
    Subscribing,
    Subscribed,
}

pub(crate) struct SubscriptionInner {
    pub(crate) channel: Arc<str>,
    pub(crate) state: State,
    on_subscribing: Option<Box<dyn FnMut() + Send + 'static>>,
    on_subscribed: Option<Box<dyn FnMut() + Send + 'static>>,
    on_unsubscribed: Option<Box<dyn FnMut() + Send + 'static>>,
    on_error: Option<Box<dyn FnMut(anyhow::Error) + Send + 'static>>,
    pub(crate) on_subscribed_ch: Vec<oneshot::Sender<Result<(), ()>>>,
    pub(crate) on_unsubscribed_ch: Vec<oneshot::Sender<()>>,
    pub(crate) pub_ch_write: Option<MessageStore>,
    read_timeout: Duration,
}

impl SubscriptionInner {
    pub fn new(channel: &str, read_timeout: Duration) -> Self {
        SubscriptionInner {
            channel: channel.into(),
            state: State::Unsubscribed,
            on_subscribing: None,
            on_subscribed: None,
            on_unsubscribed: None,
            on_error: None,
            on_subscribed_ch: Vec::new(),
            on_unsubscribed_ch: Vec::new(),
            pub_ch_write: None,
            read_timeout,
        }
    }

    pub fn move_to_subscribing(&mut self) {
        if self.pub_ch_write.is_none() {
            let (pub_ch_write, _) = MessageStore::new(self.read_timeout);
            self.pub_ch_write = Some(pub_ch_write);
        }
        self._set_state(State::Subscribing);
    }

    pub fn move_to_subscribed(&mut self) {
        self._set_state(State::Subscribed);
    }

    pub fn move_to_unsubscribed(&mut self) {
        self.pub_ch_write = None;
        self._set_state(State::Unsubscribed);
    }

    fn _set_state(&mut self, state: State) {
        log::debug!("state: {:?} -> {:?}, channel={}", self.state, state, self.channel);
        self.state = state;

        match state {
            State::Unsubscribed => {
                if let Some(ref mut on_unsubscribed) = self.on_unsubscribed {
                    on_unsubscribed();
                }
            }
            State::Subscribing => {
                if let Some(ref mut on_subscribing) = self.on_subscribing {
                    on_subscribing();
                }
            }
            State::Subscribed => {
                if let Some(ref mut on_subscribed) = self.on_subscribed {
                    on_subscribed();
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct Subscription {
    pub(crate) id: SubscriptionId,
    client: Client,
}

impl Subscription {
    pub(crate) fn new(client: &Client, key: SubscriptionId) -> Self {
        Subscription {
            id: key,
            client: client.clone(),
        }
    }

    pub fn subscribe(&self) -> FutureResult<impl Future<Output = Result<(), ()>>> {
        let (tx, rx) = oneshot::channel();
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            if sub.state != State::Unsubscribed {
                let _ = tx.send(Err(()));
            } else {
                sub.on_subscribed_ch.push(tx);
                sub.move_to_subscribing();
                if let Some(channel) = inner.sub_ch_write.as_ref() {
                    let _ = channel.send(self.id);
                }
            }
        } else {
            let _ = tx.send(Err(()));
        }
        FutureResult(async {
            match rx.await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(())) => Err(()),
                Err(_) => Err(()),
            }
        })
    }

    pub fn unsubscribe(&self) -> FutureResult<impl Future<Output = ()>> {
        let (tx, rx) = oneshot::channel();
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            if sub.state == State::Unsubscribed {
                let _ = tx.send(());
            } else {
                sub.on_unsubscribed_ch.push(tx);
                sub.move_to_unsubscribed();
                if let Some(channel) = inner.sub_ch_write.as_ref() {
                    let _ = channel.send(self.id);
                }
            }
        } else {
            let _ = tx.send(());
        }
        FutureResult(async {
            let _ = rx.await;
        })
    }

    pub fn publish(
        &self,
        data: Vec<u8>,
    ) -> FutureResult<impl Future<Output = Result<(), ()>>> {
        let mut inner = self.client.0.lock().unwrap();
        let read_timeout = inner.read_timeout;
        let deadline = Instant::now() + read_timeout;
        let rx = if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            if let Some(ref mut pub_ch_write) = sub.pub_ch_write {
                pub_ch_write.publish(sub.channel.clone(), data)
            } else {
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(Err(ReplyError::Closed));
                rx
            }
        } else {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Err(ReplyError::Closed));
            rx
        };
        FutureResult(async move {
            let result = tokio::time::timeout_at(deadline.into(), rx).await;
            if let Ok(Ok(Ok(Reply::Publish(_)))) = result {
                Ok(())
            } else {
                Err(())
            }
        })
    }

    pub fn on_subscribing(&self, func: impl FnMut() + Send + 'static) {
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            sub.on_subscribing = Some(Box::new(func));
        }
    }

    pub fn on_subscribed(&self, func: impl FnMut() + Send + 'static) {
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            sub.on_subscribed = Some(Box::new(func));
        }
    }

    pub fn on_unsubscribed(&self, func: impl FnMut() + Send + 'static) {
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            sub.on_unsubscribed = Some(Box::new(func));
        }
    }

    pub fn on_error(&self, func: impl FnMut(anyhow::Error) + Send + 'static) {
        let mut inner = self.client.0.lock().unwrap();
        if let Some(sub) = inner.subscriptions.get_mut(self.id) {
            sub.on_error = Some(Box::new(func));
        }
    }

    pub fn state(&self) -> State {
        let inner = self.client.0.lock().unwrap();
        inner.subscriptions.get(self.id).map(|s| s.state).unwrap_or(State::Unsubscribed)
    }
}
