//! Callable storage qualification campaigns (SPEC-002 §127–§179, SPEC-010 §7).
//!
//! Each campaign returns `Ok(stats)` when every checked property held for the explored cases and
//! `Err(description)` naming the first violating case. The integration tests and the SPEC-010
//! runner (`carolina-qualify`) share these functions so that a verdict is computed by the same code
//! the tests run. Nothing here panics on a property violation.
//!
//! Fault model: process kill / short write inside one process. OS page-cache loss on machine power
//! failure is not modelled (SPEC-010 §7); the runner records that scope.

use std::collections::BTreeMap;
use std::sync::Arc;

use carolina_core::error::ErrorCode;
use carolina_core::ids::{LocalCommitSeq, PlanGeneration, RecordRevision, SchemaHash};
use carolina_core::rng::DetRng;

use crate::batch::*;
use crate::btree::{BTree, TreeCtx, ENTRY_FLAG_TOMBSTONE};
use crate::buffer::BufferPool;
use crate::format::PAGE_SIZE;
use crate::io::{CrashAtNth, FaultPoint, Faults, FilePageIo, NoFaults, PageIo, ALL_FAULT_POINTS};
use crate::journal::DurabilityMode;
use crate::kernel::*;
use crate::memkernel::MemKernel;
use crate::testutil::*;

macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            return Err(format!($($arg)*));
        }
    };
}

