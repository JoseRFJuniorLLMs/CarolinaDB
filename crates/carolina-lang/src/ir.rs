//! Typed canonical IR (SPEC-003 §5–§10).
//!
//! Every node carries a versioned `kind` tag in its canonical encoding. Source spans
//! are not part of the IR. Hashes:
//!
//! * `SchemaHash`    = H(astra.schema.v1, records + enums + indexes + resources + invariants)
//! * `InvariantHash` = H(astra.invariant.v1, InvariantIR)
//! * `ContractHash`  = H(astra.contract.v1, ContractIR)
//! * `OperationHash` = H(astra.operation.v1, OperationIR)   (includes its contract and referenced types)
//! * `ModuleHash`    = H(astra.module.v1, ModuleIR)

use std::collections::BTreeMap;

use carolina_core::canon::{decode_set, CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, domains};
use carolina_core::ids::*;

use crate::ast::{AggregateOp, BinOp, CmpOp};
use crate::types::{Type, Value};

pub const IR_VERSION: u32 = 1;
pub const LANGUAGE_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleIR {
    pub ir_version: u32,
    pub language_version: u32,
    pub key_codec_version: u32,
    pub records: Vec<RecordIR>,
    pub enums: Vec<EnumIR>,
    pub indexes: Vec<IndexIR>,
    pub resources: Vec<ResourceIR>,
    pub invariants: Vec<InvariantIR>,
    pub operations: Vec<OperationIR>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordIR {
    pub id: RecordId,
    pub name: String,
    pub immutable: bool,
    pub fields: Vec<FieldIR>,
    pub primary_key: FieldId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldIR {
    pub id: FieldId,
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumIR {
    pub id: EnumId,
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexIR {
    pub id: IndexId,
    pub name: String,
    pub record: RecordId,
    pub fields: Vec<FieldId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceIR {
    pub id: ResourceDeclId,
    pub name: String,
    pub record: RecordId,
    pub available: FieldId,
    pub reserved: FieldId,
    pub reservation_record: RecordId,
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InvariantKind {
    LowerBound,
    UpperBound,
    Unique,
    Referential,
    AggregateLowerBound,
    AggregateUpperBound,
    Conservation,
    Monotonic,
    StateTransition,
    CausalPrerequisite,
    Arbitrary,
}

impl InvariantKind {
    pub fn label(&self) -> &'static str {
        match self {
            InvariantKind::LowerBound => "LowerBound",
            InvariantKind::UpperBound => "UpperBound",
            InvariantKind::Unique => "Unique",
            InvariantKind::Referential => "Referential",
            InvariantKind::AggregateLowerBound => "AggregateLowerBound",
            InvariantKind::AggregateUpperBound => "AggregateUpperBound",
            InvariantKind::Conservation => "Conservation",
            InvariantKind::Monotonic => "Monotonic",
            InvariantKind::StateTransition => "StateTransition",
            InvariantKind::CausalPrerequisite => "CausalPrerequisite",
            InvariantKind::Arbitrary => "Arbitrary",
        }
    }
    pub fn from_label(s: &str) -> Option<Self> {
        Some(match s {
            "LowerBound" => InvariantKind::LowerBound,
            "UpperBound" => InvariantKind::UpperBound,
            "Unique" => InvariantKind::Unique,
            "Referential" => InvariantKind::Referential,
            "AggregateLowerBound" => InvariantKind::AggregateLowerBound,
            "AggregateUpperBound" => InvariantKind::AggregateUpperBound,
            "Conservation" => InvariantKind::Conservation,
            "Monotonic" => InvariantKind::Monotonic,
            "StateTransition" => InvariantKind::StateTransition,
            "CausalPrerequisite" => InvariantKind::CausalPrerequisite,
            "Arbitrary" => InvariantKind::Arbitrary,
            _ => return None,
        })
    }
}

/// Scope of an invariant or result observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeExpr {
    /// One instance per primary key of `record` (key bound to the row variable / parameter).
    PerKey { record: RecordId },
    /// One instance per group value of `record`.
    PerGroup { record: RecordId, group_by: ExprIR },
    /// The complete record set(s).
    Global { records: Vec<RecordId> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantIR {
    pub id: InvariantId,
    pub name: String,
    pub version: u32,
    pub scope: ScopeExpr,
    pub kind: InvariantKind,
    pub predicate: PredicateIR,
    pub dependencies: Vec<DomainSelector>,
    pub evaluator_version: u32,
}

/// Row variable binding index inside a predicate/operation (`BindingId(0)` is the invariant row variable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BindingId(pub u32);

impl Canonical for BindingId {
    fn to_canon(&self) -> CanonValue {
        CanonValue::u32(self.0)
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(BindingId(v.as_u32()?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateIR {
    ForAll {
        record: RecordId,
        var: BindingId,
        filter: Option<ExprIR>,
        predicate: ExprIR,
    },
    Unique {
        record: RecordId,
        var: BindingId,
        filter: Option<ExprIR>,
        key: ExprIR,
    },
    ExistsReference {
        child: RecordId,
        var: BindingId,
        parent_key: ExprIR,
        parent: RecordId,
    },
    Aggregate {
        op: AggregateOp,
        record: RecordId,
        var: BindingId,
        filter: Option<ExprIR>,
        group_by: Option<ExprIR>,
        value: ExprIR,
        comparison: CmpOp,
        /// May reference `ExprNodeIR::GroupKey` when `group_by` is present.
        bound: ExprIR,
    },
    TransitionPredicate {
        record: RecordId,
        field: FieldId,
        enum_id: EnumId,
        edges: Vec<(u32, u32)>,
    },
    /// Invocations of `operation` require a committed row of immutable record `fact` whose primary key
    /// is built from the operation arguments at `key_params` (a tuple when more than one).
    RequiresFact {
        operation: OperationId,
        key_params: Vec<u32>,
        fact: RecordId,
    },
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationIR {
    pub identity: OperationRef,
    pub name: String,
    pub parameters: Vec<ParameterIR>,
    pub pre: ExprIR,
    pub reads: Vec<ReadBindingIR>,
    pub effects: Vec<EffectIR>,
    pub post: ExprIR,
    pub result: ExprIR,
    pub contract: ContractIR,
    pub footprint: OperationFootprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterIR {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadBindingIR {
    pub binding: BindingId,
    pub name: String,
    pub kind: ReadKind,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadKind {
    Row {
        record: RecordId,
        key: ExprIR,
    },
    OptionalRow {
        record: RecordId,
        key: ExprIR,
    },
    Exists {
        record: RecordId,
        key: ExprIR,
    },
    Scan {
        record: RecordId,
        var: BindingId,
        filter: ExprIR,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectIR {
    pub effect_id: u32,
    pub guard: Option<ExprIR>,
    pub kind: EffectKind,
    pub read_dependencies: Vec<BindingId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathIR {
    pub record: RecordId,
    pub key: ExprIR,
    pub field: FieldId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectKind {
    Assign {
        path: PathIR,
        value: ExprIR,
    },
    Increment {
        path: PathIR,
        amount: ExprIR,
    },
    Decrement {
        path: PathIR,
        amount: ExprIR,
    },
    Insert {
        record: RecordId,
        key: ExprIR,
        fields: Vec<(FieldId, ExprIR)>,
    },
    Delete {
        record: RecordId,
        key: ExprIR,
    },
    AddToSet {
        path: PathIR,
        value: ExprIR,
    },
    RemoveFromSet {
        path: PathIR,
        value: ExprIR,
    },
    CompareAndSwap {
        path: PathIR,
        expected: ExprIR,
        value: ExprIR,
    },
    Reserve {
        resource: ResourceDeclId,
        key: ExprIR,
        reservation_id: ExprIR,
        amount: ExprIR,
    },
    Release {
        resource: ResourceDeclId,
        key: ExprIR,
        reservation_id: ExprIR,
        amount: ExprIR,
    },
    TransferQuantity {
        source: PathIR,
        destination: PathIR,
        amount: ExprIR,
    },
    AdvanceState {
        path: PathIR,
        enum_id: EnumId,
        expected: u32,
        next: u32,
    },
    EmitFact {
        fact: RecordId,
        key: ExprIR,
        fields: Vec<(FieldId, ExprIR)>,
    },
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// Which state a row lookup reads (SPEC-003 §6: pre-state and candidate-state reads are distinguishable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateRef {
    Pre,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprIR {
    pub ty: Type,
    pub node: ExprNodeIR,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprNodeIR {
    Lit(Value),
    Param(u32),
    Binding(BindingId),
    /// Group key of an aggregate invariant.
    GroupKey,
    /// Field of a row value.
    Field {
        base: Box<ExprIR>,
        field: FieldId,
        name: String,
    },
    /// Field of an anonymous struct value.
    StructField {
        base: Box<ExprIR>,
        name: String,
    },
    RowLookup {
        record: RecordId,
        key: Box<ExprIR>,
        state: StateRef,
    },
    Exists {
        record: RecordId,
        key: Box<ExprIR>,
        state: StateRef,
    },
    Tuple(Vec<ExprIR>),
    Struct(Vec<(String, ExprIR)>),
    SetLit(Vec<ExprIR>),
    /// `Some(e)` with a non-literal operand.
    SomeOf(Box<ExprIR>),
    Neg(Box<ExprIR>),
    Not(Box<ExprIR>),
    Bin {
        op: BinOp,
        lhs: Box<ExprIR>,
        rhs: Box<ExprIR>,
    },
    IsNone(Box<ExprIR>),
    IsSome(Box<ExprIR>),
    UnwrapOr {
        value: Box<ExprIR>,
        default: Box<ExprIR>,
    },
    Size(Box<ExprIR>),
    SumOver {
        var: BindingId,
        set: Box<ExprIR>,
        value: Box<ExprIR>,
    },
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InputVisibility {
    LocalSnapshot,
    CausalContext,
    CertifiedScope,
    SerialScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultSemantics {
    Receipt,
    SnapshotValue,
    ExactOrderedValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SessionGuarantee {
    ReadYourWrites,
    MonotonicReads,
    CausalDependencies,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionScope {
    None,
    ReplicationGroup(String),
    CompositeScope(Vec<RecordId>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Durability {
    LocalStable,
    ReplicatedStable(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PartitionOutcome {
    Wait,
    Unavailable,
    AuthorityUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefusalSemantics {
    BusinessPredicate,
    MissingAuthority,
    MissingDependency,
}

/// Result scope of an operation contract (`result_scope`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultScope {
    /// `PerKey(Record[param])`
    PerKey {
        record: RecordId,
        param: u32,
    },
    PerGroup {
        record: RecordId,
        param: u32,
    },
    Global {
        records: Vec<RecordId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractIR {
    pub input_visibility: InputVisibility,
    pub result_semantics: ResultSemantics,
    pub result_scope: ResultScope,
    pub session: Vec<SessionGuarantee>,
    pub session_scope: SessionScope,
    pub durability: Durability,
    pub partition_outcomes: Vec<PartitionOutcome>,
    pub authority_requirements: Vec<String>,
    pub refusal_semantics: RefusalSemantics,
    pub request_namespace: String,
}

// ---------------------------------------------------------------------------
// Footprints
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Purpose {
    ValueRead,
    Write,
    Membership,
    Absence,
    ResultRead,
    Authority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySet {
    /// One key given by an expression over parameters/bindings. `static_key` is true when the
    /// expression depends only on operation parameters (a sound selector for IDC instantiation).
    Point { key: ExprIR, static_key: bool },
    /// The entire record.
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainSelector {
    pub record: RecordId,
    pub key_set: KeySet,
    /// Empty means all fields.
    pub fields: Vec<FieldId>,
    pub purpose: Purpose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactSelector {
    pub record: RecordId,
    pub key: ExprIR,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OperationFootprint {
    pub reads: Vec<DomainSelector>,
    pub writes: Vec<DomainSelector>,
    pub predicates: Vec<DomainSelector>,
    pub facts: Vec<FactSelector>,
    pub atomic_groups: Vec<Vec<u32>>,
}

// ---------------------------------------------------------------------------
// Hashes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleHashes {
    pub schema_hash: SchemaHash,
    pub module_hash: ModuleHash,
    pub invariant_hashes: BTreeMap<InvariantId, InvariantHash>,
    pub operation_hashes: BTreeMap<OperationRef, OperationHash>,
    pub contract_hashes: BTreeMap<OperationRef, ContractHash>,
}

impl ModuleIR {
    pub fn record(&self, id: RecordId) -> CoreResult<&RecordIR> {
        self.records
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidIr, format!("unknown record {id}")))
    }
    pub fn enum_def(&self, id: EnumId) -> CoreResult<&EnumIR> {
        self.enums
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidIr, format!("unknown enum {id}")))
    }
    pub fn resource(&self, id: ResourceDeclId) -> CoreResult<&ResourceIR> {
        self.resources
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidIr, format!("unknown resource {id}")))
    }
    pub fn operation(&self, r: OperationRef) -> CoreResult<&OperationIR> {
        self.operations
            .iter()
            .find(|o| o.identity == r)
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidIr, format!("unknown operation {r:?}")))
    }
    pub fn operation_by_name(&self, name: &str) -> Option<&OperationIR> {
        self.operations.iter().find(|o| o.name == name)
    }
    pub fn record_by_name(&self, name: &str) -> Option<&RecordIR> {
        self.records.iter().find(|r| r.name == name)
    }
    pub fn invariant(&self, id: InvariantId) -> CoreResult<&InvariantIR> {
        self.invariants
            .iter()
            .find(|i| i.id == id)
            .ok_or_else(|| CoreError::new(ErrorCode::InvalidIr, format!("unknown invariant {id}")))
    }

    /// Schema canonical payload: all schema declarations and invariant versions (SPEC-003 §10).
    pub fn schema_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("enums", &self.enums)
            .fvec("indexes", &self.indexes)
            .fvec("invariants", &self.invariants)
            .fu32("ir_version", self.ir_version)
            .fu32("key_codec_version", self.key_codec_version)
            .fstr("kind", "schema.v1")
            .fu32("language_version", self.language_version)
            .fvec("records", &self.records)
            .fvec("resources", &self.resources)
            .build()
    }

    pub fn hashes(&self) -> ModuleHashes {
        let schema_hash = SchemaHash(domain_hash(
            domains::SCHEMA_V1,
            &self.schema_canon().encode(),
        ));
        let module_hash = ModuleHash(domain_hash(domains::MODULE_V1, &self.encode()));
        let mut invariant_hashes = BTreeMap::new();
        for inv in &self.invariants {
            invariant_hashes.insert(
                inv.id,
                InvariantHash(domain_hash(domains::INVARIANT_V1, &inv.encode())),
            );
        }
        let mut operation_hashes = BTreeMap::new();
        let mut contract_hashes = BTreeMap::new();
        for op in &self.operations {
            operation_hashes.insert(
                op.identity,
                OperationHash(domain_hash(domains::OPERATION_V1, &op.encode())),
            );
            contract_hashes.insert(
                op.identity,
                ContractHash(domain_hash(domains::CONTRACT_V1, &op.contract.encode())),
            );
        }
        ModuleHashes {
            schema_hash,
            module_hash,
            invariant_hashes,
            operation_hashes,
            contract_hashes,
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical encoding
// ---------------------------------------------------------------------------

fn kind_err(what: &str, got: &str) -> CoreError {
    CoreError::new(
        ErrorCode::NonCanonicalEncoding,
        format!("unexpected {what} kind `{got}`"),
    )
}

impl Canonical for ModuleIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("enums", &self.enums)
            .fvec("indexes", &self.indexes)
            .fvec("invariants", &self.invariants)
            .fu32("ir_version", self.ir_version)
            .fu32("key_codec_version", self.key_codec_version)
            .fstr("kind", "module.v1")
            .fu32("language_version", self.language_version)
            .fvec("operations", &self.operations)
            .fvec("records", &self.records)
            .fvec("resources", &self.resources)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "enums",
            "indexes",
            "invariants",
            "ir_version",
            "key_codec_version",
            "kind",
            "language_version",
            "operations",
            "records",
            "resources",
        ])?;
        if v.field("kind")?.as_str()? != "module.v1" {
            return Err(kind_err("module", v.field("kind")?.as_str()?));
        }
        let ir_version = v.field("ir_version")?.as_u32()?;
        if ir_version != IR_VERSION {
            return Err(CoreError::new(
                ErrorCode::UnsupportedIrVersion,
                format!("ir_version {ir_version} unsupported"),
            ));
        }
        Ok(ModuleIR {
            ir_version,
            language_version: v.field("language_version")?.as_u32()?,
            key_codec_version: v.field("key_codec_version")?.as_u32()?,
            records: Vec::from_canon(v.field("records")?)?,
            enums: Vec::from_canon(v.field("enums")?)?,
            indexes: Vec::from_canon(v.field("indexes")?)?,
            resources: Vec::from_canon(v.field("resources")?)?,
            invariants: Vec::from_canon(v.field("invariants")?)?,
            operations: Vec::from_canon(v.field("operations")?)?,
        })
    }
}

impl Canonical for RecordIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("fields", &self.fields)
            .fc("id", &self.id)
            .fbool("immutable", self.immutable)
            .fstr("kind", "record.v1")
            .fstr("name", &self.name)
            .fc("primary_key", &self.primary_key)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["fields", "id", "immutable", "kind", "name", "primary_key"])?;
        Ok(RecordIR {
            id: RecordId::from_canon(v.field("id")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            immutable: v.field("immutable")?.as_bool()?,
            fields: Vec::from_canon(v.field("fields")?)?,
            primary_key: FieldId::from_canon(v.field("primary_key")?)?,
        })
    }
}

impl Canonical for FieldIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("id", &self.id)
            .fstr("kind", "field.v1")
            .fstr("name", &self.name)
            .fc("type", &self.ty)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["id", "kind", "name", "type"])?;
        Ok(FieldIR {
            id: FieldId::from_canon(v.field("id")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            ty: Type::from_canon(v.field("type")?)?,
        })
    }
}

impl Canonical for EnumIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("id", &self.id)
            .fstr("kind", "enum.v1")
            .fstr("name", &self.name)
            .fvec("variants", &self.variants)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["id", "kind", "name", "variants"])?;
        Ok(EnumIR {
            id: EnumId::from_canon(v.field("id")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            variants: Vec::from_canon(v.field("variants")?)?,
        })
    }
}

impl Canonical for IndexIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("fields", &self.fields)
            .fc("id", &self.id)
            .fstr("kind", "index.v1")
            .fstr("name", &self.name)
            .fc("record", &self.record)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["fields", "id", "kind", "name", "record"])?;
        Ok(IndexIR {
            id: IndexId::from_canon(v.field("id")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            record: RecordId::from_canon(v.field("record")?)?,
            fields: Vec::from_canon(v.field("fields")?)?,
        })
    }
}

impl Canonical for ResourceIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("available", &self.available)
            .fc("id", &self.id)
            .fstr("kind", "resource.v1")
            .fstr("name", &self.name)
            .fc("record", &self.record)
            .fc("reservation_record", &self.reservation_record)
            .fc("reserved", &self.reserved)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "available",
            "id",
            "kind",
            "name",
            "record",
            "reservation_record",
            "reserved",
        ])?;
        Ok(ResourceIR {
            id: ResourceDeclId::from_canon(v.field("id")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            record: RecordId::from_canon(v.field("record")?)?,
            available: FieldId::from_canon(v.field("available")?)?,
            reserved: FieldId::from_canon(v.field("reserved")?)?,
            reservation_record: RecordId::from_canon(v.field("reservation_record")?)?,
        })
    }
}

impl Canonical for InvariantKind {
    fn to_canon(&self) -> CanonValue {
        CanonValue::str(self.label())
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        InvariantKind::from_label(v.as_str()?)
            .ok_or_else(|| kind_err("invariant", v.as_str().unwrap_or("?")))
    }
}

impl Canonical for ScopeExpr {
    fn to_canon(&self) -> CanonValue {
        match self {
            ScopeExpr::PerKey { record } => CanonValue::obj()
                .fstr("kind", "per_key")
                .fc("record", record)
                .build(),
            ScopeExpr::PerGroup { record, group_by } => CanonValue::obj()
                .fc("group_by", group_by)
                .fstr("kind", "per_group")
                .fc("record", record)
                .build(),
            ScopeExpr::Global { records } => CanonValue::obj()
                .fstr("kind", "global")
                .fvec("records", records)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "per_key" => ScopeExpr::PerKey {
                record: RecordId::from_canon(v.field("record")?)?,
            },
            "per_group" => ScopeExpr::PerGroup {
                record: RecordId::from_canon(v.field("record")?)?,
                group_by: ExprIR::from_canon(v.field("group_by")?)?,
            },
            "global" => ScopeExpr::Global {
                records: Vec::from_canon(v.field("records")?)?,
            },
            k => return Err(kind_err("scope", k)),
        })
    }
}

impl Canonical for InvariantIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("dependencies", &self.dependencies)
            .fu32("evaluator_version", self.evaluator_version)
            .fc("id", &self.id)
            .fc("invariant_kind", &self.kind)
            .fstr("kind", "invariant.v1")
            .fstr("name", &self.name)
            .fc("predicate", &self.predicate)
            .fc("scope", &self.scope)
            .fu32("version", self.version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "dependencies",
            "evaluator_version",
            "id",
            "invariant_kind",
            "kind",
            "name",
            "predicate",
            "scope",
            "version",
        ])?;
        Ok(InvariantIR {
            id: InvariantId::from_canon(v.field("id")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            version: v.field("version")?.as_u32()?,
            scope: ScopeExpr::from_canon(v.field("scope")?)?,
            kind: InvariantKind::from_canon(v.field("invariant_kind")?)?,
            predicate: PredicateIR::from_canon(v.field("predicate")?)?,
            dependencies: Vec::from_canon(v.field("dependencies")?)?,
            evaluator_version: v.field("evaluator_version")?.as_u32()?,
        })
    }
}

fn agg_label(op: AggregateOp) -> &'static str {
    match op {
        AggregateOp::Sum => "sum",
        AggregateOp::Count => "count",
    }
}
fn cmp_label(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "eq",
        CmpOp::Ne => "ne",
        CmpOp::Lt => "lt",
        CmpOp::Le => "le",
        CmpOp::Gt => "gt",
        CmpOp::Ge => "ge",
    }
}
fn cmp_from(s: &str) -> CoreResult<CmpOp> {
    Ok(match s {
        "eq" => CmpOp::Eq,
        "ne" => CmpOp::Ne,
        "lt" => CmpOp::Lt,
        "le" => CmpOp::Le,
        "gt" => CmpOp::Gt,
        "ge" => CmpOp::Ge,
        k => return Err(kind_err("comparison", k)),
    })
}
fn bin_label(op: BinOp) -> String {
    match op {
        BinOp::Add => "add".into(),
        BinOp::Sub => "sub".into(),
        BinOp::Mul => "mul".into(),
        BinOp::And => "and".into(),
        BinOp::Or => "or".into(),
        BinOp::In => "in".into(),
        BinOp::Cmp(c) => format!("cmp_{}", cmp_label(c)),
    }
}
fn bin_from(s: &str) -> CoreResult<BinOp> {
    Ok(match s {
        "add" => BinOp::Add,
        "sub" => BinOp::Sub,
        "mul" => BinOp::Mul,
        "and" => BinOp::And,
        "or" => BinOp::Or,
        "in" => BinOp::In,
        s if s.starts_with("cmp_") => BinOp::Cmp(cmp_from(&s[4..])?),
        k => return Err(kind_err("binop", k)),
    })
}

fn u32_vec(v: &[u32]) -> CanonValue {
    CanonValue::Array(v.iter().map(|x| CanonValue::u32(*x)).collect())
}
fn u32_vec_from(v: &CanonValue) -> CoreResult<Vec<u32>> {
    v.as_array()?.iter().map(|x| x.as_u32()).collect()
}

impl Canonical for PredicateIR {
    fn to_canon(&self) -> CanonValue {
        match self {
            PredicateIR::ForAll {
                record,
                var,
                filter,
                predicate,
            } => CanonValue::obj()
                .fopt("filter", filter)
                .fstr("kind", "forall")
                .fc("predicate", predicate)
                .fc("record", record)
                .fc("var", var)
                .build(),
            PredicateIR::Unique {
                record,
                var,
                filter,
                key,
            } => CanonValue::obj()
                .fopt("filter", filter)
                .fc("key", key)
                .fstr("kind", "unique")
                .fc("record", record)
                .fc("var", var)
                .build(),
            PredicateIR::ExistsReference {
                child,
                var,
                parent_key,
                parent,
            } => CanonValue::obj()
                .fc("child", child)
                .fstr("kind", "exists_reference")
                .fc("parent", parent)
                .fc("parent_key", parent_key)
                .fc("var", var)
                .build(),
            PredicateIR::Aggregate {
                op,
                record,
                var,
                filter,
                group_by,
                value,
                comparison,
                bound,
            } => CanonValue::obj()
                .fc("bound", bound)
                .fstr("comparison", cmp_label(*comparison))
                .fopt("filter", filter)
                .fopt("group_by", group_by)
                .fstr("kind", "aggregate")
                .fstr("op", agg_label(*op))
                .fc("record", record)
                .fc("value", value)
                .fc("var", var)
                .build(),
            PredicateIR::TransitionPredicate {
                record,
                field,
                enum_id,
                edges,
            } => CanonValue::obj()
                .f(
                    "edges",
                    CanonValue::Array(
                        edges
                            .iter()
                            .map(|(a, b)| {
                                CanonValue::Array(vec![CanonValue::u32(*a), CanonValue::u32(*b)])
                            })
                            .collect(),
                    ),
                )
                .fc("enum", enum_id)
                .fc("field", field)
                .fstr("kind", "transition")
                .fc("record", record)
                .build(),
            PredicateIR::RequiresFact {
                operation,
                key_params,
                fact,
            } => CanonValue::obj()
                .fc("fact", fact)
                .f("key_params", u32_vec(key_params))
                .fstr("kind", "requires_fact")
                .fc("operation", operation)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "forall" => PredicateIR::ForAll {
                record: RecordId::from_canon(v.field("record")?)?,
                var: BindingId::from_canon(v.field("var")?)?,
                filter: Option::from_canon(v.field("filter")?)?,
                predicate: ExprIR::from_canon(v.field("predicate")?)?,
            },
            "unique" => PredicateIR::Unique {
                record: RecordId::from_canon(v.field("record")?)?,
                var: BindingId::from_canon(v.field("var")?)?,
                filter: Option::from_canon(v.field("filter")?)?,
                key: ExprIR::from_canon(v.field("key")?)?,
            },
            "exists_reference" => PredicateIR::ExistsReference {
                child: RecordId::from_canon(v.field("child")?)?,
                var: BindingId::from_canon(v.field("var")?)?,
                parent_key: ExprIR::from_canon(v.field("parent_key")?)?,
                parent: RecordId::from_canon(v.field("parent")?)?,
            },
            "aggregate" => PredicateIR::Aggregate {
                op: match v.field("op")?.as_str()? {
                    "sum" => AggregateOp::Sum,
                    "count" => AggregateOp::Count,
                    k => return Err(kind_err("aggregate op", k)),
                },
                record: RecordId::from_canon(v.field("record")?)?,
                var: BindingId::from_canon(v.field("var")?)?,
                filter: Option::from_canon(v.field("filter")?)?,
                group_by: Option::from_canon(v.field("group_by")?)?,
                value: ExprIR::from_canon(v.field("value")?)?,
                comparison: cmp_from(v.field("comparison")?.as_str()?)?,
                bound: ExprIR::from_canon(v.field("bound")?)?,
            },
            "transition" => PredicateIR::TransitionPredicate {
                record: RecordId::from_canon(v.field("record")?)?,
                field: FieldId::from_canon(v.field("field")?)?,
                enum_id: EnumId::from_canon(v.field("enum")?)?,
                edges: v
                    .field("edges")?
                    .as_array()?
                    .iter()
                    .map(|e| {
                        let a = e.as_array()?;
                        if a.len() != 2 {
                            return Err(CoreError::new(
                                ErrorCode::NonCanonicalEncoding,
                                "edge must be a pair",
                            ));
                        }
                        Ok((a[0].as_u32()?, a[1].as_u32()?))
                    })
                    .collect::<CoreResult<_>>()?,
            },
            "requires_fact" => PredicateIR::RequiresFact {
                operation: OperationId::from_canon(v.field("operation")?)?,
                key_params: u32_vec_from(v.field("key_params")?)?,
                fact: RecordId::from_canon(v.field("fact")?)?,
            },
            k => return Err(kind_err("predicate", k)),
        })
    }
}

impl Canonical for OperationIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("contract", &self.contract)
            .fvec("effects", &self.effects)
            .fc("footprint", &self.footprint)
            .fc("identity", &self.identity)
            .fstr("kind", "operation.v1")
            .fstr("name", &self.name)
            .fvec("parameters", &self.parameters)
            .fc("post", &self.post)
            .fc("pre", &self.pre)
            .fvec("reads", &self.reads)
            .fc("result", &self.result)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "contract",
            "effects",
            "footprint",
            "identity",
            "kind",
            "name",
            "parameters",
            "post",
            "pre",
            "reads",
            "result",
        ])?;
        Ok(OperationIR {
            identity: OperationRef::from_canon(v.field("identity")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            parameters: Vec::from_canon(v.field("parameters")?)?,
            pre: ExprIR::from_canon(v.field("pre")?)?,
            reads: Vec::from_canon(v.field("reads")?)?,
            effects: Vec::from_canon(v.field("effects")?)?,
            post: ExprIR::from_canon(v.field("post")?)?,
            result: ExprIR::from_canon(v.field("result")?)?,
            contract: ContractIR::from_canon(v.field("contract")?)?,
            footprint: OperationFootprint::from_canon(v.field("footprint")?)?,
        })
    }
}

impl Canonical for ParameterIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("kind", "parameter.v1")
            .fstr("name", &self.name)
            .fc("type", &self.ty)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["kind", "name", "type"])?;
        Ok(ParameterIR {
            name: v.field("name")?.as_str()?.to_string(),
            ty: Type::from_canon(v.field("type")?)?,
        })
    }
}

