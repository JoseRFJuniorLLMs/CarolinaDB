//! Deterministic local schedules over the runtime engine (SPEC-010 §6/§7, SPEC-014 §4).
//!
//! All nondeterminism is the schedule seed and the fault schedule. The storage model is the real
//! file-backed kernel with its fault injector (process-kill / short-write model): the same seed,
//! schedule and fault produce the same history and trace digest. OS time, random generators and
//! threads are never consulted by the engine.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::sha256;
use carolina_core::ids::*;
use carolina_core::rng::DetRng;
use carolina_lang::types::Value;
use carolina_runtime::engine::{make_invoke, EngineOptions, LocalEngine};
use carolina_runtime::LocalCatalog;
use carolina_storage::io::{CrashAtNth, FaultPoint, NoFaults, ALL_FAULT_POINTS};
use carolina_storage::journal::DurabilityMode;
use carolina_storage::kernel::VerifyMode;
use carolina_storage::StoreOptions;
use carolina_wire::records::*;

use crate::history::{History, HistoryEvent};
use crate::w1::{Id, InventoryModel, ItemState, ResState, Reservation, W1Op};

pub const TENANT: &str = "tenant-q";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduledOp {
    New { req: String, op: W1Op },
    Retry { req: String },
    Mismatch { req: String, op: W1Op },
    Evict { req: String },
    Retire,
    Restart,
    Checkpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub seed: u64,
    pub items: Vec<(Id, i64)>,
    pub ops: Vec<ScheduledOp>,
    pub fault: Option<(FaultPoint, u32)>,
}

pub fn fault_point_label(p: FaultPoint) -> String {
    format!("{p:?}")
}

pub fn fault_point_from_label(s: &str) -> Option<FaultPoint> {
    ALL_FAULT_POINTS.into_iter().find(|p| format!("{p:?}") == s)
}

impl Canonical for ScheduledOp {
    fn to_canon(&self) -> CanonValue {
        match self {
            ScheduledOp::New { req, op } => CanonValue::obj()
                .fstr("kind", "New")
                .fc("op", op)
                .fstr("req", req)
                .build(),
            ScheduledOp::Retry { req } => CanonValue::obj()
                .fstr("kind", "Retry")
                .fstr("req", req)
                .build(),
            ScheduledOp::Mismatch { req, op } => CanonValue::obj()
                .fstr("kind", "Mismatch")
                .fc("op", op)
                .fstr("req", req)
                .build(),
            ScheduledOp::Evict { req } => CanonValue::obj()
                .fstr("kind", "Evict")
                .fstr("req", req)
                .build(),
            ScheduledOp::Retire => CanonValue::obj().fstr("kind", "Retire").build(),
            ScheduledOp::Restart => CanonValue::obj().fstr("kind", "Restart").build(),
            ScheduledOp::Checkpoint => CanonValue::obj().fstr("kind", "Checkpoint").build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let req = |v: &CanonValue| {
            v.field("req")
                .and_then(|r| r.as_str().map(|s| s.to_string()))
        };
        Ok(match v.field("kind")?.as_str()? {
            "New" => ScheduledOp::New {
                req: req(v)?,
                op: W1Op::from_canon(v.field("op")?)?,
            },
            "Retry" => ScheduledOp::Retry { req: req(v)? },
            "Mismatch" => ScheduledOp::Mismatch {
                req: req(v)?,
                op: W1Op::from_canon(v.field("op")?)?,
            },
            "Evict" => ScheduledOp::Evict { req: req(v)? },
            "Retire" => ScheduledOp::Retire,
            "Restart" => ScheduledOp::Restart,
            "Checkpoint" => ScheduledOp::Checkpoint,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("scheduled op {k}"),
                ))
            }
        })
    }
}

