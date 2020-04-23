use crate::errors::Error;
use bytes::{Buf, Bytes, BytesMut};
use serde::Deserialize;
use subslice::SubsliceExt;
use tokio_util::codec::{Decoder, Encoder};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ServerOp {
    Info(ServerInfo),
    Msg(usize, String, Bytes),
    Ok,
    Err(String),
    Ping,
    Pong,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
pub(crate) struct ServerInfo {
    server_id: String,
    server_name: String,
    version: String,
    go: String,
    host: String,
    port: u32,
    max_payload: usize,
    client_id: u32,
    client_ip: String,
    connect_urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ClientOp {
    Connect(ClientInfo),
    Pub(String, Bytes),
    Sub(String, usize),
    Unsub(usize),
    Ping,
    Pong,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub(crate) struct ClientInfo {
    pub(crate) verbose: bool,
    pub(crate) pedantic: bool,
    pub(crate) name: Option<String>,
    pub(crate) lang: String,
    pub(crate) version: String,
}

pub(crate) struct NatsCodec {}

impl NatsCodec {
    pub(crate) fn new() -> Self {
        NatsCodec {}
    }
}

impl Decoder for NatsCodec {
    type Item = ServerOp;
    type Error = Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.starts_with(b"MSG ") {
            let line_end = if let Some(end) = src.find(b"\r\n") {
                end
            } else {
                return Ok(None);
            };
            let mut parts = src[4..line_end].split(|c| c == &b' ');
            let subject =
                std::str::from_utf8(parts.next().ok_or_else(|| Error::ProtocolError)?)?.to_string();
            let sid = std::str::from_utf8(parts.next().ok_or_else(|| Error::ProtocolError)?)?
                .parse::<usize>()?;
            let len = std::str::from_utf8(parts.next().ok_or_else(|| Error::ProtocolError)?)?
                .parse::<usize>()?;
            if line_end + len + 4 <= src.len() {
                src.advance(line_end + 2);
                let message = src.split_to(len);
                src.advance(2);
                Ok(Some(ServerOp::Msg(sid, subject, message.freeze())))
            } else {
                Ok(None)
            }
        } else if src.starts_with(b"+OK\r\n") {
            src.advance(5);
            Ok(Some(ServerOp::Ok))
        } else if src.starts_with(b"PING\r\n") {
            src.advance(6);
            Ok(Some(ServerOp::Ping))
        } else if src.starts_with(b"PONG\r\n") {
            src.advance(6);
            Ok(Some(ServerOp::Pong))
        } else if src.starts_with(b"INFO") {
            if let Some(n) = src.find(b"\r\n") {
                let info_string = std::str::from_utf8(&src[5..n])?.trim();
                let server_op = ServerOp::Info(serde_json::from_str(info_string)?);
                src.advance(n + 2);
                Ok(Some(server_op))
            } else {
                Ok(None)
            }
        } else if src.starts_with(b"-ERR '") {
            if let Some(n) = src.find(b"'\r\n") {
                let err_string = std::str::from_utf8(&src[6..n])?;
                Ok(Some(ServerOp::Err(err_string.to_string())))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }
}

impl Encoder for NatsCodec {
    type Item = ClientOp;
    type Error = Error;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            ClientOp::Connect(client_info) => {
                dst.extend_from_slice(b"CONNECT ");
                dst.extend_from_slice(&serde_json::to_vec(&client_info)?);
                dst.extend_from_slice(b"\r\n");
            }
            ClientOp::Pub(subject, data) => {
                dst.extend_from_slice(b"PUB ");
                dst.extend_from_slice(subject.as_bytes());
                dst.extend_from_slice(format!(" {}\r\n", data.len()).as_bytes());
                dst.extend_from_slice(&data);
                dst.extend_from_slice(b"\r\n");
            }
            ClientOp::Sub(subject, sid) => {
                dst.extend_from_slice(b"SUB ");
                dst.extend_from_slice(subject.as_bytes());
                dst.extend_from_slice(format!(" {}\r\n", sid).as_bytes());
            }
            ClientOp::Unsub(sid) => {
                dst.extend_from_slice(b"UNSUB ");
                dst.extend_from_slice(format!("{}\r\n", sid).as_bytes());
            }
            ClientOp::Ping => dst.extend_from_slice(b"PING\r\n"),
            ClientOp::Pong => dst.extend_from_slice(b"PONG\r\n"),
        }
        Ok(())
    }
}