impl Canonical for ReadBindingIR {
    fn to_canon(&self) -> CanonValue {
        let kind = match &self.kind {
            ReadKind::Row { record, key } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "row")
                .fc("record", record)
                .build(),
            ReadKind::OptionalRow { record, key } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "optional_row")
                .fc("record", record)
                .build(),
            ReadKind::Exists { record, key } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "exists")
                .fc("record", record)
                .build(),
            ReadKind::Scan {
                record,
                var,
                filter,
            } => CanonValue::obj()
                .fc("filter", filter)
                .fstr("kind", "scan")
                .fc("record", record)
                .fc("var", var)
                .build(),
        };
        CanonValue::obj()
            .fc("binding", &self.binding)
            .fstr("kind", "read.v1")
            .fstr("name", &self.name)
            .f("read", kind)
            .fc("type", &self.ty)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["binding", "kind", "name", "read", "type"])?;
        let r = v.field("read")?;
        let kind = match r.field("kind")?.as_str()? {
            "row" => ReadKind::Row {
                record: RecordId::from_canon(r.field("record")?)?,
                key: ExprIR::from_canon(r.field("key")?)?,
            },
            "optional_row" => ReadKind::OptionalRow {
                record: RecordId::from_canon(r.field("record")?)?,
                key: ExprIR::from_canon(r.field("key")?)?,
            },
            "exists" => ReadKind::Exists {
                record: RecordId::from_canon(r.field("record")?)?,
                key: ExprIR::from_canon(r.field("key")?)?,
            },
            "scan" => ReadKind::Scan {
                record: RecordId::from_canon(r.field("record")?)?,
                var: BindingId::from_canon(r.field("var")?)?,
                filter: ExprIR::from_canon(r.field("filter")?)?,
            },
            k => return Err(kind_err("read", k)),
        };
        Ok(ReadBindingIR {
            binding: BindingId::from_canon(v.field("binding")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            kind,
            ty: Type::from_canon(v.field("type")?)?,
        })
    }
}

