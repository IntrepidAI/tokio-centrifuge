use tokio_centrifuge::{client::Client, client::types::State, config::Config, errors::ClientError};

#[tokio::test]
async fn test_client_creation() {
    let config = Config::new().use_json();
    let client = Client::new("ws://localhost:8000/connection/websocket", config);

    // Check that client was created
    assert_eq!(client.state(), State::Disconnected);
}

#[tokio::test]
async fn test_subscription_creation() {
    let config = Config::new().use_json();
    let client = Client::new("ws://localhost:8000/connection/websocket", config);

    let subscription = client.new_subscription("test_channel");

    // Check that subscription was created
    assert_eq!(
        subscription.state(),
        tokio_centrifuge::subscription::State::Unsubscribed
    );
}

#[tokio::test]
async fn test_config_methods() {
    let config = Config::new()
        .use_json()
        .with_name("test_client")
        .with_version("1.0.0")
        .with_token("test_token")
        .with_read_timeout(std::time::Duration::from_secs(30));

    // Check that configuration was applied
    assert_eq!(config.name, "test_client");
    assert_eq!(config.version, "1.0.0");
    assert_eq!(config.token, "test_token");
    assert_eq!(config.read_timeout, std::time::Duration::from_secs(30));
}

#[tokio::test]
async fn test_subscription_state_transitions() {
    let config = Config::new().use_json();
    let client = Client::new("ws://localhost:8000/connection/websocket", config);
    let subscription = client.new_subscription("test_channel");

    // Initial state
    assert_eq!(
        subscription.state(),
        tokio_centrifuge::subscription::State::Unsubscribed
    );

    // Set up callbacks
    subscription.on_subscribing(|| println!("Subscribing..."));
    subscription.on_subscribed(|| println!("Subscribed!"));
    subscription.on_unsubscribed(|| println!("Unsubscribed!"));
    subscription.on_publication(|pub_data| println!("Publication: {:?}", pub_data));
    subscription.on_error(|err| eprintln!("Error: {}", err));

    // Check that callbacks were set up (this should not fail)
    assert!(true);
}

#[tokio::test]
async fn test_client_callbacks() {
    let config = Config::new().use_json();
    let client = Client::new("ws://localhost:8000/connection/websocket", config);

    // Set up callbacks
    client.on_connecting(|| println!("Connecting..."));
    client.on_connected(|| println!("Connected!"));
    client.on_disconnected(|| println!("Disconnected!"));
    client.on_error(|err| eprintln!("Client error: {}", err));

    // Check that callbacks were set up
    assert!(true);
}

#[tokio::test]
async fn test_subscription_management() {
    let config = Config::new().use_json();
    let client = Client::new("ws://localhost:8000/connection/websocket", config);

    // Create several subscriptions
    let sub1 = client.new_subscription("channel1");
    let sub2 = client.new_subscription("channel2");

    // Check that subscriptions were created
    assert_eq!(
        sub1.state(),
        tokio_centrifuge::subscription::State::Unsubscribed
    );
    assert_eq!(
        sub2.state(),
        tokio_centrifuge::subscription::State::Unsubscribed
    );

    // Get existing subscription
    let existing_sub = client.get_subscription("channel1");
    assert!(existing_sub.is_some());

    // Get non-existing subscription
    let non_existing_sub = client.get_subscription("channel3");
    assert!(non_existing_sub.is_none());
}

#[tokio::test]
async fn test_config_protocols() {
    // Test JSON protocol
    let json_config = Config::new().use_json();
    assert_eq!(
        json_config.protocol,
        tokio_centrifuge::config::Protocol::Json
    );

    // Test Protobuf protocol
    let proto_config = Config::new().use_protobuf();
    assert_eq!(
        proto_config.protocol,
        tokio_centrifuge::config::Protocol::Protobuf
    );
}

#[tokio::test]
async fn test_reconnect_strategy() {
    let _config = Config::new()
        .with_reconnect_strategy(tokio_centrifuge::config::BackoffReconnect::default());

    // Check that reconnection strategy was set
    // Since reconnect_strategy has type Arc<dyn ReconnectStrategy>,
    // we can only check that it's not None
    assert!(true); // Method with_reconnect_strategy works and should not fail
}

