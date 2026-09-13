//! Slow sequential reference interpreter (SPEC-003 §2, §6, §7).
//!
//! `evaluate(S, arguments) -> Rejected(reason) | Candidate(S', normalized_effects, result)`.
//! A `Candidate` is private: it becomes a durable outcome only through a plan's decision.
//! Business rejection changes no state. All arithmetic is checked; overflow, missing
//! rows, duplicate keys and invalid transitions abort the entire candidate.
//!
//! The interpreter is deliberately simple and complete: invariants are evaluated over
//! their full scope on the post-state. It is the independent oracle other components are
//! compared against (SPEC-010 §3) and it never imports compiler classification logic.

use std::collections::{BTreeMap, BTreeSet};

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::decimal::Decimal;
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, domains, Hash256};
use carolina_core::ids::*;

use crate::ast::{AggregateOp, BinOp, CmpOp};
use crate::ir::*;
use crate::types::{Type, Value};

/// Logical state: record → primary key → field id → value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub rows: BTreeMap<RecordId, BTreeMap<Value, BTreeMap<FieldId, Value>>>,
}

impl State {
    pub fn get(&self, record: RecordId, key: &Value) -> Option<&BTreeMap<FieldId, Value>> {
        self.rows.get(&record).and_then(|t| t.get(key))
    }
    pub fn get_mut(
        &mut self,
        record: RecordId,
        key: &Value,
    ) -> Option<&mut BTreeMap<FieldId, Value>> {
        self.rows.get_mut(&record).and_then(|t| t.get_mut(key))
    }
    pub fn insert(&mut self, record: RecordId, key: Value, row: BTreeMap<FieldId, Value>) -> bool {
        let t = self.rows.entry(record).or_default();
        if t.contains_key(&key) {
            return false;
        }
        t.insert(key, row);
        true
    }
    pub fn remove(&mut self, record: RecordId, key: &Value) -> Option<BTreeMap<FieldId, Value>> {
        self.rows.get_mut(&record).and_then(|t| t.remove(key))
    }
    pub fn table(
        &self,
        record: RecordId,
    ) -> impl Iterator<Item = (&Value, &BTreeMap<FieldId, Value>)> {
        self.rows.get(&record).into_iter().flat_map(|t| t.iter())
    }
    /// Insert a row given by field-name values (test/fixture helper).
    pub fn put_row(
        &mut self,
        module: &ModuleIR,
        record_name: &str,
        fields: &[(&str, Value)],
    ) -> CoreResult<()> {
        let rec = module
            .record_by_name(record_name)
            .ok_or_else(|| CoreError::new(ErrorCode::MissingRecord, record_name.to_string()))?;
        let mut row = BTreeMap::new();
        for (n, v) in fields {
            let f =
                rec.fields.iter().find(|f| f.name == *n).ok_or_else(|| {
                    CoreError::new(ErrorCode::MissingRecord, format!("field {n}"))
                })?;
            v.check_type(&f.ty)?;
            row.insert(f.id, v.clone());
        }
        for f in &rec.fields {
            if !row.contains_key(&f.id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidIr,
                    format!("missing field {}", f.name),
                ));
            }
        }
        let key = row[&rec.primary_key].clone();
        if !self.insert(rec.id, key, row) {
            return Err(CoreError::new(
                ErrorCode::DuplicateIdentity,
                "duplicate primary key",
            ));
        }
        Ok(())
    }
    pub fn field(
        &self,
        module: &ModuleIR,
        record_name: &str,
        key: &Value,
        field: &str,
    ) -> Option<Value> {
        let rec = module.record_by_name(record_name)?;
        let f = rec.fields.iter().find(|f| f.name == field)?;
        self.get(rec.id, key).and_then(|r| r.get(&f.id).cloned())
    }
    /// Canonical logical digest of the whole state (for replay/idempotence checks).
    pub fn digest(&self) -> Hash256 {
        domain_hash("astra.state.v1", &self.to_canon().encode())
    }
}