impl Canonical for PathIR {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("field", &self.field)
            .fc("key", &self.key)
            .fstr("kind", "path.v1")
            .fc("record", &self.record)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["field", "key", "kind", "record"])?;
        Ok(PathIR {
            record: RecordId::from_canon(v.field("record")?)?,
            key: ExprIR::from_canon(v.field("key")?)?,
            field: FieldId::from_canon(v.field("field")?)?,
        })
    }
}

fn field_inits(fields: &[(FieldId, ExprIR)]) -> CanonValue {
    CanonValue::Array(
        fields
            .iter()
            .map(|(f, e)| CanonValue::obj().fc("field", f).fc("value", e).build())
            .collect(),
    )
}
fn field_inits_from(v: &CanonValue) -> CoreResult<Vec<(FieldId, ExprIR)>> {
    v.as_array()?
        .iter()
        .map(|x| {
            Ok((
                FieldId::from_canon(x.field("field")?)?,
                ExprIR::from_canon(x.field("value")?)?,
            ))
        })
        .collect()
}

impl Canonical for EffectIR {
    fn to_canon(&self) -> CanonValue {
        let k = match &self.kind {
            EffectKind::Assign { path, value } => CanonValue::obj()
                .fstr("kind", "assign")
                .fc("path", path)
                .fc("value", value)
                .build(),
            EffectKind::Increment { path, amount } => CanonValue::obj()
                .fc("amount", amount)
                .fstr("kind", "increment")
                .fc("path", path)
                .build(),
            EffectKind::Decrement { path, amount } => CanonValue::obj()
                .fc("amount", amount)
                .fstr("kind", "decrement")
                .fc("path", path)
                .build(),
            EffectKind::Insert {
                record,
                key,
                fields,
            } => CanonValue::obj()
                .f("fields", field_inits(fields))
                .fc("key", key)
                .fstr("kind", "insert")
                .fc("record", record)
                .build(),
            EffectKind::Delete { record, key } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "delete")
                .fc("record", record)
                .build(),
            EffectKind::AddToSet { path, value } => CanonValue::obj()
                .fstr("kind", "add_to_set")
                .fc("path", path)
                .fc("value", value)
                .build(),
            EffectKind::RemoveFromSet { path, value } => CanonValue::obj()
                .fstr("kind", "remove_from_set")
                .fc("path", path)
                .fc("value", value)
                .build(),
            EffectKind::CompareAndSwap {
                path,
                expected,
                value,
            } => CanonValue::obj()
                .fc("expected", expected)
                .fstr("kind", "compare_and_swap")
                .fc("path", path)
                .fc("value", value)
                .build(),
            EffectKind::Reserve {
                resource,
                key,
                reservation_id,
                amount,
            } => CanonValue::obj()
                .fc("amount", amount)
                .fc("key", key)
                .fstr("kind", "reserve")
                .fc("reservation_id", reservation_id)
                .fc("resource", resource)
                .build(),
            EffectKind::Release {
                resource,
                key,
                reservation_id,
                amount,
            } => CanonValue::obj()
                .fc("amount", amount)
                .fc("key", key)
                .fstr("kind", "release")
                .fc("reservation_id", reservation_id)
                .fc("resource", resource)
                .build(),
            EffectKind::TransferQuantity {
                source,
                destination,
                amount,
            } => CanonValue::obj()
                .fc("amount", amount)
                .fc("destination", destination)
                .fstr("kind", "transfer_quantity")
                .fc("source", source)
                .build(),
            EffectKind::AdvanceState {
                path,
                enum_id,
                expected,
                next,
            } => CanonValue::obj()
                .fc("enum", enum_id)
                .fu32("expected", *expected)
                .fstr("kind", "advance_state")
                .fu32("next", *next)
                .fc("path", path)
                .build(),
            EffectKind::EmitFact { fact, key, fields } => CanonValue::obj()
                .fc("fact", fact)
                .f("fields", field_inits(fields))
                .fc("key", key)
                .fstr("kind", "emit_fact")
                .build(),
        };
        CanonValue::obj()
            .f("effect", k)
            .fu32("effect_id", self.effect_id)
            .fopt("guard", &self.guard)
            .fstr("kind", "effect.v1")
            .fvec("read_dependencies", &self.read_dependencies)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["effect", "effect_id", "guard", "kind", "read_dependencies"])?;
        let e = v.field("effect")?;
        let path = |n: &str| PathIR::from_canon(e.field(n)?);
        let ex = |n: &str| ExprIR::from_canon(e.field(n)?);
        let kind = match e.field("kind")?.as_str()? {
            "assign" => EffectKind::Assign {
                path: path("path")?,
                value: ex("value")?,
            },
            "increment" => EffectKind::Increment {
                path: path("path")?,
                amount: ex("amount")?,
            },
            "decrement" => EffectKind::Decrement {
                path: path("path")?,
                amount: ex("amount")?,
            },
            "insert" => EffectKind::Insert {
                record: RecordId::from_canon(e.field("record")?)?,
                key: ex("key")?,
                fields: field_inits_from(e.field("fields")?)?,
            },
            "delete" => EffectKind::Delete {
                record: RecordId::from_canon(e.field("record")?)?,
                key: ex("key")?,
            },
            "add_to_set" => EffectKind::AddToSet {
                path: path("path")?,
                value: ex("value")?,
            },
            "remove_from_set" => EffectKind::RemoveFromSet {
                path: path("path")?,
                value: ex("value")?,
            },
            "compare_and_swap" => EffectKind::CompareAndSwap {
                path: path("path")?,
                expected: ex("expected")?,
                value: ex("value")?,
            },
            "reserve" => EffectKind::Reserve {
                resource: ResourceDeclId::from_canon(e.field("resource")?)?,
                key: ex("key")?,
                reservation_id: ex("reservation_id")?,
                amount: ex("amount")?,
            },
            "release" => EffectKind::Release {
                resource: ResourceDeclId::from_canon(e.field("resource")?)?,
                key: ex("key")?,
                reservation_id: ex("reservation_id")?,
                amount: ex("amount")?,
            },
            "transfer_quantity" => EffectKind::TransferQuantity {
                source: path("source")?,
                destination: path("destination")?,
                amount: ex("amount")?,
            },
            "advance_state" => EffectKind::AdvanceState {
                path: path("path")?,
                enum_id: EnumId::from_canon(e.field("enum")?)?,
                expected: e.field("expected")?.as_u32()?,
                next: e.field("next")?.as_u32()?,
            },
            "emit_fact" => EffectKind::EmitFact {
                fact: RecordId::from_canon(e.field("fact")?)?,
                key: ex("key")?,
                fields: field_inits_from(e.field("fields")?)?,
            },
            k => return Err(kind_err("effect", k)),
        };
        Ok(EffectIR {
            effect_id: v.field("effect_id")?.as_u32()?,
            guard: Option::from_canon(v.field("guard")?)?,
            kind,
            read_dependencies: Vec::from_canon(v.field("read_dependencies")?)?,
        })
    }
}

