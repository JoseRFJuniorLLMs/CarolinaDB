//! Binary wire envelope, limits and framing (SPEC-012 §9).
//!
//! 32-byte header, all integers unsigned little-endian:
//!
//! | offset | bytes | field |
//! |---:|---:|---|
//! | 0 | 4 | magic `ASTR` |
//! | 4 | 2 | wire major = 1 |
//! | 6 | 2 | wire minor = 0 |
//! | 8 | 2 | message kind |
//! | 10 | 2 | flags = 0 |
//! | 12 | 4 | payload length |
//! | 16 | 8 | stream id (0 = negotiation) |
//! | 24 | 8 | per-direction frame sequence, from 0, +1, never wraps |
//!
//! Exactly `payload_length` canonical payload bytes follow. Unknown magic/version/kind/flags,
//! sequence gaps or reuse, truncated or oversized frames fail the connection before dispatch.

use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::limits::Limits;

pub const MAGIC: [u8; 4] = *b"ASTR";
pub const WIRE_MAJOR: u16 = 1;
pub const WIRE_MINOR: u16 = 0;
pub const HEADER_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum MessageKind {
    Hello = 1,
    HelloAck = 2,
    Invoke = 3,
    Reply = 4,
    ResolveRequest = 5,
    ResolveReply = 6,
    Read = 7,
    ReadReply = 8,
    ProtocolRecord = 9,
    SnapshotManifest = 10,
    SnapshotChunk = 11,
    /// Node-to-node consensus traffic (`carolina-consensus` envelopes); never accepted on a
    /// client-role connection.
    Consensus = 12,
    /// Administrative catalog commands and their replies (SPEC-011 §4).
    Admin = 13,
    AdminReply = 14,
}

