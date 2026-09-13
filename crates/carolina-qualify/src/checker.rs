//! History checker (SPEC-010 §5) for the local profile.
//!
//! The checker replays the observable history against the independent W1 oracle. Unknown replies
//! without durable resolution are explored both ways (committed / not committed) within a budget;
//! exceeding the budget yields `INCONCLUSIVE` with the explored bound (SPEC-010 §5, QA-04). An
//! observed durable commit is never dropped to make a history legal (QA-03).
//!
//! Rules Q-C06…Q-C12 concern capabilities the local profile does not enable (causal sessions,
//! C1 merge, escrow, certification, multi-IDC, migration). They are reported `NOT_APPLICABLE`
//! with that justification and never counted as PASS.

use std::collections::{BTreeMap, BTreeSet};

use carolina_core::hash::Hash256;

use crate::history::{History, HistoryEvent};
use crate::verdict::{CheckResult, Status};
use crate::w1::{InventoryModel, W1Op};

pub const CHECKER_VERSION: &str = "history-checker/0.1.0 (local profile Q-C01..05,13,14)";

pub struct CheckerInput<'a> {
    pub history: &'a History,
    pub initial: &'a InventoryModel,
    /// Engine state read after the final restart (None when the run could not read it).
    pub final_state: Option<&'a InventoryModel>,
    /// Maximum (branches × events) the checker may explore before reporting INCONCLUSIVE.
    pub budget: usize,
}

#[derive(Debug, Clone)]
struct Branch {
    model: InventoryModel,
    /// Requests whose unknown attempt this branch assumed committed.
    assumed_committed: BTreeSet<String>,
    /// The contract result expected for each assumed commit (evaluated on the pre-state).
    assumed_results: BTreeMap<String, Option<Hash256>>,
}

#[derive(Debug, Default)]
struct Findings {
    fails: BTreeMap<&'static str, Vec<String>>,
    inconclusive: Option<String>,
    explored: usize,
}

impl Findings {
    fn fail(&mut self, rule: &'static str, msg: String) {
        self.fails.entry(rule).or_default().push(msg);
    }
}

/// (kind, txn id, receipt digest, result digest) as returned by `resolve`.
type Resolution = (String, Vec<u8>, Option<Hash256>, Option<Hash256>);

#[derive(Debug, Clone)]
struct ReqInfo {
    request_key: Vec<u8>,
    op: W1Op,
    /// The reply that made the request final (outcome, txn, receipt digest, result digest).
    final_reply: Option<(String, Vec<u8>, Hash256, Hash256)>,
    unknown_attempts: Vec<u64>,
    resolved: Option<Resolution>,
    evicted: bool,
    decision: Option<(Vec<u8>, String)>,
}

