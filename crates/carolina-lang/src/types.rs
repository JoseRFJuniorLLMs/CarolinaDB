//! Types and values of the restricted language (SPEC-003 §4).
//!
//! Values are totally ordered so they can be set elements and canonical keys.
//! Decimals compare by coefficient within one `(precision, scale)`; mixing types
//! in one set is prevented by the type checker.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::decimal::Decimal;
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::{EnumId, RecordId};
use carolina_core::keycodec::KeyWriter;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Bool,
    I64,
    U64,
    Uuid,
    Bytes(u32),
    String(u32),
    Decimal(u8, u8),
    Enum(EnumId),
    Option(Box<Type>),
    Set(Box<Type>),
    Tuple(Vec<Type>),
    /// A row of the given record (value of a `READ` binding or `Record[key]`).
    Row(RecordId),
    /// Anonymous result struct `{ a: T, b: U }` (fields sorted by name).
    Struct(Vec<(String, Type)>),
    Unit,
}

impl Type {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::I64 | Type::U64 | Type::Decimal(_, _))
    }
    pub fn is_comparable(&self) -> bool {
        match self {
            Type::Option(inner) => inner.is_comparable(),
            Type::Tuple(items) => items.iter().all(Type::is_comparable),
            Type::Set(_) | Type::Row(_) | Type::Struct(_) | Type::Unit => false,
            _ => true,
        }
    }
    /// Receipts contain explicitly projected, bounded values, including nested ones.
    pub fn is_result_type(&self) -> bool {
        match self {
            Type::Option(inner) => inner.is_result_type(),
            Type::Tuple(items) => items.iter().all(Type::is_result_type),
            Type::Struct(fields) => fields.iter().all(|(_, ty)| ty.is_result_type()),
            Type::Row(_) | Type::Set(_) => false,
            _ => true,
        }
    }
    pub fn describe(&self) -> String {
        match self {
            Type::Bool => "Bool".into(),
            Type::I64 => "I64".into(),
            Type::U64 => "U64".into(),
            Type::Uuid => "Uuid".into(),
            Type::Bytes(n) => format!("Bytes({n})"),
            Type::String(n) => format!("String({n})"),
            Type::Decimal(p, s) => format!("Decimal({p},{s})"),
            Type::Enum(e) => format!("Enum#{}", e.0),
            Type::Option(t) => format!("Option<{}>", t.describe()),
            Type::Set(t) => format!("Set<{}>", t.describe()),
            Type::Tuple(ts) => format!(
                "({})",
                ts.iter()
                    .map(|t| t.describe())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Type::Row(r) => format!("Row<Record#{}>", r.0),
            Type::Struct(fs) => format!(
                "{{{}}}",
                fs.iter()
                    .map(|(n, t)| format!("{n}: {}", t.describe()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Type::Unit => "Unit".into(),
        }
    }
}

impl Canonical for Type {
    fn to_canon(&self) -> CanonValue {
        match self {
            Type::Bool => CanonValue::obj().fstr("kind", "bool").build(),
            Type::I64 => CanonValue::obj().fstr("kind", "i64").build(),
            Type::U64 => CanonValue::obj().fstr("kind", "u64").build(),
            Type::Uuid => CanonValue::obj().fstr("kind", "uuid").build(),
            Type::Bytes(n) => CanonValue::obj()
                .fstr("kind", "bytes")
                .fu32("max_len", *n)
                .build(),
            Type::String(n) => CanonValue::obj()
                .fstr("kind", "string")
                .fu32("max_utf8_bytes", *n)
                .build(),
            Type::Decimal(p, s) => CanonValue::obj()
                .fstr("kind", "decimal")
                .fu32("precision", *p as u32)
                .fu32("scale", *s as u32)
                .build(),
            Type::Enum(e) => CanonValue::obj().fc("enum", e).fstr("kind", "enum").build(),
            Type::Option(t) => CanonValue::obj()
                .fc("inner", t.as_ref())
                .fstr("kind", "option")
                .build(),
            Type::Set(t) => CanonValue::obj()
                .fc("element", t.as_ref())
                .fstr("kind", "set")
                .build(),
            Type::Tuple(ts) => CanonValue::obj()
                .fvec("items", ts)
                .fstr("kind", "tuple")
                .build(),
            Type::Row(r) => CanonValue::obj()
                .fstr("kind", "row")
                .fc("record", r)
                .build(),
            Type::Struct(fs) => {
                let mut o = BTreeMap::new();
                for (n, t) in fs {
                    o.insert(n.clone(), t.to_canon());
                }
                CanonValue::obj()
                    .f("fields", CanonValue::Object(o))
                    .fstr("kind", "struct")
                    .build()
            }
            Type::Unit => CanonValue::obj().fstr("kind", "unit").build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "bool" => Type::Bool,
            "i64" => Type::I64,
            "u64" => Type::U64,
            "uuid" => Type::Uuid,
            "bytes" => Type::Bytes(v.field("max_len")?.as_u32()?),
            "string" => Type::String(v.field("max_utf8_bytes")?.as_u32()?),
            "decimal" => Type::Decimal(
                v.field("precision")?.as_u32()? as u8,
                v.field("scale")?.as_u32()? as u8,
            ),
            "enum" => Type::Enum(EnumId::from_canon(v.field("enum")?)?),
            "option" => Type::Option(Box::new(Type::from_canon(v.field("inner")?)?)),
            "set" => Type::Set(Box::new(Type::from_canon(v.field("element")?)?)),
            "tuple" => Type::Tuple(Vec::<Type>::from_canon(v.field("items")?)?),
            "row" => Type::Row(RecordId::from_canon(v.field("record")?)?),
            "struct" => {
                let mut fs = Vec::new();
                for (n, t) in v.field("fields")?.as_object()? {
                    fs.push((n.clone(), Type::from_canon(t)?));
                }
                Type::Struct(fs)
            }
            "unit" => Type::Unit,
            other => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("unknown type kind `{other}`"),
                ))
            }
        })
    }
}

