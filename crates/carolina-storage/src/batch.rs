//! `CompiledBatch`, `ProtocolOnlyBatch`, `ProtocolMutation` and `TxnStatusRecord` (SPEC-002 §41–§44, §70).
//!
//! These are the normative logical schemas of the durable boundary. They are persisted through
//! the Commit Journal in canonical encoding (`compiled_batch:1` / `protocol_only_batch:1`).
//! `SemanticDigest` hashes the semantic payload excluding physical LSN/seq, transport envelopes
//! and any evidence that later references the digest.

use carolina_core::canon::{decode_set, CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, domains};
use carolina_core::ids::*;
use carolina_core::keycodec::{KeyReader, Namespace};
use carolina_wire::records::{AcceptedResultV1, FinalReceiptV1};

pub const BATCH_VERSION: u32 = 1;

/// Opaque canonical logical key bytes (SPEC-002 §5) — the first byte is the namespace tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LogicalKey(pub Vec<u8>);

impl LogicalKey {
    pub fn namespace(&self) -> CoreResult<Namespace> {
        let mut r = KeyReader::new(&self.0);
        r.namespace()
    }
}

impl Canonical for LogicalKey {
    fn to_canon(&self) -> CanonValue {
        CanonValue::bytes(&self.0)
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(LogicalKey(v.as_bytes()?))
    }
}

/// Semantic metadata attached to a version (SPEC-002 §18–§19): flags and opaque canonical bytes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SemanticMeta {
    pub flags: u8,
    pub bytes: Vec<u8>,
}

pub const FLAG_TOMBSTONE: u8 = 0x01;
pub const FLAG_SYSTEM: u8 = 0x02;
pub const FLAG_HAS_CAUSAL_META: u8 = 0x04;
pub const FLAG_HAS_SERIAL_META: u8 = 0x08;
pub const FLAG_HAS_ESCROW_META: u8 = 0x10;

impl Canonical for SemanticMeta {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fbytes("bytes", &self.bytes)
            .fu32("flags", self.flags as u32)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["bytes", "flags"])?;
        Ok(SemanticMeta {
            flags: v.field("flags")?.as_u32()? as u8,
            bytes: v.field("bytes")?.as_bytes()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageMutation {
    Put {
        key: LogicalKey,
        value: Vec<u8>,
        semantic_meta: SemanticMeta,
    },
    Delete {
        key: LogicalKey,
        semantic_meta: SemanticMeta,
    },
}

impl StorageMutation {
    pub fn key(&self) -> &LogicalKey {
        match self {
            StorageMutation::Put { key, .. } | StorageMutation::Delete { key, .. } => key,
        }
    }
}

impl Canonical for StorageMutation {
    fn to_canon(&self) -> CanonValue {
        match self {
            StorageMutation::Put {
                key,
                value,
                semantic_meta,
            } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "put")
                .fc("semantic_meta", semantic_meta)
                .fbytes("value", value)
                .build(),
            StorageMutation::Delete { key, semantic_meta } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "delete")
                .fc("semantic_meta", semantic_meta)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "put" => {
                v.expect_fields(&["key", "kind", "semantic_meta", "value"])?;
                StorageMutation::Put {
                    key: LogicalKey::from_canon(v.field("key")?)?,
                    value: v.field("value")?.as_bytes()?,
                    semantic_meta: SemanticMeta::from_canon(v.field("semantic_meta")?)?,
                }
            }
            "delete" => {
                v.expect_fields(&["key", "kind", "semantic_meta"])?;
                StorageMutation::Delete {
                    key: LogicalKey::from_canon(v.field("key")?)?,
                    semantic_meta: SemanticMeta::from_canon(v.field("semantic_meta")?)?,
                }
            }
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("mutation kind {k}"),
                ))
            }
        })
    }
}

/// `ExpectedRecordRevision = Absent | Exact(RecordRevision)`: there is no unchecked overwrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedRecordRevision {
    Absent,
    Exact(RecordRevision),
}

