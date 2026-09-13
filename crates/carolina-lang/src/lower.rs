//! Resolve names, allocate stable identities, type-check and lower the AST into
//! canonical IR with footprints (SPEC-003 §3.1, §4, §7, §8).
//!
//! Identity allocation is deterministic: names already present in a prior
//! [`IdAllocation`] keep their IDs; new names receive the next free ID in
//! declaration order. Nothing here depends on hash-map iteration or host paths.

use std::collections::{BTreeMap, BTreeSet};

use carolina_core::decimal::Decimal;
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::*;
use carolina_core::keycodec::KEY_CODEC_VERSION;

use crate::ast::{
    self, AggregateOp, BinOp, CmpOp, ContractValue, Expr, ExprNode, InvariantBody, Item, Module,
    ReadExpr, TypeExpr,
};
use crate::ir::*;
use crate::types::{Type, Value};

/// Stable identities assigned by the catalog allocation step (SPEC-003 §3.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdAllocation {
    pub records: BTreeMap<String, RecordId>,
    pub fields: BTreeMap<(String, String), FieldId>,
    pub enums: BTreeMap<String, EnumId>,
    pub indexes: BTreeMap<String, IndexId>,
    pub resources: BTreeMap<String, ResourceDeclId>,
    pub invariants: BTreeMap<String, InvariantId>,
    pub operations: BTreeMap<String, OperationId>,
}

impl IdAllocation {
    /// Return the existing id for `name` or allocate the next free id deterministically.
    fn get_or_alloc<T: Copy>(
        map: &mut BTreeMap<String, T>,
        name: &str,
        raw: fn(&T) -> u64,
        mk: fn(u64) -> T,
    ) -> T {
        if let Some(v) = map.get(name) {
            return *v;
        }
        let max = map.values().map(raw).max().unwrap_or(0);
        let id = mk(max + 1);
        map.insert(name.to_string(), id);
        id
    }
}

fn err(code: ErrorCode, msg: impl Into<String>, span: ast::Span) -> CoreError {
    CoreError::new(code, msg).at(span.to_string())
}

#[derive(Debug, Clone)]
struct RecordInfo {
    id: RecordId,
    #[allow(dead_code)]
    name: String,
    immutable: bool,
    fields: Vec<(String, FieldId, Type)>,
    pk: FieldId,
    pk_type: Type,
}

impl RecordInfo {
    fn field(&self, name: &str) -> Option<(FieldId, &Type)> {
        self.fields
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, id, t)| (*id, t))
    }
    fn field_by_id(&self, id: FieldId) -> Option<(&str, &Type)> {
        self.fields
            .iter()
            .find(|(_, i, _)| *i == id)
            .map(|(n, _, t)| (n.as_str(), t))
    }
    fn row_type(&self) -> Type {
        Type::Row(self.id)
    }
}

#[derive(Debug, Clone)]
struct EnumInfo {
    id: EnumId,
    variants: Vec<String>,
}

#[derive(Debug, Clone)]
struct OpSig {
    id: OperationId,
    params: Vec<(String, Type)>,
}

struct Env {
    records: BTreeMap<String, RecordInfo>,
    enums: BTreeMap<String, EnumInfo>,
    resources: BTreeMap<String, ResourceIR>,
    ops: BTreeMap<String, OpSig>,
}

impl Env {
    fn record(&self, id: &ast::Ident) -> CoreResult<&RecordInfo> {
        self.records.get(&id.name).ok_or_else(|| {
            err(
                ErrorCode::MissingRecord,
                format!("unknown record `{}`", id.name),
                id.span,
            )
        })
    }
    fn record_by_id(&self, id: RecordId) -> &RecordInfo {
        self.records
            .values()
            .find(|r| r.id == id)
            .expect("record id")
    }
    fn enum_by_id(&self, id: EnumId) -> &EnumInfo {
        self.enums.values().find(|e| e.id == id).expect("enum id")
    }
}

#[derive(Debug, Clone)]
enum Sym {
    Param(u32, Type),
    Binding(BindingId, Type),
    GroupKey(Type),
}

#[derive(Default, Clone)]
struct Scope {
    syms: Vec<(String, Sym)>,
}

impl Scope {
    fn lookup(&self, name: &str) -> Option<&Sym> {
        self.syms
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
    }
    fn push(&mut self, name: &str, sym: Sym) {
        self.syms.push((name.to_string(), sym));
    }
}

struct Ctx<'a> {
    env: &'a Env,
    scope: Scope,
    state: StateRef,
    next_binding: u32,
    /// Row lookups / exists encountered (for footprint inference): (record, key expr, purpose)
    lookups: Vec<(RecordId, ExprIR, Purpose)>,
    bindings_used: BTreeSet<BindingId>,
    depth: usize,
}