impl Canonical for State {
    fn to_canon(&self) -> CanonValue {
        let mut tables = BTreeMap::new();
        for (rid, t) in &self.rows {
            let rows: Vec<CanonValue> = t
                .iter()
                .map(|(k, row)| {
                    let mut fields = BTreeMap::new();
                    for (fid, v) in row {
                        fields.insert(fid.0.to_string(), v.to_canon());
                    }
                    CanonValue::obj()
                        .f("fields", CanonValue::Object(fields))
                        .fc("key", k)
                        .build()
                })
                .collect();
            tables.insert(rid.0.to_string(), CanonValue::Array(rows));
        }
        CanonValue::obj()
            .fstr("kind", "state.v1")
            .f("tables", CanonValue::Object(tables))
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let mut st = State::default();
        for (rid, rows) in v.field("tables")?.as_object()? {
            let rid = RecordId(carolina_core::canon::parse_canonical_int(rid)? as u64);
            for r in rows.as_array()? {
                let key = Value::from_canon(r.field("key")?)?;
                let mut row = BTreeMap::new();
                for (fid, fv) in r.field("fields")?.as_object()? {
                    row.insert(
                        FieldId(carolina_core::canon::parse_canonical_int(fid)? as u64),
                        Value::from_canon(fv)?,
                    );
                }
                st.insert(rid, key, row);
            }
        }
        Ok(st)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Definedness,
    Precondition,
    Read,
    Effect(u32),
    Postcondition,
    Invariant(InvariantId),
    Result,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub code: ErrorCode,
    pub reason: String,
    pub stage: Stage,
}

/// Normalized effect with literal operands (SPEC-003 §7 `NormalizedInvocation`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEffect {
    pub effect_id: u32,
    pub kind: AppliedKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliedKind {
    Assign {
        record: RecordId,
        key: Value,
        field: FieldId,
        value: Value,
    },
    Increment {
        record: RecordId,
        key: Value,
        field: FieldId,
        amount: Value,
    },
    Decrement {
        record: RecordId,
        key: Value,
        field: FieldId,
        amount: Value,
    },
    Insert {
        record: RecordId,
        key: Value,
        fields: BTreeMap<FieldId, Value>,
    },
    Delete {
        record: RecordId,
        key: Value,
    },
    AddToSet {
        record: RecordId,
        key: Value,
        field: FieldId,
        value: Value,
    },
    RemoveFromSet {
        record: RecordId,
        key: Value,
        field: FieldId,
        value: Value,
    },
    CompareAndSwap {
        record: RecordId,
        key: Value,
        field: FieldId,
        expected: Value,
        value: Value,
    },
    Reserve {
        resource: ResourceDeclId,
        key: Value,
        reservation_id: Value,
        amount: Value,
    },
    Release {
        resource: ResourceDeclId,
        key: Value,
        reservation_id: Value,
        amount: Value,
    },
    Transfer {
        source: (RecordId, Value, FieldId),
        destination: (RecordId, Value, FieldId),
        amount: Value,
    },
    AdvanceState {
        record: RecordId,
        key: Value,
        field: FieldId,
        expected: u32,
        next: u32,
    },
    EmitFact {
        fact: RecordId,
        key: Value,
        fields: BTreeMap<FieldId, Value>,
    },
}

fn loc(record: RecordId, key: &Value, field: Option<FieldId>) -> CanonValue {
    let mut o = CanonValue::obj().fc("key", key).fc("record", &record);
    if let Some(f) = field {
        o = o.fc("field", &f);
    }
    o.build()
}

fn fields_canon(fields: &BTreeMap<FieldId, Value>) -> CanonValue {
    let mut o = BTreeMap::new();
    for (f, v) in fields {
        o.insert(f.0.to_string(), v.to_canon());
    }
    CanonValue::Object(o)
}

impl Canonical for AppliedEffect {
    fn to_canon(&self) -> CanonValue {
        let k = match &self.kind {
            AppliedKind::Assign {
                record,
                key,
                field,
                value,
            } => CanonValue::obj()
                .fstr("kind", "assign")
                .f("target", loc(*record, key, Some(*field)))
                .fc("value", value)
                .build(),
            AppliedKind::Increment {
                record,
                key,
                field,
                amount,
            } => CanonValue::obj()
                .fc("amount", amount)
                .fstr("kind", "increment")
                .f("target", loc(*record, key, Some(*field)))
                .build(),
            AppliedKind::Decrement {
                record,
                key,
                field,
                amount,
            } => CanonValue::obj()
                .fc("amount", amount)
                .fstr("kind", "decrement")
                .f("target", loc(*record, key, Some(*field)))
                .build(),
            AppliedKind::Insert {
                record,
                key,
                fields,
            } => CanonValue::obj()
                .f("fields", fields_canon(fields))
                .fstr("kind", "insert")
                .f("target", loc(*record, key, None))
                .build(),
            AppliedKind::Delete { record, key } => CanonValue::obj()
                .fstr("kind", "delete")
                .f("target", loc(*record, key, None))
                .build(),
            AppliedKind::AddToSet {
                record,
                key,
                field,
                value,
            } => CanonValue::obj()
                .fstr("kind", "add_to_set")
                .f("target", loc(*record, key, Some(*field)))
                .fc("value", value)
                .build(),
            AppliedKind::RemoveFromSet {
                record,
                key,
                field,
                value,
            } => CanonValue::obj()
                .fstr("kind", "remove_from_set")
                .f("target", loc(*record, key, Some(*field)))
                .fc("value", value)
                .build(),
            AppliedKind::CompareAndSwap {
                record,
                key,
                field,
                expected,
                value,
            } => CanonValue::obj()
                .fc("expected", expected)
                .fstr("kind", "compare_and_swap")
                .f("target", loc(*record, key, Some(*field)))
                .fc("value", value)
                .build(),
            AppliedKind::Reserve {
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
            AppliedKind::Release {
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
            AppliedKind::Transfer {
                source,
                destination,
                amount,
            } => CanonValue::obj()
                .fc("amount", amount)
                .f(
                    "destination",
                    loc(destination.0, &destination.1, Some(destination.2)),
                )
                .fstr("kind", "transfer")
                .f("source", loc(source.0, &source.1, Some(source.2)))
                .build(),
            AppliedKind::AdvanceState {
                record,
                key,
                field,
                expected,
                next,
            } => CanonValue::obj()
                .fu32("expected", *expected)
                .fstr("kind", "advance_state")
                .fu32("next", *next)
                .f("target", loc(*record, key, Some(*field)))
                .build(),
            AppliedKind::EmitFact { fact, key, fields } => CanonValue::obj()
                .f("fields", fields_canon(fields))
                .fstr("kind", "emit_fact")
                .f("target", loc(*fact, key, None))
                .build(),
        };
        CanonValue::obj()
            .f("effect", k)
            .fu32("effect_id", self.effect_id)
            .build()
    }
    fn from_canon(_v: &CanonValue) -> CoreResult<Self> {
        Err(CoreError::new(
            ErrorCode::NonCanonicalEncoding,
            "AppliedEffect decoding is provided by the runtime replay codec",
        ))
    }
}

/// Accepted private candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub post_state: State,
    pub effects: Vec<AppliedEffect>,
    pub result: Value,
    pub captured_reads: BTreeMap<BindingId, Value>,
    pub guard_decisions: Vec<(u32, bool)>,
}

impl Candidate {
    /// `NormalizedInvocation` canonical payload (SPEC-003 §7): arguments, captured reads,
    /// guard decisions, literal effects, exact result and operation identity.
    pub fn normalized_invocation(&self, op: OperationRef, args: &[Value]) -> CanonValue {
        let mut reads = BTreeMap::new();
        for (b, v) in &self.captured_reads {
            reads.insert(b.0.to_string(), v.to_canon());
        }
        CanonValue::obj()
            .fvec("arguments", args)
            .f("captured_reads", CanonValue::Object(reads))
            .fvec("effects", &self.effects)
            .f(
                "guard_decisions",
                CanonValue::Array(
                    self.guard_decisions
                        .iter()
                        .map(|(i, b)| {
                            CanonValue::obj()
                                .fbool("accepted", *b)
                                .fu32("effect_id", *i)
                                .build()
                        })
                        .collect(),
                ),
            )
            .fstr("kind", "invocation.v1")
            .fc("operation", &op)
            .fc("result", &self.result)
            .build()
    }
    pub fn invocation_digest(&self, op: OperationRef, args: &[Value]) -> Hash256 {
        domain_hash(
            domains::INVOCATION_V1,
            &self.normalized_invocation(op, args).encode(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Rejected(Rejection),
    Accepted(Box<Candidate>),
}

impl Outcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, Outcome::Accepted(_))
    }
    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            Outcome::Rejected(r) => Some(r),
            _ => None,
        }
    }
    pub fn candidate(&self) -> Option<&Candidate> {
        match self {
            Outcome::Accepted(c) => Some(c),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub invariant: InvariantId,
    pub instance: Option<Value>,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Evaluation environment
// ---------------------------------------------------------------------------

struct Env<'a> {
    module: &'a ModuleIR,
    args: &'a [Value],
    bindings: BTreeMap<BindingId, Value>,
    group_key: Option<Value>,
}

fn rej(code: ErrorCode, reason: impl Into<String>, stage: Stage) -> Rejection {
    Rejection {
        code,
        reason: reason.into(),
        stage,
    }
}

fn row_value(
    module: &ModuleIR,
    record: RecordId,
    key: &Value,
    row: &BTreeMap<FieldId, Value>,
) -> Value {
    let rec = module.record(record).expect("record");
    let mut fields = BTreeMap::new();
    for f in &rec.fields {
        if let Some(v) = row.get(&f.id) {
            fields.insert(f.name.clone(), v.clone());
        }
    }
    let _ = key;
    Value::Row { record, fields }
}

fn checked_arith(op: BinOp, a: &Value, b: &Value) -> CoreResult<Value> {
    let ov = || CoreError::new(ErrorCode::NumericOverflow, "arithmetic overflow");
    Ok(match (a, b) {
        (Value::I64(x), Value::I64(y)) => Value::I64(match op {
            BinOp::Add => x.checked_add(*y).ok_or_else(ov)?,
            BinOp::Sub => x.checked_sub(*y).ok_or_else(ov)?,
            BinOp::Mul => x.checked_mul(*y).ok_or_else(ov)?,
            _ => unreachable!(),
        }),
        (Value::U64(x), Value::U64(y)) => Value::U64(match op {
            BinOp::Add => x.checked_add(*y).ok_or_else(ov)?,
            BinOp::Sub => x.checked_sub(*y).ok_or_else(ov)?,
            BinOp::Mul => x.checked_mul(*y).ok_or_else(ov)?,
            _ => unreachable!(),
        }),
        (Value::Decimal(x), Value::Decimal(y)) => Value::Decimal(match op {
            BinOp::Add => x.checked_add(y)?,
            BinOp::Sub => x.checked_sub(y)?,
            BinOp::Mul => {
                // affine fragment: one operand must be an integral constant of the same type
                let int_of = |d: &Decimal| -> Option<i128> {
                    let f = 10i128.pow(d.scale() as u32);
                    if d.coefficient() % f == 0 {
                        Some(d.coefficient() / f)
                    } else {
                        None
                    }
                };
                match (int_of(y), int_of(x)) {
                    (Some(k), _) => x.checked_mul_int(k)?,
                    (_, Some(k)) => y.checked_mul_int(k)?,
                    _ => {
                        return Err(CoreError::new(
                            ErrorCode::UnsupportedEffect,
                            "decimal multiplication requires an integral constant",
                        ))
                    }
                }
            }
            _ => unreachable!(),
        }),
        _ => {
            return Err(CoreError::new(
                ErrorCode::TypeMismatch,
                "arithmetic on mismatched operands",
            ))
        }
    })
}

fn compare(op: CmpOp, a: &Value, b: &Value) -> CoreResult<bool> {
    if std::mem::discriminant(a) != std::mem::discriminant(b) {
        return Err(CoreError::new(
            ErrorCode::TypeMismatch,
            "comparison of mismatched operands",
        ));
    }
    if let (Value::Decimal(x), Value::Decimal(y)) = (a, b) {
        if x.scale() != y.scale() || x.precision() != y.precision() {
            return Err(CoreError::new(
                ErrorCode::TypeMismatch,
                "cross-scale decimal comparison",
            ));
        }
    }
    let ord = a.cmp(b);
    Ok(match op {
        CmpOp::Eq => ord.is_eq(),
        CmpOp::Ne => ord.is_ne(),
        CmpOp::Lt => ord.is_lt(),
        CmpOp::Le => ord.is_le(),
        CmpOp::Gt => ord.is_gt(),
        CmpOp::Ge => ord.is_ge(),
    })
}

fn zero_of(ty: &Type) -> CoreResult<Value> {
    Ok(match ty {
        Type::I64 => Value::I64(0),
        Type::U64 => Value::U64(0),
        Type::Decimal(p, s) => Value::Decimal(Decimal::zero(*p, *s)?),
        _ => {
            return Err(CoreError::new(
                ErrorCode::TypeMismatch,
                "zero of non-numeric type",
            ))
        }
    })
}

impl<'a> Env<'a> {
    fn eval(&mut self, e: &ExprIR, pre: &State, cand: &State) -> CoreResult<Value> {
        let v = match &e.node {
            ExprNodeIR::Lit(v) => v.clone(),
            ExprNodeIR::Param(i) => self
                .args
                .get(*i as usize)
                .cloned()
                .ok_or_else(|| CoreError::new(ErrorCode::InvalidIr, "missing argument"))?,
            ExprNodeIR::Binding(b) => self.bindings.get(b).cloned().ok_or_else(|| {
                CoreError::new(ErrorCode::InvalidIr, format!("unbound binding {}", b.0))
            })?,
            ExprNodeIR::GroupKey => self.group_key.clone().ok_or_else(|| {
                CoreError::new(ErrorCode::InvalidIr, "GROUP key outside aggregate")
            })?,
            ExprNodeIR::Field { base, name, .. } => match self.eval(base, pre, cand)? {
                Value::Row { fields, .. } => fields.get(name).cloned().ok_or_else(|| {
                    CoreError::new(ErrorCode::InvalidIr, format!("row has no field {name}"))
                })?,
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::TypeMismatch,
                        "field access on non-row",
                    ))
                }
            },
            ExprNodeIR::StructField { base, name } => match self.eval(base, pre, cand)? {
                Value::Struct(fields) => fields.get(name).cloned().ok_or_else(|| {
                    CoreError::new(ErrorCode::InvalidIr, format!("struct has no field {name}"))
                })?,
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::TypeMismatch,
                        "struct field access on non-struct",
                    ))
                }
            },
            ExprNodeIR::RowLookup { record, key, state } => {
                let k = self.eval(key, pre, cand)?;
                let st = if *state == StateRef::Pre { pre } else { cand };
                match st.get(*record, &k) {
                    Some(row) => row_value(self.module, *record, &k, row),
                    None => {
                        return Err(CoreError::new(
                            ErrorCode::MissingRecord,
                            format!("record {} key {:?} is absent", record, k),
                        ))
                    }
                }
            }
            ExprNodeIR::Exists { record, key, state } => {
                let k = self.eval(key, pre, cand)?;
                let st = if *state == StateRef::Pre { pre } else { cand };
                Value::Bool(st.get(*record, &k).is_some())
            }
            ExprNodeIR::Tuple(items) => Value::Tuple(
                items
                    .iter()
                    .map(|i| self.eval(i, pre, cand))
                    .collect::<CoreResult<_>>()?,
            ),
            ExprNodeIR::Struct(fields) => {
                let mut m = BTreeMap::new();
                for (n, fe) in fields {
                    m.insert(n.clone(), self.eval(fe, pre, cand)?);
                }
                Value::Struct(m)
            }
            ExprNodeIR::SetLit(items) => {
                let mut s = BTreeSet::new();
                for i in items {
                    s.insert(self.eval(i, pre, cand)?);
                }
                Value::Set(s)
            }
            ExprNodeIR::SomeOf(inner) => Value::Some(Box::new(self.eval(inner, pre, cand)?)),
            ExprNodeIR::Neg(inner) => match self.eval(inner, pre, cand)? {
                Value::I64(x) => Value::I64(x.checked_neg().ok_or_else(|| {
                    CoreError::new(ErrorCode::NumericOverflow, "negation overflow")
                })?),
                Value::Decimal(d) => Value::Decimal(d.checked_neg()?),
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::TypeMismatch,
                        "negation of non-numeric",
                    ))
                }
            },
            ExprNodeIR::Not(inner) => Value::Bool(!self.eval(inner, pre, cand)?.as_bool()?),
            ExprNodeIR::Bin { op, lhs, rhs } => match op {
                BinOp::And => {
                    let l = self.eval(lhs, pre, cand)?.as_bool()?;
                    // short-circuit is semantically safe: pure expressions
                    Value::Bool(l && self.eval(rhs, pre, cand)?.as_bool()?)
                }
                BinOp::Or => {
                    let l = self.eval(lhs, pre, cand)?.as_bool()?;
                    Value::Bool(l || self.eval(rhs, pre, cand)?.as_bool()?)
                }
                BinOp::Add | BinOp::Sub | BinOp::Mul => {
                    let l = self.eval(lhs, pre, cand)?;
                    let r = self.eval(rhs, pre, cand)?;
                    checked_arith(*op, &l, &r)?
                }
                BinOp::Cmp(c) => {
                    let l = self.eval(lhs, pre, cand)?;
                    let r = self.eval(rhs, pre, cand)?;
                    Value::Bool(compare(*c, &l, &r)?)
                }
                BinOp::In => {
                    let l = self.eval(lhs, pre, cand)?;
                    match self.eval(rhs, pre, cand)? {
                        Value::Set(s) => Value::Bool(s.contains(&l)),
                        _ => return Err(CoreError::new(ErrorCode::TypeMismatch, "IN on non-set")),
                    }
                }
            },
            ExprNodeIR::IsNone(inner) => {
                Value::Bool(matches!(self.eval(inner, pre, cand)?, Value::None))
            }
            ExprNodeIR::IsSome(inner) => {
                Value::Bool(matches!(self.eval(inner, pre, cand)?, Value::Some(_)))
            }
            ExprNodeIR::UnwrapOr { value, default } => match self.eval(value, pre, cand)? {
                Value::Some(v) => *v,
                Value::None => self.eval(default, pre, cand)?,
                _ => return Err(CoreError::new(ErrorCode::TypeMismatch, "?? on non-option")),
            },
            ExprNodeIR::Size(inner) => match self.eval(inner, pre, cand)? {
                Value::Set(s) => Value::U64(s.len() as u64),
                _ => return Err(CoreError::new(ErrorCode::TypeMismatch, "SIZE on non-set")),
            },
            ExprNodeIR::SumOver { var, set, value } => {
                let items = match self.eval(set, pre, cand)? {
                    Value::Set(s) => s,
                    _ => return Err(CoreError::new(ErrorCode::TypeMismatch, "SUM over non-set")),
                };
                let mut acc = zero_of(&value.ty)?;
                for it in items {
                    self.bindings.insert(*var, it);
                    let v = self.eval(value, pre, cand)?;
                    acc = checked_arith(BinOp::Add, &acc, &v)?;
                }
                self.bindings.remove(var);
                acc
            }
        };
        Ok(v)
    }
}

