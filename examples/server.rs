use std::task::Poll;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio_centrifuge::errors::DisconnectErrorCode;
use tokio_centrifuge::server::{AuthContext, Context, Server};

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
                if self.counter == 5 {
                    return Poll::Ready(None);
                }

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

    let mut server = Server::new();

    server.authenticate_with(async move |ctx: AuthContext| {
        if !ctx.token.is_empty() {
            return Err(DisconnectErrorCode::InvalidToken);
        }
        Ok(())
    });

    server.add_rpc_method("test/{id}", async move |ctx: Context, req: TestRequest| {
        log::debug!("rpc method test called: {:?}, id={:?}", &req, ctx.params.get("id"));
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(TestResponse {
            world: req.hello.to_string(),
        })
    }).unwrap();

    server.add_channel("test_channel", |_| {
        // if true {
        //     return Err(ClientErrorCode::PermissionDenied.into());
        // }
        log::debug!("channel test_channel connected");
        Ok(TestStream::new(Duration::from_millis(500)))
    }).unwrap();

    while let Ok((stream, addr)) = listener.accept().await {
        log::info!("accepted connection from: {}", addr);
        let server = server.clone();
        tokio::task::spawn(async move {
            let Ok(stream) = tokio_tungstenite::accept_async(stream).await
                .map_err(|err| log::error!("error: {:?}", err)) else { return };
            server.serve(stream, tokio_centrifuge::config::Protocol::Json).await;
            log::info!("closed connection with: {}", addr);
        });
    }
}