/// Runtime/literal value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    I64(i64),
    U64(u64),
    Uuid([u8; 16]),
    Bytes(Vec<u8>),
    Str(String),
    Decimal(Decimal),
    Enum {
        enum_id: EnumId,
        variant: u32,
    },
    None,
    Some(Box<Value>),
    Set(BTreeSet<Value>),
    Tuple(Vec<Value>),
    Row {
        record: RecordId,
        fields: BTreeMap<String, Value>,
    },
    Struct(BTreeMap<String, Value>),
    Unit,
}

fn rank(v: &Value) -> u8 {
    match v {
        Value::Unit => 0,
        Value::Bool(_) => 1,
        Value::I64(_) => 2,
        Value::U64(_) => 3,
        Value::Uuid(_) => 4,
        Value::Bytes(_) => 5,
        Value::Str(_) => 6,
        Value::Decimal(_) => 7,
        Value::Enum { .. } => 8,
        Value::None => 9,
        Value::Some(_) => 10,
        Value::Set(_) => 11,
        Value::Tuple(_) => 12,
        Value::Row { .. } => 13,
        Value::Struct(_) => 14,
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        use Value::*;
        match (self, other) {
            (Bool(a), Bool(b)) => a.cmp(b),
            (I64(a), I64(b)) => a.cmp(b),
            (U64(a), U64(b)) => a.cmp(b),
            (Uuid(a), Uuid(b)) => a.cmp(b),
            (Bytes(a), Bytes(b)) => a.cmp(b),
            (Str(a), Str(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Decimal(a), Decimal(b)) => (a.scale(), a.precision(), a.coefficient()).cmp(&(
                b.scale(),
                b.precision(),
                b.coefficient(),
            )),
            (
                Enum {
                    enum_id: e1,
                    variant: v1,
                },
                Enum {
                    enum_id: e2,
                    variant: v2,
                },
            ) => (e1, v1).cmp(&(e2, v2)),
            (Some(a), Some(b)) => a.cmp(b),
            (Set(a), Set(b)) => a.cmp(b),
            (Tuple(a), Tuple(b)) => a.cmp(b),
            (
                Row {
                    record: r1,
                    fields: f1,
                },
                Row {
                    record: r2,
                    fields: f2,
                },
            ) => (r1, f1).cmp(&(r2, f2)),
            (Struct(a), Struct(b)) => a.cmp(b),
            _ => rank(self).cmp(&rank(other)),
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Value {
    pub fn type_of(&self) -> Option<Type> {
        Some(match self {
            Value::Bool(_) => Type::Bool,
            Value::I64(_) => Type::I64,
            Value::U64(_) => Type::U64,
            Value::Uuid(_) => Type::Uuid,
            Value::Decimal(d) => Type::Decimal(d.precision(), d.scale()),
            Value::Enum { enum_id, .. } => Type::Enum(*enum_id),
            Value::Unit => Type::Unit,
            Value::Row { record, .. } => Type::Row(*record),
            _ => return None, // needs declared type context (lengths, element types)
        })
    }

    pub fn as_bool(&self) -> CoreResult<bool> {
        match self {
            Value::Bool(b) => Ok(*b),
            _ => Err(CoreError::new(
                ErrorCode::TypeMismatch,
                "expected Bool value",
            )),
        }
    }

    /// Order-preserving key bytes (SPEC-002 §11) for use as a primary/unique key component.
    pub fn write_key(&self, w: KeyWriter) -> CoreResult<KeyWriter> {
        Ok(match self {
            Value::Bool(b) => w.bool(*b),
            Value::I64(v) => w.i64(*v),
            Value::U64(v) => w.u64(*v),
            Value::Uuid(u) => w.id16(u),
            Value::Bytes(b) => w.bytes(b),
            Value::Str(s) => w.str(s),
            Value::Decimal(d) => w.decimal(d.coefficient()),
            Value::Enum { variant, .. } => w.enum_variant(*variant),
            Value::None => w.none(),
            Value::Some(v) => v.write_key(w.some_marker())?,
            Value::Tuple(items) => {
                let mut w = w.tuple_header(items.len() as u16);
                for it in items {
                    w = it.write_key(w)?;
                }
                w
            }
            Value::Set(_) | Value::Row { .. } | Value::Struct(_) | Value::Unit => {
                return Err(CoreError::new(
                    ErrorCode::TypeMismatch,
                    "value cannot be a key component",
                ));
            }
        })
    }

    pub fn key_bytes(&self) -> CoreResult<Vec<u8>> {
        Ok(self.write_key(KeyWriter::new())?.finish())
    }

    /// Validate a value against its declared type (lengths, precision, element types).
    pub fn check_type(&self, ty: &Type) -> CoreResult<()> {
        let bad = || {
            CoreError::new(
                ErrorCode::TypeMismatch,
                format!("value {self:?} does not match type {}", ty.describe()),
            )
        };
        match (self, ty) {
            (Value::Bool(_), Type::Bool)
            | (Value::I64(_), Type::I64)
            | (Value::U64(_), Type::U64)
            | (Value::Uuid(_), Type::Uuid)
            | (Value::Unit, Type::Unit) => Ok(()),
            (Value::Bytes(b), Type::Bytes(n)) => {
                if b.len() > *n as usize {
                    Err(CoreError::new(
                        ErrorCode::TypeMismatch,
                        "bytes exceed declared length",
                    ))
                } else {
                    Ok(())
                }
            }
            (Value::Str(s), Type::String(n)) => {
                if s.len() > *n as usize {
                    Err(CoreError::new(
                        ErrorCode::TypeMismatch,
                        "string exceeds declared UTF-8 length",
                    ))
                } else {
                    Ok(())
                }
            }
            (Value::Decimal(d), Type::Decimal(p, s)) => {
                if d.precision() == *p && d.scale() == *s {
                    Ok(())
                } else {
                    Err(bad())
                }
            }
            (Value::Enum { enum_id, .. }, Type::Enum(e)) => {
                if enum_id == e {
                    Ok(())
                } else {
                    Err(bad())
                }
            }
            (Value::None, Type::Option(_)) => Ok(()),
            (Value::Some(v), Type::Option(t)) => v.check_type(t),
            (Value::Set(items), Type::Set(t)) => items.iter().try_for_each(|v| v.check_type(t)),
            (Value::Tuple(items), Type::Tuple(ts)) => {
                if items.len() != ts.len() {
                    return Err(bad());
                }
                items.iter().zip(ts).try_for_each(|(v, t)| v.check_type(t))
            }
            (Value::Row { record, .. }, Type::Row(r)) => {
                if record == r {
                    Ok(())
                } else {
                    Err(bad())
                }
            }
            (Value::Struct(fs), Type::Struct(ts)) => {
                if fs.len() != ts.len() {
                    return Err(bad());
                }
                for (n, t) in ts {
                    match fs.get(n) {
                        Some(v) => v.check_type(t)?,
                        None => return Err(bad()),
                    }
                }
                Ok(())
            }
            _ => Err(bad()),
        }
    }
}

impl Canonical for Value {
    fn to_canon(&self) -> CanonValue {
        match self {
            Value::Bool(b) => CanonValue::obj()
                .fstr("kind", "bool")
                .fbool("value", *b)
                .build(),
            Value::I64(v) => CanonValue::obj()
                .fstr("kind", "i64")
                .f("value", CanonValue::int(*v as i128))
                .build(),
            Value::U64(v) => CanonValue::obj()
                .fstr("kind", "u64")
                .fu64("value", *v)
                .build(),
            Value::Uuid(u) => CanonValue::obj()
                .fstr("kind", "uuid")
                .fbytes("value", u)
                .build(),
            Value::Bytes(b) => CanonValue::obj()
                .fstr("kind", "bytes")
                .fbytes("value", b)
                .build(),
            Value::Str(s) => CanonValue::obj()
                .fstr("kind", "string")
                .fstr("value", s)
                .build(),
            Value::Decimal(d) => CanonValue::obj()
                .f("coefficient", CanonValue::int(d.coefficient()))
                .fstr("kind", "decimal")
                .fu32("precision", d.precision() as u32)
                .fu32("scale", d.scale() as u32)
                .build(),
            Value::Enum { enum_id, variant } => CanonValue::obj()
                .fc("enum", enum_id)
                .fstr("kind", "enum")
                .fu32("variant", *variant)
                .build(),
            Value::None => CanonValue::obj().fstr("kind", "none").build(),
            Value::Some(v) => CanonValue::obj()
                .fstr("kind", "some")
                .fc("value", v.as_ref())
                .build(),
            Value::Set(items) => {
                let items: Vec<CanonValue> = items.iter().map(|v| v.to_canon()).collect();
                CanonValue::obj()
                    .f(
                        "items",
                        CanonValue::set(items).expect("set values are unique"),
                    )
                    .fstr("kind", "set")
                    .build()
            }
            Value::Tuple(items) => CanonValue::obj()
                .fvec("items", items)
                .fstr("kind", "tuple")
                .build(),
            Value::Row { record, fields } => {
                let mut o = BTreeMap::new();
                for (k, v) in fields {
                    o.insert(k.clone(), v.to_canon());
                }
                CanonValue::obj()
                    .f("fields", CanonValue::Object(o))
                    .fstr("kind", "row")
                    .fc("record", record)
                    .build()
            }
            Value::Struct(fields) => {
                let mut o = BTreeMap::new();
                for (k, v) in fields {
                    o.insert(k.clone(), v.to_canon());
                }
                CanonValue::obj()
                    .f("fields", CanonValue::Object(o))
                    .fstr("kind", "struct")
                    .build()
            }
            Value::Unit => CanonValue::obj().fstr("kind", "unit").build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "bool" => {
                v.expect_fields(&["kind", "value"])?;
                Value::Bool(v.field("value")?.as_bool()?)
            }
            "i64" => {
                v.expect_fields(&["kind", "value"])?;
                Value::I64(v.field("value")?.as_i64()?)
            }
            "u64" => {
                v.expect_fields(&["kind", "value"])?;
                Value::U64(v.field("value")?.as_u64()?)
            }
            "uuid" => {
                v.expect_fields(&["kind", "value"])?;
                let b = v.field("value")?.as_bytes()?;
                if b.len() != 16 {
                    return Err(CoreError::new(
                        ErrorCode::NonCanonicalEncoding,
                        "uuid must be 16 bytes",
                    ));
                }
                let mut a = [0u8; 16];
                a.copy_from_slice(&b);
                Value::Uuid(a)
            }
            "bytes" => {
                v.expect_fields(&["kind", "value"])?;
                Value::Bytes(v.field("value")?.as_bytes()?)
            }
            "string" => {
                v.expect_fields(&["kind", "value"])?;
                Value::Str(v.field("value")?.as_str()?.to_string())
            }
            "decimal" => {
                v.expect_fields(&["coefficient", "kind", "precision", "scale"])?;
                Value::Decimal(Decimal::new(
                    v.field("coefficient")?.as_i128()?,
                    v.field("precision")?.as_u32()? as u8,
                    v.field("scale")?.as_u32()? as u8,
                )?)
            }
            "enum" => {
                v.expect_fields(&["enum", "kind", "variant"])?;
                Value::Enum {
                    enum_id: EnumId::from_canon(v.field("enum")?)?,
                    variant: v.field("variant")?.as_u32()?,
                }
            }
            "none" => {
                v.expect_fields(&["kind"])?;
                Value::None
            }
            "some" => {
                v.expect_fields(&["kind", "value"])?;
                Value::Some(Box::new(Value::from_canon(v.field("value")?)?))
            }
            "set" => {
                v.expect_fields(&["items", "kind"])?;
                let items = v.field("items")?.as_set()?;
                Value::Set(
                    items
                        .iter()
                        .map(Value::from_canon)
                        .collect::<CoreResult<_>>()?,
                )
            }
            "tuple" => {
                v.expect_fields(&["items", "kind"])?;
                Value::Tuple(Vec::<Value>::from_canon(v.field("items")?)?)
            }
            "row" => {
                v.expect_fields(&["fields", "kind", "record"])?;
                let mut fields = BTreeMap::new();
                for (k, fv) in v.field("fields")?.as_object()? {
                    fields.insert(k.clone(), Value::from_canon(fv)?);
                }
                Value::Row {
                    record: RecordId::from_canon(v.field("record")?)?,
                    fields,
                }
            }
            "struct" => {
                v.expect_fields(&["fields", "kind"])?;
                let mut fields = BTreeMap::new();
                for (k, fv) in v.field("fields")?.as_object()? {
                    fields.insert(k.clone(), Value::from_canon(fv)?);
                }
                Value::Struct(fields)
            }
            "unit" => {
                v.expect_fields(&["kind"])?;
                Value::Unit
            }
            other => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("unknown value kind `{other}`"),
                ))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carolina_core::limits::Limits;

    #[test]
    fn value_canonical_roundtrip() {
        let mut fields = BTreeMap::new();
        fields.insert("a".into(), Value::I64(-3));
        fields.insert(
            "b".into(),
            Value::Set(
                [Value::Str("x".into()), Value::Str("a".into())]
                    .into_iter()
                    .collect(),
            ),
        );
        let v = Value::Struct(fields);
        let bytes = v.encode();
        assert_eq!(Value::decode(&bytes, &Limits::v1()).unwrap(), v);
        let d = Value::Decimal(Decimal::parse("1.50", 10, 2).unwrap());
        assert_eq!(Value::decode(&d.encode(), &Limits::v1()).unwrap(), d);
    }

    #[test]
    fn key_bytes_order() {
        let a = Value::Tuple(vec![Value::Uuid([0; 16]), Value::I64(1)])
            .key_bytes()
            .unwrap();
        let b = Value::Tuple(vec![Value::Uuid([0; 16]), Value::I64(2)])
            .key_bytes()
            .unwrap();
        assert!(a < b);
        assert!(Value::Set(BTreeSet::new()).key_bytes().is_err());
    }

    #[test]
    fn type_check_lengths() {
        assert!(Value::Str("abcd".into())
            .check_type(&Type::String(3))
            .is_err());
        assert!(Value::Str("abc".into())
            .check_type(&Type::String(3))
            .is_ok());
        assert!(Value::Some(Box::new(Value::I64(1)))
            .check_type(&Type::Option(Box::new(Type::U64)))
            .is_err());
    }
}
