//! Executable operation plans, execution profiles, assumptions, judgments and certificates
//! (SPEC-004 §3, §5, §6, §11, §12, §13).
//!
//! Plans and certificates use the canonical artifact encoding with domains `astra.plan.v1`
//! and `astra.certificate.v1`. A plan hash covers the complete plan payload without its own
//! digest; the certificate refers to plan hashes and is hashed separately (no circular digest).

use carolina_core::canon::{decode_set, CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, domains, Hash256};
use carolina_core::ids::*;
use carolina_lang::ir::{InputVisibility, ResultScope, ResultSemantics, SessionGuarantee};

use crate::library::CostDescriptor;

pub const PLAN_FORMAT_VERSION: u32 = 1;
pub const CERTIFICATE_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Obligations and judgments (SPEC-003 §11, SPEC-004 §5)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObligationKind {
    IrDef,
    IrSeq,
    IrConc,
    IrObs,
    IrFoot,
    IrAtom,
    IrComp,
    Durability,
    Session,
    Authority,
    RuntimeCapability,
}

impl ObligationKind {
    pub fn label(&self) -> &'static str {
        match self {
            ObligationKind::IrDef => "IR-DEF",
            ObligationKind::IrSeq => "IR-SEQ",
            ObligationKind::IrConc => "IR-CONC",
            ObligationKind::IrObs => "IR-OBS",
            ObligationKind::IrFoot => "IR-FOOT",
            ObligationKind::IrAtom => "IR-ATOM",
            ObligationKind::IrComp => "IR-COMP",
            ObligationKind::Durability => "DURABILITY",
            ObligationKind::Session => "SESSION",
            ObligationKind::Authority => "AUTHORITY",
            ObligationKind::RuntimeCapability => "RUNTIME-CAPABILITY",
        }
    }
    pub fn from_label(s: &str) -> Option<Self> {
        Some(match s {
            "IR-DEF" => ObligationKind::IrDef,
            "IR-SEQ" => ObligationKind::IrSeq,
            "IR-CONC" => ObligationKind::IrConc,
            "IR-OBS" => ObligationKind::IrObs,
            "IR-FOOT" => ObligationKind::IrFoot,
            "IR-ATOM" => ObligationKind::IrAtom,
            "IR-COMP" => ObligationKind::IrComp,
            "DURABILITY" => ObligationKind::Durability,
            "SESSION" => ObligationKind::Session,
            "AUTHORITY" => ObligationKind::Authority,
            "RUNTIME-CAPABILITY" => ObligationKind::RuntimeCapability,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownReason {
    UnsupportedFragment(String),
    UnsupportedComposition(String),
    SolverUnknown(String),
    BudgetExceeded,
    MissingAuthorityModel(String),
    MissingRuntimeCapability(String),
}

impl UnknownReason {
    fn label(&self) -> (&'static str, String) {
        match self {
            UnknownReason::UnsupportedFragment(s) => ("UnsupportedFragment", s.clone()),
            UnknownReason::UnsupportedComposition(s) => ("UnsupportedComposition", s.clone()),
            UnknownReason::SolverUnknown(s) => ("SolverUnknown", s.clone()),
            UnknownReason::BudgetExceeded => ("BudgetExceeded", String::new()),
            UnknownReason::MissingAuthorityModel(s) => ("MissingAuthorityModel", s.clone()),
            UnknownReason::MissingRuntimeCapability(s) => ("MissingRuntimeCapability", s.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofStatus {
    Proven {
        rule_id: String,
        rule_version: u32,
        premises: Vec<String>,
    },
    /// Replayable counterexample, canonical bytes.
    Disproven {
        counterexample: Vec<u8>,
        summary: String,
    },
    Unknown(UnknownReason),
}

impl ProofStatus {
    pub fn is_proven(&self) -> bool {
        matches!(self, ProofStatus::Proven { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisJudgment {
    pub obligation_id: String,
    pub kind: ObligationKind,
    pub subjects: Vec<String>,
    pub status: ProofStatus,
    pub assumptions: Vec<String>,
}

impl Canonical for AnalysisJudgment {
    fn to_canon(&self) -> CanonValue {
        let status = match &self.status {
            ProofStatus::Proven {
                rule_id,
                rule_version,
                premises,
            } => CanonValue::obj()
                .fvec("premises", premises)
                .fstr("rule_id", rule_id)
                .fu32("rule_version", *rule_version)
                .fstr("status", "Proven")
                .build(),
            ProofStatus::Disproven {
                counterexample,
                summary,
            } => CanonValue::obj()
                .fbytes("counterexample", counterexample)
                .fstr("status", "Disproven")
                .fstr("summary", summary)
                .build(),
            ProofStatus::Unknown(r) => {
                let (k, d) = r.label();
                CanonValue::obj()
                    .fstr("detail", &d)
                    .fstr("reason", k)
                    .fstr("status", "Unknown")
                    .build()
            }
        };
        CanonValue::obj()
            .fvec("assumptions", &self.assumptions)
            .fstr("kind", self.kind.label())
            .fstr("obligation_id", &self.obligation_id)
            .f("status", status)
            .fvec("subjects", &self.subjects)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let s = v.field("status")?;
        let status = match s.field("status")?.as_str()? {
            "Proven" => ProofStatus::Proven {
                rule_id: s.field("rule_id")?.as_str()?.to_string(),
                rule_version: s.field("rule_version")?.as_u32()?,
                premises: Vec::from_canon(s.field("premises")?)?,
            },
            "Disproven" => ProofStatus::Disproven {
                counterexample: s.field("counterexample")?.as_bytes()?,
                summary: s.field("summary")?.as_str()?.to_string(),
            },
            "Unknown" => {
                let d = s.field("detail")?.as_str()?.to_string();
                ProofStatus::Unknown(match s.field("reason")?.as_str()? {
                    "UnsupportedFragment" => UnknownReason::UnsupportedFragment(d),
                    "UnsupportedComposition" => UnknownReason::UnsupportedComposition(d),
                    "SolverUnknown" => UnknownReason::SolverUnknown(d),
                    "BudgetExceeded" => UnknownReason::BudgetExceeded,
                    "MissingAuthorityModel" => UnknownReason::MissingAuthorityModel(d),
                    "MissingRuntimeCapability" => UnknownReason::MissingRuntimeCapability(d),
                    k => {
                        return Err(CoreError::new(
                            ErrorCode::NonCanonicalEncoding,
                            format!("unknown reason {k}"),
                        ))
                    }
                })
            }
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("unknown proof status {k}"),
                ))
            }
        };
        Ok(AnalysisJudgment {
            obligation_id: v.field("obligation_id")?.as_str()?.to_string(),
            kind: ObligationKind::from_label(v.field("kind")?.as_str()?).ok_or_else(|| {
                CoreError::new(ErrorCode::NonCanonicalEncoding, "unknown obligation kind")
            })?,
            subjects: Vec::from_canon(v.field("subjects")?)?,
            status,
            assumptions: Vec::from_canon(v.field("assumptions")?)?,
        })
    }
}

// ---------------------------------------------------------------------------
// Execution profile programs (SPEC-004 §3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionCheck {
    PlanHash,
    SchemaHash,
    ContractHash,
    OperationVersion,
    IdcBindings,
    AuthorityGrant,
    NodeReadiness,
    LocalFence,
    ExclusiveWriterEpoch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionProgram {
    pub checks: Vec<AdmissionCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisibilityProgram {
    LocalSnapshot,
    CausalContext { group: String },
    SerialAuthoritative { authority: AuthorityId },
    CertifiedCut { authority: AuthorityId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityProgram {
    ExclusiveLocal { domain: AuthorityId },
    OrderedSerial { domain: AuthorityId, voters: u32 },
    EscrowHolder { domain: AuthorityId },
    Certifier { domain: AuthorityId },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderingProgram {
    /// Deterministic serial order scoped to the listed IDC templates.
    Serial {
        idc_templates: Vec<String>,
    },
    Causal {
        group: String,
    },
    Unordered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationProgram {
    /// Evaluate preconditions, checked effects, postconditions and every invariant of the closure at execution.
    FullEvaluation {
        invariants: Vec<InvariantId>,
    },
    Certification {
        program_version: u32,
    },
    AcceptedEffectsOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtomicityProgram {
    /// One SPEC-002 CompiledBatch at one authority.
    LocalBatch,
    /// SPEC-008 prepare/decision/install/publication/completion across participants.
    CompositePublication { participants: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurabilityProgram {
    LocalStable,
    ReplicatedStable {
        policy: String,
        required_copies: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicationProgram {
    None,
    SemanticEffects { handler: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultProgram {
    pub semantics: ResultSemantics,
    pub scope: ResultScope,
    /// Final receipt persisted at RequestHome with identity + exact result (SPEC-012 §7).
    pub receipt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryContract {
    pub resolve_by_request_key: bool,
    pub in_doubt: String,
    pub timeout_is_not_abort: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionProfile {
    pub family: ConsistencyClass,
    pub admission: AdmissionProgram,
    pub visibility: VisibilityProgram,
    pub authority: AuthorityProgram,
    pub ordering: OrderingProgram,
    pub validation: ValidationProgram,
    pub atomicity: AtomicityProgram,
    pub durability: DurabilityProgram,
    pub replication: ReplicationProgram,
    pub result: ResultProgram,
    pub recovery: RecoveryContract,
}

fn s(v: &str) -> CanonValue {
    CanonValue::str(v)
}

impl Canonical for AdmissionCheck {
    fn to_canon(&self) -> CanonValue {
        s(match self {
            AdmissionCheck::PlanHash => "plan_hash",
            AdmissionCheck::SchemaHash => "schema_hash",
            AdmissionCheck::ContractHash => "contract_hash",
            AdmissionCheck::OperationVersion => "operation_version",
            AdmissionCheck::IdcBindings => "idc_bindings",
            AdmissionCheck::AuthorityGrant => "authority_grant",
            AdmissionCheck::NodeReadiness => "node_readiness",
            AdmissionCheck::LocalFence => "local_fence",
            AdmissionCheck::ExclusiveWriterEpoch => "exclusive_writer_epoch",
        })
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.as_str()? {
            "plan_hash" => AdmissionCheck::PlanHash,
            "schema_hash" => AdmissionCheck::SchemaHash,
            "contract_hash" => AdmissionCheck::ContractHash,
            "operation_version" => AdmissionCheck::OperationVersion,
            "idc_bindings" => AdmissionCheck::IdcBindings,
            "authority_grant" => AdmissionCheck::AuthorityGrant,
            "node_readiness" => AdmissionCheck::NodeReadiness,
            "local_fence" => AdmissionCheck::LocalFence,
            "exclusive_writer_epoch" => AdmissionCheck::ExclusiveWriterEpoch,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("unknown admission check {k}"),
                ))
            }
        })
    }
}

fn vis_label(v: InputVisibility) -> &'static str {
    match v {
        InputVisibility::LocalSnapshot => "LocalSnapshot",
        InputVisibility::CausalContext => "CausalContext",
        InputVisibility::CertifiedScope => "CertifiedScope",
        InputVisibility::SerialScope => "SerialScope",
    }
}

impl Canonical for ExecutionProfile {
    fn to_canon(&self) -> CanonValue {
        let visibility = match &self.visibility {
            VisibilityProgram::LocalSnapshot => {
                CanonValue::obj().fstr("kind", "local_snapshot").build()
            }
            VisibilityProgram::CausalContext { group } => CanonValue::obj()
                .fstr("group", group)
                .fstr("kind", "causal_context")
                .build(),
            VisibilityProgram::SerialAuthoritative { authority } => CanonValue::obj()
                .fc("authority", authority)
                .fstr("kind", "serial_authoritative")
                .build(),
            VisibilityProgram::CertifiedCut { authority } => CanonValue::obj()
                .fc("authority", authority)
                .fstr("kind", "certified_cut")
                .build(),
        };
        let authority = match &self.authority {
            AuthorityProgram::ExclusiveLocal { domain } => CanonValue::obj()
                .fc("domain", domain)
                .fstr("kind", "exclusive_local")
                .build(),
            AuthorityProgram::OrderedSerial { domain, voters } => CanonValue::obj()
                .fc("domain", domain)
                .fstr("kind", "ordered_serial")
                .fu32("voters", *voters)
                .build(),
            AuthorityProgram::EscrowHolder { domain } => CanonValue::obj()
                .fc("domain", domain)
                .fstr("kind", "escrow_holder")
                .build(),
            AuthorityProgram::Certifier { domain } => CanonValue::obj()
                .fc("domain", domain)
                .fstr("kind", "certifier")
                .build(),
            AuthorityProgram::None => CanonValue::obj().fstr("kind", "none").build(),
        };
        let ordering = match &self.ordering {
            OrderingProgram::Serial { idc_templates } => CanonValue::obj()
                .fset("idc_templates", idc_templates)
                .fstr("kind", "serial")
                .build(),
            OrderingProgram::Causal { group } => CanonValue::obj()
                .fstr("group", group)
                .fstr("kind", "causal")
                .build(),
            OrderingProgram::Unordered => CanonValue::obj().fstr("kind", "unordered").build(),
        };
        let validation = match &self.validation {
            ValidationProgram::FullEvaluation { invariants } => CanonValue::obj()
                .fset("invariants", invariants)
                .fstr("kind", "full_evaluation")
                .build(),
            ValidationProgram::Certification { program_version } => CanonValue::obj()
                .fstr("kind", "certification")
                .fu32("program_version", *program_version)
                .build(),
            ValidationProgram::AcceptedEffectsOnly => CanonValue::obj()
                .fstr("kind", "accepted_effects_only")
                .build(),
        };
        let atomicity = match &self.atomicity {
            AtomicityProgram::LocalBatch => CanonValue::obj().fstr("kind", "local_batch").build(),
            AtomicityProgram::CompositePublication { participants } => CanonValue::obj()
                .fstr("kind", "composite_publication")
                .fset("participants", participants)
                .build(),
        };
        let durability = match &self.durability {
            DurabilityProgram::LocalStable => {
                CanonValue::obj().fstr("kind", "local_stable").build()
            }
            DurabilityProgram::ReplicatedStable {
                policy,
                required_copies,
            } => CanonValue::obj()
                .fstr("kind", "replicated_stable")
                .fstr("policy", policy)
                .fu32("required_copies", *required_copies)
                .build(),
        };
        let replication = match &self.replication {
            ReplicationProgram::None => CanonValue::obj().fstr("kind", "none").build(),
            ReplicationProgram::SemanticEffects { handler } => CanonValue::obj()
                .fstr("handler", handler)
                .fstr("kind", "semantic_effects")
                .build(),
        };
        let scope = match &self.result.scope {
            ResultScope::PerKey { record, param } => CanonValue::obj()
                .fstr("kind", "per_key")
                .fu32("param", *param)
                .fc("record", record)
                .build(),
            ResultScope::PerGroup { record, param } => CanonValue::obj()
                .fstr("kind", "per_group")
                .fu32("param", *param)
                .fc("record", record)
                .build(),
            ResultScope::Global { records } => CanonValue::obj()
                .fstr("kind", "global")
                .fvec("records", records)
                .build(),
        };
        let result = CanonValue::obj()
            .fbool("receipt", self.result.receipt)
            .f("scope", scope)
            .fstr(
                "semantics",
                match self.result.semantics {
                    ResultSemantics::Receipt => "Receipt",
                    ResultSemantics::SnapshotValue => "SnapshotValue",
                    ResultSemantics::ExactOrderedValue => "ExactOrderedValue",
                },
            )
            .build();
        let recovery = CanonValue::obj()
            .fstr("in_doubt", &self.recovery.in_doubt)
            .fbool(
                "resolve_by_request_key",
                self.recovery.resolve_by_request_key,
            )
            .fbool("timeout_is_not_abort", self.recovery.timeout_is_not_abort)
            .build();
        CanonValue::obj()
            .f(
                "admission",
                CanonValue::obj()
                    .fvec("checks", &self.admission.checks)
                    .build(),
            )
            .f("atomicity", atomicity)
            .f("authority", authority)
            .f("durability", durability)
            .fc("family", &self.family)
            .fstr("kind", "execution-profile.v1")
            .f("ordering", ordering)
            .f("recovery", recovery)
            .f("replication", replication)
            .f("result", result)
            .f("validation", validation)
            .f("visibility", visibility)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let vis = v.field("visibility")?;
        let visibility = match vis.field("kind")?.as_str()? {
            "local_snapshot" => VisibilityProgram::LocalSnapshot,
            "causal_context" => VisibilityProgram::CausalContext {
                group: vis.field("group")?.as_str()?.to_string(),
            },
            "serial_authoritative" => VisibilityProgram::SerialAuthoritative {
                authority: AuthorityId::from_canon(vis.field("authority")?)?,
            },
            "certified_cut" => VisibilityProgram::CertifiedCut {
                authority: AuthorityId::from_canon(vis.field("authority")?)?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("visibility {k}"),
                ))
            }
        };
        let a = v.field("authority")?;
        let authority = match a.field("kind")?.as_str()? {
            "exclusive_local" => AuthorityProgram::ExclusiveLocal {
                domain: AuthorityId::from_canon(a.field("domain")?)?,
            },
            "ordered_serial" => AuthorityProgram::OrderedSerial {
                domain: AuthorityId::from_canon(a.field("domain")?)?,
                voters: a.field("voters")?.as_u32()?,
            },
            "escrow_holder" => AuthorityProgram::EscrowHolder {
                domain: AuthorityId::from_canon(a.field("domain")?)?,
            },
            "certifier" => AuthorityProgram::Certifier {
                domain: AuthorityId::from_canon(a.field("domain")?)?,
            },
            "none" => AuthorityProgram::None,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("authority {k}"),
                ))
            }
        };
        let o = v.field("ordering")?;
        let ordering = match o.field("kind")?.as_str()? {
            "serial" => OrderingProgram::Serial {
                idc_templates: decode_set(o.field("idc_templates")?)?,
            },
            "causal" => OrderingProgram::Causal {
                group: o.field("group")?.as_str()?.to_string(),
            },
            "unordered" => OrderingProgram::Unordered,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("ordering {k}"),
                ))
            }
        };
        let val = v.field("validation")?;
        let validation = match val.field("kind")?.as_str()? {
            "full_evaluation" => ValidationProgram::FullEvaluation {
                invariants: decode_set(val.field("invariants")?)?,
            },
            "certification" => ValidationProgram::Certification {
                program_version: val.field("program_version")?.as_u32()?,
            },
            "accepted_effects_only" => ValidationProgram::AcceptedEffectsOnly,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("validation {k}"),
                ))
            }
        };
        let at = v.field("atomicity")?;
        let atomicity = match at.field("kind")?.as_str()? {
            "local_batch" => AtomicityProgram::LocalBatch,
            "composite_publication" => AtomicityProgram::CompositePublication {
                participants: decode_set(at.field("participants")?)?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("atomicity {k}"),
                ))
            }
        };
        let d = v.field("durability")?;
        let durability = match d.field("kind")?.as_str()? {
            "local_stable" => DurabilityProgram::LocalStable,
            "replicated_stable" => DurabilityProgram::ReplicatedStable {
                policy: d.field("policy")?.as_str()?.to_string(),
                required_copies: d.field("required_copies")?.as_u32()?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("durability {k}"),
                ))
            }
        };
        let r = v.field("replication")?;
        let replication = match r.field("kind")?.as_str()? {
            "none" => ReplicationProgram::None,
            "semantic_effects" => ReplicationProgram::SemanticEffects {
                handler: r.field("handler")?.as_str()?.to_string(),
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("replication {k}"),
                ))
            }
        };
        let res = v.field("result")?;
        let sc = res.field("scope")?;
        let scope = match sc.field("kind")?.as_str()? {
            "per_key" => ResultScope::PerKey {
                record: RecordId::from_canon(sc.field("record")?)?,
                param: sc.field("param")?.as_u32()?,
            },
            "per_group" => ResultScope::PerGroup {
                record: RecordId::from_canon(sc.field("record")?)?,
                param: sc.field("param")?.as_u32()?,
            },
            "global" => ResultScope::Global {
                records: Vec::from_canon(sc.field("records")?)?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("scope {k}"),
                ))
            }
        };
        let semantics = match res.field("semantics")?.as_str()? {
            "Receipt" => ResultSemantics::Receipt,
            "SnapshotValue" => ResultSemantics::SnapshotValue,
            "ExactOrderedValue" => ResultSemantics::ExactOrderedValue,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("semantics {k}"),
                ))
            }
        };
        let rec = v.field("recovery")?;
        Ok(ExecutionProfile {
            family: ConsistencyClass::from_canon(v.field("family")?)?,
            admission: AdmissionProgram {
                checks: Vec::from_canon(v.field("admission")?.field("checks")?)?,
            },
            visibility,
            authority,
            ordering,
            validation,
            atomicity,
            durability,
            replication,
            result: ResultProgram {
                semantics,
                scope,
                receipt: res.field("receipt")?.as_bool()?,
            },
            recovery: RecoveryContract {
                resolve_by_request_key: rec.field("resolve_by_request_key")?.as_bool()?,
                in_doubt: rec.field("in_doubt")?.as_str()?.to_string(),
                timeout_is_not_abort: rec.field("timeout_is_not_abort")?.as_bool()?,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// Assumptions, routing, plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssumptionPredicate {
    ExclusiveWriterEpoch { domain: AuthorityId },
    OrderedAuthorityActive { domain: AuthorityId },
    AcceptedPlanGeneration,
    StorageReady,
    CapabilityVersion { family: ConsistencyClass },
    DurableWitnessPolicy { policy: String },
    ClosedOperationSet { module_hash: ModuleHash },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enforcement {
    CompileTimeEvidence,
    AdmissionCheck,
    DurableFence,
    ProtocolInvariant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureAction {
    Reject,
    Wait,
    QuiesceAndTransition(PlanRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assumption {
    pub id: String,
    pub predicate: AssumptionPredicate,
    pub enforcement: Enforcement,
    pub failure_action: FailureAction,
}

impl Canonical for Assumption {
    fn to_canon(&self) -> CanonValue {
        let p = match &self.predicate {
            AssumptionPredicate::ExclusiveWriterEpoch { domain } => CanonValue::obj()
                .fc("domain", domain)
                .fstr("kind", "exclusive_writer_epoch")
                .build(),
            AssumptionPredicate::OrderedAuthorityActive { domain } => CanonValue::obj()
                .fc("domain", domain)
                .fstr("kind", "ordered_authority_active")
                .build(),
            AssumptionPredicate::AcceptedPlanGeneration => CanonValue::obj()
                .fstr("kind", "accepted_plan_generation")
                .build(),
            AssumptionPredicate::StorageReady => {
                CanonValue::obj().fstr("kind", "storage_ready").build()
            }
            AssumptionPredicate::CapabilityVersion { family } => CanonValue::obj()
                .fc("family", family)
                .fstr("kind", "capability_version")
                .build(),
            AssumptionPredicate::DurableWitnessPolicy { policy } => CanonValue::obj()
                .fstr("kind", "durable_witness_policy")
                .fstr("policy", policy)
                .build(),
            AssumptionPredicate::ClosedOperationSet { module_hash } => CanonValue::obj()
                .fstr("kind", "closed_operation_set")
                .fc("module_hash", module_hash)
                .build(),
        };
        let fa = match &self.failure_action {
            FailureAction::Reject => CanonValue::obj().fstr("kind", "reject").build(),
            FailureAction::Wait => CanonValue::obj().fstr("kind", "wait").build(),
            FailureAction::QuiesceAndTransition(p) => CanonValue::obj()
                .fstr("kind", "quiesce_and_transition")
                .fc("plan", p)
                .build(),
        };
        CanonValue::obj()
            .fstr(
                "enforcement",
                match self.enforcement {
                    Enforcement::CompileTimeEvidence => "compile_time_evidence",
                    Enforcement::AdmissionCheck => "admission_check",
                    Enforcement::DurableFence => "durable_fence",
                    Enforcement::ProtocolInvariant => "protocol_invariant",
                },
            )
            .f("failure_action", fa)
            .fstr("id", &self.id)
            .f("predicate", p)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let p = v.field("predicate")?;
        let predicate = match p.field("kind")?.as_str()? {
            "exclusive_writer_epoch" => AssumptionPredicate::ExclusiveWriterEpoch {
                domain: AuthorityId::from_canon(p.field("domain")?)?,
            },
            "ordered_authority_active" => AssumptionPredicate::OrderedAuthorityActive {
                domain: AuthorityId::from_canon(p.field("domain")?)?,
            },
            "accepted_plan_generation" => AssumptionPredicate::AcceptedPlanGeneration,
            "storage_ready" => AssumptionPredicate::StorageReady,
            "capability_version" => AssumptionPredicate::CapabilityVersion {
                family: ConsistencyClass::from_canon(p.field("family")?)?,
            },
            "durable_witness_policy" => AssumptionPredicate::DurableWitnessPolicy {
                policy: p.field("policy")?.as_str()?.to_string(),
            },
            "closed_operation_set" => AssumptionPredicate::ClosedOperationSet {
                module_hash: ModuleHash::from_canon(p.field("module_hash")?)?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("assumption {k}"),
                ))
            }
        };
        let enforcement = match v.field("enforcement")?.as_str()? {
            "compile_time_evidence" => Enforcement::CompileTimeEvidence,
            "admission_check" => Enforcement::AdmissionCheck,
            "durable_fence" => Enforcement::DurableFence,
            "protocol_invariant" => Enforcement::ProtocolInvariant,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("enforcement {k}"),
                ))
            }
        };
        let fa = v.field("failure_action")?;
        let failure_action = match fa.field("kind")?.as_str()? {
            "reject" => FailureAction::Reject,
            "wait" => FailureAction::Wait,
            "quiesce_and_transition" => {
                FailureAction::QuiesceAndTransition(PlanRef::from_canon(fa.field("plan")?)?)
            }
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("failure action {k}"),
                ))
            }
        };
        Ok(Assumption {
            id: v.field("id")?.as_str()?.to_string(),
            predicate,
            enforcement,
            failure_action,
        })
    }
}

