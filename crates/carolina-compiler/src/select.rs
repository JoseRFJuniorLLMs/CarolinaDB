//! Joint deterministic selection, plan construction and certificate assembly (SPEC-004 §11–§13).
//!
//! Selection starts from the qualified conservative candidate (C5) for every closure and
//! visits lower-cost replacements in canonical `(template, operation)` order. A replacement is
//! accepted only if every operation of the interacting closure accepts it with all obligations
//! proven. This is a deterministic safe heuristic, not a global optimum.

use std::collections::{BTreeMap, BTreeSet};

use carolina_core::canon::Canonical;
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, sha256, Hash256};
use carolina_core::ids::*;
use carolina_lang::ir::*;

use crate::analysis::{
    analyze_candidate, build_closure, name_of, CandidateAnalysis, Closure, PairCache,
};
use crate::input::{AuthorityKind, CompileInput};
use crate::library::{T_C0_LOCAL, T_C5_SERIAL};
use crate::plan::*;

pub struct CompileOutput {
    pub plans: Vec<OperationPlan>,
    pub idc_templates: Vec<IdcTemplateRef>,
    pub interaction_graph: Vec<InteractionEdge>,
    pub certificate: ConsistencyCertificate,
    pub diagnostics: Vec<String>,
    pub migration_requirements: Vec<MigrationRequirement>,
    pub closure: Closure,
    pub candidates: Vec<CandidateAnalysis>,
}

pub const COMPILER_VERSION: &str = "carolina-compiler/0.1.0";

pub fn compiler_build_hash(input: &CompileInput) -> Hash256 {
    let mut payload = Vec::new();
    payload.extend_from_slice(COMPILER_VERSION.as_bytes());
    payload.push(0);
    payload.extend_from_slice(input.analysis_rules.hash().as_bytes());
    payload.extend_from_slice(input.protocol_library.hash().as_bytes());
    sha256(&payload)
}