impl<'a> Ctx<'a> {
    fn new(env: &'a Env, state: StateRef) -> Self {
        Ctx {
            env,
            scope: Scope::default(),
            state,
            next_binding: 0,
            lookups: Vec::new(),
            bindings_used: BTreeSet::new(),
            depth: 0,
        }
    }
    fn fresh_binding(&mut self) -> BindingId {
        let b = BindingId(self.next_binding);
        self.next_binding += 1;
        b
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn lower_module(
    module: &Module,
    prior: Option<&IdAllocation>,
) -> CoreResult<(ModuleIR, IdAllocation)> {
    let mut alloc = prior.cloned().unwrap_or_default();

    // ---- pass 1: declare enums and records (IDs) --------------------------
    let mut enums_ir = Vec::new();
    let mut env = Env {
        records: BTreeMap::new(),
        enums: BTreeMap::new(),
        resources: BTreeMap::new(),
        ops: BTreeMap::new(),
    };
    let mut seen_names: BTreeSet<String> = BTreeSet::new();
    let mut check_dup = |id: &ast::Ident| -> CoreResult<()> {
        if !seen_names.insert(id.name.clone()) {
            return Err(err(
                ErrorCode::DuplicateIdentity,
                format!("duplicate declaration `{}`", id.name),
                id.span,
            ));
        }
        Ok(())
    };
    for item in &module.items {
        if let Item::Enum(e) = item {
            check_dup(&e.name)?;
            let id = IdAllocation::get_or_alloc(&mut alloc.enums, &e.name.name, |v| v.0, EnumId);
            let mut variants = Vec::new();
            for v in &e.variants {
                if variants.contains(&v.name) {
                    return Err(err(
                        ErrorCode::DuplicateIdentity,
                        format!("duplicate enum variant `{}`", v.name),
                        v.span,
                    ));
                }
                variants.push(v.name.clone());
            }
            env.enums.insert(
                e.name.name.clone(),
                EnumInfo {
                    id,
                    variants: variants.clone(),
                },
            );
            enums_ir.push(EnumIR {
                id,
                name: e.name.name.clone(),
                variants,
            });
        }
    }
    let mut records_ir = Vec::new();
    for item in &module.items {
        if let Item::Record(r) = item {
            check_dup(&r.name)?;
            let id =
                IdAllocation::get_or_alloc(&mut alloc.records, &r.name.name, |v| v.0, RecordId);
            let mut fields = Vec::new();
            let mut fields_ir = Vec::new();
            let mut pk: Option<(FieldId, Type)> = None;
            let mut used: BTreeSet<String> = BTreeSet::new();
            for f in &r.fields {
                if !used.insert(f.name.name.clone()) {
                    return Err(err(
                        ErrorCode::DuplicateIdentity,
                        format!("duplicate field `{}`", f.name.name),
                        f.name.span,
                    ));
                }
                let ty = lower_type(&f.ty, &env, f.name.span)?;
                let key = (r.name.name.clone(), f.name.name.clone());
                let fid = match alloc.fields.get(&key) {
                    Some(v) => *v,
                    None => {
                        let max = alloc
                            .fields
                            .iter()
                            .filter(|((rn, _), _)| *rn == r.name.name)
                            .map(|(_, v)| v.0)
                            .max()
                            .unwrap_or(0);
                        let fid = FieldId(max + 1);
                        alloc.fields.insert(key, fid);
                        fid
                    }
                };
                if f.primary_key {
                    if pk.is_some() {
                        return Err(err(
                            ErrorCode::InvalidIr,
                            "record has more than one primary key",
                            f.name.span,
                        ));
                    }
                    if !ty.is_comparable() || matches!(ty, Type::Option(_)) {
                        return Err(err(
                            ErrorCode::TypeMismatch,
                            "primary key type must be a comparable non-optional scalar/tuple",
                            f.name.span,
                        ));
                    }
                    pk = Some((fid, ty.clone()));
                }
                fields.push((f.name.name.clone(), fid, ty.clone()));
                fields_ir.push(FieldIR {
                    id: fid,
                    name: f.name.name.clone(),
                    ty,
                });
            }
            let (pk, pk_type) = pk.ok_or_else(|| {
                err(
                    ErrorCode::InvalidIr,
                    format!("record `{}` has no primary key", r.name.name),
                    r.name.span,
                )
            })?;
            env.records.insert(
                r.name.name.clone(),
                RecordInfo {
                    id,
                    name: r.name.name.clone(),
                    immutable: r.immutable,
                    fields,
                    pk,
                    pk_type,
                },
            );
            records_ir.push(RecordIR {
                id,
                name: r.name.name.clone(),
                immutable: r.immutable,
                fields: fields_ir,
                primary_key: pk,
            });
        }
    }

    // ---- indexes, resources -------------------------------------------
    let mut indexes_ir = Vec::new();
    let mut resources_ir = Vec::new();
    for item in &module.items {
        match item {
            Item::Index(ix) => {
                check_dup(&ix.name)?;
                let rec = env.record(&ix.record)?;
                let id =
                    IdAllocation::get_or_alloc(&mut alloc.indexes, &ix.name.name, |v| v.0, IndexId);
                let mut fields = Vec::new();
                for f in &ix.fields {
                    let (fid, _) = rec.field(&f.name).ok_or_else(|| {
                        err(
                            ErrorCode::MissingRecord,
                            format!("unknown field `{}`", f.name),
                            f.span,
                        )
                    })?;
                    fields.push(fid);
                }
                indexes_ir.push(IndexIR {
                    id,
                    name: ix.name.name.clone(),
                    record: rec.id,
                    fields,
                });
            }
            Item::Resource(rs) => {
                check_dup(&rs.name)?;
                let rec = env.record(&rs.record)?;
                let (available, aty) = rec.field(&rs.available.name).ok_or_else(|| {
                    err(
                        ErrorCode::MissingRecord,
                        "unknown available field",
                        rs.available.span,
                    )
                })?;
                let (reserved, rty) = rec.field(&rs.reserved.name).ok_or_else(|| {
                    err(
                        ErrorCode::MissingRecord,
                        "unknown reserved field",
                        rs.reserved.span,
                    )
                })?;
                if aty != rty || !aty.is_numeric() {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "resource available/reserved fields must share one numeric type",
                        rs.name.span,
                    ));
                }
                let resv = env.record(&rs.reservation_record)?;
                // reservation record shape: PK id, `resource` (key type of resource record), `amount` (quantity type), `state` (enum Active/Consumed/Released)
                let (_, res_ty) = resv.field("resource").ok_or_else(|| {
                    err(
                        ErrorCode::InvalidIr,
                        "reservation record needs field `resource`",
                        rs.reservation_record.span,
                    )
                })?;
                if *res_ty != rec.pk_type {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "reservation.resource must have the resource record key type",
                        rs.reservation_record.span,
                    ));
                }
                let (_, amt_ty) = resv.field("amount").ok_or_else(|| {
                    err(
                        ErrorCode::InvalidIr,
                        "reservation record needs field `amount`",
                        rs.reservation_record.span,
                    )
                })?;
                if amt_ty != aty {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "reservation.amount must have the resource quantity type",
                        rs.reservation_record.span,
                    ));
                }
                let (_, st_ty) = resv.field("state").ok_or_else(|| {
                    err(
                        ErrorCode::InvalidIr,
                        "reservation record needs field `state`",
                        rs.reservation_record.span,
                    )
                })?;
                match st_ty {
                    Type::Enum(e) => {
                        let info = env.enum_by_id(*e);
                        for v in ["Active", "Consumed", "Released"] {
                            if !info.variants.iter().any(|x| x == v) {
                                return Err(err(
                                    ErrorCode::InvalidIr,
                                    format!("reservation state enum needs variant `{v}`"),
                                    rs.reservation_record.span,
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(err(
                            ErrorCode::TypeMismatch,
                            "reservation.state must be an enum",
                            rs.reservation_record.span,
                        ))
                    }
                }
                let id = IdAllocation::get_or_alloc(
                    &mut alloc.resources,
                    &rs.name.name,
                    |v| v.0,
                    ResourceDeclId,
                );
                let ir = ResourceIR {
                    id,
                    name: rs.name.name.clone(),
                    record: rec.id,
                    available,
                    reserved,
                    reservation_record: resv.id,
                };
                env.resources.insert(rs.name.name.clone(), ir.clone());
                resources_ir.push(ir);
            }
            _ => {}
        }
    }

    // ---- operation signatures (needed by RequiresFact invariants) ------
    for item in &module.items {
        if let Item::Operation(op) = item {
            check_dup(&op.name)?;
            let id = IdAllocation::get_or_alloc(
                &mut alloc.operations,
                &op.name.name,
                |v| v.0,
                OperationId,
            );
            let mut params = Vec::new();
            for p in &op.params {
                if params.iter().any(|(n, _)| n == &p.name.name) {
                    return Err(err(
                        ErrorCode::DuplicateIdentity,
                        format!("duplicate parameter `{}`", p.name.name),
                        p.name.span,
                    ));
                }
                params.push((p.name.name.clone(), lower_type(&p.ty, &env, p.name.span)?));
            }
            env.ops.insert(op.name.name.clone(), OpSig { id, params });
        }
    }

    // ---- invariants ----------------------------------------------------
    let mut invariants_ir = Vec::new();
    for item in &module.items {
        if let Item::Invariant(inv) = item {
            check_dup(&inv.name)?;
            let id = IdAllocation::get_or_alloc(
                &mut alloc.invariants,
                &inv.name.name,
                |v| v.0,
                InvariantId,
            );
            invariants_ir.push(lower_invariant(inv, id, &env)?);
        }
    }

    // ---- operations ----------------------------------------------------
    let mut operations_ir = Vec::new();
    for item in &module.items {
        if let Item::Operation(op) = item {
            let sig = env.ops.get(&op.name.name).expect("sig");
            operations_ir.push(lower_operation(op, sig, &env)?);
        }
    }

    // canonical ordering by stable id (SPEC-003 §10)
    records_ir.sort_by_key(|r| r.id);
    enums_ir.sort_by_key(|e| e.id);
    indexes_ir.sort_by_key(|i| i.id);
    resources_ir.sort_by_key(|r| r.id);
    invariants_ir.sort_by_key(|i| i.id);
    operations_ir.sort_by_key(|o| o.identity);

    Ok((
        ModuleIR {
            ir_version: IR_VERSION,
            language_version: LANGUAGE_VERSION,
            key_codec_version: KEY_CODEC_VERSION,
            records: records_ir,
            enums: enums_ir,
            indexes: indexes_ir,
            resources: resources_ir,
            invariants: invariants_ir,
            operations: operations_ir,
        },
        alloc,
    ))
}

fn lower_type(t: &TypeExpr, env: &Env, span: ast::Span) -> CoreResult<Type> {
    Ok(match t {
        TypeExpr::Bool => Type::Bool,
        TypeExpr::I64 => Type::I64,
        TypeExpr::U64 => Type::U64,
        TypeExpr::Uuid => Type::Uuid,
        TypeExpr::Bytes(n) => Type::Bytes(*n),
        TypeExpr::String(n) => Type::String(*n),
        TypeExpr::Decimal(p, s) => {
            Decimal::zero(*p, *s).map_err(|e| err(e.code, e.message, span))?;
            Type::Decimal(*p, *s)
        }
        TypeExpr::Named(id) => match env.enums.get(&id.name) {
            Some(e) => Type::Enum(e.id),
            None => {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    format!("unknown type `{}`", id.name),
                    id.span,
                ))
            }
        },
        TypeExpr::Option(inner) => Type::Option(Box::new(lower_type(inner, env, span)?)),
        TypeExpr::Set(inner) => {
            let t = lower_type(inner, env, span)?;
            if !t.is_comparable() {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "set element type must be comparable",
                    span,
                ));
            }
            Type::Set(Box::new(t))
        }
        TypeExpr::Tuple(items) => Type::Tuple(
            items
                .iter()
                .map(|i| lower_type(i, env, span))
                .collect::<CoreResult<_>>()?,
        ),
    })
}

// ---------------------------------------------------------------------------
// Invariants
// ---------------------------------------------------------------------------

