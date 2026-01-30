use std::time::Duration;

use tokio_centrifuge::client::Client;
use tokio_centrifuge::config::Config;
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            Targets::new()
                .with_default(LevelFilter::INFO)
                .with_target("tokio_centrifuge", LevelFilter::TRACE),
        )
        .init();

    let client = Client::new(
        "ws://localhost:8000/connection/websocket?format=json",
        Config::new(),
    );

    client.on_connecting(|e| {
        log::info!("connecting: code={} reason={}", e.code, e.reason);
    });

    client.on_connected(|_| {
        log::info!("connected");
    });

    client.on_disconnected(|e| {
        log::info!("disconnected: code={}, reason={}", e.code, e.reason);
    });

    client.on_error(|err| {
        log::error!("client error: {:?}", err);
    });

    let sub = client.new_subscription("test");

    sub.on_subscribing(|e| {
        log::info!(
            "subscribing to {} (code={}, reason={})",
            e.channel,
            e.code,
            e.reason
        );
    });

    sub.on_subscribed(|e| {
        log::info!("subscribed to {}", e.channel);
    });

    sub.on_unsubscribed(|e| {
        log::info!(
            "unsubscribed from {} (reason={})",
            e.channel,
            e.reason
        );
    });

    sub.on_publication(|pub_| {
        log::info!(
            "publication: offset={} data={:?}",
            pub_.offset,
            String::from_utf8_lossy(&pub_.data)
        );
    });

    //subscribe intent first, then connect.
    sub.subscribe();
    client.connect();

    // Force a disconnect after a short delay, wait 10s, then reconnect.
    tokio::spawn({
        let client = client.clone();
        async move {
            tokio::time::sleep(Duration::from_secs(5)).await;

            log::info!("FORCING DISCONNECT for 10s. Publish messages now.");
            client.disconnect();

            tokio::time::sleep(Duration::from_secs(10)).await;

            log::info!("RECONNECTING now. Missed messages should replay if recovery is enabled.");
            client.connect();
        }
    });

    // keep process alive
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
