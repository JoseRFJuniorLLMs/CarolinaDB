//! Campaign composition (SPEC-010 §17, §18): executes every check the local profile can run,
//! records what it cannot, derives gates and writes evidence bundles.
//!
//! Configuration is validated before anything runs: an unsafe durability mode is rejected
//! (QA-07) and every data directory must lie under the allowlisted test root (QA-10).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use carolina_compiler::plan::ProofStatus;
use carolina_compiler::{check_artifacts, compile, CompileInput, OperationPlan};
use carolina_core::canon::Canonical;
use carolina_core::hash::{hex_encode, sha256, Hash256};
use carolina_core::limits::Limits;
use carolina_lang::counterexample::{explore_concurrent, replay, Failure, Invocation};
use carolina_lang::fixtures::{fixture_source, load_fixture, render_hashes, FIXTURE_NAMES};
use carolina_lang::interp::State;
use carolina_lang::types::Value;
use carolina_storage::campaign as storage;
use carolina_storage::io::ALL_FAULT_POINTS;

use crate::bundle::{write_bundle, BundleInput};
use crate::checker::{any_failure, check, CheckerInput, CHECKER_VERSION};
use crate::history::History;
use crate::local::{generate_schedule, run_schedule, Schedule};
use crate::manifest::*;
use crate::verdict::{derive_gate, CheckResult, Status, Verdict};

#[derive(Debug, Clone)]
pub struct CampaignConfig {
    /// Repository root (source tree digest, fixtures, golden files).
    pub root: PathBuf,
    /// Allowlisted scratch root; every data directory is created below it.
    pub test_root: PathBuf,
    /// Where bundles and the campaign verdict are written (None = no files).
    pub out_dir: Option<PathBuf>,
    pub campaign_id: String,
    pub seeds: Vec<u64>,
    pub ops_per_schedule: usize,
    pub crash_nth_max: u32,
    pub storage_crash_nth_max: u32,
    pub btree_ops: usize,
    pub checker_budget: usize,
    pub minimizer_runs: usize,
    pub durability_mode: String,
    /// Check ids omitted on purpose (recorded NOT_RUN; QA-05).
    pub skip: Vec<String>,
    pub keep_passing_bundles: bool,
    /// State budget for each formal model exploration (FM-1/2/3).
    pub model_state_budget: usize,
}