impl MessageKind {
    pub fn from_u16(v: u16) -> CoreResult<MessageKind> {
        Ok(match v {
            1 => MessageKind::Hello,
            2 => MessageKind::HelloAck,
            3 => MessageKind::Invoke,
            4 => MessageKind::Reply,
            5 => MessageKind::ResolveRequest,
            6 => MessageKind::ResolveReply,
            7 => MessageKind::Read,
            8 => MessageKind::ReadReply,
            9 => MessageKind::ProtocolRecord,
            10 => MessageKind::SnapshotManifest,
            11 => MessageKind::SnapshotChunk,
            12 => MessageKind::Consensus,
            13 => MessageKind::Admin,
            14 => MessageKind::AdminReply,
            _ => {
                return Err(CoreError::new(
                    ErrorCode::ProtocolError,
                    format!("unknown message kind {v}"),
                ))
            }
        })
    }
    /// Record kind expected in the payload of this frame kind (frame/body mismatch is rejected).
    pub fn expected_record_kind(&self) -> Option<&'static str> {
        Some(match self {
            MessageKind::Hello => "hello",
            MessageKind::HelloAck => "hello_ack",
            MessageKind::Invoke => "invoke",
            MessageKind::Reply => "client_reply",
            MessageKind::ResolveRequest => "resolve_request",
            MessageKind::ResolveReply => "resolve_reply",
            MessageKind::SnapshotManifest => "snapshot_manifest",
            MessageKind::SnapshotChunk => "snapshot_chunk",
            MessageKind::Read
            | MessageKind::ReadReply
            | MessageKind::ProtocolRecord
            | MessageKind::Consensus
            | MessageKind::Admin
            | MessageKind::AdminReply => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeader {
    pub kind: MessageKind,
    pub payload_length: u32,
    pub stream_id: u64,
    pub sequence: u64,
}

impl FrameHeader {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0..4].copy_from_slice(&MAGIC);
        h[4..6].copy_from_slice(&WIRE_MAJOR.to_le_bytes());
        h[6..8].copy_from_slice(&WIRE_MINOR.to_le_bytes());
        h[8..10].copy_from_slice(&(self.kind as u16).to_le_bytes());
        h[10..12].copy_from_slice(&0u16.to_le_bytes());
        h[12..16].copy_from_slice(&self.payload_length.to_le_bytes());
        h[16..24].copy_from_slice(&self.stream_id.to_le_bytes());
        h[24..32].copy_from_slice(&self.sequence.to_le_bytes());
        h
    }

    pub fn decode(bytes: &[u8], limits: &Limits) -> CoreResult<FrameHeader> {
        if bytes.len() < HEADER_LEN {
            return Err(CoreError::new(
                ErrorCode::ProtocolError,
                "truncated frame header",
            ));
        }
        if bytes[0..4] != MAGIC {
            return Err(CoreError::new(ErrorCode::ProtocolError, "bad magic"));
        }
        let major = u16::from_le_bytes([bytes[4], bytes[5]]);
        let minor = u16::from_le_bytes([bytes[6], bytes[7]]);
        if major != WIRE_MAJOR || minor != WIRE_MINOR {
            return Err(CoreError::new(
                ErrorCode::IncompatiblePeer,
                format!("unsupported wire version {major}.{minor}"),
            ));
        }
        let kind = MessageKind::from_u16(u16::from_le_bytes([bytes[8], bytes[9]]))?;
        let flags = u16::from_le_bytes([bytes[10], bytes[11]]);
        if flags != 0 {
            return Err(CoreError::new(
                ErrorCode::ProtocolError,
                format!("unsupported flags {flags:#x}"),
            ));
        }
        let payload_length = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        if payload_length as usize > limits.max_frame_payload {
            return Err(CoreError::new(
                ErrorCode::ResourceLimit,
                format!("payload length {payload_length} exceeds limit"),
            ));
        }
        let stream_id = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let sequence = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        Ok(FrameHeader {
            kind,
            payload_length,
            stream_id,
            sequence,
        })
    }
}

/// Encode a complete frame (header + payload). Fails if the payload exceeds the limit.
pub fn encode_frame(
    kind: MessageKind,
    stream_id: u64,
    sequence: u64,
    payload: &[u8],
    limits: &Limits,
) -> CoreResult<Vec<u8>> {
    if payload.len() > limits.max_frame_payload {
        return Err(CoreError::new(
            ErrorCode::ResourceLimit,
            "payload exceeds max_frame_payload",
        ));
    }
    if kind == MessageKind::Hello || kind == MessageKind::HelloAck {
        if stream_id != 0 {
            return Err(CoreError::new(
                ErrorCode::ProtocolError,
                "negotiation frames use stream 0",
            ));
        }
    } else if stream_id == 0 {
        return Err(CoreError::new(
            ErrorCode::ProtocolError,
            "non-negotiation frames need a nonzero stream id",
        ));
    }
    let header = FrameHeader {
        kind,
        payload_length: payload.len() as u32,
        stream_id,
        sequence,
    };
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&header.encode());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Incremental frame reader over an ordered byte stream. Enforces per-direction sequence
/// continuity (starts at 0, +1 each frame) and limits before allocation.
#[derive(Debug, Default)]
pub struct FrameReader {
    buf: Vec<u8>,
    next_sequence: u64,
    negotiated: bool,
    failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub header: FrameHeader,
    pub payload: Vec<u8>,
}

impl FrameReader {
    pub fn new() -> Self {
        Self::default()
    }
    /// Feed bytes; returns complete frames in order. A protocol failure poisons the reader.
    pub fn feed(&mut self, bytes: &[u8], limits: &Limits) -> CoreResult<Vec<Frame>> {
        if self.failed {
            return Err(CoreError::new(
                ErrorCode::ProtocolError,
                "connection failed; no further frames are dispatched",
            ));
        }
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < HEADER_LEN {
                break;
            }
            let header = match FrameHeader::decode(&self.buf[..HEADER_LEN], limits) {
                Ok(h) => h,
                Err(e) => {
                    self.failed = true;
                    return Err(e);
                }
            };
            if header.sequence != self.next_sequence {
                self.failed = true;
                return Err(CoreError::new(
                    ErrorCode::ProtocolError,
                    format!(
                        "frame sequence gap or reuse: expected {}, got {}",
                        self.next_sequence, header.sequence
                    ),
                ));
            }
            let is_negotiation = matches!(header.kind, MessageKind::Hello | MessageKind::HelloAck);
            if !self.negotiated && !is_negotiation {
                self.failed = true;
                return Err(CoreError::new(
                    ErrorCode::ProtocolError,
                    "frame before negotiation",
                ));
            }
            if is_negotiation && header.stream_id != 0 || !is_negotiation && header.stream_id == 0 {
                self.failed = true;
                return Err(CoreError::new(
                    ErrorCode::ProtocolError,
                    "stream id inconsistent with message kind",
                ));
            }
            let total = HEADER_LEN + header.payload_length as usize;
            if self.buf.len() < total {
                break;
            }
            let payload = self.buf[HEADER_LEN..total].to_vec();
            self.buf.drain(..total);
            self.next_sequence = self
                .next_sequence
                .checked_add(1)
                .ok_or_else(|| CoreError::new(ErrorCode::ProtocolError, "sequence exhausted"))?;
            if header.kind == MessageKind::HelloAck || header.kind == MessageKind::Hello {
                self.negotiated = true;
            }
            out.push(Frame { header, payload });
        }
        Ok(out)
    }
    /// Mark negotiation complete (after Hello/HelloAck semantic validation by the caller).
    pub fn mark_negotiated(&mut self) {
        self.negotiated = true;
    }
}
