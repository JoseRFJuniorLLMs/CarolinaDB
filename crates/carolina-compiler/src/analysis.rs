//! Semantic closure, IDC templates, directed interactions and per-candidate obligations (SPEC-004 §4–§10).

use std::collections::{BTreeMap, BTreeSet};

use carolina_core::error::CoreResult;
use carolina_core::ids::*;
use carolina_lang::counterexample::Failure;
use carolina_lang::ir::*;

use crate::explore::explore_pair;
use crate::input::{AuthorityKind, CompileInput};
use crate::library::{
    ProtocolTemplate, T_C0_LOCAL, T_C1_COMMUTATIVE, T_C2_CAUSAL, T_C3_ESCROW, T_C4_CERTIFIED,
    T_C5_SERIAL,
};
use crate::plan::*;
use crate::rules::*;

// ---------------------------------------------------------------------------
// Closure
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct IdcTemplate {
    pub name: String,
    pub records: Vec<RecordId>,
    pub per_key: bool,
    pub invariants: Vec<InvariantId>,
}

#[derive(Debug, Clone)]
pub struct Closure {
    pub templates: Vec<IdcTemplate>,
    pub record_template: BTreeMap<RecordId, usize>,
    /// Complete affected records per operation after the fixed point.
    pub op_records: BTreeMap<OperationRef, BTreeSet<RecordId>>,
    /// Affected invariants per operation.
    pub op_invariants: BTreeMap<OperationRef, BTreeSet<InvariantId>>,
    /// Templates each operation instantiates.
    pub op_templates: BTreeMap<OperationRef, Vec<usize>>,
    /// Interacting closure of operations: operations sharing any template.
    pub op_closure: BTreeMap<OperationRef, BTreeSet<OperationRef>>,
    pub implicit_invariants: Vec<String>,
    pub interactions: Vec<InteractionEdge>,
}

fn invariant_records(inv: &InvariantIR) -> BTreeSet<RecordId> {
    let mut s = BTreeSet::new();
    match &inv.scope {
        ScopeExpr::PerKey { record } | ScopeExpr::PerGroup { record, .. } => {
            s.insert(*record);
        }
        ScopeExpr::Global { records } => s.extend(records.iter().copied()),
    }
    for d in &inv.dependencies {
        s.insert(d.record);
    }
    if let PredicateIR::RequiresFact { fact, .. } = &inv.predicate {
        s.insert(*fact);
    }
    s
}

fn footprint_records(op: &OperationIR) -> BTreeSet<RecordId> {
    let fp = &op.footprint;
    fp.reads
        .iter()
        .chain(&fp.writes)
        .chain(&fp.predicates)
        .map(|d| d.record)
        .chain(fp.facts.iter().map(|f| f.record))
        .collect()
}

fn written_records(op: &OperationIR) -> BTreeSet<RecordId> {
    op.footprint.writes.iter().map(|d| d.record).collect()
}

/// Records whose pre-state is read by REQUIRE or by effect guards (guard reads).
fn guard_records(op: &OperationIR) -> BTreeSet<RecordId> {
    let mut out = BTreeSet::new();
    fn walk(e: &ExprIR, out: &mut BTreeSet<RecordId>) {
        match &e.node {
            ExprNodeIR::RowLookup { record, key, .. } | ExprNodeIR::Exists { record, key, .. } => {
                out.insert(*record);
                walk(key, out);
            }
            ExprNodeIR::Field { base, .. } | ExprNodeIR::StructField { base, .. } => {
                walk(base, out)
            }
            ExprNodeIR::Tuple(items) | ExprNodeIR::SetLit(items) => {
                items.iter().for_each(|i| walk(i, out))
            }
            ExprNodeIR::Struct(fields) => fields.iter().for_each(|(_, i)| walk(i, out)),
            ExprNodeIR::SomeOf(i)
            | ExprNodeIR::Neg(i)
            | ExprNodeIR::Not(i)
            | ExprNodeIR::IsNone(i)
            | ExprNodeIR::IsSome(i)
            | ExprNodeIR::Size(i) => walk(i, out),
            ExprNodeIR::Bin { lhs, rhs, .. } => {
                walk(lhs, out);
                walk(rhs, out);
            }
            ExprNodeIR::UnwrapOr { value, default } => {
                walk(value, out);
                walk(default, out);
            }
            ExprNodeIR::SumOver { set, value, .. } => {
                walk(set, out);
                walk(value, out);
            }
            ExprNodeIR::Lit(_)
            | ExprNodeIR::Param(_)
            | ExprNodeIR::Binding(_)
            | ExprNodeIR::GroupKey => {}
        }
    }
    walk(&op.pre, &mut out);
    for r in &op.reads {
        match &r.kind {
            ReadKind::Row { record, .. }
            | ReadKind::OptionalRow { record, .. }
            | ReadKind::Exists { record, .. }
            | ReadKind::Scan { record, .. } => {
                out.insert(*record);
            }
        }
    }
    for e in &op.effects {
        if let Some(g) = &e.guard {
            walk(g, &mut out);
        }
    }
    out
}