// ---------------------------------------------------------------------------
// Operation evaluation
// ---------------------------------------------------------------------------

/// Evaluate one invocation against `pre` (SPEC-003 §2). Never mutates `pre`.
pub fn evaluate(
    module: &ModuleIR,
    op_ref: OperationRef,
    args: &[Value],
    pre: &State,
) -> CoreResult<Outcome> {
    evaluate_with_scope(module, op_ref, args, pre, None)
}

/// Evaluate a runtime-loaded closure. `invariants` must be the compiler's complete
/// invariant closure for this operation, and `pre` must include all of its records.
/// The ordinary reference evaluator continues to check the full module state.
pub fn evaluate_scoped(
    module: &ModuleIR,
    op_ref: OperationRef,
    args: &[Value],
    pre: &State,
    invariants: &BTreeSet<InvariantId>,
) -> CoreResult<Outcome> {
    for id in invariants {
        module.invariant(*id)?;
    }
    evaluate_with_scope(module, op_ref, args, pre, Some(invariants))
}

fn evaluate_with_scope(
    module: &ModuleIR,
    op_ref: OperationRef,
    args: &[Value],
    pre: &State,
    invariant_scope: Option<&BTreeSet<InvariantId>>,
) -> CoreResult<Outcome> {
    let op = module.operation(op_ref)?;
    if args.len() != op.parameters.len() {
        return Ok(Outcome::Rejected(rej(
            ErrorCode::TypeMismatch,
            "argument count mismatch",
            Stage::Definedness,
        )));
    }
    for (a, p) in args.iter().zip(&op.parameters) {
        if let Err(e) = a.check_type(&p.ty) {
            return Ok(Outcome::Rejected(rej(
                ErrorCode::TypeMismatch,
                e.message,
                Stage::Definedness,
            )));
        }
    }
    let mut env = Env {
        module,
        args,
        bindings: BTreeMap::new(),
        group_key: None,
    };
    let mut cand = pre.clone();

    // REQUIRE (pre-state)
    match env.eval(&op.pre, pre, &cand) {
        Ok(Value::Bool(true)) => {}
        Ok(_) => {
            return Ok(Outcome::Rejected(rej(
                ErrorCode::PreconditionRejected,
                "REQUIRE evaluated to false",
                Stage::Precondition,
            )))
        }
        Err(e) => {
            return Ok(Outcome::Rejected(rej(
                e.code,
                e.message,
                Stage::Precondition,
            )))
        }
    }

    // READ (pre-state snapshot)
    let mut captured = BTreeMap::new();
    for rd in &op.reads {
        let v = match &rd.kind {
            ReadKind::Row { record, key } => {
                let k = match env.eval(key, pre, &cand) {
                    Ok(k) => k,
                    Err(e) => return Ok(Outcome::Rejected(rej(e.code, e.message, Stage::Read))),
                };
                match pre.get(*record, &k) {
                    Some(row) => row_value(module, *record, &k, row),
                    None => {
                        return Ok(Outcome::Rejected(rej(
                            ErrorCode::MissingRecord,
                            format!("read `{}`: record absent", rd.name),
                            Stage::Read,
                        )))
                    }
                }
            }
            ReadKind::OptionalRow { record, key } => {
                let k = match env.eval(key, pre, &cand) {
                    Ok(k) => k,
                    Err(e) => return Ok(Outcome::Rejected(rej(e.code, e.message, Stage::Read))),
                };
                match pre.get(*record, &k) {
                    Some(row) => Value::Some(Box::new(row_value(module, *record, &k, row))),
                    None => Value::None,
                }
            }
            ReadKind::Exists { record, key } => {
                let k = match env.eval(key, pre, &cand) {
                    Ok(k) => k,
                    Err(e) => return Ok(Outcome::Rejected(rej(e.code, e.message, Stage::Read))),
                };
                Value::Bool(pre.get(*record, &k).is_some())
            }
            ReadKind::Scan {
                record,
                var,
                filter,
            } => {
                let mut out = BTreeSet::new();
                for (k, row) in pre.table(*record) {
                    let rv = row_value(module, *record, k, row);
                    env.bindings.insert(*var, rv.clone());
                    match env.eval(filter, pre, &cand) {
                        Ok(Value::Bool(true)) => {
                            out.insert(rv);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            return Ok(Outcome::Rejected(rej(e.code, e.message, Stage::Read)))
                        }
                    }
                }
                env.bindings.remove(var);
                Value::Set(out)
            }
        };
        env.bindings.insert(rd.binding, v.clone());
        captured.insert(rd.binding, v);
    }

    // EFFECTS (candidate state, source order)
    let mut applied = Vec::new();
    let mut guards = Vec::new();
    for ef in &op.effects {
        if let Some(g) = &ef.guard {
            match env.eval(g, pre, &cand) {
                Ok(Value::Bool(true)) => guards.push((ef.effect_id, true)),
                Ok(_) => {
                    guards.push((ef.effect_id, false));
                    continue;
                }
                Err(e) => {
                    return Ok(Outcome::Rejected(rej(
                        e.code,
                        e.message,
                        Stage::Effect(ef.effect_id),
                    )))
                }
            }
        }
        match apply_effect_ir(module, &mut env, ef, pre, &mut cand) {
            Ok(a) => applied.push(a),
            Err(e) => {
                return Ok(Outcome::Rejected(rej(
                    e.code,
                    e.message,
                    Stage::Effect(ef.effect_id),
                )))
            }
        }
    }

    // ENSURE (post-state)
    match env.eval(&op.post, pre, &cand) {
        Ok(Value::Bool(true)) => {}
        Ok(_) => {
            return Ok(Outcome::Rejected(rej(
                ErrorCode::PostconditionRejected,
                "ENSURE evaluated to false",
                Stage::Postcondition,
            )))
        }
        Err(e) => {
            return Ok(Outcome::Rejected(rej(
                e.code,
                e.message,
                Stage::Postcondition,
            )))
        }
    }

    // invariants: causal prerequisites on the pre-state, state predicates on the post-state, transitions on the pair
    for inv in &module.invariants {
        if invariant_scope.is_some_and(|scope| !scope.contains(&inv.id)) {
            continue;
        }
        if let PredicateIR::RequiresFact {
            operation,
            key_params,
            fact,
        } = &inv.predicate
        {
            if *operation == op_ref.operation_id {
                let key = if key_params.len() == 1 {
                    args[key_params[0] as usize].clone()
                } else {
                    Value::Tuple(
                        key_params
                            .iter()
                            .map(|i| args[*i as usize].clone())
                            .collect(),
                    )
                };
                if pre.get(*fact, &key).is_none() {
                    return Ok(Outcome::Rejected(rej(
                        ErrorCode::MissingDependency,
                        format!(
                            "invariant `{}`: required fact absent from the causal past",
                            inv.name
                        ),
                        Stage::Invariant(inv.id),
                    )));
                }
            }
        }
    }
    let mut violations = Vec::new();
    for inv in &module.invariants {
        if invariant_scope.is_none_or(|scope| scope.contains(&inv.id)) {
            check_invariant(module, inv, pre, &cand, &mut violations)?;
        }
    }
    if let Some(v) = violations.first() {
        let inv = module.invariant(v.invariant)?;
        return Ok(Outcome::Rejected(rej(
            ErrorCode::InvariantRejected,
            format!("invariant `{}` violated: {}", inv.name, v.detail),
            Stage::Invariant(v.invariant),
        )));
    }

    // RETURN (post-state)
    let result = match env.eval(&op.result, pre, &cand) {
        Ok(v) => v,
        Err(e) => return Ok(Outcome::Rejected(rej(e.code, e.message, Stage::Result))),
    };

    Ok(Outcome::Accepted(Box::new(Candidate {
        post_state: cand,
        effects: applied,
        result,
        captured_reads: captured,
        guard_decisions: guards,
    })))
}

