//! Result statuses, checks, gates and the verdict document (SPEC-010 §2, §17).

use std::collections::BTreeMap;

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, Hash256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Pass,
    Fail,
    Inconclusive,
    NotRun,
    NotApplicable,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Inconclusive => "INCONCLUSIVE",
            Status::NotRun => "NOT_RUN",
            Status::NotApplicable => "NOT_APPLICABLE",
        }
    }
    pub fn from_label(s: &str) -> CoreResult<Status> {
        Ok(match s {
            "PASS" => Status::Pass,
            "FAIL" => Status::Fail,
            "INCONCLUSIVE" => Status::Inconclusive,
            "NOT_RUN" => Status::NotRun,
            "NOT_APPLICABLE" => Status::NotApplicable,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("status {k}"),
                ))
            }
        })
    }
}

/// One named check with the scope it actually explored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub id: String,
    pub status: Status,
    /// What was explored (seeds, cases, bounds, fault model). Never wider than what ran.
    pub scope: String,
    pub detail: String,
    /// References into bundles/evidence (paths or hashes).
    pub evidence: Vec<String>,
}

impl CheckResult {
    pub fn pass(id: &str, scope: impl Into<String>, detail: impl Into<String>) -> CheckResult {
        CheckResult {
            id: id.into(),
            status: Status::Pass,
            scope: scope.into(),
            detail: detail.into(),
            evidence: vec![],
        }
    }
    pub fn fail(id: &str, scope: impl Into<String>, detail: impl Into<String>) -> CheckResult {
        CheckResult {
            id: id.into(),
            status: Status::Fail,
            scope: scope.into(),
            detail: detail.into(),
            evidence: vec![],
        }
    }
    pub fn inconclusive(
        id: &str,
        scope: impl Into<String>,
        detail: impl Into<String>,
    ) -> CheckResult {
        CheckResult {
            id: id.into(),
            status: Status::Inconclusive,
            scope: scope.into(),
            detail: detail.into(),
            evidence: vec![],
        }
    }
    pub fn not_run(id: &str, reason: impl Into<String>) -> CheckResult {
        CheckResult {
            id: id.into(),
            status: Status::NotRun,
            scope: String::new(),
            detail: reason.into(),
            evidence: vec![],
        }
    }
    pub fn not_applicable(id: &str, justification: impl Into<String>) -> CheckResult {
        CheckResult {
            id: id.into(),
            status: Status::NotApplicable,
            scope: String::new(),
            detail: justification.into(),
            evidence: vec![],
        }
    }
    /// From a campaign result: `Ok` is PASS with the given scope, `Err` is FAIL with the violation.
    pub fn from_result<T>(
        id: &str,
        scope: impl Into<String>,
        r: Result<T, String>,
        detail: impl FnOnce(&T) -> String,
    ) -> CheckResult {
        match r {
            Ok(v) => {
                let d = detail(&v);
                CheckResult::pass(id, scope, d)
            }
            Err(e) => CheckResult::fail(id, scope, e),
        }
    }
}

impl Canonical for CheckResult {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("detail", &self.detail)
            .f(
                "evidence",
                CanonValue::Array(self.evidence.iter().map(CanonValue::str).collect()),
            )
            .fstr("id", &self.id)
            .fstr("scope", &self.scope)
            .fstr("status", self.status.label())
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["detail", "evidence", "id", "scope", "status"])?;
        Ok(CheckResult {
            id: v.field("id")?.as_str()?.to_string(),
            status: Status::from_label(v.field("status")?.as_str()?)?,
            scope: v.field("scope")?.as_str()?.to_string(),
            detail: v.field("detail")?.as_str()?.to_string(),
            evidence: v
                .field("evidence")?
                .as_array()?
                .iter()
                .map(|e| e.as_str().map(|s| s.to_string()))
                .collect::<CoreResult<Vec<_>>>()?,
        })
    }
}

/// A SPEC-010 §17 gate derived from its required checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateVerdict {
    pub gate: String,
    pub status: Status,
    pub required: Vec<String>,
    pub detail: String,
}