fn e2s<T>(r: Result<T, carolina_core::error::CoreError>, what: &str) -> Result<T, String> {
    r.map_err(|e| format!("{what}: {e}"))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CampaignStats {
    pub cases: u64,
    pub exercised: u64,
    pub detail: BTreeMap<String, u64>,
}

// ---------------------------------------------------------------------------
// P7 / P4 — B+Tree differential against BTreeMap<key, Vec<version>>
// ---------------------------------------------------------------------------

type Model = BTreeMap<Vec<u8>, Vec<(u64, Option<Vec<u8>>)>>;
type Rows = Vec<(Vec<u8>, Vec<u8>)>;

fn model_get(model: &Model, key: &[u8], visible: u64) -> Option<Vec<u8>> {
    model
        .get(key)?
        .iter()
        .rev()
        .find(|(s, _)| *s <= visible)
        .and_then(|(_, v)| v.clone())
}

fn model_scan(model: &Model, start: &[u8], end: Option<&[u8]>, visible: u64) -> Rows {
    let mut out = Vec::new();
    for (k, _) in model.range(start.to_vec()..) {
        if let Some(e) = end {
            if k.as_slice() >= e {
                break;
            }
        }
        if let Some(v) = model_get(model, k, visible) {
            out.push((k.clone(), v));
        }
    }
    out
}

struct TreeHarness {
    dir: std::path::PathBuf,
    io: FilePageIo,
    pool: BufferPool,
    tree: BTree,
    faults: Faults,
    lsn: u64,
    total_splits: u64,
    frames: usize,
}

impl TreeHarness {
    fn new(frames: usize) -> Result<TreeHarness, String> {
        let dir = temp_dir("btree");
        let faults: Faults = Arc::new(NoFaults);
        let io = e2s(
            FilePageIo::open(&dir.join("data.astr"), PAGE_SIZE, faults.clone()),
            "open",
        )?;
        let mut pool = BufferPool::new(frames);
        let tree = {
            let mut ctx = TreeCtx {
                pool: &mut pool,
                io: &io,
                durable_lsn: 0,
                current_lsn: 0,
                faults: &faults,
            };
            e2s(BTree::create(&mut ctx, 1), "create")?
        };
        Ok(TreeHarness {
            dir,
            io,
            pool,
            tree,
            faults,
            lsn: 1,
            total_splits: 0,
            frames,
        })
    }
    fn ctx(&mut self) -> (&BTree, TreeCtx<'_>) {
        (
            &self.tree,
            TreeCtx {
                pool: &mut self.pool,
                io: &self.io,
                durable_lsn: self.lsn,
                current_lsn: self.lsn,
                faults: &self.faults,
            },
        )
    }
    fn insert(&mut self, key: &[u8], seq: u64, value: Option<&[u8]>) -> Result<bool, String> {
        let flags = if value.is_none() {
            ENTRY_FLAG_TOMBSTONE
        } else {
            0
        };
        loop {
            let r = {
                let tree = &mut self.tree;
                let mut ctx = TreeCtx {
                    pool: &mut self.pool,
                    io: &self.io,
                    durable_lsn: self.lsn,
                    current_lsn: self.lsn,
                    faults: &self.faults,
                };
                tree.insert_version(
                    &mut ctx,
                    key,
                    seq,
                    [seq as u8; 32],
                    flags,
                    &[],
                    value.unwrap_or(&[]),
                )
            };
            match r {
                Ok(b) => return Ok(b),
                Err(e) if e.code == ErrorCode::ResourceLimit => self.persist_and_reopen()?,
                Err(e) => return Err(format!("insert: {e}")),
            }
        }
    }
    fn get(&mut self, key: &[u8], visible: u64) -> Result<Option<Vec<u8>>, String> {
        let (tree, mut ctx) = self.ctx();
        match e2s(tree.get_version(&mut ctx, key, visible), "get")? {
            Some(e) if !e.is_tombstone() => {
                Ok(Some(e2s(tree.read_value(&mut ctx, &e), "read_value")?))
            }
            _ => Ok(None),
        }
    }
    fn scan(&mut self, start: &[u8], end: Option<&[u8]>, visible: u64) -> Result<Rows, String> {
        let (tree, mut ctx) = self.ctx();
        let entries = e2s(
            tree.scan_visible(&mut ctx, start, end, visible, usize::MAX),
            "scan",
        )?;
        let mut out = Vec::new();
        for e in entries {
            if !e.is_tombstone() {
                let v = e2s(tree.read_value(&mut ctx, &e), "read_value")?;
                out.push((e.key, v));
            }
        }
        Ok(out)
    }
    fn persist_and_reopen(&mut self) -> Result<(), String> {
        self.lsn += 1;
        let root = {
            let tree = &mut self.tree;
            let mut ctx = TreeCtx {
                pool: &mut self.pool,
                io: &self.io,
                durable_lsn: self.lsn,
                current_lsn: self.lsn,
                faults: &self.faults,
            };
            e2s(tree.persist(&mut ctx), "persist")?
        };
        e2s(self.io.sync_data(), "sync")?;
        let next = self.tree.next_page_id;
        self.total_splits += self.tree.splits;
        let frames = self.frames;
        self.pool = BufferPool::new(frames);
        self.tree = BTree::open(root, next);
        Ok(())
    }
    fn verify(&mut self) -> Result<(), String> {
        let (tree, mut ctx) = self.ctx();
        e2s(tree.verify(&mut ctx), "verify").map(|_| ())
    }
}

fn key_of(rng: &mut DetRng, space: u64) -> Vec<u8> {
    let k = rng.below(space);
    let mut v = format!("k{k:06}").into_bytes();
    if k % 7 == 0 {
        v.extend(std::iter::repeat_n(b'x', 40));
    }
    v
}

/// P7 (random ordered insertion, splits, overflow values) and P4 (old snapshots stay stable)
/// against the reference model, including copy-on-write persist + reopen.
pub fn btree_differential(
    seed: u64,
    ops: usize,
    frames: usize,
    space: u64,
) -> Result<CampaignStats, String> {
    let mut rng = DetRng::new(seed);
    let mut h = TreeHarness::new(frames)?;
    let mut model: Model = BTreeMap::new();
    let mut seq = 0u64;
    let mut reads = 0u64;
    for i in 0..ops {
        seq += 1;
        let key = key_of(&mut rng, space);
        let value: Option<Vec<u8>> = if rng.chance(1, 5) {
            None
        } else if rng.chance(1, 40) {
            let n = 2000 + rng.below(9000) as usize;
            Some(rng.bytes(n))
        } else {
            let n = 1 + rng.below(120) as usize;
            Some(rng.bytes(n))
        };
        ensure!(
            h.insert(&key, seq, value.as_deref())?,
            "seed {seed}: fresh (key, seq) must insert"
        );
        ensure!(
            !h.insert(&key, seq, value.as_deref())?,
            "seed {seed}: duplicate (key, seq) must be idempotent"
        );
        model.entry(key.clone()).or_default().push((seq, value));
        if i % 97 == 0 {
            for _ in 0..5 {
                let k = key_of(&mut rng, space);
                let vis = rng.below(seq + 1);
                reads += 1;
                ensure!(
                    h.get(&k, vis)? == model_get(&model, &k, vis),
                    "seed {seed}: get({k:?}, {vis}) differs from model"
                );
            }
        }
        if i % 503 == 0 {
            let vis = rng.below(seq + 1);
            let start = key_of(&mut rng, space);
            let end = key_of(&mut rng, space);
            let (s, e) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            ensure!(
                h.scan(&s, Some(&e), vis)? == model_scan(&model, &s, Some(&e), vis),
                "seed {seed}: range scan differs at {vis}"
            );
            ensure!(
                h.scan(&[], None, vis)? == model_scan(&model, &[], None, vis),
                "seed {seed}: full scan differs at {vis}"
            );
        }
        if i % 1500 == 1499 {
            h.verify()?;
            h.persist_and_reopen()?;
            h.verify()?;
        }
    }
    h.verify()?;
    ensure!(
        h.scan(&[], None, seq)? == model_scan(&model, &[], None, seq),
        "seed {seed}: final scan differs"
    );
    let old = seq / 2;
    ensure!(
        h.scan(&[], None, old)? == model_scan(&model, &[], None, old),
        "seed {seed}: old snapshot differs (P4)"
    );
    h.persist_and_reopen()?;
    ensure!(
        h.scan(&[], None, seq)? == model_scan(&model, &[], None, seq),
        "seed {seed}: scan after reopen differs"
    );
    let splits = h.total_splits + h.tree.splits;
    ensure!(
        splits > 0,
        "seed {seed}: workload must exercise page splits"
    );
    let _ = std::fs::remove_dir_all(&h.dir);
    let mut detail = BTreeMap::new();
    detail.insert("ops".into(), ops as u64);
    detail.insert("splits".into(), splits);
    detail.insert("point_reads".into(), reads);
    Ok(CampaignStats {
        cases: ops as u64,
        exercised: ops as u64,
        detail,
    })
}

/// Many versions of one key spanning several leaves (separators carry `(key, seq)`).
pub fn btree_hot_key(versions: u64) -> Result<CampaignStats, String> {
    let mut h = TreeHarness::new(32)?;
    let key = b"hot".to_vec();
    for seq in 1..=versions {
        h.insert(&key, seq, Some(&seq.to_le_bytes()))?;
    }
    for vis in [1u64, 2, versions / 30, versions / 2, versions - 1, versions] {
        ensure!(
            h.get(&key, vis)? == Some(vis.to_le_bytes().to_vec()),
            "hot key visible at {vis} differs"
        );
    }
    ensure!(h.get(&key, 0)?.is_none(), "nothing visible at seq 0");
    ensure!(
        h.scan(&[], None, versions)?.len() == 1,
        "one key must scan as one row"
    );
    h.verify()?;
    h.persist_and_reopen()?;
    let probe = versions / 4 + 1;
    ensure!(
        h.get(&key, probe)? == Some(probe.to_le_bytes().to_vec()),
        "hot key after reopen"
    );
    ensure!(
        h.total_splits + h.tree.splits > 0,
        "hot key must split leaves"
    );
    let _ = std::fs::remove_dir_all(&h.dir);
    Ok(CampaignStats {
        cases: versions,
        exercised: versions,
        detail: BTreeMap::new(),
    })
}

// ---------------------------------------------------------------------------
// P1/P2/P3/P6/P9 — crash matrix at every durable boundary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Step {
    Commit {
        seed: u64,
        puts: Vec<(LogicalKey, Vec<u8>)>,
        dels: Vec<LogicalKey>,
    },
    Prepare {
        seed: u64,
        puts: Vec<(LogicalKey, Vec<u8>)>,
    },
    CommitPrepared {
        seed: u64,
    },
    AbortPrepared {
        seed: u64,
    },
    Checkpoint,
}

