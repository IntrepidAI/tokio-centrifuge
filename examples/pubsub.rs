use tokio_centrifuge::client::Client;
use tokio_centrifuge::config::Config;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() {
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct Message {
        hello: i32,
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            Targets::new()
                .with_default(LevelFilter::INFO)
                .with_target("tokio_centrifuge", LevelFilter::TRACE)
        )
        .init();

    let client = Client::new(
        "ws://localhost:8000/connection/websocket?format=protobuf",
        Config::new().use_protobuf()
    );

    client.on_connecting(|| {
        log::info!("connecting");
    });
    client.on_connected(|| {
        log::info!("connected");
    });
    client.on_disconnected(|| {
        log::info!("disconnected");
    });
    client.on_error(|err| {
        log::info!("error: {:?}", err);
    });

    let sub = client.new_subscription("news");
    sub.on_subscribed(|e| {
        log::info!("subscribed to {}", e.channel);
    });
    sub.on_unsubscribed(|e| {
        log::info!("unsubscribed from {} (code={}, reason={})", e.channel, e.code, e.reason);
    });
    sub.on_subscribing(|e| {
        log::info!("subscribing to {} (code={}, reason={})", e.channel, e.code, e.reason);
    });
    sub.on_publication(|data| {
        let data: Message = serde_json::from_slice(&data.data).unwrap();
        log::info!("publication: {:?}", data);
    });
    sub.subscribe();

    client.connect();

    for i in 0.. {
        sub.publish(serde_json::to_vec(&Message { hello: i }).unwrap());
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
