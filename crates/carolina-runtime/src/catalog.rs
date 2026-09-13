//! Local catalog: one module, its canonical plans and certificate, checked for the local profile.
//!
//! SPEC-014 §5: every active plan binds operation/contract versions, template, authority and
//! durability models. In the local profile only plans whose atomicity is one local batch and whose
//! durability is `LocalStable` may activate; anything else is refused here rather than mocked.

use std::collections::BTreeMap;

use carolina_compiler::plan::{AtomicityProgram, DurabilityProgram};
use carolina_compiler::{check_artifacts, compile, CompileInput, CompileOutput, OperationPlan};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::{CatalogGeneration, ConsistencyClass, OperationRef, RecordId};
use carolina_core::limits::Limits;
use carolina_lang::ir::{ModuleHashes, ModuleIR};
use carolina_lang::lower::lower_module;
use carolina_lang::parser::parse_module;

pub struct LocalCatalog {
    pub module: ModuleIR,
    pub hashes: ModuleHashes,
    pub output: CompileOutput,
    pub generation: CatalogGeneration,
    plans_by_op: BTreeMap<OperationRef, usize>,
}

impl LocalCatalog {
    /// Parse, lower, compile and check a module for the local single-node topology.
    pub fn from_source(src: &str) -> CoreResult<LocalCatalog> {
        let ast = parse_module(src, &Limits::v1())?;
        let (ir, _) = lower_module(&ast, None)?;
        Self::from_module(ir)
    }

    pub fn from_module(module: ModuleIR) -> CoreResult<LocalCatalog> {
        let input = CompileInput::local(module.clone());
        let output = compile(&input)?;
        // the artifact checker is the same one `carolina check` runs: refuse to activate what it refuses
        check_artifacts(
            &output.plans,
            &output.certificate,
            &input.analysis_rules,
            &input.protocol_library,
        )?;
        let mut plans_by_op = BTreeMap::new();
        for (i, p) in output.plans.iter().enumerate() {
            Self::check_local_profile(p)?;
            plans_by_op.insert(p.operation, i);
        }
        let hashes = module.hashes();
        Ok(LocalCatalog {
            module,
            hashes,
            output,
            generation: input.active_generation,
            plans_by_op,
        })
    }

    fn check_local_profile(p: &OperationPlan) -> CoreResult<()> {
        if p.profile.atomicity != AtomicityProgram::LocalBatch {
            return Err(CoreError::new(
                ErrorCode::MissingRuntimeCapability,
                format!(
                    "plan for {} needs composite atomicity; not available in the local profile",
                    p.operation_name
                ),
            ));
        }
        if p.profile.durability != DurabilityProgram::LocalStable {
            return Err(CoreError::new(
                ErrorCode::MissingRuntimeCapability,
                format!(
                    "plan for {} needs replicated durability; not available in the local profile",
                    p.operation_name
                ),
            ));
        }
        if !matches!(
            p.profile.family,
            ConsistencyClass::C0Local | ConsistencyClass::C5Serial
        ) {
            return Err(CoreError::new(
                ErrorCode::MissingRuntimeCapability,
                format!(
                    "plan family {} for {} is not qualified in the local profile",
                    p.profile.family.label(),
                    p.operation_name
                ),
            ));
        }
        Ok(())
    }

    pub fn plan_for(&self, op: OperationRef) -> Option<&OperationPlan> {
        self.plans_by_op.get(&op).map(|i| &self.output.plans[*i])
    }

    /// Records an invocation of `op` may read or write, including invariant closure (SPEC-004 §5).
    pub fn closure_records(&self, op: OperationRef) -> Vec<RecordId> {
        match self.output.closure.op_records.get(&op) {
            Some(set) if !set.is_empty() => set.iter().copied().collect(),
            _ => self.module.records.iter().map(|r| r.id).collect(),
        }
    }
}