pub fn compile(input: &CompileInput) -> CoreResult<CompileOutput> {
    let module = &input.module_ir;
    validate_input(input)?;
    let hashes = module.hashes();
    let closure = build_closure(module);
    let mut cache = PairCache::new();

    // analyze every (operation, template) candidate in canonical order
    let mut candidates: Vec<CandidateAnalysis> = Vec::new();
    for op in &module.operations {
        for t in &input.protocol_library.templates {
            candidates.push(analyze_candidate(input, &closure, op, t, &mut cache)?);
        }
    }

    // ---- selection ------------------------------------------------------
    // closures = connected components over op_closure
    let mut selected: BTreeMap<OperationRef, TemplateId> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let accepted = |op: OperationRef, t: TemplateId| {
        candidates
            .iter()
            .any(|c| c.operation == op && c.template.id == t && c.accepted())
    };
    let mut components: Vec<BTreeSet<OperationRef>> = Vec::new();
    let mut seen: BTreeSet<OperationRef> = BTreeSet::new();
    for op in &module.operations {
        if seen.contains(&op.identity) {
            continue;
        }
        let mut comp = BTreeSet::new();
        let mut stack = vec![op.identity];
        while let Some(o) = stack.pop() {
            if !comp.insert(o) {
                continue;
            }
            for p in &closure.op_closure[&o] {
                if !comp.contains(p) {
                    stack.push(*p);
                }
            }
        }
        seen.extend(comp.iter().copied());
        components.push(comp);
    }
    let force_serial: BTreeSet<OperationRef> = module
        .operations
        .iter()
        .filter(|o| input.policy.force_serial.contains(&o.name))
        .map(|o| o.identity)
        .collect();
    for comp in &components {
        // conservative baseline: C5 for every member
        let base_ok = comp.iter().all(|o| accepted(*o, T_C5_SERIAL));
        if !base_ok {
            let names: Vec<String> = comp.iter().map(|o| name_of(module, *o)).collect();
            let failing: Vec<String> = comp
                .iter()
                .filter(|o| !accepted(**o, T_C5_SERIAL))
                .flat_map(|o| {
                    candidates
                        .iter()
                        .filter(|c| c.operation == *o && c.template.id == T_C5_SERIAL)
                        .flat_map(|c| c.failures())
                        .map(|(k, why)| format!("{k}: {why}"))
                        .collect::<Vec<_>>()
                })
                .collect();
            // classify the most specific error
            let code = classify_failure(&failing);
            return Err(CoreError::new(
                code,
                format!(
                    "no safe plan for closure [{}]: {}",
                    names.join(", "),
                    failing.join("; ")
                ),
            ));
        }
        for o in comp {
            selected.insert(*o, T_C5_SERIAL);
        }
        // lower-cost replacements in canonical template order (by cost then id)
        let mut sorted: Vec<_> = input.protocol_library.templates.iter().collect();
        sorted.sort_by_key(|t| t.cost);
        for t in sorted {
            if t.id == T_C5_SERIAL {
                continue;
            }
            let c5_cost = input.protocol_library.template(T_C5_SERIAL).unwrap().cost;
            if t.cost >= c5_cost {
                continue;
            }
            if comp.iter().any(|o| force_serial.contains(o)) {
                diagnostics.push(format!(
                    "policy force_serial keeps [{}] on C5",
                    comp.iter()
                        .map(|o| name_of(module, *o))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                break;
            }
            if comp.iter().all(|o| accepted(*o, t.id)) {
                for o in comp {
                    selected.insert(*o, t.id);
                }
                diagnostics.push(format!(
                    "closure [{}] replaced C5 by {} (all obligations proven)",
                    comp.iter()
                        .map(|o| name_of(module, *o))
                        .collect::<Vec<_>>()
                        .join(", "),
                    t.name
                ));
                break;
            }
        }
    }

    // ---- plans ------------------------------------------------------------
    let idc_templates: Vec<IdcTemplateRef> = closure
        .templates
        .iter()
        .map(|t| IdcTemplateRef {
            template_name: t.name.clone(),
            records: t.records.clone(),
            per_key: t.per_key,
        })
        .collect();
    let invariant_set_hash = {
        let mut ids: Vec<String> = module
            .invariants
            .iter()
            .map(|i| hashes.invariant_hashes[&i.id].to_hex())
            .collect();
        ids.sort();
        domain_hash("astra.invariant-set.v1", ids.join("\n").as_bytes())
    };
    let mut plans = Vec::new();
    let mut all_assumptions: Vec<Assumption> = Vec::new();
    let mut migration_requirements = Vec::new();
    for op in &module.operations {
        let tid = selected[&op.identity];
        let cand = candidates
            .iter()
            .find(|c| c.operation == op.identity && c.template.id == tid)
            .unwrap();
        let t = &cand.template;
        let authority = cand.authority.unwrap_or(AuthorityId::NIL);
        let domain = input
            .topology
            .authority_domains
            .iter()
            .find(|d| d.id == authority);
        let voters = match domain.map(|d| &d.kind) {
            Some(AuthorityKind::OrderedSerial { voters }) => *voters,
            _ => 1,
        };
        let mut op_templates: Vec<String> = closure.op_templates[&op.identity]
            .iter()
            .map(|i| closure.templates[*i].name.clone())
            .collect();
        op_templates.sort();
        let invariants: Vec<InvariantId> = closure.op_invariants[&op.identity]
            .iter()
            .copied()
            .collect();
        let profile = ExecutionProfile {
            family: t.family,
            admission: AdmissionProgram {
                checks: {
                    let mut c = vec![
                        AdmissionCheck::PlanHash,
                        AdmissionCheck::SchemaHash,
                        AdmissionCheck::ContractHash,
                        AdmissionCheck::OperationVersion,
                        AdmissionCheck::IdcBindings,
                        AdmissionCheck::AuthorityGrant,
                        AdmissionCheck::NodeReadiness,
                        AdmissionCheck::LocalFence,
                    ];
                    if tid == T_C0_LOCAL {
                        c.push(AdmissionCheck::ExclusiveWriterEpoch);
                    }
                    c
                },
            },
            visibility: VisibilityProgram::SerialAuthoritative { authority },
            authority: if tid == T_C0_LOCAL {
                AuthorityProgram::ExclusiveLocal { domain: authority }
            } else {
                AuthorityProgram::OrderedSerial {
                    domain: authority,
                    voters,
                }
            },
            ordering: OrderingProgram::Serial {
                idc_templates: op_templates.clone(),
            },
            validation: ValidationProgram::FullEvaluation {
                invariants: invariants.clone(),
            },
            atomicity: AtomicityProgram::LocalBatch,
            durability: match &op.contract.durability {
                Durability::LocalStable => DurabilityProgram::LocalStable,
                Durability::ReplicatedStable(p) => DurabilityProgram::ReplicatedStable {
                    policy: p.clone(),
                    required_copies: input
                        .topology
                        .policy(p)
                        .map(|x| x.required_durable_copies)
                        .unwrap_or(0),
                },
            },
            replication: ReplicationProgram::None,
            result: ResultProgram {
                semantics: op.contract.result_semantics,
                scope: op.contract.result_scope.clone(),
                receipt: true,
            },
            recovery: RecoveryContract {
                resolve_by_request_key: true,
                in_doubt: "block until the durable decision is recovered; never infer abort".into(),
                timeout_is_not_abort: true,
            },
        };
        let mut assumptions = vec![
            Assumption {
                id: format!("{}:authority", op.name),
                predicate: if tid == T_C0_LOCAL {
                    AssumptionPredicate::ExclusiveWriterEpoch { domain: authority }
                } else {
                    AssumptionPredicate::OrderedAuthorityActive { domain: authority }
                },
                enforcement: Enforcement::DurableFence,
                failure_action: FailureAction::Reject,
            },
            Assumption {
                id: format!("{}:plan-generation", op.name),
                predicate: AssumptionPredicate::AcceptedPlanGeneration,
                enforcement: Enforcement::AdmissionCheck,
                failure_action: FailureAction::Reject,
            },
            Assumption {
                id: format!("{}:storage-ready", op.name),
                predicate: AssumptionPredicate::StorageReady,
                enforcement: Enforcement::AdmissionCheck,
                failure_action: FailureAction::Wait,
            },
            Assumption {
                id: format!("{}:closed-operation-set", op.name),
                predicate: AssumptionPredicate::ClosedOperationSet {
                    module_hash: hashes.module_hash,
                },
                enforcement: Enforcement::CompileTimeEvidence,
                failure_action: FailureAction::Reject,
            },
            Assumption {
                id: format!("{}:capability", op.name),
                predicate: AssumptionPredicate::CapabilityVersion { family: t.family },
                enforcement: Enforcement::AdmissionCheck,
                failure_action: FailureAction::Reject,
            },
        ];
        if let Durability::ReplicatedStable(p) = &op.contract.durability {
            assumptions.push(Assumption {
                id: format!("{}:durability", op.name),
                predicate: AssumptionPredicate::DurableWitnessPolicy { policy: p.clone() },
                enforcement: Enforcement::ProtocolInvariant,
                failure_action: FailureAction::Wait,
            });
        }
        // migration requirements for any advertised alternative (SPEC-004 §14): the other qualified template
        for alt in &input.protocol_library.templates {
            if alt.id != tid && alt.qualified && accepted(op.identity, alt.id) {
                migration_requirements.push(MigrationRequirement { target: format!("{}:{}", op.name, alt.name), requirement: "close/drain old authority, resolve prepared work, retain request outcomes, install and activate through catalog CAS (SPEC-009)".into() });
            }
        }
        let routing = RoutingProgram {
            instance_keys: closure.op_templates[&op.identity]
                .iter()
                .map(|i| {
                    let t = &closure.templates[*i];
                    let key = if t.per_key {
                        match &op.contract.result_scope {
                            ResultScope::PerKey { param, record }
                            | ResultScope::PerGroup { param, record }
                                if t.records.contains(record) =>
                            {
                                Some(*param)
                            }
                            _ => first_static_param(op, &t.records),
                        }
                    } else {
                        None
                    };
                    (t.name.clone(), key)
                })
                .collect(),
            authority,
            placement_epoch: input
                .topology
                .placements
                .first()
                .map(|p| p.placement_epoch)
                .unwrap_or(PlacementEpoch(0)),
        };
        let plan_generation = PlanGeneration(
            input
                .prior_plans
                .iter()
                .map(|p| p.generation.0)
                .max()
                .unwrap_or(0)
                + 1,
        );
        let plan = OperationPlan {
            plan_format_version: PLAN_FORMAT_VERSION,
            plan_id: PlanId::derive(&format!(
                "{}:{}@{}:{}",
                hashes.module_hash, op.name, op.identity.version, t.name
            )),
            operation: op.identity,
            operation_name: op.name.clone(),
            operation_hash: hashes.operation_hashes[&op.identity],
            schema_hash: hashes.schema_hash,
            contract_hash: hashes.contract_hashes[&op.identity],
            invariant_set_hash,
            module_hash: hashes.module_hash,
            catalog_generation: input.active_generation,
            plan_generation,
            idc_templates: closure.op_templates[&op.identity]
                .iter()
                .map(|i| idc_templates[*i].clone())
                .collect(),
            template_id: t.id,
            template_version: t.version,
            profile,
            routing,
            assumptions: assumptions.clone(),
            fallback_plan_refs: vec![],
            migration_requirements: migration_requirements
                .iter()
                .filter(|m| m.target.starts_with(&format!("{}:", op.name)))
                .cloned()
                .collect(),
            protocol_capabilities: {
                let mut caps = vec![
                    format!("{}:{}", t.name, t.version),
                    "wire:1.0".into(),
                    "ir:1".into(),
                ];
                caps.sort();
                caps
            },
            cost: t.cost,
        };
        all_assumptions.extend(assumptions);
        plans.push(plan);
    }

    // ---- certificate ------------------------------------------------------
    let mut compatibility_rules = Vec::new();
    for comp in &components {
        let ops: Vec<OperationRef> = comp.iter().copied().collect();
        for (i, a) in ops.iter().enumerate() {
            for b in ops.iter().skip(i) {
                let scope: Vec<RecordId> = closure.op_records[a]
                    .intersection(&closure.op_records[b])
                    .copied()
                    .collect();
                let deps: Vec<InteractionEdge> = closure
                    .interactions
                    .iter()
                    .filter(|e| {
                        (e.predecessor == *a && e.successor == *b)
                            || (e.predecessor == *b && e.successor == *a)
                    })
                    .cloned()
                    .collect();
                let auth = plans
                    .iter()
                    .find(|p| p.operation == *a)
                    .map(|p| p.routing.authority)
                    .unwrap_or(AuthorityId::NIL);
                compatibility_rules.push(CompatibilityRequirement {
                    left: *a,
                    right: *b,
                    interaction_scope: scope,
                    rule: crate::rules::R_COMP_SAME_AUTHORITY.into(),
                    shared_authority: vec![auth],
                    dependencies: deps,
                    assumptions: vec!["same serial authority orders both operations".into()],
                });
            }
        }
    }
    let rejected: Vec<CandidateRejection> = candidates
        .iter()
        .filter(|c| !c.accepted())
        .map(|c| CandidateRejection {
            operation: c.operation,
            operation_name: name_of(module, c.operation),
            template_id: c.template.id,
            template_name: c.template.name.clone(),
            family: c.template.family,
            failed: c.failures(),
        })
        .collect();
    let mut evidence_manifest = Vec::new();
    for c in &candidates {
        for j in &c.judgments {
            if let ProofStatus::Disproven { counterexample, .. } = &j.status {
                evidence_manifest.push((
                    format!("counterexample:{}", j.obligation_id),
                    sha256(counterexample),
                ));
            }
        }
    }
    let judgments: Vec<AnalysisJudgment> = candidates
        .iter()
        .flat_map(|c| c.judgments.iter().cloned())
        .collect();
    let certificate = ConsistencyCertificate {
        certificate_version: CERTIFICATE_VERSION,
        compiler_build_hash: compiler_build_hash(input),
        input_hashes: CompileInputHashes {
            module_hash: hashes.module_hash,
            schema_hash: hashes.schema_hash,
            topology_hash: input.topology.topology_hash(),
            policy_hash: domain_hash("astra.compile-policy.v1", &input.policy.encode()),
            budget_hash: domain_hash("astra.analysis-budget.v1", &input.analysis_budget.encode()),
        },
        rule_manifest_hash: input.analysis_rules.hash(),
        protocol_library_hash: input.protocol_library.hash(),
        plan_refs: {
            let mut v: Vec<PlanHash> = plans.iter().map(|p| p.plan_hash()).collect();
            v.sort();
            v.dedup();
            v
        },
        judgments,
        rejected_candidates: rejected,
        compatibility_rules,
        assumptions: all_assumptions,
        evidence_manifest,
    };
    Ok(CompileOutput {
        plans,
        idc_templates,
        interaction_graph: closure.interactions.clone(),
        certificate,
        diagnostics,
        migration_requirements,
        closure,
        candidates,
    })
}

fn first_static_param(op: &OperationIR, records: &[RecordId]) -> Option<u32> {
    for d in op.footprint.writes.iter().chain(&op.footprint.reads) {
        if records.contains(&d.record) {
            if let KeySet::Point {
                key,
                static_key: true,
            } = &d.key_set
            {
                if let ExprNodeIR::Param(i) = key.node {
                    return Some(i);
                }
            }
        }
    }
    None
}

fn classify_failure(failing: &[String]) -> ErrorCode {
    let joined = failing.join(" ");
    if joined.contains("DURABILITY") && joined.contains("MissingAuthorityModel") {
        ErrorCode::UnsatisfiableDurability
    } else if joined.contains("SESSION") && joined.contains("UnsupportedComposition") {
        ErrorCode::UnsupportedSessionScope
    } else if joined.contains("IR-OBS") {
        ErrorCode::UnmetObservationContract
    } else {
        ErrorCode::NoSafePlan
    }
}

fn validate_input(input: &CompileInput) -> CoreResult<()> {
    let m = &input.module_ir;
    if m.ir_version != IR_VERSION {
        return Err(CoreError::new(
            ErrorCode::InvalidIr,
            "unsupported IR version",
        ));
    }
    if m.operations.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidIr,
            "module declares no operations",
        ));
    }
    if input.topology.authority_domains.is_empty() {
        return Err(CoreError::new(
            ErrorCode::MissingRuntimeCapability,
            "topology declares no authority domains",
        ));
    }
    for op in &m.operations {
        if op.contract.request_namespace.is_empty() {
            return Err(CoreError::new(
                ErrorCode::IncompleteContract,
                format!("operation {} has no request namespace", op.name),
            ));
        }
    }
    Ok(())
}