impl Canonical for Schedule {
    fn to_canon(&self) -> CanonValue {
        let items: Vec<CanonValue> = self
            .items
            .iter()
            .map(|(id, a)| {
                CanonValue::obj()
                    .f("available", CanonValue::Str(a.to_string()))
                    .fbytes("id", id)
                    .build()
            })
            .collect();
        let fault = match &self.fault {
            Some((p, n)) => CanonValue::obj()
                .fu32("nth", *n)
                .fstr("point", &fault_point_label(*p))
                .build(),
            None => CanonValue::Null,
        };
        CanonValue::obj()
            .f("fault", fault)
            .f("items", CanonValue::Array(items))
            .fvec("ops", &self.ops)
            .fu64("seed", self.seed)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["fault", "items", "ops", "seed"])?;
        let mut items = Vec::new();
        for it in v.field("items")?.as_array()? {
            let id: Id = it
                .field("id")?
                .as_bytes()?
                .try_into()
                .map_err(|_| CoreError::new(ErrorCode::NonCanonicalEncoding, "id"))?;
            let a = it
                .field("available")?
                .as_str()?
                .parse::<i64>()
                .map_err(|_| CoreError::new(ErrorCode::NonCanonicalEncoding, "available"))?;
            items.push((id, a));
        }
        let fault = match v.field("fault")? {
            CanonValue::Null => None,
            f => {
                let p = fault_point_from_label(f.field("point")?.as_str()?).ok_or_else(|| {
                    CoreError::new(ErrorCode::NonCanonicalEncoding, "fault point")
                })?;
                Some((p, f.field("nth")?.as_u64()? as u32))
            }
        };
        Ok(Schedule {
            seed: v.field("seed")?.as_u64()?,
            items,
            ops: v
                .field("ops")?
                .as_array()?
                .iter()
                .map(ScheduledOp::from_canon)
                .collect::<CoreResult<Vec<_>>>()?,
            fault,
        })
    }
}

fn item_id(i: u64) -> Id {
    let mut a = [0u8; 16];
    a[0] = 0x10;
    a[15] = i as u8;
    a
}

fn rid_of(n: u64) -> Id {
    let mut a = [0u8; 16];
    a[0] = 0x20;
    a[8..].copy_from_slice(&n.to_be_bytes());
    a
}

