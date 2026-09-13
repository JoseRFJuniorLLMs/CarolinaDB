use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use carolina_core::canon::Canonical;
use carolina_qualify::bundle::{load_bundle, write_bundle, BundleInput};
use carolina_qualify::local::generate_schedule;
use carolina_qualify::runner::{build_manifest, check_schedule, CampaignConfig};
use carolina_qualify::verdict::Verdict;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "carolina-qualify-cli-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_carolina"))
}

#[test]
fn qualification_rejects_malformed_options_and_empty_budgets() {
    for args in [
        vec!["--seeds", "invalid"],
        vec!["--seeds", "1,invalid,2"],
        vec!["--seeds", "1,"],
        vec!["--seeds", ""],
        vec!["--seeds"],
        vec!["--seeds", "--quick"],
        vec!["--seed", "1"],
        vec!["--ops", "0"],
        vec!["--ops", "bad"],
        vec!["--crash-nth", "0"],
        vec!["--ops", "1", "--ops", "2"],
        vec!["--skip"],
    ] {
        let output = command().arg("qualify").args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
}

fn bundle(root: &Path, budget: usize) -> PathBuf {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut cfg = CampaignConfig::quick(&repo, root);
    cfg.checker_budget = budget;
    let manifest = build_manifest(&cfg, root).unwrap();
    let schedule = generate_schedule(7, 8, None);
    let (history, checks, err) = check_schedule(root, &schedule, 200_000).unwrap();
    assert!(err.is_none());
    let verdict = Verdict {
        verdict_version: Verdict::VERSION,
        campaign_id: "cli-regression".into(),
        run_id: "run".into(),
        manifest_hash: manifest.manifest_hash(),
        trace_digest: history.trace_digest(),
        checks,
        gates: vec![],
        metrics: BTreeMap::new(),
        claimed_gates: vec![],
    };
    write_bundle(root, &BundleInput {
        manifest: &manifest, verdict: &verdict, history: Some(&history), schedule: Some(&schedule),
        initial: None, final_state: None, contracts: vec![], plans: vec![], evidence: vec![],
        metrics: &verdict.metrics, reproduction: String::new(),
    }).unwrap()
}

#[test]
fn replay_and_minimize_do_not_report_inconclusive_as_success() {
    let tr = Scratch::new();
    let dir = bundle(&tr.0, 1);
    for subcommand in ["replay", "minimize"] {
        let output = command().arg(subcommand).arg("--bundle").arg(&dir)
            .arg("--test-root").arg(&tr.0).output().unwrap();
        assert_eq!(output.status.code(), Some(3), "{subcommand}: {} {}",
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
fn bundles_verify_manifest_and_history_binding_and_replay_determinism() {
    let tr = Scratch::new();
    let dir = bundle(&tr.0, 200_000);
    let original = load_bundle(&dir).unwrap();
    let mut manifest = original.manifest.clone();
    manifest.seed_set = vec![99];
    std::fs::write(dir.join("manifest.json"), manifest.encode()).unwrap();
    assert!(load_bundle(&dir).err().unwrap().contains("manifest hash"));
    std::fs::write(dir.join("manifest.json"), original.manifest.encode()).unwrap();
    std::fs::write(dir.join("history.jsonl"), "").unwrap();
    assert!(load_bundle(&dir).err().unwrap().contains("trace digest"));
    std::fs::write(dir.join("history.jsonl"), original.history.unwrap().to_jsonl()).unwrap();

    let pass = command().arg("replay").arg("--bundle").arg(&dir)
        .arg("--test-root").arg(&tr.0).output().unwrap();
    assert!(pass.status.success(), "{} {}", String::from_utf8_lossy(&pass.stdout), String::from_utf8_lossy(&pass.stderr));
    // A different valid schedule still passes the oracle, but cannot reproduce the retained trace.
    std::fs::write(dir.join("schedule.json"), generate_schedule(9, 8, None).encode()).unwrap();
    let changed = command().arg("replay").arg("--bundle").arg(&dir)
        .arg("--test-root").arg(&tr.0).output().unwrap();
    assert_eq!(changed.status.code(), Some(1), "{}", String::from_utf8_lossy(&changed.stdout));
}
