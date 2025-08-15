//! Utility functions for encoding, decoding, and protocol handling.
//!
//! This module provides helper functions for working with different
//! protocol formats (JSON and Protobuf) and handling WebSocket frames.
//!
//! # Features
//!
//! - **JSON Encoding/Decoding**: Handle JSON serialization with empty data support
//! - **Protobuf Encoding/Decoding**: Binary protocol support for efficiency
//! - **Protocol-Agnostic Functions**: Functions that work with both protocols
//! - **Frame Processing**: Utilities for processing WebSocket message frames

use std::io::BufRead;

use anyhow::anyhow;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_tungstenite::tungstenite::Message;

use crate::config::Protocol;
use crate::errors::ClientError;

/// Decodes JSON data with support for empty input.
///
/// This function handles empty data by treating it as `null`, ensuring
/// that `client.rpc("method")` and `client.rpc("method", null)` are equivalent.
///
/// # Arguments
///
/// * `data` - The JSON data to decode
///
/// # Returns
///
/// The decoded value of type `T`, or a `ClientError` on failure.
///
/// # Example
///
/// ```rust
/// use tokio_centrifuge::utils::decode_json;
/// use tokio_centrifuge::errors::ClientError;
///
/// fn main() -> Result<(), ClientError> {
///     let data = b"{\"key\": \"value\"}";
///     let result: serde_json::Value = decode_json(data)?;
///
///     let empty_data = b"";
///     let null_result: serde_json::Value = decode_json(empty_data)?; // Decodes as null
///     Ok(())
/// }
/// ```
pub fn decode_json<T: serde::de::DeserializeOwned>(mut data: &[u8]) -> Result<T, ClientError> {
    if data.is_empty() {
        // make sure `client.rpc("method")` and `client.rpc("method", null)` are equivalent,
        // otherwise former will be [] and fail json deserialization
        data = b"null";
    }

    serde_json::from_slice(data).map_err(|err| ClientError::bad_request(err.to_string()))
}

/// Encodes data to JSON format.
///
/// # Arguments
///
/// * `data` - The data to encode
///
/// # Returns
///
/// The JSON-encoded data as bytes, or a `ClientError` on failure.
///
/// # Example
///
/// ```rust
/// use tokio_centrifuge::utils::encode_json;
/// use tokio_centrifuge::errors::ClientError;
///
/// fn main() -> Result<(), ClientError> {
///     let data = serde_json::json!({"key": "value"});
///     let encoded = encode_json(&data)?;
///     Ok(())
/// }
/// ```
pub fn encode_json<T: serde::Serialize>(data: &T) -> Result<Vec<u8>, ClientError> {
    serde_json::to_vec(data).map_err(|err| ClientError::internal(err.to_string()))
}

/// Decodes Protobuf data.
///
/// # Arguments
///
/// * `data` - The Protobuf data to decode
///
/// # Returns
///
/// The decoded value of type `T`, or a `ClientError` on failure.
///
/// # Example
///
/// ```rust
/// use tokio_centrifuge::utils::decode_proto;
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let data = b"protobuf_data";
///     // Note: MyMessage would need to be a real protobuf message type
///     // let result: MyMessage = decode_proto(data)?;
///     Ok(())
/// }
/// ```
pub fn decode_proto<T: prost::Message + Default>(data: &[u8]) -> Result<T, ClientError> {
    T::decode(data).map_err(|_| ClientError::bad_request("failed to decode protobuf"))
}

/// Encodes data to Protobuf format.
///
/// # Arguments
///
/// * `data` - The data to encode
///
/// # Returns
///
/// The Protobuf-encoded data as bytes, or a `ClientError` on failure.
///
/// # Example
///
/// ```rust
/// use tokio_centrifuge::utils::encode_proto;
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // Note: MyMessage would need to be a real protobuf message type
///     // let message = MyMessage::default();
///     // let encoded = encode_proto(&message)?;
///     Ok(())
/// }
/// ```
pub fn encode_proto<T: prost::Message>(data: &T) -> Result<Vec<u8>, ClientError> {
    let mut buf = Vec::new();
    data.encode(&mut buf)
        .map_err(|_| ClientError::internal("failed to encode protobuf"))?;
    Ok(buf)
}

/// Decodes data using the specified protocol.
///
/// This function automatically chooses the appropriate decoding method
/// based on the protocol configuration.
///
/// # Arguments
///
/// * `data` - The data to decode
/// * `protocol` - The protocol to use for decoding
///
/// # Returns
///
/// The decoded value of type `T`, or a `ClientError` on failure.
///
/// # Example
///
/// ```rust
/// use tokio_centrifuge::config::Protocol;
/// use tokio_centrifuge::utils::decode_with;
/// use tokio_centrifuge::errors::ClientError;
/// use serde_json::Value;
///
/// fn main() -> Result<(), ClientError> {
///     let json_data = b"{\"key\": \"value\"}";
///     // Note: decode_with requires types that implement both DeserializeOwned and prost::Message
///     // For JSON data, you would typically use a custom struct or a compatible type
///     // let result: MyStruct = decode_with(json_data, Protocol::Json)?;
///
///     let proto_data = b"protobuf_data";
///     // Note: MyMessage would need to be a real protobuf message type
///     // let result: MyMessage = decode_with(proto_data, Protocol::Protobuf)?;
///     Ok(())
/// }
/// ```
pub fn decode_with<T: DeserializeOwned + prost::Message + Default>(
    data: &[u8],
    protocol: Protocol,
) -> Result<T, ClientError> {
    match protocol {
        Protocol::Json => decode_json(data),
        Protocol::Protobuf => decode_proto(data),
    }
}