/// Parameterized IDC template reference carried by a plan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IdcTemplateRef {
    pub template_name: String,
    pub records: Vec<RecordId>,
    /// `true` when instances are parameterized by a primary key; `false` for global components.
    pub per_key: bool,
}

impl Canonical for IdcTemplateRef {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fbool("per_key", self.per_key)
            .fset("records", &self.records)
            .fstr("template_name", &self.template_name)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(IdcTemplateRef {
            template_name: v.field("template_name")?.as_str()?.to_string(),
            records: decode_set(v.field("records")?)?,
            per_key: v.field("per_key")?.as_bool()?,
        })
    }
}

/// Routing program: how concrete IDC instances and participants are computed from canonical arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingProgram {
    /// For each IDC template: the parameter index that keys the instance (None = global instance).
    pub instance_keys: Vec<(String, Option<u32>)>,
    /// Authority domain that executes the invocation.
    pub authority: AuthorityId,
    /// Placement epoch assumed at compile time; validated at admission.
    pub placement_epoch: PlacementEpoch,
}

impl Canonical for RoutingProgram {
    fn to_canon(&self) -> CanonValue {
        let keys: Vec<CanonValue> = self
            .instance_keys
            .iter()
            .map(|(t, k)| {
                CanonValue::obj()
                    .f(
                        "key_param",
                        match k {
                            Some(i) => CanonValue::u32(*i),
                            None => CanonValue::Null,
                        },
                    )
                    .fstr("template", t)
                    .build()
            })
            .collect();
        CanonValue::obj()
            .fc("authority", &self.authority)
            .f("instance_keys", CanonValue::Array(keys))
            .fc("placement_epoch", &self.placement_epoch)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let mut instance_keys = Vec::new();
        for k in v.field("instance_keys")?.as_array()? {
            let kp = k.field("key_param")?;
            instance_keys.push((
                k.field("template")?.as_str()?.to_string(),
                if kp.is_null() {
                    None
                } else {
                    Some(kp.as_u32()?)
                },
            ));
        }
        Ok(RoutingProgram {
            instance_keys,
            authority: AuthorityId::from_canon(v.field("authority")?)?,
            placement_epoch: PlacementEpoch::from_canon(v.field("placement_epoch")?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationRequirement {
    pub target: String,
    pub requirement: String,
}

impl Canonical for MigrationRequirement {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("requirement", &self.requirement)
            .fstr("target", &self.target)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(MigrationRequirement {
            target: v.field("target")?.as_str()?.to_string(),
            requirement: v.field("requirement")?.as_str()?.to_string(),
        })
    }
}

/// Immutable executable operation plan (SPEC-004 §12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationPlan {
    pub plan_format_version: u32,
    pub plan_id: PlanId,
    pub operation: OperationRef,
    pub operation_name: String,
    pub operation_hash: OperationHash,
    pub schema_hash: SchemaHash,
    pub contract_hash: ContractHash,
    pub invariant_set_hash: Hash256,
    pub module_hash: ModuleHash,
    pub catalog_generation: CatalogGeneration,
    pub plan_generation: PlanGeneration,
    pub idc_templates: Vec<IdcTemplateRef>,
    pub template_id: TemplateId,
    pub template_version: u32,
    pub profile: ExecutionProfile,
    pub routing: RoutingProgram,
    pub assumptions: Vec<Assumption>,
    pub fallback_plan_refs: Vec<PlanRef>,
    pub migration_requirements: Vec<MigrationRequirement>,
    pub protocol_capabilities: Vec<String>,
    pub cost: CostDescriptor,
}

impl OperationPlan {
    pub fn plan_hash(&self) -> PlanHash {
        PlanHash(domain_hash(domains::PLAN_V1, &self.encode()))
    }
    pub fn plan_ref(&self) -> PlanRef {
        PlanRef {
            plan_id: self.plan_id,
            generation: self.plan_generation,
            hash: self.plan_hash(),
        }
    }
}

impl Canonical for OperationPlan {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("assumptions", &self.assumptions)
            .fc("catalog_generation", &self.catalog_generation)
            .fc("contract_hash", &self.contract_hash)
            .fc("cost", &self.cost)
            .fset("fallback_plan_refs", &self.fallback_plan_refs)
            .fvec("idc_templates", &self.idc_templates)
            .fc("invariant_set_hash", &self.invariant_set_hash)
            .fstr("kind", "plan.v1")
            .fvec("migration_requirements", &self.migration_requirements)
            .fc("module_hash", &self.module_hash)
            .fc("operation", &self.operation)
            .fc("operation_hash", &self.operation_hash)
            .fstr("operation_name", &self.operation_name)
            .fu32("plan_format_version", self.plan_format_version)
            .fc("plan_generation", &self.plan_generation)
            .fc("plan_id", &self.plan_id)
            .fc("profile", &self.profile)
            .fset("protocol_capabilities", &self.protocol_capabilities)
            .fc("routing", &self.routing)
            .fc("schema_hash", &self.schema_hash)
            .fc("template_id", &self.template_id)
            .fu32("template_version", self.template_version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "assumptions",
            "catalog_generation",
            "contract_hash",
            "cost",
            "fallback_plan_refs",
            "idc_templates",
            "invariant_set_hash",
            "kind",
            "migration_requirements",
            "module_hash",
            "operation",
            "operation_hash",
            "operation_name",
            "plan_format_version",
            "plan_generation",
            "plan_id",
            "profile",
            "protocol_capabilities",
            "routing",
            "schema_hash",
            "template_id",
            "template_version",
        ])?;
        if v.field("kind")?.as_str()? != "plan.v1" {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "expected plan.v1",
            ));
        }
        let fmt = v.field("plan_format_version")?.as_u32()?;
        if fmt != PLAN_FORMAT_VERSION {
            return Err(CoreError::new(
                ErrorCode::UnsupportedIrVersion,
                format!("plan format {fmt} unsupported"),
            ));
        }
        Ok(OperationPlan {
            plan_format_version: fmt,
            plan_id: PlanId::from_canon(v.field("plan_id")?)?,
            operation: OperationRef::from_canon(v.field("operation")?)?,
            operation_name: v.field("operation_name")?.as_str()?.to_string(),
            operation_hash: OperationHash::from_canon(v.field("operation_hash")?)?,
            schema_hash: SchemaHash::from_canon(v.field("schema_hash")?)?,
            contract_hash: ContractHash::from_canon(v.field("contract_hash")?)?,
            invariant_set_hash: Hash256::from_canon(v.field("invariant_set_hash")?)?,
            module_hash: ModuleHash::from_canon(v.field("module_hash")?)?,
            catalog_generation: CatalogGeneration::from_canon(v.field("catalog_generation")?)?,
            plan_generation: PlanGeneration::from_canon(v.field("plan_generation")?)?,
            idc_templates: Vec::from_canon(v.field("idc_templates")?)?,
            template_id: TemplateId::from_canon(v.field("template_id")?)?,
            template_version: v.field("template_version")?.as_u32()?,
            profile: ExecutionProfile::from_canon(v.field("profile")?)?,
            routing: RoutingProgram::from_canon(v.field("routing")?)?,
            assumptions: Vec::from_canon(v.field("assumptions")?)?,
            fallback_plan_refs: decode_set(v.field("fallback_plan_refs")?)?,
            migration_requirements: Vec::from_canon(v.field("migration_requirements")?)?,
            protocol_capabilities: decode_set(v.field("protocol_capabilities")?)?,
            cost: CostDescriptor::from_canon(v.field("cost")?)?,
        })
    }
}