fn lower_invariant(
    inv: &ast::InvariantDecl,
    id: InvariantId,
    env: &Env,
) -> CoreResult<InvariantIR> {
    let hint = match &inv.kind_hint {
        Some(h) => Some(InvariantKind::from_label(&h.name).ok_or_else(|| {
            err(
                ErrorCode::InvalidIr,
                format!("unknown invariant kind `{}`", h.name),
                h.span,
            )
        })?),
        None => None,
    };
    let mut ctx = Ctx::new(env, StateRef::Candidate);
    let (scope, kind, predicate) = match &inv.body {
        InvariantBody::ForAll {
            var,
            record,
            filter,
            predicate,
        } => {
            let rec = env.record(record)?;
            let b = ctx.fresh_binding();
            ctx.scope.push(&var.name, Sym::Binding(b, rec.row_type()));
            let filter = filter
                .as_ref()
                .map(|f| lower_expr(&mut ctx, f, Some(&Type::Bool)))
                .transpose()?;
            let pred = lower_expr(&mut ctx, predicate, Some(&Type::Bool))?;
            let inferred = infer_bound_kind(&pred, b).unwrap_or(InvariantKind::Arbitrary);
            let kind = match hint {
                Some(
                    k @ (InvariantKind::Conservation
                    | InvariantKind::Monotonic
                    | InvariantKind::Arbitrary
                    | InvariantKind::LowerBound
                    | InvariantKind::UpperBound),
                ) => k,
                Some(other) => {
                    return Err(err(
                        ErrorCode::InvalidIr,
                        format!("kind {} does not match a FORALL body", other.label()),
                        inv.name.span,
                    ))
                }
                None => inferred,
            };
            (
                ScopeExpr::PerKey { record: rec.id },
                kind,
                PredicateIR::ForAll {
                    record: rec.id,
                    var: b,
                    filter,
                    predicate: pred,
                },
            )
        }
        InvariantBody::Unique {
            var,
            record,
            filter,
            key,
        } => {
            let rec = env.record(record)?;
            let b = ctx.fresh_binding();
            ctx.scope.push(&var.name, Sym::Binding(b, rec.row_type()));
            let filter = filter
                .as_ref()
                .map(|f| lower_expr(&mut ctx, f, Some(&Type::Bool)))
                .transpose()?;
            let key = lower_expr(&mut ctx, key, None)?;
            if !key.ty.is_comparable() {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "unique key must be comparable",
                    inv.name.span,
                ));
            }
            (
                ScopeExpr::Global {
                    records: vec![rec.id],
                },
                InvariantKind::Unique,
                PredicateIR::Unique {
                    record: rec.id,
                    var: b,
                    filter,
                    key,
                },
            )
        }
        InvariantBody::Reference {
            var,
            record,
            parent_key,
            parent,
        } => {
            let child = env.record(record)?;
            let parent_rec = env.record(parent)?;
            let b = ctx.fresh_binding();
            ctx.scope.push(&var.name, Sym::Binding(b, child.row_type()));
            let pk = lower_expr(&mut ctx, parent_key, Some(&parent_rec.pk_type))?;
            (
                ScopeExpr::Global {
                    records: vec![child.id, parent_rec.id],
                },
                InvariantKind::Referential,
                PredicateIR::ExistsReference {
                    child: child.id,
                    var: b,
                    parent_key: pk,
                    parent: parent_rec.id,
                },
            )
        }
        InvariantBody::Aggregate {
            op,
            value,
            var,
            record,
            filter,
            group_by,
            comparison,
            bound,
        } => {
            let rec = env.record(record)?;
            let b = ctx.fresh_binding();
            ctx.scope.push(&var.name, Sym::Binding(b, rec.row_type()));
            let filter = filter
                .as_ref()
                .map(|f| lower_expr(&mut ctx, f, Some(&Type::Bool)))
                .transpose()?;
            let value = lower_expr(&mut ctx, value, None)?;
            let agg_ty = match op {
                AggregateOp::Sum => {
                    if !value.ty.is_numeric() {
                        return Err(err(
                            ErrorCode::TypeMismatch,
                            "SUM value must be numeric",
                            inv.name.span,
                        ));
                    }
                    value.ty.clone()
                }
                AggregateOp::Count => Type::U64,
            };
            let group_by = match group_by {
                Some(g) => {
                    let ge = lower_expr(&mut ctx, g, None)?;
                    if !ge.ty.is_comparable() {
                        return Err(err(
                            ErrorCode::TypeMismatch,
                            "GROUP BY key must be comparable",
                            inv.name.span,
                        ));
                    }
                    Some(ge)
                }
                None => None,
            };
            // bound scope: the row variable is out of scope; GROUP key is available
            let mut bctx = Ctx::new(env, StateRef::Candidate);
            bctx.next_binding = ctx.next_binding;
            if let Some(g) = &group_by {
                bctx.scope.push("GROUP", Sym::GroupKey(g.ty.clone()));
            }
            let bound = lower_expr(&mut bctx, bound, Some(&agg_ty))?;
            ctx.lookups.extend(bctx.lookups);
            let kind = match hint {
                Some(k) => k,
                None => match comparison {
                    CmpOp::Le | CmpOp::Lt => InvariantKind::AggregateUpperBound,
                    CmpOp::Ge | CmpOp::Gt => InvariantKind::AggregateLowerBound,
                    CmpOp::Eq => InvariantKind::Conservation,
                    CmpOp::Ne => InvariantKind::Arbitrary,
                },
            };
            let scope = match &group_by {
                Some(g) => ScopeExpr::PerGroup {
                    record: rec.id,
                    group_by: g.clone(),
                },
                None => ScopeExpr::Global {
                    records: vec![rec.id],
                },
            };
            (
                scope,
                kind,
                PredicateIR::Aggregate {
                    op: *op,
                    record: rec.id,
                    var: b,
                    filter,
                    group_by,
                    value,
                    comparison: *comparison,
                    bound,
                },
            )
        }
        InvariantBody::Transition {
            record,
            field,
            edges,
            ..
        } => {
            let rec = env.record(record)?;
            let (fid, fty) = rec.field(&field.name).ok_or_else(|| {
                err(
                    ErrorCode::MissingRecord,
                    format!("unknown field `{}`", field.name),
                    field.span,
                )
            })?;
            let enum_id = match fty {
                Type::Enum(e) => *e,
                _ => {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "transition field must be an enum",
                        field.span,
                    ))
                }
            };
            let info = env.enum_by_id(enum_id);
            let mut es = Vec::new();
            for (a, b) in edges {
                let ai = info
                    .variants
                    .iter()
                    .position(|v| *v == a.name)
                    .ok_or_else(|| {
                        err(
                            ErrorCode::TypeMismatch,
                            format!("unknown variant `{}`", a.name),
                            a.span,
                        )
                    })?;
                let bi = info
                    .variants
                    .iter()
                    .position(|v| *v == b.name)
                    .ok_or_else(|| {
                        err(
                            ErrorCode::TypeMismatch,
                            format!("unknown variant `{}`", b.name),
                            b.span,
                        )
                    })?;
                es.push((ai as u32, bi as u32));
            }
            (
                ScopeExpr::PerKey { record: rec.id },
                InvariantKind::StateTransition,
                PredicateIR::TransitionPredicate {
                    record: rec.id,
                    field: fid,
                    enum_id,
                    edges: es,
                },
            )
        }
        InvariantBody::Requires {
            operation,
            op_params,
            fact,
            fact_params,
        } => {
            let sig = env.ops.get(&operation.name).ok_or_else(|| {
                err(
                    ErrorCode::MissingRecord,
                    format!("unknown operation `{}`", operation.name),
                    operation.span,
                )
            })?;
            let fact_rec = env.record(fact)?;
            if !fact_rec.immutable {
                return Err(err(
                    ErrorCode::InvalidIr,
                    "REQUIRES fact must be an IMMUTABLE record",
                    fact.span,
                ));
            }
            if fact_params.len() != op_params.len()
                || fact_params
                    .iter()
                    .zip(op_params)
                    .any(|(a, b)| a.name != b.name)
            {
                return Err(err(
                    ErrorCode::InvalidIr,
                    "REQUIRES fact parameter list must repeat the operation parameters",
                    fact.span,
                ));
            }
            let mut key_params = Vec::new();
            let mut key_types = Vec::new();
            for p in op_params {
                let idx = sig
                    .params
                    .iter()
                    .position(|(n, _)| *n == p.name)
                    .ok_or_else(|| {
                        err(
                            ErrorCode::MissingRecord,
                            format!("unknown parameter `{}`", p.name),
                            p.span,
                        )
                    })?;
                key_params.push(idx as u32);
                key_types.push(sig.params[idx].1.clone());
            }
            let key_ty = if key_types.len() == 1 {
                key_types.remove(0)
            } else {
                Type::Tuple(key_types)
            };
            if key_ty != fact_rec.pk_type {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "REQUIRES key parameters must match the fact primary key type",
                    fact.span,
                ));
            }
            (
                ScopeExpr::Global {
                    records: vec![fact_rec.id],
                },
                InvariantKind::CausalPrerequisite,
                PredicateIR::RequiresFact {
                    operation: sig.id,
                    key_params,
                    fact: fact_rec.id,
                },
            )
        }
    };
    let mut dependencies = Vec::new();
    // primary dependency: the scoped record(s)
    match &scope {
        ScopeExpr::PerKey { record } | ScopeExpr::PerGroup { record, .. } => {
            dependencies.push(DomainSelector {
                record: *record,
                key_set: KeySet::All,
                fields: vec![],
                purpose: Purpose::ValueRead,
            })
        }
        ScopeExpr::Global { records } => {
            for r in records {
                dependencies.push(DomainSelector {
                    record: *r,
                    key_set: KeySet::All,
                    fields: vec![],
                    purpose: Purpose::Membership,
                });
            }
        }
    }
    for (rec, key, purpose) in ctx.lookups {
        dependencies.push(DomainSelector {
            record: rec,
            key_set: KeySet::Point {
                static_key: false,
                key,
            },
            fields: vec![],
            purpose,
        });
    }
    Ok(InvariantIR {
        id,
        name: inv.name.name.clone(),
        version: 1,
        scope,
        kind,
        predicate,
        dependencies,
        evaluator_version: 1,
    })
}