fn state_label(s: StateRef) -> &'static str {
    match s {
        StateRef::Pre => "pre",
        StateRef::Candidate => "candidate",
    }
}
fn state_from(s: &str) -> CoreResult<StateRef> {
    Ok(match s {
        "pre" => StateRef::Pre,
        "candidate" => StateRef::Candidate,
        k => return Err(kind_err("state", k)),
    })
}

impl Canonical for ExprIR {
    fn to_canon(&self) -> CanonValue {
        let n = match &self.node {
            ExprNodeIR::Lit(v) => CanonValue::obj().fstr("kind", "lit").fc("value", v).build(),
            ExprNodeIR::Param(i) => CanonValue::obj()
                .fu32("index", *i)
                .fstr("kind", "param")
                .build(),
            ExprNodeIR::Binding(b) => CanonValue::obj()
                .fc("binding", b)
                .fstr("kind", "binding")
                .build(),
            ExprNodeIR::GroupKey => CanonValue::obj().fstr("kind", "group_key").build(),
            ExprNodeIR::Field { base, field, name } => CanonValue::obj()
                .fc("base", base.as_ref())
                .fc("field", field)
                .fstr("kind", "field")
                .fstr("name", name)
                .build(),
            ExprNodeIR::StructField { base, name } => CanonValue::obj()
                .fc("base", base.as_ref())
                .fstr("kind", "struct_field")
                .fstr("name", name)
                .build(),
            ExprNodeIR::RowLookup { record, key, state } => CanonValue::obj()
                .fc("key", key.as_ref())
                .fstr("kind", "row_lookup")
                .fc("record", record)
                .fstr("state", state_label(*state))
                .build(),
            ExprNodeIR::Exists { record, key, state } => CanonValue::obj()
                .fc("key", key.as_ref())
                .fstr("kind", "exists")
                .fc("record", record)
                .fstr("state", state_label(*state))
                .build(),
            ExprNodeIR::Tuple(items) => CanonValue::obj()
                .fvec("items", items)
                .fstr("kind", "tuple")
                .build(),
            ExprNodeIR::Struct(fields) => {
                let mut o = BTreeMap::new();
                for (k, e) in fields {
                    o.insert(k.clone(), e.to_canon());
                }
                CanonValue::obj()
                    .f("fields", CanonValue::Object(o))
                    .fstr("kind", "struct")
                    .build()
            }
            ExprNodeIR::SetLit(items) => CanonValue::obj()
                .fvec("items", items)
                .fstr("kind", "set_lit")
                .build(),
            ExprNodeIR::SomeOf(e) => CanonValue::obj()
                .fc("expr", e.as_ref())
                .fstr("kind", "some_of")
                .build(),
            ExprNodeIR::Neg(e) => CanonValue::obj()
                .fc("expr", e.as_ref())
                .fstr("kind", "neg")
                .build(),
            ExprNodeIR::Not(e) => CanonValue::obj()
                .fc("expr", e.as_ref())
                .fstr("kind", "not")
                .build(),
            ExprNodeIR::Bin { op, lhs, rhs } => CanonValue::obj()
                .fstr("kind", "bin")
                .fc("lhs", lhs.as_ref())
                .fstr("op", &bin_label(*op))
                .fc("rhs", rhs.as_ref())
                .build(),
            ExprNodeIR::IsNone(e) => CanonValue::obj()
                .fc("expr", e.as_ref())
                .fstr("kind", "is_none")
                .build(),
            ExprNodeIR::IsSome(e) => CanonValue::obj()
                .fc("expr", e.as_ref())
                .fstr("kind", "is_some")
                .build(),
            ExprNodeIR::UnwrapOr { value, default } => CanonValue::obj()
                .fc("default", default.as_ref())
                .fstr("kind", "unwrap_or")
                .fc("value", value.as_ref())
                .build(),
            ExprNodeIR::Size(e) => CanonValue::obj()
                .fc("expr", e.as_ref())
                .fstr("kind", "size")
                .build(),
            ExprNodeIR::SumOver { var, set, value } => CanonValue::obj()
                .fstr("kind", "sum_over")
                .fc("set", set.as_ref())
                .fc("value", value.as_ref())
                .fc("var", var)
                .build(),
        };
        CanonValue::obj()
            .fstr("kind", "expr.v1")
            .f("node", n)
            .fc("type", &self.ty)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["kind", "node", "type"])?;
        let ty = Type::from_canon(v.field("type")?)?;
        let n = v.field("node")?;
        let bx = |name: &str| -> CoreResult<Box<ExprIR>> {
            Ok(Box::new(ExprIR::from_canon(n.field(name)?)?))
        };
        let node = match n.field("kind")?.as_str()? {
            "lit" => ExprNodeIR::Lit(Value::from_canon(n.field("value")?)?),
            "param" => ExprNodeIR::Param(n.field("index")?.as_u32()?),
            "binding" => ExprNodeIR::Binding(BindingId::from_canon(n.field("binding")?)?),
            "group_key" => ExprNodeIR::GroupKey,
            "field" => ExprNodeIR::Field {
                base: bx("base")?,
                field: FieldId::from_canon(n.field("field")?)?,
                name: n.field("name")?.as_str()?.to_string(),
            },
            "struct_field" => ExprNodeIR::StructField {
                base: bx("base")?,
                name: n.field("name")?.as_str()?.to_string(),
            },
            "row_lookup" => ExprNodeIR::RowLookup {
                record: RecordId::from_canon(n.field("record")?)?,
                key: bx("key")?,
                state: state_from(n.field("state")?.as_str()?)?,
            },
            "exists" => ExprNodeIR::Exists {
                record: RecordId::from_canon(n.field("record")?)?,
                key: bx("key")?,
                state: state_from(n.field("state")?.as_str()?)?,
            },
            "tuple" => ExprNodeIR::Tuple(Vec::from_canon(n.field("items")?)?),
            "struct" => {
                let mut fields = Vec::new();
                for (k, e) in n.field("fields")?.as_object()? {
                    fields.push((k.clone(), ExprIR::from_canon(e)?));
                }
                ExprNodeIR::Struct(fields)
            }
            "set_lit" => ExprNodeIR::SetLit(Vec::from_canon(n.field("items")?)?),
            "some_of" => ExprNodeIR::SomeOf(bx("expr")?),
            "neg" => ExprNodeIR::Neg(bx("expr")?),
            "not" => ExprNodeIR::Not(bx("expr")?),
            "bin" => ExprNodeIR::Bin {
                op: bin_from(n.field("op")?.as_str()?)?,
                lhs: bx("lhs")?,
                rhs: bx("rhs")?,
            },
            "is_none" => ExprNodeIR::IsNone(bx("expr")?),
            "is_some" => ExprNodeIR::IsSome(bx("expr")?),
            "unwrap_or" => ExprNodeIR::UnwrapOr {
                value: bx("value")?,
                default: bx("default")?,
            },
            "size" => ExprNodeIR::Size(bx("expr")?),
            "sum_over" => ExprNodeIR::SumOver {
                var: BindingId::from_canon(n.field("var")?)?,
                set: bx("set")?,
                value: bx("value")?,
            },
            k => return Err(kind_err("expr", k)),
        };
        Ok(ExprIR { ty, node })
    }
}

