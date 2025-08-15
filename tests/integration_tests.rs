use std::time::Duration;
use tokio::net::TcpListener;
use tokio_centrifuge::{
    client::Client,
    config::Config,
    server::{Server, types::ConnectContext},
    utils::{decode_json, encode_json},
};
use tokio_tungstenite::accept_async;

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
struct TestMessage {
    id: u32,
    content: String,
    timestamp: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
struct TestRequest {
    action: String,
    data: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
struct TestResponse {
    status: String,
    message: String,
    data: String,
}

/// Test basic client-server connection
#[tokio::test]
async fn test_basic_client_server_connection() {
    // Start server on random port
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}/connection/websocket", server_addr);

    let mut server = Server::new();

    // Simple connection handler
    server.on_connect(async move |_ctx: ConnectContext| Ok(()));

    // Start server in separate task
    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _addr)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                let ws_stream = accept_async(stream).await.unwrap();
                server
                    .serve(ws_stream, tokio_centrifuge::config::Protocol::Json.into())
                    .await;
            });
        }
    });

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create client
    let config = Config::new().use_json();
    let client = Client::new(&ws_url, config);

    // Set up callbacks
    let connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let connected_clone = connected.clone();

    client.on_connected(move || {
        connected_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    // Connect to server
    let connect_result = client.connect().await;
    assert!(connect_result.is_ok());

    // Wait for connection
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(connected.load(std::sync::atomic::Ordering::SeqCst));

    // Disconnect
    let _ = client.disconnect().await;

    // Stop server
    server_handle.abort();
}

/// Test RPC calls between client and server
#[tokio::test]
async fn test_rpc_communication() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}/connection/websocket", server_addr);

    let mut server = Server::new();

    server.on_connect(async move |_ctx: ConnectContext| Ok(()));

    // Add RPC method
    server
        .add_rpc_method("echo", async move |_ctx, req| {
            let request: TestRequest = decode_json(&req).unwrap();
            let response = TestResponse {
                status: "success".to_string(),
                message: format!("Echo: {}", request.data),
                data: request.data,
            };
            encode_json(&response)
        })
        .unwrap();

    // Add RPC method with parameters
    server
        .add_rpc_method("hello/{name}", async move |ctx, req| {
            let name = ctx
                .params
                .get("name")
                .map(|s| s.as_str())
                .unwrap_or("World");
            let request: TestRequest = decode_json(&req).unwrap();
            let response = TestResponse {
                status: "success".to_string(),
                message: format!("Hello, {}! Action: {}", name, request.action),
                data: request.data,
            };
            encode_json(&response)
        })
        .unwrap();

    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _addr)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                let ws_stream = accept_async(stream).await.unwrap();
                server
                    .serve(ws_stream, tokio_centrifuge::config::Protocol::Json.into())
                    .await;
            });
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let config = Config::new().use_json();
    let client = Client::new(&ws_url, config);

    // Connect
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Test simple RPC call
    let request = TestRequest {
        action: "test".to_string(),
        data: "Hello, RPC!".to_string(),
    };

    let rpc_result = client.rpc("echo", encode_json(&request).unwrap()).await;
    assert!(rpc_result.is_ok());

    let response_data = rpc_result.unwrap();
    let response: TestResponse = decode_json(&response_data).unwrap();

    assert_eq!(response.status, "success");
    assert_eq!(response.data, "Hello, RPC!");

    // Test RPC with parameters
    let rpc_result = client
        .rpc("hello/test_user", encode_json(&request).unwrap())
        .await;
    assert!(rpc_result.is_ok());

    let response_data = rpc_result.unwrap();
    let response: TestResponse = decode_json(&response_data).unwrap();

    assert_eq!(response.status, "success");
    assert!(response.message.contains("Hello, test_user!"));

    let _ = client.disconnect().await;
    server_handle.abort();
}

