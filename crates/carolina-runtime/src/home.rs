//! Local RequestHome (SPEC-012 §3): durable `RequestKey -> TxnId` mapping with BindIfAbsent.
//!
//! `TxnId = RequestHomeId || RequestHomeEpoch || RequestAllocationSeq`. The epoch advances on every
//! open of the home, so allocation sequences never repeat across restarts without scanning old
//! bindings. The binding record is durable (journal barrier) before any execution is dispatched;
//! a second request under the same key with different content is an identity conflict, never a
//! second execution.

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::*;
use carolina_core::limits::Limits;
use carolina_storage::batch::{
    ExpectedRecordRevision, ProtocolOnlyBatch, ProtocolRecordKey, ProtocolRecordWrite,
    VersionedProtocolRecord,
};
use carolina_storage::kernel::{request_binding_record_key, DurableStorageKernel};
use carolina_wire::records::{BindingState, RequestBindingV1};
use carolina_wire::registry::CanonicalRecord;

pub const HOME_STATE_KIND: &str = "request_home_state";

/// Durable state of one RequestHome (one record per home id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHomeStateV1 {
    pub home_id: RequestHomeId,
    pub epoch: RequestHomeEpoch,
    /// Highest sequence allocated in `epoch` at the time the record was written (advisory: bindings are authoritative).
    pub allocated_hint: RequestAllocationSeq,
}

impl Canonical for RequestHomeStateV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("allocated_hint", &self.allocated_hint)
            .fc("epoch", &self.epoch)
            .fc("home_id", &self.home_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["allocated_hint", "epoch", "home_id"])?;
        Ok(RequestHomeStateV1 {
            home_id: RequestHomeId::from_canon(v.field("home_id")?)?,
            epoch: RequestHomeEpoch::from_canon(v.field("epoch")?)?,
            allocated_hint: RequestAllocationSeq::from_canon(v.field("allocated_hint")?)?,
        })
    }
}

pub fn home_state_key(home: RequestHomeId) -> ProtocolRecordKey {
    ProtocolRecordKey {
        record_kind: HOME_STATE_KIND.into(),
        scope_key: home.0.to_vec(),
        record_id: vec![],
    }
}

fn versioned<T: Canonical>(kind: &str, body: &T) -> CoreResult<VersionedProtocolRecord> {
    let rec = CanonicalRecord::wrap(kind, body)?;
    Ok(VersionedProtocolRecord {
        record_kind: rec.record_kind,
        record_version: rec.record_version,
        canonical_payload: body.encode(),
    })
}

#[derive(Debug)]
pub struct LocalRequestHome {
    pub home_id: RequestHomeId,
    pub epoch: RequestHomeEpoch,
    next_seq: u64,
    /// Storage revision of the `request_home_state` record (CAS base for allocations).
    state_rev: RecordRevision,
}