/// Artifact checker (SPEC-004 §13, S004-A12): canonical bytes, hashes, obligation coverage and references.
pub fn check_artifacts(
    plans: &[OperationPlan],
    certificate: &ConsistencyCertificate,
    rules: &crate::rules::AnalysisRuleManifest,
    library: &crate::library::ProtocolLibraryManifest,
) -> CoreResult<CheckReport> {
    let mut report = CheckReport::default();
    for p in plans {
        let bytes = p.encode();
        let back = OperationPlan::decode(&bytes, &carolina_core::limits::Limits::v1())?;
        if back != *p {
            return Err(CoreError::new(
                ErrorCode::InvalidEvidence,
                "plan canonical roundtrip mismatch",
            ));
        }
        let h = p.plan_hash();
        if !certificate.plan_refs.contains(&h) {
            return Err(CoreError::new(
                ErrorCode::InvalidEvidence,
                format!("plan {} not referenced by certificate", p.operation_name),
            ));
        }
        if library.template(p.template_id).map(|t| t.version) != Some(p.template_version) {
            return Err(CoreError::new(
                ErrorCode::InvalidEvidence,
                "unknown template version in plan",
            ));
        }
        report.structurally_valid += 1;
        // obligation coverage: every mandatory kind proven for the selected template
        let needed = [
            ObligationKind::IrDef,
            ObligationKind::IrSeq,
            ObligationKind::IrConc,
            ObligationKind::IrObs,
            ObligationKind::IrFoot,
            ObligationKind::IrAtom,
            ObligationKind::IrComp,
            ObligationKind::Durability,
            ObligationKind::Session,
            ObligationKind::Authority,
            ObligationKind::RuntimeCapability,
        ];
        let tname = &library.template(p.template_id).unwrap().name;
        for k in needed {
            let prefix = format!("{}:{}:{}", p.operation_name, tname, k.label());
            let j = certificate
                .judgments
                .iter()
                .find(|j| j.obligation_id == prefix)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidEvidence,
                        format!("missing obligation {prefix}"),
                    )
                })?;
            match &j.status {
                ProofStatus::Proven {
                    rule_id,
                    rule_version,
                    ..
                } => {
                    let r = rules.rule(rule_id).ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidEvidence,
                            format!("unknown rule {rule_id}"),
                        )
                    })?;
                    if r.version != *rule_version {
                        return Err(CoreError::new(
                            ErrorCode::InvalidEvidence,
                            format!("rule version mismatch for {rule_id}"),
                        ));
                    }
                }
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::InvalidEvidence,
                        format!("obligation {prefix} is not proven for the selected plan"),
                    ))
                }
            }
        }
        report.obligations_checked += needed.len();
    }
    let cbytes = certificate.encode();
    let back = ConsistencyCertificate::decode(&cbytes, &carolina_core::limits::Limits::v1())?;
    if back != *certificate {
        return Err(CoreError::new(
            ErrorCode::InvalidEvidence,
            "certificate canonical roundtrip mismatch",
        ));
    }
    if certificate.rule_manifest_hash != rules.hash()
        || certificate.protocol_library_hash != library.hash()
    {
        return Err(CoreError::new(
            ErrorCode::InvalidEvidence,
            "certificate manifests do not match the checker's rule/library manifests",
        ));
    }
    // Evidence manifest (SPEC-004 §13, S004-A12): every Disproven judgment carries the
    // counterexample bytes whose digest the manifest lists, and the manifest lists nothing else.
    // Forged bytes, a dropped entry or a dangling entry all fail the check.
    let mut expected: Vec<(String, Hash256)> = Vec::new();
    for j in &certificate.judgments {
        if let ProofStatus::Disproven { counterexample, .. } = &j.status {
            if counterexample.is_empty() {
                return Err(CoreError::new(
                    ErrorCode::InvalidEvidence,
                    format!(
                        "Disproven judgment {} carries no counterexample",
                        j.obligation_id
                    ),
                ));
            }
            expected.push((
                format!("counterexample:{}", j.obligation_id),
                sha256(counterexample),
            ));
        }
    }
    for e in &expected {
        if !certificate.evidence_manifest.contains(e) {
            return Err(CoreError::new(
                ErrorCode::InvalidEvidence,
                format!(
                    "evidence manifest does not match the counterexample bytes of {}",
                    e.0
                ),
            ));
        }
    }
    for e in &certificate.evidence_manifest {
        if !expected.contains(e) {
            return Err(CoreError::new(
                ErrorCode::InvalidEvidence,
                format!("dangling evidence manifest entry {}", e.0),
            ));
        }
    }
    report.evidence_checked = expected.len();
    report.certificate_hash = certificate.certificate_hash();
    Ok(report)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub structurally_valid: usize,
    pub obligations_checked: usize,
    /// Disproven judgments whose counterexample bytes matched the evidence manifest.
    pub evidence_checked: usize,
    pub certificate_hash: CertificateHash,
}
