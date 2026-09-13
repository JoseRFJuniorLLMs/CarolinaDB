//! CarolinaDB coordination compiler (SPEC-004).
//!
//! `compile(input)` evaluates a closed module against the finite protocol-template library:
//! it builds the conservative semantic closure, derives IDC templates and directed interactions,
//! judges every candidate's obligations (with bounded counterexample search), selects a
//! deterministic safe plan per interacting closure, and emits canonical plans, a certificate,
//! diagnostics and migration prerequisites. Compilation is side-effect free and never activates.

pub mod analysis;
pub mod explain;
pub mod explore;
pub mod input;
pub mod library;
pub mod plan;
pub mod rules;
pub mod select;

pub use input::{CompileInput, CompilePolicy, DeterministicBudget, TopologySnapshot};
pub use plan::{ConsistencyCertificate, OperationPlan};
pub use select::{check_artifacts, compile, CheckReport, CompileOutput};

#[cfg(test)]
mod tests {
    use super::*;
    use carolina_core::canon::Canonical;
    use carolina_core::error::ErrorCode;
    use carolina_core::ids::ConsistencyClass;
    use carolina_lang::fixtures::load_fixture;
    use library::{T_C0_LOCAL, T_C1_COMMUTATIVE, T_C2_CAUSAL};
    use plan::{ObligationKind, ProofStatus};

