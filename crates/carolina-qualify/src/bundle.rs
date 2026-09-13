//! Evidence and failure bundles (SPEC-010 §15).
//!
//! ```text
//! <out>/<campaign_id>/<run_id>/
//!   manifest.json  verdict.json  history.jsonl  schedule.json  metrics.json  reproduction.md
//!   initial-state/  contracts/  plans/  evidence/
//! ```
//! Bundles contain only synthetic data. A bundle is written for every FAIL/INCONCLUSIVE run and,
//! when requested, for passing runs as retained evidence. Repeated run identities receive a
//! fresh directory suffix; an existing bundle is never overwritten or reused.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};

use carolina_core::canon::{CanonValue, Canonical};

use crate::history::History;
use crate::local::Schedule;
use crate::manifest::QualificationManifest;
use crate::verdict::Verdict;
use crate::w1::InventoryModel;

pub const EVIDENCE_SCHEMA_VERSION: u32 = 1;

pub struct BundleInput<'a> {
    pub manifest: &'a QualificationManifest,
    pub verdict: &'a Verdict,
    pub history: Option<&'a History>,
    pub schedule: Option<&'a Schedule>,
    pub initial: Option<&'a InventoryModel>,
    pub final_state: Option<&'a InventoryModel>,
    pub contracts: Vec<(String, Vec<u8>)>,
    pub plans: Vec<(String, Vec<u8>)>,
    pub evidence: Vec<(String, Vec<u8>)>,
    pub metrics: &'a BTreeMap<String, u64>,
    pub reproduction: String,
}

fn model_canon(m: &InventoryModel) -> CanonValue {
    let items: Vec<CanonValue> = m
        .items
        .iter()
        .map(|(id, it)| {
            CanonValue::obj()
                .f("available", CanonValue::Str(it.available.to_string()))
                .fbytes("id", id)
                .f("reserved", CanonValue::Str(it.reserved.to_string()))
                .f("total", CanonValue::Str(it.total.to_string()))
                .build()
        })
        .collect();
    let res: Vec<CanonValue> = m
        .reservations
        .iter()
        .map(|(rid, r)| {
            CanonValue::obj()
                .f("amount", CanonValue::Str(r.amount.to_string()))
                .fbytes("item", &r.item)
                .fbytes("rid", rid)
                .fstr("state", &format!("{:?}", r.state))
                .build()
        })
        .collect();
    CanonValue::obj()
        .f("items", CanonValue::Array(items))
        .f("reservations", CanonValue::Array(res))
        .build()
}

fn file_component(name: &str) -> std::io::Result<()> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\', ':', '\0'])
        || name.ends_with(['.', ' '])
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("invalid bundle component {name:?}"),
        ));
    }
    Ok(())
}

fn validate_input(input: &BundleInput<'_>) -> std::io::Result<()> {
    file_component(&input.verdict.campaign_id)?;
    file_component(&input.verdict.run_id)?;
    if input.manifest.manifest_hash() != input.verdict.manifest_hash {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "verdict manifest hash mismatch",
        ));
    }
    if input.schedule.is_some() && input.history.is_none() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "schedule.json requires history.jsonl",
        ));
    }
    if let Some(history) = input.history {
        if history.trace_digest() != input.verdict.trace_digest {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "verdict trace digest mismatch",
            ));
        }
    }
    for (artifacts, reserved) in [
        (&input.contracts, None),
        (&input.plans, None),
        (
            &input.evidence,
            input.final_state.map(|_| "final-state.json"),
        ),
    ] {
        let mut names = BTreeSet::new();
        if let Some(name) = reserved {
            names.insert(name.to_string());
        }
        for (name, _) in artifacts {
            file_component(name)?;
            // Portable bundles must not alias files on case-insensitive filesystems either.
            if !names.insert(name.to_lowercase()) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("duplicate bundle artifact {name}"),
                ));
            }
        }
    }
    Ok(())
}

