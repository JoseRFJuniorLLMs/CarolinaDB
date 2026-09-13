//! Registered canonical record wrapper and protocol-record references (SPEC-012 §7–§8).
//!
//! `{"body":<typed body>,"record_kind":<registered name>,"record_version":"1"}`.
//! Protocol-record hashes use `astra.protocol-record.v1` over the complete wrapped record.

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, domains, Hash256};
use carolina_core::ids::ProtocolRecordRef;
use carolina_core::limits::Limits;

/// Closed registry of record kinds understood by this build. Unknown kinds fail closed.
pub const REGISTERED_KINDS: &[(&str, u32, &str)] = &[
    // kind, version, owning SPEC
    ("request_content", 1, "SPEC-012"),
    ("invoke", 1, "SPEC-012"),
    ("client_reply", 1, "SPEC-012"),
    ("resolve_request", 1, "SPEC-012"),
    ("resolve_reply", 1, "SPEC-012"),
    ("request_binding", 1, "SPEC-012"),
    ("accepted_result", 1, "SPEC-012"),
    ("final_receipt", 1, "SPEC-012"),
    ("result_tombstone", 1, "SPEC-012"),
    ("observation_token", 1, "SPEC-012"),
    ("read_contract", 1, "SPEC-012"),
    ("hello", 1, "SPEC-012"),
    ("hello_ack", 1, "SPEC-012"),
    ("snapshot_manifest", 1, "SPEC-012"),
    ("snapshot_chunk", 1, "SPEC-012"),
    ("codec_manifest", 1, "SPEC-012"),
    ("compiled_batch", 1, "SPEC-002"),
    ("protocol_only_batch", 1, "SPEC-002"),
    ("txn_status", 1, "SPEC-002"),
    ("local_decision", 1, "SPEC-002"),
    ("namespace_retirement", 1, "SPEC-011"),
    ("authority_grant", 1, "SPEC-011"),
    ("request_home_state", 1, "SPEC-012"),
    ("catalog_command", 1, "SPEC-011"),
    ("catalog_commit", 1, "SPEC-011"),
    ("migration_record", 1, "SPEC-009"),
    ("close_certificate", 1, "SPEC-009"),
    ("semantic_commit", 1, "SPEC-005"),
    ("causal_context", 1, "SPEC-005"),
    ("session_token", 1, "SPEC-005"),
    ("holder_state", 1, "SPEC-006"),
    ("transfer_terms", 1, "SPEC-006"),
    ("transfer_decision", 1, "SPEC-006"),
    ("txn_begin", 1, "SPEC-008"),
    ("prepare_vote", 1, "SPEC-008"),
    ("decision_certificate", 1, "SPEC-008"),
    ("installed", 1, "SPEC-008"),
    ("publication_certificate", 1, "SPEC-008"),
    ("publish_seen", 1, "SPEC-008"),
    ("completion_certificate", 1, "SPEC-008"),
];

pub fn is_registered(kind: &str, version: u32) -> bool {
    REGISTERED_KINDS
        .iter()
        .any(|(k, v, _)| *k == kind && *v == version)
}

/// A wrapped canonical record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRecord {
    pub record_kind: String,
    pub record_version: u32,
    pub body: CanonValue,
}

impl CanonicalRecord {
    pub fn wrap<T: Canonical>(kind: &str, body: &T) -> CoreResult<CanonicalRecord> {
        if !is_registered(kind, 1) {
            return Err(CoreError::new(
                ErrorCode::UnsupportedCodec,
                format!("record kind `{kind}` is not registered"),
            ));
        }
        Ok(CanonicalRecord {
            record_kind: kind.into(),
            record_version: 1,
            body: body.to_canon(),
        })
    }
    pub fn body<T: Canonical>(&self) -> CoreResult<T> {
        T::from_canon(&self.body)
    }
    /// `astra.protocol-record.v1` hash over the complete wrapped record.
    pub fn record_hash(&self) -> Hash256 {
        domain_hash(domains::PROTOCOL_RECORD_V1, &self.encode())
    }
    pub fn reference(&self, record_key: Vec<u8>) -> ProtocolRecordRef {
        ProtocolRecordRef {
            record_kind: self.record_kind.clone(),
            record_version: self.record_version,
            record_key,
            payload_hash: self.record_hash(),
        }
    }
    pub fn decode_checked(bytes: &[u8], limits: &Limits) -> CoreResult<CanonicalRecord> {
        let r = CanonicalRecord::decode(bytes, limits)?;
        if !is_registered(&r.record_kind, r.record_version) {
            return Err(CoreError::new(
                ErrorCode::UnsupportedCodec,
                format!(
                    "unknown record kind/version {}/{}",
                    r.record_kind, r.record_version
                ),
            ));
        }
        Ok(r)
    }
}

impl Canonical for CanonicalRecord {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .f("body", self.body.clone())
            .fstr("record_kind", &self.record_kind)
            .fu32("record_version", self.record_version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["body", "record_kind", "record_version"])?;
        Ok(CanonicalRecord {
            record_kind: v.field("record_kind")?.as_str()?.to_string(),
            record_version: v.field("record_version")?.as_u32()?,
            body: v.field("body")?.clone(),
        })
    }
}

/// Verify that a reference matches a record's bytes (kind, version and payload hash).
pub fn verify_reference(r: &ProtocolRecordRef, record: &CanonicalRecord) -> CoreResult<()> {
    if r.record_kind != record.record_kind || r.record_version != record.record_version {
        return Err(CoreError::new(
            ErrorCode::InvalidEvidence,
            "protocol record kind/version mismatch",
        ));
    }
    if r.payload_hash != record.record_hash() {
        return Err(CoreError::new(
            ErrorCode::InvalidEvidence,
            "protocol record payload hash mismatch",
        ));
    }
    Ok(())
}