fn labels<T: Copy>(items: &[T], f: fn(T) -> &'static str) -> CanonValue {
    CanonValue::set(items.iter().map(|x| CanonValue::str(f(*x))).collect()).expect("unique labels")
}

impl Canonical for ContractIR {
    fn to_canon(&self) -> CanonValue {
        let vis = match self.input_visibility {
            InputVisibility::LocalSnapshot => "LocalSnapshot",
            InputVisibility::CausalContext => "CausalContext",
            InputVisibility::CertifiedScope => "CertifiedScope",
            InputVisibility::SerialScope => "SerialScope",
        };
        let rs = match self.result_semantics {
            ResultSemantics::Receipt => "Receipt",
            ResultSemantics::SnapshotValue => "SnapshotValue",
            ResultSemantics::ExactOrderedValue => "ExactOrderedValue",
        };
        let scope = match &self.result_scope {
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
        let session_scope = match &self.session_scope {
            SessionScope::None => CanonValue::obj().fstr("kind", "none").build(),
            SessionScope::ReplicationGroup(g) => CanonValue::obj()
                .fstr("group", g)
                .fstr("kind", "replication_group")
                .build(),
            SessionScope::CompositeScope(rs) => CanonValue::obj()
                .fstr("kind", "composite_scope")
                .fvec("records", rs)
                .build(),
        };
        let durability = match &self.durability {
            Durability::LocalStable => CanonValue::obj().fstr("kind", "LocalStable").build(),
            Durability::ReplicatedStable(p) => CanonValue::obj()
                .fstr("kind", "ReplicatedStable")
                .fstr("policy", p)
                .build(),
        };
        let refusal = match self.refusal_semantics {
            RefusalSemantics::BusinessPredicate => "BusinessPredicate",
            RefusalSemantics::MissingAuthority => "MissingAuthority",
            RefusalSemantics::MissingDependency => "MissingDependency",
        };
        CanonValue::obj()
            .fstr("atomicity", "WholeInvocation")
            .fvec("authority_requirements", &self.authority_requirements)
            .fstr("commitment", "FinalWhenDurable")
            .f("durability", durability)
            .fstr("input_visibility", vis)
            .fstr("kind", "contract.v1")
            .f(
                "partition_outcomes",
                labels(&self.partition_outcomes, |p| match p {
                    PartitionOutcome::Wait => "Wait",
                    PartitionOutcome::Unavailable => "Unavailable",
                    PartitionOutcome::AuthorityUnavailable => "AuthorityUnavailable",
                }),
            )
            .fstr("refusal_semantics", refusal)
            .fstr("request_namespace", &self.request_namespace)
            .f("result_scope", scope)
            .fstr("result_semantics", rs)
            .f(
                "session",
                labels(&self.session, |s| match s {
                    SessionGuarantee::ReadYourWrites => "ReadYourWrites",
                    SessionGuarantee::MonotonicReads => "MonotonicReads",
                    SessionGuarantee::CausalDependencies => "CausalDependencies",
                }),
            )
            .f("session_scope", session_scope)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "atomicity",
            "authority_requirements",
            "commitment",
            "durability",
            "input_visibility",
            "kind",
            "partition_outcomes",
            "refusal_semantics",
            "request_namespace",
            "result_scope",
            "result_semantics",
            "session",
            "session_scope",
        ])?;
        if v.field("atomicity")?.as_str()? != "WholeInvocation"
            || v.field("commitment")?.as_str()? != "FinalWhenDurable"
        {
            return Err(CoreError::new(
                ErrorCode::InvalidContract,
                "unsupported atomicity/commitment",
            ));
        }
        let input_visibility = match v.field("input_visibility")?.as_str()? {
            "LocalSnapshot" => InputVisibility::LocalSnapshot,
            "CausalContext" => InputVisibility::CausalContext,
            "CertifiedScope" => InputVisibility::CertifiedScope,
            "SerialScope" => InputVisibility::SerialScope,
            k => return Err(kind_err("input_visibility", k)),
        };
        let result_semantics = match v.field("result_semantics")?.as_str()? {
            "Receipt" => ResultSemantics::Receipt,
            "SnapshotValue" => ResultSemantics::SnapshotValue,
            "ExactOrderedValue" => ResultSemantics::ExactOrderedValue,
            k => return Err(kind_err("result_semantics", k)),
        };
        let s = v.field("result_scope")?;
        let result_scope = match s.field("kind")?.as_str()? {
            "per_key" => ResultScope::PerKey {
                record: RecordId::from_canon(s.field("record")?)?,
                param: s.field("param")?.as_u32()?,
            },
            "per_group" => ResultScope::PerGroup {
                record: RecordId::from_canon(s.field("record")?)?,
                param: s.field("param")?.as_u32()?,
            },
            "global" => ResultScope::Global {
                records: Vec::from_canon(s.field("records")?)?,
            },
            k => return Err(kind_err("result_scope", k)),
        };
        let session = decode_set::<String>(v.field("session")?)?
            .iter()
            .map(|s| match s.as_str() {
                "ReadYourWrites" => Ok(SessionGuarantee::ReadYourWrites),
                "MonotonicReads" => Ok(SessionGuarantee::MonotonicReads),
                "CausalDependencies" => Ok(SessionGuarantee::CausalDependencies),
                k => Err(kind_err("session guarantee", k)),
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let ss = v.field("session_scope")?;
        let session_scope = match ss.field("kind")?.as_str()? {
            "none" => SessionScope::None,
            "replication_group" => {
                SessionScope::ReplicationGroup(ss.field("group")?.as_str()?.to_string())
            }
            "composite_scope" => {
                SessionScope::CompositeScope(Vec::from_canon(ss.field("records")?)?)
            }
            k => return Err(kind_err("session_scope", k)),
        };
        let d = v.field("durability")?;
        let durability = match d.field("kind")?.as_str()? {
            "LocalStable" => Durability::LocalStable,
            "ReplicatedStable" => {
                Durability::ReplicatedStable(d.field("policy")?.as_str()?.to_string())
            }
            k => return Err(kind_err("durability", k)),
        };
        let partition_outcomes = decode_set::<String>(v.field("partition_outcomes")?)?
            .iter()
            .map(|s| match s.as_str() {
                "Wait" => Ok(PartitionOutcome::Wait),
                "Unavailable" => Ok(PartitionOutcome::Unavailable),
                "AuthorityUnavailable" => Ok(PartitionOutcome::AuthorityUnavailable),
                k => Err(kind_err("partition outcome", k)),
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let refusal_semantics = match v.field("refusal_semantics")?.as_str()? {
            "BusinessPredicate" => RefusalSemantics::BusinessPredicate,
            "MissingAuthority" => RefusalSemantics::MissingAuthority,
            "MissingDependency" => RefusalSemantics::MissingDependency,
            k => return Err(kind_err("refusal_semantics", k)),
        };
        Ok(ContractIR {
            input_visibility,
            result_semantics,
            result_scope,
            session,
            session_scope,
            durability,
            partition_outcomes,
            authority_requirements: Vec::from_canon(v.field("authority_requirements")?)?,
            refusal_semantics,
            request_namespace: v.field("request_namespace")?.as_str()?.to_string(),
        })
    }
}

impl Canonical for Purpose {
    fn to_canon(&self) -> CanonValue {
        CanonValue::str(match self {
            Purpose::ValueRead => "value_read",
            Purpose::Write => "write",
            Purpose::Membership => "membership",
            Purpose::Absence => "absence",
            Purpose::ResultRead => "result_read",
            Purpose::Authority => "authority",
        })
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.as_str()? {
            "value_read" => Purpose::ValueRead,
            "write" => Purpose::Write,
            "membership" => Purpose::Membership,
            "absence" => Purpose::Absence,
            "result_read" => Purpose::ResultRead,
            "authority" => Purpose::Authority,
            k => return Err(kind_err("purpose", k)),
        })
    }
}

impl Canonical for DomainSelector {
    fn to_canon(&self) -> CanonValue {
        let ks = match &self.key_set {
            KeySet::Point { key, static_key } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "point")
                .fbool("static_key", *static_key)
                .build(),
            KeySet::All => CanonValue::obj().fstr("kind", "all").build(),
        };
        CanonValue::obj()
            .fvec("fields", &self.fields)
            .f("key_set", ks)
            .fstr("kind", "selector.v1")
            .fc("purpose", &self.purpose)
            .fc("record", &self.record)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["fields", "key_set", "kind", "purpose", "record"])?;
        let ks = v.field("key_set")?;
        let key_set = match ks.field("kind")?.as_str()? {
            "point" => KeySet::Point {
                key: ExprIR::from_canon(ks.field("key")?)?,
                static_key: ks.field("static_key")?.as_bool()?,
            },
            "all" => KeySet::All,
            k => return Err(kind_err("key_set", k)),
        };
        Ok(DomainSelector {
            record: RecordId::from_canon(v.field("record")?)?,
            key_set,
            fields: Vec::from_canon(v.field("fields")?)?,
            purpose: Purpose::from_canon(v.field("purpose")?)?,
        })
    }
}

