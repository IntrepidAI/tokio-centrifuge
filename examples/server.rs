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

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct TestRequest {
        hello: i32,
    }
    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct TestResponse {
        world: String,
    }

    let server = Server::new();
    server.add_rpc_method("test", |req: TestRequest| async move {
        log::debug!("rpc method test called: {:?}", req);
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        Ok(TestResponse {
            world: req.hello.to_string(),
        })
    });

    while let Ok((stream, addr)) = listener.accept().await {
        log::info!("accepted connection from: {}", addr);
        let server = server.clone();
        tokio::task::spawn(async move {
            let _ = server.accept_async(stream, tokio_centrifuge::config::Protocol::Json).await
                .map_err(|err| log::error!("error: {:?}", err));
            log::info!("closed connection with: {}", addr);
        });
    }
}
