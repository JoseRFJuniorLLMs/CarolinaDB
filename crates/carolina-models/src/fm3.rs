//! FM-3 — Migration, fencing and plan evolution (SPEC-009 §7–§8, SPEC-011 §4/§7; SPEC-010 §16).
//!
//! One migration over `N` source authorities and one target authority, coordinated by workers
//! that must hold the current catalog claim (compare-and-set revision). Sources admit
//! old-generation requests until fenced; fenced sources keep resolving already admitted requests;
//! results produced are retained across activation and retirement. Sources may go offline; an
//! offline source cannot produce a close certificate. Old-generation packets may arrive late.
//!
//! ```text
//! PROPOSED -> CLOSING -> DRAINING -> INSTALLING -> ACTIVE -> RETIRED ; CANCELLED before CLOSING
//! CLOSING/DRAINING/INSTALLING -> CANCELLED by SUPERSEDE: sources resume under a successor
//! incarnation (new fence epoch), the old target must never admit (SPEC-009 §7)
//! ```
//!
//! Invariants checked in every reachable state:
//! * no overlapping incompatible admitting authorities (a source admitting while the target admits);
//! * a closed authority never reopens under the same epoch;
//! * old accepted results remain resolvable (retained == produced);
//! * an offline holder blocks unsafe replacement (ACTIVE ⇒ close certificate from every source);
//! * no late packet creates new authority (an admission after the fence is refused);
//! * activation requires complete close/drain/install evidence; a stale worker cannot advance.