impl CampaignConfig {
    /// Small budgets for CI and tests; PASS verdicts carry these bounds in their scope.
    pub fn quick(root: &Path, test_root: &Path) -> CampaignConfig {
        CampaignConfig {
            root: root.to_path_buf(),
            test_root: test_root.to_path_buf(),
            out_dir: None,
            campaign_id: "local-quick".into(),
            seeds: vec![1, 2],
            ops_per_schedule: 24,
            crash_nth_max: 2,
            storage_crash_nth_max: 2,
            btree_ops: 3000,
            checker_budget: 200_000,
            minimizer_runs: 60,
            durability_mode: "sync".into(),
            skip: vec![],
            keep_passing_bundles: false,
            model_state_budget: 3_000_000,
        }
    }
    /// The default `carolina qualify` budgets.
    pub fn standard(root: &Path, test_root: &Path) -> CampaignConfig {
        CampaignConfig {
            campaign_id: "local-standard".into(),
            seeds: vec![1, 2, 3, 4],
            ops_per_schedule: 40,
            crash_nth_max: 3,
            storage_crash_nth_max: 4,
            btree_ops: 6000,
            ..CampaignConfig::quick(root, test_root)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// Empty campaigns and zero exercise budgets cannot establish qualification.
    InvalidBudget(String),
    /// QA-07: unsafe no-fsync mode is excluded from durability qualification.
    UnsafeDurability(String),
    /// QA-10: data directories must lie under the allowlisted test root.
    TestRootOutsideAllowlist(String),
    Io(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::InvalidBudget(m) => write!(f, "invalid configuration: {m}"),
            ConfigError::UnsafeDurability(m) => write!(f, "invalid configuration (QA-07): {m}"),
            ConfigError::TestRootOutsideAllowlist(m) => {
                write!(f, "invalid configuration (QA-10): {m}")
            }
            ConfigError::Io(m) => write!(f, "invalid configuration: {m}"),
        }
    }
}

pub struct CampaignReport {
    pub manifest: QualificationManifest,
    pub verdict: Verdict,
    pub bundle_dirs: Vec<PathBuf>,
}

/// Allowlist: the OS temp dir, or `<root>/target`, or a path whose last component contains
/// `carolina` and `qualif` (an explicitly named scratch root). Resolved paths are checked, not
/// the spelling the caller passed.
pub fn validate_test_root(root: &Path, test_root: &Path) -> Result<PathBuf, ConfigError> {
    std::fs::create_dir_all(test_root)
        .map_err(|e| ConfigError::Io(format!("test root {}: {e}", test_root.display())))?;
    let resolved = std::fs::canonicalize(test_root)
        .map_err(|e| ConfigError::Io(format!("test root {}: {e}", test_root.display())))?;
    let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
    let target = std::fs::canonicalize(root.join("target")).ok();
    let cargo_target =
        std::env::var_os("CARGO_TARGET_DIR").and_then(|p| std::fs::canonicalize(p).ok());
    let last = resolved
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let named_scratch = last.contains("carolina") && last.contains("qualif");
    let under = |base: &Option<PathBuf>| {
        base.as_ref()
            .map(|b| resolved.starts_with(b))
            .unwrap_or(false)
    };
    if resolved.starts_with(&temp) || under(&target) || under(&cargo_target) || named_scratch {
        Ok(resolved)
    } else {
        Err(ConfigError::TestRootOutsideAllowlist(format!(
            "{} is not under the OS temp dir, the cargo target dir, or a *carolina*qualif* scratch dir",
            resolved.display()
        )))
    }
}

pub fn validate_config(cfg: &CampaignConfig) -> Result<PathBuf, ConfigError> {
    if cfg.seeds.is_empty() {
        return Err(ConfigError::InvalidBudget(
            "at least one seed is required".into(),
        ));
    }
    for (name, value) in [
        ("ops_per_schedule", cfg.ops_per_schedule as u64),
        ("crash_nth_max", cfg.crash_nth_max as u64),
        ("storage_crash_nth_max", cfg.storage_crash_nth_max as u64),
        ("btree_ops", cfg.btree_ops as u64),
        ("checker_budget", cfg.checker_budget as u64),
        ("model_state_budget", cfg.model_state_budget as u64),
    ] {
        if value == 0 {
            return Err(ConfigError::InvalidBudget(format!(
                "{name} must be greater than zero"
            )));
        }
    }
    match cfg.durability_mode.as_str() {
        "sync" => {}
        other => {
            return Err(ConfigError::UnsafeDurability(format!(
                "durability mode `{other}` is excluded from durability qualification; only `sync` qualifies"
            )))
        }
    }
    validate_test_root(&cfg.root, &cfg.test_root)
}

fn skipped(cfg: &CampaignConfig, id: &str) -> Option<CheckResult> {
    if cfg.skip.iter().any(|s| s == id) {
        Some(CheckResult::not_run(
            id,
            "omitted by configuration (--skip)",
        ))
    } else {
        None
    }
}

macro_rules! run_check {
    ($cfg:expr, $checks:expr, $id:expr, $body:expr) => {
        if let Some(c) = skipped($cfg, $id) {
            $checks.push(c);
        } else {
            $checks.push($body);
        }
    };
}

pub fn build_manifest(
    cfg: &CampaignConfig,
    test_root: &Path,
) -> Result<QualificationManifest, ConfigError> {
    let (tree, files) =
        working_tree_digest(&cfg.root).map_err(|e| ConfigError::Io(format!("tree digest: {e}")))?;
    let mut operation_definitions = Vec::new();
    let mut schema_hashes = Vec::new();
    let mut plan_hashes = Vec::new();
    for name in FIXTURE_NAMES {
        if let Ok((ir, _)) = load_fixture(name) {
            let h = ir.hashes();
            operation_definitions.push((name.to_string(), h.module_hash.0));
            schema_hashes.push(h.schema_hash.0);
            if let Ok(out) = compile(&CompileInput::local(ir)) {
                for p in &out.plans {
                    plan_hashes.push(p.plan_hash().0);
                }
            }
        }
    }
    let limits = Limits::v1();
    let dirty = git_dirty(&cfg.root);
    Ok(QualificationManifest {
        manifest_version: QualificationManifest::VERSION,
        source_revision: source_revision(&cfg.root),
        working_tree_digest: tree,
        working_tree_files: files,
        dirty_tree_unverified: dirty.is_none() || dirty == Some(true),
        build_profile: if cfg!(debug_assertions) {
            "debug".into()
        } else {
            "release".into()
        },
        toolchain_id: toolchain_id(&cfg.root),
        dependency_lock_digest: dependency_lock_digest(&cfg.root),
        storage_format_versions: vec![
            "page:1 (8 KiB, CRC32C)".into(),
            "journal:1".into(),
            "manifest:1".into(),
            "batch:1".into(),
        ],
        compiler_rule_versions: vec![
            format!(
                "rules:{}",
                carolina_compiler::rules::AnalysisRuleManifest::v1().hash()
            ),
            format!(
                "library:{}",
                carolina_compiler::library::ProtocolLibraryManifest::v1().hash()
            ),
        ],
        protocol_versions: vec![
            "wire:1.0".into(),
            "receipt:1".into(),
            "request_binding:1".into(),
            "txn_status:1".into(),
        ],
        supported_contract_fragment: vec![
            "WholeInvocation".into(),
            "SerialScope/LocalSnapshot visibility".into(),
            "Receipt results".into(),
            "PerKey/Global result scope".into(),
            "no session".into(),
            "LocalStable durability".into(),
        ],
        enabled_features: vec![
            "C0_LOCAL".into(),
            "C5_SERIAL(single-IDC, fixed three-voter consensus)".into(),
            "replicated RequestHome (ordered allocation)".into(),
            "catalog (genesis, grants, routes, CAS)".into(),
            "ASTR/TCP transport, DEV_LOCAL profile".into(),
            "resolve".into(),
            "result eviction".into(),
            "namespace retirement".into(),
        ],
        disabled_features: vec![
            ("C1_COMMUTATIVE".into(), "MVP-5 not started".into()),
            ("C2_CAUSAL".into(), "MVP-5 not started".into()),
            ("C3_ESCROW".into(), "MVP-6 not started".into()),
            ("C4_CERTIFIED".into(), "MVP-8 not started".into()),
            (
                "multi-IDC atomic publication".into(),
                "MVP-4 not started".into(),
            ),
            (
                "mTLS/authorization (SPEC-013)".into(),
                "not implemented; DEV_LOCAL only".into(),
            ),
            (
                "dynamic consensus membership".into(),
                "unsupported in v1 (SPEC-011 §8)".into(),
            ),
            ("plan evolution".into(), "MVP-7 not started".into()),
        ],
        operation_definitions,
        schema_hashes,
        plan_hashes,
        client_durability_profiles: vec!["LocalStable".into()],
        authority_durability_policies: vec!["local grant (single node)".into()],
        transfer_decision_durability_policies: vec![],
        failure_assumptions: vec![
            "process kill / short write inside one process".into(),
            "fsync semantics of the host filesystem are trusted".into(),
            "OS page-cache loss on power failure is NOT modelled".into(),
            "local slice: one process; C5 slice: three loopback processes with leader and majority kills".into(),
            "consensus simulation: message loss, duplication, delay and directional partitions".into(),
            "no independent machine, disk or region failure domains are qualified".into(),
        ],
        formal_model_refs: vec![
            "models/FM1_Escrow.tla + crates/carolina-models/src/fm1.rs".into(),
            "models/FM2_Decision.tla + crates/carolina-models/src/fm2.rs".into(),
            "models/FM3_Migration.tla + crates/carolina-models/src/fm3.rs".into(),
        ],
        formal_bounds: vec![
            "FM-1: T=3, transfers q=[1,2], donor close, stale replica".into(),
            "FM-2: 2 participants, leader epochs 1..2".into(),
            "FM-3: 2 sources, 2 workers, <=2 old admissions".into(),
            format!("state budget {} per model", cfg.model_state_budget),
        ],
        refinement_mapping_refs: vec![],
        catalog_capabilities: vec![
            "fixed three-voter catalog: genesis, CAS commands, grants, routes, scope locks".into(),
        ],
        identity_codec_manifest: carolina_wire::snapshot::CodecManifest::v1().manifest_hash(),
        security_profile: "DEV_LOCAL (plaintext loopback TCP; no mTLS or authentication)".into(),
        topology: "local slice: one process; C5 slice: three processes on loopback with isolated data directories".into(),
        authority_configuration: "explicit local grant (local slice); replicated home grant over a three-voter authority (C5 slice)".into(),
        placement: "single machine".into(),
        resource_limits: vec![
            ("max_payload_bytes".into(), limits.max_payload_bytes as u64),
            ("max_depth".into(), limits.max_depth as u64),
            (
                "max_collection_len".into(),
                limits.max_collection_len as u64,
            ),
            ("pool_frames".into(), 64),
        ],
        timeout_and_retry_policy: "local retries are explicit schedule steps; C5 clients use bounded transport timeouts and resolve the original request identity".into(),
        seed_set: cfg.seeds.clone(),
        schedules: vec![
            "w1-generated".into(),
            "storage-crash-matrix".into(),
            "btree-differential".into(),
            "consensus-simulation".into(),
            "three-process-c5".into(),
        ],
        checker_versions: vec![CHECKER_VERSION.into(), "storage-campaign/0.1.0".into()],
        run_budget: RunBudget {
            ops_per_schedule: cfg.ops_per_schedule as u64,
            crash_nth_max: cfg.crash_nth_max as u64,
            storage_crash_nth_max: cfg.storage_crash_nth_max as u64,
            checker_budget: cfg.checker_budget as u64,
            minimizer_runs: cfg.minimizer_runs as u64,
            btree_ops: cfg.btree_ops as u64,
        },
        workload_definitions: vec![
            "W1 inventory/reservations (fixtures/dsl/inventory_reserve_release.cdl)".into(),
        ],
        load_sweep: vec![],
        baseline_configs: vec![],
        durability_mode: cfg.durability_mode.clone(),
        test_root: test_root.to_string_lossy().to_string(),
    })
}

/// `Some(true)` when git reports modifications, `Some(false)` when clean, `None` when git is not
/// usable here (then the tree digest alone identifies the source).
fn git_dirty(root: &Path) -> Option<bool> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(!out.stdout.iter().all(|b| b.is_ascii_whitespace()))
}