fn script() -> Vec<Step> {
    let mut v = Vec::new();
    for i in 1..=12u64 {
        v.push(Step::Commit {
            seed: i,
            puts: vec![
                (user_key(1, i), vec![i as u8; 20]),
                (user_key(2, i % 3), i.to_le_bytes().to_vec()),
            ],
            dels: if i > 6 {
                vec![user_key(1, i - 6)]
            } else {
                vec![]
            },
        });
    }
    v.push(Step::Checkpoint);
    v.push(Step::Prepare {
        seed: 100,
        puts: vec![(user_key(3, 1), b"p100".to_vec())],
    });
    v.push(Step::Prepare {
        seed: 101,
        puts: vec![(user_key(3, 2), b"p101".to_vec())],
    });
    v.push(Step::Commit {
        seed: 13,
        puts: vec![(user_key(1, 13), vec![13; 3000])],
        dels: vec![],
    });
    v.push(Step::CommitPrepared { seed: 100 });
    v.push(Step::AbortPrepared { seed: 101 });
    v.push(Step::Checkpoint);
    for i in 14..=20u64 {
        v.push(Step::Commit {
            seed: i,
            puts: vec![(user_key(1, i), vec![i as u8; 5])],
            dels: vec![],
        });
    }
    v
}

fn model_after(steps: &[Step], acked: usize) -> BTreeMap<LogicalKey, Vec<u8>> {
    let mut m = BTreeMap::new();
    let mut pend: BTreeMap<u64, Vec<(LogicalKey, Vec<u8>)>> = BTreeMap::new();
    for s in &steps[..acked] {
        match s {
            Step::Commit { puts, dels, .. } => {
                for (k, v) in puts {
                    m.insert(k.clone(), v.clone());
                }
                for k in dels {
                    m.remove(k);
                }
            }
            Step::Prepare { seed, puts } => {
                pend.insert(*seed, puts.clone());
            }
            Step::CommitPrepared { seed } => {
                if let Some(p) = pend.remove(seed) {
                    for (k, v) in p {
                        m.insert(k, v);
                    }
                }
            }
            Step::AbortPrepared { seed } => {
                pend.remove(seed);
            }
            Step::Checkpoint => {}
        }
    }
    m
}

