//! Domain-separated SHA-256 (SPEC-003 §10, SPEC-012 §8).
//!
//! `hash = SHA-256(UTF8(domain) || 0x00 || canonical_bytes)`.

use sha2::{Digest, Sha256};
use std::fmt;

use crate::error::{CoreError, ErrorCode};

/// 32-byte digest, canonical text is 64 lowercase hexadecimal characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Hash256(pub [u8; 32]);

impl Hash256 {
    pub const ZERO: Hash256 = Hash256([0u8; 32]);

    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }

    pub fn from_hex(s: &str) -> Result<Self, CoreError> {
        let bytes = hex_decode(s)?;
        if bytes.len() != 32 {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                format!("hash must be 64 hex chars, got {}", s.len()),
            ));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(Hash256(out))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Hash256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash256({})", &self.to_hex()[..16])
    }
}

impl fmt::Display for Hash256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Compute `SHA-256(UTF8(domain) || 0x00 || payload)`.
pub fn domain_hash(domain: &str, payload: &[u8]) -> Hash256 {
    let mut h = Sha256::new();
    h.update(domain.as_bytes());
    h.update([0u8]);
    h.update(payload);
    let out = h.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    Hash256(arr)
}

/// Plain SHA-256 without domain separation (used only for physical checksums of opaque bytes).
pub fn sha256(payload: &[u8]) -> Hash256 {
    let out = Sha256::digest(payload);
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    Hash256(arr)
}

/// Hash-domain constants (byte-stable, historical `astra` codename).
pub mod domains {
    // SPEC-003 §10
    pub const SCHEMA_V1: &str = "astra.schema.v1";
    pub const OPERATION_V1: &str = "astra.operation.v1";
    pub const INVARIANT_V1: &str = "astra.invariant.v1";
    pub const CONTRACT_V1: &str = "astra.contract.v1";
    pub const MODULE_V1: &str = "astra.module.v1";
    // SPEC-004 §13
    pub const PLAN_V1: &str = "astra.plan.v1";
    pub const CERTIFICATE_V1: &str = "astra.certificate.v1";
    // SPEC-012 §8
    pub const REQUEST_V1: &str = "astra.request.v1";
    pub const RESULT_V1: &str = "astra.result.v1";
    pub const RECEIPT_V1: &str = "astra.receipt.v1";
    pub const PROTOCOL_RECORD_V1: &str = "astra.protocol-record.v1";
    pub const SNAPSHOT_CHUNK_V1: &str = "astra.snapshot-chunk.v1";
    pub const SNAPSHOT_MANIFEST_V1: &str = "astra.snapshot-manifest.v1";
    pub const NEGOTIATION_V1: &str = "astra.negotiation.v1";
    // SPEC-002 §41 semantic digest of a CompiledBatch payload
    pub const BATCH_V1: &str = "astra.batch.v1";
    // SPEC-003 §7 normalized invocation
    pub const INVOCATION_V1: &str = "astra.invocation.v1";
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Strict lowercase hexadecimal decoder: even length, only `0-9a-f`.
pub fn hex_decode(s: &str) -> Result<Vec<u8>, CoreError> {
    if s.len() % 2 != 0 {
        return Err(CoreError::new(
            ErrorCode::NonCanonicalEncoding,
            "odd hex length",
        ));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    let nib = |c: u8| -> Result<u8, CoreError> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "non-canonical hex (uppercase or non-hex)",
            )),
        }
    };
    for i in (0..b.len()).step_by(2) {
        out.push((nib(b[i])? << 4) | nib(b[i + 1])?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_separation_changes_digest() {
        let a = domain_hash("astra.schema.v1", b"{}");
        let b = domain_hash("astra.operation.v1", b"{}");
        assert_ne!(a, b);
        assert_eq!(a, domain_hash("astra.schema.v1", b"{}"));
    }

    #[test]
    fn hex_roundtrip_and_rejects_uppercase() {
        let h = sha256(b"x");
        assert_eq!(Hash256::from_hex(&h.to_hex()).unwrap(), h);
        assert!(Hash256::from_hex(&h.to_hex().to_uppercase()).is_err());
        assert!(Hash256::from_hex("abc").is_err());
    }

    #[test]
    fn known_vector() {
        // SHA-256("x") = 2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881
        assert_eq!(
            sha256(b"x").to_hex(),
            "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
        );
    }
}