// ---------------------------------------------------------------------------
// Q0 — formal core
// ---------------------------------------------------------------------------

fn q0_golden(root: &Path) -> Result<String, String> {
    let dir = root.join("fixtures").join("golden");
    let mut n = 0;
    for name in FIXTURE_NAMES {
        let (ir, _) = load_fixture(name).map_err(|e| format!("{name}: {e}"))?;
        let golden = std::fs::read(dir.join(format!("{name}.module.json")))
            .map_err(|e| format!("{name}: golden bytes missing: {e}"))?;
        if golden != ir.encode() {
            return Err(format!("{name}: IR bytes differ from the golden corpus"));
        }
        let golden_h = std::fs::read_to_string(dir.join(format!("{name}.hashes.txt")))
            .map_err(|e| format!("{name}: golden hashes missing: {e}"))?
            .replace("\r\n", "\n");
        if golden_h != render_hashes(&ir.hashes()) {
            return Err(format!("{name}: hashes differ from the golden corpus"));
        }
        let back = carolina_lang::ir::ModuleIR::decode(&golden, &Limits::v1())
            .map_err(|e| format!("{name}: golden decode: {e}"))?;
        if back.encode() != golden {
            return Err(format!(
                "{name}: golden bytes are not a fixed point of decode/encode"
            ));
        }
        n += 1;
    }
    Ok(format!(
        "{n} fixtures match fixtures/golden (bytes + hashes)"
    ))
}

