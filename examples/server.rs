use std::task::Poll;
use std::time::Duration;

use futures::StreamExt;
use tokio::net::TcpListener;
use tokio_centrifuge::errors::DisconnectErrorCode;
use tokio_centrifuge::server::{Server, types::ConnectContext};
use tokio_centrifuge::utils::{decode_json, encode_json};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::prelude::*;

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
                Poll::Ready(Some(TestStreamItem {
                    foobar: self.counter,
                }))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

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

    let listener = TcpListener::bind("127.0.0.1:8000").await.unwrap();
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

    server
        .add_rpc_method("test/{id}", async move |ctx, req| {
            let req: TestRequest = decode_json(&req)?;
            log::debug!(
                "rpc method test called: {:?}, id={:?}",
                &req,
                ctx.params.get("id")
            );
            tokio::time::sleep(Duration::from_secs(2)).await;
            encode_json(&TestResponse {
                world: req.hello.to_string(),
            })
        })
        .unwrap();

    server
        .add_channel("test_channel", async |_ctx| {
            // dbg!(_ctx.data);
            // if true {
            //     return Err(ClientErrorCode::PermissionDenied.into());
            // }
            tokio::time::sleep(Duration::from_secs(2)).await;
            log::debug!("channel test_channel connected");
            Ok(TestStream::new(Duration::from_millis(500))
                .filter_map(async move |item| encode_json(&item).ok()))
        })
        .unwrap();

    let (echo_channel_tx, _) = tokio::sync::broadcast::channel::<Vec<u8>>(16);

    let echo_channel_tx_ = echo_channel_tx.clone();
    server
        .add_channel("echo_channel", move |mut _ctx| {
            let mut echo_channel_rx = echo_channel_tx_.subscribe();

            async move {
                Ok(async_stream::stream! {
                    while let Ok(msg) = echo_channel_rx.recv().await {
                        yield msg;
                    }

                    // for i in 0.. {
                    //     tokio::time::sleep(Duration::from_millis(500)).await;
                    //     yield TestStreamItem { foobar: i };
                    // }
                })
            }
        })
        .unwrap();

    server
        .on_publication("echo_channel", move |_ctx, data| {
            let echo_channel_tx = echo_channel_tx.clone();

            async move {
                let _ = echo_channel_tx.send(data);
                Ok(())
            }
        })
        .unwrap();

    while let Ok((stream, addr)) = listener.accept().await {
        log::info!("accepted connection from: {}", addr);
        let server = server.clone();
        tokio::task::spawn(async move {
            let Ok(stream) = tokio_tungstenite::accept_async(stream)
                .await
                .map_err(|err| log::error!("error: {:?}", err))
            else {
                return;
            };
            server
                .serve(stream, tokio_centrifuge::config::Protocol::Json.into())
                .await;
            log::info!("closed connection with: {}", addr);
        });
    }
}