// ---------------------------------------------------------------------------
// Candidates, compatibility, certificate (SPEC-004 §11, §13)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRejection {
    pub operation: OperationRef,
    pub operation_name: String,
    pub template_id: TemplateId,
    pub template_name: String,
    pub family: ConsistencyClass,
    /// Obligation ids that were not proven and why.
    pub failed: Vec<(String, String)>,
}

impl Canonical for CandidateRejection {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .f(
                "failed",
                CanonValue::Array(
                    self.failed
                        .iter()
                        .map(|(o, r)| {
                            CanonValue::obj()
                                .fstr("obligation", o)
                                .fstr("reason", r)
                                .build()
                        })
                        .collect(),
                ),
            )
            .fc("family", &self.family)
            .fc("operation", &self.operation)
            .fstr("operation_name", &self.operation_name)
            .fc("template_id", &self.template_id)
            .fstr("template_name", &self.template_name)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let mut failed = Vec::new();
        for f in v.field("failed")?.as_array()? {
            failed.push((
                f.field("obligation")?.as_str()?.to_string(),
                f.field("reason")?.as_str()?.to_string(),
            ));
        }
        Ok(CandidateRejection {
            operation: OperationRef::from_canon(v.field("operation")?)?,
            operation_name: v.field("operation_name")?.as_str()?.to_string(),
            template_id: TemplateId::from_canon(v.field("template_id")?)?,
            template_name: v.field("template_name")?.as_str()?.to_string(),
            family: ConsistencyClass::from_canon(v.field("family")?)?,
            failed,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityRequirement {
    pub left: OperationRef,
    pub right: OperationRef,
    pub interaction_scope: Vec<RecordId>,
    pub rule: String,
    pub shared_authority: Vec<AuthorityId>,
    pub dependencies: Vec<InteractionEdge>,
    pub assumptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionKind {
    RequiresVisible,
    RequiresAuthority,
    InvalidatesGuard,
    RequiresDrain,
    ExcludesConcurrent,
    AtomicWith,
}

impl InteractionKind {
    pub fn label(&self) -> &'static str {
        match self {
            InteractionKind::RequiresVisible => "RequiresVisible",
            InteractionKind::RequiresAuthority => "RequiresAuthority",
            InteractionKind::InvalidatesGuard => "InvalidatesGuard",
            InteractionKind::RequiresDrain => "RequiresDrain",
            InteractionKind::ExcludesConcurrent => "ExcludesConcurrent",
            InteractionKind::AtomicWith => "AtomicWith",
        }
    }
}

/// Directed interaction edge (SPEC-004 §6). Direction is semantic; the reverse edge is never inferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionEdge {
    pub predecessor: OperationRef,
    pub successor: OperationRef,
    pub kind: InteractionKind,
    pub key_relation: String,
    pub invariant_refs: Vec<InvariantId>,
}

impl Canonical for InteractionEdge {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fset("invariant_refs", &self.invariant_refs)
            .fstr("key_relation", &self.key_relation)
            .fstr("kind", self.kind.label())
            .fc("predecessor", &self.predecessor)
            .fc("successor", &self.successor)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let kind = match v.field("kind")?.as_str()? {
            "RequiresVisible" => InteractionKind::RequiresVisible,
            "RequiresAuthority" => InteractionKind::RequiresAuthority,
            "InvalidatesGuard" => InteractionKind::InvalidatesGuard,
            "RequiresDrain" => InteractionKind::RequiresDrain,
            "ExcludesConcurrent" => InteractionKind::ExcludesConcurrent,
            "AtomicWith" => InteractionKind::AtomicWith,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("interaction {k}"),
                ))
            }
        };
        Ok(InteractionEdge {
            predecessor: OperationRef::from_canon(v.field("predecessor")?)?,
            successor: OperationRef::from_canon(v.field("successor")?)?,
            kind,
            key_relation: v.field("key_relation")?.as_str()?.to_string(),
            invariant_refs: decode_set(v.field("invariant_refs")?)?,
        })
    }
}