/// Seeded W1 schedule: reserves/releases/consumes/supplies over 2 items, duplicate deliveries,
/// changed content under a bound key, a result eviction and a restart; contention on the last units.
pub fn generate_schedule(seed: u64, len: usize, fault: Option<(FaultPoint, u32)>) -> Schedule {
    let mut rng = DetRng::new(seed);
    let items = vec![
        (item_id(1), 3 + rng.below(4) as i64),
        (item_id(2), 2 + rng.below(3) as i64),
    ];
    let mut ops = Vec::new();
    let mut next_rid = 1u64;
    let mut live: Vec<(String, Id, Id, i64)> = Vec::new(); // (req, item, rid, q) reserves issued
    let mut reqs: Vec<String> = Vec::new();
    for i in 0..len {
        let req = format!("r{i:03}");
        let roll = rng.below(100);
        let op = if roll < 45 || live.is_empty() {
            let (item, _) = items[rng.below(items.len() as u64) as usize];
            let q = 1 + rng.below(3) as i64;
            let rid = rid_of(next_rid);
            next_rid += 1;
            live.push((req.clone(), item, rid, q));
            W1Op::Reserve { item, q, rid }
        } else if roll < 65 {
            let (_, item, rid, q) = live[rng.below(live.len() as u64) as usize].clone();
            let q = if rng.chance(1, 5) { q + 1 } else { q }; // sometimes a wrong amount
            W1Op::Release { item, q, rid }
        } else if roll < 78 {
            let (_, item, rid, q) = live[rng.below(live.len() as u64) as usize].clone();
            W1Op::Consume { item, q, rid }
        } else if roll < 86 {
            let (item, _) = items[rng.below(items.len() as u64) as usize];
            let q = if rng.chance(1, 8) {
                i64::MAX
            } else {
                1 + rng.below(3) as i64
            };
            W1Op::Supply { item, q }
        } else if roll < 93 && !reqs.is_empty() {
            let r = reqs[rng.below(reqs.len() as u64) as usize].clone();
            ops.push(ScheduledOp::Retry { req: r });
            continue;
        } else if roll < 96 && !reqs.is_empty() {
            let r = reqs[rng.below(reqs.len() as u64) as usize].clone();
            let (item, _) = items[0];
            ops.push(ScheduledOp::Mismatch {
                req: r,
                op: W1Op::Supply { item, q: 1 },
            });
            continue;
        } else if roll < 98 {
            ops.push(ScheduledOp::Checkpoint);
            continue;
        } else {
            ops.push(ScheduledOp::Restart);
            continue;
        };
        reqs.push(req.clone());
        ops.push(ScheduledOp::New { req, op });
    }
    // deterministic coverage of Q-C03: changed content under a bound key, then a plain retry
    if let Some(r) = reqs.last() {
        let (item, _) = items[1];
        ops.push(ScheduledOp::Mismatch {
            req: r.clone(),
            op: W1Op::Supply { item, q: 7 },
        });
        ops.push(ScheduledOp::Retry { req: r.clone() });
    }
    if let Some(r) = reqs.first() {
        ops.push(ScheduledOp::Evict { req: r.clone() });
        ops.push(ScheduledOp::Retry { req: r.clone() });
    }
    ops.push(ScheduledOp::Restart);
    if let Some(r) = reqs.get(1) {
        ops.push(ScheduledOp::Retry { req: r.clone() });
    }
    Schedule {
        seed,
        items,
        ops,
        fault,
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunMetrics {
    pub attempts: u64,
    pub committed: u64,
    pub rejected: u64,
    pub refused: u64,
    pub unknown: u64,
    pub expired: u64,
    pub crashes: u64,
    pub restarts: u64,
    pub resolved: u64,
}

impl RunMetrics {
    pub fn to_map(&self, prefix: &str) -> BTreeMap<String, u64> {
        let mut m = BTreeMap::new();
        for (k, v) in [
            ("attempts", self.attempts),
            ("committed", self.committed),
            ("rejected", self.rejected),
            ("refused", self.refused),
            ("unknown", self.unknown),
            ("expired", self.expired),
            ("crashes", self.crashes),
            ("restarts", self.restarts),
            ("resolved", self.resolved),
        ] {
            m.insert(format!("{prefix}.{k}"), v);
        }
        m
    }
}

pub struct RunOutcome {
    pub history: History,
    pub initial: InventoryModel,
    pub final_state: Option<InventoryModel>,
    pub metrics: RunMetrics,
    pub verify_error: Option<String>,
}

struct Runner {
    catalog: Arc<LocalCatalog>,
    dir: PathBuf,
    engine: Option<LocalEngine>,
    injector: Option<Arc<CrashAtNth>>,
    fault: Option<(FaultPoint, u32)>,
    crashed: bool,
    history: History,
    reqs: BTreeMap<String, InvokeV1>,
    unknown: BTreeSet<String>,
    attempt: u64,
    metrics: RunMetrics,
}

fn store_opts(faults: carolina_storage::io::Faults) -> StoreOptions {
    StoreOptions {
        durability: DurabilityMode::Sync,
        pool_frames: 64,
        faults,
        auto_checkpoint_dirty_pages: 32,
        segment_bytes: 256 * 1024,
        ..Default::default()
    }
}

impl Runner {
    fn open_clean(&mut self) -> Result<(), String> {
        let catalog = self.catalog.clone();
        let e = LocalEngine::open(
            &self.dir,
            catalog,
            EngineOptions::local(store_opts(Arc::new(NoFaults))),
        )
        .map_err(|e| format!("reopen: {e}"))?;
        self.history.push(HistoryEvent::Restart {
            home_epoch: e.home_epoch().0,
        });
        self.metrics.restarts += 1;
        self.engine = Some(e);
        Ok(())
    }

    fn fault_fired(&self) -> bool {
        match (&self.injector, self.fault) {
            (Some(i), Some((_, nth))) => i.occurrences() >= nth,
            _ => false,
        }
    }

    /// After a crash: drop the engine (volatile state lost), reopen without faults, resolve unknowns.
    fn handle_crash(&mut self) -> Result<(), String> {
        let (p, n) = self.fault.unwrap();
        self.history.push(HistoryEvent::Crash {
            fault_point: fault_point_label(p),
            nth: n,
        });
        self.metrics.crashes += 1;
        self.crashed = true;
        self.engine = None;
        self.open_clean()?;
        self.resolve_unknowns()
    }

    fn resolve_unknowns(&mut self) -> Result<(), String> {
        let pending: Vec<String> = self.unknown.iter().cloned().collect();
        for req in pending {
            self.resolve(&req)?;
            self.unknown.remove(&req);
        }
        Ok(())
    }

    fn resolve(&mut self, req: &str) -> Result<(), String> {
        let inv = self
            .reqs
            .get(req)
            .cloned()
            .ok_or_else(|| format!("resolve unknown request {req}"))?;
        let e = self.engine.as_mut().ok_or("no engine")?;
        let reply = e.resolve(&ResolveRequestV1 {
            request_key: inv.content.request_key,
            expected_request_hash: inv.request_hash,
        });
        self.metrics.resolved += 1;
        let ev = match reply {
            ResolveReplyV1::Terminal(b) => match *b {
                ClientReplyV1::Committed(r) | ClientReplyV1::Rejected(r) => {
                    // the persisted receipt carries its durable decision reference
                    self.history.push(HistoryEvent::Decision {
                        req: req.into(),
                        txn_id: r.txn_id.0.to_vec(),
                        outcome: r.outcome.label().into(),
                        decision_ref: r.decision_ref.payload_hash,
                    });
                    HistoryEvent::Resolve {
                        req: req.into(),
                        kind: r.outcome.label().into(),
                        txn_id: r.txn_id.0.to_vec(),
                        receipt_digest: Some(r.receipt_digest().0),
                        result_digest: Some(sha256(&r.exact_result_bytes)),
                    }
                }
                ClientReplyV1::ResultExpired(t) => {
                    self.history.push(HistoryEvent::Decision {
                        req: req.into(),
                        txn_id: t.txn_id.0.to_vec(),
                        outcome: t.outcome.label().into(),
                        decision_ref: t.decision_ref.payload_hash,
                    });
                    HistoryEvent::Resolve {
                        req: req.into(),
                        kind: "EXPIRED".into(),
                        txn_id: t.txn_id.0.to_vec(),
                        receipt_digest: Some(t.receipt_digest.0),
                        result_digest: None,
                    }
                }
                ClientReplyV1::IdentityExpired { .. } => HistoryEvent::Resolve {
                    req: req.into(),
                    kind: "IDENTITY_EXPIRED".into(),
                    txn_id: vec![],
                    receipt_digest: None,
                    result_digest: None,
                },
                ClientReplyV1::RequestIdentityMismatch => HistoryEvent::Resolve {
                    req: req.into(),
                    kind: "MISMATCH".into(),
                    txn_id: vec![],
                    receipt_digest: None,
                    result_digest: None,
                },
                other => HistoryEvent::Resolve {
                    req: req.into(),
                    kind: other.kind().to_uppercase(),
                    txn_id: vec![],
                    receipt_digest: None,
                    result_digest: None,
                },
            },
            ResolveReplyV1::Pending { txn_id, .. } => HistoryEvent::Resolve {
                req: req.into(),
                kind: "PENDING".into(),
                txn_id: txn_id.0.to_vec(),
                receipt_digest: None,
                result_digest: None,
            },
            ResolveReplyV1::AbsentAtBarrier { .. } => HistoryEvent::Resolve {
                req: req.into(),
                kind: "ABSENT".into(),
                txn_id: vec![],
                receipt_digest: None,
                result_digest: None,
            },
            ResolveReplyV1::Unavailable(r) => HistoryEvent::Resolve {
                req: req.into(),
                kind: format!("UNAVAILABLE:{}", r.code),
                txn_id: vec![],
                receipt_digest: None,
                result_digest: None,
            },
        };
        self.history.push(ev);
        Ok(())
    }

    fn invoke(&mut self, req: &str, inv: InvokeV1, op: &W1Op) -> Result<(), String> {
        self.attempt += 1;
        let attempt = self.attempt;
        self.metrics.attempts += 1;
        self.history.push(HistoryEvent::Invoke {
            attempt,
            req: req.into(),
            request_key: inv.content.request_key.encode(),
            request_hash: inv.request_hash.0,
            op: op.clone(),
        });
        let e = self.engine.as_mut().ok_or("no engine")?;
        let reply = e.invoke(&inv);
        match &reply {
            ClientReplyV1::Committed(r) | ClientReplyV1::Rejected(r) => {
                if r.outcome == Outcome::Committed {
                    self.metrics.committed += 1;
                } else {
                    self.metrics.rejected += 1;
                }
                self.history.push(HistoryEvent::Decision {
                    req: req.into(),
                    txn_id: r.txn_id.0.to_vec(),
                    outcome: r.outcome.label().into(),
                    decision_ref: r.decision_ref.payload_hash,
                });
                self.history.push(HistoryEvent::FinalReply {
                    attempt,
                    req: req.into(),
                    outcome: r.outcome.label().into(),
                    txn_id: r.txn_id.0.to_vec(),
                    receipt_digest: r.receipt_digest().0,
                    result_digest: sha256(&r.exact_result_bytes),
                });
            }
            ClientReplyV1::OutcomeUnknown(_) => {
                self.metrics.unknown += 1;
                self.history.push(HistoryEvent::UnknownReply {
                    attempt,
                    req: req.into(),
                });
                self.unknown.insert(req.into());
            }
            ClientReplyV1::Unavailable(r) => {
                self.metrics.refused += 1;
                self.history.push(HistoryEvent::AdmissionRefusal {
                    attempt,
                    req: req.into(),
                    code: r.code.clone(),
                });
                if r.possibly_admitted {
                    self.unknown.insert(req.into());
                }
            }
            ClientReplyV1::RequestIdentityMismatch => {
                self.metrics.refused += 1;
                self.history.push(HistoryEvent::AdmissionRefusal {
                    attempt,
                    req: req.into(),
                    code: "RequestIdentityMismatch".into(),
                });
            }
            ClientReplyV1::ResultExpired(_) => {
                self.metrics.expired += 1;
                self.history.push(HistoryEvent::ExpiredReply {
                    attempt,
                    req: req.into(),
                    kind: "ResultExpired".into(),
                });
            }
            ClientReplyV1::IdentityExpired { .. } => {
                self.metrics.expired += 1;
                self.history.push(HistoryEvent::ExpiredReply {
                    attempt,
                    req: req.into(),
                    kind: "IdentityExpired".into(),
                });
            }
            ClientReplyV1::ProtocolError { code, .. } => {
                self.metrics.refused += 1;
                self.history.push(HistoryEvent::AdmissionRefusal {
                    attempt,
                    req: req.into(),
                    code: code.clone(),
                });
            }
        }
        if !self.crashed && self.fault_fired() {
            self.handle_crash()?;
        }
        Ok(())
    }
}

/// Convert engine rows into the oracle's model.
pub fn model_from_engine(e: &mut LocalEngine) -> Result<InventoryModel, String> {
    let mut m = InventoryModel::default();
    for (k, row) in e.dump_record("Item").map_err(|e| e.to_string())? {
        let id = match k {
            Value::Uuid(u) => u,
            _ => return Err("item key is not a uuid".into()),
        };
        let g = |n: &str| match row.get(n) {
            Some(Value::I64(v)) => Ok(*v),
            other => Err(format!("Item.{n}: {other:?}")),
        };
        m.items.insert(
            id,
            ItemState {
                available: g("available")?,
                reserved: g("reserved")?,
                total: g("total")?,
            },
        );
    }
    for (k, row) in e.dump_record("Reservation").map_err(|e| e.to_string())? {
        let rid = match k {
            Value::Uuid(u) => u,
            _ => return Err("reservation key is not a uuid".into()),
        };
        let item = match row.get("resource") {
            Some(Value::Uuid(u)) => *u,
            other => return Err(format!("Reservation.resource: {other:?}")),
        };
        let amount = match row.get("amount") {
            Some(Value::I64(v)) => *v,
            other => return Err(format!("Reservation.amount: {other:?}")),
        };
        let state = match row.get("state") {
            Some(Value::Enum { variant, .. }) => {
                ResState::from_variant(*variant).ok_or("bad state variant")?
            }
            other => return Err(format!("Reservation.state: {other:?}")),
        };
        m.reservations.insert(
            rid,
            Reservation {
                item,
                amount,
                state,
            },
        );
    }
    Ok(m)
}

/// Run one schedule in a fresh directory under `root`. The history is complete: it ends with a
/// restart, a resolve of every request and the recovered state.
pub fn run_schedule(
    root: &Path,
    catalog: &Arc<LocalCatalog>,
    sched: &Schedule,
) -> Result<RunOutcome, String> {
    let dir = root.join(format!(
        "w1-{}-{}",
        sched.seed,
        sched
            .fault
            .map(|(p, n)| format!("{p:?}-{n}"))
            .unwrap_or_else(|| "nofault".into())
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut r = Runner {
        catalog: catalog.clone(),
        dir: dir.clone(),
        engine: None,
        injector: None,
        fault: sched.fault,
        crashed: false,
        history: History::default(),
        reqs: BTreeMap::new(),
        unknown: BTreeSet::new(),
        attempt: 0,
        metrics: RunMetrics::default(),
    };
    let initial = InventoryModel::seed(&sched.items);
    // create + seed rows without faults so the initial state is a fixed, durable starting point
    {
        let mut e = LocalEngine::create(
            &dir,
            catalog.clone(),
            EngineOptions::local(store_opts(Arc::new(NoFaults))),
        )
        .map_err(|e| format!("create: {e}"))?;
        let rows: Vec<(Value, Vec<(&str, Value)>)> = sched
            .items
            .iter()
            .map(|(id, a)| {
                (
                    Value::Uuid(*id),
                    vec![
                        ("id", Value::Uuid(*id)),
                        ("available", Value::I64(*a)),
                        ("reserved", Value::I64(0)),
                        ("total", Value::I64(*a)),
                    ],
                )
            })
            .collect();
        e.load_rows("seed", "Item", &rows)
            .map_err(|e| format!("seed: {e}"))?;
    }
    // (re)open with the fault schedule armed
    {
        let faults: carolina_storage::io::Faults = match sched.fault {
            Some((p, n)) => {
                let inj = Arc::new(CrashAtNth::new(p, n));
                r.injector = Some(inj.clone());
                inj
            }
            None => Arc::new(NoFaults),
        };
        match LocalEngine::open(
            &dir,
            catalog.clone(),
            EngineOptions::local(store_opts(faults)),
        ) {
            Ok(e) => {
                r.history.push(HistoryEvent::Restart {
                    home_epoch: e.home_epoch().0,
                });
                r.engine = Some(e);
            }
            Err(_) if r.fault_fired() => {
                // crash during open (home epoch bump): recover without faults
                r.handle_crash()?;
            }
            Err(e) => return Err(format!("open with faults: {e}")),
        }
    }
    let catalog_for_invokes = catalog.clone();
    for op in &sched.ops {
        if r.engine.is_none() {
            r.open_clean()?;
        }
        match op {
            ScheduledOp::New { req, op } => {
                let inv = make_invoke(&catalog_for_invokes, TENANT, req, op.name(), op.args())
                    .map_err(|e| e.to_string())?;
                r.reqs.insert(req.clone(), inv.clone());
                r.invoke(req, inv, op)?;
            }
            ScheduledOp::Retry { req } => {
                if let Some(inv) = r.reqs.get(req).cloned() {
                    let op = op_of(&inv)?;
                    r.invoke(req, inv, &op)?;
                }
            }
            ScheduledOp::Mismatch { req, op } => {
                if r.reqs.contains_key(req) {
                    let inv = make_invoke(&catalog_for_invokes, TENANT, req, op.name(), op.args())
                        .map_err(|e| e.to_string())?;
                    r.invoke(req, inv, op)?;
                }
            }
            ScheduledOp::Evict { req } => {
                if let Some(inv) = r.reqs.get(req).cloned() {
                    let e = r.engine.as_mut().unwrap();
                    match e.evict_result(&inv.content.request_key) {
                        Ok(_) => {
                            r.history
                                .push(HistoryEvent::ResultEvicted { req: req.clone() });
                        }
                        Err(err)
                            if err.code == ErrorCode::TxnInDoubt
                                || err.code == ErrorCode::MissingRecord => {}
                        Err(err) => {
                            if r.fault_fired() && !r.crashed {
                                r.handle_crash()?;
                            } else {
                                return Err(format!("evict: {err}"));
                            }
                        }
                    }
                }
            }
            ScheduledOp::Retire => {
                let e = r.engine.as_mut().unwrap();
                let ns = RequestNamespace::derive("inventory");
                match e.retire_namespace(TenantId::derive(TENANT), ns, "schedule") {
                    Ok(_) => {
                        r.history.push(HistoryEvent::NamespaceRetired {
                            namespace: ns.0.to_vec(),
                        });
                    }
                    Err(err) if err.code == ErrorCode::Conflict => {}
                    Err(err) => {
                        if r.fault_fired() && !r.crashed {
                            r.handle_crash()?;
                        } else {
                            return Err(format!("retire: {err}"));
                        }
                    }
                }
            }
            ScheduledOp::Restart => {
                r.engine = None;
                r.open_clean()?;
                r.resolve_unknowns()?;
            }
            ScheduledOp::Checkpoint => {
                let e = r.engine.as_mut().unwrap();
                match e.checkpoint() {
                    Ok(_) => {
                        r.history.push(HistoryEvent::Checkpoint);
                    }
                    Err(_) if r.fault_fired() && !r.crashed => r.handle_crash()?,
                    Err(err) => return Err(format!("checkpoint: {err}")),
                }
            }
        }
    }
    // final recovery: everything must resolve identically, then read the durable state
    r.engine = None;
    r.open_clean()?;
    let all: Vec<String> = r.reqs.keys().cloned().collect();
    for req in all {
        r.resolve(&req)?;
    }
    r.unknown.clear();
    let e = r.engine.as_mut().unwrap();
    let verify_error = e.verify(VerifyMode::Full).err().map(|e| e.to_string());
    let final_state = model_from_engine(e).ok();
    let outcome = RunOutcome {
        history: r.history,
        initial,
        final_state,
        metrics: r.metrics,
        verify_error,
    };
    drop(r.engine);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(outcome)
}

fn op_of(inv: &InvokeV1) -> Result<W1Op, String> {
    let args = match Value::from_canon(&inv.content.arguments).map_err(|e| e.to_string())? {
        Value::Tuple(v) => v,
        _ => return Err("arguments".into()),
    };
    let u = |v: &Value| match v {
        Value::Uuid(u) => Ok(*u),
        _ => Err("uuid".to_string()),
    };
    let q = |v: &Value| match v {
        Value::I64(q) => Ok(*q),
        _ => Err("i64".to_string()),
    };
    Ok(match args.len() {
        2 => W1Op::Supply {
            item: u(&args[0])?,
            q: q(&args[1])?,
        },
        3 => {
            // the operation name is not in the invoke; recover it from the request namespace/op id
            let item = u(&args[0])?;
            let qq = q(&args[1])?;
            let rid = u(&args[2])?;
            match inv.content.operation.operation_id.0 {
                1 => W1Op::Reserve { item, q: qq, rid },
                2 => W1Op::Release { item, q: qq, rid },
                3 => W1Op::Consume { item, q: qq, rid },
                other => return Err(format!("operation id {other}")),
            }
        }
        n => return Err(format!("{n} arguments")),
    })
}
