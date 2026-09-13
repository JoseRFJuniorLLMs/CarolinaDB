//! SPEC-014 §4 first end-to-end workload on the local slice (MVP-2).
//!
//! One inventory key, explicit receipt-returning reserve/release. Schedules: concurrent
//! reservation of the last unit, duplicate release, overflow, changed content under one key,
//! crash after allocation, crash after commit before reply, result eviction and retired namespace
//! after restore. Every check compares replies with durable state after restart.

use std::sync::Arc;

use carolina_core::canon::Canonical;
use carolina_core::ids::*;
use carolina_lang::fixtures::fixture_source;
use carolina_lang::types::Value;
use carolina_runtime::engine::{make_invoke, EngineOptions, LocalEngine};
use carolina_runtime::LocalCatalog;
use carolina_storage::io::{CrashAtNth, FaultPoint, NoFaults};
use carolina_storage::journal::DurabilityMode;
use carolina_storage::kernel::VerifyMode;
use carolina_storage::testutil::temp_dir;
use carolina_storage::StoreOptions;
use carolina_wire::records::*;

const ITEM: [u8; 16] = [0x11; 16];

fn catalog() -> Arc<LocalCatalog> {
    Arc::new(
        LocalCatalog::from_source(fixture_source("inventory_reserve_release").unwrap()).unwrap(),
    )
}

fn opts(faults: carolina_storage::io::Faults) -> EngineOptions {
    EngineOptions::local(StoreOptions {
        durability: DurabilityMode::Sync,
        pool_frames: 64,
        faults,
        auto_checkpoint_dirty_pages: 32,
        ..Default::default()
    })
}

fn no_faults() -> EngineOptions {
    opts(Arc::new(NoFaults))
}

fn uuid(b: u8) -> Value {
    Value::Uuid([b; 16])
}

fn seeded(dir: &std::path::Path, available: i64) -> LocalEngine {
    let mut e = LocalEngine::create(dir, catalog(), no_faults()).unwrap();
    e.load_rows(
        "seed",
        "Item",
        &[(
            Value::Uuid(ITEM),
            vec![
                ("id", Value::Uuid(ITEM)),
                ("available", Value::I64(available)),
                ("reserved", Value::I64(0)),
                ("total", Value::I64(available)),
            ],
        )],
    )
    .unwrap();
    e
}

fn reserve(e: &LocalEngine, req: &str, q: i64, rid: u8) -> InvokeV1 {
    make_invoke(
        &e.catalog,
        "tenant-a",
        req,
        "reserve",
        vec![Value::Uuid(ITEM), Value::I64(q), uuid(rid)],
    )
    .unwrap()
}

fn release(e: &LocalEngine, req: &str, q: i64, rid: u8) -> InvokeV1 {
    make_invoke(
        &e.catalog,
        "tenant-a",
        req,
        "release",
        vec![Value::Uuid(ITEM), Value::I64(q), uuid(rid)],
    )
    .unwrap()
}

fn item(e: &mut LocalEngine) -> (i64, i64, i64) {
    let row = e.read_row("Item", &Value::Uuid(ITEM)).unwrap().unwrap();
    let g = |n: &str| match row[n] {
        Value::I64(v) => v,
        _ => panic!("field {n}"),
    };
    (g("available"), g("reserved"), g("total"))
}

fn committed(r: &ClientReplyV1) -> &FinalReceiptV1 {
    match r {
        ClientReplyV1::Committed(r) => r,
        other => panic!("expected Committed, got {}", other.kind()),
    }
}

fn rejected(r: &ClientReplyV1) -> &FinalReceiptV1 {
    match r {
        ClientReplyV1::Rejected(r) => r,
        other => panic!("expected Rejected, got {}", other.kind()),
    }
}

fn resolve(e: &mut LocalEngine, inv: &InvokeV1) -> ResolveReplyV1 {
    e.resolve(&ResolveRequestV1 {
        request_key: inv.content.request_key,
        expected_request_hash: inv.request_hash,
    })
}