impl Canonical for CompatibilityRequirement {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("assumptions", &self.assumptions)
            .fvec("dependencies", &self.dependencies)
            .fset("interaction_scope", &self.interaction_scope)
            .fc("left", &self.left)
            .fc("right", &self.right)
            .fstr("rule", &self.rule)
            .fset("shared_authority", &self.shared_authority)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(CompatibilityRequirement {
            left: OperationRef::from_canon(v.field("left")?)?,
            right: OperationRef::from_canon(v.field("right")?)?,
            interaction_scope: decode_set(v.field("interaction_scope")?)?,
            rule: v.field("rule")?.as_str()?.to_string(),
            shared_authority: decode_set(v.field("shared_authority")?)?,
            dependencies: Vec::from_canon(v.field("dependencies")?)?,
            assumptions: Vec::from_canon(v.field("assumptions")?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileInputHashes {
    pub module_hash: ModuleHash,
    pub schema_hash: SchemaHash,
    pub topology_hash: Hash256,
    pub policy_hash: Hash256,
    pub budget_hash: Hash256,
}

impl Canonical for CompileInputHashes {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("budget_hash", &self.budget_hash)
            .fc("module_hash", &self.module_hash)
            .fc("policy_hash", &self.policy_hash)
            .fc("schema_hash", &self.schema_hash)
            .fc("topology_hash", &self.topology_hash)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(CompileInputHashes {
            module_hash: ModuleHash::from_canon(v.field("module_hash")?)?,
            schema_hash: SchemaHash::from_canon(v.field("schema_hash")?)?,
            topology_hash: Hash256::from_canon(v.field("topology_hash")?)?,
            policy_hash: Hash256::from_canon(v.field("policy_hash")?)?,
            budget_hash: Hash256::from_canon(v.field("budget_hash")?)?,
        })
    }
}

/// Consistency certificate (SPEC-004 §13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsistencyCertificate {
    pub certificate_version: u32,
    pub compiler_build_hash: Hash256,
    pub input_hashes: CompileInputHashes,
    pub rule_manifest_hash: Hash256,
    pub protocol_library_hash: Hash256,
    pub plan_refs: Vec<PlanHash>,
    pub judgments: Vec<AnalysisJudgment>,
    pub rejected_candidates: Vec<CandidateRejection>,
    pub compatibility_rules: Vec<CompatibilityRequirement>,
    pub assumptions: Vec<Assumption>,
    /// Evidence references: (label, hash)
    pub evidence_manifest: Vec<(String, Hash256)>,
}

impl ConsistencyCertificate {
    pub fn certificate_hash(&self) -> CertificateHash {
        CertificateHash(domain_hash(domains::CERTIFICATE_V1, &self.encode()))
    }
}

impl Canonical for ConsistencyCertificate {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("assumptions", &self.assumptions)
            .fu32("certificate_version", self.certificate_version)
            .fvec("compatibility_rules", &self.compatibility_rules)
            .fc("compiler_build_hash", &self.compiler_build_hash)
            .f(
                "evidence_manifest",
                CanonValue::Array(
                    self.evidence_manifest
                        .iter()
                        .map(|(l, h)| CanonValue::obj().fc("hash", h).fstr("label", l).build())
                        .collect(),
                ),
            )
            .fc("input_hashes", &self.input_hashes)
            .fvec("judgments", &self.judgments)
            .fstr("kind", "certificate.v1")
            .fset("plan_refs", &self.plan_refs)
            .fc("protocol_library_hash", &self.protocol_library_hash)
            .fvec("rejected_candidates", &self.rejected_candidates)
            .fc("rule_manifest_hash", &self.rule_manifest_hash)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "assumptions",
            "certificate_version",
            "compatibility_rules",
            "compiler_build_hash",
            "evidence_manifest",
            "input_hashes",
            "judgments",
            "kind",
            "plan_refs",
            "protocol_library_hash",
            "rejected_candidates",
            "rule_manifest_hash",
        ])?;
        let mut evidence_manifest = Vec::new();
        for e in v.field("evidence_manifest")?.as_array()? {
            evidence_manifest.push((
                e.field("label")?.as_str()?.to_string(),
                Hash256::from_canon(e.field("hash")?)?,
            ));
        }
        Ok(ConsistencyCertificate {
            certificate_version: v.field("certificate_version")?.as_u32()?,
            compiler_build_hash: Hash256::from_canon(v.field("compiler_build_hash")?)?,
            input_hashes: CompileInputHashes::from_canon(v.field("input_hashes")?)?,
            rule_manifest_hash: Hash256::from_canon(v.field("rule_manifest_hash")?)?,
            protocol_library_hash: Hash256::from_canon(v.field("protocol_library_hash")?)?,
            plan_refs: decode_set(v.field("plan_refs")?)?,
            judgments: Vec::from_canon(v.field("judgments")?)?,
            rejected_candidates: Vec::from_canon(v.field("rejected_candidates")?)?,
            compatibility_rules: Vec::from_canon(v.field("compatibility_rules")?)?,
            assumptions: Vec::from_canon(v.field("assumptions")?)?,
            evidence_manifest,
        })
    }
}