fn q0_determinism() -> Result<String, String> {
    let mut plans = 0;
    let mut tampered = 0;
    for name in FIXTURE_NAMES {
        let (ir, _) = load_fixture(name).map_err(|e| format!("{name}: {e}"))?;
        let a = compile(&CompileInput::local(ir.clone()))
            .map_err(|e| format!("{name}: compile: {e}"))?;
        let b =
            compile(&CompileInput::local(ir)).map_err(|e| format!("{name}: compile again: {e}"))?;
        if a.certificate.certificate_hash() != b.certificate.certificate_hash() {
            return Err(format!(
                "{name}: certificate hash differs between two compilations"
            ));
        }
        let ha: Vec<_> = a.plans.iter().map(|p| p.plan_hash()).collect();
        let hb: Vec<_> = b.plans.iter().map(|p| p.plan_hash()).collect();
        if ha != hb {
            return Err(format!(
                "{name}: plan hashes differ between two compilations"
            ));
        }
        let rules = carolina_compiler::rules::AnalysisRuleManifest::v1();
        let lib = carolina_compiler::library::ProtocolLibraryManifest::v1();
        check_artifacts(&a.plans, &a.certificate, &rules, &lib)
            .map_err(|e| format!("{name}: checker refused genuine artifacts: {e}"))?;
        plans += a.plans.len();
        // tampering: a flipped byte in a plan must not pass the checker
        if let Some(p0) = a.plans.first() {
            let mut bytes = p0.encode();
            let idx = bytes.len() / 2;
            bytes[idx] = if bytes[idx] == b'1' { b'2' } else { b'1' };
            match OperationPlan::decode(&bytes, &Limits::v1()) {
                Err(_) => tampered += 1,
                Ok(p) => {
                    let mut ps = a.plans.clone();
                    ps[0] = p;
                    if check_artifacts(&ps, &a.certificate, &rules, &lib).is_ok() && ps[0] != *p0 {
                        return Err(format!("{name}: tampered plan passed the artifact checker"));
                    }
                    tampered += 1;
                }
            }
        }
    }
    Ok(format!(
        "{} fixtures, {plans} plans deterministic; {tampered} tampered plans refused",
        FIXTURE_NAMES.len()
    ))
}

/// The independent evaluator finds the classic last-unit race and stays quiet when there is
/// room (negative control), and the compiler's Disproven judgments carry replayable bytes.
fn q0_reference_evaluator() -> Result<String, String> {
    let (ir, _) = load_fixture("inventory_reserve_release").map_err(|e| e.to_string())?;
    let reserve = ir
        .operation_by_name("reserve")
        .ok_or("reserve missing")?
        .identity;
    let item = Value::Uuid([7u8; 16]);
    let mut pre = State::default();
    pre.put_row(
        &ir,
        "Item",
        &[
            ("id", item.clone()),
            ("available", Value::I64(1)),
            ("reserved", Value::I64(0)),
            ("total", Value::I64(1)),
        ],
    )
    .map_err(|e| e.to_string())?;
    let invs = vec![
        Invocation {
            operation: reserve,
            arguments: vec![item.clone(), Value::I64(1), Value::Uuid([1; 16])],
        },
        Invocation {
            operation: reserve,
            arguments: vec![item.clone(), Value::I64(1), Value::Uuid([2; 16])],
        },
    ];
    let cx = explore_concurrent(&ir, &pre, &invs)
        .map_err(|e| e.to_string())?
        .ok_or("evaluator missed the last-unit race")?;
    if !matches!(cx.failure, Failure::InvariantViolated { .. }) {
        return Err(format!("unexpected failure kind {:?}", cx.failure));
    }
    if !replay(&ir, &cx).map_err(|e| e.to_string())? {
        return Err("counterexample does not replay".into());
    }
    let mut roomy = State::default();
    roomy
        .put_row(
            &ir,
            "Item",
            &[
                ("id", item.clone()),
                ("available", Value::I64(2)),
                ("reserved", Value::I64(0)),
                ("total", Value::I64(2)),
            ],
        )
        .map_err(|e| e.to_string())?;
    if explore_concurrent(&ir, &roomy, &invs)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("negative control: evaluator reported a race with enough stock".into());
    }
    let mut disproven = 0;
    for name in FIXTURE_NAMES {
        let (ir, _) = load_fixture(name).map_err(|e| e.to_string())?;
        let out = compile(&CompileInput::local(ir)).map_err(|e| e.to_string())?;
        for c in &out.candidates {
            for j in &c.judgments {
                if let ProofStatus::Disproven { counterexample, .. } = &j.status {
                    carolina_core::canon::CanonValue::decode(counterexample, &Limits::v1())
                        .map_err(|e| format!("{name}: counterexample bytes not canonical: {e}"))?;
                    disproven += 1;
                }
            }
        }
    }
    Ok(format!("last-unit race found and replayed; negative control quiet; {disproven} compiler counterexamples canonical"))
}

// ---------------------------------------------------------------------------
// Campaign
// ---------------------------------------------------------------------------