#[test]
fn reserve_release_receipts_are_exact_and_idempotent() {
    let dir = temp_dir("rt-basic");
    let mut e = seeded(&dir, 5);
    let inv = reserve(&e, "r1", 3, 1);
    let r1 = committed(&e.invoke(&inv)).clone();
    r1.verify().unwrap();
    assert_eq!(r1.outcome, Outcome::Committed);
    assert_eq!(item(&mut e), (2, 3, 5));
    assert_eq!(e.count_rows("Reservation").unwrap(), 1);
    // exact typed result: { item, quantity, reservation }
    let result =
        Value::decode(&r1.exact_result_bytes, &carolina_core::limits::Limits::v1()).unwrap();
    match result {
        Value::Struct(f) => {
            assert_eq!(f["quantity"], Value::I64(3));
            assert_eq!(f["reservation"], uuid(1));
        }
        other => panic!("{other:?}"),
    }
    // duplicate delivery: byte-identical receipt, no second execution
    let again = committed(&e.invoke(&inv)).clone();
    assert_eq!(again.encode(), r1.encode());
    assert_eq!(item(&mut e), (2, 3, 5));
    match resolve(&mut e, &inv) {
        ResolveReplyV1::Terminal(b) => assert_eq!(committed(&b).encode(), r1.encode()),
        other => panic!("{other:?}"),
    }
    // release, then duplicate release under a NEW key is a business rejection with a receipt
    let rel = release(&e, "rel1", 3, 1);
    let r2 = committed(&e.invoke(&rel)).clone();
    assert_eq!(
        r2.txn_id.seq(),
        RequestAllocationSeq(3),
        "seed, reserve, release"
    );
    assert_eq!(item(&mut e), (5, 0, 5));
    let rel2 = release(&e, "rel2", 3, 1);
    let rj = rejected(&e.invoke(&rel2)).clone();
    rj.verify().unwrap();
    assert_eq!(item(&mut e), (5, 0, 5));
    // the rejection is final and durable: resolve returns the same REJECTED receipt
    match resolve(&mut e, &rel2) {
        ResolveReplyV1::Terminal(b) => assert_eq!(rejected(&b).encode(), rj.encode()),
        other => panic!("{other:?}"),
    }
    // unknown request: absent at barrier, never invented
    let ghost = reserve(&e, "ghost", 1, 9);
    assert!(matches!(
        resolve(&mut e, &ghost),
        ResolveReplyV1::AbsentAtBarrier { .. }
    ));
    e.verify(VerifyMode::Full).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn last_unit_is_reserved_exactly_once() {
    let dir = temp_dir("rt-last-unit");
    let mut e = seeded(&dir, 1);
    let a = reserve(&e, "a", 1, 1);
    let b = reserve(&e, "b", 1, 2);
    let ra = e.invoke(&a);
    let rb = e.invoke(&b);
    let wins = matches!(ra, ClientReplyV1::Committed(_)) as u8
        + matches!(rb, ClientReplyV1::Committed(_)) as u8;
    assert_eq!(wins, 1, "exactly one reservation of the last unit");
    let loser = rejected(&rb);
    let body = carolina_core::canon::CanonValue::decode(
        &loser.exact_result_bytes,
        &carolina_core::limits::Limits::v1(),
    )
    .unwrap();
    // the rejection names the declared predicate that failed (ENSURE available >= 0 fires before
    // the closure invariants, SPEC-003 §7 ordering), not a generic error
    let stage = body.field("stage").unwrap().as_str().unwrap().to_string();
    assert!(
        stage == "Postcondition" || stage.starts_with("Invariant"),
        "{stage}"
    );
    assert_eq!(
        body.field("code").unwrap().as_str().unwrap(),
        "PostconditionRejected"
    );
    assert_eq!(item(&mut e), (0, 1, 1));
    assert_eq!(e.count_rows("Reservation").unwrap(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn overflow_is_a_final_rejection_receipt() {
    let dir = temp_dir("rt-overflow");
    let mut e = seeded(&dir, 5);
    let inv = make_invoke(
        &e.catalog,
        "tenant-a",
        "big",
        "supply",
        vec![Value::Uuid(ITEM), Value::I64(i64::MAX)],
    )
    .unwrap();
    let rj = rejected(&e.invoke(&inv)).clone();
    let body = carolina_core::canon::CanonValue::decode(
        &rj.exact_result_bytes,
        &carolina_core::limits::Limits::v1(),
    )
    .unwrap();
    assert!(body
        .field("code")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("Overflow"));
    assert_eq!(item(&mut e), (5, 0, 5));
    // same request again: same rejection bytes
    assert_eq!(rejected(&e.invoke(&inv)).encode(), rj.encode());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn changed_content_under_one_key_is_an_identity_mismatch() {
    let dir = temp_dir("rt-mismatch");
    let mut e = seeded(&dir, 5);
    let inv = reserve(&e, "same-key", 2, 1);
    let r1 = committed(&e.invoke(&inv)).clone();
    let other = reserve(&e, "same-key", 4, 2); // same RequestKey, different content
    assert!(matches!(
        e.invoke(&other),
        ClientReplyV1::RequestIdentityMismatch
    ));
    assert_eq!(item(&mut e), (3, 2, 5), "the second content never executed");
    // resolving with the wrong expected hash is also a mismatch; with the right one, the receipt
    assert!(matches!(
        resolve(&mut e, &other),
        ResolveReplyV1::Terminal(b) if matches!(*b, ClientReplyV1::RequestIdentityMismatch)
    ));
    match resolve(&mut e, &inv) {
        ResolveReplyV1::Terminal(b) => assert_eq!(committed(&b).encode(), r1.encode()),
        other => panic!("{other:?}"),
    }
    // a tampered client hash is refused before anything is bound
    let mut forged = reserve(&e, "forged", 1, 3);
    forged.request_hash = RequestHash(carolina_core::hash::sha256(b"forged"));
    assert!(matches!(
        e.invoke(&forged),
        ClientReplyV1::RequestIdentityMismatch
    ));
    assert!(matches!(
        resolve(&mut e, &forged),
        ResolveReplyV1::AbsentAtBarrier { .. }
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Crash right after the binding reached the journal (allocation durable, nothing executed).
#[test]
fn crash_after_allocation_reexecutes_under_the_same_txn_id() {
    let dir = temp_dir("rt-crash-alloc");
    {
        let _e = seeded(&dir, 5);
    }
    // occurrences of AfterFsyncBeforePublish after open: 1 = home epoch bump, 2 = binding
    let epoch_at_crash;
    {
        let mut e = LocalEngine::open(
            &dir,
            catalog(),
            opts(Arc::new(CrashAtNth::new(
                FaultPoint::AfterFsyncBeforePublish,
                2,
            ))),
        )
        .unwrap();
        epoch_at_crash = e.home_epoch();
        let inv = reserve(&e, "alloc", 2, 1);
        assert!(matches!(e.invoke(&inv), ClientReplyV1::OutcomeUnknown(_)));
    }
    let mut e = LocalEngine::open(&dir, catalog(), no_faults()).unwrap();
    assert_eq!(e.home_epoch(), RequestHomeEpoch(epoch_at_crash.0 + 1));
    let inv = reserve(&e, "alloc", 2, 1);
    match resolve(&mut e, &inv) {
        ResolveReplyV1::Pending { txn_id, .. } => {
            assert_eq!(txn_id.epoch(), epoch_at_crash, "binding survived the crash");
            assert_eq!(txn_id.seq(), RequestAllocationSeq(1));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(item(&mut e), (5, 0, 5), "nothing executed before the crash");
    let r = committed(&e.invoke(&inv)).clone();
    assert_eq!(
        r.txn_id.epoch(),
        epoch_at_crash,
        "re-execution keeps the bound TxnId"
    );
    assert_eq!(r.txn_id.seq(), RequestAllocationSeq(1));
    assert_eq!(item(&mut e), (3, 2, 5));
    assert_eq!(e.count_rows("Reservation").unwrap(), 1);
    // a new request in the new epoch gets a fresh, non-colliding TxnId
    let r2 = committed(&e.invoke(&reserve(&e, "after", 1, 2))).clone();
    assert_eq!(r2.txn_id.epoch(), e.home_epoch());
    assert_ne!(r2.txn_id, r.txn_id);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Crash after the decision reached the journal but before the reply (SPEC-014 §4).
#[test]
fn crash_after_commit_before_reply_resolves_to_the_identical_receipt() {
    let dir = temp_dir("rt-crash-commit");
    {
        let _e = seeded(&dir, 5);
    }
    {
        // 1 = home epoch bump, 2 = binding, 3 = the CompiledBatch commit
        let mut e = LocalEngine::open(
            &dir,
            catalog(),
            opts(Arc::new(CrashAtNth::new(
                FaultPoint::AfterFsyncBeforePublish,
                3,
            ))),
        )
        .unwrap();
        let inv = reserve(&e, "lost-reply", 2, 1);
        match e.invoke(&inv) {
            ClientReplyV1::OutcomeUnknown(h) => assert_eq!(h.request_key, inv.content.request_key),
            other => panic!("expected OutcomeUnknown, got {}", other.kind()),
        }
    }
    let mut e = LocalEngine::open(&dir, catalog(), no_faults()).unwrap();
    e.verify(VerifyMode::Full).unwrap();
    let inv = reserve(&e, "lost-reply", 2, 1);
    let resolved = match resolve(&mut e, &inv) {
        ResolveReplyV1::Terminal(b) => committed(&b).clone(),
        other => panic!("expected the durable receipt, got {other:?}"),
    };
    assert_eq!(item(&mut e), (3, 2, 5), "committed exactly once");
    assert_eq!(e.count_rows("Reservation").unwrap(), 1);
    // re-invoking the same request returns the same bytes; a second execution never happens
    let again = committed(&e.invoke(&inv)).clone();
    assert_eq!(again.encode(), resolved.encode());
    assert_eq!(item(&mut e), (3, 2, 5));
    // and the identical receipt survives another restart + checkpoint
    e.checkpoint().unwrap();
    drop(e);
    let mut e = LocalEngine::open(&dir, catalog(), no_faults()).unwrap();
    match resolve(&mut e, &inv) {
        ResolveReplyV1::Terminal(b) => assert_eq!(committed(&b).encode(), resolved.encode()),
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn result_eviction_and_namespace_retirement_survive_restart() {
    let dir = temp_dir("rt-evict");
    let mut e = seeded(&dir, 5);
    let inv = reserve(&e, "evicted", 1, 1);
    let r = committed(&e.invoke(&inv)).clone();
    let tomb = e.evict_result(&inv.content.request_key).unwrap();
    assert_eq!(tomb.receipt_digest, r.receipt_digest());
    assert_eq!(tomb.txn_id, r.txn_id);
    // the result is gone but the outcome is not: no re-execution, no invented receipt
    match e.invoke(&inv) {
        ClientReplyV1::ResultExpired(t) => assert_eq!(t, tomb),
        other => panic!("{}", other.kind()),
    }
    assert_eq!(item(&mut e), (4, 1, 5));
    let ns = inv.content.request_key.request_namespace;
    let tenant = inv.content.request_key.tenant_id;
    let retirement = e
        .retire_namespace(tenant, ns, "restore from backup")
        .unwrap();
    match e.invoke(&inv) {
        ClientReplyV1::IdentityExpired {
            namespace,
            retirement_ref,
        } => {
            assert_eq!(namespace, ns);
            assert_eq!(retirement_ref, retirement);
        }
        other => panic!("{}", other.kind()),
    }
    // even a brand-new key in the retired namespace is refused: identities are never reused
    assert!(matches!(
        e.invoke(&reserve(&e, "fresh-after-retire", 1, 7)),
        ClientReplyV1::IdentityExpired { .. }
    ));
    assert_eq!(item(&mut e), (4, 1, 5));
    drop(e);
    let mut e = LocalEngine::open(&dir, catalog(), no_faults()).unwrap();
    assert!(matches!(
        resolve(&mut e, &inv),
        ResolveReplyV1::Terminal(b) if matches!(*b, ClientReplyV1::IdentityExpired { .. })
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stale_schema_and_unknown_operation_are_refused_before_binding() {
    let dir = temp_dir("rt-stale");
    let mut e = seeded(&dir, 5);
    let mut inv = reserve(&e, "stale", 1, 1);
    inv.content.schema_hash = SchemaHash(carolina_core::hash::sha256(b"other schema"));
    inv.request_hash = inv.content.request_hash();
    match e.invoke(&inv) {
        ClientReplyV1::Unavailable(r) => {
            assert_eq!(r.code, "StaleSchema");
            assert!(!r.possibly_admitted);
        }
        other => panic!("{}", other.kind()),
    }
    assert!(matches!(
        resolve(&mut e, &inv),
        ResolveReplyV1::AbsentAtBarrier { .. }
    ));
    let mut inv = reserve(&e, "wrong-type", 1, 1);
    inv.content.arguments = Value::Tuple(vec![Value::I64(1)]).to_canon();
    inv.request_hash = inv.content.request_hash();
    assert!(matches!(e.invoke(&inv), ClientReplyV1::Unavailable(_)));
    assert_eq!(item(&mut e), (5, 0, 5));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn many_requests_survive_checkpoint_and_reopen_with_identical_receipts() {
    let dir = temp_dir("rt-many");
    let mut receipts = Vec::new();
    {
        let mut e = seeded(&dir, 1000);
        for i in 0..60u8 {
            let inv = reserve(&e, &format!("req-{i}"), 2, i);
            receipts.push((inv.clone(), committed(&e.invoke(&inv)).clone()));
            if i % 20 == 19 {
                e.checkpoint().unwrap();
            }
        }
        assert_eq!(item(&mut e), (880, 120, 1000));
    }
    let mut e = LocalEngine::open(&dir, catalog(), no_faults()).unwrap();
    e.verify(VerifyMode::Full).unwrap();
    assert_eq!(item(&mut e), (880, 120, 1000));
    assert_eq!(e.count_rows("Reservation").unwrap(), 60);
    for (inv, r) in &receipts {
        match resolve(&mut e, inv) {
            ResolveReplyV1::Terminal(b) => assert_eq!(committed(&b).encode(), r.encode()),
            other => panic!("{other:?}"),
        }
        assert_eq!(committed(&e.invoke(inv)).encode(), r.encode());
    }
    assert_eq!(item(&mut e), (880, 120, 1000));
    let _ = std::fs::remove_dir_all(&dir);
}