fn field_type(module: &ModuleIR, record: RecordId, field: FieldId) -> CoreResult<Type> {
    let rec = module.record(record)?;
    rec.fields
        .iter()
        .find(|f| f.id == field)
        .map(|f| f.ty.clone())
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidIr, "unknown field"))
}

fn apply_effect_ir(
    module: &ModuleIR,
    env: &mut Env,
    ef: &EffectIR,
    pre: &State,
    cand: &mut State,
) -> CoreResult<AppliedEffect> {
    let kind = match &ef.kind {
        EffectKind::Assign { path, value } => {
            let key = env.eval(&path.key, pre, cand)?;
            let v = env.eval(value, pre, cand)?;
            AppliedKind::Assign {
                record: path.record,
                key,
                field: path.field,
                value: v,
            }
        }
        EffectKind::Increment { path, amount } => {
            let key = env.eval(&path.key, pre, cand)?;
            let a = env.eval(amount, pre, cand)?;
            AppliedKind::Increment {
                record: path.record,
                key,
                field: path.field,
                amount: a,
            }
        }
        EffectKind::Decrement { path, amount } => {
            let key = env.eval(&path.key, pre, cand)?;
            let a = env.eval(amount, pre, cand)?;
            AppliedKind::Decrement {
                record: path.record,
                key,
                field: path.field,
                amount: a,
            }
        }
        EffectKind::Insert {
            record,
            key,
            fields,
        } => {
            let k = env.eval(key, pre, cand)?;
            let mut fs = BTreeMap::new();
            for (f, e) in fields {
                fs.insert(*f, env.eval(e, pre, cand)?);
            }
            AppliedKind::Insert {
                record: *record,
                key: k,
                fields: fs,
            }
        }
        EffectKind::Delete { record, key } => AppliedKind::Delete {
            record: *record,
            key: env.eval(key, pre, cand)?,
        },
        EffectKind::AddToSet { path, value } => {
            let key = env.eval(&path.key, pre, cand)?;
            AppliedKind::AddToSet {
                record: path.record,
                key,
                field: path.field,
                value: env.eval(value, pre, cand)?,
            }
        }
        EffectKind::RemoveFromSet { path, value } => {
            let key = env.eval(&path.key, pre, cand)?;
            AppliedKind::RemoveFromSet {
                record: path.record,
                key,
                field: path.field,
                value: env.eval(value, pre, cand)?,
            }
        }
        EffectKind::CompareAndSwap {
            path,
            expected,
            value,
        } => {
            let key = env.eval(&path.key, pre, cand)?;
            AppliedKind::CompareAndSwap {
                record: path.record,
                key,
                field: path.field,
                expected: env.eval(expected, pre, cand)?,
                value: env.eval(value, pre, cand)?,
            }
        }
        EffectKind::Reserve {
            resource,
            key,
            reservation_id,
            amount,
        } => AppliedKind::Reserve {
            resource: *resource,
            key: env.eval(key, pre, cand)?,
            reservation_id: env.eval(reservation_id, pre, cand)?,
            amount: env.eval(amount, pre, cand)?,
        },
        EffectKind::Release {
            resource,
            key,
            reservation_id,
            amount,
        } => AppliedKind::Release {
            resource: *resource,
            key: env.eval(key, pre, cand)?,
            reservation_id: env.eval(reservation_id, pre, cand)?,
            amount: env.eval(amount, pre, cand)?,
        },
        EffectKind::TransferQuantity {
            source,
            destination,
            amount,
        } => AppliedKind::Transfer {
            source: (
                source.record,
                env.eval(&source.key, pre, cand)?,
                source.field,
            ),
            destination: (
                destination.record,
                env.eval(&destination.key, pre, cand)?,
                destination.field,
            ),
            amount: env.eval(amount, pre, cand)?,
        },
        EffectKind::AdvanceState {
            path,
            expected,
            next,
            ..
        } => {
            let key = env.eval(&path.key, pre, cand)?;
            AppliedKind::AdvanceState {
                record: path.record,
                key,
                field: path.field,
                expected: *expected,
                next: *next,
            }
        }
        EffectKind::EmitFact { fact, key, fields } => {
            let k = env.eval(key, pre, cand)?;
            let mut fs = BTreeMap::new();
            for (f, e) in fields {
                fs.insert(*f, env.eval(e, pre, cand)?);
            }
            AppliedKind::EmitFact {
                fact: *fact,
                key: k,
                fields: fs,
            }
        }
    };
    let applied = AppliedEffect {
        effect_id: ef.effect_id,
        kind,
    };
    apply_literal(module, cand, &applied)?;
    Ok(applied)
}

