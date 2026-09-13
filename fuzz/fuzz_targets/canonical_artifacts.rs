#![no_main]

use carolina_compiler::{ConsistencyCertificate, OperationPlan};
use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::limits::Limits;
use carolina_lang::ir::ModuleIR;
use carolina_storage::{CompiledBatch, ProtocolOnlyBatch};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::tiny();
    let _ = CanonValue::decode(data, &limits);
    let _ = ModuleIR::decode(data, &limits);
    let _ = OperationPlan::decode(data, &limits);
    let _ = ConsistencyCertificate::decode(data, &limits);
    let _ = CompiledBatch::decode(data, &limits);
    let _ = ProtocolOnlyBatch::decode(data, &limits);
});
