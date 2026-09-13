//! Versioned resource limits (SPEC-003 §13, SPEC-012 §9).
//!
//! Limits are explicit inputs to compilation, decoding and transport. They are
//! part of reproducibility manifests; exceeding a frontend/decoder bound rejects
//! the input before allocation.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub limits_version: u32,
    /// Maximum canonical payload bytes accepted by a decoder.
    pub max_payload_bytes: usize,
    /// Maximum nesting depth of canonical values.
    pub max_depth: usize,
    /// Maximum elements in one collection (array/object/set).
    pub max_collection_len: usize,
    /// Maximum bytes of one string or byte array.
    pub max_scalar_bytes: usize,
    /// Maximum DSL source bytes.
    pub max_source_bytes: usize,
    /// Maximum declarations (records + indexes + invariants + operations) in one module.
    pub max_declarations: usize,
    /// Maximum expression depth in the DSL/IR.
    pub max_expr_depth: usize,
    /// Maximum set cardinality of a literal or evaluated set.
    pub max_set_cardinality: usize,
    /// Deterministic analysis work budget (abstract steps).
    pub analysis_budget: u64,
    /// SPEC-012 §9 transport limits
    pub max_frame_payload: usize,
    pub max_arguments_bytes: usize,
    pub max_result_bytes: usize,
    pub max_token_bytes: usize,
    pub max_snapshot_chunk_bytes: usize,
}

impl Limits {
    /// The v1 defaults from SPEC-012 §9 and conservative compiler bounds.
    pub const fn v1() -> Limits {
        Limits {
            limits_version: 1,
            max_payload_bytes: 16 * 1024 * 1024,
            max_depth: 64,
            max_collection_len: 65_536,
            max_scalar_bytes: 1024 * 1024,
            max_source_bytes: 4 * 1024 * 1024,
            max_declarations: 4096,
            max_expr_depth: 64,
            max_set_cardinality: 65_536,
            analysis_budget: 5_000_000,
            max_frame_payload: 16 * 1024 * 1024,
            max_arguments_bytes: 1024 * 1024,
            max_result_bytes: 1024 * 1024,
            max_token_bytes: 64 * 1024,
            max_snapshot_chunk_bytes: 4 * 1024 * 1024,
        }
    }

    /// Small limits for fuzz/negative tests.
    pub const fn tiny() -> Limits {
        Limits {
            limits_version: 1,
            max_payload_bytes: 4096,
            max_depth: 8,
            max_collection_len: 16,
            max_scalar_bytes: 256,
            max_source_bytes: 4096,
            max_declarations: 32,
            max_expr_depth: 8,
            max_set_cardinality: 16,
            analysis_budget: 10_000,
            max_frame_payload: 4096,
            max_arguments_bytes: 1024,
            max_result_bytes: 1024,
            max_token_bytes: 512,
            max_snapshot_chunk_bytes: 2048,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits::v1()
    }
}
