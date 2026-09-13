//! Acceptance of the qualification system itself (SPEC-010 §18, QA-01…QA-08, QA-10) and one
//! quick campaign run over the local profile.

use std::path::PathBuf;

use carolina_core::hash::Hash256;
use carolina_lang::fixtures::fixture_source;
use carolina_qualify::checker::{check, CheckerInput};
use carolina_qualify::history::{History, HistoryEvent};
use carolina_qualify::local::{generate_schedule, run_schedule, RunOutcome, Schedule, ScheduledOp};
use carolina_qualify::minimize::minimize;
use carolina_qualify::runner::{run_campaign, validate_config, CampaignConfig, ConfigError};
use carolina_qualify::verdict::Status;
use carolina_qualify::w1::{ResState, W1Op};
use carolina_storage::io::FaultPoint;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn test_root(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("carolina-qualify-{label}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

fn catalog() -> std::sync::Arc<carolina_runtime::LocalCatalog> {
    std::sync::Arc::new(
        carolina_runtime::LocalCatalog::from_source(
            fixture_source("inventory_reserve_release").unwrap(),
        )
        .unwrap(),
    )
}

fn run(root: &std::path::Path, s: &Schedule) -> RunOutcome {
    run_schedule(root, &catalog(), s).unwrap()
}

fn statuses(o: &RunOutcome, budget: usize) -> Vec<(String, Status, String)> {
    check(&CheckerInput {
        history: &o.history,
        initial: &o.initial,
        final_state: o.final_state.as_ref(),
        budget,
    })
    .into_iter()
    .map(|c| (c.id, c.status, c.detail))
    .collect()
}

fn status_of(v: &[(String, Status, String)], id: &str) -> Status {
    v.iter()
        .find(|(i, _, _)| i == id)
        .map(|(_, s, _)| *s)
        .unwrap()
}

fn all_local_pass(v: &[(String, Status, String)]) -> bool {
    [
        "Q-C01", "Q-C02", "Q-C03", "Q-C04", "Q-C05", "Q-C13", "Q-C14",
    ]
    .iter()
    .all(|id| status_of(v, id) == Status::Pass)
}

#[test]
fn qa01_identical_seed_and_schedule_reproduce_the_trace_digest() {
    let tr = test_root("qa01");
    let s = generate_schedule(7, 30, Some((FaultPoint::AfterFsyncBeforePublish, 3)));
    let a = run(&tr, &s);
    let b = run(&tr, &s);
    assert_eq!(a.history.trace_digest(), b.history.trace_digest());
    assert!(a.history.len() > 30);
    let v = statuses(&a, 100_000);
    assert!(all_local_pass(&v), "{v:?}");
    // the crash was really injected and resolved
    assert!(
        a.history
            .events
            .iter()
            .any(|e| matches!(e.event, HistoryEvent::Crash { .. })),
        "no crash injected"
    );
    let _ = std::fs::remove_dir_all(&tr);
}

#[test]
fn qa02_negative_controls_are_detected() {
    let tr = test_root("qa02");
    let s = generate_schedule(3, 30, None);
    let o = run(&tr, &s);
    let base = statuses(&o, 100_000);
    assert!(all_local_pass(&base), "{base:?}");

    // (a) a retried request returns a different result: Q-C03
    let mut h = o.history.clone();
    let mut seen = std::collections::BTreeSet::new();
    let mut mutated = false;
    for e in h.events.iter_mut() {
        if let HistoryEvent::FinalReply {
            req, result_digest, ..
        } = &mut e.event
        {
            if !seen.insert(req.clone()) {
                *result_digest = Hash256([0xAB; 32]);
                mutated = true;
                break;
            }
        }
    }
    assert!(mutated, "schedule must contain a retry with a final reply");
    let v = check(&CheckerInput {
        history: &h,
        initial: &o.initial,
        final_state: o.final_state.as_ref(),
        budget: 100_000,
    });
    assert!(
        v.iter()
            .any(|c| c.id == "Q-C03" && c.status == Status::Fail),
        "{v:?}"
    );

    // (b) acknowledged before durable: a committed reply whose resolution says ABSENT: Q-C01
    let mut h = o.history.clone();
    let target = h
        .events
        .iter()
        .find_map(|e| match &e.event {
            HistoryEvent::FinalReply { req, outcome, .. } if outcome == "COMMITTED" => {
                Some(req.clone())
            }
            _ => None,
        })
        .unwrap();
    for e in h.events.iter_mut() {
        if let HistoryEvent::Resolve {
            req,
            kind,
            receipt_digest,
            result_digest,
            ..
        } = &mut e.event
        {
            if *req == target {
                *kind = "ABSENT".into();
                *receipt_digest = None;
                *result_digest = None;
            }
        }
    }
    let v = check(&CheckerInput {
        history: &h,
        initial: &o.initial,
        final_state: None,
        budget: 100_000,
    });
    assert!(
        v.iter()
            .any(|c| c.id == "Q-C01" && c.status == Status::Fail),
        "{v:?}"
    );

    // (c) QA-08: the final counters are consistent but an earlier final reservation was erased
    let mut fs = o.final_state.clone().unwrap();
    let victim = fs
        .reservations
        .keys()
        .next()
        .copied()
        .expect("a reservation exists");
    let r = fs.reservations.remove(&victim).unwrap();
    if r.state == ResState::Active {
        let it = fs.items.get_mut(&r.item).unwrap();
        it.reserved -= r.amount;
        it.available += r.amount; // counters still satisfy every invariant
    }
    assert!(fs.invariants().is_ok());
    let v = check(&CheckerInput {
        history: &o.history,
        initial: &o.initial,
        final_state: Some(&fs),
        budget: 100_000,
    });
    assert!(
        v.iter()
            .any(|c| (c.id == "Q-C02" || c.id == "Q-C01") && c.status == Status::Fail),
        "{v:?}"
    );

    // (d) unknown treated as abort: a fresh execution under the same key without resolving: Q-C14
    let mut h = History::default();
    let item = s.items[0].0;
    let op = W1Op::Reserve {
        item,
        q: 1,
        rid: [5; 16],
    };
    let key = vec![1u8; 48];
    h.push(HistoryEvent::Invoke {
        attempt: 1,
        req: "x".into(),
        request_key: key.clone(),
        request_hash: Hash256([1; 32]),
        op: op.clone(),
    });
    h.push(HistoryEvent::UnknownReply {
        attempt: 1,
        req: "x".into(),
    });
    h.push(HistoryEvent::Invoke {
        attempt: 2,
        req: "x".into(),
        request_key: key,
        request_hash: Hash256([1; 32]),
        op,
    });
    h.push(HistoryEvent::FinalReply {
        attempt: 2,
        req: "x".into(),
        outcome: "COMMITTED".into(),
        txn_id: vec![9],
        receipt_digest: Hash256([2; 32]),
        result_digest: Hash256([3; 32]),
    });
    let v = check(&CheckerInput {
        history: &h,
        initial: &o.initial,
        final_state: None,
        budget: 100_000,
    });
    assert!(
        v.iter()
            .any(|c| c.id == "Q-C14" && c.status == Status::Fail),
        "{v:?}"
    );
    let _ = std::fs::remove_dir_all(&tr);
}

#[test]
fn qa03_unknown_request_that_committed_is_accepted_only_with_its_durable_commit() {
    let tr = test_root("qa03");
    // AfterFsyncBeforePublish #3 in a fresh open: 1 = home epoch bump, 2 = first binding, 3 = first commit
    let s = Schedule {
        seed: 42,
        items: vec![([0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], 5)],
        ops: vec![
            ScheduledOp::New {
                req: "a".into(),
                op: W1Op::Reserve {
                    item: [0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
                    q: 2,
                    rid: [1; 16],
                },
            },
            ScheduledOp::New {
                req: "b".into(),
                op: W1Op::Reserve {
                    item: [0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
                    q: 1,
                    rid: [2; 16],
                },
            },
        ],
        fault: Some((FaultPoint::AfterFsyncBeforePublish, 3)),
    };
    let o = run(&tr, &s);
    let unknown = o
        .history
        .events
        .iter()
        .any(|e| matches!(&e.event, HistoryEvent::UnknownReply { req, .. } if req == "a"));
    let resolved_committed = o.history.events.iter().any(|e| matches!(&e.event, HistoryEvent::Resolve { req, kind, .. } if req == "a" && kind == "COMMITTED"));
    assert!(
        unknown && resolved_committed,
        "expected an unknown reply resolved to COMMITTED: {}",
        o.history.to_jsonl()
    );
    let v = statuses(&o, 100_000);
    assert!(all_local_pass(&v), "{v:?}");
    // pretending the unknown attempt aborted contradicts the durable commit and the final state
    let mut h = o.history.clone();
    for e in h.events.iter_mut() {
        if let HistoryEvent::Resolve { req, kind, .. } = &mut e.event {
            if req == "a" && kind == "COMMITTED" {
                *kind = "REJECTED".into();
            }
        }
    }
    let v = check(&CheckerInput {
        history: &h,
        initial: &o.initial,
        final_state: o.final_state.as_ref(),
        budget: 100_000,
    });
    assert!(v.iter().any(|c| c.status == Status::Fail), "{v:?}");
    let _ = std::fs::remove_dir_all(&tr);
}

#[test]
fn qa04_checker_budget_exhaustion_is_inconclusive() {
    let tr = test_root("qa04");
    let s = generate_schedule(5, 20, None);
    let o = run(&tr, &s);
    let v = statuses(&o, 3);
    assert_eq!(status_of(&v, "Q-C01"), Status::Inconclusive);
    assert!(v
        .iter()
        .find(|(i, _, _)| i == "Q-C04")
        .unwrap()
        .2
        .contains("budget"));
    let _ = std::fs::remove_dir_all(&tr);
}

#[test]
fn qa07_and_qa10_reject_invalid_configuration_before_running() {
    let tr = test_root("qa07");
    let mut cfg = CampaignConfig::quick(&root(), &tr);
    cfg.durability_mode = "unsafe-no-fsync".into();
    assert!(matches!(
        validate_config(&cfg),
        Err(ConfigError::UnsafeDurability(_))
    ));
    assert!(matches!(
        run_campaign(&cfg),
        Err(ConfigError::UnsafeDurability(_))
    ));
    let mut cfg = CampaignConfig::quick(&root(), &root().join("md"));
    cfg.durability_mode = "sync".into();
    assert!(matches!(
        validate_config(&cfg),
        Err(ConfigError::TestRootOutsideAllowlist(_))
    ));
    let _ = std::fs::remove_dir_all(&tr);
}

#[test]
fn empty_campaigns_and_zero_exercise_budgets_are_rejected_before_io() {
    let tr = std::env::temp_dir().join(format!("carolina-qualify-invalid-{}", std::process::id()));
    let base = CampaignConfig::quick(&root(), &tr);
    let mut cases = Vec::new();
    let mut empty = base.clone();
    empty.seeds.clear();
    cases.push(empty);
    for field in 0..6 {
        let mut cfg = base.clone();
        match field {
            0 => cfg.ops_per_schedule = 0,
            1 => cfg.crash_nth_max = 0,
            2 => cfg.storage_crash_nth_max = 0,
            3 => cfg.btree_ops = 0,
            4 => cfg.checker_budget = 0,
            _ => cfg.model_state_budget = 0,
        }
        cases.push(cfg);
    }
    for cfg in cases {
        assert!(matches!(
            run_campaign(&cfg),
            Err(ConfigError::InvalidBudget(_))
        ));
    }
    assert!(
        !tr.exists(),
        "invalid campaigns must not create a scratch directory"
    );
}

#[test]
fn minimizer_shrinks_a_failing_schedule_and_the_oracle_rechecks_it() {
    let tr = test_root("min");
    let s = generate_schedule(9, 24, None);
    // "failure" = the checker rejects a history whose first committed reply is corrupted
    let cat = catalog();
    let fails = |c: &Schedule| {
        let o = run_schedule(&tr, &cat, c).unwrap();
        let mut h = o.history.clone();
        let mut done = false;
        for e in h.events.iter_mut() {
            if let HistoryEvent::FinalReply {
                outcome,
                result_digest,
                ..
            } = &mut e.event
            {
                if outcome == "COMMITTED" && !done {
                    *result_digest = Hash256([7; 32]);
                    done = true;
                }
            }
        }
        done && check(&CheckerInput {
            history: &h,
            initial: &o.initial,
            final_state: None,
            budget: 100_000,
        })
        .iter()
        .any(|c| c.status == Status::Fail)
    };
    let (m, runs) = minimize(&s, fails, 40);
    assert!(
        m.ops.len() < s.ops.len(),
        "{} vs {}",
        m.ops.len(),
        s.ops.len()
    );
    assert!(runs >= 2);
    let _ = std::fs::remove_dir_all(&tr);
}

#[test]
fn quick_campaign_passes_q0_q1_q2_and_reports_the_rest_not_run() {
    let tr = test_root("campaign");
    let out = tr.join("out");
    let mut cfg = CampaignConfig::quick(&root(), &tr);
    cfg.out_dir = Some(out.clone());
    cfg.seeds = vec![1];
    cfg.crash_nth_max = 1;
    cfg.storage_crash_nth_max = 1;
    cfg.btree_ops = 1600;
    cfg.ops_per_schedule = 16;
    let report = run_campaign(&cfg).unwrap();
    let v = &report.verdict;
    eprintln!("{}", v.summary());
    for g in &v.gates {
        match g.gate.as_str() {
            "Q0" | "Q1" | "Q2" | "FM" | "Q3-C5" => {
                assert_eq!(g.status, Status::Pass, "{}: {}", g.gate, g.detail)
            }
            _ => assert_eq!(g.status, Status::NotRun, "{}: {}", g.gate, g.detail),
        }
    }
    assert_eq!(v.exit_code(), 0);
    assert!(v
        .checks
        .iter()
        .any(|c| c.id == "FM-2" && c.status == Status::Pass));
    assert!(
        v.gates
            .iter()
            .any(|g| g.gate == "Q4" && g.status == Status::NotRun),
        "Q4 needs the implemented protocol, not only the model"
    );
    assert!(report
        .manifest
        .enabled_features
        .iter()
        .any(|f| f.contains("C5")));
    // QA-05: omitting a required scenario is NOT_RUN and never a green exit
    let mut cfg2 = cfg.clone();
    cfg2.skip = vec!["Q1-CRASH-MATRIX".into()];
    cfg2.out_dir = None;
    let r2 = run_campaign(&cfg2).unwrap();
    let q1 = r2.verdict.gates.iter().find(|g| g.gate == "Q1").unwrap();
    assert_eq!(q1.status, Status::NotRun);
    assert_ne!(r2.verdict.exit_code(), 0);
    // bundle round trip
    let dir = report.bundle_dirs.last().unwrap();
    let loaded = carolina_qualify::bundle::load_bundle(dir).unwrap();
    assert_eq!(
        loaded.verdict.manifest_hash,
        report.manifest.manifest_hash()
    );
    let _ = std::fs::remove_dir_all(&tr);
}
