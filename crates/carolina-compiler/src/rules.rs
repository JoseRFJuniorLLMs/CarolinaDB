//! Versioned analysis rules (SPEC-004 §5).
//!
//! `Proven` means a specific accepted rule discharges the obligation under enumerated
//! premises. Each rule has a reviewable statement, a supported fragment and explicit
//! assumptions. Rules are small and versioned; their manifest hash is part of the certificate.

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, Hash256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisRule {
    pub id: String,
    pub version: u32,
    /// Human-reviewable mathematical statement of what the rule establishes.
    pub statement: String,
    /// Fragment of inputs the rule accepts.
    pub fragment: String,
    /// Runtime assumptions the rule depends on.
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisRuleManifest {
    pub manifest_version: u32,
    pub rules: Vec<AnalysisRule>,
}

pub const R_IR_DEF_EVAL: &str = "IR-DEF-EVAL-V1";
pub const R_FOOT_CONSERVATIVE: &str = "FOOT-CONSERVATIVE-V1";
pub const R_C5_SERIAL_VALIDATE: &str = "C5-SERIAL-VALIDATE-V1";
pub const R_C0_EXCLUSIVE_LOCAL: &str = "C0-EXCLUSIVE-LOCAL-V1";
pub const R_OBS_RESULT_SCOPE: &str = "OBS-RESULT-SCOPE-V1";
pub const R_ATOM_SINGLE_AUTHORITY: &str = "ATOM-SINGLE-AUTHORITY-V1";
pub const R_COMP_SAME_AUTHORITY: &str = "COMP-SAME-AUTHORITY-V1";
pub const R_SESSION_SERIAL_SUBSUMES: &str = "SESSION-SERIAL-SUBSUMES-V1";
pub const R_C1_GROWONLY_FACTS: &str = "C1-GROWONLY-FACTS-V1";
pub const R_CONC_BOUNDED_EXPLORATION: &str = "CONC-BOUNDED-EXPLORATION-V1";
pub const R_DURABILITY_POLICY: &str = "DURABILITY-POLICY-V1";

impl AnalysisRuleManifest {
    pub fn v1() -> AnalysisRuleManifest {
        let r = |id: &str, statement: &str, fragment: &str, assumptions: &[&str]| AnalysisRule {
            id: id.into(),
            version: 1,
            statement: statement.into(),
            fragment: fragment.into(),
            assumptions: assumptions.iter().map(|s| s.to_string()).collect(),
        };
        AnalysisRuleManifest {
            manifest_version: 1,
            rules: vec![
                r(R_IR_DEF_EVAL, "Every IR node has a deterministic, terminating, checked evaluator in the reference interpreter; failure is a typed rejection.", "IR v1 node set", &[]),
                r(R_FOOT_CONSERVATIVE, "Footprint selectors cover all reads/writes/predicates; dynamic keys are widened to the entire record.", "IR v1 footprints", &[]),
                r(
                    R_C5_SERIAL_VALIDATE,
                    "Under a durable ordered authority that evaluates preconditions, checked effects, postconditions and every affected invariant against the authoritative serial pre-state before acceptance, every accepted history is equivalent to a sequential execution of the exact contracts; therefore IR-SEQ and IR-CONC hold for all admitted histories of the scope.",
                    "operations whose affected invariant scope is finite or conservatively widened and fully evaluable",
                    &["single ordered authority over the complete scope", "fenced alternative writers", "atomic local publication of effects, result and identity"],
                ),
                r(
                    R_C0_EXCLUSIVE_LOCAL,
                    "With one fenced exclusive local writer over the complete affected scope, local serialization of evaluated invocations yields the same guarantee as C5-SERIAL-VALIDATE without remote coordination.",
                    "operations whose scope is covered by one ExclusiveLocal authority domain",
                    &["exclusive writer epoch enforced at admission", "failover preserves state, request outcomes and authority"],
                ),
                r(
                    R_OBS_RESULT_SCOPE,
                    "Receipt results depend only on accepted effects; SnapshotValue results require an identified admissible snapshot; ExactOrderedValue results require SerialScope visibility at an ordered position.",
                    "ContractIR v1",
                    &[],
                ),
                r(R_ATOM_SINGLE_AUTHORITY, "All atomic groups of an invocation are indivisible when every participant IDC is executed and published by one authority through one local atomic batch.", "single-authority participants", &["SPEC-002 atomic CompiledBatch"]),
                r(R_COMP_SAME_AUTHORITY, "Two plans in one closure are compatible when they share the same ordered authority and validation program: their interleavings are serialized by construction.", "same-template closures", &[]),
                r(
                    R_SESSION_SERIAL_SUBSUMES,
                    "Read-your-writes, monotonic reads and causal dependencies scoped to one group whose records are all executed by one serial authority are satisfied by authoritative serial reads.",
                    "group-scoped sessions inside one closure",
                    &["session reads pass the authority read barrier"],
                ),
                r(
                    R_C1_GROWONLY_FACTS,
                    "Operations whose only effects emit immutable facts (insert-only, key from arguments) with receipt results and no invariant beyond primary-key identity converge under duplicate-suppressed unordered application and preserve results.",
                    "EmitFact-only operations over immutable records without Unique/Aggregate/Referential invariants",
                    &["stable origin identity and deduplication", "representation limit: bounded key space"],
                ),
                r(
                    R_CONC_BOUNDED_EXPLORATION,
                    "Bounded exploration of concurrent acceptance from generated small states either yields a replayable counterexample (Disproven) or Unknown; it is never a proof.",
                    "operation pairs within the deterministic budget",
                    &[],
                ),
                r(R_DURABILITY_POLICY, "A contract durability requirement is satisfiable when the topology declares a policy with at least the required durable copies and failure domains.", "LocalStable / ReplicatedStable(policy)", &[]),
            ],
        }
    }

    pub fn rule(&self, id: &str) -> Option<&AnalysisRule> {
        self.rules.iter().find(|r| r.id == id)
    }

    pub fn hash(&self) -> Hash256 {
        domain_hash("astra.analysis-rules.v1", &self.to_canon().encode())
    }
}

impl Canonical for AnalysisRule {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("assumptions", &self.assumptions)
            .fstr("fragment", &self.fragment)
            .fstr("id", &self.id)
            .fstr("statement", &self.statement)
            .fu32("version", self.version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(AnalysisRule {
            id: v.field("id")?.as_str()?.to_string(),
            version: v.field("version")?.as_u32()?,
            statement: v.field("statement")?.as_str()?.to_string(),
            fragment: v.field("fragment")?.as_str()?.to_string(),
            assumptions: Vec::from_canon(v.field("assumptions")?)?,
        })
    }
}

impl Canonical for AnalysisRuleManifest {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("kind", "analysis-rules.v1")
            .fu32("manifest_version", self.manifest_version)
            .fvec("rules", &self.rules)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        if v.field("kind")?.as_str()? != "analysis-rules.v1" {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "expected analysis-rules.v1",
            ));
        }
        Ok(AnalysisRuleManifest {
            manifest_version: v.field("manifest_version")?.as_u32()?,
            rules: Vec::from_canon(v.field("rules")?)?,
        })
    }
}