/// `var.field >= lit` → LowerBound; `var.field <= lit` → UpperBound.
fn infer_bound_kind(pred: &ExprIR, var: BindingId) -> Option<InvariantKind> {
    if let ExprNodeIR::Bin {
        op: BinOp::Cmp(c),
        lhs,
        rhs,
    } = &pred.node
    {
        let is_var_field = |e: &ExprIR| matches!(&e.node, ExprNodeIR::Field { base, .. } if matches!(base.node, ExprNodeIR::Binding(b) if b == var));
        let is_lit = |e: &ExprIR| matches!(e.node, ExprNodeIR::Lit(_));
        if is_var_field(lhs) && is_lit(rhs) {
            return match c {
                CmpOp::Ge | CmpOp::Gt => Some(InvariantKind::LowerBound),
                CmpOp::Le | CmpOp::Lt => Some(InvariantKind::UpperBound),
                _ => None,
            };
        }
        if is_lit(lhs) && is_var_field(rhs) {
            return match c {
                CmpOp::Le | CmpOp::Lt => Some(InvariantKind::LowerBound),
                CmpOp::Ge | CmpOp::Gt => Some(InvariantKind::UpperBound),
                _ => None,
            };
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

fn lower_operation(op: &ast::OperationDecl, sig: &OpSig, env: &Env) -> CoreResult<OperationIR> {
    let mut ctx = Ctx::new(env, StateRef::Pre);
    for (i, (name, ty)) in sig.params.iter().enumerate() {
        ctx.scope.push(name, Sym::Param(i as u32, ty.clone()));
    }
    let parameters: Vec<ParameterIR> = sig
        .params
        .iter()
        .map(|(n, t)| ParameterIR {
            name: n.clone(),
            ty: t.clone(),
        })
        .collect();

    // REQUIRE reads pre-state
    let pre = lower_expr(&mut ctx, &op.require, Some(&Type::Bool))?;
    let pre_lookups = std::mem::take(&mut ctx.lookups);

    // READ bindings (pre-state snapshot)
    let mut reads = Vec::new();
    let mut read_selectors = Vec::new();
    for rd in &op.reads {
        if ctx.scope.lookup(&rd.binding.name).is_some() {
            return Err(err(
                ErrorCode::DuplicateIdentity,
                format!("binding `{}` shadows an existing name", rd.binding.name),
                rd.binding.span,
            ));
        }
        let b = ctx.fresh_binding();
        let (kind, ty) = match &rd.expr {
            ReadExpr::Row { record, key } => {
                let rec = env.record(record)?;
                let k = lower_expr(&mut ctx, key, Some(&rec.pk_type))?;
                read_selectors.push(selector(rec.id, &k, Purpose::ValueRead, &ctx));
                (
                    ReadKind::Row {
                        record: rec.id,
                        key: k,
                    },
                    rec.row_type(),
                )
            }
            ReadExpr::OptionalRow { record, key } => {
                let rec = env.record(record)?;
                let k = lower_expr(&mut ctx, key, Some(&rec.pk_type))?;
                read_selectors.push(selector(rec.id, &k, Purpose::ValueRead, &ctx));
                read_selectors.push(selector(rec.id, &k, Purpose::Absence, &ctx));
                (
                    ReadKind::OptionalRow {
                        record: rec.id,
                        key: k,
                    },
                    Type::Option(Box::new(rec.row_type())),
                )
            }
            ReadExpr::Exists { record, key } => {
                let rec = env.record(record)?;
                let k = lower_expr(&mut ctx, key, Some(&rec.pk_type))?;
                read_selectors.push(selector(rec.id, &k, Purpose::Absence, &ctx));
                (
                    ReadKind::Exists {
                        record: rec.id,
                        key: k,
                    },
                    Type::Bool,
                )
            }
            ReadExpr::Scan {
                var,
                record,
                filter,
            } => {
                let rec = env.record(record)?;
                let v = ctx.fresh_binding();
                ctx.scope.push(&var.name, Sym::Binding(v, rec.row_type()));
                let f = lower_expr(&mut ctx, filter, Some(&Type::Bool))?;
                ctx.scope.syms.pop();
                read_selectors.push(DomainSelector {
                    record: rec.id,
                    key_set: KeySet::All,
                    fields: vec![],
                    purpose: Purpose::Membership,
                });
                (
                    ReadKind::Scan {
                        record: rec.id,
                        var: v,
                        filter: f,
                    },
                    Type::Set(Box::new(rec.row_type())),
                )
            }
        };
        ctx.scope
            .push(&rd.binding.name, Sym::Binding(b, ty.clone()));
        reads.push(ReadBindingIR {
            binding: b,
            name: rd.binding.name.clone(),
            kind,
            ty,
        });
    }
    let read_lookups = std::mem::take(&mut ctx.lookups);

    // EFFECTS operate on the candidate state
    ctx.state = StateRef::Candidate;
    let mut effects = Vec::new();
    let mut writes = Vec::new();
    let mut predicates = Vec::new();
    let mut facts = Vec::new();
    for (i, ef) in op.effects.iter().enumerate() {
        ctx.bindings_used.clear();
        let guard = ef
            .guard
            .as_ref()
            .map(|g| lower_expr(&mut ctx, g, Some(&Type::Bool)))
            .transpose()?;
        let kind = lower_effect(&mut ctx, ef, &mut writes, &mut predicates, &mut facts)?;
        let read_dependencies: Vec<BindingId> = ctx.bindings_used.iter().copied().collect();
        effects.push(EffectIR {
            effect_id: i as u32,
            guard,
            kind,
            read_dependencies,
        });
    }
    let effect_lookups = std::mem::take(&mut ctx.lookups);

    let post = lower_expr(&mut ctx, &op.ensure, Some(&Type::Bool))?;
    let post_lookups = std::mem::take(&mut ctx.lookups);
    let result = lower_expr(&mut ctx, &op.result, None)?;
    let result_lookups = std::mem::take(&mut ctx.lookups);
    if !result.ty.is_result_type() {
        // returning whole rows/sets would leak unbounded state through a receipt; require explicit struct
        return Err(err(
            ErrorCode::InvalidContract,
            "RETURN must contain only scalars, tuples, structs, options or Unit; project rows and aggregate sets explicitly",
            op.result.span,
        ));
    }

    let contract = lower_contract(&op.contract, sig, env)?;

    // ---- footprint ------------------------------------------------------
    let mut fp_reads = read_selectors;
    for (rec, key, purpose) in pre_lookups
        .into_iter()
        .chain(read_lookups)
        .chain(effect_lookups)
        .chain(post_lookups)
    {
        fp_reads.push(DomainSelector {
            record: rec,
            key_set: point_key(&key),
            fields: vec![],
            purpose,
        });
    }
    for (rec, key, _) in result_lookups {
        fp_reads.push(DomainSelector {
            record: rec,
            key_set: point_key(&key),
            fields: vec![],
            purpose: Purpose::ResultRead,
        });
    }
    dedup_selectors(&mut fp_reads);
    dedup_selectors(&mut writes);
    dedup_selectors(&mut predicates);
    let atomic_groups = if effects.is_empty() {
        vec![]
    } else {
        vec![(0..effects.len() as u32).collect()]
    };
    let footprint = OperationFootprint {
        reads: fp_reads,
        writes,
        predicates,
        facts,
        atomic_groups,
    };

    Ok(OperationIR {
        identity: OperationRef {
            operation_id: sig.id,
            version: op.version,
        },
        name: op.name.name.clone(),
        parameters,
        pre,
        reads,
        effects,
        post,
        result,
        contract,
        footprint,
    })
}

fn dedup_selectors(v: &mut Vec<DomainSelector>) {
    let mut seen: Vec<DomainSelector> = Vec::new();
    for s in v.drain(..) {
        if !seen.contains(&s) {
            seen.push(s);
        }
    }
    *v = seen;
}

fn is_static(e: &ExprIR) -> bool {
    match &e.node {
        ExprNodeIR::Lit(_) | ExprNodeIR::Param(_) => true,
        ExprNodeIR::Tuple(items) => items.iter().all(is_static),
        _ => false,
    }
}

fn point_key(key: &ExprIR) -> KeySet {
    KeySet::Point {
        static_key: is_static(key),
        key: key.clone(),
    }
}

fn selector(record: RecordId, key: &ExprIR, purpose: Purpose, _ctx: &Ctx) -> DomainSelector {
    DomainSelector {
        record,
        key_set: point_key(key),
        fields: vec![],
        purpose,
    }
}

fn lower_path(ctx: &mut Ctx, p: &ast::PathExpr) -> CoreResult<(PathIR, Type)> {
    let rec = ctx.env.record(&p.record)?;
    let key = lower_expr(ctx, &p.key, Some(&rec.pk_type.clone()))?;
    let rec = ctx.env.record(&p.record)?;
    let (fid, fty) = rec.field(&p.field.name).ok_or_else(|| {
        err(
            ErrorCode::MissingRecord,
            format!("unknown field `{}`", p.field.name),
            p.field.span,
        )
    })?;
    if fid == rec.pk {
        return Err(err(
            ErrorCode::UnsupportedEffect,
            "primary key fields cannot be mutated",
            p.field.span,
        ));
    }
    if rec.immutable {
        return Err(err(
            ErrorCode::UnsupportedEffect,
            "immutable fact records cannot be mutated",
            p.field.span,
        ));
    }
    Ok((
        PathIR {
            record: rec.id,
            key,
            field: fid,
        },
        fty.clone(),
    ))
}

fn lower_effect(
    ctx: &mut Ctx,
    ef: &ast::Effect,
    writes: &mut Vec<DomainSelector>,
    predicates: &mut Vec<DomainSelector>,
    facts: &mut Vec<FactSelector>,
) -> CoreResult<EffectKind> {
    use ast::EffectKindAst as A;
    let write = |writes: &mut Vec<DomainSelector>, path: &PathIR| {
        writes.push(DomainSelector {
            record: path.record,
            key_set: point_key(&path.key),
            fields: vec![path.field],
            purpose: Purpose::Write,
        });
    };
    Ok(match &ef.kind {
        A::Assign { path, value } => {
            let (p, ty) = lower_path(ctx, path)?;
            let v = lower_expr(ctx, value, Some(&ty))?;
            write(writes, &p);
            EffectKind::Assign { path: p, value: v }
        }
        A::Increment { path, amount } | A::Decrement { path, amount } => {
            let (p, ty) = lower_path(ctx, path)?;
            if !ty.is_numeric() {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "INCREMENT/DECREMENT target must be numeric",
                    ef.span,
                ));
            }
            let a = lower_expr(ctx, amount, Some(&ty))?;
            write(writes, &p);
            if matches!(ef.kind, A::Increment { .. }) {
                EffectKind::Increment { path: p, amount: a }
            } else {
                EffectKind::Decrement { path: p, amount: a }
            }
        }
        A::Insert { record, fields } | A::Emit { record, fields } => {
            let is_emit = matches!(ef.kind, A::Emit { .. });
            let rec = ctx.env.record(record)?.clone();
            if is_emit && !rec.immutable {
                return Err(err(
                    ErrorCode::UnsupportedEffect,
                    "EMIT target must be an IMMUTABLE record",
                    ef.span,
                ));
            }
            if !is_emit && rec.immutable {
                return Err(err(
                    ErrorCode::UnsupportedEffect,
                    "IMMUTABLE records accept only EMIT",
                    ef.span,
                ));
            }
            let mut inits = Vec::new();
            let mut key: Option<ExprIR> = None;
            let mut seen = BTreeSet::new();
            for (fname, fexpr) in fields {
                let (fid, fty) = rec.field(&fname.name).ok_or_else(|| {
                    err(
                        ErrorCode::MissingRecord,
                        format!("unknown field `{}`", fname.name),
                        fname.span,
                    )
                })?;
                if !seen.insert(fid) {
                    return Err(err(
                        ErrorCode::DuplicateIdentity,
                        "duplicate field initializer",
                        fname.span,
                    ));
                }
                let e = lower_expr(ctx, fexpr, Some(&fty.clone()))?;
                if fid == rec.pk {
                    key = Some(e.clone());
                }
                inits.push((fid, e));
            }
            for (fname, fid, _) in &rec.fields {
                if !seen.contains(fid) {
                    return Err(err(
                        ErrorCode::InvalidIr,
                        format!("missing field `{fname}` in insert"),
                        ef.span,
                    ));
                }
            }
            let key = key.expect("pk present");
            inits.sort_by_key(|(f, _)| *f);
            writes.push(DomainSelector {
                record: rec.id,
                key_set: point_key(&key),
                fields: vec![],
                purpose: Purpose::Write,
            });
            predicates.push(DomainSelector {
                record: rec.id,
                key_set: point_key(&key),
                fields: vec![],
                purpose: Purpose::Absence,
            });
            if is_emit {
                facts.push(FactSelector {
                    record: rec.id,
                    key: key.clone(),
                });
                EffectKind::EmitFact {
                    fact: rec.id,
                    key,
                    fields: inits,
                }
            } else {
                EffectKind::Insert {
                    record: rec.id,
                    key,
                    fields: inits,
                }
            }
        }
        A::Delete { record, key } => {
            let rec = ctx.env.record(record)?.clone();
            if rec.immutable {
                return Err(err(
                    ErrorCode::UnsupportedEffect,
                    "immutable facts cannot be deleted",
                    ef.span,
                ));
            }
            let k = lower_expr(ctx, key, Some(&rec.pk_type))?;
            writes.push(DomainSelector {
                record: rec.id,
                key_set: point_key(&k),
                fields: vec![],
                purpose: Purpose::Write,
            });
            predicates.push(DomainSelector {
                record: rec.id,
                key_set: point_key(&k),
                fields: vec![],
                purpose: Purpose::Membership,
            });
            EffectKind::Delete {
                record: rec.id,
                key: k,
            }
        }
        A::AddToSet { path, value } | A::RemoveFromSet { path, value } => {
            let (p, ty) = lower_path(ctx, path)?;
            let elem = match &ty {
                Type::Set(e) => e.as_ref().clone(),
                _ => {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "ADD/REMOVE target must be a Set",
                        ef.span,
                    ))
                }
            };
            let v = lower_expr(ctx, value, Some(&elem))?;
            write(writes, &p);
            if matches!(ef.kind, A::AddToSet { .. }) {
                EffectKind::AddToSet { path: p, value: v }
            } else {
                EffectKind::RemoveFromSet { path: p, value: v }
            }
        }
        A::CompareAndSwap {
            path,
            expected,
            value,
        } => {
            let (p, ty) = lower_path(ctx, path)?;
            let e = lower_expr(ctx, expected, Some(&ty))?;
            let v = lower_expr(ctx, value, Some(&ty))?;
            write(writes, &p);
            EffectKind::CompareAndSwap {
                path: p,
                expected: e,
                value: v,
            }
        }
        A::Reserve {
            resource,
            key,
            amount,
            reservation_id,
        }
        | A::Release {
            resource,
            key,
            amount,
            reservation_id,
        } => {
            let rs = ctx
                .env
                .resources
                .get(&resource.name)
                .ok_or_else(|| {
                    err(
                        ErrorCode::MissingRecord,
                        format!("unknown resource `{}`", resource.name),
                        resource.span,
                    )
                })?
                .clone();
            let rec = ctx.env.record_by_id(rs.record).clone();
            let resv = ctx.env.record_by_id(rs.reservation_record).clone();
            let k = lower_expr(ctx, key, Some(&rec.pk_type))?;
            let (_, qty) = rec.field_by_id(rs.available).expect("available");
            let a = lower_expr(ctx, amount, Some(&qty.clone()))?;
            let rid = lower_expr(ctx, reservation_id, Some(&resv.pk_type))?;
            writes.push(DomainSelector {
                record: rec.id,
                key_set: point_key(&k),
                fields: vec![rs.available, rs.reserved],
                purpose: Purpose::Write,
            });
            writes.push(DomainSelector {
                record: resv.id,
                key_set: point_key(&rid),
                fields: vec![],
                purpose: Purpose::Write,
            });
            if matches!(ef.kind, A::Reserve { .. }) {
                predicates.push(DomainSelector {
                    record: resv.id,
                    key_set: point_key(&rid),
                    fields: vec![],
                    purpose: Purpose::Absence,
                });
                EffectKind::Reserve {
                    resource: rs.id,
                    key: k,
                    reservation_id: rid,
                    amount: a,
                }
            } else {
                predicates.push(DomainSelector {
                    record: resv.id,
                    key_set: point_key(&rid),
                    fields: vec![],
                    purpose: Purpose::Membership,
                });
                EffectKind::Release {
                    resource: rs.id,
                    key: k,
                    reservation_id: rid,
                    amount: a,
                }
            }
        }
        A::Transfer {
            amount,
            source,
            destination,
        } => {
            let (s, sty) = lower_path(ctx, source)?;
            let (d, dty) = lower_path(ctx, destination)?;
            if sty != dty || !sty.is_numeric() {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "TRANSFER endpoints must share one numeric type",
                    ef.span,
                ));
            }
            let a = lower_expr(ctx, amount, Some(&sty))?;
            write(writes, &s);
            write(writes, &d);
            EffectKind::TransferQuantity {
                source: s,
                destination: d,
                amount: a,
            }
        }
        A::AdvanceState {
            path,
            expected,
            next,
        } => {
            let (p, ty) = lower_path(ctx, path)?;
            let enum_id = match ty {
                Type::Enum(e) => e,
                _ => {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "ADVANCE target must be an enum field",
                        ef.span,
                    ))
                }
            };
            let info = ctx.env.enum_by_id(enum_id);
            let ei = info
                .variants
                .iter()
                .position(|v| *v == expected.name)
                .ok_or_else(|| {
                    err(
                        ErrorCode::TypeMismatch,
                        format!("unknown variant `{}`", expected.name),
                        expected.span,
                    )
                })?;
            let ni = info
                .variants
                .iter()
                .position(|v| *v == next.name)
                .ok_or_else(|| {
                    err(
                        ErrorCode::TypeMismatch,
                        format!("unknown variant `{}`", next.name),
                        next.span,
                    )
                })?;
            write(writes, &p);
            EffectKind::AdvanceState {
                path: p,
                enum_id,
                expected: ei as u32,
                next: ni as u32,
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

fn ctor<'a>(v: &'a ContractValue, field: &str) -> CoreResult<(&'a str, &'a [ContractValue])> {
    match v {
        ContractValue::Ctor { name, args } => Ok((name.name.as_str(), args.as_slice())),
        _ => Err(CoreError::new(
            ErrorCode::InvalidContract,
            format!("contract field `{field}` expects a constructor"),
        )),
    }
}