fn crash_opts(faults: Faults) -> StoreOptions {
    StoreOptions {
        durability: DurabilityMode::Sync,
        segment_bytes: 64 * 1024,
        pool_frames: 32,
        faults,
        auto_checkpoint_dirty_pages: 24,
        ..Default::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaseOutcome {
    /// A crash was injected after at least one acknowledged step and every property held.
    Exercised,
    /// The n-th occurrence fell inside `Store::create`: nothing was acknowledged, nothing to check.
    DuringCreate,
    /// The fault point was never reached by the script.
    NotReached,
}

/// One crash case; `Err` on a property violation.
fn crash_case(point: FaultPoint, nth: u32) -> Result<CaseOutcome, String> {
    let tag = format!("{point:?}#{nth}");
    let dir = temp_dir(&format!("crash-{point:?}-{nth}"));
    let steps = script();
    let injector = Arc::new(CrashAtNth::new(point, nth));
    let mut acked = 0usize;
    let mut crashed = false;
    let mut crashed_step: Option<Step> = None;
    let mut tokens: BTreeMap<u64, PreparedToken> = BTreeMap::new();
    {
        let mut s = match Store::create(&dir, crash_opts(injector.clone())) {
            Ok(s) => s,
            Err(_) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Ok(CaseOutcome::DuringCreate);
            }
        };
        for step in &steps {
            let r: Result<(), carolina_core::error::CoreError> = match step {
                Step::Commit { seed, puts, dels } => {
                    s.commit(batch(*seed, puts, dels, true)).map(|_| ())
                }
                Step::Prepare { seed, puts } => s.prepare(prepare_batch(*seed, puts)).map(|t| {
                    tokens.insert(*seed, t);
                }),
                Step::CommitPrepared { seed } => s
                    .commit_prepared(
                        tokens[seed].clone(),
                        CommitDecision {
                            decision_ref: decision_ref(*seed),
                        },
                    )
                    .map(|_| ()),
                Step::AbortPrepared { seed } => s.abort_prepared(
                    tokens[seed].clone(),
                    AbortDecision {
                        decision_ref: decision_ref(*seed),
                    },
                ),
                Step::Checkpoint => s.checkpoint().map(|_| ()),
            };
            match r {
                Ok(()) => acked += 1,
                Err(_) => {
                    crashed = true;
                    crashed_step = Some(step.clone());
                    break;
                }
            }
        }
    }
    if !crashed {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(CaseOutcome::NotReached);
    }
    let mut s = Store::open(&dir, crash_opts(Arc::new(NoFaults)))
        .map_err(|e| format!("{tag}: recovery failed: {e}"))?;
    s.verify(VerifyMode::Full)
        .map_err(|e| format!("{tag}: verifier failed after recovery: {e}"))?;
    let expected = model_after(&steps, acked);
    let snap = s.snapshot();
    let touched: Vec<LogicalKey> = match &crashed_step {
        Some(Step::Commit { puts, dels, .. }) => puts
            .iter()
            .map(|(k, _)| k.clone())
            .chain(dels.iter().cloned())
            .collect(),
        Some(Step::CommitPrepared { seed }) => match steps
            .iter()
            .find(|st| matches!(st, Step::Prepare { seed: s2, .. } if s2 == seed))
        {
            Some(Step::Prepare { puts, .. }) => puts.iter().map(|(k, _)| k.clone()).collect(),
            _ => vec![],
        },
        _ => vec![],
    };
    for (k, v) in &expected {
        if touched.contains(k) {
            continue;
        }
        let got = e2s(s.get(k, snap), "get")?;
        ensure!(
            got.as_ref() == Some(v),
            "{tag}: acknowledged key {k:?} lost (P1)"
        );
    }
    if let Some(step) = &crashed_step {
        match step {
            Step::Commit { puts, dels, .. } => {
                let mut present: Vec<bool> = Vec::new();
                for (k, v) in puts {
                    present.push(e2s(s.get(k, snap), "get")?.as_ref() == Some(v));
                }
                for k in dels {
                    present.push(e2s(s.get(k, snap), "get")?.is_none() && expected.contains_key(k));
                }
                if !present.is_empty() {
                    let all = present.iter().all(|x| *x);
                    let mut none = true;
                    for (k, v) in puts {
                        if e2s(s.get(k, snap), "get")?.as_ref() == Some(v)
                            && expected.get(k) != Some(v)
                        {
                            none = false;
                        }
                    }
                    ensure!(
                        all || none,
                        "{tag}: partial batch after crash (P3): {present:?}"
                    );
                }
            }
            Step::CommitPrepared { seed } => {
                let puts = match steps
                    .iter()
                    .find(|st| matches!(st, Step::Prepare { seed: s2, .. } if s2 == seed))
                {
                    Some(Step::Prepare { puts, .. }) => puts.clone(),
                    _ => vec![],
                };
                let mut all = true;
                let mut none = true;
                for (k, v) in &puts {
                    let got = e2s(s.get(k, snap), "get")?;
                    if got.as_ref() != Some(v) {
                        all = false;
                    }
                    if got.is_some() {
                        none = false;
                    }
                }
                ensure!(all || none, "{tag}: partial prepared install (P3)");
                if none {
                    ensure!(
                        s.in_doubt().iter().any(|t| t.txn_id == txn(*seed)),
                        "{tag}: undecided prepare must remain IN_DOUBT (P9)"
                    );
                }
            }
            Step::Prepare { puts, .. } => {
                for (k, _) in puts {
                    ensure!(
                        e2s(s.get(k, snap), "get")?.is_none(),
                        "{tag}: prepared data visible (P9)"
                    );
                }
            }
            Step::AbortPrepared { .. } | Step::Checkpoint => {}
        }
    }
    let mut pend: BTreeMap<u64, Vec<(LogicalKey, Vec<u8>)>> = BTreeMap::new();
    for st in &steps[..acked] {
        match st {
            Step::Prepare { seed, puts } => {
                pend.insert(*seed, puts.clone());
            }
            Step::CommitPrepared { seed } | Step::AbortPrepared { seed } => {
                pend.remove(seed);
            }
            _ => {}
        }
    }
    for (seed, puts) in &pend {
        let decided_by_crash = matches!(&crashed_step, Some(Step::CommitPrepared { seed: s2 } | Step::AbortPrepared { seed: s2 }) if s2 == seed);
        if !decided_by_crash {
            ensure!(
                s.in_doubt().iter().any(|t| t.txn_id == txn(*seed)),
                "{tag}: prepared txn {seed} not in doubt after recovery (P9)"
            );
            for (k, _) in puts {
                ensure!(
                    e2s(s.get(k, snap), "get")?.is_none(),
                    "{tag}: prepared data of {seed} visible (P9)"
                );
            }
        }
    }
    let seq1 = s.durable_commit_seq();
    let d1 = e2s(state_digest(&mut s), "digest")?;
    drop(s);
    let mut s2 = Store::open(&dir, crash_opts(Arc::new(NoFaults)))
        .map_err(|e| format!("{tag}: second recovery failed: {e}"))?;
    ensure!(
        e2s(state_digest(&mut s2), "digest")? == d1,
        "{tag}: recovery not idempotent (P6)"
    );
    ensure!(
        s2.durable_commit_seq() == seq1,
        "{tag}: durable seq changed on second recovery (P6)"
    );
    ensure!(s2.durable_commit_seq() >= LocalCommitSeq(0), "{tag}: seq");
    drop(s2);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(CaseOutcome::Exercised)
}

/// Every fault point × occurrence 1..=`nth_max`. `detail` counts exercised crashes per point plus
/// `_during_create` / `_not_reached` totals. Crashes that fall inside `Store::create` acknowledge
/// nothing and are excluded from the exercised ratio.
pub fn crash_matrix(nth_max: u32) -> Result<CampaignStats, String> {
    let mut stats = CampaignStats::default();
    let mut during_create = 0u64;
    let mut not_reached = 0u64;
    for point in ALL_FAULT_POINTS {
        for nth in 1..=nth_max {
            stats.cases += 1;
            match crash_case(point, nth)? {
                CaseOutcome::Exercised => {
                    stats.exercised += 1;
                    *stats.detail.entry(format!("{point:?}")).or_insert(0) += 1;
                }
                CaseOutcome::DuringCreate => during_create += 1,
                CaseOutcome::NotReached => not_reached += 1,
            }
        }
    }
    stats.detail.insert("_during_create".into(), during_create);
    stats.detail.insert("_not_reached".into(), not_reached);
    let effective = stats.cases - during_create;
    ensure!(stats.exercised * 3 >= effective * 2, "the campaign must actually inject crashes at most points ({} of {} effective cases exercised)", stats.exercised, effective);
    Ok(stats)
}

/// An injected I/O error on the commit path never yields success and never exposes the batch.
pub fn io_error_never_yields_success() -> Result<(), String> {
    let probe_dir = temp_dir("crash-ioerr-probe");
    let probe = Arc::new(CrashAtNth::new(FaultPoint::BeforeFsync, u32::MAX));
    {
        let mut p = e2s(
            Store::create(
                &probe_dir,
                StoreOptions {
                    durability: DurabilityMode::Sync,
                    faults: probe.clone(),
                    pool_frames: 32,
                    ..Default::default()
                },
            ),
            "probe create",
        )?;
        e2s(
            p.commit(batch(1, &[(user_key(1, 1), vec![1])], &[], true)),
            "probe commit",
        )?;
    }
    let _ = std::fs::remove_dir_all(&probe_dir);
    let dir = temp_dir("crash-ioerr");
    let injector = Arc::new(CrashAtNth::io_error(
        FaultPoint::BeforeFsync,
        probe.occurrences() + 1,
    ));
    let opts = StoreOptions {
        durability: DurabilityMode::Sync,
        faults: injector,
        pool_frames: 32,
        ..Default::default()
    };
    let mut s = e2s(Store::create(&dir, opts), "create")?;
    e2s(
        s.commit(batch(1, &[(user_key(1, 1), vec![1])], &[], true)),
        "first commit",
    )?;
    let err = match s.commit(batch(2, &[(user_key(1, 2), vec![2])], &[], true)) {
        Ok(_) => return Err("commit succeeded despite injected I/O error".into()),
        Err(e) => e,
    };
    ensure!(err.code == ErrorCode::Io, "expected Io, got {:?}", err.code);
    let snap = s.snapshot();
    ensure!(
        e2s(s.get(&user_key(1, 2), snap), "get")?.is_none(),
        "failed commit must stay invisible"
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Torn tail is discarded (never durable); a corrupted committed frame fails closed.
pub fn torn_tail_and_mid_log_corruption() -> Result<(), String> {
    let opts = || StoreOptions {
        durability: DurabilityMode::Sync,
        pool_frames: 64,
        ..Default::default()
    };
    let dir = temp_dir("kernel-torn");
    {
        let mut s = e2s(Store::create(&dir, opts()), "create")?;
        for i in 1..=5u64 {
            e2s(
                s.commit(batch(i, &[(user_key(1, i), vec![i as u8])], &[], true)),
                "commit",
            )?;
        }
    }
    let seg = dir.join("journal").join("journal-0000000000000001.astj");
    let bytes = std::fs::read(&seg).map_err(|e| e.to_string())?;
    std::fs::write(&seg, &bytes[..bytes.len() - 100]).map_err(|e| e.to_string())?;
    {
        let mut s = e2s(Store::open(&dir, opts()), "open after torn tail")?;
        let snap = s.snapshot();
        ensure!(
            e2s(s.get(&user_key(1, 4), snap), "get")? == Some(vec![4]),
            "durable frame lost after torn tail"
        );
        ensure!(
            e2s(s.get(&user_key(1, 5), snap), "get")?.is_none(),
            "torn frame must not be durable"
        );
        e2s(
            s.commit(batch(6, &[(user_key(1, 6), vec![6])], &[], true)),
            "commit after truncation",
        )?;
    }
    let bytes = std::fs::read(&seg).map_err(|e| e.to_string())?;
    let mut corrupt = bytes.clone();
    corrupt[64 + 20] ^= 0xff;
    std::fs::write(&seg, &corrupt).map_err(|e| e.to_string())?;
    match Store::open(&dir, opts()) {
        Ok(_) => return Err("mid-log corruption must fail closed".into()),
        Err(e) => ensure!(
            e.code == ErrorCode::Corruption,
            "expected Corruption, got {e}"
        ),
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

// ---------------------------------------------------------------------------
// P3/P6 — Store ⇄ MemKernel differential (business rows + txn statuses)
// ---------------------------------------------------------------------------

pub fn kernel_differential(seed: u64, ops: u64) -> Result<CampaignStats, String> {
    let dir = temp_dir("kernel-mem-diff");
    let opts = StoreOptions {
        durability: DurabilityMode::Sync,
        pool_frames: 64,
        ..Default::default()
    };
    let mut s = e2s(Store::create(&dir, opts), "create")?;
    let mut m = MemKernel::new();
    let mut rng = DetRng::new(seed);
    for i in 1..=ops {
        let n = 1 + rng.below(4) as usize;
        let mut puts = Vec::new();
        let mut dels = Vec::new();
        let mut used = std::collections::BTreeSet::new();
        for _ in 0..n {
            let k = user_key(1, rng.below(60));
            if !used.insert(k.clone()) {
                continue;
            }
            if rng.chance(1, 4) {
                dels.push(k);
            } else {
                let n = 1 + rng.below(300) as usize;
                puts.push((k, rng.bytes(n)));
            }
        }
        let b = batch(i, &puts, &dels, i % 3 == 0);
        let rs = e2s(s.commit(b.clone()), "store commit")?;
        let rm = e2s(m.commit(b), "mem commit")?;
        ensure!(
            rs.local_version.seq == rm.local_version.seq,
            "seq differs at op {i}"
        );
        if i % 50 == 0 {
            ensure!(
                e2s(state_digest(&mut s), "digest")? == m.digest(),
                "digest differs at op {i}"
            );
        }
    }
    ensure!(
        e2s(state_digest(&mut s), "digest")? == m.digest(),
        "final digest differs"
    );
    for i in (3..=ops).step_by(3) {
        ensure!(
            e2s(s.txn_status(txn(i)), "status")? == e2s(m.txn_status(txn(i)), "status")?,
            "txn status {i} differs"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(CampaignStats {
        cases: ops,
        exercised: ops,
        detail: BTreeMap::new(),
    })
}

// ---------------------------------------------------------------------------
// P9 — prepared transactions stay invisible until a valid decision
// ---------------------------------------------------------------------------

pub fn prepared_invisibility() -> Result<(), String> {
    let opts = || StoreOptions {
        durability: DurabilityMode::Sync,
        pool_frames: 64,
        ..Default::default()
    };
    let dir = temp_dir("kernel-prepare");
    let token;
    {
        let mut s = e2s(Store::create(&dir, opts()), "create")?;
        e2s(
            s.commit(batch(1, &[(user_key(1, 1), b"base".to_vec())], &[], true)),
            "base",
        )?;
        token = e2s(
            s.prepare(prepare_batch(
                2,
                &[
                    (user_key(1, 1), b"prepared".to_vec()),
                    (user_key(1, 2), b"new".to_vec()),
                ],
            )),
            "prepare",
        )?;
        let again = e2s(
            s.prepare(prepare_batch(
                2,
                &[
                    (user_key(1, 1), b"prepared".to_vec()),
                    (user_key(1, 2), b"new".to_vec()),
                ],
            )),
            "re-prepare",
        )?;
        ensure!(again == token, "identical re-prepare must be idempotent");
        match s.prepare(prepare_batch(2, &[(user_key(1, 1), b"other".to_vec())])) {
            Ok(_) => return Err("different batch under the same txn must conflict".into()),
            Err(e) => ensure!(
                e.code == ErrorCode::IdentityConflict,
                "expected IdentityConflict, got {e}"
            ),
        }
        let snap = s.snapshot();
        ensure!(
            e2s(s.get(&user_key(1, 1), snap), "get")? == Some(b"base".to_vec()),
            "prepared data visible (P9)"
        );
        ensure!(
            e2s(s.get(&user_key(1, 2), snap), "get")?.is_none(),
            "prepared insert visible (P9)"
        );
        ensure!(s.in_doubt().len() == 1, "one in-doubt txn expected");
        e2s(s.checkpoint(), "checkpoint")?;
    }
    {
        let mut s = e2s(Store::open(&dir, opts()), "reopen")?;
        ensure!(
            s.readiness() == Readiness::WaitingProtocolReconciliation,
            "in-doubt work must block readiness"
        );
        ensure!(
            s.in_doubt() == vec![token.clone()],
            "in-doubt token must survive checkpoint + reopen"
        );
        let snap = s.snapshot();
        ensure!(
            e2s(s.get(&user_key(1, 1), snap), "get")? == Some(b"base".to_vec()),
            "prepared data visible after reopen (P9)"
        );
        let r = e2s(
            s.commit_prepared(
                token.clone(),
                CommitDecision {
                    decision_ref: decision_ref(2),
                },
            ),
            "commit_prepared",
        )?;
        ensure!(r.txn_id == Some(txn(2)), "txn id");
        ensure!(s.readiness() == Readiness::Ready, "ready after decision");
        let snap = s.snapshot();
        ensure!(
            e2s(s.get(&user_key(1, 1), snap), "get")? == Some(b"prepared".to_vec()),
            "decided data invisible"
        );
        ensure!(
            e2s(s.get(&user_key(1, 2), snap), "get")? == Some(b"new".to_vec()),
            "decided insert invisible"
        );
        match s.commit_prepared(
            token.clone(),
            CommitDecision {
                decision_ref: decision_ref(2),
            },
        ) {
            Ok(_) => return Err("duplicate decision applied twice".into()),
            Err(e) => ensure!(
                e.code == ErrorCode::TxnAlreadyCommitted,
                "expected TxnAlreadyCommitted, got {e}"
            ),
        }
    }
    {
        let mut s = e2s(Store::open(&dir, opts()), "reopen 2")?;
        let snap = s.snapshot();
        ensure!(
            e2s(s.get(&user_key(1, 2), snap), "get")? == Some(b"new".to_vec()),
            "committed prepared work lost on reopen"
        );
        let t = e2s(
            s.prepare(prepare_batch(3, &[(user_key(1, 3), b"never".to_vec())])),
            "prepare 3",
        )?;
        e2s(
            s.abort_prepared(
                t.clone(),
                AbortDecision {
                    decision_ref: decision_ref(3),
                },
            ),
            "abort",
        )?;
        ensure!(s.in_doubt().is_empty(), "abort must clear in-doubt");
        match s.abort_prepared(
            t.clone(),
            AbortDecision {
                decision_ref: decision_ref(3),
            },
        ) {
            Ok(_) => return Err("duplicate abort accepted".into()),
            Err(e) => ensure!(
                e.code == ErrorCode::TxnAlreadyAborted,
                "expected TxnAlreadyAborted, got {e}"
            ),
        }
        match s.commit_prepared(
            t,
            CommitDecision {
                decision_ref: decision_ref(3),
            },
        ) {
            Ok(_) => return Err("commit after abort accepted".into()),
            Err(e) => ensure!(
                e.code == ErrorCode::TxnAlreadyAborted,
                "expected TxnAlreadyAborted, got {e}"
            ),
        }
        ensure!(
            e2s(s.txn_status(txn(3)), "status")?.map(|r| r.phase) == Some(TxnPhase::Aborted),
            "aborted status must be durable"
        );
    }
    {
        let mut s = e2s(Store::open(&dir, opts()), "reopen 3")?;
        let snap = s.snapshot();
        ensure!(
            e2s(s.get(&user_key(1, 3), snap), "get")?.is_none(),
            "aborted data must never appear"
        );
        ensure!(s.in_doubt().is_empty(), "nothing in doubt");
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

// ---------------------------------------------------------------------------
// P10 (narrow) — an old storage epoch never serves as the current one
// ---------------------------------------------------------------------------

pub fn stale_epoch_is_refused() -> Result<(), String> {
    let opts = || StoreOptions {
        durability: DurabilityMode::Sync,
        pool_frames: 64,
        ..Default::default()
    };
    let dir = temp_dir("kernel-epoch");
    let mut s = e2s(Store::create(&dir, opts()), "create")?;
    e2s(
        s.commit(batch(1, &[(user_key(1, 1), vec![1])], &[], true)),
        "commit",
    )?;
    let old = s.snapshot();
    let e1 = s.storage_epoch();
    let e2 = e2s(s.advance_storage_epoch(), "advance epoch")?;
    ensure!(e2.0 == e1.0 + 1, "epoch must advance by one");
    match s.get(&user_key(1, 1), old) {
        Ok(_) => return Err("snapshot from an old storage epoch was served".into()),
        Err(e) => ensure!(
            e.code == ErrorCode::StaleEpoch,
            "expected StaleEpoch, got {e}"
        ),
    }
    let fresh = s.snapshot();
    ensure!(
        e2s(s.get(&user_key(1, 1), fresh), "get")? == Some(vec![1]),
        "data must survive an epoch change"
    );
    drop(s);
    let s = e2s(Store::open(&dir, opts()), "reopen")?;
    ensure!(
        s.storage_epoch() == e2,
        "epoch change must be durable ({:?} vs {:?})",
        s.storage_epoch(),
        e2
    );
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

// ---------------------------------------------------------------------------
// Admission refusals before persistence (stale plan/schema)
// ---------------------------------------------------------------------------

pub fn stale_admission_is_refused() -> Result<(), String> {
    let dir = temp_dir("kernel-stale");
    let mut s = e2s(
        Store::create(
            &dir,
            StoreOptions {
                durability: DurabilityMode::Sync,
                pool_frames: 64,
                ..Default::default()
            },
        ),
        "create",
    )?;
    s.min_plan_generation = PlanGeneration(12);
    match s.commit(batch(1, &[(user_key(1, 1), vec![1])], &[], true)) {
        Ok(_) => return Err("stale plan accepted".into()),
        Err(e) => ensure!(
            e.code == ErrorCode::StalePlan,
            "expected StalePlan, got {e}"
        ),
    }
    s.min_plan_generation = PlanGeneration(0);
    s.active_schema = Some(SchemaHash(carolina_core::hash::sha256(b"other")));
    match s.commit(batch(1, &[(user_key(1, 1), vec![1])], &[], true)) {
        Ok(_) => return Err("stale schema accepted".into()),
        Err(e) => ensure!(
            e.code == ErrorCode::StaleSchema,
            "expected StaleSchema, got {e}"
        ),
    }
    let snap = s.snapshot();
    ensure!(
        e2s(s.get(&user_key(1, 1), snap), "get")?.is_none(),
        "refused batch must leave no trace"
    );
    ensure!(s.metrics.commit_total == 0, "no commit counted");
    let _ = RecordRevision(0);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