/// Whether every access of `op` to `record` uses a static point key (a sound per-key selector).
fn static_point_access(op: &OperationIR, record: RecordId) -> bool {
    let fp = &op.footprint;
    fp.reads
        .iter()
        .chain(&fp.writes)
        .chain(&fp.predicates)
        .filter(|d| d.record == record)
        .all(|d| {
            matches!(
                d.key_set,
                KeySet::Point {
                    static_key: true,
                    ..
                }
            )
        })
}

struct Uf(Vec<usize>);
impl Uf {
    fn find(&mut self, i: usize) -> usize {
        if self.0[i] != i {
            let r = self.find(self.0[i]);
            self.0[i] = r;
        }
        self.0[i]
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

pub fn build_closure(module: &ModuleIR) -> Closure {
    let records: Vec<RecordId> = module.records.iter().map(|r| r.id).collect();
    let idx: BTreeMap<RecordId, usize> = records.iter().enumerate().map(|(i, r)| (*r, i)).collect();
    let mut uf = Uf((0..records.len()).collect());
    let mut global_records: BTreeSet<RecordId> = BTreeSet::new();
    let mut record_invariants: BTreeMap<RecordId, BTreeSet<InvariantId>> = BTreeMap::new();

    for inv in &module.invariants {
        let recs = invariant_records(inv);
        for r in &recs {
            record_invariants.entry(*r).or_default().insert(inv.id);
        }
        // multi-record invariants merge their records into one template
        let v: Vec<RecordId> = recs.iter().copied().collect();
        for w in v.windows(2) {
            uf.union(idx[&w[0]], idx[&w[1]]);
        }
        match &inv.scope {
            ScopeExpr::Global { .. } | ScopeExpr::PerGroup { .. } => {
                global_records.extend(recs.iter().copied())
            }
            ScopeExpr::PerKey { .. } => {
                // a per-key invariant that depends on another record makes the pair global
                if recs.len() > 1 {
                    global_records.extend(recs.iter().copied());
                }
            }
        }
    }
    // implicit invariants: primary-key identity and finite representation of every numeric field
    let mut implicit = Vec::new();
    for r in &module.records {
        implicit.push(format!(
            "identity: {} primary key is unique (record {})",
            r.name, r.id
        ));
        for f in &r.fields {
            if f.ty.is_numeric() {
                implicit.push(format!(
                    "representation: {}.{} stays within {}",
                    r.name,
                    f.name,
                    f.ty.describe()
                ));
            }
        }
    }

    // templates = union-find components
    let mut comp_members: BTreeMap<usize, Vec<RecordId>> = BTreeMap::new();
    for (i, r) in records.iter().enumerate() {
        comp_members.entry(uf.find(i)).or_default().push(*r);
    }
    let mut templates = Vec::new();
    let mut record_template = BTreeMap::new();
    for (_, members) in comp_members {
        // per-key only if no member is global and every op accesses every member by static point keys
        let mut per_key = members.iter().all(|r| !global_records.contains(r));
        if per_key {
            for op in &module.operations {
                for r in &members {
                    if footprint_records(op).contains(r) && !static_point_access(op, *r) {
                        per_key = false;
                    }
                }
            }
        }
        let names: Vec<String> = members
            .iter()
            .map(|r| {
                module
                    .record(*r)
                    .map(|x| x.name.clone())
                    .unwrap_or_default()
            })
            .collect();
        let name = format!(
            "idc:{}{}",
            names.join("+"),
            if per_key { "[key]" } else { "[global]" }
        );
        let mut invs: BTreeSet<InvariantId> = BTreeSet::new();
        for r in &members {
            if let Some(s) = record_invariants.get(r) {
                invs.extend(s.iter().copied());
            }
        }
        let ti = templates.len();
        for r in &members {
            record_template.insert(*r, ti);
        }
        templates.push(IdcTemplate {
            name,
            records: members,
            per_key,
            invariants: invs.into_iter().collect(),
        });
    }

    // per-operation closure: fixed point over records ↔ invariants
    let mut op_records = BTreeMap::new();
    let mut op_invariants = BTreeMap::new();
    let mut op_templates = BTreeMap::new();
    for op in &module.operations {
        let mut recs = footprint_records(op);
        let mut invs: BTreeSet<InvariantId> = BTreeSet::new();
        loop {
            let before = (recs.len(), invs.len());
            for inv in &module.invariants {
                let ir = invariant_records(inv);
                let touches = ir.iter().any(|r| recs.contains(r));
                let by_requires = matches!(&inv.predicate, PredicateIR::RequiresFact { operation, .. } if *operation == op.identity.operation_id);
                if touches || by_requires {
                    invs.insert(inv.id);
                    recs.extend(ir.iter().copied());
                }
            }
            if (recs.len(), invs.len()) == before {
                break;
            }
        }
        let mut ts: Vec<usize> = recs
            .iter()
            .filter_map(|r| record_template.get(r).copied())
            .collect();
        ts.sort();
        ts.dedup();
        op_templates.insert(op.identity, ts);
        op_records.insert(op.identity, recs);
        op_invariants.insert(op.identity, invs);
    }
    // interacting operations: share a template
    let mut op_closure: BTreeMap<OperationRef, BTreeSet<OperationRef>> = BTreeMap::new();
    for a in &module.operations {
        let mut set = BTreeSet::new();
        for b in &module.operations {
            if op_templates[&a.identity]
                .iter()
                .any(|t| op_templates[&b.identity].contains(t))
            {
                set.insert(b.identity);
            }
        }
        op_closure.insert(a.identity, set);
    }

    // directed interactions
    let mut interactions = Vec::new();
    for inv in &module.invariants {
        if let PredicateIR::RequiresFact {
            operation, fact, ..
        } = &inv.predicate
        {
            for a in &module.operations {
                let emits = a
                    .effects
                    .iter()
                    .any(|e| matches!(&e.kind, EffectKind::EmitFact { fact: f, .. } if f == fact));
                if emits {
                    if let Some(b) = module
                        .operations
                        .iter()
                        .find(|o| o.identity.operation_id == *operation)
                    {
                        interactions.push(InteractionEdge {
                            predecessor: a.identity,
                            successor: b.identity,
                            kind: InteractionKind::RequiresVisible,
                            key_relation: format!(
                                "fact {} keyed by the successor's arguments",
                                module
                                    .record(*fact)
                                    .map(|r| r.name.clone())
                                    .unwrap_or_default()
                            ),
                            invariant_refs: vec![inv.id],
                        });
                    }
                }
            }
        }
    }
    for a in &module.operations {
        let writes = written_records(a);
        for b in &module.operations {
            let guards = guard_records(b);
            let overlap: Vec<RecordId> = writes.intersection(&guards).copied().collect();
            if !overlap.is_empty() {
                interactions.push(InteractionEdge {
                    predecessor: a.identity,
                    successor: b.identity,
                    kind: InteractionKind::InvalidatesGuard,
                    key_relation: format!(
                        "writes records read by the successor's guards: {}",
                        overlap
                            .iter()
                            .map(|r| r.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    invariant_refs: vec![],
                });
            }
        }
        if op_templates[&a.identity].len() > 1 {
            interactions.push(InteractionEdge {
                predecessor: a.identity,
                successor: a.identity,
                kind: InteractionKind::AtomicWith,
                key_relation: format!(
                    "whole-invocation atomicity across {} IDC templates",
                    op_templates[&a.identity].len()
                ),
                invariant_refs: vec![],
            });
        }
    }

    Closure {
        templates,
        record_template,
        op_records,
        op_invariants,
        op_templates,
        op_closure,
        implicit_invariants: implicit,
        interactions,
    }
}

// ---------------------------------------------------------------------------
// Candidate obligations
// ---------------------------------------------------------------------------

pub struct CandidateAnalysis {
    pub operation: OperationRef,
    pub template: ProtocolTemplate,
    pub judgments: Vec<AnalysisJudgment>,
    pub authority: Option<AuthorityId>,
}

impl CandidateAnalysis {
    pub fn accepted(&self) -> bool {
        self.judgments.iter().all(|j| j.status.is_proven())
    }
    pub fn failures(&self) -> Vec<(String, String)> {
        self.judgments
            .iter()
            .filter(|j| !j.status.is_proven())
            .map(|j| {
                let why = match &j.status {
                    ProofStatus::Disproven { summary, .. } => format!("Disproven: {summary}"),
                    ProofStatus::Unknown(r) => format!("Unknown: {r:?}"),
                    ProofStatus::Proven { .. } => unreachable!(),
                };
                (j.obligation_id.clone(), why)
            })
            .collect()
    }
}

fn proven(
    id: &str,
    kind: ObligationKind,
    subjects: Vec<String>,
    rule: &str,
    premises: Vec<String>,
    assumptions: Vec<String>,
) -> AnalysisJudgment {
    AnalysisJudgment {
        obligation_id: id.into(),
        kind,
        subjects,
        status: ProofStatus::Proven {
            rule_id: rule.into(),
            rule_version: 1,
            premises,
        },
        assumptions,
    }
}
fn unknown(
    id: &str,
    kind: ObligationKind,
    subjects: Vec<String>,
    reason: UnknownReason,
) -> AnalysisJudgment {
    AnalysisJudgment {
        obligation_id: id.into(),
        kind,
        subjects,
        status: ProofStatus::Unknown(reason),
        assumptions: vec![],
    }
}

/// Cache of pair explorations shared across candidates (keyed by ordered pair).
/// Outcome of one explored pair: `Err(())` = budget exceeded; `Ok(None)` = no counterexample; `Ok(Some((bytes, summary)))`.
pub type PairResult = Result<Option<(Vec<u8>, String)>, ()>;

pub struct PairCache {
    pub results: BTreeMap<(OperationRef, OperationRef), PairResult>,
    pub steps: u64,
}

impl PairCache {
    pub fn new() -> Self {
        PairCache {
            results: BTreeMap::new(),
            steps: 0,
        }
    }
}

impl Default for PairCache {
    fn default() -> Self {
        Self::new()
    }
}

fn family_serial(t: &ProtocolTemplate) -> bool {
    t.id == T_C5_SERIAL || t.id == T_C0_LOCAL
}

fn authority_for(
    input: &CompileInput,
    template: &ProtocolTemplate,
    records: &BTreeSet<RecordId>,
) -> Option<AuthorityId> {
    input
        .topology
        .authority_domains
        .iter()
        .find(|d| {
            let kind_ok = matches!(
                (&d.kind, template.id),
                (AuthorityKind::OrderedSerial { .. }, T_C5_SERIAL)
                    | (AuthorityKind::ExclusiveLocal, T_C0_LOCAL)
                    | (AuthorityKind::EscrowHolder, T_C3_ESCROW)
                    | (AuthorityKind::Certifier { .. }, T_C4_CERTIFIED)
            );
            let covers = d.covered_records.is_empty()
                || records.iter().all(|r| d.covered_records.contains(r));
            kind_ok && covers
        })
        .map(|d| d.id)
}

/// Judge every obligation of one (operation, template) candidate.
pub fn analyze_candidate(
    input: &CompileInput,
    closure: &Closure,
    op: &OperationIR,
    template: &ProtocolTemplate,
    cache: &mut PairCache,
) -> CoreResult<CandidateAnalysis> {
    let module = &input.module_ir;
    let opid = op.identity;
    let subj = vec![format!("{}@{}", op.name, op.identity.version)];
    let records = &closure.op_records[&opid];
    let invs = &closure.op_invariants[&opid];
    let inv_names: Vec<String> = invs
        .iter()
        .filter_map(|i| module.invariant(*i).ok())
        .map(|i| i.name.clone())
        .collect();
    let mut j = Vec::new();
    let oid = |k: &str| format!("{}:{}:{}", op.name, template.name, k);

    // IR-DEF
    j.push(proven(
        &oid("IR-DEF"),
        ObligationKind::IrDef,
        subj.clone(),
        R_IR_DEF_EVAL,
        vec!["reference interpreter evaluates every IR node with checked arithmetic".into()],
        vec![],
    ));

    // IR-FOOT
    let widened: Vec<String> = op
        .footprint
        .reads
        .iter()
        .chain(&op.footprint.writes)
        .chain(&op.footprint.predicates)
        .filter(|d| {
            !matches!(
                d.key_set,
                KeySet::Point {
                    static_key: true,
                    ..
                }
            )
        })
        .map(|d| format!("record {} widened to full scope", d.record))
        .collect();
    j.push(proven(
        &oid("IR-FOOT"),
        ObligationKind::IrFoot,
        subj.clone(),
        R_FOOT_CONSERVATIVE,
        widened,
        vec![],
    ));

    // DURABILITY
    match &op.contract.durability {
        Durability::LocalStable => j.push(proven(
            &oid("DURABILITY"),
            ObligationKind::Durability,
            subj.clone(),
            R_DURABILITY_POLICY,
            vec!["LocalStable: local durable journal barrier".into()],
            vec![],
        )),
        Durability::ReplicatedStable(p) => match input.topology.policy(p) {
            Some(pol)
                if pol.required_durable_copies > 1
                    && pol.failure_domains >= pol.required_durable_copies =>
            {
                j.push(proven(
                    &oid("DURABILITY"),
                    ObligationKind::Durability,
                    subj.clone(),
                    R_DURABILITY_POLICY,
                    vec![format!(
                        "policy {} requires {} copies over {} failure domains",
                        p, pol.required_durable_copies, pol.failure_domains
                    )],
                    vec![],
                ))
            }
            _ => j.push(unknown(
                &oid("DURABILITY"),
                ObligationKind::Durability,
                subj.clone(),
                UnknownReason::MissingAuthorityModel(format!(
                    "no failure-domain policy `{p}` with replicated witnesses in the topology"
                )),
            )),
        },
    }

    // AUTHORITY
    let authority = authority_for(input, template, records);
    let needs_authority = matches!(
        template.id,
        T_C5_SERIAL | T_C0_LOCAL | T_C3_ESCROW | T_C4_CERTIFIED
    );
    if needs_authority {
        match authority {
            Some(a) => j.push(proven(
                &oid("AUTHORITY"),
                ObligationKind::Authority,
                subj.clone(),
                if template.id == T_C0_LOCAL {
                    R_C0_EXCLUSIVE_LOCAL
                } else {
                    R_C5_SERIAL_VALIDATE
                },
                vec![format!(
                    "authority domain {a} covers the complete affected scope"
                )],
                vec![if template.id == T_C0_LOCAL {
                    "exclusive writer epoch enforced at admission".into()
                } else {
                    "ordered authority active and fenced".into()
                }],
            )),
            None => j.push(unknown(
                &oid("AUTHORITY"),
                ObligationKind::Authority,
                subj.clone(),
                UnknownReason::MissingAuthorityModel(format!(
                    "no {} authority domain covers the affected records",
                    template.name
                )),
            )),
        }
    } else {
        j.push(proven(
            &oid("AUTHORITY"),
            ObligationKind::Authority,
            subj.clone(),
            R_FOOT_CONSERVATIVE,
            vec!["no exclusive authority required by this family".into()],
            vec![],
        ));
    }

    // RUNTIME-CAPABILITY
    if template.qualified {
        j.push(proven(
            &oid("RUNTIME-CAPABILITY"),
            ObligationKind::RuntimeCapability,
            subj.clone(),
            R_IR_DEF_EVAL,
            vec![format!(
                "template {} qualified at {}",
                template.name, template.milestone
            )],
            vec![],
        ));
    } else {
        j.push(unknown(
            &oid("RUNTIME-CAPABILITY"),
            ObligationKind::RuntimeCapability,
            subj.clone(),
            UnknownReason::MissingRuntimeCapability(format!(
                "template {} is not qualified before {}",
                template.name, template.milestone
            )),
        ));
    }

    // IR-OBS
    let serial = family_serial(template);
    let obs_ok = match (op.contract.result_semantics, op.contract.input_visibility) {
        (ResultSemantics::ExactOrderedValue, _) => serial,
        (_, InputVisibility::SerialScope) => serial,
        (_, InputVisibility::CertifiedScope) => serial || template.id == T_C4_CERTIFIED,
        (ResultSemantics::SnapshotValue, _) => true,
        (ResultSemantics::Receipt, _) => true,
    };
    if obs_ok {
        j.push(proven(
            &oid("IR-OBS"),
            ObligationKind::IrObs,
            subj.clone(),
            R_OBS_RESULT_SCOPE,
            vec![format!(
                "result {:?} with visibility {:?} is satisfiable by {}",
                op.contract.result_semantics, op.contract.input_visibility, template.name
            )],
            vec![],
        ));
    } else {
        j.push(unknown(
            &oid("IR-OBS"),
            ObligationKind::IrObs,
            subj.clone(),
            UnknownReason::UnsupportedFragment(format!(
                "{:?}/{:?} requires serial scope; {} cannot provide it",
                op.contract.result_semantics, op.contract.input_visibility, template.name
            )),
        ));
    }

    // SESSION
    if op.contract.session.is_empty() {
        j.push(proven(
            &oid("SESSION"),
            ObligationKind::Session,
            subj.clone(),
            R_OBS_RESULT_SCOPE,
            vec!["no session guarantees requested".into()],
            vec![],
        ));
    } else {
        match &op.contract.session_scope {
            SessionScope::ReplicationGroup(g) if serial => j.push(proven(
                &oid("SESSION"),
                ObligationKind::Session,
                subj.clone(),
                R_SESSION_SERIAL_SUBSUMES,
                vec![format!(
                    "group {g} records executed by one serial authority"
                )],
                vec!["session reads pass the authority read barrier".into()],
            )),
            SessionScope::ReplicationGroup(g) if template.id == T_C2_CAUSAL => j.push(proven(
                &oid("SESSION"),
                ObligationKind::Session,
                subj.clone(),
                R_OBS_RESULT_SCOPE,
                vec![format!("one replication group {g}")],
                vec![],
            )),
            SessionScope::ReplicationGroup(_) => j.push(unknown(
                &oid("SESSION"),
                ObligationKind::Session,
                subj.clone(),
                UnknownReason::UnsupportedFragment(format!(
                    "{} does not carry group session frontiers",
                    template.name
                )),
            )),
            SessionScope::CompositeScope(_) => j.push(unknown(
                &oid("SESSION"),
                ObligationKind::Session,
                subj.clone(),
                UnknownReason::UnsupportedComposition(
                    "cross-group session scope requires a separately qualified composite plan"
                        .into(),
                ),
            )),
            SessionScope::None => j.push(unknown(
                &oid("SESSION"),
                ObligationKind::Session,
                subj.clone(),
                UnknownReason::UnsupportedFragment("nonempty session without scope".into()),
            )),
        }
    }

    // IR-SEQ / IR-CONC
    let rule = if template.id == T_C0_LOCAL {
        R_C0_EXCLUSIVE_LOCAL
    } else {
        R_C5_SERIAL_VALIDATE
    };
    if serial {
        j.push(proven(&oid("IR-SEQ"), ObligationKind::IrSeq, subj.clone(), rule, vec![format!("runtime evaluates invariants [{}] and implicit representation limits at the serial position", inv_names.join(", "))], vec![]));
        j.push(proven(
            &oid("IR-CONC"),
            ObligationKind::IrConc,
            subj.clone(),
            rule,
            vec!["admitted histories are serialized by the authority".into()],
            vec![],
        ));
    } else {
        // static grow-only facts rule
        let grow_only = !op.effects.is_empty()
            && op.effects.iter().all(|e| match &e.kind {
                EffectKind::EmitFact { key, fields, .. } => {
                    // the payload must be a function of the key: concurrent emits with equal keys are then identical
                    let key_params = params_of(key);
                    fields
                        .iter()
                        .all(|(_, f)| params_of(f).is_subset(&key_params))
                }
                _ => false,
            });
        let no_hard_invariants = invs
            .iter()
            .filter_map(|i| module.invariant(*i).ok())
            .all(|i| {
                matches!(
                    i.kind,
                    InvariantKind::CausalPrerequisite | InvariantKind::Monotonic
                )
            });
        let closure_ops: Vec<OperationRef> = closure.op_closure[&opid].iter().copied().collect();
        let mut disproven: Option<(Vec<u8>, String)> = None;
        let mut budget_hit = false;
        let rec_list: Vec<RecordId> = records.iter().copied().collect();
        for other in &closure_ops {
            for (a, b) in [(opid, *other), (*other, opid)] {
                let key = (a, b);
                if !cache.results.contains_key(&key) {
                    let r = explore_pair(
                        module,
                        a,
                        b,
                        &rec_list,
                        &input.analysis_budget,
                        &mut cache.steps,
                    )?;
                    let entry = if r.budget_exceeded {
                        Err(())
                    } else {
                        Ok(r.counterexample.map(|cx| {
                            let summary = match &cx.failure {
                                Failure::InvariantViolated { invariant_name, detail } => format!("concurrent acceptance of {} and {} violates `{}`: {}", name_of(module, a), name_of(module, b), invariant_name, detail),
                                Failure::EffectInapplicable { detail } => format!("accepted effects of {} and {} cannot both apply: {}", name_of(module, a), name_of(module, b), detail),
                                Failure::NoSequentialJustification => format!("results of concurrently accepted {} and {} are not justified by any sequential order", name_of(module, a), name_of(module, b)),
                            };
                            (cx.to_canon().encode(), summary)
                        }))
                    };
                    cache.results.insert(key, entry);
                }
                match &cache.results[&key] {
                    Err(()) => budget_hit = true,
                    Ok(Some(cx)) => {
                        if disproven.is_none() {
                            disproven = Some(cx.clone());
                        }
                    }
                    Ok(None) => {}
                }
            }
        }
        if let Some((bytes, summary)) = disproven {
            j.push(AnalysisJudgment {
                obligation_id: oid("IR-CONC"),
                kind: ObligationKind::IrConc,
                subjects: subj.clone(),
                status: ProofStatus::Disproven {
                    counterexample: bytes,
                    summary,
                },
                assumptions: vec![],
            });
            j.push(unknown(
                &oid("IR-SEQ"),
                ObligationKind::IrSeq,
                subj.clone(),
                UnknownReason::UnsupportedFragment(format!(
                    "no static preservation rule for {} in v1",
                    template.name
                )),
            ));
        } else if budget_hit {
            j.push(unknown(
                &oid("IR-CONC"),
                ObligationKind::IrConc,
                subj.clone(),
                UnknownReason::BudgetExceeded,
            ));
            j.push(unknown(
                &oid("IR-SEQ"),
                ObligationKind::IrSeq,
                subj.clone(),
                UnknownReason::BudgetExceeded,
            ));
        } else if grow_only
            && no_hard_invariants
            && (template.id == T_C1_COMMUTATIVE || template.id == T_C2_CAUSAL)
        {
            j.push(proven(
                &oid("IR-SEQ"),
                ObligationKind::IrSeq,
                subj.clone(),
                R_C1_GROWONLY_FACTS,
                vec!["effects only emit immutable facts keyed by arguments".into()],
                vec!["stable origin identity and deduplication".into()],
            ));
            j.push(proven(&oid("IR-CONC"), ObligationKind::IrConc, subj.clone(), R_C1_GROWONLY_FACTS, vec![format!("bounded exploration of {} pairs found no counterexample; grow-only fact algebra converges", closure_ops.len())], vec!["stable origin identity and deduplication".into()]));
        } else {
            j.push(unknown(&oid("IR-CONC"), ObligationKind::IrConc, subj.clone(), UnknownReason::SolverUnknown(format!("bounded exploration ({} steps so far) found no counterexample; no proof rule covers {} for this operation set (finite representation of numeric fields is an undeclared bound)", cache.steps, template.name))));
            j.push(unknown(
                &oid("IR-SEQ"),
                ObligationKind::IrSeq,
                subj.clone(),
                UnknownReason::UnsupportedFragment(format!(
                    "no static preservation rule for {} in v1",
                    template.name
                )),
            ));
        }
    }

    // IR-ATOM
    let n_templates = closure.op_templates[&opid].len();
    if n_templates <= 1 || (serial && authority.is_some()) {
        j.push(proven(
            &oid("IR-ATOM"),
            ObligationKind::IrAtom,
            subj.clone(),
            R_ATOM_SINGLE_AUTHORITY,
            vec![format!(
                "{n_templates} IDC template(s) executed by one authority in one atomic batch"
            )],
            vec![],
        ));
    } else {
        j.push(unknown(&oid("IR-ATOM"), ObligationKind::IrAtom, subj.clone(), UnknownReason::UnsupportedComposition(format!("{n_templates} IDC templates need SPEC-008 composite publication, unavailable for {}", template.name))));
    }

    // IR-COMP: interacting operations must share a compatible authority; decided jointly in selection,
    // here we record the per-candidate requirement.
    let peers: Vec<String> = closure.op_closure[&opid]
        .iter()
        .filter(|o| **o != opid)
        .map(|o| name_of(module, *o))
        .collect();
    if serial {
        j.push(proven(&oid("IR-COMP"), ObligationKind::IrComp, subj.clone(), R_COMP_SAME_AUTHORITY, vec![format!("interacting operations [{}] serialize through the same authority when they select the same template", peers.join(", "))], vec![]));
    } else if peers.is_empty() {
        j.push(proven(
            &oid("IR-COMP"),
            ObligationKind::IrComp,
            subj.clone(),
            R_COMP_SAME_AUTHORITY,
            vec!["no interacting operations".into()],
            vec![],
        ));
    } else {
        j.push(unknown(
            &oid("IR-COMP"),
            ObligationKind::IrComp,
            subj.clone(),
            UnknownReason::UnsupportedComposition(format!(
                "no qualified compatibility rule between {} and interacting [{}]",
                template.name,
                peers.join(", ")
            )),
        ));
    }

    Ok(CandidateAnalysis {
        operation: opid,
        template: template.clone(),
        judgments: j,
        authority,
    })
}

/// Parameter indices an expression depends on.
fn params_of(e: &ExprIR) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    fn walk(e: &ExprIR, out: &mut BTreeSet<u32>) {
        match &e.node {
            ExprNodeIR::Param(i) => {
                out.insert(*i);
            }
            ExprNodeIR::RowLookup { key, .. } | ExprNodeIR::Exists { key, .. } => walk(key, out),
            ExprNodeIR::Field { base, .. } | ExprNodeIR::StructField { base, .. } => {
                walk(base, out)
            }
            ExprNodeIR::Tuple(items) | ExprNodeIR::SetLit(items) => {
                items.iter().for_each(|i| walk(i, out))
            }
            ExprNodeIR::Struct(fields) => fields.iter().for_each(|(_, i)| walk(i, out)),
            ExprNodeIR::SomeOf(i)
            | ExprNodeIR::Neg(i)
            | ExprNodeIR::Not(i)
            | ExprNodeIR::IsNone(i)
            | ExprNodeIR::IsSome(i)
            | ExprNodeIR::Size(i) => walk(i, out),
            ExprNodeIR::Bin { lhs, rhs, .. } => {
                walk(lhs, out);
                walk(rhs, out);
            }
            ExprNodeIR::UnwrapOr { value, default } => {
                walk(value, out);
                walk(default, out);
            }
            ExprNodeIR::SumOver { set, value, .. } => {
                walk(set, out);
                walk(value, out);
            }
            ExprNodeIR::Binding(_) => {
                // a read binding may depend on state: treat as not key-determined
                out.insert(u32::MAX);
            }
            ExprNodeIR::Lit(_) | ExprNodeIR::GroupKey => {}
        }
    }
    walk(e, &mut out);
    out
}

pub fn name_of(module: &ModuleIR, op: OperationRef) -> String {
    module
        .operation(op)
        .map(|o| format!("{}@{}", o.name, o.identity.version))
        .unwrap_or_else(|_| format!("{op:?}"))
}