impl Canonical for GateVerdict {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("detail", &self.detail)
            .fstr("gate", &self.gate)
            .f(
                "required",
                CanonValue::Array(self.required.iter().map(CanonValue::str).collect()),
            )
            .fstr("status", self.status.label())
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["detail", "gate", "required", "status"])?;
        Ok(GateVerdict {
            gate: v.field("gate")?.as_str()?.to_string(),
            status: Status::from_label(v.field("status")?.as_str()?)?,
            required: v
                .field("required")?
                .as_array()?
                .iter()
                .map(|e| e.as_str().map(|s| s.to_string()))
                .collect::<CoreResult<Vec<_>>>()?,
            detail: v.field("detail")?.as_str()?.to_string(),
        })
    }
}

/// Gate rule (SPEC-010 §2/§17): PASS only when every required check ran and passed. A missing or
/// NOT_RUN check leaves the gate NOT_RUN; a NOT_APPLICABLE required check cannot open a gate; any
/// FAIL closes it; INCONCLUSIVE without FAIL stays INCONCLUSIVE.
pub fn derive_gate(gate: &str, required: &[&str], checks: &[CheckResult]) -> GateVerdict {
    let mut status = Status::Pass;
    let mut notes = Vec::new();
    for id in required {
        match checks.iter().find(|c| c.id == *id) {
            None => {
                notes.push(format!("{id}: missing"));
                status = worst(status, Status::NotRun);
            }
            Some(c) => match c.status {
                Status::Pass => {}
                Status::Fail => {
                    notes.push(format!("{id}: FAIL"));
                    status = Status::Fail;
                }
                Status::Inconclusive => {
                    notes.push(format!("{id}: INCONCLUSIVE"));
                    status = worst(status, Status::Inconclusive);
                }
                Status::NotRun => {
                    notes.push(format!("{id}: NOT_RUN"));
                    status = worst(status, Status::NotRun);
                }
                Status::NotApplicable => {
                    notes.push(format!(
                        "{id}: NOT_APPLICABLE cannot satisfy a required check"
                    ));
                    status = worst(status, Status::NotRun);
                }
            },
        }
    }
    GateVerdict {
        gate: gate.into(),
        status,
        required: required.iter().map(|s| s.to_string()).collect(),
        detail: if notes.is_empty() {
            "all required checks PASS".into()
        } else {
            notes.join("; ")
        },
    }
}

/// Severity order for gate derivation: FAIL > INCONCLUSIVE > NOT_RUN > PASS.
fn worst(a: Status, b: Status) -> Status {
    let rank = |s: Status| match s {
        Status::Fail => 3,
        Status::Inconclusive => 2,
        Status::NotRun => 1,
        Status::NotApplicable => 1,
        Status::Pass => 0,
    };
    if rank(b) > rank(a) {
        b
    } else {
        a
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub verdict_version: u32,
    pub campaign_id: String,
    pub run_id: String,
    pub manifest_hash: Hash256,
    pub trace_digest: Hash256,
    pub checks: Vec<CheckResult>,
    pub gates: Vec<GateVerdict>,
    pub metrics: BTreeMap<String, u64>,
    /// Gates this campaign claims for the enabled profile; the exit code is 0 only when all pass.
    pub claimed_gates: Vec<String>,
}

impl Verdict {
    pub const VERSION: u32 = 1;

    pub fn verdict_hash(&self) -> Hash256 {
        domain_hash("astra.qualification-verdict.v1", &self.encode())
    }

    /// Overall status: FAIL if any executed check failed; INCONCLUSIVE if any is inconclusive;
    /// otherwise PASS. NOT_RUN/NOT_APPLICABLE checks are reported, never hidden, and never PASS.
    pub fn overall(&self) -> Status {
        if self.checks.iter().any(|c| c.status == Status::Fail) {
            Status::Fail
        } else if self.checks.iter().any(|c| c.status == Status::Inconclusive) {
            Status::Inconclusive
        } else {
            Status::Pass
        }
    }

    /// Process exit code (SPEC-010 §15): 0 only when no check failed and every claimed gate is
    /// PASS; 1 on any FAIL; 3 when a claimed gate is INCONCLUSIVE or NOT_RUN (2 is invalid config).
    /// An omitted required scenario therefore never yields a green exit (QA-05).
    pub fn exit_code(&self) -> u8 {
        match self.overall() {
            Status::Fail => 1,
            Status::Inconclusive => 3,
            _ => {
                let all_claimed_pass = self.claimed_gates.iter().all(|g| {
                    self.gates
                        .iter()
                        .any(|x| &x.gate == g && x.status == Status::Pass)
                });
                if all_claimed_pass {
                    0
                } else {
                    3
                }
            }
        }
    }

    pub fn summary(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "campaign {} run {}\nmanifest {}\ntrace {}\noverall {}\n\n",
            self.campaign_id,
            self.run_id,
            self.manifest_hash,
            self.trace_digest,
            self.overall().label()
        ));
        s.push_str("gates:\n");
        for g in &self.gates {
            let claimed = if self.claimed_gates.contains(&g.gate) {
                "claimed"
            } else {
                "not claimed"
            };
            s.push_str(&format!(
                "  {:<4} {:<14} ({claimed}) {}\n",
                g.gate,
                g.status.label(),
                g.detail
            ));
        }
        s.push_str("checks:\n");
        for c in &self.checks {
            s.push_str(&format!(
                "  {:<22} {:<14} {}",
                c.id,
                c.status.label(),
                c.detail
            ));
            if !c.scope.is_empty() {
                s.push_str(&format!("  [scope: {}]", c.scope));
            }
            s.push('\n');
        }
        if !self.metrics.is_empty() {
            s.push_str("metrics:\n");
            for (k, v) in &self.metrics {
                s.push_str(&format!("  {k} = {v}\n"));
            }
        }
        s
    }
}

