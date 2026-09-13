//! Reference in-memory kernel with an explicit simulated durability model (SPEC-002 §123, §144).
//!
//! Every accepted commit is recorded as durable only after the simulated barrier; `crash()`
//! discards everything that never crossed it. This backend never claims disk durability; its
//! purpose is differential testing of the native kernel and deterministic simulation.

use std::collections::BTreeMap;

use carolina_core::canon::Canonical;
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::*;
use carolina_core::limits::Limits;
use carolina_wire::records::RequestBindingV1;

use crate::batch::*;
use crate::kernel::*;

#[derive(Debug, Clone)]
struct Version {
    seq: u64,
    value: Option<Vec<u8>>,
    durable: bool,
}

#[derive(Debug, Default, Clone)]
pub struct MemKernel {
    versions: BTreeMap<Vec<u8>, Vec<Version>>,
    records: BTreeMap<Vec<u8>, (RecordRevision, VersionedProtocolRecord, bool)>,
    statuses: BTreeMap<TxnId, (TxnStatusRecord, bool)>,
    prepared: BTreeMap<TxnId, (CompiledBatch, bool)>,
    next_seq: u64,
    durable_seq: u64,
    /// When false, "sync" is skipped and nothing becomes durable until `sync()` is called explicitly.
    pub sync_each_commit: bool,
    pub commits: u64,
}

impl MemKernel {
    pub fn new() -> MemKernel {
        MemKernel {
            next_seq: 1,
            sync_each_commit: true,
            ..Default::default()
        }
    }

    /// Simulated durable barrier: everything appended so far becomes durable.
    pub fn sync(&mut self) {
        for vs in self.versions.values_mut() {
            for v in vs {
                v.durable = true;
            }
        }
        for r in self.records.values_mut() {
            r.2 = true;
        }
        for s in self.statuses.values_mut() {
            s.1 = true;
        }
        for p in self.prepared.values_mut() {
            p.1 = true;
        }
        self.durable_seq = self.next_seq - 1;
    }

    /// Simulated process crash: forget every non-durable change.
    pub fn crash(&mut self) {
        for vs in self.versions.values_mut() {
            vs.retain(|v| v.durable);
        }
        self.versions.retain(|_, vs| !vs.is_empty());
        self.records.retain(|_, r| r.2);
        self.statuses.retain(|_, s| s.1);
        self.prepared.retain(|_, p| p.1);
        self.next_seq = self.durable_seq + 1;
    }

    fn newest_visible(&self, key: &[u8], visible: u64) -> Option<&Version> {
        self.versions
            .get(key)?
            .iter()
            .rev()
            .find(|v| v.seq <= visible)
    }

    fn current_record(
        &self,
        lk: &[u8],
    ) -> Option<&(RecordRevision, VersionedProtocolRecord, bool)> {
        self.records.get(lk)
    }

    fn validate(&self, pms: &[ProtocolMutation]) -> CoreResult<()> {
        for pm in pms {
            match pm {
                ProtocolMutation::PutRecord(w) => {
                    let lk = w.key.logical_key().0;
                    match (&w.expected, self.current_record(&lk)) {
                        (ExpectedRecordRevision::Absent, None) => {}
                        (ExpectedRecordRevision::Absent, Some((_, ex, _))) if *ex == w.next => {}
                        (ExpectedRecordRevision::Absent, Some(_)) => {
                            return Err(CoreError::new(ErrorCode::Conflict, "record exists"))
                        }
                        (ExpectedRecordRevision::Exact(r), Some((rev, _, _))) if r == rev => {}
                        _ => return Err(CoreError::new(ErrorCode::Conflict, "revision mismatch")),
                    }
                }
                ProtocolMutation::SetTxnStatus(t) => {
                    let prev = self.statuses.get(&t.txn_id).map(|s| s.0.clone());
                    match (&t.expected_revision, &prev) {
                        (ExpectedRecordRevision::Absent, None) => {}
                        (ExpectedRecordRevision::Absent, Some(p)) if *p == t.next => continue,
                        (ExpectedRecordRevision::Exact(r), Some(p)) if *r == p.revision => {}
                        _ => {
                            return Err(CoreError::new(
                                ErrorCode::Conflict,
                                "status revision mismatch",
                            ))
                        }
                    }
                    TxnStatusRecord::validate_transition(prev.as_ref(), &t.next)?;
                }
            }
        }
        Ok(())
    }

