//! Native <-> Manager named-pipe protocol, version 1.
//!
//! Wire format is a little-endian u32 length prefix followed by a JSON envelope:
//!
//! ```text
//! [u32 le length][{ "protocolVersion":1, "requestId":"..", "type":"..", "payload":{...} }]
//! ```
//!
//! The contract mirrors `NativePipeFrameCodec.cs` and `native-manager-protocol.ts`
//! so all three sides agree byte for byte.

pub mod contract;
pub mod host;
pub mod hotkey;
pub mod pipe;
pub mod prewarm;
pub mod projection;
pub mod projection_cache;
pub mod quick_result;
pub mod server;
pub mod session;
pub mod settings;
pub mod state;
pub mod super_panel;
pub use host::{isolated_options, HostOptions, HostProcess};
pub use pipe::Connection;
pub use prewarm::{Decision, PrewarmPolicy, Trigger};
pub use server::PipeServer;
pub use session::{HelloInfo, HostOwnership, HostSession, SUPPORTED_FEATURES};

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: i32 = 1;
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_REQUEST_ID: usize = 128;
pub const MAX_TYPE_LEN: usize = 48;

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    FrameTooLarge(usize),
    EmptyFrame,
    Malformed(&'static str),
    Json(serde_json::Error),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "pipe io: {e}"),
            Self::FrameTooLarge(n) => write!(f, "frame of {n} bytes exceeds {}", MAX_MESSAGE_BYTES),
            Self::EmptyFrame => write!(f, "empty frame"),
            Self::Malformed(what) => write!(f, "malformed envelope: {what}"),
            Self::Json(e) => write!(f, "json: {e}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ProtocolError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub type Result<T> = std::result::Result<T, ProtocolError>;

/// One protocol message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: i32,
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub payload: serde_json::Value,
}

impl Envelope {
    pub fn new(kind: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: new_request_id(),
            kind: kind.into(),
            payload,
        }
    }

    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = id.into();
        self
    }

    fn validate(&self) -> Result<()> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolError::Malformed("protocolVersion"));
        }
        if self.request_id.is_empty() || self.request_id.len() > MAX_REQUEST_ID {
            return Err(ProtocolError::Malformed("requestId"));
        }
        if self.kind.is_empty() || self.kind.len() > MAX_TYPE_LEN {
            return Err(ProtocolError::Malformed("type"));
        }
        if self.payload.is_null() {
            return Err(ProtocolError::Malformed("payload"));
        }
        Ok(())
    }
}

/// One reply to a request, as produced by the manager side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ack {
    #[serde(rename = "replyToId")]
    pub reply_to_id: String,
    #[serde(rename = "replyToType")]
    pub reply_to_type: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AckError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckError {
    pub code: String,
    pub message: String,
}

impl Ack {
    pub fn into_result(self) -> std::result::Result<serde_json::Value, AckError> {
        if self.ok {
            Ok(self.result.unwrap_or(serde_json::Value::Null))
        } else {
            Err(self.error.unwrap_or(AckError {
                code: "unknown".into(),
                message: "ack reported failure without detail".into(),
            }))
        }
    }
}

/// Handshake payload sent by the manager host once its renderer is usable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostReady {
    #[serde(rename = "processId")]
    pub process_id: u32,
    #[serde(rename = "managerVersion", default)]
    pub manager_version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(
        rename = "developmentAuthenticationToken",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub development_authentication_token: Option<String>,
}

