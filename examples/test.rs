use tokio_centrifuge::{client::Centrifuge, config::Config};

#[tokio::main]
async fn main() {
    simple_logger::SimpleLogger::new().with_level(log::LevelFilter::Debug).init().unwrap();

    let client = Centrifuge::new("ws://172.17.185.187:8000/connection/websocket", Config::new());

    client.on_connected(|| {
        log::info!("connected");
    });
    client.on_error(|err| {
        log::info!("error: {:?}", err);
    });
    client.on_disconnected(|| {
        log::info!("disconnected");
    });
    client.on_connecting(|| {
        log::info!("connecting");
    });

    client.connect();
    tokio::signal::ctrl_c().await.unwrap();
}