fn lower_contract(c: &ast::ContractDecl, sig: &OpSig, env: &Env) -> CoreResult<ContractIR> {
    let mut map: BTreeMap<&str, &ContractValue> = BTreeMap::new();
    for (k, v) in &c.fields {
        if map.insert(k.name.as_str(), v).is_some() {
            return Err(err(
                ErrorCode::InvalidContract,
                format!("duplicate contract field `{}`", k.name),
                k.span,
            ));
        }
    }
    let known = [
        "atomicity",
        "input_visibility",
        "result_semantics",
        "result_scope",
        "session",
        "session_scope",
        "durability",
        "partition_outcomes",
        "authority_requirements",
        "refusal_semantics",
        "commitment",
        "request_namespace",
    ];
    for k in map.keys() {
        if !known.contains(k) {
            return Err(err(
                ErrorCode::InvalidContract,
                format!("unknown contract field `{k}`"),
                c.span,
            ));
        }
    }
    let get = |name: &str| -> CoreResult<&ContractValue> {
        map.get(name).copied().ok_or_else(|| {
            err(
                ErrorCode::InvalidContract,
                format!("missing contract field `{name}`"),
                c.span,
            )
        })
    };
    let inv = |name: &str, msg: &str| {
        err(
            ErrorCode::InvalidContract,
            format!("contract field `{name}`: {msg}"),
            c.span,
        )
    };

    if ctor(get("atomicity")?, "atomicity")?.0 != "WholeInvocation" {
        return Err(inv("atomicity", "only WholeInvocation is supported"));
    }
    if ctor(get("commitment")?, "commitment")?.0 != "FinalWhenDurable" {
        return Err(inv("commitment", "only FinalWhenDurable is supported"));
    }
    let input_visibility = match ctor(get("input_visibility")?, "input_visibility")?.0 {
        "LocalSnapshot" => InputVisibility::LocalSnapshot,
        "CausalContext" => InputVisibility::CausalContext,
        "CertifiedScope" => InputVisibility::CertifiedScope,
        "SerialScope" => InputVisibility::SerialScope,
        _ => return Err(inv("input_visibility", "unknown value")),
    };
    let result_semantics = match ctor(get("result_semantics")?, "result_semantics")?.0 {
        "Receipt" => ResultSemantics::Receipt,
        "SnapshotValue" => ResultSemantics::SnapshotValue,
        "ExactOrderedValue" => ResultSemantics::ExactOrderedValue,
        _ => return Err(inv("result_semantics", "unknown value")),
    };
    let key_ref = |v: &ContractValue| -> CoreResult<(RecordId, u32)> {
        match v {
            ContractValue::KeyRef { record, param } => {
                let rec = env.record(record)?;
                let idx = sig
                    .params
                    .iter()
                    .position(|(n, _)| *n == param.name)
                    .ok_or_else(|| {
                        err(
                            ErrorCode::InvalidContract,
                            format!("unknown parameter `{}` in scope", param.name),
                            param.span,
                        )
                    })?;
                if sig.params[idx].1 != rec.pk_type {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "scope parameter type must match the record key type",
                        param.span,
                    ));
                }
                Ok((rec.id, idx as u32))
            }
            _ => Err(inv("result_scope", "expected Record[param]")),
        }
    };
    let result_scope = {
        let (name, args) = ctor(get("result_scope")?, "result_scope")?;
        match name {
            "PerKey" => {
                if args.len() != 1 {
                    return Err(inv("result_scope", "PerKey takes one Record[param]"));
                }
                let (record, param) = key_ref(&args[0])?;
                ResultScope::PerKey { record, param }
            }
            "PerGroup" => {
                if args.len() != 1 {
                    return Err(inv("result_scope", "PerGroup takes one Record[param]"));
                }
                let (record, param) = key_ref(&args[0])?;
                ResultScope::PerGroup { record, param }
            }
            "Global" => {
                let mut records = Vec::new();
                for a in args {
                    let (n, _) = ctor(a, "result_scope")?;
                    records.push(
                        env.records
                            .get(n)
                            .ok_or_else(|| inv("result_scope", "unknown record"))?
                            .id,
                    );
                }
                records.sort();
                records.dedup();
                ResultScope::Global { records }
            }
            _ => return Err(inv("result_scope", "expected PerKey/PerGroup/Global")),
        }
    };
    let session = match get("session")? {
        ContractValue::Set(items) => {
            let mut s = Vec::new();
            for it in items {
                let g = match ctor(it, "session")?.0 {
                    "ReadYourWrites" => SessionGuarantee::ReadYourWrites,
                    "MonotonicReads" => SessionGuarantee::MonotonicReads,
                    "CausalDependencies" => SessionGuarantee::CausalDependencies,
                    _ => return Err(inv("session", "unknown guarantee")),
                };
                if s.contains(&g) {
                    return Err(inv("session", "duplicate guarantee"));
                }
                s.push(g);
            }
            s.sort();
            s
        }
        _ => return Err(inv("session", "expected a set")),
    };
    let session_scope = {
        let (name, args) = ctor(get("session_scope")?, "session_scope")?;
        match name {
            "None" => SessionScope::None,
            "ReplicationGroup" => match args {
                [ContractValue::Str(g)] => SessionScope::ReplicationGroup(g.clone()),
                [ContractValue::Ctor { name, args }] if args.is_empty() => {
                    SessionScope::ReplicationGroup(name.name.clone())
                }
                _ => {
                    return Err(inv(
                        "session_scope",
                        "ReplicationGroup takes one group name",
                    ))
                }
            },
            "CompositeScope" => {
                let mut records = Vec::new();
                for a in args {
                    let (n, _) = ctor(a, "session_scope")?;
                    records.push(
                        env.records
                            .get(n)
                            .ok_or_else(|| inv("session_scope", "unknown record"))?
                            .id,
                    );
                }
                SessionScope::CompositeScope(records)
            }
            _ => {
                return Err(inv(
                    "session_scope",
                    "expected None/ReplicationGroup/CompositeScope",
                ))
            }
        }
    };
    if !session.is_empty() && session_scope == SessionScope::None {
        return Err(inv(
            "session_scope",
            "nonempty session guarantees require an explicit scope (SPEC-003 §9)",
        ));
    }
    let durability = {
        let (name, args) = ctor(get("durability")?, "durability")?;
        match name {
            "LocalStable" => Durability::LocalStable,
            "ReplicatedStable" => match args {
                [ContractValue::Str(p)] => Durability::ReplicatedStable(p.clone()),
                [ContractValue::Ctor { name, args }] if args.is_empty() => {
                    Durability::ReplicatedStable(name.name.clone())
                }
                _ => {
                    return Err(inv(
                        "durability",
                        "ReplicatedStable requires a named failure-domain policy",
                    ))
                }
            },
            _ => {
                return Err(inv(
                    "durability",
                    "expected LocalStable or ReplicatedStable(policy)",
                ))
            }
        }
    };
    let partition_outcomes = match get("partition_outcomes")? {
        ContractValue::Set(items) => {
            let mut s = Vec::new();
            for it in items {
                let p = match ctor(it, "partition_outcomes")?.0 {
                    "Wait" => PartitionOutcome::Wait,
                    "Unavailable" => PartitionOutcome::Unavailable,
                    "AuthorityUnavailable" => PartitionOutcome::AuthorityUnavailable,
                    _ => return Err(inv("partition_outcomes", "unknown outcome")),
                };
                if s.contains(&p) {
                    return Err(inv("partition_outcomes", "duplicate outcome"));
                }
                s.push(p);
            }
            if s.is_empty() {
                return Err(inv(
                    "partition_outcomes",
                    "at least one outcome is required",
                ));
            }
            s.sort();
            s
        }
        _ => return Err(inv("partition_outcomes", "expected a set")),
    };
    let authority_requirements = match map.get("authority_requirements") {
        None => vec![],
        Some(ContractValue::List(items)) | Some(ContractValue::Set(items)) => items
            .iter()
            .map(|i| match i {
                ContractValue::Str(s) => Ok(s.clone()),
                ContractValue::Ctor { name, args } if args.is_empty() => Ok(name.name.clone()),
                _ => Err(inv("authority_requirements", "expected names")),
            })
            .collect::<CoreResult<_>>()?,
        Some(_) => return Err(inv("authority_requirements", "expected a list")),
    };
    let refusal_semantics = match ctor(get("refusal_semantics")?, "refusal_semantics")?.0 {
        "BusinessPredicate" => RefusalSemantics::BusinessPredicate,
        "MissingAuthority" => RefusalSemantics::MissingAuthority,
        "MissingDependency" => RefusalSemantics::MissingDependency,
        _ => return Err(inv("refusal_semantics", "unknown value")),
    };
    let request_namespace = match get("request_namespace")? {
        ContractValue::Str(s) if !s.is_empty() => s.clone(),
        _ => return Err(inv("request_namespace", "expected a non-empty string")),
    };
    Ok(ContractIR {
        input_visibility,
        result_semantics,
        result_scope,
        session,
        session_scope,
        durability,
        partition_outcomes,
        authority_requirements,
        refusal_semantics,
        request_namespace,
    })
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