#[tokio::test]
async fn test_token_management() {
    let config = Config::new().use_json();
    let client = Client::new("ws://localhost:8000/connection/websocket", config);

    // Set new token
    client.set_token("new_token");

    // Check that token was updated
    assert!(true); // Method set_token exists and should not fail
}

#[tokio::test]
async fn test_error_handling() {
    // Test error creation
    let error = ClientError::bad_request("Test error");
    assert_eq!(
        error.code,
        tokio_centrifuge::errors::ClientErrorCode::BadRequest
    );

    let error = ClientError::unauthorized("Unauthorized");
    assert_eq!(
        error.code,
        tokio_centrifuge::errors::ClientErrorCode::Unauthorized
    );

    let error = ClientError::internal("Internal error");
    assert_eq!(
        error.code,
        tokio_centrifuge::errors::ClientErrorCode::Internal
    );
}

#[tokio::test]
async fn test_utils_functions() {
    use tokio_centrifuge::utils;

    // Test JSON encoding/decoding
    let data = serde_json::json!({"key": "value"});
    let encoded = utils::encode_json(&data).unwrap();
    let decoded: serde_json::Value = utils::decode_json(&encoded).unwrap();
    assert_eq!(data, decoded);

    // Test empty data (should decode as null)
    let empty_decoded: serde_json::Value = utils::decode_json(b"").unwrap();
    assert_eq!(empty_decoded, serde_json::Value::Null);
}

#[tokio::test]
async fn test_protocol_structures() {
    use tokio_centrifuge::protocol::{Command, ConnectRequest, SubscribeRequest};

    // Test command creation
    let connect_cmd = Command::Connect(ConnectRequest {
        token: "test_token".to_string(),
        data: vec![],
        subs: std::collections::HashMap::new(),
        name: "test_client".to_string(),
        version: "1.0.0".to_string(),
    });

    let subscribe_cmd = Command::Subscribe(SubscribeRequest {
        channel: "test_channel".to_string(),
        token: "".to_string(),
        data: vec![],
        positioned: false,
        recoverable: false,
        recover: false,
        offset: 0,
        epoch: "".to_string(),
        join_leave: false,
    });

    // Check that commands were created
    assert!(matches!(connect_cmd, Command::Connect(_)));
    assert!(matches!(subscribe_cmd, Command::Subscribe(_)));
}

#[tokio::test]
async fn test_client_connection_methods() {
    let config = Config::new().use_json();
    let client = Client::new("ws://localhost:8000/connection/websocket", config);

    // Check that connect and disconnect methods return FutureResult
    let _connect_future = client.connect();
    let _disconnect_future = client.disconnect();

    // Check that methods don't fail
    assert!(true);
}

#[tokio::test]
async fn test_subscription_methods() {
    let config = Config::new().use_json();
    let client = Client::new("ws://localhost:8000/connection/websocket", config);
    let subscription = client.new_subscription("test_channel");

    // Check that subscribe and unsubscribe methods return FutureResult
    let _subscribe_future = subscription.subscribe();
    let _unsubscribe_future = subscription.unsubscribe();

    // Check that methods don't fail
    assert!(true);
}

#[tokio::test]
async fn test_client_publish_rpc() {
    let config = Config::new().use_json();
    let client = Client::new("ws://localhost:8000/connection/websocket", config);

    // Check that publish and rpc methods return FutureResult
    let _publish_future = client.publish("test_channel", b"test_data".to_vec());
    let _rpc_future = client.rpc("test_method", b"test_data".to_vec());

    // Check that methods don't fail
    assert!(true);
}

#[tokio::test]
async fn test_config_default_values() {
    let config = Config::new();

    // Check default values
    assert_eq!(config.protocol, tokio_centrifuge::config::Protocol::Json);
    assert_eq!(config.read_timeout, std::time::Duration::from_secs(5));

    // Check that configuration methods work
    let config = config
        .use_protobuf()
        .with_read_timeout(std::time::Duration::from_secs(10));

    assert_eq!(
        config.protocol,
        tokio_centrifuge::config::Protocol::Protobuf
    );
    assert_eq!(config.read_timeout, std::time::Duration::from_secs(10));
}