    fn install(&mut self, batch: &CompiledBatch, seq: u64) {
        for m in &batch.mutations {
            let (k, v) = match m {
                StorageMutation::Put { key, value, .. } => (key.0.clone(), Some(value.clone())),
                StorageMutation::Delete { key, .. } => (key.0.clone(), None),
            };
            self.versions.entry(k).or_default().push(Version {
                seq,
                value: v,
                durable: false,
            });
        }
        for pm in &batch.protocol_mutations {
            match pm {
                ProtocolMutation::PutRecord(w) => {
                    let lk = w.key.logical_key().0;
                    let rev = match self.records.get(&lk) {
                        Some((r, ex, _)) if *ex == w.next => *r,
                        Some((r, _, _)) => RecordRevision(r.0 + 1),
                        None => RecordRevision(1),
                    };
                    self.records.insert(lk, (rev, w.next.clone(), false));
                }
                ProtocolMutation::SetTxnStatus(t) => {
                    self.statuses.insert(t.txn_id, (t.next.clone(), false));
                }
            }
        }
    }

    /// Mirror of `Store::decision_status`: the decision record replaces the journal-only PREPARED status.
    fn record_decision(
        &mut self,
        batch: &CompiledBatch,
        decision_ref: &ProtocolRecordRef,
        aborted: bool,
    ) {
        let rec = crate::kernel::Store::decision_status(batch, decision_ref, aborted);
        self.statuses.insert(batch.txn_id, (rec, false));
    }

    fn decided_error(&self, txn: TxnId) -> CoreError {
        match self.statuses.get(&txn).map(|(r, _)| r.phase) {
            Some(TxnPhase::Installed) | Some(TxnPhase::Terminal) => CoreError::new(
                ErrorCode::TxnAlreadyCommitted,
                "prepared transaction already committed",
            ),
            Some(TxnPhase::Aborted) => CoreError::new(
                ErrorCode::TxnAlreadyAborted,
                "prepared transaction already aborted",
            ),
            _ => CoreError::new(ErrorCode::TxnInDoubt, "no prepared batch for this token"),
        }
    }

    pub fn digest(&self) -> carolina_core::hash::Hash256 {
        let mut bytes = Vec::new();
        for (k, vs) in &self.versions {
            if let Some(v) = vs.iter().rev().find(|v| v.seq <= self.durable_seq) {
                if let Some(val) = &v.value {
                    bytes.extend_from_slice(&(k.len() as u32).to_le_bytes());
                    bytes.extend_from_slice(k);
                    bytes.extend_from_slice(&(val.len() as u32).to_le_bytes());
                    bytes.extend_from_slice(val);
                }
            }
        }
        carolina_core::hash::sha256(&bytes)
    }
}

