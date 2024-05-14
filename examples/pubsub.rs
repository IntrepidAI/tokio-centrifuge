use tokio_centrifuge::client::Client;
use tokio_centrifuge::config::Config;

#[tokio::main]
async fn main() {
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct Message {
        hello: i32,
    }

    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .with_module_level("tokio_centrifuge", log::LevelFilter::Trace)
        .init().unwrap();

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
    sub.on_subscribed(|| {
        log::info!("subscribed to news");
    });
    sub.on_unsubscribed(|| {
        log::info!("unsubscribed from news");
    });
    sub.on_subscribing(|| {
        log::info!("subscribing to news");
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