/// Encodes data using the specified protocol.
///
/// This function automatically chooses the appropriate encoding method
/// based on the protocol configuration.
///
/// # Arguments
///
/// * `data` - The data to encode
/// * `protocol` - The protocol to use for encoding
///
/// # Returns
///
/// The encoded data as bytes, or a `ClientError` on failure.
///
/// # Example
///
/// ```rust
/// use tokio_centrifuge::config::Protocol;
/// use tokio_centrifuge::utils::encode_with;
/// use tokio_centrifuge::errors::ClientError;
///
/// fn main() -> Result<(), ClientError> {
///     // Note: encode_with requires types that implement both Serialize and prost::Message
///     // For JSON data, you would typically use a custom struct or a compatible type
///     // let data = MyStruct { key: "value".to_string() };
///     // let json_encoded = encode_with(&data, Protocol::Json)?;
///     
///     // Note: Protobuf encoding requires a real protobuf message type
///     // let proto_encoded = encode_with(&data, Protocol::Protobuf)?;
///     Ok(())
/// }
/// ```
pub fn encode_with<T: Serialize + prost::Message>(
    data: &T,
    protocol: Protocol,
) -> Result<Vec<u8>, ClientError> {
    match protocol {
        Protocol::Json => encode_json(data),
        Protocol::Protobuf => encode_proto(data),
    }
}

/// Decodes frames from data using the specified protocol.
///
/// This function processes multiple frames from the input data and
/// calls the provided handler function for each frame.
///
/// # Arguments
///
/// * `data` - The data containing multiple frames
/// * `protocol` - The protocol to use for decoding
/// * `handle_frame` - Function to handle each decoded frame
///
/// # Returns
///
/// `Ok(())` on success, or an error if frame processing fails.
pub(crate) fn decode_frames<T: DeserializeOwned + prost::Message + Default>(
    data: &[u8],
    protocol: Protocol,
    handle_frame: impl FnMut(anyhow::Result<T>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    match protocol {
        Protocol::Json => decode_frames_json(data, handle_frame),
        Protocol::Protobuf => decode_frames_protobuf(data, handle_frame),
    }
}

/// Decodes JSON frames from data.
///
/// Processes the data line by line, treating each line as a separate JSON frame.
///
/// # Arguments
///
/// * `data` - The data containing JSON frames
/// * `handle_frame` - Function to handle each decoded frame
///
/// # Returns
///
/// `Ok(())` on success, or an error if frame processing fails.
fn decode_frames_json<T: DeserializeOwned>(
    data: &[u8],
    mut handle_frame: impl FnMut(anyhow::Result<T>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    for line in data.lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                log::debug!("failed to read line: {}", err);
                handle_frame(Err(anyhow!(err)))?;
                continue;
            }
        };

        log::trace!("<-- {}", line);

        handle_frame(match serde_json::from_str(&line) {
            Ok(frame) => Ok(frame),
            Err(err) => {
                log::debug!("failed to parse frame: {}", err);
                Err(anyhow!(err))
            }
        })?;
    }

    Ok(())
}

/// Decodes Protobuf frames from data.
///
/// Processes the data as length-delimited Protobuf messages.
///
/// # Arguments
///
/// * `data` - The data containing Protobuf frames
/// * `handle_frame` - Function to handle each decoded frame
///
/// # Returns
///
/// `Ok(())` on success, or an error if frame processing fails.
fn decode_frames_protobuf<T: prost::Message + Default>(
    mut data: &[u8],
    mut handle_frame: impl FnMut(anyhow::Result<T>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    while !data.is_empty() {
        let Ok(len) = prost::decode_length_delimiter(data) else {
            break;
        };
        let len_delimiter_len = prost::length_delimiter_len(len);
        if len_delimiter_len + len > data.len() {
            // need bounds check because len_delimiter is user controlled
            log::trace!("<-- {} (??)", format_protobuf(data));
            break;
        }

        log::trace!("<-- {}", format_protobuf(&data[..len_delimiter_len + len]));
        data = &data[len_delimiter_len..];

        let result = (|| -> Result<T, anyhow::Error> {
            let frame = T::decode(&data[..len])?;
            Ok(frame)
        })();

        data = &data[len..];
        handle_frame(result)?;
    }

    Ok(())
}

pub(crate) fn encode_frames<T: Serialize + prost::Message>(
    commands: &[T],
    protocol: Protocol,
    mut on_encode_error: impl FnMut(usize),
) -> Option<Message> {
    match protocol {
        Protocol::Json => {
            let mut lines = Vec::with_capacity(commands.len());
            for (idx, command) in commands.iter().enumerate() {
                match serde_json::to_string(command) {
                    Ok(line) => {
                        log::trace!("--> {}", &line);
                        lines.push(line);
                    }
                    Err(err) => {
                        on_encode_error(idx);
                        log::debug!("failed to encode command: {:?}", err);
                    }
                }
            }

            if lines.is_empty() {
                None
            } else {
                Some(Message::Text(lines.join("\n").into()))
            }
        }
        Protocol::Protobuf => {
            let mut buf = Vec::new();
            for command in commands.iter() {
                let buf_len = buf.len();
                command.encode_length_delimited(&mut buf).unwrap();
                log::trace!("--> {}", format_protobuf(&buf[buf_len..]));
            }
            Some(Message::Binary(buf.into()))
        }
    }
}

fn format_protobuf(buf: &[u8]) -> String {
    fn buf_to_hex(buf: &[u8]) -> String {
        buf.iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join("")
    }

    let Ok(len) = prost::decode_length_delimiter(buf) else {
        return buf_to_hex(buf);
    };
    let len_delimiter_len = prost::length_delimiter_len(len);

    let (len, body) = buf
        .split_at_checked(len_delimiter_len)
        .unwrap_or((buf, &[]));
    format!("{} {}", buf_to_hex(len), buf_to_hex(body))
}