impl DurableStorageKernel for MemKernel {
    fn capabilities(&self) -> KernelCapabilities {
        KernelCapabilities {
            durable_prepare: true,
            atomic_metadata_and_result: true,
            recoverable_semantic_records: true,
            registered_snapshots: true,
            retained_references: true,
            local_crash_durability: false,
        }
    }
    fn readiness(&self) -> Readiness {
        if self.prepared.is_empty() {
            Readiness::Ready
        } else {
            Readiness::WaitingProtocolReconciliation
        }
    }
    fn snapshot(&mut self) -> LocalSnapshot {
        LocalSnapshot {
            epoch: StorageEpoch(1),
            visible_seq: LocalCommitSeq(self.durable_seq),
        }
    }
    fn release_snapshot(&mut self, _snap: LocalSnapshot) {}
    fn get(&mut self, key: &LogicalKey, snap: LocalSnapshot) -> CoreResult<Option<Vec<u8>>> {
        Ok(self
            .newest_visible(&key.0, snap.visible_seq.0)
            .and_then(|v| v.value.clone()))
    }
    fn scan(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        snap: LocalSnapshot,
        limit: usize,
    ) -> CoreResult<Vec<(LogicalKey, Vec<u8>)>> {
        let mut out = Vec::new();
        for (k, _) in self.versions.range(start.to_vec()..) {
            if let Some(e) = end {
                if k.as_slice() >= e {
                    break;
                }
            }
            if let Some(v) = self
                .newest_visible(k, snap.visible_seq.0)
                .and_then(|v| v.value.clone())
            {
                out.push((LogicalKey(k.clone()), v));
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }
    fn commit(&mut self, batch: CompiledBatch) -> CoreResult<CommitResult> {
        batch.verify()?;
        match self.statuses.get(&batch.txn_id).map(|s| s.0.phase) {
            Some(TxnPhase::Aborted) => {
                return Err(CoreError::new(
                    ErrorCode::TxnAlreadyAborted,
                    "duplicate commit of an aborted transaction refused (SPEC-002 §69)",
                ))
            }
            Some(TxnPhase::Installed) | Some(TxnPhase::Terminal) => {
                return Err(CoreError::new(
                    ErrorCode::TxnAlreadyCommitted,
                    "duplicate commit of a decided transaction refused (SPEC-002 §69)",
                ))
            }
            _ => {}
        }
        self.validate(&batch.protocol_mutations)?;
        let seq = self.next_seq;
        self.next_seq += 1;
        self.install(&batch, seq);
        if self.sync_each_commit {
            self.sync();
        }
        self.commits += 1;
        Ok(CommitResult {
            txn_id: Some(batch.txn_id),
            local_version: VersionStamp {
                epoch: StorageEpoch(1),
                seq: LocalCommitSeq(seq),
            },
            journal_lsn: JournalLsn(seq),
            local_durability: DurabilityState::Unsafe,
            stored_evidence: vec![],
        })
    }
    fn commit_protocol(&mut self, batch: ProtocolOnlyBatch) -> CoreResult<CommitResult> {
        let pms: Vec<ProtocolMutation> = batch
            .writes
            .iter()
            .map(|w| ProtocolMutation::PutRecord(w.clone()))
            .collect();
        self.validate(&pms)?;
        let seq = self.next_seq;
        self.next_seq += 1;
        let fake = CompiledBatch {
            batch_version: 1,
            txn_id: TxnId::default(),
            request_key: RequestKey::default(),
            request_hash: RequestHash::default(),
            operation: OperationRef::default(),
            operation_hash: OperationHash::default(),
            contract_hash: ContractHash::default(),
            schema_hash: SchemaHash::default(),
            plan: PlanRef::default(),
            idc_bindings: vec![],
            consistency_class: ConsistencyClass::C0Local,
            origin: None,
            semantic_evidence: vec![],
            captured_inputs: vec![],
            mutations: vec![],
            protocol_mutations: pms,
            terminal_outcome: None,
            semantic_digest: SemanticDigest::default(),
        };
        self.install(&fake, seq);
        if self.sync_each_commit {
            self.sync();
        }
        Ok(CommitResult {
            txn_id: None,
            local_version: VersionStamp {
                epoch: StorageEpoch(1),
                seq: LocalCommitSeq(seq),
            },
            journal_lsn: JournalLsn(seq),
            local_durability: DurabilityState::Unsafe,
            stored_evidence: vec![],
        })
    }
    fn prepare(&mut self, batch: CompiledBatch) -> CoreResult<PreparedToken> {
        batch.verify()?;
        self.validate(&batch.protocol_mutations)?;
        let digest = batch.semantic_digest;
        let txn = batch.txn_id;
        self.prepared.insert(txn, (batch, false));
        if self.sync_each_commit {
            self.sync();
        }
        Ok(PreparedToken {
            txn_id: txn,
            lsn: JournalLsn(self.next_seq),
            digest,
        })
    }
    fn commit_prepared(
        &mut self,
        token: PreparedToken,
        decision: CommitDecision,
    ) -> CoreResult<CommitResult> {
        let (batch, _) = match self.prepared.remove(&token.txn_id) {
            Some(p) => p,
            None => return Err(self.decided_error(token.txn_id)),
        };
        if batch.semantic_digest != token.digest {
            self.prepared.insert(token.txn_id, (batch, false));
            return Err(CoreError::new(
                ErrorCode::IdentityMismatch,
                "prepared token digest mismatch",
            ));
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.install(&batch, seq);
        self.record_decision(&batch, &decision.decision_ref, false);
        if self.sync_each_commit {
            self.sync();
        }
        Ok(CommitResult {
            txn_id: Some(token.txn_id),
            local_version: VersionStamp {
                epoch: StorageEpoch(1),
                seq: LocalCommitSeq(seq),
            },
            journal_lsn: JournalLsn(seq),
            local_durability: DurabilityState::Unsafe,
            stored_evidence: vec![],
        })
    }
    fn abort_prepared(&mut self, token: PreparedToken, decision: AbortDecision) -> CoreResult<()> {
        let (batch, _) = match self.prepared.remove(&token.txn_id) {
            Some(p) => p,
            None => return Err(self.decided_error(token.txn_id)),
        };
        if batch.semantic_digest != token.digest {
            self.prepared.insert(token.txn_id, (batch, false));
            return Err(CoreError::new(
                ErrorCode::IdentityMismatch,
                "prepared token digest mismatch",
            ));
        }
        self.next_seq += 1;
        self.record_decision(&batch, &decision.decision_ref, true);
        if self.sync_each_commit {
            self.sync();
        }
        Ok(())
    }
    fn in_doubt(&self) -> Vec<PreparedToken> {
        self.prepared
            .iter()
            .map(|(t, (b, _))| PreparedToken {
                txn_id: *t,
                lsn: JournalLsn(0),
                digest: b.semantic_digest,
            })
            .collect()
    }
    fn resolve(&mut self, request: &RequestKey) -> CoreResult<LocalRequestEvidence> {
        let key = request_binding_record_key(request).logical_key().0;
        let binding = match self.records.get(&key) {
            Some((_, rec, _)) => Some(RequestBindingV1::decode(
                &rec.canonical_payload,
                &Limits::v1(),
            )?),
            None => None,
        };
        let (status, in_doubt) = match &binding {
            Some(b) => (
                self.statuses.get(&b.txn_id).map(|s| s.0.clone()),
                self.prepared.contains_key(&b.txn_id),
            ),
            None => (None, false),
        };
        Ok(LocalRequestEvidence {
            binding,
            status,
            in_doubt,
        })
    }
    fn read_protocol_record(
        &mut self,
        key: &ProtocolRecordKey,
    ) -> CoreResult<Option<(RecordRevision, VersionedProtocolRecord)>> {
        Ok(self
            .records
            .get(&key.logical_key().0)
            .map(|(r, rec, _)| (*r, rec.clone())))
    }
    fn txn_status(&mut self, txn: TxnId) -> CoreResult<Option<TxnStatusRecord>> {
        Ok(self.statuses.get(&txn).map(|s| s.0.clone()))
    }
    fn checkpoint(&mut self) -> CoreResult<CheckpointInfo> {
        Ok(CheckpointInfo {
            checkpoint_lsn: JournalLsn(self.durable_seq),
            checkpoint_commit_seq: LocalCommitSeq(self.durable_seq),
            root_page_id: 0,
            manifest_generation: 0,
            pages_written: 0,
        })
    }
    fn verify(&mut self, _mode: VerifyMode) -> CoreResult<VerifyReport> {
        Ok(VerifyReport {
            entries: self.versions.values().map(|v| v.len() as u64).sum(),
            prepared_in_doubt: self.prepared.len() as u64,
            ..Default::default()
        })
    }
}
