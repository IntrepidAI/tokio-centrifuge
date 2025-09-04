#[derive(Debug)]
/// ConnectedEvent is a connected event context passed to `Client::on_connected` callback.
pub struct ConnectedEvent<'a> {
    pub client_id: &'a str,
    pub version: &'a str,
    pub data: Vec<u8>,
}

#[derive(Debug)]
/// ConnectingEvent is a connecting event context passed to `Client::on_connecting` callback.
pub struct ConnectingEvent<'a> {
    pub code: u32,
    pub reason: &'a str,
}

#[derive(Debug)]
/// DisconnectedEvent is a disconnected event context passed to `Client::on_disconnected` callback.
pub struct DisconnectedEvent<'a> {
    pub code: u32,
    pub reason: &'a str,
}

#[derive(Debug)]
/// SubscribedEvent is a subscribed event context passed to `Subscription::on_subscribed` callback.
pub struct SubscribedEvent<'a> {
    pub channel: &'a str,
    pub data: Vec<u8>,
}

#[derive(Debug)]
/// SubscribingEvent is a subscribing event context passed to `Subscription::on_subscribing` callback.
pub struct SubscribingEvent<'a> {
    pub channel: &'a str,
    pub code: u32,
    pub reason: &'a str,
}

#[derive(Debug)]
/// UnsubscribedEvent is a unsubscribed event context passed to `Subscription::on_unsubscribed` callback.
pub struct UnsubscribedEvent<'a> {
    pub channel: &'a str,
    pub code: u32,
    pub reason: &'a str,
}
