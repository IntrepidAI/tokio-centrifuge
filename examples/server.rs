use std::task::Poll;
use std::time::Duration;

use futures::StreamExt;
use tokio::net::TcpListener;
use tokio_centrifuge::errors::DisconnectErrorCode;
use tokio_centrifuge::server::{ConnectContext, Server};
use tokio_centrifuge::utils::{decode_json, encode_json};

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

    server.on_connect(async move |ctx: ConnectContext| {
        if !ctx.token.is_empty() {
            return Err(DisconnectErrorCode::InvalidToken);
        }
        Ok(())
    });

    server.add_rpc_method("test/{id}", async move |ctx, req| {
        let req: TestRequest = decode_json(&req)?;
        log::debug!("rpc method test called: {:?}, id={:?}", &req, ctx.params.get("id"));
        tokio::time::sleep(Duration::from_secs(2)).await;
        encode_json(TestResponse {
            world: req.hello.to_string(),
        })
    }).unwrap();

    server.add_channel("test_channel", async |_ctx| {
        // dbg!(_ctx.data);
        // if true {
        //     return Err(ClientErrorCode::PermissionDenied.into());
        // }
        tokio::time::sleep(Duration::from_secs(2)).await;
        log::debug!("channel test_channel connected");
        Ok(TestStream::new(Duration::from_millis(500)).filter_map(async move |item| {
            encode_json(item).ok()
        }))
    }).unwrap();

    server.add_channel("echo_channel", async |mut ctx| {
        Ok(async_stream::stream! {
            while let Some(msg) = ctx.stream.recv().await {
                yield serde_json::from_slice(&msg).unwrap_or(serde_json::Value::Null);
            }

            // for i in 0.. {
            //     tokio::time::sleep(Duration::from_millis(500)).await;
            //     yield TestStreamItem { foobar: i };
            // }
        }.filter_map(async move |item| {
            encode_json(item).ok()
        }))
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