    fn compile_fixture(name: &str) -> CompileOutput {
        let (ir, _) = load_fixture(name).unwrap();
        compile(&CompileInput::local(ir)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn status_of(
        out: &CompileOutput,
        module: &carolina_lang::ir::ModuleIR,
        op: &str,
        template: carolina_core::ids::TemplateId,
        kind: ObligationKind,
    ) -> ProofStatus {
        let opref = module.operation_by_name(op).unwrap().identity;
        let c = out
            .candidates
            .iter()
            .find(|c| c.operation == opref && c.template.id == template)
            .unwrap();
        c.judgments
            .iter()
            .find(|j| j.kind == kind)
            .unwrap()
            .status
            .clone()
    }

    /// S004-A03: sell/sell stock-1 counterexample rejects C1/C2; C5 selected in the local profile.
    #[test]
    fn inventory_sell_rejects_c1_c2_with_counterexample() {
        let (ir, _) = load_fixture("inventory_sell").unwrap();
        let out = compile(&CompileInput::local(ir.clone())).unwrap();
        for t in [T_C1_COMMUTATIVE, T_C2_CAUSAL] {
            match status_of(&out, &ir, "sell", t, ObligationKind::IrConc) {
                ProofStatus::Disproven {
                    summary,
                    counterexample,
                } => {
                    assert!(summary.contains("stock_nonneg"), "{summary}");
                    assert!(!counterexample.is_empty());
                }
                other => panic!("expected Disproven, got {other:?}"),
            }
        }
        // C5 and C0 are eligible; the local profile selects the cheaper qualified C0 for the whole closure
        let plan = out
            .plans
            .iter()
            .find(|p| p.operation_name == "sell")
            .unwrap();
        assert!(matches!(
            plan.profile.family,
            ConsistencyClass::C0Local | ConsistencyClass::C5Serial
        ));
        assert!(!out.certificate.rejected_candidates.is_empty());
        assert!(out
            .certificate
            .evidence_manifest
            .iter()
            .any(|(l, _)| l.contains("IR-CONC")));
        // artifacts check
        let report = check_artifacts(
            &out.plans,
            &out.certificate,
            &rules::AnalysisRuleManifest::v1(),
            &library::ProtocolLibraryManifest::v1(),
        )
        .unwrap();
        assert_eq!(report.structurally_valid, 2);
    }

    /// S004-A01: identical inputs produce identical plans and certificates.
    #[test]
    fn compilation_is_deterministic() {
        let a = compile_fixture("account_transfer");
        let b = compile_fixture("account_transfer");
        assert_eq!(
            a.certificate.certificate_hash(),
            b.certificate.certificate_hash()
        );
        assert_eq!(
            a.plans.iter().map(|p| p.plan_hash()).collect::<Vec<_>>(),
            b.plans.iter().map(|p| p.plan_hash()).collect::<Vec<_>>()
        );
        // canonical roundtrip of a plan
        let bytes = a.plans[0].encode();
        let back =
            plan::OperationPlan::decode(&bytes, &carolina_core::limits::Limits::v1()).unwrap();
        assert_eq!(back, a.plans[0]);
    }

    /// S004-A09-like (local): account transfer is one closure with Account+Ledger merged by conservation.
    #[test]
    fn account_transfer_closure_and_atomicity() {
        let (ir, _) = load_fixture("account_transfer").unwrap();
        let out = compile(&CompileInput::local(ir.clone())).unwrap();
        assert_eq!(
            out.closure.templates.len(),
            1,
            "Account and Ledger are one global component"
        );
        assert!(!out.closure.templates[0].per_key);
        let c1 = status_of(
            &out,
            &ir,
            "transfer",
            T_C1_COMMUTATIVE,
            ObligationKind::IrConc,
        );
        assert!(matches!(c1, ProofStatus::Disproven { .. }), "{c1:?}");
        let plan = out
            .plans
            .iter()
            .find(|p| p.operation_name == "transfer")
            .unwrap();
        assert!(matches!(
            plan.profile.atomicity,
            plan::AtomicityProgram::LocalBatch
        ));
    }

    /// S004-A05: payment -> ship keeps direction; refund extension adds guard invalidation and exclusion.
    #[test]
    fn causal_dependencies_are_directed() {
        let (ir, _) = load_fixture("causal_ship").unwrap();
        let out = compile(&CompileInput::local(ir.clone())).unwrap();
        let pay = ir.operation_by_name("confirm_payment").unwrap().identity;
        let ship = ir.operation_by_name("ship").unwrap().identity;
        assert!(out.interaction_graph.iter().any(|e| e.predecessor == pay
            && e.successor == ship
            && e.kind == plan::InteractionKind::RequiresVisible));
        assert!(
            !out.interaction_graph.iter().any(|e| e.predecessor == ship
                && e.successor == pay
                && e.kind == plan::InteractionKind::RequiresVisible),
            "reverse edge must not be inferred"
        );
        // confirm_payment keys the fact by `order` only: two concurrent confirmations with different
        // payment ids conflict on the fact identity, so C1/C2 are disproven with a counterexample
        let cap = status_of(
            &out,
            &ir,
            "confirm_payment",
            T_C2_CAUSAL,
            ObligationKind::RuntimeCapability,
        );
        assert!(matches!(
            cap,
            ProofStatus::Unknown(plan::UnknownReason::MissingRuntimeCapability(_))
        ));
        let conc = status_of(
            &out,
            &ir,
            "confirm_payment",
            T_C2_CAUSAL,
            ObligationKind::IrConc,
        );
        assert!(
            matches!(&conc, ProofStatus::Disproven { summary, .. } if summary.contains("cannot both apply")),
            "{conc:?}"
        );
        // a fact whose payload is determined by its key is grow-only: C1/C2 obligations are proven at the
        // rule level and the candidate is still blocked only by runtime qualification
        let src = r#"
RECORD Seen IMMUTABLE { id: Uuid PRIMARY KEY }
OPERATION mark(id: Uuid) VERSION 1 {
  REQUIRE true
  READ {}
  EFFECT { EMIT Seen { id: id } }
  ENSURE true
  RETURN { id: id }
  CONTRACT { atomicity: WholeInvocation, input_visibility: LocalSnapshot, result_semantics: Receipt,
    result_scope: PerKey(Seen[id]), session: {}, session_scope: None, durability: LocalStable,
    partition_outcomes: { Wait }, refusal_semantics: BusinessPredicate, commitment: FinalWhenDurable,
    request_namespace: "facts" }
}
"#;
        let ast =
            carolina_lang::parser::parse_module(src, &carolina_core::limits::Limits::v1()).unwrap();
        let (irf, _) = carolina_lang::lower::lower_module(&ast, None).unwrap();
        let outf = compile(&CompileInput::local(irf.clone())).unwrap();
        let seq = status_of(&outf, &irf, "mark", T_C1_COMMUTATIVE, ObligationKind::IrSeq);
        assert!(seq.is_proven(), "{seq:?}");
        let conc = status_of(
            &outf,
            &irf,
            "mark",
            T_C1_COMMUTATIVE,
            ObligationKind::IrConc,
        );
        assert!(conc.is_proven(), "{conc:?}");
        let cap = status_of(
            &outf,
            &irf,
            "mark",
            T_C1_COMMUTATIVE,
            ObligationKind::RuntimeCapability,
        );
        assert!(!cap.is_proven());
        assert!(outf
            .certificate
            .rejected_candidates
            .iter()
            .any(|r| r.template_name == "C1_COMMUTATIVE_V1"));
        let (ir2, _) = load_fixture("causal_refund_extension").unwrap();
        let out2 = compile(&CompileInput::local(ir2.clone())).unwrap();
        let ship2 = ir2.operation_by_name("ship").unwrap().identity;
        let refund = ir2.operation_by_name("refund").unwrap().identity;
        assert!(out2
            .interaction_graph
            .iter()
            .any(|e| e.predecessor == refund
                && e.successor == ship2
                && e.kind == plan::InteractionKind::InvalidatesGuard));
        let conc = status_of(&out2, &ir2, "ship", T_C2_CAUSAL, ObligationKind::IrConc);
        assert!(matches!(conc, ProofStatus::Disproven { .. }), "{conc:?}");
        // the two certificates differ: retaining the old causal-only certificate is impossible
        assert_ne!(
            out.certificate.certificate_hash(),
            out2.certificate.certificate_hash()
        );
    }

    /// S004-A10: receipt vs exact ordered result yield distinct obligations.
    #[test]
    fn increment_result_obligations_differ() {
        let (ir, _) = load_fixture("increment_result").unwrap();
        let out = compile(&CompileInput::local(ir.clone())).unwrap();
        let obs_bump = status_of(&out, &ir, "bump", T_C1_COMMUTATIVE, ObligationKind::IrObs);
        let obs_get = status_of(
            &out,
            &ir,
            "bump_and_get",
            T_C1_COMMUTATIVE,
            ObligationKind::IrObs,
        );
        assert!(obs_bump.is_proven());
        assert!(!obs_get.is_proven());
        let conc_get = status_of(
            &out,
            &ir,
            "bump_and_get",
            T_C1_COMMUTATIVE,
            ObligationKind::IrConc,
        );
        assert!(
            matches!(conc_get, ProofStatus::Disproven { .. }),
            "{conc_get:?}"
        );
        // bump alone under C1: no counterexample but no proof (finite representation) => Unknown, never safe
        let conc_bump = status_of(&out, &ir, "bump", T_C1_COMMUTATIVE, ObligationKind::IrConc);
        assert!(
            matches!(conc_bump, ProofStatus::Unknown(_)),
            "{conc_bump:?}"
        );
    }

    /// S004-A06/A07-like: unique username needs global authority; C0/C5 accepted, C1 disproven.
    #[test]
    fn unique_username_global_component() {
        let (ir, _) = load_fixture("unique_username").unwrap();
        let out = compile(&CompileInput::local(ir.clone())).unwrap();
        assert!(!out.closure.templates[0].per_key);
        let c1 = status_of(
            &out,
            &ir,
            "register",
            T_C1_COMMUTATIVE,
            ObligationKind::IrConc,
        );
        assert!(matches!(c1, ProofStatus::Disproven { .. }));
    }

    /// S004-A11/A16-like: three-node topology has no exclusive local writer, so C0 cannot be selected;
    /// ReplicatedStable with an unknown policy is unsatisfiable.
    #[test]
    fn topology_governs_authority_and_durability() {
        let (ir, _) = load_fixture("inventory_sell").unwrap();
        let mut input = CompileInput::local(ir.clone());
        input.topology = TopologySnapshot::three_node_serial();
        let out = compile(&input).unwrap();
        for p in &out.plans {
            assert_eq!(p.profile.family, ConsistencyClass::C5Serial);
        }
        let c0 = status_of(&out, &ir, "sell", T_C0_LOCAL, ObligationKind::Authority);
        assert!(matches!(
            c0,
            ProofStatus::Unknown(plan::UnknownReason::MissingAuthorityModel(_))
        ));
        // unsatisfiable durability
        let src = carolina_lang::fixtures::fixture_source("inventory_sell")
            .unwrap()
            .replace(
                "durability: LocalStable",
                "durability: ReplicatedStable(\"three-regions\")",
            );
        let ast = carolina_lang::parser::parse_module(&src, &carolina_core::limits::Limits::v1())
            .unwrap();
        let (ir2, _) = carolina_lang::lower::lower_module(&ast, None).unwrap();
        let err = match compile(&CompileInput::local(ir2)) {
            Err(e) => e,
            Ok(_) => panic!("expected UnsatisfiableDurability"),
        };
        assert_eq!(err.code, ErrorCode::UnsatisfiableDurability);
    }

    /// S004-A12: tampered certificates and plans fail the checker.
    #[test]
    fn checker_rejects_tampering() {
        let out = compile_fixture("inventory_sell");
        let rules = rules::AnalysisRuleManifest::v1();
        let lib = library::ProtocolLibraryManifest::v1();
        assert!(check_artifacts(&out.plans, &out.certificate, &rules, &lib).is_ok());
        // remove a required judgment
        let mut cert = out.certificate.clone();
        cert.judgments.retain(|j| j.kind != ObligationKind::IrConc);
        assert_eq!(
            check_artifacts(&out.plans, &cert, &rules, &lib)
                .unwrap_err()
                .code,
            ErrorCode::InvalidEvidence
        );
        // modify a plan after certification
        let mut plans = out.plans.clone();
        plans[0].cost.remote_participants += 1;
        assert_eq!(
            check_artifacts(&plans, &out.certificate, &rules, &lib)
                .unwrap_err()
                .code,
            ErrorCode::InvalidEvidence
        );
        // unknown rule version
        let mut rules2 = rules.clone();
        rules2.rules[0].version = 2;
        assert!(check_artifacts(&out.plans, &out.certificate, &rules2, &lib).is_err());
    }

    /// S004-A13: EXPLAIN names candidates, rejections, unknowns, scope and assumptions.
    #[test]
    fn explain_mentions_candidates_and_evidence() {
        let (ir, _) = load_fixture("inventory_sell").unwrap();
        let out = compile(&CompileInput::local(ir.clone())).unwrap();
        let text = explain::explain_operation(&ir, &out, "sell").unwrap();
        assert!(text.contains("C1_COMMUTATIVE_V1"));
        assert!(text.contains("DISPROVEN"));
        assert!(text.contains("stock_nonneg"));
        assert!(text.contains("candidate plan"));
        assert!(text.contains("Assumptions:"));
        assert!(
            text.contains("not qualified before MVP-6"),
            "C3 must be reported as unqualified: {text}"
        );
    }

    /// S004-A08-like: reserve/release compiles; every operation is C0/C5 in the local profile.
    #[test]
    fn reserve_release_compiles() {
        let out = compile_fixture("inventory_reserve_release");
        assert_eq!(out.plans.len(), 4);
        for p in &out.plans {
            assert!(matches!(
                p.profile.family,
                ConsistencyClass::C0Local | ConsistencyClass::C5Serial
            ));
            assert!(matches!(
                p.profile.validation,
                plan::ValidationProgram::FullEvaluation { .. }
            ));
        }
    }
}