impl Canonical for FactSelector {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("key", &self.key)
            .fstr("kind", "fact_selector.v1")
            .fc("record", &self.record)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["key", "kind", "record"])?;
        Ok(FactSelector {
            record: RecordId::from_canon(v.field("record")?)?,
            key: ExprIR::from_canon(v.field("key")?)?,
        })
    }
}

impl Canonical for OperationFootprint {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .f(
                "atomic_groups",
                CanonValue::Array(self.atomic_groups.iter().map(|g| u32_vec(g)).collect()),
            )
            .fvec("facts", &self.facts)
            .fstr("kind", "footprint.v1")
            .fvec("predicates", &self.predicates)
            .fvec("reads", &self.reads)
            .fvec("writes", &self.writes)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "atomic_groups",
            "facts",
            "kind",
            "predicates",
            "reads",
            "writes",
        ])?;
        Ok(OperationFootprint {
            reads: Vec::from_canon(v.field("reads")?)?,
            writes: Vec::from_canon(v.field("writes")?)?,
            predicates: Vec::from_canon(v.field("predicates")?)?,
            facts: Vec::from_canon(v.field("facts")?)?,
            atomic_groups: v
                .field("atomic_groups")?
                .as_array()?
                .iter()
                .map(u32_vec_from)
                .collect::<CoreResult<_>>()?,
        })
    }
}