fn positive(v: &Value) -> CoreResult<()> {
    let ok = match v {
        Value::I64(x) => *x > 0,
        Value::U64(x) => *x > 0,
        Value::Decimal(d) => d.coefficient() > 0,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::ArithmeticError,
            "amount must be positive",
        ))
    }
}

fn get_field_mut<'s>(
    cand: &'s mut State,
    record: RecordId,
    key: &Value,
    field: FieldId,
) -> CoreResult<&'s mut Value> {
    let row = cand.get_mut(record, key).ok_or_else(|| {
        CoreError::new(
            ErrorCode::MissingRecord,
            format!("record {record} key {key:?} absent"),
        )
    })?;
    row.get_mut(&field)
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidIr, "field absent in row"))
}

/// Deterministic semantic application of a literal effect (SPEC-003 §7, SPEC-005 §6).
/// Deltas are applied to the *current* value, never overwritten with a stale computed value.
pub fn apply_literal(module: &ModuleIR, cand: &mut State, ef: &AppliedEffect) -> CoreResult<()> {
    match &ef.kind {
        AppliedKind::Assign {
            record,
            key,
            field,
            value,
        } => {
            let ty = field_type(module, *record, *field)?;
            value.check_type(&ty)?;
            *get_field_mut(cand, *record, key, *field)? = value.clone();
        }
        AppliedKind::Increment {
            record,
            key,
            field,
            amount,
        } => {
            positive(amount)?;
            let slot = get_field_mut(cand, *record, key, *field)?;
            *slot = checked_arith(BinOp::Add, slot, amount)?;
        }
        AppliedKind::Decrement {
            record,
            key,
            field,
            amount,
        } => {
            positive(amount)?;
            let slot = get_field_mut(cand, *record, key, *field)?;
            *slot = checked_arith(BinOp::Sub, slot, amount)?;
        }
        AppliedKind::Insert {
            record,
            key,
            fields,
        }
        | AppliedKind::EmitFact {
            fact: record,
            key,
            fields,
        } => {
            let is_fact = matches!(ef.kind, AppliedKind::EmitFact { .. });
            let rec = module.record(*record)?;
            let mut row = BTreeMap::new();
            for f in &rec.fields {
                let v = fields.get(&f.id).ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidIr,
                        format!("insert missing field {}", f.name),
                    )
                })?;
                v.check_type(&f.ty)?;
                row.insert(f.id, v.clone());
            }
            if row[&rec.primary_key] != *key {
                return Err(CoreError::new(
                    ErrorCode::IdentityMismatch,
                    "insert key does not match primary key field",
                ));
            }
            if let Some(existing) = cand.get(*record, key) {
                // SPEC-003 §6: an identical immutable fact cannot create another fact (idempotent);
                // a different payload under one fact identity is a conflict, and mutable inserts never overwrite.
                if is_fact && *existing == row {
                    return Ok(());
                }
                return Err(CoreError::new(
                    ErrorCode::DuplicateIdentity,
                    format!("record {} key {:?} already exists", record, key),
                ));
            }
            cand.insert(*record, key.clone(), row);
        }
        AppliedKind::Delete { record, key } => {
            if cand.remove(*record, key).is_none() {
                return Err(CoreError::new(
                    ErrorCode::MissingRecord,
                    "delete of absent row",
                ));
            }
        }
        AppliedKind::AddToSet {
            record,
            key,
            field,
            value,
        } => match get_field_mut(cand, *record, key, *field)? {
            Value::Set(s) => {
                s.insert(value.clone());
            }
            _ => return Err(CoreError::new(ErrorCode::TypeMismatch, "ADD to non-set")),
        },
        AppliedKind::RemoveFromSet {
            record,
            key,
            field,
            value,
        } => match get_field_mut(cand, *record, key, *field)? {
            Value::Set(s) => {
                s.remove(value);
            }
            _ => {
                return Err(CoreError::new(
                    ErrorCode::TypeMismatch,
                    "REMOVE from non-set",
                ))
            }
        },
        AppliedKind::CompareAndSwap {
            record,
            key,
            field,
            expected,
            value,
        } => {
            let slot = get_field_mut(cand, *record, key, *field)?;
            if slot != expected {
                return Err(CoreError::new(
                    ErrorCode::Conflict,
                    "compare-and-swap mismatch",
                ));
            }
            *slot = value.clone();
        }
        AppliedKind::Reserve {
            resource,
            key,
            reservation_id,
            amount,
        } => {
            positive(amount)?;
            let rs = module.resource(*resource)?.clone();
            {
                let avail = get_field_mut(cand, rs.record, key, rs.available)?;
                *avail = checked_arith(BinOp::Sub, avail, amount)?;
            }
            {
                let held = get_field_mut(cand, rs.record, key, rs.reserved)?;
                *held = checked_arith(BinOp::Add, held, amount)?;
            }
            let resv = module.record(rs.reservation_record)?;
            let mut row = BTreeMap::new();
            for f in &resv.fields {
                let v = match f.name.as_str() {
                    "resource" => key.clone(),
                    "amount" => amount.clone(),
                    "state" => match &f.ty {
                        Type::Enum(e) => Value::Enum {
                            enum_id: *e,
                            variant: module
                                .enum_def(*e)?
                                .variants
                                .iter()
                                .position(|v| v == "Active")
                                .unwrap() as u32,
                        },
                        _ => unreachable!(),
                    },
                    _ if f.id == resv.primary_key => reservation_id.clone(),
                    other => {
                        return Err(CoreError::new(
                            ErrorCode::InvalidIr,
                            format!("reservation record has unsupported extra field `{other}`"),
                        ))
                    }
                };
                row.insert(f.id, v);
            }
            if !cand.insert(rs.reservation_record, reservation_id.clone(), row) {
                return Err(CoreError::new(
                    ErrorCode::DuplicateIdentity,
                    "reservation id already exists",
                ));
            }
        }
        AppliedKind::Release {
            resource,
            key,
            reservation_id,
            amount,
        } => {
            positive(amount)?;
            let rs = module.resource(*resource)?.clone();
            let resv = module.record(rs.reservation_record)?.clone();
            let fid = |n: &str| {
                resv.fields
                    .iter()
                    .find(|f| f.name == n)
                    .map(|f| f.id)
                    .unwrap()
            };
            let (state_fid, res_fid, amt_fid) = (fid("state"), fid("resource"), fid("amount"));
            let enum_id = match field_type(module, resv.id, state_fid)? {
                Type::Enum(e) => e,
                _ => unreachable!(),
            };
            let variants = &module.enum_def(enum_id)?.variants;
            let active = variants.iter().position(|v| v == "Active").unwrap() as u32;
            let released = variants.iter().position(|v| v == "Released").unwrap() as u32;
            let row = cand
                .get(resv.id, reservation_id)
                .ok_or_else(|| CoreError::new(ErrorCode::MissingRecord, "unknown reservation"))?;
            if row[&res_fid] != *key {
                return Err(CoreError::new(
                    ErrorCode::IdentityMismatch,
                    "reservation belongs to another resource key",
                ));
            }
            if row[&amt_fid] != *amount {
                return Err(CoreError::new(
                    ErrorCode::UnsupportedEffect,
                    "v1 supports whole-reservation release only; amount mismatch",
                ));
            }
            match &row[&state_fid] {
                Value::Enum { variant, .. } if *variant == active => {}
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::Conflict,
                        "reservation is not ACTIVE",
                    ))
                }
            }
            *get_field_mut(cand, resv.id, reservation_id, state_fid)? = Value::Enum {
                enum_id,
                variant: released,
            };
            {
                let held = get_field_mut(cand, rs.record, key, rs.reserved)?;
                *held = checked_arith(BinOp::Sub, held, amount)?;
            }
            {
                let avail = get_field_mut(cand, rs.record, key, rs.available)?;
                *avail = checked_arith(BinOp::Add, avail, amount)?;
            }
        }
        AppliedKind::Transfer {
            source,
            destination,
            amount,
        } => {
            positive(amount)?;
            if source == destination {
                return Err(CoreError::new(
                    ErrorCode::UnsupportedEffect,
                    "transfer requires distinct endpoints",
                ));
            }
            {
                let s = get_field_mut(cand, source.0, &source.1, source.2)?;
                *s = checked_arith(BinOp::Sub, s, amount)?;
            }
            {
                let d = get_field_mut(cand, destination.0, &destination.1, destination.2)?;
                *d = checked_arith(BinOp::Add, d, amount)?;
            }
        }
        AppliedKind::AdvanceState {
            record,
            key,
            field,
            expected,
            next,
        } => {
            let slot = get_field_mut(cand, *record, key, *field)?;
            match slot {
                Value::Enum { variant, enum_id } if *variant == *expected => {
                    *slot = Value::Enum {
                        enum_id: *enum_id,
                        variant: *next,
                    };
                }
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::Conflict,
                        "ADVANCE expected state mismatch",
                    ))
                }
            }
        }
    }
    Ok(())
}

