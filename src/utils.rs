use std::io::BufRead;

use anyhow::anyhow;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_tungstenite::tungstenite::Message;

use crate::config::Protocol;
use crate::errors::ClientError;

// same as serde_json::from_slice, but handles empty data correctly
pub fn decode_json<T: serde::de::DeserializeOwned>(mut data: &[u8]) -> Result<T, ClientError> {
    if data.is_empty() {
        // make sure `client.rpc("method")` and `client.rpc("method", null)` are equivalent,
        // otherwise former will be [] and fail json deserialization
        data = b"null";
    }

    serde_json::from_slice(data).map_err(|err| {
        ClientError::bad_request(err.to_string())
    })
}

pub fn encode_json<T: serde::Serialize>(data: &T) -> Result<Vec<u8>, ClientError> {
    serde_json::to_vec(data).map_err(|err| ClientError::internal(err.to_string()))
}

pub fn decode_proto<T: prost::Message + Default>(data: &[u8]) -> Result<T, ClientError> {
    T::decode(data).map_err(|_| ClientError::bad_request("failed to decode protobuf"))
}

pub fn encode_proto<T: prost::Message>(data: &T) -> Result<Vec<u8>, ClientError> {
    let mut buf = Vec::new();
    data.encode(&mut buf).map_err(|_| ClientError::internal("failed to encode protobuf"))?;
    Ok(buf)
}

pub fn decode_with<T: DeserializeOwned + prost::Message + Default>(data: &[u8], protocol: Protocol) -> Result<T, ClientError> {
    match protocol {
        Protocol::Json => decode_json(data),
        Protocol::Protobuf => decode_proto(data),
    }
}

pub fn encode_with<T: Serialize + prost::Message>(data: &T, protocol: Protocol) -> Result<Vec<u8>, ClientError> {
    match protocol {
        Protocol::Json => encode_json(data),
        Protocol::Protobuf => encode_proto(data),
    }
}

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
        buf.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join("")
    }

    let Ok(len) = prost::decode_length_delimiter(buf) else {
        return buf_to_hex(buf);
    };
    let len_delimiter_len = prost::length_delimiter_len(len);

    let (len, body) = buf.split_at_checked(len_delimiter_len).unwrap_or((buf, &[]));
    format!("{} {}", buf_to_hex(len), buf_to_hex(body))
}