/// Test publication and subscription to channels
#[tokio::test]
async fn test_pubsub_channels() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}/connection/websocket", server_addr);

    let mut server = Server::new();

    server.on_connect(async move |_ctx: ConnectContext| Ok(()));

    // Add channel for subscription
    server
        .add_channel("test_channel", async |_ctx| {
            Ok(async_stream::stream! {
                // Send several messages
                for i in 0..3 {
                    let message = TestMessage {
                        id: i,
                        content: format!("Message {}", i),
                        timestamp: i as u64,
                    };
                    yield encode_json(&message).unwrap();
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
        })
        .unwrap();

    // Publication handler
    server
        .on_publication("test_channel", move |_ctx, data| {
            async move {
                // Just log the publication
                println!("Received publication: {:?}", data);
                Ok(())
            }
        })
        .unwrap();

    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _addr)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                let ws_stream = accept_async(stream).await.unwrap();
                server
                    .serve(ws_stream, tokio_centrifuge::config::Protocol::Json.into())
                    .await;
            });
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let config = Config::new().use_json();
    let client = Client::new(&ws_url, config);

    // Connect
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create subscription
    let subscription = client.new_subscription("test_channel");

    // Counter for received messages
    let message_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let message_count_clone = message_count.clone();

    subscription.on_publication(move |pub_data| {
        let count = message_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        println!("Received publication {}: {:?}", count + 1, pub_data);
    });

    // Subscribe to channel
    subscription.subscribe().await.unwrap();

    // Wait for messages
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Check that we received messages
    let final_count = message_count.load(std::sync::atomic::Ordering::SeqCst);
    assert!(final_count > 0);

    // Publish message
    let message = TestMessage {
        id: 999,
        content: "Test publication".to_string(),
        timestamp: 1234567890,
    };

    let publish_result = client
        .publish("test_channel", encode_json(&message).unwrap())
        .await;
    assert!(publish_result.is_ok());

    // Unsubscribe
    subscription.unsubscribe().await;

    let _ = client.disconnect().await;
    server_handle.abort();
}

/// Test authentication and tokens
#[tokio::test]
async fn test_authentication() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}/connection/websocket", server_addr);

    let mut server = Server::new();

    // Connection handler with token validation
    server.on_connect(async move |ctx: ConnectContext| {
        if ctx.token != "valid_token" {
            return Err(tokio_centrifuge::errors::DisconnectErrorCode::InvalidToken);
        }
        Ok(())
    });

    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _addr)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                let ws_stream = accept_async(stream).await.unwrap();
                server
                    .serve(ws_stream, tokio_centrifuge::config::Protocol::Json.into())
                    .await;
            });
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Test with invalid token
    let config = Config::new().use_json().with_token("invalid_token");
    let client = Client::new(&ws_url, config);

    let connect_result = client.connect().await;
    // Client should get authentication error
    assert!(connect_result.is_err());

    // Test with valid token
    let config = Config::new().use_json().with_token("valid_token");
    let client = Client::new(&ws_url, config);

    let connect_result = client.connect().await;
    assert!(connect_result.is_ok());

    let _ = client.disconnect().await;
    server_handle.abort();
}

/// Test client reconnection
#[tokio::test]
async fn test_client_reconnection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}/connection/websocket", server_addr);

    let mut server = Server::new();

    server.on_connect(async move |_ctx: ConnectContext| Ok(()));

    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _addr)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                let ws_stream = accept_async(stream).await.unwrap();
                server
                    .serve(ws_stream, tokio_centrifuge::config::Protocol::Json.into())
                    .await;
            });
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let config = Config::new()
        .use_json()
        .with_reconnect_strategy(tokio_centrifuge::config::BackoffReconnect::default());

    let client = Client::new(&ws_url, config);

    // Connect
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Disconnect
    let _ = client.disconnect().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect again
    let connect_result = client.connect().await;
    assert!(connect_result.is_ok());

    let _ = client.disconnect().await;
    server_handle.abort();
}

