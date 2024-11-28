use async_std::net::TcpListener;
use tokio_centrifuge::server::Server;

#[tokio::main]
async fn main() {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .with_module_level("tokio_centrifuge", log::LevelFilter::Trace)
        .init().unwrap();

    let listener = TcpListener::bind("127.0.0.1:8001").await.unwrap();
    log::info!("listening on: {}", listener.local_addr().unwrap());

    while let Ok((stream, addr)) = listener.accept().await {
        log::info!("accepted connection from: {}", addr);
        tokio::task::spawn(async move {
            let _ = Server::accept_async(stream, tokio_centrifuge::config::Protocol::Json).await
                .map_err(|err| log::error!("error: {:?}", err));
            log::info!("closed connection with: {}", addr);
        });
    }
}