/// Apply already-accepted literal effects (replay / replication). Every effect either applies or the call fails
/// with the state left untouched.
pub fn apply_effects(
    module: &ModuleIR,
    state: &mut State,
    effects: &[AppliedEffect],
) -> CoreResult<()> {
    let mut work = state.clone();
    for e in effects {
        apply_literal(module, &mut work, e)?;
    }
    *state = work;
    Ok(())
}

// ---------------------------------------------------------------------------
// Invariant checking
// ---------------------------------------------------------------------------

/// Check every declared invariant over the full scope of `post` (and transitions from `pre`).
pub fn check_invariants(
    module: &ModuleIR,
    pre: &State,
    post: &State,
) -> CoreResult<Vec<Violation>> {
    let mut out = Vec::new();
    for inv in &module.invariants {
        check_invariant(module, inv, pre, post, &mut out)?;
    }
    Ok(out)
}

pub fn check_invariant(
    module: &ModuleIR,
    inv: &InvariantIR,
    pre: &State,
    post: &State,
    out: &mut Vec<Violation>,
) -> CoreResult<()> {
    match check_invariant_inner(module, inv, pre, post, out) {
        Err(e)
            if matches!(
                e.code,
                ErrorCode::NumericOverflow | ErrorCode::MissingRecord
            ) =>
        {
            // A partial expression becoming undefined in the candidate state is a semantic
            // rejection. Letting it escape would strand an already-bound runtime request as
            // OutcomeUnknown forever. Malformed IR and other internal failures still propagate.
            out.push(Violation {
                invariant: inv.id,
                instance: None,
                detail: format!("evaluation error: {e}"),
            });
            Ok(())
        }
        result => result,
    }
}