/// Test working with multiple clients simultaneously
#[tokio::test]
async fn test_multiple_clients() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    let mut server = Server::new();

    server.on_connect(async move |_ctx: ConnectContext| Ok(()));

    // Channel for echo functionality
    let (echo_tx, _) = tokio::sync::broadcast::channel::<Vec<u8>>(16);
    let echo_tx_clone = echo_tx.clone();

    server
        .add_channel("echo_channel", move |_ctx| {
            let mut echo_rx = echo_tx_clone.subscribe();
            async move {
                Ok(async_stream::stream! {
                    while let Ok(msg) = echo_rx.recv().await {
                        yield msg;
                    }
                })
            }
        })
        .unwrap();

    server
        .on_publication("echo_channel", move |_ctx, data| {
            let echo_tx = echo_tx.clone();
            async move {
                let _ = echo_tx.send(data);
                Ok(())
            }
        })
        .unwrap();

    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _addr)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                let ws_stream = accept_async(stream).await.unwrap();
                server
                    .serve(ws_stream, tokio_centrifuge::config::Protocol::Json.into())
                    .await;
            });
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create several clients
    let mut clients = Vec::new();
    let mut subscriptions = Vec::new();

    for i in 0..3 {
        let ws_url = format!("ws://{}/connection/websocket", server_addr);
        let config = Config::new().use_json().with_name(format!("client_{}", i));
        let client = Client::new(&ws_url, config);

        // Connect
        client.connect().await.unwrap();

        // Create subscription
        let subscription = client.new_subscription("echo_channel");
        subscription.subscribe().await.unwrap();

        clients.push(client);
        subscriptions.push(subscription);
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // First client publishes message
    let message = TestMessage {
        id: 1,
        content: "Hello from client 0".to_string(),
        timestamp: 1234567890,
    };

    let publish_result = clients[0]
        .publish("echo_channel", encode_json(&message).unwrap())
        .await;
    assert!(publish_result.is_ok());

    // Wait for message to reach all clients
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Disconnect all clients
    for client in clients {
        let _ = client.disconnect().await;
    }

    server_handle.abort();
}

/// Test error handling
#[tokio::test]
async fn test_error_handling() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}/connection/websocket", server_addr);

    let mut server = Server::new();

    server.on_connect(async move |_ctx: ConnectContext| Ok(()));

    // RPC method that returns error
    server
        .add_rpc_method("error_method", async move |_ctx, _req| {
            Err(tokio_centrifuge::errors::ClientError::bad_request(
                "Test error",
            ))
        })
        .unwrap();

    // RPC method that panics
    server
        .add_rpc_method("panic_method", async move |_ctx, _req| {
            panic!("Intentional panic for testing");
        })
        .unwrap();

    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _addr)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                let ws_stream = accept_async(stream).await.unwrap();
                server
                    .serve(ws_stream, tokio_centrifuge::config::Protocol::Json.into())
                    .await;
            });
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let config = Config::new().use_json();
    let client = Client::new(&ws_url, config);

    // Connect
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Test RPC method that returns error
    let rpc_result = client.rpc("error_method", b"test".to_vec()).await;
    assert!(rpc_result.is_err());

    // Test RPC method that panics
    let rpc_result = client.rpc("panic_method", b"test".to_vec()).await;
    // Server should properly handle panic and return error
    assert!(rpc_result.is_err());

    let _ = client.disconnect().await;
    server_handle.abort();
}

/// Test performance with high message volume
#[tokio::test]
async fn test_performance_high_message_volume() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}/connection/websocket", server_addr);

    let mut server = Server::new();

    server.on_connect(async move |_ctx: ConnectContext| Ok(()));

    // Channel for high-load transmission
    server
        .add_channel("performance_channel", async |_ctx| {
            Ok(async_stream::stream! {
                for i in 0..100 {
                    let message = TestMessage {
                        id: i,
                        content: format!("Performance message {}", i),
                        timestamp: i as u64,
                    };
                    yield encode_json(&message).unwrap();
                    tokio::time::sleep(Duration::from_millis(10)).await; // Fast sending
                }
            })
        })
        .unwrap();

    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _addr)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                let ws_stream = accept_async(stream).await.unwrap();
                server
                    .serve(ws_stream, tokio_centrifuge::config::Protocol::Json.into())
                    .await;
            });
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let config = Config::new().use_json();
    let client = Client::new(&ws_url, config);

    // Connect
    client.connect().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create subscription
    let subscription = client.new_subscription("performance_channel");

    let message_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let message_count_clone = message_count.clone();

    subscription.on_publication(move |_pub_data| {
        message_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });

    // Subscribe to channel
    subscription.subscribe().await.unwrap();

    // Wait for all messages
    tokio::time::sleep(Duration::from_millis(2000)).await;

    // Check that we received all messages
    let final_count = message_count.load(std::sync::atomic::Ordering::SeqCst);
    assert!(final_count >= 90); // Should receive most messages

    // Unsubscribe
    subscription.unsubscribe().await;

    let _ = client.disconnect().await;
    server_handle.abort();
}