use crate::{explore, GateEvidence, ModelReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Proposed,
    Closing,
    Draining,
    Installing,
    Active,
    Retired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Source {
    pub admitting: bool,
    pub fenced: bool,
    pub fence_epoch: u8,
    pub offline: bool,
    pub pending: u8,
    pub close_cert: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct State {
    pub phase: Phase,
    pub catalog_revision: u8,
    pub worker_claims: [u8; 2],
    pub sources: Vec<Source>,
    pub target_installed: bool,
    pub target_admitting: bool,
    pub produced_results: u8,
    pub retained_results: u8,
    pub late_admissions: u8,
    pub reopened_same_epoch: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Activation ignores a missing close certificate (offline source).
    ActivateWithOfflineSource,
    /// A worker with a stale claim may advance phases (no CAS).
    StaleWorkerAdvances,
    /// Cancelling after closure deletes the fence (rollback), reopening the same epoch.
    RollbackFence,
    /// A late old-generation packet is admitted after the fence.
    LatePacketAdmitted,
    /// Retirement drops retained results/mappings.
    RetireDropsResults,
}

const MAX_PENDING: u8 = 1;
/// Admissions under the old generation are bounded so the state space is finite.
const MAX_ADMISSIONS: u8 = 2;

pub fn invariant(s: &State) -> Result<(), String> {
    if s.target_admitting && s.sources.iter().any(|x| x.admitting) {
        return Err("overlapping admitting authorities (source and target)".into());
    }
    if s.reopened_same_epoch > 0 {
        return Err("closed authority reopened under the same epoch".into());
    }
    if s.retained_results != s.produced_results {
        return Err("an accepted result is no longer resolvable".into());
    }
    if s.phase == Phase::Active || s.phase == Phase::Retired {
        if s.sources.iter().any(|x| !x.close_cert) {
            return Err(
                "activated without a close certificate from every source (offline holder)".into(),
            );
        }
        if !s.target_installed {
            return Err("activated without target installation".into());
        }
        if s.sources.iter().any(|x| x.pending > 0) {
            return Err("activated with an undrained admitted request".into());
        }
    }
    if s.late_admissions > 0 {
        return Err(
            "late packet admitted after the fence (new authority from an old packet)".into(),
        );
    }
    for (i, x) in s.sources.iter().enumerate() {
        if x.fenced && x.admitting {
            return Err(format!("source {i} admits while fenced"));
        }
    }
    Ok(())
}

/// A worker advances only with a non-empty claim equal to the catalog's current revision (CAS).
fn current(s: &State, w: usize) -> bool {
    s.worker_claims[w] != 0 && s.worker_claims[w] == s.catalog_revision
}

pub fn next(s: &State, control: Option<Control>) -> Vec<(String, State)> {
    let mut out = Vec::new();
    let n = s.sources.len();
    // workers claim the migration through the catalog CAS (a newer claim supersedes older ones)
    for w in 0..2 {
        if !current(s, w) && s.catalog_revision < 3 {
            let mut ns = s.clone();
            ns.catalog_revision += 1;
            ns.worker_claims[w] = ns.catalog_revision;
            out.push((format!("WorkerClaim({w})"), ns));
        }
    }
    for w in 0..2 {
        if !current(s, w) {
            continue;
        }
        match s.phase {
            Phase::Proposed => {
                let mut ns = s.clone();
                ns.phase = Phase::Closing;
                out.push((format!("StartClosing({w})"), ns));
                let mut ns = s.clone();
                ns.phase = Phase::Cancelled;
                out.push((format!("Cancel({w})"), ns));
            }
            Phase::Closing => {
                let certs_ok = s.sources.iter().all(|x| x.close_cert)
                    || (control == Some(Control::ActivateWithOfflineSource)
                        && s.sources.iter().all(|x| x.close_cert || x.offline));
                if certs_ok {
                    let mut ns = s.clone();
                    ns.phase = Phase::Draining;
                    out.push((format!("StartDraining({w})"), ns));
                }
                if control == Some(Control::RollbackFence) {
                    let mut ns = s.clone();
                    ns.phase = Phase::Cancelled;
                    for x in ns.sources.iter_mut() {
                        if x.fenced {
                            x.fenced = false;
                            x.admitting = true;
                            ns.reopened_same_epoch += 1;
                        }
                    }
                    out.push((format!("CancelWithRollback({w})"), ns));
                }
            }
            Phase::Draining => {
                let drained = s.sources.iter().all(|x| x.pending == 0)
                    || (control == Some(Control::ActivateWithOfflineSource)
                        && s.sources.iter().all(|x| x.pending == 0 || x.offline));
                if drained {
                    let mut ns = s.clone();
                    ns.phase = Phase::Installing;
                    out.push((format!("DrainComplete({w})"), ns));
                }
            }
            Phase::Installing => {
                if !s.target_installed {
                    let mut ns = s.clone();
                    ns.target_installed = true;
                    out.push((format!("InstallTarget({w})"), ns));
                }
                let evidence_ok =
                    s.target_installed && s.sources.iter().all(|x| x.close_cert && x.pending == 0);
                let relaxed = control == Some(Control::ActivateWithOfflineSource)
                    && s.target_installed
                    && s.sources
                        .iter()
                        .all(|x| (x.close_cert && x.pending == 0) || x.offline);
                if evidence_ok || relaxed {
                    let mut ns = s.clone();
                    ns.phase = Phase::Active;
                    ns.target_admitting = true;
                    out.push((format!("Activate({w})"), ns));
                }
            }
            Phase::Active => {
                let mut ns = s.clone();
                ns.phase = Phase::Retired;
                if control == Some(Control::RetireDropsResults) {
                    ns.retained_results = 0;
                }
                out.push((format!("Retire({w})"), ns));
            }
            Phase::Retired | Phase::Cancelled => {}
        }
        // supersede a closed-but-unactivated migration: sources resume under a successor epoch
        if matches!(
            s.phase,
            Phase::Closing | Phase::Draining | Phase::Installing
        ) && s.sources.iter().all(|x| x.fence_epoch < 2)
        {
            let mut ns = s.clone();
            ns.phase = Phase::Cancelled;
            ns.target_admitting = false;
            for x in ns.sources.iter_mut() {
                if x.fenced {
                    x.fence_epoch += 1;
                    x.fenced = false;
                    x.admitting = true;
                    x.close_cert = false;
                }
            }
            out.push((format!("Supersede({w})"), ns));
        }
    }
    // negative control: a worker with a stale claim applies its cached activation after the
    // migration was superseded (the catalog CAS would have refused the stale revision)
    if control == Some(Control::StaleWorkerAdvances) {
        for w in 0..2 {
            if s.worker_claims[w] != 0
                && !current(s, w)
                && s.target_installed
                && s.phase == Phase::Cancelled
            {
                let mut ns = s.clone();
                ns.phase = Phase::Active;
                ns.target_admitting = true;
                out.push((format!("StaleActivate({w})"), ns));
            }
        }
    }
    for i in 0..n {
        let x = s.sources[i];
        // admissions under the old generation while open
        let admitted_total: u8 =
            s.produced_results + s.sources.iter().map(|y| y.pending).sum::<u8>();
        if x.admitting && !x.fenced && x.pending < MAX_PENDING && admitted_total < MAX_ADMISSIONS {
            let mut ns = s.clone();
            ns.sources[i].pending += 1;
            out.push((format!("AdmitOld({i})"), ns));
        }
        // a late old-generation packet reaches a fenced source
        if x.fenced {
            let mut ns = s.clone();
            if control == Some(Control::LatePacketAdmitted) && admitted_total < MAX_ADMISSIONS {
                ns.sources[i].pending += 1;
                ns.late_admissions += 1;
                out.push((format!("LatePacketAdmitted({i})"), ns));
            } else {
                // refused with a typed rejection: no state change beyond the observation
                let _ = ns;
            }
        }
        // resolving an admitted request produces a retained result (even while fenced)
        if x.pending > 0 && !x.offline {
            let mut ns = s.clone();
            ns.sources[i].pending -= 1;
            ns.produced_results += 1;
            ns.retained_results += 1;
            out.push((format!("ResolveOld({i})"), ns));
        }
        // closing: fence, then certificate (needs the source online)
        if matches!(s.phase, Phase::Closing) && !x.fenced && !x.offline {
            let mut ns = s.clone();
            ns.sources[i].fenced = true;
            ns.sources[i].admitting = false;
            out.push((format!("FenceSource({i})"), ns));
        }
        if x.fenced && !x.close_cert && !x.offline {
            let mut ns = s.clone();
            ns.sources[i].close_cert = true;
            out.push((format!("CloseCertificate({i})"), ns));
        }
        if !x.offline {
            let mut ns = s.clone();
            ns.sources[i].offline = true;
            out.push((format!("SourceOffline({i})"), ns));
        } else {
            let mut ns = s.clone();
            ns.sources[i].offline = false;
            out.push((format!("SourceBack({i})"), ns));
        }
    }
    out
}

pub fn initial(sources: usize) -> State {
    State {
        phase: Phase::Proposed,
        catalog_revision: 0,
        worker_claims: [0, 0],
        sources: vec![
            Source {
                admitting: true,
                fenced: false,
                fence_epoch: 1,
                offline: false,
                pending: 0,
                close_cert: false,
            };
            sources
        ],
        target_installed: false,
        target_admitting: false,
        produced_results: 0,
        retained_results: 0,
        late_admissions: 0,
        reopened_same_epoch: 0,
    }
}

fn describe(s: &State) -> String {
    format!(
        "phase={:?} rev={} claims={:?} target(installed={},admitting={}) results={}/{} sources={:?}",
        s.phase,
        s.catalog_revision,
        s.worker_claims,
        s.target_installed,
        s.target_admitting,
        s.retained_results,
        s.produced_results,
        s.sources
            .iter()
            .map(|x| format!(
                "{}{}{}p{}",
                if x.admitting { "A" } else { "-" },
                if x.fenced { "F" } else { "-" },
                if x.offline { "off" } else { "on" },
                x.pending
            ))
            .collect::<Vec<_>>()
    )
}

pub fn run(sources: usize, control: Option<Control>, max_states: usize) -> ModelReport {
    let bounds = format!(
        "sources={sources}, 2 workers, catalog revisions <=3, <={MAX_PENDING} pending admission per source, <={MAX_ADMISSIONS} old-generation admissions, offline/online sources{}",
        control.map(|c| format!(", control={c:?}")).unwrap_or_default()
    );
    explore(
        "FM-3",
        &bounds,
        initial(sources),
        |s| next(s, control),
        invariant,
        describe,
        max_states,
    )
}

pub fn evidence(max_states: usize) -> GateEvidence {
    GateEvidence {
        gate: "FM-3",
        positive: run(2, None, max_states),
        controls: [
            Control::ActivateWithOfflineSource,
            Control::StaleWorkerAdvances,
            Control::RollbackFence,
            Control::LatePacketAdmitted,
            Control::RetireDropsResults,
        ]
        .into_iter()
        .map(|c| (format!("{c:?}"), run(2, Some(c), max_states)))
        .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_model_holds_and_controls_fail() {
        let ev = evidence(3_000_000);
        assert!(ev.positive.violation.is_none(), "{}", ev.positive.summary());
        assert!(!ev.positive.truncated, "{}", ev.positive.summary());
        for (name, r) in &ev.controls {
            assert!(
                r.violation.is_some(),
                "control {name} found nothing: {}",
                r.summary()
            );
        }
        assert!(ev.verdict().is_ok(), "{:?}", ev.verdict());
    }
}