impl Canonical for SessionGuaranteeSet {
    fn to_canon(&self) -> CanonValue {
        CanonValue::set(
            self.0
                .iter()
                .map(|g| {
                    CanonValue::str(match g {
                        SessionGuarantee::ReadYourWrites => "ReadYourWrites",
                        SessionGuarantee::MonotonicReads => "MonotonicReads",
                        SessionGuarantee::CausalDependencies => "CausalDependencies",
                    })
                })
                .collect(),
        )
        .expect("unique")
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let mut out = Vec::new();
        for s in decode_set::<String>(v)? {
            out.push(match s.as_str() {
                "ReadYourWrites" => SessionGuarantee::ReadYourWrites,
                "MonotonicReads" => SessionGuarantee::MonotonicReads,
                "CausalDependencies" => SessionGuarantee::CausalDependencies,
                k => {
                    return Err(CoreError::new(
                        ErrorCode::NonCanonicalEncoding,
                        format!("session guarantee {k}"),
                    ))
                }
            });
        }
        Ok(SessionGuaranteeSet(out))
    }
}

/// Helper newtype for canonical session sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionGuaranteeSet(pub Vec<SessionGuarantee>);

#[allow(dead_code)]
fn _vis(v: InputVisibility) -> &'static str {
    vis_label(v)
}