pub fn run_campaign(cfg: &CampaignConfig) -> Result<CampaignReport, ConfigError> {
    let test_root = validate_config(cfg)?;
    let manifest = build_manifest(cfg, &test_root)?;
    let manifest_hash = manifest.manifest_hash();
    let mut checks: Vec<CheckResult> = Vec::new();
    let mut metrics: BTreeMap<String, u64> = BTreeMap::new();
    let mut bundle_dirs = Vec::new();
    let mut trace_parts: Vec<Hash256> = Vec::new();

    // ---- Q0
    run_check!(
        cfg,
        checks,
        "Q0-IR-GOLDEN",
        CheckResult::from_result(
            "Q0-IR-GOLDEN",
            "fixtures/golden",
            q0_golden(&cfg.root),
            |s| s.clone()
        )
    );
    run_check!(
        cfg,
        checks,
        "Q0-DETERMINISM",
        CheckResult::from_result(
            "Q0-DETERMINISM",
            "all fixtures, local topology",
            q0_determinism(),
            |s| s.clone()
        )
    );
    run_check!(
        cfg,
        checks,
        "Q0-REFERENCE-EVALUATOR",
        CheckResult::from_result(
            "Q0-REFERENCE-EVALUATOR",
            "inventory_reserve_release last-unit race",
            q0_reference_evaluator(),
            |s| s.clone()
        )
    );

    // ---- Q1 storage
    let btree_scope = format!(
        "seeds 1..3, {} ops each, pools 16/64/128, hot key 3000 versions",
        cfg.btree_ops
    );
    run_check!(cfg, checks, "Q1-P7-BTREE", {
        let r = (|| -> Result<String, String> {
            let a = storage::btree_differential(1, cfg.btree_ops, 16, 400)?;
            let b = storage::btree_differential(2, cfg.btree_ops, 64, 40)?;
            let c = storage::btree_differential(3, cfg.btree_ops, 128, 5000)?;
            storage::btree_hot_key(3000)?;
            let splits = a.detail.get("splits").copied().unwrap_or(0)
                + b.detail.get("splits").copied().unwrap_or(0)
                + c.detail.get("splits").copied().unwrap_or(0);
            Ok(format!(
                "{} tree ops, {splits} splits, reference scans agree (P7/P4)",
                a.cases + b.cases + c.cases
            ))
        })();
        CheckResult::from_result("Q1-P7-BTREE", btree_scope.clone(), r, |s| s.clone())
    });
    let mut checkpoint_crashes = 0u64;
    run_check!(cfg, checks, "Q1-CRASH-MATRIX", {
        let r = storage::crash_matrix(cfg.storage_crash_nth_max);
        if let Ok(s) = &r {
            checkpoint_crashes = s.detail.get("DuringCheckpoint").copied().unwrap_or(0)
                + s.detail
                    .get("AfterManifestWriteBeforeFsync")
                    .copied()
                    .unwrap_or(0)
                + s.detail.get("BeforeManifestPublish").copied().unwrap_or(0);
            metrics.insert("storage.crash_cases_exercised".into(), s.exercised);
        }
        CheckResult::from_result(
            "Q1-CRASH-MATRIX",
            format!(
                "{} fault points × nth 1..={}; process-kill/short-write model",
                ALL_FAULT_POINTS.len(),
                cfg.storage_crash_nth_max
            ),
            r,
            |s| {
                format!(
                    "P1/P2/P3/P6/P9 held in {} crashing cases of {} scheduled: {:?}",
                    s.exercised, s.cases, s.detail
                )
            },
        )
    });
    run_check!(
        cfg,
        checks,
        "Q1-CORRUPTION",
        CheckResult::from_result(
            "Q1-CORRUPTION",
            "torn tail, mid-log corruption, injected I/O error",
            (|| -> Result<String, String> {
                storage::torn_tail_and_mid_log_corruption()?;
                storage::io_error_never_yields_success()?;
                Ok("torn tail discarded; corrupted committed frame fails closed; I/O error never acknowledged".into())
            })(),
            |s| s.clone()
        )
    );
    run_check!(cfg, checks, "Q1-P8-CHECKPOINT", {
        if cfg.skip.iter().any(|s| s == "Q1-CRASH-MATRIX")
            || checks
                .iter()
                .any(|c| c.id == "Q1-CRASH-MATRIX" && c.status != Status::Pass)
        {
            CheckResult::not_run("Q1-P8-CHECKPOINT", "depends on Q1-CRASH-MATRIX")
        } else if checkpoint_crashes == 0 {
            CheckResult::inconclusive(
                "Q1-P8-CHECKPOINT",
                "checkpoint-stage crashes",
                "no checkpoint-stage crash was exercised at this budget",
            )
        } else {
            CheckResult::pass("Q1-P8-CHECKPOINT", "checkpoint-stage crashes inside the matrix", format!("{checkpoint_crashes} crashes during checkpoint/manifest stages recovered to the durable journal state; pages are written only after the journal barrier"))
        }
    });

    // ---- Q2 local prepared/metadata
    run_check!(
        cfg,
        checks,
        "Q2-P9-PREPARED",
        CheckResult::from_result(
            "Q2-P9-PREPARED",
            "prepare/commit_prepared/abort_prepared with checkpoint + reopen",
            storage::prepared_invisibility(),
            |_| {
                "prepared data invisible until a durable decision; duplicates refused; IN_DOUBT reported".into()
            }
        )
    );
    run_check!(cfg, checks, "Q2-P10-EPOCH", CheckResult::from_result("Q2-P10-EPOCH", "storage epoch advance; old snapshots refused (narrow: no restored-backup authority test)", storage::stale_epoch_is_refused(), |_| "old-epoch snapshot refused with StaleEpoch; epoch change durable".into()));
    run_check!(
        cfg,
        checks,
        "Q2-KERNEL-DIFFERENTIAL",
        CheckResult::from_result(
            "Q2-KERNEL-DIFFERENTIAL",
            "seed 11, 300 batches, business rows + txn statuses",
            storage::kernel_differential(11, 300),
            |s| format!(
                "Store and MemKernel agree over {} batches (P3/P6 atomic metadata)",
                s.cases
            )
        )
    );
    run_check!(
        cfg,
        checks,
        "Q2-ADMISSION",
        CheckResult::from_result(
            "Q2-ADMISSION",
            "stale plan/schema",
            storage::stale_admission_is_refused(),
            |_| "stale plan and schema refused before persistence".into()
        )
    );

    // ---- Q2 W1 local campaign with the independent oracle and the history checker
    let catalog_src = fixture_source("inventory_reserve_release").unwrap();
    let catalog = std::sync::Arc::new(
        carolina_runtime::LocalCatalog::from_source(catalog_src)
            .map_err(|e| ConfigError::Io(format!("catalog: {e}")))?,
    );
    let mut w1_runs = 0u64;
    let mut w1_fail: Option<String> = None;
    let mut w1_inconclusive: Option<String> = None;
    let mut w1_metrics = BTreeMap::new();
    if !cfg.skip.iter().any(|s| s == "Q2-W1-LOCAL") {
        let mut schedules: Vec<Schedule> = Vec::new();
        for seed in &cfg.seeds {
            schedules.push(generate_schedule(*seed, cfg.ops_per_schedule, None));
            for point in ALL_FAULT_POINTS {
                for nth in 1..=cfg.crash_nth_max {
                    schedules.push(generate_schedule(
                        *seed,
                        cfg.ops_per_schedule,
                        Some((point, nth)),
                    ));
                }
            }
        }
        'runs: for sched in &schedules {
            w1_runs += 1;
            let outcome = match run_schedule(&test_root, &catalog, sched) {
                Ok(o) => o,
                Err(e) => {
                    w1_fail = Some(format!(
                        "seed {} fault {:?}: runner error: {e}",
                        sched.seed, sched.fault
                    ));
                    break 'runs;
                }
            };
            trace_parts.push(outcome.history.trace_digest());
            for (k, v) in outcome.metrics.to_map("w1") {
                *w1_metrics.entry(k).or_insert(0) += v;
            }
            let results = check(&CheckerInput {
                history: &outcome.history,
                initial: &outcome.initial,
                final_state: outcome.final_state.as_ref(),
                budget: cfg.checker_budget,
            });
            let mut problem: Option<(Status, String)> = None;
            if let Some(v) = &outcome.verify_error {
                problem = Some((
                    Status::Fail,
                    format!("storage verifier after recovery: {v}"),
                ));
            } else if any_failure(&results) {
                let f: Vec<String> = results
                    .iter()
                    .filter(|r| r.status == Status::Fail)
                    .map(|r| format!("{}: {}", r.id, r.detail))
                    .collect();
                problem = Some((Status::Fail, f.join(" || ")));
            } else if results.iter().any(|r| r.status == Status::Inconclusive) {
                let d = results
                    .iter()
                    .find(|r| r.status == Status::Inconclusive)
                    .map(|r| r.detail.clone())
                    .unwrap_or_default();
                problem = Some((Status::Inconclusive, d));
            }
            let write = problem.is_some() || cfg.keep_passing_bundles;
            if let (Some(out), true) = (&cfg.out_dir, write) {
                let run_id = format!(
                    "w1-{}-{}",
                    sched.seed,
                    sched
                        .fault
                        .map(|(p, n)| format!("{p:?}-{n}"))
                        .unwrap_or_else(|| "nofault".into())
                );
                let mut v = Verdict {
                    verdict_version: Verdict::VERSION,
                    campaign_id: cfg.campaign_id.clone(),
                    run_id,
                    manifest_hash,
                    trace_digest: outcome.history.trace_digest(),
                    checks: results.clone(),
                    gates: vec![],
                    metrics: outcome.metrics.to_map("w1"),
                    claimed_gates: vec![],
                };
                if let Some((st, d)) = &problem {
                    v.checks.push(CheckResult {
                        id: "RUN".into(),
                        status: *st,
                        scope: "single schedule".into(),
                        detail: d.clone(),
                        evidence: vec![],
                    });
                }
                let repro = format!(
                    "# Reproduction\n\n```bash\ncarolina simulate --seed {}{} --ops {}\n```\n\nThe schedule is `schedule.json`; the runner is deterministic for the same seed, schedule and fault.\nRe-check with `carolina replay --bundle <this dir>`; shrink with `carolina minimize --bundle <this dir>`.\n",
                    sched.seed,
                    sched.fault.map(|(p, n)| format!(" --fault {p:?}#{n}")).unwrap_or_default(),
                    cfg.ops_per_schedule
                );
                let bi = BundleInput {
                    manifest: &manifest,
                    verdict: &v,
                    history: Some(&outcome.history),
                    schedule: Some(sched),
                    initial: Some(&outcome.initial),
                    final_state: outcome.final_state.as_ref(),
                    contracts: vec![(
                        "inventory_reserve_release.cdl".into(),
                        catalog_src.as_bytes().to_vec(),
                    )],
                    plans: vec![],
                    evidence: vec![],
                    metrics: &v.metrics,
                    reproduction: repro,
                };
                match write_bundle(out, &bi) {
                    Ok(d) => bundle_dirs.push(d),
                    Err(e) => {
                        w1_fail = Some(format!("cannot write bundle: {e}"));
                        break 'runs;
                    }
                }
            }
            match problem {
                Some((Status::Fail, d)) => {
                    w1_fail = Some(format!("seed {} fault {:?}: {d}", sched.seed, sched.fault));
                    break 'runs;
                }
                Some((_, d)) => {
                    w1_inconclusive
                        .get_or_insert(format!("seed {} fault {:?}: {d}", sched.seed, sched.fault));
                }
                None => {}
            }
        }
        for (k, v) in &w1_metrics {
            metrics.insert(k.clone(), *v);
        }
        metrics.insert("w1.runs".into(), w1_runs);
        let scope = format!(
            "seeds {:?}, {} ops/schedule, {} fault points × nth 1..={}, checker budget {}",
            cfg.seeds,
            cfg.ops_per_schedule,
            ALL_FAULT_POINTS.len(),
            cfg.crash_nth_max,
            cfg.checker_budget
        );
        // SPEC-014 §4: usefulness is measured separately from safety. A campaign that refuses or
        // rejects every invocation satisfies the checker vacuously, so it can never be a PASS.
        let committed = *w1_metrics.get("w1.committed").unwrap_or(&0);
        let attempts = *w1_metrics.get("w1.attempts").unwrap_or(&0);
        checks.push(match (w1_fail, w1_inconclusive) {
            (Some(f), _) => CheckResult::fail("Q2-W1-LOCAL", scope, f),
            (None, Some(i)) => CheckResult::inconclusive("Q2-W1-LOCAL", scope, i),
            (None, None) if committed == 0 => CheckResult::fail(
                "Q2-W1-LOCAL",
                scope,
                format!("no useful progress: 0 of {attempts} invocations committed over {w1_runs} schedules (SPEC-014 §4)"),
            ),
            (None, None) => CheckResult::pass("Q2-W1-LOCAL", scope, format!("{w1_runs} schedules, {committed} of {attempts} invocations committed: Q-C01/02/03/04/05/13/14 held against the independent W1 oracle; every receipt resolved identically after recovery")),
        });
    } else {
        checks.push(CheckResult::not_run(
            "Q2-W1-LOCAL",
            "omitted by configuration (--skip)",
        ));
    }
    // QA-01 determinism of the local runner
    run_check!(cfg, checks, "Q2-DETERMINISM", {
        let seed = cfg.seeds.first().copied().unwrap_or(1);
        let s = generate_schedule(
            seed,
            cfg.ops_per_schedule.min(16),
            Some((ALL_FAULT_POINTS[3], 2)),
        );
        let r = (|| -> Result<String, String> {
            let a = run_schedule(&test_root, &catalog, &s)?;
            let b = run_schedule(&test_root, &catalog, &s)?;
            if a.history.trace_digest() != b.history.trace_digest() {
                return Err(
                    "two runs of the same seed/schedule/fault produced different trace digests"
                        .into(),
                );
            }
            Ok(format!(
                "identical trace digest {} over {} events",
                a.history.trace_digest(),
                a.history.len()
            ))
        })();
        CheckResult::from_result(
            "Q2-DETERMINISM",
            format!("seed {seed}, fault {:?}#2", ALL_FAULT_POINTS[3]),
            r,
            |s| s.clone(),
        )
    });

    // ---- QI / Q3+ / FM
    run_check!(cfg, checks, "QI-CODEC-CORPUS", CheckResult::from_result(
        "QI-CODEC-CORPUS",
        "fixtures/codec: frozen canonical bytes + digests for every codec used by the enabled slices, decode/encode fixed point, negative vectors",
        crate::codec_corpus::check(&cfg.root, false),
        |s| format!(
            "{} vectors frozen ({} registered kinds), {} negative vectors refused; registered kinds without an implemented codec (disabled features): {}",
            s.frozen,
            s.kinds_frozen.len(),
            s.negatives,
            s.kinds_without_codec.join(", ")
        ),
    ));
    checks.push(CheckResult::not_run(
        "QI-SECURITY",
        "SPEC-013 mTLS/authorization not implemented; DEV_LOCAL profile only",
    ));
    run_check!(cfg, checks, "QI-CATALOG", CheckResult::from_result(
        "QI-CATALOG",
        "SPEC-011 §11 subset CAT-01/02/05/12/15/16 + closed-never-reopens on the catalog state machine",
        carolina_catalog::acceptance_campaign(),
        |p| format!("{} scenarios held: {}", p.len(), p.join(", ")),
    ));
    // Q3 — single-IDC C5 slice: deterministic consensus simulation + real three-process campaign
    run_check!(cfg, checks, "Q3-CONSENSUS-SIM", CheckResult::from_result(
        "Q3-CONSENSUS-SIM",
        format!("seeds {:?}, 20 proposals each, loss/dup/delay, crash+restart of every voter, isolated leader, replay determinism", cfg.seeds),
        carolina_consensus::sim::campaign(&cfg.seeds, 20),
        |m| format!("election safety, log matching, state-machine safety and leader completeness held; {m:?}"),
    ));
    run_check!(cfg, checks, "Q3-C5-PROCESS", {
        match carolina_node::campaign::find_node_binary() {
            None => CheckResult::not_run("Q3-C5-PROCESS", "carolina-node binary not found next to the runner (build with `cargo build -p carolina-node` or set CAROLINA_NODE_BIN)"),
            Some(bin) => {
                let root = test_root.join("c5-process");
                let r = carolina_node::campaign::three_process_c5(&bin, &root);
                if let Ok(s) = &r {
                    for (k, v) in &s.metrics {
                        metrics.insert(format!("c5.{k}"), *v);
                    }
                }
                CheckResult::from_result(
                    "Q3-C5-PROCESS",
                    "three carolina-node processes on loopback, isolated data directories, real process kills (leader, then a majority), restart + replay",
                    r,
                    |s| format!("C5-001/009/010/021 + replicated determinism: {}", s.checks.join(" | ")),
                )
            }
        }
    });
    for (id, why) in [
        (
            "Q3-PROTOCOLS",
            "C1/C2/C3 slices not implemented (MVP-5/6); only the single-IDC C5 slice exists (gate Q3-C5)",
        ),
        (
            "Q4-COMPOSITION",
            "multi-IDC publication not implemented (MVP-4)",
        ),
        ("Q5-EVOLUTION", "SPEC-009 evolution not implemented (MVP-7)"),
        (
            "Q6-CERTIFICATION",
            "C4 disabled and not implemented (MVP-8)",
        ),
        (
            "Q7-EVALUATION",
            "E1-E5 need pinned baselines and the distributed slices",
        ),
    ] {
        checks.push(CheckResult::not_run(id, why));
    }
    // formal model targets: explicit-state checkers with negative controls (bounded evidence for
    // the MODEL; the protocols themselves are not implemented, so Q3+ stay NOT_RUN)
    for ev in carolina_models::all_gates(cfg.model_state_budget) {
        let id = ev.gate;
        if let Some(c) = skipped(cfg, id) {
            checks.push(c);
            continue;
        }
        let scope = format!(
            "explicit-state BFS (carolina-models {}), bounds: {}; TLA+ source in models/ not run by TLC",
            env!("CARGO_PKG_VERSION"),
            ev.positive.bounds
        );
        checks.push(match ev.verdict() {
            Ok(d) => CheckResult::pass(
                id,
                scope,
                format!("model invariants hold within bounds; {d}"),
            ),
            Err(e) => {
                if ev.positive.truncated {
                    CheckResult::inconclusive(id, scope, e)
                } else {
                    CheckResult::fail(id, scope, e)
                }
            }
        });
        metrics.insert(format!("{id}.states"), ev.positive.states as u64);
    }

    let gates = vec![
        derive_gate(
            "Q0",
            &["Q0-IR-GOLDEN", "Q0-DETERMINISM", "Q0-REFERENCE-EVALUATOR"],
            &checks,
        ),
        derive_gate(
            "Q1",
            &[
                "Q1-P7-BTREE",
                "Q1-CRASH-MATRIX",
                "Q1-CORRUPTION",
                "Q1-P8-CHECKPOINT",
            ],
            &checks,
        ),
        derive_gate(
            "Q2",
            &[
                "Q2-P9-PREPARED",
                "Q2-P10-EPOCH",
                "Q2-KERNEL-DIFFERENTIAL",
                "Q2-ADMISSION",
                "Q2-W1-LOCAL",
                "Q2-DETERMINISM",
            ],
            &checks,
        ),
        derive_gate(
            "QI",
            &["QI-CODEC-CORPUS", "QI-SECURITY", "QI-CATALOG"],
            &checks,
        ),
        derive_gate("FM", &["FM-1", "FM-2", "FM-3"], &checks),
        derive_gate(
            "Q3-C5",
            &["QI-CATALOG", "Q3-CONSENSUS-SIM", "Q3-C5-PROCESS", "FM-2"],
            &checks,
        ),
        derive_gate("Q3", &["Q3-PROTOCOLS", "FM-1", "FM-2"], &checks),
        derive_gate("Q4", &["Q4-COMPOSITION", "FM-2"], &checks),
        derive_gate("Q5", &["Q5-EVOLUTION", "FM-3"], &checks),
        derive_gate("Q6", &["Q6-CERTIFICATION"], &checks),
        derive_gate("Q7", &["Q7-EVALUATION"], &checks),
    ];
    let mut trace_payload = Vec::new();
    for t in &trace_parts {
        trace_payload.extend_from_slice(&t.0);
    }
    let trace_digest = sha256(&trace_payload);
    let run_id = format!(
        "run-{}",
        &hex_encode(&sha256(&[manifest_hash.0.as_slice(), trace_digest.0.as_slice()].concat()).0)
            [..16]
    );
    let verdict = Verdict {
        verdict_version: Verdict::VERSION,
        campaign_id: cfg.campaign_id.clone(),
        run_id,
        manifest_hash,
        trace_digest,
        checks,
        gates,
        metrics,
        claimed_gates: vec![
            "Q0".into(),
            "Q1".into(),
            "Q2".into(),
            "FM".into(),
            "Q3-C5".into(),
        ],
    };
    if let Some(out) = &cfg.out_dir {
        let bi = BundleInput {
            manifest: &manifest,
            verdict: &verdict,
            history: None,
            schedule: None,
            initial: None,
            final_state: None,
            contracts: vec![("inventory_reserve_release.cdl".into(), catalog_src.as_bytes().to_vec())],
            plans: vec![],
            evidence: vec![],
            metrics: &verdict.metrics,
            reproduction: format!("# Reproduction\n\n```bash\ncarolina qualify --seeds {} --ops {} --crash-nth {} --out <dir>\n```\n", cfg.seeds.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(","), cfg.ops_per_schedule, cfg.crash_nth_max),
        };
        match write_bundle(out, &bi) {
            Ok(d) => {
                let _ = std::fs::write(d.join("report.txt"), verdict.summary());
                bundle_dirs.push(d);
            }
            Err(e) => {
                return Err(ConfigError::Io(format!(
                    "cannot write campaign bundle: {e}"
                )))
            }
        }
    }
    Ok(CampaignReport {
        manifest,
        verdict,
        bundle_dirs,
    })
}

/// Re-run one schedule and check it (used by `carolina replay`/`minimize`).
pub fn local_catalog() -> Result<std::sync::Arc<carolina_runtime::LocalCatalog>, String> {
    let src = fixture_source("inventory_reserve_release").unwrap();
    Ok(std::sync::Arc::new(
        carolina_runtime::LocalCatalog::from_source(src).map_err(|e| format!("catalog: {e}"))?,
    ))
}

pub fn check_schedule(
    test_root: &Path,
    sched: &Schedule,
    budget: usize,
) -> Result<(History, Vec<CheckResult>, Option<String>), String> {
    let catalog = local_catalog()?;
    let outcome = run_schedule(test_root, &catalog, sched)?;
    let results = check(&CheckerInput {
        history: &outcome.history,
        initial: &outcome.initial,
        final_state: outcome.final_state.as_ref(),
        budget,
    });
    Ok((outcome.history, results, outcome.verify_error))
}
