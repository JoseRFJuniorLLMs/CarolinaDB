//! Kernel behavior: commit/recovery (P1–P3, P6), protocol CAS, prepared transactions (P9),
//! torn tail versus mid-log corruption (SPEC-002 §176), stale plan admission (§179).

use std::sync::Arc;

use carolina_core::error::ErrorCode;
use carolina_core::ids::*;
use carolina_storage::batch::*;
use carolina_storage::io::NoFaults;
use carolina_storage::journal::DurabilityMode;
use carolina_storage::kernel::*;
use carolina_storage::testutil::*;

fn opts() -> StoreOptions {
    StoreOptions {
        durability: DurabilityMode::Sync,
        segment_bytes: 256 * 1024,
        pool_frames: 64,
        faults: Arc::new(NoFaults),
        ..Default::default()
    }
}

fn status_write(
    seed: u64,
    phase: TxnPhase,
    expected: ExpectedRecordRevision,
    revision: u64,
) -> ProtocolMutation {
    let r = receipt(seed, carolina_wire::records::Outcome::Committed, &[1]);
    ProtocolMutation::SetTxnStatus(TxnStatusTransition {
        txn_id: r.txn_id,
        request_key: r.request_key,
        request_hash: r.request_hash,
        expected_revision: expected,
        next: TxnStatusRecord {
            revision: RecordRevision(revision),
            phase,
            plan: r.plan,
            idc_bindings: r.idc_bindings.clone(),
            prepared_digest: None,
            decision_ref: None,
            accepted_result: None,
            terminal_outcome: if phase == TxnPhase::Terminal {
                Some(TerminalOutcome::Committed(r.clone()))
            } else {
                None
            },
        },
    })
}