fn check_invariant_inner(
    module: &ModuleIR,
    inv: &InvariantIR,
    pre: &State,
    post: &State,
    out: &mut Vec<Violation>,
) -> CoreResult<()> {
    let mut env = Env {
        module,
        args: &[],
        bindings: BTreeMap::new(),
        group_key: None,
    };
    match &inv.predicate {
        PredicateIR::ForAll {
            record,
            var,
            filter,
            predicate,
        } => {
            for (k, row) in post.table(*record) {
                env.bindings
                    .insert(*var, row_value(module, *record, k, row));
                if let Some(f) = filter {
                    if !env.eval(f, post, post)?.as_bool()? {
                        continue;
                    }
                }
                if !env.eval(predicate, post, post)?.as_bool()? {
                    out.push(Violation {
                        invariant: inv.id,
                        instance: Some(k.clone()),
                        detail: format!("predicate false for key {k:?}"),
                    });
                }
            }
        }
        PredicateIR::Unique {
            record,
            var,
            filter,
            key,
        } => {
            let mut seen: BTreeMap<Value, Value> = BTreeMap::new();
            for (k, row) in post.table(*record) {
                env.bindings
                    .insert(*var, row_value(module, *record, k, row));
                if let Some(f) = filter {
                    if !env.eval(f, post, post)?.as_bool()? {
                        continue;
                    }
                }
                let uk = env.eval(key, post, post)?;
                if let Some(prev) = seen.insert(uk.clone(), k.clone()) {
                    out.push(Violation {
                        invariant: inv.id,
                        instance: Some(uk),
                        detail: format!("duplicate unique key across rows {prev:?} and {k:?}"),
                    });
                }
            }
        }
        PredicateIR::ExistsReference {
            child,
            var,
            parent_key,
            parent,
        } => {
            for (k, row) in post.table(*child) {
                env.bindings.insert(*var, row_value(module, *child, k, row));
                let pk = env.eval(parent_key, post, post)?;
                if post.get(*parent, &pk).is_none() {
                    out.push(Violation {
                        invariant: inv.id,
                        instance: Some(k.clone()),
                        detail: format!("dangling reference to parent key {pk:?}"),
                    });
                }
            }
        }
        PredicateIR::Aggregate {
            op,
            record,
            var,
            filter,
            group_by,
            value,
            comparison,
            bound,
        } => {
            let mut groups: BTreeMap<Option<Value>, Value> = BTreeMap::new();
            let zero = match op {
                AggregateOp::Sum => zero_of(&value.ty)?,
                AggregateOp::Count => Value::U64(0),
            };
            // groups present in rows
            for (k, row) in post.table(*record) {
                env.bindings
                    .insert(*var, row_value(module, *record, k, row));
                if let Some(f) = filter {
                    if !env.eval(f, post, post)?.as_bool()? {
                        continue;
                    }
                }
                let g = match group_by {
                    Some(ge) => Some(env.eval(ge, post, post)?),
                    None => None,
                };
                let contrib = match op {
                    AggregateOp::Sum => env.eval(value, post, post)?,
                    AggregateOp::Count => Value::U64(1),
                };
                let acc = groups.entry(g).or_insert_with(|| zero.clone());
                *acc = checked_arith(BinOp::Add, acc, &contrib)?;
            }
            if group_by.is_none() {
                groups.entry(None).or_insert(zero.clone());
            }
            for (g, total) in groups {
                env.group_key = g.clone();
                let b = env.eval(bound, post, post)?;
                if !compare(*comparison, &total, &b)? {
                    out.push(Violation {
                        invariant: inv.id,
                        instance: g.clone(),
                        detail: format!("aggregate {total:?} violates bound {b:?}"),
                    });
                }
            }
        }
        PredicateIR::TransitionPredicate {
            record,
            field,
            edges,
            ..
        } => {
            for (k, new_row) in post.table(*record) {
                if let Some(old_row) = pre.get(*record, k) {
                    let (o, n) = (&old_row[field], &new_row[field]);
                    if o != n {
                        let (ov, nv) = match (o, n) {
                            (Value::Enum { variant: ov, .. }, Value::Enum { variant: nv, .. }) => {
                                (*ov, *nv)
                            }
                            _ => continue,
                        };
                        if !edges.contains(&(ov, nv)) {
                            out.push(Violation {
                                invariant: inv.id,
                                instance: Some(k.clone()),
                                detail: format!("illegal transition {ov} -> {nv}"),
                            });
                        }
                    }
                }
            }
        }
        PredicateIR::RequiresFact { .. } => {
            // checked at invocation time against the causal past (evaluate()); nothing to check on a state pair
        }
    }
    Ok(())
}

/// Convenience: check that `state` satisfies every state invariant (no transition context).
pub fn state_valid(module: &ModuleIR, state: &State) -> CoreResult<Vec<Violation>> {
    check_invariants(module, state, state)
}