fn reserve_directory(out: &Path, campaign_id: &str, run_id: &str) -> std::io::Result<PathBuf> {
    let campaign = out.join(campaign_id);
    std::fs::create_dir_all(&campaign)?;
    for occurrence in 1u64.. {
        let name = if occurrence == 1 {
            run_id.to_string()
        } else {
            format!("{run_id}-{occurrence}")
        };
        let dir = campaign.join(name);
        // create_dir reserves a new directory atomically, including when other writers race.
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(Error::new(
        ErrorKind::AlreadyExists,
        "bundle directory names exhausted",
    ))
}

pub fn write_bundle(out: &Path, input: &BundleInput<'_>) -> std::io::Result<PathBuf> {
    validate_input(input)?;
    let dir = reserve_directory(out, &input.verdict.campaign_id, &input.verdict.run_id)?;
    for sub in ["initial-state", "contracts", "plans", "evidence"] {
        std::fs::create_dir_all(dir.join(sub))?;
    }
    std::fs::write(dir.join("manifest.json"), input.manifest.encode())?;
    if let Some(h) = input.history {
        std::fs::write(dir.join("history.jsonl"), h.to_jsonl())?;
    }
    if let Some(s) = input.schedule {
        std::fs::write(dir.join("schedule.json"), s.encode())?;
    }
    if let Some(m) = input.initial {
        std::fs::write(
            dir.join("initial-state").join("w1.json"),
            model_canon(m).encode(),
        )?;
    }
    if let Some(m) = input.final_state {
        std::fs::write(
            dir.join("evidence").join("final-state.json"),
            model_canon(m).encode(),
        )?;
    }
    for (name, bytes) in &input.contracts {
        std::fs::write(dir.join("contracts").join(name), bytes)?;
    }
    for (name, bytes) in &input.plans {
        std::fs::write(dir.join("plans").join(name), bytes)?;
    }
    for (name, bytes) in &input.evidence {
        std::fs::write(dir.join("evidence").join(name), bytes)?;
    }
    let mut metrics = BTreeMap::new();
    for (k, v) in input.metrics {
        metrics.insert(k.clone(), CanonValue::uint(*v));
    }
    metrics.insert(
        "evidence_schema_version".into(),
        CanonValue::uint(EVIDENCE_SCHEMA_VERSION as u64),
    );
    std::fs::write(
        dir.join("metrics.json"),
        CanonValue::Object(metrics).encode(),
    )?;
    std::fs::write(dir.join("reproduction.md"), &input.reproduction)?;
    // Publish the verdict only after all of this run's evidence has been written. A failed
    // write leaves an incomplete fresh directory, without damaging any previous evidence.
    std::fs::write(dir.join("verdict.json"), input.verdict.encode())?;
    Ok(dir)
}

/// Read a bundle's schedule, history and manifest for `replay`/`minimize`.
pub struct LoadedBundle {
    pub manifest: QualificationManifest,
    pub verdict: Verdict,
    pub schedule: Option<Schedule>,
    pub history: Option<History>,
}

pub fn load_bundle(dir: &Path) -> Result<LoadedBundle, String> {
    let limits = carolina_core::limits::Limits::v1();
    let manifest = QualificationManifest::decode(
        &std::fs::read(dir.join("manifest.json")).map_err(|e| format!("manifest.json: {e}"))?,
        &limits,
    )
    .map_err(|e| format!("manifest.json: {e}"))?;
    let verdict = Verdict::decode(
        &std::fs::read(dir.join("verdict.json")).map_err(|e| format!("verdict.json: {e}"))?,
        &limits,
    )
    .map_err(|e| format!("verdict.json: {e}"))?;
    if verdict.manifest_hash != manifest.manifest_hash() {
        return Err("verdict.json: manifest hash does not match manifest.json".into());
    }
    let schedule = match std::fs::read(dir.join("schedule.json")) {
        Ok(b) => Some(Schedule::decode(&b, &limits).map_err(|e| format!("schedule.json: {e}"))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("schedule.json: {e}")),
    };
    let history = match std::fs::read_to_string(dir.join("history.jsonl")) {
        Ok(t) => Some(History::from_jsonl(&t).map_err(|e| format!("history.jsonl: {e}"))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("history.jsonl: {e}")),
    };
    if schedule.is_some() && history.is_none() {
        return Err("history.jsonl: required for a bundle containing schedule.json".into());
    }
    if let Some(h) = &history {
        if h.trace_digest() != verdict.trace_digest {
            return Err("history.jsonl: trace digest does not match verdict.json".into());
        }
    }
    Ok(LoadedBundle {
        manifest,
        verdict,
        schedule,
        history,
    })
}