impl Canonical for Verdict {
    fn to_canon(&self) -> CanonValue {
        let mut metrics = BTreeMap::new();
        for (k, v) in &self.metrics {
            metrics.insert(k.clone(), CanonValue::uint(*v));
        }
        CanonValue::obj()
            .fstr("campaign_id", &self.campaign_id)
            .fvec("checks", &self.checks)
            .f(
                "claimed_gates",
                CanonValue::Array(self.claimed_gates.iter().map(CanonValue::str).collect()),
            )
            .fvec("gates", &self.gates)
            .fc("manifest_hash", &self.manifest_hash)
            .f("metrics", CanonValue::Object(metrics))
            .fstr("run_id", &self.run_id)
            .fc("trace_digest", &self.trace_digest)
            .fu32("verdict_version", self.verdict_version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "campaign_id",
            "checks",
            "claimed_gates",
            "gates",
            "manifest_hash",
            "metrics",
            "run_id",
            "trace_digest",
            "verdict_version",
        ])?;
        let mut metrics = BTreeMap::new();
        for (k, x) in v.field("metrics")?.as_object()? {
            metrics.insert(k.clone(), x.as_u64()?);
        }
        Ok(Verdict {
            verdict_version: v.field("verdict_version")?.as_u64()? as u32,
            campaign_id: v.field("campaign_id")?.as_str()?.to_string(),
            run_id: v.field("run_id")?.as_str()?.to_string(),
            manifest_hash: Hash256::from_canon(v.field("manifest_hash")?)?,
            trace_digest: Hash256::from_canon(v.field("trace_digest")?)?,
            checks: v
                .field("checks")?
                .as_array()?
                .iter()
                .map(CheckResult::from_canon)
                .collect::<CoreResult<Vec<_>>>()?,
            gates: v
                .field("gates")?
                .as_array()?
                .iter()
                .map(GateVerdict::from_canon)
                .collect::<CoreResult<Vec<_>>>()?,
            metrics,
            claimed_gates: v
                .field("claimed_gates")?
                .as_array()?
                .iter()
                .map(|e| e.as_str().map(|s| s.to_string()))
                .collect::<CoreResult<Vec<_>>>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_rules() {
        let checks = vec![
            CheckResult::pass("A", "", ""),
            CheckResult::not_applicable("B", "disabled"),
            CheckResult::inconclusive("C", "", "budget"),
        ];
        assert_eq!(derive_gate("G", &["A"], &checks).status, Status::Pass);
        assert_eq!(
            derive_gate("G", &["A", "B"], &checks).status,
            Status::NotRun
        );
        assert_eq!(
            derive_gate("G", &["A", "C"], &checks).status,
            Status::Inconclusive
        );
        assert_eq!(
            derive_gate("G", &["A", "Z"], &checks).status,
            Status::NotRun
        );
        let mut c2 = checks.clone();
        c2.push(CheckResult::fail("D", "", "bad"));
        assert_eq!(derive_gate("G", &["A", "C", "D"], &c2).status, Status::Fail);
    }
}