fn lower_expr(ctx: &mut Ctx, e: &Expr, expected: Option<&Type>) -> CoreResult<ExprIR> {
    ctx.depth += 1;
    if ctx.depth > 128 {
        return Err(err(
            ErrorCode::ResourceLimit,
            "expression nesting too deep",
            e.span,
        ));
    }
    let r = lower_expr_inner(ctx, e, expected);
    ctx.depth -= 1;
    let r = r?;
    if let Some(exp) = expected {
        if &r.ty != exp {
            return Err(err(
                ErrorCode::TypeMismatch,
                format!("expected {}, found {}", exp.describe(), r.ty.describe()),
                e.span,
            ));
        }
    }
    Ok(r)
}

fn lit(ty: Type, v: Value) -> ExprIR {
    ExprIR {
        ty,
        node: ExprNodeIR::Lit(v),
    }
}

fn lower_expr_inner(ctx: &mut Ctx, e: &Expr, expected: Option<&Type>) -> CoreResult<ExprIR> {
    let span = e.span;
    Ok(match &e.node {
        ExprNode::IntLit(v) => match expected {
            Some(Type::U64) => {
                let u = u64::try_from(*v).map_err(|_| {
                    err(ErrorCode::NumericOverflow, "literal out of U64 range", span)
                })?;
                lit(Type::U64, Value::U64(u))
            }
            Some(Type::Decimal(p, s)) => {
                let d = Decimal::parse(&v.to_string(), *p, *s)
                    .map_err(|e| err(e.code, e.message, span))?;
                lit(Type::Decimal(*p, *s), Value::Decimal(d))
            }
            _ => {
                let i = i64::try_from(*v).map_err(|_| {
                    err(ErrorCode::NumericOverflow, "literal out of I64 range", span)
                })?;
                lit(Type::I64, Value::I64(i))
            }
        },
        ExprNode::DecimalLit(text) => match expected {
            Some(Type::Decimal(p, s)) => {
                let d = Decimal::parse(text, *p, *s).map_err(|e| err(e.code, e.message, span))?;
                lit(Type::Decimal(*p, *s), Value::Decimal(d))
            }
            _ => {
                return Err(err(
                    ErrorCode::InvalidDecimal,
                    "decimal literal requires a Decimal(p,s) context",
                    span,
                ))
            }
        },
        ExprNode::StrLit(s) => {
            let ty = match expected {
                Some(Type::String(n)) => {
                    if s.len() > *n as usize {
                        return Err(err(
                            ErrorCode::TypeMismatch,
                            "string literal exceeds declared length",
                            span,
                        ));
                    }
                    Type::String(*n)
                }
                _ => Type::String(s.len() as u32),
            };
            lit(ty, Value::Str(s.clone()))
        }
        ExprNode::BoolLit(b) => lit(Type::Bool, Value::Bool(*b)),
        ExprNode::UuidLit(u) => lit(Type::Uuid, Value::Uuid(*u)),
        ExprNode::BytesLit(b) => {
            let ty = match expected {
                Some(Type::Bytes(n)) => {
                    if b.len() > *n as usize {
                        return Err(err(
                            ErrorCode::TypeMismatch,
                            "bytes literal exceeds declared length",
                            span,
                        ));
                    }
                    Type::Bytes(*n)
                }
                _ => Type::Bytes(b.len() as u32),
            };
            lit(ty, Value::Bytes(b.clone()))
        }
        ExprNode::Unit => lit(Type::Unit, Value::Unit),
        ExprNode::None => match expected {
            Some(t @ Type::Option(_)) => lit(t.clone(), Value::None),
            _ => {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "None requires an Option context",
                    span,
                ))
            }
        },
        ExprNode::Some(inner) => {
            let inner_expected = match expected {
                Some(Type::Option(t)) => Some(t.as_ref().clone()),
                Some(_) => {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "Some(...) where a non-Option is expected",
                        span,
                    ))
                }
                None => None,
            };
            let ir = lower_expr(ctx, inner, inner_expected.as_ref())?;
            let ty = Type::Option(Box::new(ir.ty.clone()));
            match ir.node {
                ExprNodeIR::Lit(v) => lit(ty, Value::Some(Box::new(v))),
                _ => ExprIR {
                    ty,
                    node: ExprNodeIR::SomeOf(Box::new(ir)),
                },
            }
        }
        ExprNode::Var(id) => match ctx.scope.lookup(&id.name).cloned() {
            Some(Sym::Param(i, t)) => ExprIR {
                ty: t,
                node: ExprNodeIR::Param(i),
            },
            Some(Sym::Binding(b, t)) => {
                ctx.bindings_used.insert(b);
                ExprIR {
                    ty: t,
                    node: ExprNodeIR::Binding(b),
                }
            }
            Some(Sym::GroupKey(t)) => ExprIR {
                ty: t,
                node: ExprNodeIR::GroupKey,
            },
            None => {
                return Err(err(
                    ErrorCode::MissingRecord,
                    format!("unresolved name `{}`", id.name),
                    id.span,
                ))
            }
        },
        ExprNode::EnumVariant { enum_name, variant } => {
            let info = ctx.env.enums.get(&enum_name.name).ok_or_else(|| {
                err(
                    ErrorCode::TypeMismatch,
                    format!("unknown enum `{}`", enum_name.name),
                    enum_name.span,
                )
            })?;
            let idx = info
                .variants
                .iter()
                .position(|v| *v == variant.name)
                .ok_or_else(|| {
                    err(
                        ErrorCode::TypeMismatch,
                        format!("unknown variant `{}`", variant.name),
                        variant.span,
                    )
                })?;
            lit(
                Type::Enum(info.id),
                Value::Enum {
                    enum_id: info.id,
                    variant: idx as u32,
                },
            )
        }
        ExprNode::Field { base, field } => {
            let b = lower_expr(ctx, base, None)?;
            match &b.ty {
                Type::Row(rid) => {
                    let rec = ctx.env.record_by_id(*rid);
                    let (fid, fty) = rec.field(&field.name).ok_or_else(|| {
                        err(
                            ErrorCode::MissingRecord,
                            format!("unknown field `{}`", field.name),
                            field.span,
                        )
                    })?;
                    ExprIR {
                        ty: fty.clone(),
                        node: ExprNodeIR::Field {
                            base: Box::new(b),
                            field: fid,
                            name: field.name.clone(),
                        },
                    }
                }
                Type::Struct(fs) => {
                    let (_, t) = fs.iter().find(|(n, _)| *n == field.name).ok_or_else(|| {
                        err(
                            ErrorCode::MissingRecord,
                            format!("unknown struct field `{}`", field.name),
                            field.span,
                        )
                    })?;
                    ExprIR {
                        ty: t.clone(),
                        node: ExprNodeIR::StructField {
                            base: Box::new(b),
                            name: field.name.clone(),
                        },
                    }
                }
                other => {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        format!("field access on non-row type {}", other.describe()),
                        span,
                    ))
                }
            }
        }
        ExprNode::RowLookup { record, key } => {
            let rec = ctx.env.record(record)?.clone();
            let k = lower_expr(ctx, key, Some(&rec.pk_type))?;
            ctx.lookups.push((rec.id, k.clone(), Purpose::ValueRead));
            ExprIR {
                ty: rec.row_type(),
                node: ExprNodeIR::RowLookup {
                    record: rec.id,
                    key: Box::new(k),
                    state: ctx.state,
                },
            }
        }
        ExprNode::Exists { record, key } => {
            let rec = ctx.env.record(record)?.clone();
            let k = lower_expr(ctx, key, Some(&rec.pk_type))?;
            ctx.lookups.push((rec.id, k.clone(), Purpose::Absence));
            ExprIR {
                ty: Type::Bool,
                node: ExprNodeIR::Exists {
                    record: rec.id,
                    key: Box::new(k),
                    state: ctx.state,
                },
            }
        }
        ExprNode::Tuple(items) => {
            let exp_items: Option<Vec<Type>> = match expected {
                Some(Type::Tuple(ts)) if ts.len() == items.len() => Some(ts.clone()),
                _ => None,
            };
            let mut irs = Vec::new();
            for (i, it) in items.iter().enumerate() {
                irs.push(lower_expr(ctx, it, exp_items.as_ref().map(|t| &t[i]))?);
            }
            let ty = Type::Tuple(irs.iter().map(|i| i.ty.clone()).collect());
            if irs.iter().all(|i| matches!(i.node, ExprNodeIR::Lit(_))) {
                let vals = irs.into_iter().map(|i| match i.node {
                    ExprNodeIR::Lit(v) => v,
                    _ => unreachable!(),
                });
                lit(ty, Value::Tuple(vals.collect()))
            } else {
                ExprIR {
                    ty,
                    node: ExprNodeIR::Tuple(irs),
                }
            }
        }
        ExprNode::Struct(fields) => {
            let mut irs: Vec<(String, ExprIR)> = Vec::new();
            for (name, fe) in fields {
                if irs.iter().any(|(n, _)| *n == name.name) {
                    return Err(err(
                        ErrorCode::DuplicateIdentity,
                        "duplicate struct field",
                        name.span,
                    ));
                }
                let exp_field = match expected {
                    Some(Type::Struct(fs)) => fs
                        .iter()
                        .find(|(n, _)| *n == name.name)
                        .map(|(_, t)| t.clone()),
                    _ => None,
                };
                irs.push((name.name.clone(), lower_expr(ctx, fe, exp_field.as_ref())?));
            }
            irs.sort_by(|a, b| a.0.cmp(&b.0));
            let ty = Type::Struct(irs.iter().map(|(n, e)| (n.clone(), e.ty.clone())).collect());
            ExprIR {
                ty,
                node: ExprNodeIR::Struct(irs),
            }
        }
        ExprNode::SetLit(items) => {
            let elem_expected = match expected {
                Some(Type::Set(t)) => Some(t.as_ref().clone()),
                _ => None,
            };
            let mut irs = Vec::new();
            let mut elem_ty = elem_expected.clone();
            for it in items {
                let ir = lower_expr(ctx, it, elem_ty.as_ref())?;
                if elem_ty.is_none() {
                    elem_ty = Some(ir.ty.clone());
                }
                irs.push(ir);
            }
            let elem_ty = elem_ty.ok_or_else(|| {
                err(
                    ErrorCode::TypeMismatch,
                    "empty set literal requires a Set context",
                    span,
                )
            })?;
            if !elem_ty.is_comparable() {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "set elements must be comparable",
                    span,
                ));
            }
            ExprIR {
                ty: Type::Set(Box::new(elem_ty)),
                node: ExprNodeIR::SetLit(irs),
            }
        }
        ExprNode::Neg(inner) => {
            let ir = lower_expr(ctx, inner, expected)?;
            match ir.ty {
                Type::I64 | Type::Decimal(_, _) => {}
                _ => {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "negation requires I64 or Decimal",
                        span,
                    ))
                }
            }
            let ty = ir.ty.clone();
            ExprIR {
                ty,
                node: ExprNodeIR::Neg(Box::new(ir)),
            }
        }
        ExprNode::Not(inner) => {
            let ir = lower_expr(ctx, inner, Some(&Type::Bool))?;
            ExprIR {
                ty: Type::Bool,
                node: ExprNodeIR::Not(Box::new(ir)),
            }
        }
        ExprNode::Bin { op, lhs, rhs } => lower_bin(ctx, *op, lhs, rhs, expected, span)?,
        ExprNode::IsNone(inner) | ExprNode::IsSome(inner) => {
            let ir = lower_expr(ctx, inner, None)?;
            if !matches!(ir.ty, Type::Option(_)) {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "IS NONE/SOME requires an Option",
                    span,
                ));
            }
            if matches!(e.node, ExprNode::IsNone(_)) {
                ExprIR {
                    ty: Type::Bool,
                    node: ExprNodeIR::IsNone(Box::new(ir)),
                }
            } else {
                ExprIR {
                    ty: Type::Bool,
                    node: ExprNodeIR::IsSome(Box::new(ir)),
                }
            }
        }
        ExprNode::UnwrapOr { value, default } => {
            let v = lower_expr(ctx, value, None)?;
            let inner = match &v.ty {
                Type::Option(t) => t.as_ref().clone(),
                _ => {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "?? requires an Option on the left",
                        span,
                    ))
                }
            };
            let d = lower_expr(ctx, default, Some(&inner))?;
            ExprIR {
                ty: inner,
                node: ExprNodeIR::UnwrapOr {
                    value: Box::new(v),
                    default: Box::new(d),
                },
            }
        }
        ExprNode::Size(inner) => {
            let ir = lower_expr(ctx, inner, None)?;
            if !matches!(ir.ty, Type::Set(_)) {
                return Err(err(ErrorCode::TypeMismatch, "SIZE requires a Set", span));
            }
            ExprIR {
                ty: Type::U64,
                node: ExprNodeIR::Size(Box::new(ir)),
            }
        }
        ExprNode::SumOver { var, set, value } => {
            let s = lower_expr(ctx, set, None)?;
            let elem = match &s.ty {
                Type::Set(t) => t.as_ref().clone(),
                _ => return Err(err(ErrorCode::TypeMismatch, "SUM requires a Set", span)),
            };
            let b = ctx.fresh_binding();
            ctx.scope.push(&var.name, Sym::Binding(b, elem));
            let v = lower_expr(ctx, value, expected.filter(|t| t.is_numeric()))?;
            ctx.scope.syms.pop();
            if !v.ty.is_numeric() {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "SUM value must be numeric",
                    span,
                ));
            }
            let ty = v.ty.clone();
            ExprIR {
                ty,
                node: ExprNodeIR::SumOver {
                    var: b,
                    set: Box::new(s),
                    value: Box::new(v),
                },
            }
        }
    })
}

