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
    assert_eq!(
        s.txn_status(txn(9)).unwrap().unwrap().phase,
        TxnPhase::Terminal
    );
    let mut m = carolina_storage::memkernel::MemKernel::new();
    m.commit(b.clone()).unwrap();
    assert_eq!(
        m.commit(b).unwrap_err().code,
        ErrorCode::TxnAlreadyCommitted
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// SPEC-002 §21: oversized keys/values are refused before anything is journaled, so recovery
/// after the refusal replays nothing and the store stays verifiable.
#[test]
fn oversized_key_and_value_are_refused_before_persistence() {
    use carolina_storage::format::{MAX_KEY_LEN, MAX_VALUE_LEN};
    let dir = temp_dir("kernel-oversized");
    let mut s = Store::create(&dir, opts()).unwrap();
    let big_value = batch(
        1,
        &[(user_key(1, 1), vec![7u8; MAX_VALUE_LEN + 1])],
        &[],
        true,
    );
    assert_eq!(
        s.commit(big_value).unwrap_err().code,
        ErrorCode::ValueTooLarge
    );
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

/// SPEC-002 §87–§91 (S10): a checkpoint reclaims MVCC versions that no reader can select again,
/// while every registered snapshot keeps reading exactly what it read before. The reclaimed
/// database recovers to the same logical state and passes a full structural verify.
#[test]
fn checkpoint_reclaims_invisible_versions_and_never_a_registered_snapshot() {
    let dir = temp_dir("kernel-mvcc-gc");
    let mut s = Store::create(&dir, opts()).unwrap();
    // 40 versions of one key plus a second key that is deleted at the end
    for i in 1..=40u64 {
        s.commit(batch(
            i,
            &[
                (user_key(1, 1), vec![i as u8]),
                (user_key(1, 2), vec![i as u8]),
            ],
            &[],
            true,
        ))
        .unwrap();
    }
    // a reader that registered at version 40 must keep seeing version 40 after the checkpoint
    let pinned = s.snapshot();
    for i in 41..=60u64 {
        s.commit(batch(i, &[(user_key(1, 1), vec![i as u8])], &[], true))
            .unwrap();
    }
    let before = state_digest(&mut s).unwrap();
    let pinned_value = s.get(&user_key(1, 1), pinned).unwrap();
    assert_eq!(pinned_value, Some(vec![40u8]));

    s.checkpoint().unwrap();
    let reclaimed_once = s.metrics.mvcc_versions_reclaimed_total;
    assert!(
        reclaimed_once > 0,
        "superseded versions below the horizon must be reclaimed"
    );
    // the pinned snapshot is untouched (P4) and the current state is unchanged
    assert_eq!(s.get(&user_key(1, 1), pinned).unwrap(), Some(vec![40u8]));
    assert_eq!(state_digest(&mut s).unwrap(), before);
    s.verify(VerifyMode::Full).unwrap();
    assert_eq!(s.metrics.oldest_snapshot_seq, 40);

    // releasing the snapshot moves the horizon forward: another checkpoint reclaims more
    s.release_snapshot(pinned);
    s.commit(batch(61, &[(user_key(1, 1), vec![61u8])], &[], true))
        .unwrap();
    s.checkpoint().unwrap();
    assert!(
        s.metrics.mvcc_versions_reclaimed_total > reclaimed_once,
        "a released snapshot must let the horizon advance"
    );
    assert_eq!(s.metrics.oldest_snapshot_seq, 61);
    let after = state_digest(&mut s).unwrap();
    s.verify(VerifyMode::Full).unwrap();

    // recovery reproduces the reclaimed state exactly
    drop(s);
    let mut s = Store::open(&dir, opts()).unwrap();
    assert_eq!(state_digest(&mut s).unwrap(), after);
    let snap = s.snapshot();
    assert_eq!(s.get(&user_key(1, 1), snap).unwrap(), Some(vec![61u8]));
    assert_eq!(s.get(&user_key(1, 2), snap).unwrap(), Some(vec![40u8]));
    s.verify(VerifyMode::Full).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// SPEC-002 §105: after a checkpoint the journal keeps only the segment that holds
/// `checkpoint_lsn` and everything after it; older segments are deleted and recovery still
/// reproduces the same state. A prepared transaction pins the journal below its frame.
#[test]
fn checkpoint_reclaims_journal_segments_but_prepared_work_pins_them() {
    let small = || StoreOptions {
        segment_bytes: 16 * 1024,
        ..opts()
    };
    let dir = temp_dir("kernel-journal-retention");
    let count = |d: &std::path::Path| {
        std::fs::read_dir(d.join("journal"))
            .map(|it| it.count())
            .unwrap_or(0)
    };
    let mut s = Store::create(&dir, small()).unwrap();
    for i in 1..=120u64 {
        s.commit(batch(i, &[(user_key(1, i), vec![i as u8; 200])], &[], true))
            .unwrap();
    }
    let segments_before = count(&dir);
    assert!(segments_before > 1, "the workload must roll segments");
    let digest = state_digest(&mut s).unwrap();
    s.checkpoint().unwrap();
    let segments_after = count(&dir);
    assert!(
        segments_after < segments_before,
        "a checkpoint must reclaim journal segments ({segments_before} → {segments_after})"
    );
    assert!(s.metrics.journal_segments_reclaimed_total > 0);
    assert!(s.metrics.journal_bytes_reclaimed_total > 0);
    assert!(s.metrics.journal_retained_bytes > 0);
    assert!(s.manifest().journal_segment > 1);
    drop(s);
    let mut s = Store::open(&dir, small()).unwrap();
    assert_eq!(state_digest(&mut s).unwrap(), digest);
    s.verify(VerifyMode::Full).unwrap();

    // a prepared transaction pins the journal: its frame must survive every checkpoint
    let _pinned_prepare = s
        .prepare(prepare_batch(
            500,
            &[(user_key(2, 1), b"prepared".to_vec())],
        ))
        .unwrap();
    for i in 121..=240u64 {
        s.commit(batch(i, &[(user_key(1, i), vec![i as u8; 200])], &[], true))
            .unwrap();
    }
    s.checkpoint().unwrap();
    let retained = s.manifest().journal_segment;
    drop(s);
    let mut s = Store::open(&dir, small()).unwrap();
    assert_eq!(
        s.in_doubt().len(),
        1,
        "the prepared transaction must still be recoverable after journal reclamation"
    );
    assert_eq!(s.manifest().journal_segment, retained);
    // deciding it releases the pin; the next checkpoint may reclaim further
    let token = s.in_doubt().into_iter().next().unwrap();
    assert_eq!(token.txn_id, txn(500));
    s.commit_prepared(
        token,
        CommitDecision {
            decision_ref: decision_ref(500),
        },
    )
    .unwrap();
    s.checkpoint().unwrap();
    let digest = state_digest(&mut s).unwrap();
    drop(s);
    let mut s = Store::open(&dir, small()).unwrap();
    assert_eq!(state_digest(&mut s).unwrap(), digest);
    let snap = s.snapshot();
    assert_eq!(
        s.get(&user_key(2, 1), snap).unwrap(),
        Some(b"prepared".to_vec())
    );
    s.verify(VerifyMode::Full).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// SPEC-002 §84/§105: a prepared transaction pins the journal, so `checkpoint_lsn` is clamped below
/// its frame while the persisted image already holds every later commit. Recovery therefore replays
/// commits that are *already* in the image — the redo path is explicitly idempotent by `(key, seq)`.
/// A replayed commit whose value overflows must not allocate the overflow chain before it discovers
/// the duplicate: the abandoned chain is unreachable from the root, so no checkpoint writes it and
/// no eviction reclaims it, the pool never becomes clean again, and journal retention — which only
/// advances when the image is complete — silently stops for the life of the store.
#[test]
fn replaying_a_committed_overflow_value_leaves_the_image_completable() {
    let dir = temp_dir("kernel-replay-overflow");
    let big = vec![0x5Au8; 16 * 1024];
    let mut o = opts();
    o.reclaim_at_checkpoint = true;
    {
        let mut s = Store::create(&dir, o.clone()).unwrap();
        // a prepared transaction pins the journal below every later commit
        s.prepare(prepare_batch(
            900,
            &[(user_key(9, 1), b"prepared".to_vec())],
        ))
        .unwrap();
        // commits after it, with values that need overflow chains
        for i in 1..=8u64 {
            s.commit(batch(i, &[(user_key(1, i), big.clone())], &[], true))
                .unwrap();
        }
        // the image holds all eight, but checkpoint_lsn stays below the prepared frame
        s.checkpoint().unwrap();
    }
    // recovery replays all eight commits against an image that already contains them
    let mut s = Store::open(&dir, o).unwrap();
    // the undecided transaction is in doubt, exactly as P9 requires
    assert_eq!(s.readiness(), Readiness::WaitingProtocolReconciliation);
    assert_eq!(
        s.metrics.recovery_replayed_batches, 8,
        "the clamped checkpoint must force the commits to be replayed"
    );
    let snap = s.snapshot();
    assert_eq!(s.get(&user_key(1, 3), snap).unwrap(), Some(big.clone()));
    s.release_snapshot(snap);
    s.verify(VerifyMode::Full).unwrap();

    // decide it so the store is writable again, then check that the replay left nothing
    // unreachable behind: a completable image is the precondition journal retention needs
    let in_doubt = s.in_doubt();
    assert_eq!(in_doubt.len(), 1);
    s.abort_prepared(
        in_doubt[0].clone(),
        AbortDecision {
            decision_ref: decision_ref(900),
        },
    )
    .unwrap();
    assert_eq!(s.readiness(), Readiness::Ready);
    s.commit(batch(50, &[(user_key(2, 1), b"after".to_vec())], &[], true))
        .unwrap();
    s.checkpoint().unwrap();
    assert_eq!(
        s.metrics.checkpoint_image_incomplete_total, 0,
        "the checkpoint could not write every dirty page: retention is stalled for good"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