#[test]
fn commit_then_reopen_recovers_everything() {
    let dir = temp_dir("kernel-commit");
    let mut digest_before;
    {
        let mut s = Store::create(&dir, opts()).unwrap();
        for i in 1..=200u64 {
            let b = batch(
                i,
                &[
                    (user_key(1, i), vec![i as u8; (i % 50) as usize + 1]),
                    (user_key(2, i % 10), i.to_le_bytes().to_vec()),
                ],
                &[],
                true,
            );
            let r = s.commit(b).unwrap();
            assert_eq!(r.local_version.seq, LocalCommitSeq(i));
        }
        // delete some
        let b = batch(201, &[], &[user_key(1, 5), user_key(1, 6)], true);
        s.commit(b).unwrap();
        let snap = s.snapshot();
        assert_eq!(s.get(&user_key(1, 5), snap).unwrap(), None);
        assert_eq!(s.get(&user_key(1, 7), snap).unwrap(), Some(vec![7u8; 8]));
        assert_eq!(
            s.get(&user_key(2, 3), snap).unwrap(),
            Some(193u64.to_le_bytes().to_vec())
        );
        digest_before = state_digest(&mut s).unwrap();
        // checkpoint in the middle, then more commits after it
        s.checkpoint().unwrap();
        let b = batch(
            202,
            &[(user_key(3, 1), b"after-checkpoint".to_vec())],
            &[],
            true,
        );
        s.commit(b).unwrap();
        digest_before = state_digest(&mut s)
            .unwrap()
            .min(digest_before)
            .max(state_digest(&mut s).unwrap());
    }
    // reopen twice: identical logical state (P1, P6)
    for _ in 0..2 {
        let mut s = Store::open(&dir, opts()).unwrap();
        assert_eq!(s.readiness(), Readiness::Ready);
        assert_eq!(state_digest(&mut s).unwrap(), digest_before);
        let snap = s.snapshot();
        assert_eq!(
            s.get(&user_key(3, 1), snap).unwrap(),
            Some(b"after-checkpoint".to_vec())
        );
        assert_eq!(s.get(&user_key(1, 5), snap).unwrap(), None);
        // terminal status and receipt recovered
        let st = s.txn_status(txn(7)).unwrap().unwrap();
        assert_eq!(st.phase, TxnPhase::Terminal);
        assert_eq!(
            st.terminal_outcome.unwrap().receipt().exact_result_bytes,
            7u64.to_le_bytes().to_vec()
        );
        s.verify(VerifyMode::Full).unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn protocol_cas_and_status_transitions() {
    let dir = temp_dir("kernel-cas");
    let mut s = Store::create(&dir, opts()).unwrap();
    let key = ProtocolRecordKey {
        record_kind: "authority_grant".into(),
        scope_key: vec![1],
        record_id: vec![2],
    };
    let rec = |n: u8| VersionedProtocolRecord {
        record_kind: "authority_grant".into(),
        record_version: 1,
        canonical_payload: format!("{{\"v\":\"{n}\"}}").into_bytes(),
    };
    let mut b = batch(1, &[(user_key(1, 1), vec![1])], &[], false);
    b.protocol_mutations = vec![ProtocolMutation::PutRecord(ProtocolRecordWrite {
        key: key.clone(),
        expected: ExpectedRecordRevision::Absent,
        next: rec(1),
    })];
    let b = b.seal();
    s.commit(b).unwrap();
    assert_eq!(
        s.read_protocol_record(&key).unwrap().unwrap().0,
        RecordRevision(1)
    );
    // wrong expected revision → Conflict, nothing persisted
    let mut b2 = batch(2, &[(user_key(1, 2), vec![2])], &[], false);
    b2.protocol_mutations = vec![ProtocolMutation::PutRecord(ProtocolRecordWrite {
        key: key.clone(),
        expected: ExpectedRecordRevision::Absent,
        next: rec(2),
    })];
    let err = s.commit(b2.seal()).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    let snap = s.snapshot();
    assert_eq!(
        s.get(&user_key(1, 2), snap).unwrap(),
        None,
        "failed CAS must not persist the business prefix"
    );
    // exact revision advances
    let mut b3 = batch(3, &[(user_key(1, 3), vec![3])], &[], false);
    b3.protocol_mutations = vec![ProtocolMutation::PutRecord(ProtocolRecordWrite {
        key: key.clone(),
        expected: ExpectedRecordRevision::Exact(RecordRevision(1)),
        next: rec(2),
    })];
    s.commit(b3.seal()).unwrap();
    let (rev, r) = s.read_protocol_record(&key).unwrap().unwrap();
    assert_eq!(rev, RecordRevision(2));
    assert_eq!(r, rec(2));
    // unregistered record kind is refused
    let mut b4 = batch(4, &[], &[], false);
    b4.protocol_mutations = vec![ProtocolMutation::PutRecord(ProtocolRecordWrite {
        key: ProtocolRecordKey {
            record_kind: "bogus".into(),
            scope_key: vec![],
            record_id: vec![],
        },
        expected: ExpectedRecordRevision::Absent,
        next: VersionedProtocolRecord {
            record_kind: "bogus".into(),
            record_version: 1,
            canonical_payload: b"{}".to_vec(),
        },
    })];
    assert_eq!(
        s.commit(b4.seal()).unwrap_err().code,
        ErrorCode::UnsupportedCodec
    );
    // txn status: BOUND(1) -> ADMITTED(2) -> TERMINAL(3); a rewind to BOUND is refused; retry of identical is a no-op success
    let mut b5 = batch(5, &[], &[], false);
    b5.protocol_mutations = vec![status_write(
        5,
        TxnPhase::Bound,
        ExpectedRecordRevision::Absent,
        1,
    )];
    s.commit(b5.seal()).unwrap();
    let mut b6 = batch(5, &[], &[], false);
    b6.protocol_mutations = vec![status_write(
        5,
        TxnPhase::Admitted,
        ExpectedRecordRevision::Exact(RecordRevision(1)),
        2,
    )];
    s.commit(b6.seal()).unwrap();
    let mut b7 = batch(5, &[], &[], false);
    b7.protocol_mutations = vec![status_write(
        5,
        TxnPhase::Bound,
        ExpectedRecordRevision::Exact(RecordRevision(2)),
        3,
    )];
    assert_eq!(s.commit(b7.seal()).unwrap_err().code, ErrorCode::Conflict);
    let mut b8 = batch(5, &[], &[], false);
    b8.protocol_mutations = vec![status_write(
        5,
        TxnPhase::Terminal,
        ExpectedRecordRevision::Exact(RecordRevision(2)),
        3,
    )];
    s.commit(b8.seal()).unwrap();
    assert_eq!(
        s.txn_status(txn(5)).unwrap().unwrap().phase,
        TxnPhase::Terminal
    );
    // business mutation into a reserved namespace is refused
    let reserved = LogicalKey(
        carolina_core::keycodec::KeyWriter::new()
            .namespace(carolina_core::keycodec::Namespace::TxnStatus)
            .u64(1)
            .finish(),
    );
    let bad = batch(9, &[(reserved, vec![1])], &[], false);
    assert_eq!(s.commit(bad).unwrap_err().code, ErrorCode::ReadOnly);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prepared_transactions_stay_invisible_until_decided() {
    carolina_storage::campaign::prepared_invisibility().unwrap();
}

#[test]
fn torn_tail_is_truncated_but_mid_log_corruption_fails_closed() {
    carolina_storage::campaign::torn_tail_and_mid_log_corruption().unwrap();
}

#[test]
fn stale_plan_and_schema_are_refused_before_persistence() {
    carolina_storage::campaign::stale_admission_is_refused().unwrap();
}

#[test]
fn second_writer_cannot_open_the_same_directory() {
    let dir = temp_dir("kernel-lock");
    let a = Store::create(&dir, opts()).unwrap();
    let err = Store::open(&dir, opts())
        .err()
        .expect("second writer must fail (SPEC-002 §78)");
    assert_eq!(err.code, ErrorCode::Conflict);
    drop(a);
    // the lock is released with the store: a later writer opens normally
    let _b = Store::open(&dir, opts()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// SPEC-002 §69: a decided transaction is never installed twice; the duplicate is a typed refusal.
#[test]
fn duplicate_commit_of_a_decided_transaction_is_refused() {
    let dir = temp_dir("kernel-dup-commit");
    let mut s = Store::create(&dir, opts()).unwrap();
    let b = batch(9, &[(user_key(1, 1), b"once".to_vec())], &[], true);
    s.commit(b.clone()).unwrap();
    let seq_after_first = s.durable_commit_seq();
    let err = s.commit(b.clone()).unwrap_err();
    assert_eq!(err.code, ErrorCode::TxnAlreadyCommitted);
    // a different batch under the same (decided) txn id is refused as well
    let b2 = batch(9, &[(user_key(1, 2), b"twice".to_vec())], &[], false);
    assert_eq!(
        s.commit(b2).unwrap_err().code,
        ErrorCode::TxnAlreadyCommitted
    );
    assert_eq!(s.durable_commit_seq(), seq_after_first);
    let snap = s.snapshot();
    assert_eq!(s.get(&user_key(1, 2), snap).unwrap(), None);
    // the refusal is not journaled: recovery sees exactly one install
    drop(s);
    let mut s = Store::open(&dir, opts()).unwrap();
    assert_eq!(s.durable_commit_seq(), seq_after_first);
    assert_eq!(s.txn_status(txn(9)).unwrap().unwrap().phase, TxnPhase::Terminal);
    let mut m = carolina_storage::memkernel::MemKernel::new();
    m.commit(b.clone()).unwrap();
    assert_eq!(m.commit(b).unwrap_err().code, ErrorCode::TxnAlreadyCommitted);
    let _ = std::fs::remove_dir_all(&dir);
}

/// SPEC-002 §21: oversized keys/values are refused before anything is journaled, so recovery
/// after the refusal replays nothing and the store stays verifiable.
#[test]
fn oversized_key_and_value_are_refused_before_persistence() {
    use carolina_storage::format::{MAX_KEY_LEN, MAX_VALUE_LEN};
    let dir = temp_dir("kernel-oversized");
    let mut s = Store::create(&dir, opts()).unwrap();
    let big_value = batch(1, &[(user_key(1, 1), vec![7u8; MAX_VALUE_LEN + 1])], &[], true);
    assert_eq!(s.commit(big_value).unwrap_err().code, ErrorCode::ValueTooLarge);
    let big_key = LogicalKey(vec![1u8; MAX_KEY_LEN + 1]);
    let bk = batch(2, &[(big_key, b"v".to_vec())], &[], true);
    assert_eq!(s.commit(bk).unwrap_err().code, ErrorCode::KeyTooLarge);
    assert_eq!(s.durable_commit_seq(), LocalCommitSeq(0));
    // the largest accepted value round-trips
    let ok = batch(3, &[(user_key(1, 1), vec![7u8; MAX_VALUE_LEN])], &[], true);
    s.commit(ok).unwrap();
    drop(s);
    let mut s = Store::open(&dir, opts()).unwrap();
    assert_eq!(s.durable_commit_seq(), LocalCommitSeq(1));
    let snap = s.snapshot();
    assert_eq!(
        s.get(&user_key(1, 1), snap).unwrap().map(|v| v.len()),
        Some(MAX_VALUE_LEN)
    );
    s.verify(VerifyMode::Full).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// SPEC-002 §57: the buffer pool refuses to write a dirty page whose LSN is not yet durable.
#[test]
fn wal_before_page_refuses_an_undurable_flush() {
    use carolina_storage::buffer::BufferPool;
    use carolina_storage::format::{PageType, PAGE_SIZE};
    use carolina_storage::io::FilePageIo;
    let dir = temp_dir("kernel-wal-before-page");
    std::fs::create_dir_all(&dir).unwrap();
    let io = FilePageIo::open(&dir.join("data.astr"), PAGE_SIZE, Arc::new(NoFaults)).unwrap();
    let mut pool = BufferPool::new(4);
    pool.insert_new(&io, 1, PageType::Leaf, 10, 0).unwrap();
    let err = pool.flush_page(&io, 1, 9).unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.contains("WAL-before-page"));
    pool.flush_page(&io, 1, 10).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn memkernel_matches_store_on_random_workload() {
    carolina_storage::campaign::kernel_differential(11, 300).unwrap();
}