fn lower_bin(
    ctx: &mut Ctx,
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
    expected: Option<&Type>,
    span: ast::Span,
) -> CoreResult<ExprIR> {
    match op {
        BinOp::And | BinOp::Or => {
            let l = lower_expr(ctx, lhs, Some(&Type::Bool))?;
            let r = lower_expr(ctx, rhs, Some(&Type::Bool))?;
            Ok(ExprIR {
                ty: Type::Bool,
                node: ExprNodeIR::Bin {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
            })
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul => {
            // numeric; literal on either side adopts the other side's type
            let (l, r) = lower_numeric_pair(ctx, lhs, rhs, expected)?;
            if !l.ty.is_numeric() {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "arithmetic requires numeric operands",
                    span,
                ));
            }
            if op == BinOp::Mul
                && !matches!(l.node, ExprNodeIR::Lit(_))
                && !matches!(r.node, ExprNodeIR::Lit(_))
            {
                return Err(err(ErrorCode::UnsupportedEffect, "general multiplication is outside the affine fragment; one operand must be a constant", span));
            }
            let ty = l.ty.clone();
            Ok(ExprIR {
                ty,
                node: ExprNodeIR::Bin {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
            })
        }
        BinOp::Cmp(c) => {
            let (l, r) = lower_numeric_pair(ctx, lhs, rhs, None)?;
            if !l.ty.is_comparable() {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    format!("type {} is not comparable", l.ty.describe()),
                    span,
                ));
            }
            if matches!(c, CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge)
                && matches!(l.ty, Type::Bool | Type::Enum(_) | Type::Option(_))
            {
                return Err(err(
                    ErrorCode::TypeMismatch,
                    "ordering comparison requires an ordered scalar",
                    span,
                ));
            }
            Ok(ExprIR {
                ty: Type::Bool,
                node: ExprNodeIR::Bin {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
            })
        }
        BinOp::In => {
            let r = lower_expr(ctx, rhs, None)?;
            let elem = match &r.ty {
                Type::Set(t) => t.as_ref().clone(),
                _ => {
                    return Err(err(
                        ErrorCode::TypeMismatch,
                        "IN requires a Set on the right",
                        span,
                    ))
                }
            };
            let l = lower_expr(ctx, lhs, Some(&elem))?;
            Ok(ExprIR {
                ty: Type::Bool,
                node: ExprNodeIR::Bin {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                },
            })
        }
    }
}

