//! A retained bundle must not lose its trace binding or mix evidence from different runs.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use carolina_core::canon::Canonical;
use carolina_qualify::bundle::{load_bundle, write_bundle, BundleInput};
use carolina_qualify::history::History;
use carolina_qualify::local::{generate_schedule, Schedule};
use carolina_qualify::manifest::QualificationManifest;
use carolina_qualify::runner::{build_manifest, CampaignConfig};
use carolina_qualify::verdict::Verdict;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "carolina-bundle-integrity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    manifest: QualificationManifest,
    verdict: Verdict,
    history: History,
    schedule: Schedule,
}

impl Fixture {
    fn new(root: &std::path::Path) -> Self {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cfg = CampaignConfig::quick(&repo, root);
        let manifest = build_manifest(&cfg, root).unwrap();
        let history = History::default();
        let verdict = Verdict {
            verdict_version: Verdict::VERSION,
            campaign_id: "integrity".into(),
            run_id: "same-run".into(),
            manifest_hash: manifest.manifest_hash(),
            trace_digest: history.trace_digest(),
            checks: vec![],
            gates: vec![],
            metrics: BTreeMap::new(),
            claimed_gates: vec![],
        };
        Self {
            manifest,
            verdict,
            history,
            schedule: generate_schedule(7, 8, None),
        }
    }

    fn input(&self) -> BundleInput<'_> {
        BundleInput {
            manifest: &self.manifest,
            verdict: &self.verdict,
            history: Some(&self.history),
            schedule: Some(&self.schedule),
            initial: None,
            final_state: None,
            contracts: vec![],
            plans: vec![],
            evidence: vec![],
            metrics: &self.verdict.metrics,
            reproduction: "reproduce this run".into(),
        }
    }
}

#[test]
fn schedule_cannot_silently_downgrade_to_missing_history() {
    let scratch = Scratch::new();
    let fixture = Fixture::new(&scratch.0);
    let dir = write_bundle(&scratch.0, &fixture.input()).unwrap();
    assert!(load_bundle(&dir).is_ok());
    std::fs::rename(dir.join("history.jsonl"), dir.join("history.saved")).unwrap();
    let error = load_bundle(&dir).err().expect("missing trace was accepted");
    assert!(error.contains("history.jsonl"), "{error}");
}

#[test]
fn incomplete_schedule_bundle_is_rejected_before_creating_output() {
    let scratch = Scratch::new();
    let fixture = Fixture::new(&scratch.0);
    let out = scratch.0.join("not-created");
    let mut input = fixture.input();
    input.history = None;
    assert!(write_bundle(&out, &input).is_err());
    assert!(!out.exists());
}

#[test]
fn repeated_run_ids_retain_original_evidence_without_optional_file_carryover() {
    let scratch = Scratch::new();
    let fixture = Fixture::new(&scratch.0);
    let mut first_input = fixture.input();
    first_input
        .evidence
        .push(("original.bin".into(), b"original evidence".to_vec()));
    let first = write_bundle(&scratch.0, &first_input).unwrap();
    let original_verdict = std::fs::read(first.join("verdict.json")).unwrap();
    let original_schedule = std::fs::read(first.join("schedule.json")).unwrap();
    let mut second_input = fixture.input();
    second_input.history = None;
    second_input.schedule = None;
    second_input.reproduction = "different retained run".into();
    let second = write_bundle(&scratch.0, &second_input).unwrap();
    assert_ne!(first, second, "a previous bundle was overwritten");
    assert_eq!(
        std::fs::read(first.join("verdict.json")).unwrap(),
        original_verdict
    );
    assert_eq!(
        std::fs::read(first.join("schedule.json")).unwrap(),
        original_schedule
    );
    assert_eq!(
        std::fs::read(first.join("evidence/original.bin")).unwrap(),
        b"original evidence"
    );
    let loaded = load_bundle(&second).unwrap();
    assert!(loaded.schedule.is_none());
    assert!(loaded.history.is_none());
    assert!(!second.join("evidence/original.bin").exists());
    assert_eq!(
        std::fs::read_to_string(first.join("reproduction.md")).unwrap(),
        "reproduce this run"
    );
}

#[test]
fn concurrent_writers_reserve_distinct_complete_bundles() {
    let scratch = Scratch::new();
    let fixture = Fixture::new(&scratch.0);
    let paths = std::thread::scope(|scope| {
        let writers: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| write_bundle(&scratch.0, &fixture.input()).unwrap()))
            .collect();
        writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect::<Vec<_>>()
    });
    let unique: std::collections::BTreeSet<_> = paths.iter().collect();
    assert_eq!(unique.len(), paths.len());
    for path in paths {
        assert_eq!(
            load_bundle(&path).unwrap().verdict.encode(),
            fixture.verdict.encode()
        );
    }
}

#[test]
fn escaping_artifact_names_are_rejected_before_output_creation() {
    let scratch = Scratch::new();
    let fixture = Fixture::new(&scratch.0);
    for name in [
        "../verdict.json",
        "..\\verdict.json",
        "C:\\verdict.json",
        "data:stream",
    ] {
        let out = scratch.0.join("not-created");
        let mut input = fixture.input();
        input.evidence.push((name.into(), b"replacement".to_vec()));
        assert!(write_bundle(&out, &input).is_err(), "accepted {name}");
        assert!(!out.exists(), "created output before rejecting {name}");
    }
}