impl LocalRequestHome {
    /// Open (or create) the home: bump the epoch durably before allocating anything.
    pub fn open(kernel: &mut dyn DurableStorageKernel, home_id: RequestHomeId) -> CoreResult<Self> {
        let key = home_state_key(home_id);
        let (expected, epoch) = match kernel.read_protocol_record(&key)? {
            Some((rev, rec)) => {
                let st = RequestHomeStateV1::decode(&rec.canonical_payload, &Limits::v1())?;
                if st.home_id != home_id {
                    return Err(CoreError::new(
                        ErrorCode::Corruption,
                        "request home state belongs to another home",
                    ));
                }
                (
                    ExpectedRecordRevision::Exact(rev),
                    RequestHomeEpoch(st.epoch.0 + 1),
                )
            }
            None => (ExpectedRecordRevision::Absent, RequestHomeEpoch(1)),
        };
        let next = RequestHomeStateV1 {
            home_id,
            epoch,
            allocated_hint: RequestAllocationSeq(0),
        };
        kernel.commit_protocol(ProtocolOnlyBatch {
            internal_record_id: key.clone(),
            authorizing_evidence: vec![],
            writes: vec![ProtocolRecordWrite {
                key: key.clone(),
                expected,
                next: versioned(HOME_STATE_KIND, &next)?,
            }],
        })?;
        let state_rev = kernel
            .read_protocol_record(&key)?
            .map(|(r, _)| r)
            .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "home state vanished"))?;
        Ok(LocalRequestHome {
            home_id,
            epoch,
            next_seq: 1,
            state_rev,
        })
    }

    /// Open the home under an externally pinned epoch (a replicated/ordered home whose
    /// allocation sequence continues from the durable counter instead of advancing the epoch).
    /// A stored epoch different from `epoch` is refused: the allocation history would fork.
    pub fn open_pinned(
        kernel: &mut dyn DurableStorageKernel,
        home_id: RequestHomeId,
        epoch: RequestHomeEpoch,
    ) -> CoreResult<Self> {
        let key = home_state_key(home_id);
        match kernel.read_protocol_record(&key)? {
            Some((rev, rec)) => {
                let st = RequestHomeStateV1::decode(&rec.canonical_payload, &Limits::v1())?;
                if st.home_id != home_id {
                    return Err(CoreError::new(
                        ErrorCode::Corruption,
                        "request home state belongs to another home",
                    ));
                }
                if st.epoch != epoch {
                    return Err(CoreError::new(
                        ErrorCode::StaleEpoch,
                        format!(
                            "home epoch {} on disk differs from pinned epoch {}",
                            st.epoch.0, epoch.0
                        ),
                    ));
                }
                Ok(LocalRequestHome {
                    home_id,
                    epoch,
                    next_seq: st.allocated_hint.0 + 1,
                    state_rev: rev,
                })
            }
            None => {
                let next = RequestHomeStateV1 {
                    home_id,
                    epoch,
                    allocated_hint: RequestAllocationSeq(0),
                };
                kernel.commit_protocol(ProtocolOnlyBatch {
                    internal_record_id: key.clone(),
                    authorizing_evidence: vec![],
                    writes: vec![ProtocolRecordWrite {
                        key: key.clone(),
                        expected: ExpectedRecordRevision::Absent,
                        next: versioned(HOME_STATE_KIND, &next)?,
                    }],
                })?;
                let state_rev = kernel
                    .read_protocol_record(&key)?
                    .map(|(r, _)| r)
                    .ok_or_else(|| CoreError::new(ErrorCode::Corruption, "home state vanished"))?;
                Ok(LocalRequestHome {
                    home_id,
                    epoch,
                    next_seq: 1,
                    state_rev,
                })
            }
        }
    }

    /// Read the current binding of `key`, if any, with its storage revision.
    pub fn lookup(
        kernel: &mut dyn DurableStorageKernel,
        key: &RequestKey,
    ) -> CoreResult<Option<(RecordRevision, RequestBindingV1)>> {
        match kernel.read_protocol_record(&request_binding_record_key(key))? {
            Some((rev, rec)) => {
                let b = RequestBindingV1::decode(&rec.canonical_payload, &Limits::v1())?;
                b.verify()?;
                Ok(Some((rev, b)))
            }
            None => Ok(None),
        }
    }

    /// BindIfAbsent (SPEC-012 §3): returns the existing binding for the key, or allocates one
    /// durably. Different content under the same key is `IdentityConflict`.
    pub fn bind_if_absent(
        &mut self,
        kernel: &mut dyn DurableStorageKernel,
        key: &RequestKey,
        request_hash: RequestHash,
        admitted_plan: PlanRef,
    ) -> CoreResult<(RecordRevision, RequestBindingV1)> {
        if let Some((rev, b)) = Self::lookup(kernel, key)? {
            if b.request_hash != request_hash {
                return Err(CoreError::new(
                    ErrorCode::IdentityConflict,
                    "request key already bound to different content",
                ));
            }
            return Ok((rev, b));
        }
        let seq = RequestAllocationSeq(self.next_seq);
        let binding = RequestBindingV1 {
            request_key: *key,
            request_hash,
            txn_id: TxnId::new(self.home_id, self.epoch, seq),
            allocation_home: self.home_id,
            allocation_epoch: self.epoch,
            allocation_seq: seq,
            record_revision: RecordRevision(1),
            state: BindingState::Admitted,
            admitted_plan: Some(admitted_plan),
            decision_authority: None,
            terminal_receipt: None,
            tombstone: None,
        };
        let rkey = request_binding_record_key(key);
        let hkey = home_state_key(self.home_id);
        let counter = RequestHomeStateV1 {
            home_id: self.home_id,
            epoch: self.epoch,
            allocated_hint: seq,
        };
        let result = kernel.commit_protocol(ProtocolOnlyBatch {
            internal_record_id: rkey.clone(),
            authorizing_evidence: vec![],
            writes: vec![
                ProtocolRecordWrite {
                    key: rkey,
                    expected: ExpectedRecordRevision::Absent,
                    next: versioned("request_binding", &binding)?,
                },
                ProtocolRecordWrite {
                    key: hkey,
                    expected: ExpectedRecordRevision::Exact(self.state_rev),
                    next: versioned(HOME_STATE_KIND, &counter)?,
                },
            ],
        });
        match result {
            Ok(_) => {
                self.next_seq += 1;
                self.state_rev = RecordRevision(self.state_rev.0 + 1);
                Ok((RecordRevision(1), binding))
            }
            Err(e) if e.code == ErrorCode::Conflict => {
                // lost a race with an identical key: read what won
                match Self::lookup(kernel, key)? {
                    Some((rev, b)) if b.request_hash == request_hash => Ok((rev, b)),
                    Some(_) => Err(CoreError::new(
                        ErrorCode::IdentityConflict,
                        "request key already bound to different content",
                    )),
                    None => Err(e),
                }
            }
            Err(e) => {
                // the allocation may or may not be durable: never reuse the sequence
                self.next_seq += 1;
                Err(e)
            }
        }
    }
}