/// Lower two operands that must share one type. A literal adopts the other operand's type.
fn lower_numeric_pair(
    ctx: &mut Ctx,
    lhs: &Expr,
    rhs: &Expr,
    expected: Option<&Type>,
) -> CoreResult<(ExprIR, ExprIR)> {
    let is_literal = |e: &Expr| {
        matches!(
            e.node,
            ExprNode::IntLit(_) | ExprNode::DecimalLit(_) | ExprNode::StrLit(_) | ExprNode::None
        )
    };
    if is_literal(lhs) && !is_literal(rhs) {
        let r = lower_expr(ctx, rhs, expected)?;
        let l = lower_expr(ctx, lhs, Some(&r.ty))?;
        Ok((l, r))
    } else {
        let l = lower_expr(ctx, lhs, expected)?;
        let r = lower_expr(ctx, rhs, Some(&l.ty))?;
        Ok((l, r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_module;
    use carolina_core::canon::Canonical;
    use carolina_core::limits::Limits;

    const SRC: &str = r#"
RECORD Product { id: Uuid PRIMARY KEY, stock: I64 }
INVARIANT stock_nonneg { FORALL p IN Product : p.stock >= 0 }
OPERATION sell(item: Uuid, q: I64) VERSION 1 {
  REQUIRE q > 0
  READ { p = Product[item] }
  EFFECT { DECREMENT Product[item].stock BY q }
  ENSURE Product[item].stock >= 0
  RETURN { item: item, q: q }
  CONTRACT {
    atomicity: WholeInvocation, input_visibility: SerialScope, result_semantics: Receipt,
    result_scope: PerKey(Product[item]), session: {}, session_scope: None, durability: LocalStable,
    partition_outcomes: { Unavailable }, refusal_semantics: BusinessPredicate, commitment: FinalWhenDurable,
    request_namespace: "inventory"
  }
}
"#;

    #[test]
    fn lowers_and_roundtrips_canonically() {
        let m = parse_module(SRC, &Limits::v1()).unwrap();
        let (ir, alloc) = lower_module(&m, None).unwrap();
        assert_eq!(ir.records.len(), 1);
        assert_eq!(ir.invariants[0].kind, InvariantKind::LowerBound);
        assert!(matches!(ir.invariants[0].scope, ScopeExpr::PerKey { .. }));
        let op = &ir.operations[0];
        assert_eq!(op.footprint.writes.len(), 1);
        assert!(matches!(
            op.footprint.writes[0].key_set,
            KeySet::Point {
                static_key: true,
                ..
            }
        ));
        assert!(!op.footprint.reads.is_empty());
        // canonical roundtrip
        let bytes = ir.encode();
        let back = ModuleIR::decode(&bytes, &Limits::v1()).unwrap();
        assert_eq!(back, ir);
        // stable IDs preserved on recompile with the same allocation
        let (ir2, alloc2) = lower_module(&m, Some(&alloc)).unwrap();
        assert_eq!(ir2, ir);
        assert_eq!(alloc2, alloc);
        let h1 = ir.hashes();
        let h2 = ir2.hashes();
        assert_eq!(h1.module_hash, h2.module_hash);
    }

    #[test]
    fn formatting_does_not_change_hashes_but_semantics_do() {
        let m1 = parse_module(SRC, &Limits::v1()).unwrap();
        let m2 =
            parse_module(&SRC.replace("  ", " ").replace("\n\n", "\n"), &Limits::v1()).unwrap();
        let h1 = lower_module(&m1, None).unwrap().0.hashes();
        let h2 = lower_module(&m2, None).unwrap().0.hashes();
        assert_eq!(h1.module_hash, h2.module_hash);
        let m3 = parse_module(
            &SRC.replace(
                "result_semantics: Receipt",
                "result_semantics: ExactOrderedValue",
            ),
            &Limits::v1(),
        )
        .unwrap();
        let h3 = lower_module(&m3, None).unwrap().0.hashes();
        assert_eq!(h1.schema_hash, h3.schema_hash);
        assert_ne!(h1.module_hash, h3.module_hash);
        let op = OperationRef {
            operation_id: OperationId(1),
            version: 1,
        };
        assert_ne!(h1.contract_hashes[&op], h3.contract_hashes[&op]);
        assert_ne!(h1.operation_hashes[&op], h3.operation_hashes[&op]);
    }

    #[test]
    fn rejects_type_errors_and_implicit_contracts() {
        let bad = SRC.replace(
            "DECREMENT Product[item].stock BY q",
            "DECREMENT Product[item].stock BY item",
        );
        let m = parse_module(&bad, &Limits::v1()).unwrap();
        assert_eq!(
            lower_module(&m, None).unwrap_err().code,
            ErrorCode::TypeMismatch
        );
        let bad = SRC.replace("session: {}", "session: { ReadYourWrites }");
        let m = parse_module(&bad, &Limits::v1()).unwrap();
        assert_eq!(
            lower_module(&m, None).unwrap_err().code,
            ErrorCode::InvalidContract
        );
        let bad = SRC.replace("request_namespace: \"inventory\"", "");
        let m = parse_module(&bad, &Limits::v1()).unwrap();
        assert_eq!(
            lower_module(&m, None).unwrap_err().code,
            ErrorCode::InvalidContract
        );
    }
}