pub fn encode(envelope: &Envelope) -> Result<Vec<u8>> {
    envelope.validate()?;
    let body = serde_json::to_vec(envelope)?;
    if body.is_empty() {
        return Err(ProtocolError::EmptyFrame);
    }
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::FrameTooLarge(body.len()));
    }
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn write_envelope<W: Write>(writer: &mut W, envelope: &Envelope) -> Result<()> {
    let frame = encode(envelope)?;
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

/// Blocking read of exactly one frame. Returns `Ok(None)` on a clean EOF
/// between frames (the manager closed the pipe).
pub fn read_envelope<R: Read>(reader: &mut R) -> Result<Option<Envelope>> {
    let mut header = [0u8; 4];
    match reader.read(&mut header[..1]) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(ProtocolError::Io(e)),
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 {
        return Err(ProtocolError::EmptyFrame);
    }
    if length > MAX_MESSAGE_BYTES {
        return Err(ProtocolError::FrameTooLarge(length));
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let envelope: Envelope = serde_json::from_slice(&body)?;
    envelope.validate()?;
    Ok(Some(envelope))
}

/// Incremental decoder for transports that deliver arbitrary chunks.
#[derive(Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Envelope>> {
        if chunk.is_empty() {
            return Ok(Vec::new());
        }
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > MAX_MESSAGE_BYTES + 4 {
            return Err(ProtocolError::FrameTooLarge(self.buffer.len()));
        }
        let mut out = Vec::new();
        let mut offset = 0usize;
        while self.buffer.len() - offset >= 4 {
            let length =
                u32::from_le_bytes(self.buffer[offset..offset + 4].try_into().unwrap()) as usize;
            if length == 0 {
                return Err(ProtocolError::EmptyFrame);
            }
            if length > MAX_MESSAGE_BYTES {
                return Err(ProtocolError::FrameTooLarge(length));
            }
            if self.buffer.len() - offset - 4 < length {
                break;
            }
            let envelope: Envelope =
                serde_json::from_slice(&self.buffer[offset + 4..offset + 4 + length])?;
            envelope.validate()?;
            out.push(envelope);
            offset += 4 + length;
        }
        if offset > 0 {
            self.buffer.drain(..offset);
        }
        Ok(out)
    }
}

/// RFC-4122 UUID v4 request id. The manager validates request ids against a
/// strict v4 pattern and rejects anything else as an invalid invocation.
pub fn new_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let raw = format!("{:032x}", nanos ^ ((n as u128) << 96));
    format!(
        "{}-{}-4{}-8{}-{}",
        &raw[0..8],
        &raw[8..12],
        &raw[13..16],
        &raw[17..20],
        &raw[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let envelope = Envelope::new("electron-host-ready", serde_json::json!({"processId": 42}));
        let frame = encode(&envelope).unwrap();
        assert_eq!(&frame[..4], &(frame.len() as u32 - 4).to_le_bytes());
        let decoded = read_envelope(&mut &frame[..]).unwrap().unwrap();
        assert_eq!(decoded.kind, "electron-host-ready");
        assert_eq!(decoded.payload["processId"], 42);
    }

    #[test]
    fn rejects_oversized_type() {
        let mut envelope = Envelope::new("x".repeat(MAX_TYPE_LEN + 1), serde_json::json!({}));
        envelope.request_id = "a".into();
        assert!(matches!(
            encode(&envelope),
            Err(ProtocolError::Malformed("type"))
        ));
    }

    #[test]
    fn decoder_handles_split_chunks() {
        let mut decoder = FrameDecoder::default();
        let a = encode(&Envelope::new(
            "manager-renderer-ready",
            serde_json::json!({}),
        ))
        .unwrap();
        let b = encode(&Envelope::new(
            "electron-host-closing",
            serde_json::json!({}),
        ))
        .unwrap();
        let mut all = a.clone();
        all.extend_from_slice(&b);
        let first = decoder.push(&all[..3]).unwrap();
        assert!(first.is_empty());
        let rest = decoder.push(&all[3..]).unwrap();
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[1].kind, "electron-host-closing");
    }

    #[test]
    fn ack_result_conversion() {
        let ack: Ack = serde_json::from_value(serde_json::json!({
            "replyToId": "1", "replyToType": "app-memory-get", "ok": true, "result": {"items": []}
        }))
        .unwrap();
        assert!(ack.into_result().is_ok());
        let bad: Ack = serde_json::from_value(serde_json::json!({
            "replyToId": "1", "replyToType": "app-memory-get", "ok": false,
            "error": {"code": "denied", "message": "nope"}
        }))
        .unwrap();
        assert!(bad.into_result().is_err());
    }
}
