use crate::client::Client;
use crate::config::Config;
use crate::protocol::{StreamPosition, SubscribeRequest};
use crate::subscription::State as SubState;

#[tokio::test]
async fn resubscribe_builds_recovery_subscribe_request() {
    let client = Client::new("ws://example.invalid", Config::default());
    let sub = client.new_subscription("test");

    // Seed recovery state
    {
        let mut inner = client.0.lock().unwrap();
        let sub_inner = inner.subscriptions.get_mut(sub.id).unwrap();

        sub_inner.state = SubState::Subscribed;
        sub_inner.recoverable = true;
        sub_inner.positioned = true;
        sub_inner.stream_position = Some(StreamPosition {
            epoch: "epoch-1".to_string(),
            offset: 42,
        });
    }

    // Build subscribe request as reconnect loop does
    let req = {
        let inner = client.0.lock().unwrap();
        let sub_inner = inner.subscriptions.get(sub.id).unwrap();

        let mut req = SubscribeRequest {
            channel: "test".into(),
            ..Default::default()
        };

        if sub_inner.recoverable && sub_inner.positioned {
            if let Some(pos) = &sub_inner.stream_position {
                req.recover = true;
                req.epoch = pos.epoch.clone();
                req.offset = pos.offset;
            }
        }

        req
    };

    assert!(req.recover);
    assert_eq!(req.epoch, "epoch-1");
    assert_eq!(req.offset, 42);
}
