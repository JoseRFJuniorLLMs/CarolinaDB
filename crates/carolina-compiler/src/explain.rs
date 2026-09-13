//! `EXPLAIN` rendering (SPEC-004 §13, SPEC-001 §28, §62).
//!
//! Shows the selected family and every added requirement; affected invariants and IDCs;
//! visibility and result semantics; every accepted/rejected/unknown candidate; authority and
//! durability assumptions; partition behavior; fallback prerequisites and unqualified runtime
//! features. A candidate that lacks rule or runtime evidence is reported as "candidate", never
//! as "safe active plan".

use std::fmt::Write as _;

use carolina_lang::ir::{ModuleIR, PartitionOutcome};

use crate::plan::ProofStatus;
use crate::select::CompileOutput;

pub fn explain_operation(module: &ModuleIR, out: &CompileOutput, name: &str) -> Option<String> {
    let op = module.operation_by_name(name)?;
    let plan = out.plans.iter().find(|p| p.operation == op.identity)?;
    let mut s = String::new();
    let _ = writeln!(s, "Operation: {}@{}", op.name, op.identity.version);
    let _ = writeln!(
        s,
        "Selected: {} ({}) — candidate plan {}; activation requires catalog/qualification evidence",
        plan.profile.family.label(),
        out.candidates
            .iter()
            .find(|c| c.operation == op.identity && c.template.id == plan.template_id)
            .map(|c| c.template.name.clone())
            .unwrap_or_default(),
        plan.plan_hash()
    );
    let invs: Vec<String> = out.closure.op_invariants[&op.identity]
        .iter()
        .filter_map(|i| module.invariant(*i).ok())
        .map(|i| format!("{} [{}]", i.name, i.kind.label()))
        .collect();
    let _ = writeln!(
        s,
        "Invariant scope: {}{}",
        if invs.is_empty() {
            "(none declared)".to_string()
        } else {
            invs.join(", ")
        },
        if out.closure.implicit_invariants.is_empty() {
            ""
        } else {
            " plus implicit identity/representation limits"
        }
    );
    let idcs: Vec<String> = plan
        .idc_templates
        .iter()
        .map(|t| t.template_name.clone())
        .collect();
    let _ = writeln!(s, "IDC templates: {}", idcs.join(", "));
    let _ = writeln!(
        s,
        "Input visibility: {:?}; result: {:?} ({:?}); session: {:?} scope {:?}",
        op.contract.input_visibility,
        op.contract.result_semantics,
        op.contract.result_scope,
        op.contract.session,
        op.contract.session_scope
    );
    let _ = writeln!(
        s,
        "Durability: {:?}; refusal: {:?}",
        op.contract.durability, op.contract.refusal_semantics
    );
    let partition: Vec<String> = op
        .contract
        .partition_outcomes
        .iter()
        .map(|p| match p {
            PartitionOutcome::Wait => "wait for authority".to_string(),
            PartitionOutcome::Unavailable => "typed Unavailable (no new effects)".to_string(),
            PartitionOutcome::AuthorityUnavailable => {
                "AuthorityUnavailable (not a business rejection)".to_string()
            }
        })
        .collect();
    let _ = writeln!(s, "Partition behavior: {}", partition.join("; "));
    let _ = writeln!(s, "Candidates:");
    for c in out.candidates.iter().filter(|c| c.operation == op.identity) {
        let status = if c.accepted() {
            if c.template.id == plan.template_id {
                "SELECTED"
            } else {
                "eligible (higher cost or not chosen jointly)"
            }
        } else {
            "rejected"
        };
        let _ = writeln!(s, "  {:<18} {}", c.template.name, status);
        for j in &c.judgments {
            match &j.status {
                ProofStatus::Proven {
                    rule_id, premises, ..
                } => {
                    let _ = writeln!(
                        s,
                        "      {:<20} proven by {}{}",
                        j.kind.label(),
                        rule_id,
                        if premises.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", premises.join("; "))
                        }
                    );
                }
                ProofStatus::Disproven { summary, .. } => {
                    let _ = writeln!(s, "      {:<20} DISPROVEN — {}", j.kind.label(), summary);
                }
                ProofStatus::Unknown(r) => {
                    let _ = writeln!(s, "      {:<20} unknown — {:?}", j.kind.label(), r);
                }
            }
        }
    }
    let _ = writeln!(s, "Assumptions:");
    for a in &plan.assumptions {
        let _ = writeln!(
            s,
            "  {} — {:?} enforced by {:?}, on failure {:?}",
            a.id, a.predicate, a.enforcement, a.failure_action
        );
    }
    if !plan.migration_requirements.is_empty() {
        let _ = writeln!(s, "Alternative plans require:");
        for m in &plan.migration_requirements {
            let _ = writeln!(s, "  {} — {}", m.target, m.requirement);
        }
    }
    let edges: Vec<String> = out
        .interaction_graph
        .iter()
        .filter(|e| e.predecessor == op.identity || e.successor == op.identity)
        .map(|e| {
            format!(
                "{} -> {} [{}] {}",
                crate::analysis::name_of(module, e.predecessor),
                crate::analysis::name_of(module, e.successor),
                e.kind.label(),
                e.key_relation
            )
        })
        .collect();
    if !edges.is_empty() {
        let _ = writeln!(s, "Interactions:");
        for e in edges {
            let _ = writeln!(s, "  {e}");
        }
    }
    let _ = writeln!(
        s,
        "Cost (estimate, not latency): remote_participants={} rounds={} background={}",
        plan.cost.remote_participants, plan.cost.coordination_rounds, plan.cost.background_transfer
    );
    Some(s)
}

pub fn explain_all(module: &ModuleIR, out: &CompileOutput) -> String {
    let mut s = String::new();
    for op in &module.operations {
        if let Some(e) = explain_operation(module, out, &op.name) {
            s.push_str(&e);
            s.push('\n');
        }
    }
    if !out.diagnostics.is_empty() {
        s.push_str("Diagnostics:\n");
        for d in &out.diagnostics {
            let _ = writeln!(s, "  {d}");
        }
    }
    s
}
