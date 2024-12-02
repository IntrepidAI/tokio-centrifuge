use std::task::Poll;
use std::time::Duration;

use async_std::net::TcpListener;
use tokio_centrifuge::server::Server;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TestStreamItem {
    foobar: u32,
}

struct TestStream {
    counter: u32,
    interval: tokio::time::Interval,
}

impl TestStream {
    fn new(interval: Duration) -> Self {
        Self {
            counter: 0,
            interval: tokio::time::interval(interval),
        }
    }
}

impl futures::Stream for TestStream {
    type Item = TestStreamItem;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.as_mut().interval.poll_tick(cx) {
            Poll::Ready(_) => {
                self.as_mut().counter += 1;
                Poll::Ready(Some(TestStreamItem { foobar: self.counter }))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

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
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(TestResponse {
            world: req.hello.to_string(),
        })
    });

    server.add_channel("test_channel", || {
        TestStream::new(Duration::from_millis(500))
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
