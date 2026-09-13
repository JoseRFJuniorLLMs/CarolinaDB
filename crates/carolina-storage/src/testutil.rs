//! Deterministic synthetic batches, receipts and directories for storage tests and simulations.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use carolina_core::canon::CanonValue;
use carolina_core::hash::Hash256;
use carolina_core::ids::*;
use carolina_core::keycodec::{KeyWriter, Namespace};
use carolina_wire::records::{AcceptedResultV1, FinalReceiptV1, Outcome, PolicyRef};

use crate::batch::*;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh temporary directory under the OS temp dir (removed by the caller or left for inspection).
pub fn temp_dir(label: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("carolina-{label}-{pid}-{nanos}-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn user_key(record: u64, key: u64) -> LogicalKey {
    LogicalKey(
        KeyWriter::new()
            .namespace(Namespace::User)
            .u64(record)
            .u64(key)
            .finish(),
    )
}

pub fn txn(seed: u64) -> TxnId {
    TxnId::new(
        RequestHomeId::derive("test-home"),
        RequestHomeEpoch(1),
        RequestAllocationSeq(seed),
    )
}

pub fn request_key(seed: u64) -> RequestKey {
    RequestKey {
        tenant_id: TenantId::derive("tenant"),
        request_namespace: RequestNamespace::derive("ns"),
        stable_request_id: StableRequestId::derive(&format!("req-{seed}")),
    }
}

pub fn plan_ref() -> PlanRef {
    PlanRef {
        plan_id: PlanId::derive("plan"),
        generation: PlanGeneration(1),
        hash: PlanHash(Hash256::ZERO),
    }
}

pub fn decision_ref(seed: u64) -> ProtocolRecordRef {
    ProtocolRecordRef {
        record_kind: "local_decision".into(),
        record_version: 1,
        record_key: seed.to_le_bytes().to_vec(),
        payload_hash: carolina_core::hash::sha256(&seed.to_le_bytes()),
    }
}

pub fn receipt(seed: u64, outcome: Outcome, result: &[u8]) -> FinalReceiptV1 {
    let ar = AcceptedResultV1::new(
        outcome,
        "canonical-value:1",
        Hash256::ZERO,
        result.to_vec(),
        CanonValue::obj().build(),
        None,
    );
    FinalReceiptV1 {
        receipt_version: 1,
        cluster_id: ClusterId::derive("cluster"),
        request_key: request_key(seed),
        request_hash: RequestHash(carolina_core::hash::sha256(&seed.to_le_bytes())),
        txn_id: txn(seed),
        operation: OperationRef {
            operation_id: OperationId(1),
            version: 1,
        },
        operation_hash: OperationHash(Hash256::ZERO),
        schema_hash: SchemaHash(Hash256::ZERO),
        contract_hash: ContractHash(Hash256::ZERO),
        plan: plan_ref(),
        idc_bindings: vec![IdcBinding {
            idc_id: IdcId::derive("idc"),
            idc_generation: IdcGeneration(1),
            authority_epoch: IdcAuthorityEpoch(1),
        }],
        origin_ids: vec![],
        outcome,
        result_codec: ar.result_codec.clone(),
        result_type_hash: ar.result_type_hash,
        exact_result_bytes: ar.exact_result_bytes.clone(),
        result_digest: ar.result_digest,
        commitments: ar.commitments.clone(),
        observation_token: None,
        durability_policy: PolicyRef {
            name: "local".into(),
            policy_hash: Hash256::ZERO,
        },
        durability_evidence: vec![],
        decision_ref: decision_ref(seed),
        completion_ref: None,
    }
}

/// A business batch: `puts` and `deletes` on user keys, plus a terminal status record and receipt.
pub fn batch(
    seed: u64,
    puts: &[(LogicalKey, Vec<u8>)],
    deletes: &[LogicalKey],
    with_terminal: bool,
) -> CompiledBatch {
    let mut mutations = Vec::new();
    for (k, v) in puts {
        mutations.push(StorageMutation::Put {
            key: k.clone(),
            value: v.clone(),
            semantic_meta: SemanticMeta::default(),
        });
    }
    for k in deletes {
        mutations.push(StorageMutation::Delete {
            key: k.clone(),
            semantic_meta: SemanticMeta::default(),
        });
    }
    let r = receipt(seed, Outcome::Committed, &seed.to_le_bytes());
    let mut protocol_mutations = Vec::new();
    let terminal_outcome = if with_terminal {
        protocol_mutations.push(ProtocolMutation::SetTxnStatus(TxnStatusTransition {
            txn_id: r.txn_id,
            request_key: r.request_key,
            request_hash: r.request_hash,
            expected_revision: ExpectedRecordRevision::Absent,
            next: TxnStatusRecord {
                revision: RecordRevision(1),
                phase: TxnPhase::Terminal,
                plan: r.plan,
                idc_bindings: r.idc_bindings.clone(),
                prepared_digest: None,
                decision_ref: Some(r.decision_ref.clone()),
                accepted_result: Some(r.accepted_result()),
                terminal_outcome: Some(TerminalOutcome::Committed(r.clone())),
            },
        }));
        Some(TerminalOutcome::Committed(r.clone()))
    } else {
        None
    };
    CompiledBatch {
        batch_version: BATCH_VERSION,
        txn_id: r.txn_id,
        request_key: r.request_key,
        request_hash: r.request_hash,
        operation: r.operation,
        operation_hash: r.operation_hash,
        contract_hash: r.contract_hash,
        schema_hash: r.schema_hash,
        plan: r.plan,
        idc_bindings: r.idc_bindings.clone(),
        consistency_class: ConsistencyClass::C0Local,
        origin: None,
        semantic_evidence: vec![],
        captured_inputs: vec![],
        mutations,
        protocol_mutations,
        terminal_outcome,
        semantic_digest: SemanticDigest::default(),
    }
    .seal()
}

/// A prepare batch (no terminal outcome; PREPARED status record).
pub fn prepare_batch(seed: u64, puts: &[(LogicalKey, Vec<u8>)]) -> CompiledBatch {
    let mut b = batch(seed, puts, &[], false);
    let r = receipt(seed, Outcome::Committed, &seed.to_le_bytes());
    b.protocol_mutations = vec![ProtocolMutation::SetTxnStatus(TxnStatusTransition {
        txn_id: r.txn_id,
        request_key: r.request_key,
        request_hash: r.request_hash,
        expected_revision: ExpectedRecordRevision::Absent,
        next: TxnStatusRecord {
            revision: RecordRevision(1),
            phase: TxnPhase::Prepared,
            plan: r.plan,
            idc_bindings: r.idc_bindings.clone(),
            prepared_digest: None,
            decision_ref: None,
            accepted_result: Some(r.accepted_result()),
            terminal_outcome: None,
        },
    })];
    b.seal()
}