impl Canonical for ExpectedRecordRevision {
    fn to_canon(&self) -> CanonValue {
        match self {
            ExpectedRecordRevision::Absent => CanonValue::obj().fstr("kind", "absent").build(),
            ExpectedRecordRevision::Exact(r) => CanonValue::obj()
                .fstr("kind", "exact")
                .fc("revision", r)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "absent" => ExpectedRecordRevision::Absent,
            "exact" => {
                ExpectedRecordRevision::Exact(RecordRevision::from_canon(v.field("revision")?)?)
            }
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("expected revision kind {k}"),
                ))
            }
        })
    }
}

/// `ProtocolRecordKey = (record_kind, scope_key, record_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProtocolRecordKey {
    pub record_kind: String,
    pub scope_key: Vec<u8>,
    pub record_id: Vec<u8>,
}

impl ProtocolRecordKey {
    /// Physical logical key: namespace by kind + kind string + scope + id.
    pub fn logical_key(&self) -> LogicalKey {
        let ns = match self.record_kind.as_str() {
            "txn_status" | "local_decision" => Namespace::TxnStatus,
            "request_binding" | "namespace_retirement" | "final_receipt" => {
                Namespace::RequestBinding
            }
            "holder_state" | "transfer_terms" | "transfer_decision" => Namespace::Escrow,
            "semantic_commit" | "causal_context" | "outbox" | "inbox" => Namespace::Replication,
            "plan" | "artifact" => Namespace::Plan,
            "authority_grant" | "catalog_command" | "catalog_commit" | "migration_record"
            | "close_certificate" => Namespace::Catalog,
            _ => Namespace::System,
        };
        LogicalKey(
            carolina_core::keycodec::KeyWriter::new()
                .namespace(ns)
                .str(&self.record_kind)
                .bytes(&self.scope_key)
                .bytes(&self.record_id)
                .finish(),
        )
    }
}

impl Canonical for ProtocolRecordKey {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fbytes("record_id", &self.record_id)
            .fstr("record_kind", &self.record_kind)
            .fbytes("scope_key", &self.scope_key)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["record_id", "record_kind", "scope_key"])?;
        Ok(ProtocolRecordKey {
            record_kind: v.field("record_kind")?.as_str()?.to_string(),
            scope_key: v.field("scope_key")?.as_bytes()?,
            record_id: v.field("record_id")?.as_bytes()?,
        })
    }
}

/// `VersionedProtocolRecord = (record_kind, record_version, canonical_payload)` from the closed registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedProtocolRecord {
    pub record_kind: String,
    pub record_version: u32,
    pub canonical_payload: Vec<u8>,
}

impl VersionedProtocolRecord {
    pub fn validate(&self) -> CoreResult<()> {
        if !carolina_wire::registry::is_registered(&self.record_kind, self.record_version) {
            return Err(CoreError::new(
                ErrorCode::UnsupportedCodec,
                format!(
                    "unregistered protocol record {}:{}",
                    self.record_kind, self.record_version
                ),
            ));
        }
        // payload must be canonical bytes
        carolina_core::canon::CanonValue::decode(
            &self.canonical_payload,
            &carolina_core::limits::Limits::v1(),
        )?;
        Ok(())
    }
    pub fn reference(&self, key: &ProtocolRecordKey) -> ProtocolRecordRef {
        ProtocolRecordRef {
            record_kind: self.record_kind.clone(),
            record_version: self.record_version,
            record_key: key.logical_key().0.clone(),
            payload_hash: domain_hash(domains::PROTOCOL_RECORD_V1, &self.canonical_payload),
        }
    }
}