pub fn check(input: &CheckerInput<'_>) -> Vec<CheckResult> {
    let mut f = Findings::default();
    let mut reqs: BTreeMap<String, ReqInfo> = BTreeMap::new();
    let mut key_owner: BTreeMap<Vec<u8>, (String, Hash256)> = BTreeMap::new();
    let mut attempts: BTreeMap<u64, (String, bool)> = BTreeMap::new(); // attempt -> (req, mismatched)
    let mut retired: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut last_restart_step = 0u64;
    let mut branches: Vec<Branch> = vec![Branch {
        model: input.initial.clone(),
        assumed_committed: BTreeSet::new(),
        assumed_results: BTreeMap::new(),
    }];

    // pre-scan: resolutions and the last restart (Q-C01 needs "resolvable after recovery")
    for e in &input.history.events {
        if let HistoryEvent::Restart { .. } = e.event {
            last_restart_step = e.step;
        }
    }

    for e in &input.history.events {
        f.explored += branches.len();
        if f.explored > input.budget {
            f.inconclusive = Some(format!(
                "checker budget exhausted after {} branch-events at step {} ({} live branches)",
                f.explored,
                e.step,
                branches.len()
            ));
            break;
        }
        match &e.event {
            HistoryEvent::Invoke {
                attempt,
                req,
                request_key,
                request_hash,
                op,
            } => {
                let mismatched = match key_owner.get(request_key) {
                    Some((_, h)) => *h != *request_hash,
                    None => {
                        key_owner.insert(request_key.clone(), (req.clone(), *request_hash));
                        false
                    }
                };
                attempts.insert(*attempt, (req.clone(), mismatched));
                reqs.entry(req.clone()).or_insert_with(|| ReqInfo {
                    request_key: request_key.clone(),
                    op: op.clone(),
                    final_reply: None,
                    unknown_attempts: vec![],
                    resolved: None,
                    evicted: false,
                    decision: None,
                });
            }
            HistoryEvent::AdmissionRefusal { attempt, req, code } => {
                let _ = (attempt, req, code);
            }
            HistoryEvent::FinalReply {
                attempt,
                req,
                outcome,
                txn_id,
                receipt_digest,
                result_digest,
            } => {
                let (_, mismatched) = attempts
                    .get(attempt)
                    .cloned()
                    .unwrap_or((req.clone(), false));
                if mismatched {
                    f.fail("Q-C03", format!("step {}: attempt {attempt} with different content under key of {req} executed", e.step));
                    continue;
                }
                let info = match reqs.get_mut(req) {
                    Some(i) => i,
                    None => {
                        f.fail(
                            "Q-C02",
                            format!("step {}: reply for never-invoked request {req}", e.step),
                        );
                        continue;
                    }
                };
                if retired.contains(&info.request_key[16..32]) {
                    f.fail(
                        "Q-C13",
                        format!("step {}: {req} executed in a retired namespace", e.step),
                    );
                }
                if info.evicted {
                    f.fail(
                        "Q-C13",
                        format!("step {}: {req} re-executed after result eviction", e.step),
                    );
                }
                if let Some((o, t, rd, res)) = &info.final_reply {
                    // retry: identical outcome, txn and result bytes; never a second effect
                    if o != outcome || t != txn_id || rd != receipt_digest || res != result_digest {
                        f.fail(
                            "Q-C03",
                            format!(
                                "step {}: retry of {req} returned a different receipt/result",
                                e.step
                            ),
                        );
                    }
                    continue;
                }
                if !info.unknown_attempts.is_empty() && info.resolved.is_none() {
                    // an unknown attempt followed by a fresh execution without resolving first
                    f.fail("Q-C14", format!("step {}: {req} executed again after an unknown reply without resolution", e.step));
                }
                // first final reply: the oracle decides what the contract allows
                let op = info.op.clone();
                let mut next: Vec<Branch> = Vec::new();
                for b in &branches {
                    let (expected, post) = b.model.evaluate(&op);
                    match (outcome.as_str(), expected.is_committed()) {
                        ("COMMITTED", true) => {
                            if expected.result_digest() != Some(*result_digest) {
                                f.fail("Q-C04", format!("step {}: {req} {} returned a result the contract does not produce", e.step, op.describe()));
                            }
                            let post = post.unwrap();
                            if let Err(inv) = post.invariants() {
                                f.fail("Q-C05", format!("step {}: {inv} after {req}", e.step));
                            }
                            next.push(Branch {
                                model: post,
                                assumed_committed: b.assumed_committed.clone(),
                                assumed_results: b.assumed_results.clone(),
                            });
                        }
                        ("REJECTED", false) => next.push(b.clone()),
                        ("COMMITTED", false) => {
                            // committed an invocation the contract rejects in this branch
                        }
                        ("REJECTED", true) => {
                            // rejected an invocation the contract accepts in this branch
                        }
                        _ => f.fail(
                            "Q-C04",
                            format!("step {}: unknown outcome label {outcome}", e.step),
                        ),
                    }
                }
                if next.is_empty() {
                    // no branch explains the reply: report against the sole/first branch's expectation
                    let (expected, _) = branches[0].model.evaluate(&op);
                    let msg = match (&expected, outcome.as_str()) {
                        (crate::w1::Expected::Rejected(why), "COMMITTED") => format!(
                            "step {}: {req} {} committed but the contract rejects it ({why})",
                            e.step,
                            op.describe()
                        ),
                        (crate::w1::Expected::Committed(_), "REJECTED") => format!(
                            "step {}: {req} {} rejected but the contract accepts it",
                            e.step,
                            op.describe()
                        ),
                        _ => format!(
                            "step {}: {req} inconsistent with every candidate history",
                            e.step
                        ),
                    };
                    f.fail("Q-C04", msg);
                    // keep exploring on the unchanged branches so later rules still report
                    next = branches.clone();
                }
                branches = next;
                info.final_reply = Some((
                    outcome.clone(),
                    txn_id.clone(),
                    *receipt_digest,
                    *result_digest,
                ));
            }
            HistoryEvent::UnknownReply { attempt, req } => {
                let (_, mismatched) = attempts
                    .get(attempt)
                    .cloned()
                    .unwrap_or((req.clone(), false));
                if let Some(info) = reqs.get_mut(req) {
                    if info.final_reply.is_some() || mismatched {
                        continue; // unknown after final: no new effect is possible
                    }
                    info.unknown_attempts.push(*attempt);
                    // branch: the effect may or may not have happened
                    let op = info.op.clone();
                    let mut next = Vec::new();
                    for b in &branches {
                        next.push(b.clone());
                        let (expected, post) = b.model.evaluate(&op);
                        if expected.is_committed() {
                            let mut assumed = b.assumed_committed.clone();
                            assumed.insert(req.clone());
                            let mut results = b.assumed_results.clone();
                            results.insert(req.clone(), expected.result_digest());
                            next.push(Branch {
                                model: post.unwrap(),
                                assumed_committed: assumed,
                                assumed_results: results,
                            });
                        }
                    }
                    branches = next;
                }
            }
            HistoryEvent::ExpiredReply { .. } => {}
            HistoryEvent::Decision {
                req,
                txn_id,
                outcome,
                ..
            } => {
                if let Some(info) = reqs.get_mut(req) {
                    info.decision = Some((txn_id.clone(), outcome.clone()));
                }
            }
            HistoryEvent::Crash { .. }
            | HistoryEvent::Restart { .. }
            | HistoryEvent::Checkpoint => {}
            HistoryEvent::Resolve {
                req,
                kind,
                txn_id,
                receipt_digest,
                result_digest,
            } => {
                let info = match reqs.get_mut(req) {
                    Some(i) => i,
                    None => continue,
                };
                info.resolved = Some((
                    kind.clone(),
                    txn_id.clone(),
                    *receipt_digest,
                    *result_digest,
                ));
                match kind.as_str() {
                    "COMMITTED" | "REJECTED" => {
                        if let Some((o, t, rd, _)) = &info.final_reply {
                            if o != kind || t != txn_id || Some(*rd) != *receipt_digest {
                                f.fail("Q-C01", format!("step {}: {req} resolves to a different receipt than the one replied", e.step));
                            }
                        } else if !info.unknown_attempts.is_empty() {
                            // durable evidence decides the unknown attempt: keep only consistent branches
                            let committed = kind == "COMMITTED";
                            let op = info.op.clone();
                            let before = branches.len();
                            branches.retain(|b| b.assumed_committed.contains(req) == committed);
                            if branches.is_empty() {
                                f.fail("Q-C14", format!("step {}: durable evidence for {req} ({kind}) contradicts every candidate history (before: {before} branches)", e.step));
                                branches = vec![Branch {
                                    model: input.initial.clone(),
                                    assumed_committed: BTreeSet::new(),
                                    assumed_results: BTreeMap::new(),
                                }];
                            }
                            if committed {
                                let _ = &op;
                                for b in &branches {
                                    if let (Some(Some(exp)), Some(rd)) =
                                        (b.assumed_results.get(req), result_digest)
                                    {
                                        if exp != rd {
                                            f.fail("Q-C04", format!("step {}: resolved result of {req} differs from the contract result", e.step));
                                        }
                                    }
                                }
                            }
                            info.final_reply = Some((
                                kind.clone(),
                                txn_id.clone(),
                                receipt_digest.unwrap_or(Hash256::ZERO),
                                result_digest.unwrap_or(Hash256::ZERO),
                            ));
                        }
                    }
                    "ABSENT" | "PENDING" => {
                        if info.final_reply.is_some() && e.step > last_restart_step {
                            f.fail(
                                "Q-C01",
                                format!(
                                    "step {}: final request {req} is {kind} after recovery",
                                    e.step
                                ),
                            );
                        }
                        if kind == "ABSENT" {
                            branches.retain(|b| !b.assumed_committed.contains(req));
                            if branches.is_empty() {
                                f.fail("Q-C02", format!("step {}: {req} absent at barrier but its effect was assumed", e.step));
                                branches = vec![Branch {
                                    model: input.initial.clone(),
                                    assumed_committed: BTreeSet::new(),
                                    assumed_results: BTreeMap::new(),
                                }];
                            }
                        }
                    }
                    _ => {}
                }
            }
            HistoryEvent::ResultEvicted { req } => {
                if let Some(info) = reqs.get_mut(req) {
                    info.evicted = true;
                }
            }
            HistoryEvent::NamespaceRetired { namespace } => {
                retired.insert(namespace.clone());
            }
        }
    }

    // ---- end-of-history rules ----
    if f.inconclusive.is_none() {
        for (req, info) in &reqs {
            if let Some((o, _, receipt, _)) = &info.final_reply {
                if info.decision.is_none() {
                    f.fail(
                        "Q-C01",
                        format!("{req}: {o} reply without a durable decision reference"),
                    );
                }
                if last_restart_step > 0 {
                    match &info.resolved {
                        Some((k, _, _, _)) if k == o => {}
                        Some((k, _, Some(rd), _))
                            if k == "EXPIRED" && info.evicted && rd == receipt =>
                        {
                            // evicted result: the tombstone keeps the receipt digest and the decision
                        }
                        Some((k, _, _, _)) => f.fail(
                            "Q-C01",
                            format!("{req}: final {o} but resolves to {k} after recovery"),
                        ),
                        None => f.fail(
                            "Q-C01",
                            format!("{req}: final {o} but never resolved after recovery"),
                        ),
                    }
                }
            }
            if !info.unknown_attempts.is_empty()
                && info.final_reply.is_none()
                && info.resolved.is_none()
            {
                // left pending: both candidate histories remain; nothing to assert
            }
        }
        if let Some(fs) = input.final_state {
            let consistent: Vec<&Branch> = branches.iter().filter(|b| b.model == *fs).collect();
            if consistent.is_empty() {
                let d = branches[0].model.diff(fs);
                let extra_effects = d
                    .iter()
                    .any(|x| x.contains("only on the other side") || x.contains(" vs "));
                let rule = if extra_effects { "Q-C02" } else { "Q-C01" };
                f.fail(
                    rule,
                    format!(
                        "final engine state matches none of {} candidate histories: {}",
                        branches.len(),
                        d.join("; ")
                    ),
                );
            }
            if let Err(inv) = fs.invariants() {
                f.fail("Q-C05", format!("final engine state: {inv}"));
            }
        }
    }

    // ---- results ----
    let local_rules: [(&str, &str); 7] = [
        ("Q-C01", "every final reply has a durable decision and resolves identically after recovery"),
        ("Q-C02", "no committed effect without a durable decision (final state explained by decided requests)"),
        ("Q-C03", "retries yield one effect and the same result; changed content never executes"),
        ("Q-C04", "exact result semantics and allowed rejections per the W1 contract"),
        ("Q-C05", "invariants hold in every committed state; prepared data invisible"),
        ("Q-C13", "eviction/retirement never re-execute or resurrect"),
        ("Q-C14", "unknown replies are never treated as aborts or as a license to execute anew"),
    ];
    let scope = format!(
        "{} events, {} requests, explored {} branch-events, budget {}",
        input.history.len(),
        reqs.len(),
        f.explored,
        input.budget
    );
    let mut out = Vec::new();
    for (id, what) in local_rules {
        if let Some(reason) = &f.inconclusive {
            out.push(CheckResult::inconclusive(id, scope.clone(), reason.clone()));
            continue;
        }
        match f.fails.get(id) {
            Some(v) => out.push(CheckResult::fail(
                id,
                scope.clone(),
                format!("{what}: {}", v.join(" | ")),
            )),
            None => out.push(CheckResult::pass(id, scope.clone(), what)),
        }
    }
    for (id, cap) in [
        (
            "Q-C06",
            "causal sessions (C2) not enabled in the local profile",
        ),
        ("Q-C07", "C1 merge not enabled in the local profile"),
        ("Q-C08", "C3 escrow not enabled in the local profile"),
        ("Q-C09", "C4 certification not enabled in the local profile"),
        (
            "Q-C10",
            "C5 IDC ordering is single-node serial here; distributed ordering not enabled",
        ),
        (
            "Q-C11",
            "multi-IDC finality not enabled in the local profile",
        ),
        ("Q-C12", "migration not enabled in the local profile"),
    ] {
        out.push(CheckResult::not_applicable(id, cap));
    }
    out
}

/// True when any local rule failed.
pub fn any_failure(results: &[CheckResult]) -> bool {
    results.iter().any(|r| r.status == Status::Fail)
}