impl Canonical for VersionedProtocolRecord {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fbytes("canonical_payload", &self.canonical_payload)
            .fstr("record_kind", &self.record_kind)
            .fu32("record_version", self.record_version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["canonical_payload", "record_kind", "record_version"])?;
        Ok(VersionedProtocolRecord {
            record_kind: v.field("record_kind")?.as_str()?.to_string(),
            record_version: v.field("record_version")?.as_u32()?,
            canonical_payload: v.field("canonical_payload")?.as_bytes()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolRecordWrite {
    pub key: ProtocolRecordKey,
    pub expected: ExpectedRecordRevision,
    pub next: VersionedProtocolRecord,
}

impl Canonical for ProtocolRecordWrite {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("expected", &self.expected)
            .fc("key", &self.key)
            .fc("next", &self.next)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["expected", "key", "next"])?;
        Ok(ProtocolRecordWrite {
            key: ProtocolRecordKey::from_canon(v.field("key")?)?,
            expected: ExpectedRecordRevision::from_canon(v.field("expected")?)?,
            next: VersionedProtocolRecord::from_canon(v.field("next")?)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TxnPhase {
    Bound,
    Admitted,
    Prepared,
    Installed,
    Aborted,
    Terminal,
}

impl TxnPhase {
    pub fn label(&self) -> &'static str {
        match self {
            TxnPhase::Bound => "BOUND",
            TxnPhase::Admitted => "ADMITTED",
            TxnPhase::Prepared => "PREPARED",
            TxnPhase::Installed => "INSTALLED",
            TxnPhase::Aborted => "ABORTED",
            TxnPhase::Terminal => "TERMINAL",
        }
    }
    pub fn from_label(s: &str) -> CoreResult<Self> {
        Ok(match s {
            "BOUND" => TxnPhase::Bound,
            "ADMITTED" => TxnPhase::Admitted,
            "PREPARED" => TxnPhase::Prepared,
            "INSTALLED" => TxnPhase::Installed,
            "ABORTED" => TxnPhase::Aborted,
            "TERMINAL" => TxnPhase::Terminal,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("txn phase {k}"),
                ))
            }
        })
    }
    /// Legal predecessor/successor transitions (SPEC-002 §70): BOUND -> ADMITTED -> PREPARED -> INSTALLED -> TERMINAL, ADMITTED -> TERMINAL, PREPARED -> ABORTED.
    pub fn can_advance_to(&self, next: TxnPhase) -> bool {
        matches!(
            (self, next),
            (TxnPhase::Bound, TxnPhase::Admitted)
                | (TxnPhase::Admitted, TxnPhase::Prepared)
                | (TxnPhase::Admitted, TxnPhase::Terminal)
                | (TxnPhase::Prepared, TxnPhase::Installed)
                | (TxnPhase::Prepared, TxnPhase::Aborted)
                | (TxnPhase::Installed, TxnPhase::Terminal)
                | (TxnPhase::Aborted, TxnPhase::Terminal)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalOutcome {
    Committed(FinalReceiptV1),
    Rejected(FinalReceiptV1),
}

impl TerminalOutcome {
    pub fn receipt(&self) -> &FinalReceiptV1 {
        match self {
            TerminalOutcome::Committed(r) | TerminalOutcome::Rejected(r) => r,
        }
    }
}

impl Canonical for TerminalOutcome {
    fn to_canon(&self) -> CanonValue {
        match self {
            TerminalOutcome::Committed(r) => CanonValue::obj()
                .fstr("kind", "committed")
                .fc("receipt", r)
                .build(),
            TerminalOutcome::Rejected(r) => CanonValue::obj()
                .fstr("kind", "rejected")
                .fc("receipt", r)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["kind", "receipt"])?;
        Ok(match v.field("kind")?.as_str()? {
            "committed" => {
                TerminalOutcome::Committed(FinalReceiptV1::from_canon(v.field("receipt")?)?)
            }
            "rejected" => {
                TerminalOutcome::Rejected(FinalReceiptV1::from_canon(v.field("receipt")?)?)
            }
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("terminal outcome {k}"),
                ))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxnStatusRecord {
    pub revision: RecordRevision,
    pub phase: TxnPhase,
    pub plan: PlanRef,
    pub idc_bindings: Vec<IdcBinding>,
    pub prepared_digest: Option<SemanticDigest>,
    pub decision_ref: Option<ProtocolRecordRef>,
    pub accepted_result: Option<AcceptedResultV1>,
    pub terminal_outcome: Option<TerminalOutcome>,
}

impl TxnStatusRecord {
    pub fn validate(&self) -> CoreResult<()> {
        match (self.phase, self.terminal_outcome.is_some()) {
            (TxnPhase::Terminal, false) => Err(CoreError::new(
                ErrorCode::InvalidEvidence,
                "TERMINAL status without terminal outcome",
            )),
            (TxnPhase::Terminal, true) => Ok(()),
            (_, true) => Err(CoreError::new(
                ErrorCode::InvalidEvidence,
                "non-terminal status carries a terminal outcome",
            )),
            (_, false) => Ok(()),
        }
    }
    /// Validate a legal successor relationship, including immutability of identity fields.
    pub fn validate_transition(
        prev: Option<&TxnStatusRecord>,
        next: &TxnStatusRecord,
    ) -> CoreResult<()> {
        next.validate()?;
        match prev {
            None => {
                if next.revision != RecordRevision(1) {
                    return Err(CoreError::new(
                        ErrorCode::Conflict,
                        "first status record must have revision 1",
                    ));
                }
                // any phase may be the first durable record: earlier phases may have lived only in
                // memory (BOUND/ADMITTED) or in the journal (PREPARED, see Store::decision_status)
                let _ = next.phase;
            }
            Some(p) => {
                if next.revision != RecordRevision(p.revision.0 + 1) {
                    return Err(CoreError::new(
                        ErrorCode::Conflict,
                        "status revision must advance by one",
                    ));
                }
                if p.phase == next.phase && p == next {
                    return Ok(());
                }
                if !p.phase.can_advance_to(next.phase) {
                    return Err(CoreError::new(
                        ErrorCode::Conflict,
                        format!(
                            "illegal txn phase transition {} -> {}",
                            p.phase.label(),
                            next.phase.label()
                        ),
                    ));
                }
                if p.plan != next.plan || p.idc_bindings != next.idc_bindings {
                    return Err(CoreError::new(
                        ErrorCode::IdentityMismatch,
                        "status transition changes plan or IDC bindings",
                    ));
                }
                if let (Some(a), Some(b)) = (&p.terminal_outcome, &next.terminal_outcome) {
                    if a != b {
                        return Err(CoreError::new(
                            ErrorCode::IdentityMismatch,
                            "terminal outcome cannot change",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

impl Canonical for TxnStatusRecord {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fopt("accepted_result", &self.accepted_result)
            .fopt("decision_ref", &self.decision_ref)
            .fset("idc_bindings", &self.idc_bindings)
            .fstr("phase", self.phase.label())
            .fc("plan", &self.plan)
            .fopt("prepared_digest", &self.prepared_digest)
            .fc("revision", &self.revision)
            .fopt("terminal_outcome", &self.terminal_outcome)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "accepted_result",
            "decision_ref",
            "idc_bindings",
            "phase",
            "plan",
            "prepared_digest",
            "revision",
            "terminal_outcome",
        ])?;
        let r = TxnStatusRecord {
            revision: RecordRevision::from_canon(v.field("revision")?)?,
            phase: TxnPhase::from_label(v.field("phase")?.as_str()?)?,
            plan: PlanRef::from_canon(v.field("plan")?)?,
            idc_bindings: decode_set(v.field("idc_bindings")?)?,
            prepared_digest: Option::from_canon(v.field("prepared_digest")?)?,
            decision_ref: Option::from_canon(v.field("decision_ref")?)?,
            accepted_result: Option::from_canon(v.field("accepted_result")?)?,
            terminal_outcome: Option::from_canon(v.field("terminal_outcome")?)?,
        };
        r.validate()?;
        Ok(r)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxnStatusTransition {
    pub txn_id: TxnId,
    pub request_key: RequestKey,
    pub request_hash: RequestHash,
    pub expected_revision: ExpectedRecordRevision,
    pub next: TxnStatusRecord,
}

impl Canonical for TxnStatusTransition {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("expected_revision", &self.expected_revision)
            .fc("next", &self.next)
            .fc("request_hash", &self.request_hash)
            .fc("request_key", &self.request_key)
            .fc("txn_id", &self.txn_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "expected_revision",
            "next",
            "request_hash",
            "request_key",
            "txn_id",
        ])?;
        Ok(TxnStatusTransition {
            txn_id: TxnId::from_canon(v.field("txn_id")?)?,
            request_key: RequestKey::from_canon(v.field("request_key")?)?,
            request_hash: RequestHash::from_canon(v.field("request_hash")?)?,
            expected_revision: ExpectedRecordRevision::from_canon(v.field("expected_revision")?)?,
            next: TxnStatusRecord::from_canon(v.field("next")?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// status transitions are rare next to record writes; boxing would buy nothing at the journal boundary
#[allow(clippy::large_enum_variant)]
pub enum ProtocolMutation {
    PutRecord(ProtocolRecordWrite),
    SetTxnStatus(TxnStatusTransition),
}

impl Canonical for ProtocolMutation {
    fn to_canon(&self) -> CanonValue {
        match self {
            ProtocolMutation::PutRecord(w) => CanonValue::obj()
                .fstr("kind", "put_record")
                .fc("write", w)
                .build(),
            ProtocolMutation::SetTxnStatus(t) => CanonValue::obj()
                .fstr("kind", "set_txn_status")
                .fc("transition", t)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "put_record" => {
                ProtocolMutation::PutRecord(ProtocolRecordWrite::from_canon(v.field("write")?)?)
            }
            "set_txn_status" => ProtocolMutation::SetTxnStatus(TxnStatusTransition::from_canon(
                v.field("transition")?,
            )?),
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("protocol mutation {k}"),
                ))
            }
        })
    }
}

/// The atomic local boundary for dependent business and protocol state (SPEC-002 §41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledBatch {
    pub batch_version: u32,
    pub txn_id: TxnId,
    pub request_key: RequestKey,
    pub request_hash: RequestHash,
    pub operation: OperationRef,
    pub operation_hash: OperationHash,
    pub contract_hash: ContractHash,
    pub schema_hash: SchemaHash,
    pub plan: PlanRef,
    pub idc_bindings: Vec<IdcBinding>,
    pub consistency_class: ConsistencyClass,
    pub origin: Option<OriginId>,
    pub semantic_evidence: Vec<ProtocolRecordRef>,
    pub captured_inputs: Vec<u8>,
    pub mutations: Vec<StorageMutation>,
    pub protocol_mutations: Vec<ProtocolMutation>,
    pub terminal_outcome: Option<TerminalOutcome>,
    pub semantic_digest: SemanticDigest,
}

impl CompiledBatch {
    /// Canonical semantic payload without the digest field (SPEC-002 §41).
    pub fn semantic_payload(&self) -> CanonValue {
        CanonValue::obj()
            .fu32("batch_version", self.batch_version)
            .fbytes("captured_inputs", &self.captured_inputs)
            .fc("consistency_class", &self.consistency_class)
            .fc("contract_hash", &self.contract_hash)
            .fset("idc_bindings", &self.idc_bindings)
            .fstr("kind", "batch.v1")
            .fvec("mutations", &self.mutations)
            .fc("operation", &self.operation)
            .fc("operation_hash", &self.operation_hash)
            .fopt("origin", &self.origin)
            .fc("plan", &self.plan)
            .fvec("protocol_mutations", &self.protocol_mutations)
            .fc("request_hash", &self.request_hash)
            .fc("request_key", &self.request_key)
            .fc("schema_hash", &self.schema_hash)
            .fset("semantic_evidence", &self.semantic_evidence)
            .fopt("terminal_outcome", &self.terminal_outcome)
            .fc("txn_id", &self.txn_id)
            .build()
    }
    pub fn compute_digest(&self) -> SemanticDigest {
        SemanticDigest(domain_hash(
            domains::BATCH_V1,
            &self.semantic_payload().encode(),
        ))
    }
    /// Seal the digest (call after filling all fields).
    pub fn seal(mut self) -> CompiledBatch {
        self.semantic_digest = self.compute_digest();
        self
    }
    pub fn verify(&self) -> CoreResult<()> {
        if self.batch_version != BATCH_VERSION {
            return Err(CoreError::new(
                ErrorCode::UnsupportedFormat,
                "batch version",
            ));
        }
        if self.compute_digest() != self.semantic_digest {
            return Err(CoreError::new(
                ErrorCode::Corruption,
                "semantic digest mismatch",
            ));
        }
        let mut seen: std::collections::BTreeSet<&LogicalKey> = std::collections::BTreeSet::new();
        for m in &self.mutations {
            let ns = m.key().namespace()?;
            if !ns.is_user_writable() {
                return Err(CoreError::new(
                    ErrorCode::ReadOnly,
                    format!("business mutation targets reserved namespace {ns:?}"),
                ));
            }
            if !seen.insert(m.key()) {
                // a batch carries the final per-key effect of one execution: one mutation per key
                return Err(CoreError::new(
                    ErrorCode::ProtocolError,
                    "duplicate logical key in one batch",
                ));
            }
        }
        for pm in &self.protocol_mutations {
            match pm {
                ProtocolMutation::PutRecord(w) => w.next.validate()?,
                ProtocolMutation::SetTxnStatus(t) => {
                    if t.txn_id != self.txn_id
                        || t.request_key != self.request_key
                        || t.request_hash != self.request_hash
                    {
                        return Err(CoreError::new(
                            ErrorCode::IdentityMismatch,
                            "status transition identity differs from batch identity",
                        ));
                    }
                    t.next.validate()?;
                }
            }
        }
        if let Some(t) = &self.terminal_outcome {
            let r = t.receipt();
            if r.txn_id != self.txn_id
                || r.request_key != self.request_key
                || r.request_hash != self.request_hash
                || r.plan != self.plan
            {
                return Err(CoreError::new(
                    ErrorCode::IdentityMismatch,
                    "terminal receipt identity differs from batch identity",
                ));
            }
        }
        Ok(())
    }
}

impl Canonical for CompiledBatch {
    fn to_canon(&self) -> CanonValue {
        let mut o = self.semantic_payload().as_object().unwrap().clone();
        o.insert("semantic_digest".into(), self.semantic_digest.to_canon());
        CanonValue::Object(o)
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "batch_version",
            "captured_inputs",
            "consistency_class",
            "contract_hash",
            "idc_bindings",
            "kind",
            "mutations",
            "operation",
            "operation_hash",
            "origin",
            "plan",
            "protocol_mutations",
            "request_hash",
            "request_key",
            "schema_hash",
            "semantic_digest",
            "semantic_evidence",
            "terminal_outcome",
            "txn_id",
        ])?;
        let b = CompiledBatch {
            batch_version: v.field("batch_version")?.as_u32()?,
            txn_id: TxnId::from_canon(v.field("txn_id")?)?,
            request_key: RequestKey::from_canon(v.field("request_key")?)?,
            request_hash: RequestHash::from_canon(v.field("request_hash")?)?,
            operation: OperationRef::from_canon(v.field("operation")?)?,
            operation_hash: OperationHash::from_canon(v.field("operation_hash")?)?,
            contract_hash: ContractHash::from_canon(v.field("contract_hash")?)?,
            schema_hash: SchemaHash::from_canon(v.field("schema_hash")?)?,
            plan: PlanRef::from_canon(v.field("plan")?)?,
            idc_bindings: decode_set(v.field("idc_bindings")?)?,
            consistency_class: ConsistencyClass::from_canon(v.field("consistency_class")?)?,
            origin: Option::from_canon(v.field("origin")?)?,
            semantic_evidence: decode_set(v.field("semantic_evidence")?)?,
            captured_inputs: v.field("captured_inputs")?.as_bytes()?,
            mutations: Vec::from_canon(v.field("mutations")?)?,
            protocol_mutations: Vec::from_canon(v.field("protocol_mutations")?)?,
            terminal_outcome: Option::from_canon(v.field("terminal_outcome")?)?,
            semantic_digest: SemanticDigest::from_canon(v.field("semantic_digest")?)?,
        };
        b.verify()?;
        Ok(b)
    }
}

/// Internal protocol-only batch (SPEC-002 §42): no user mutations, no client identity invented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolOnlyBatch {
    pub internal_record_id: ProtocolRecordKey,
    pub authorizing_evidence: Vec<ProtocolRecordRef>,
    pub writes: Vec<ProtocolRecordWrite>,
}

impl ProtocolOnlyBatch {
    pub fn digest(&self) -> SemanticDigest {
        SemanticDigest(domain_hash("astra.protocol-batch.v1", &self.encode()))
    }
}

impl Canonical for ProtocolOnlyBatch {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fset("authorizing_evidence", &self.authorizing_evidence)
            .fc("internal_record_id", &self.internal_record_id)
            .fstr("kind", "protocol-batch.v1")
            .fvec("writes", &self.writes)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "authorizing_evidence",
            "internal_record_id",
            "kind",
            "writes",
        ])?;
        Ok(ProtocolOnlyBatch {
            internal_record_id: ProtocolRecordKey::from_canon(v.field("internal_record_id")?)?,
            authorizing_evidence: decode_set(v.field("authorizing_evidence")?)?,
            writes: Vec::from_canon(v.field("writes")?)?,
        })
    }
}
