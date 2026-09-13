//! Independent W1 inventory oracle (SPEC-010 §4, §11).
//!
//! This model is written from the fixture contract `inventory_reserve_release.cdl` by hand. It does
//! not import the DSL interpreter, the compiler's classification or the runtime's row codec; it
//! shares only the canonical `Value` encoding of results (a codec, permitted by SPEC-010 §3) so
//! that exact result bytes can be compared digest-to-digest.
//!
//! Contract restated (all quantities `I64`, checked):
//! * `reserve(item, q, rid)`: q > 0; Item[item] must exist; rid must be new; available -= q,
//!   reserved += q; ENSURE available >= 0; result `{item, quantity: q, reservation: rid}`.
//! * `release(item, q, rid)`: q > 0; Reservation[rid] must exist, belong to `item`, hold exactly
//!   `q` and be ACTIVE; reserved -= q, available += q; state := RELEASED; result `{item, reservation}`.
//! * `consume(item, q, rid)`: q > 0; Reservation[rid] must exist and be ACTIVE → CONSUMED;
//!   reserved -= q, total -= q; ENSURE amount == q and resource == item; result
//!   `{item, quantity, reservation}`.
//! * `supply(item, q)`: q > 0; Item[item] must exist; available += q, total += q; result
//!   `{item, quantity}`.
//! * invariants after every accepted invocation: available >= 0, reserved >= 0,
//!   available + reserved == total.
//! * an invocation is atomic: a rejection leaves the state unchanged.

use std::collections::BTreeMap;

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{hex_encode, sha256, Hash256};
use carolina_lang::types::Value;

pub type Id = [u8; 16];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResState {
    Active,
    Consumed,
    Released,
}

impl ResState {
    /// Variant index in the fixture's `ENUM ReservationState { Active, Consumed, Released }`.
    pub fn variant(&self) -> u32 {
        match self {
            ResState::Active => 0,
            ResState::Consumed => 1,
            ResState::Released => 2,
        }
    }
    pub fn from_variant(v: u32) -> Option<ResState> {
        match v {
            0 => Some(ResState::Active),
            1 => Some(ResState::Consumed),
            2 => Some(ResState::Released),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemState {
    pub available: i64,
    pub reserved: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reservation {
    pub item: Id,
    pub amount: i64,
    pub state: ResState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventoryModel {
    pub items: BTreeMap<Id, ItemState>,
    pub reservations: BTreeMap<Id, Reservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum W1Op {
    Reserve { item: Id, q: i64, rid: Id },
    Release { item: Id, q: i64, rid: Id },
    Consume { item: Id, q: i64, rid: Id },
    Supply { item: Id, q: i64 },
}

impl W1Op {
    pub fn name(&self) -> &'static str {
        match self {
            W1Op::Reserve { .. } => "reserve",
            W1Op::Release { .. } => "release",
            W1Op::Consume { .. } => "consume",
            W1Op::Supply { .. } => "supply",
        }
    }
    /// Typed arguments in declaration order.
    pub fn args(&self) -> Vec<Value> {
        match self {
            W1Op::Reserve { item, q, rid }
            | W1Op::Release { item, q, rid }
            | W1Op::Consume { item, q, rid } => {
                vec![Value::Uuid(*item), Value::I64(*q), Value::Uuid(*rid)]
            }
            W1Op::Supply { item, q } => vec![Value::Uuid(*item), Value::I64(*q)],
        }
    }
    pub fn describe(&self) -> String {
        match self {
            W1Op::Reserve { item, q, rid } => {
                format!("reserve({}, {q}, {})", short(item), short(rid))
            }
            W1Op::Release { item, q, rid } => {
                format!("release({}, {q}, {})", short(item), short(rid))
            }
            W1Op::Consume { item, q, rid } => {
                format!("consume({}, {q}, {})", short(item), short(rid))
            }
            W1Op::Supply { item, q } => format!("supply({}, {q})", short(item)),
        }
    }
}

fn short(id: &Id) -> String {
    hex_encode(&id[..2])
}

fn id_field(v: &CanonValue, name: &str) -> CoreResult<Id> {
    let b = v.field(name)?.as_bytes()?;
    b.try_into()
        .map_err(|_| CoreError::new(ErrorCode::NonCanonicalEncoding, "id width"))
}

fn i64_field(v: &CanonValue, name: &str) -> CoreResult<i64> {
    v.field(name)?
        .as_str()?
        .parse::<i64>()
        .map_err(|_| CoreError::new(ErrorCode::NonCanonicalEncoding, "i64"))
}

impl Canonical for W1Op {
    fn to_canon(&self) -> CanonValue {
        match self {
            W1Op::Reserve { item, q, rid }
            | W1Op::Release { item, q, rid }
            | W1Op::Consume { item, q, rid } => CanonValue::obj()
                .fbytes("item", item)
                .fstr("kind", self.name())
                .f("q", CanonValue::Str(q.to_string()))
                .fbytes("rid", rid)
                .build(),
            W1Op::Supply { item, q } => CanonValue::obj()
                .fbytes("item", item)
                .fstr("kind", "supply")
                .f("q", CanonValue::Str(q.to_string()))
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let kind = v.field("kind")?.as_str()?.to_string();
        let item = id_field(v, "item")?;
        let q = i64_field(v, "q")?;
        Ok(match kind.as_str() {
            "reserve" => W1Op::Reserve {
                item,
                q,
                rid: id_field(v, "rid")?,
            },
            "release" => W1Op::Release {
                item,
                q,
                rid: id_field(v, "rid")?,
            },
            "consume" => W1Op::Consume {
                item,
                q,
                rid: id_field(v, "rid")?,
            },
            "supply" => W1Op::Supply { item, q },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("w1 op {k}"),
                ))
            }
        })
    }
}

/// What the contract requires the reply to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Committed(Value),
    Rejected(&'static str),
}

impl Expected {
    pub fn result_digest(&self) -> Option<Hash256> {
        match self {
            Expected::Committed(v) => Some(sha256(&v.encode())),
            Expected::Rejected(_) => None,
        }
    }
    pub fn is_committed(&self) -> bool {
        matches!(self, Expected::Committed(_))
    }
}

fn st(fields: Vec<(&str, Value)>) -> Value {
    Value::Struct(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

impl InventoryModel {
    pub fn seed(items: &[(Id, i64)]) -> InventoryModel {
        let mut m = InventoryModel::default();
        for (id, available) in items {
            m.items.insert(
                *id,
                ItemState {
                    available: *available,
                    reserved: 0,
                    total: *available,
                },
            );
        }
        m
    }

    pub fn invariants(&self) -> Result<(), String> {
        for (id, it) in &self.items {
            if it.available < 0 {
                return Err(format!("available_nonneg violated for {}", short(id)));
            }
            if it.reserved < 0 {
                return Err(format!("reserved_nonneg violated for {}", short(id)));
            }
            match it.available.checked_add(it.reserved) {
                Some(s) if s == it.total => {}
                _ => return Err(format!("supply_conserved violated for {}", short(id))),
            }
        }
        for (rid, r) in &self.reservations {
            if r.amount <= 0 {
                return Err(format!(
                    "reservation {} has non-positive amount",
                    short(rid)
                ));
            }
        }
        Ok(())
    }

    /// Evaluate without applying: the expected reply and the post-state when committed.
    pub fn evaluate(&self, op: &W1Op) -> (Expected, Option<InventoryModel>) {
        let mut next = self.clone();
        let outcome = match op {
            W1Op::Reserve { item, q, rid } => (|| {
                if *q <= 0 {
                    return Expected::Rejected("precondition q > 0");
                }
                let it = match next.items.get_mut(item) {
                    Some(it) => it,
                    None => return Expected::Rejected("read Item[item] missing"),
                };
                let (Some(av), Some(rs)) =
                    (it.available.checked_sub(*q), it.reserved.checked_add(*q))
                else {
                    return Expected::Rejected("arithmetic overflow");
                };
                it.available = av;
                it.reserved = rs;
                if next.reservations.contains_key(rid) {
                    return Expected::Rejected("duplicate reservation identity");
                }
                next.reservations.insert(
                    *rid,
                    Reservation {
                        item: *item,
                        amount: *q,
                        state: ResState::Active,
                    },
                );
                if av < 0 {
                    return Expected::Rejected("postcondition available >= 0");
                }
                Expected::Committed(st(vec![
                    ("item", Value::Uuid(*item)),
                    ("quantity", Value::I64(*q)),
                    ("reservation", Value::Uuid(*rid)),
                ]))
            })(),
            W1Op::Release { item, q, rid } => (|| {
                if *q <= 0 {
                    return Expected::Rejected("precondition q > 0");
                }
                let r = match next.reservations.get(rid).copied() {
                    Some(r) => r,
                    None => return Expected::Rejected("read Reservation[rid] missing"),
                };
                if r.item != *item {
                    return Expected::Rejected("reservation belongs to another item");
                }
                if r.amount != *q {
                    return Expected::Rejected("whole-reservation release only");
                }
                if r.state != ResState::Active {
                    return Expected::Rejected("reservation not ACTIVE");
                }
                let it = match next.items.get_mut(item) {
                    Some(it) => it,
                    None => return Expected::Rejected("Item[item] missing"),
                };
                let (Some(rs), Some(av)) =
                    (it.reserved.checked_sub(*q), it.available.checked_add(*q))
                else {
                    return Expected::Rejected("arithmetic overflow");
                };
                it.reserved = rs;
                it.available = av;
                next.reservations.get_mut(rid).unwrap().state = ResState::Released;
                Expected::Committed(st(vec![
                    ("item", Value::Uuid(*item)),
                    ("reservation", Value::Uuid(*rid)),
                ]))
            })(),
            W1Op::Consume { item, q, rid } => (|| {
                if *q <= 0 {
                    return Expected::Rejected("precondition q > 0");
                }
                let r = match next.reservations.get(rid).copied() {
                    Some(r) => r,
                    None => return Expected::Rejected("read Reservation[rid] missing"),
                };
                if r.state != ResState::Active {
                    return Expected::Rejected("ADVANCE expects ACTIVE");
                }
                next.reservations.get_mut(rid).unwrap().state = ResState::Consumed;
                let it = match next.items.get_mut(item) {
                    Some(it) => it,
                    None => return Expected::Rejected("Item[item] missing"),
                };
                let (Some(rs), Some(tt)) = (it.reserved.checked_sub(*q), it.total.checked_sub(*q))
                else {
                    return Expected::Rejected("arithmetic overflow");
                };
                it.reserved = rs;
                it.total = tt;
                if r.amount != *q || r.item != *item {
                    return Expected::Rejected("postcondition amount/resource");
                }
                Expected::Committed(st(vec![
                    ("item", Value::Uuid(*item)),
                    ("quantity", Value::I64(*q)),
                    ("reservation", Value::Uuid(*rid)),
                ]))
            })(),
            W1Op::Supply { item, q } => (|| {
                if *q <= 0 {
                    return Expected::Rejected("precondition q > 0");
                }
                let it = match next.items.get_mut(item) {
                    Some(it) => it,
                    None => return Expected::Rejected("read Item[item] missing"),
                };
                let (Some(av), Some(tt)) = (it.available.checked_add(*q), it.total.checked_add(*q))
                else {
                    return Expected::Rejected("arithmetic overflow");
                };
                it.available = av;
                it.total = tt;
                Expected::Committed(st(vec![
                    ("item", Value::Uuid(*item)),
                    ("quantity", Value::I64(*q)),
                ]))
            })(),
        };
        match outcome {
            Expected::Committed(_) => {
                if let Err(e) = next.invariants() {
                    let _ = e;
                    return (Expected::Rejected("invariant"), None);
                }
                (outcome, Some(next))
            }
            r => (r, None),
        }
    }

    /// Apply when the contract commits; a rejection leaves the model unchanged.
    pub fn apply(&mut self, op: &W1Op) -> Expected {
        let (e, next) = self.evaluate(op);
        if let Some(n) = next {
            *self = n;
        }
        e
    }

    /// Human-readable diff against another model (empty when equal).
    pub fn diff(&self, other: &InventoryModel) -> Vec<String> {
        let mut out = Vec::new();
        for (id, a) in &self.items {
            match other.items.get(id) {
                Some(b) if a == b => {}
                Some(b) => out.push(format!("item {}: {a:?} vs {b:?}", short(id))),
                None => out.push(format!("item {} missing on the other side", short(id))),
            }
        }
        for id in other.items.keys() {
            if !self.items.contains_key(id) {
                out.push(format!("item {} only on the other side", short(id)));
            }
        }
        for (id, a) in &self.reservations {
            match other.reservations.get(id) {
                Some(b) if a == b => {}
                Some(b) => out.push(format!("reservation {}: {a:?} vs {b:?}", short(id))),
                None => out.push(format!(
                    "reservation {} missing on the other side",
                    short(id)
                )),
            }
        }
        for id in other.reservations.keys() {
            if !self.reservations.contains_key(id) {
                out.push(format!("reservation {} only on the other side", short(id)));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oracle_semantics() {
        let item = [1u8; 16];
        let mut m = InventoryModel::seed(&[(item, 3)]);
        assert!(m
            .apply(&W1Op::Reserve {
                item,
                q: 2,
                rid: [9; 16]
            })
            .is_committed());
        assert!(!m
            .apply(&W1Op::Reserve {
                item,
                q: 2,
                rid: [8; 16]
            })
            .is_committed());
        assert_eq!(
            m.items[&item],
            ItemState {
                available: 1,
                reserved: 2,
                total: 3
            }
        );
        assert!(!m
            .apply(&W1Op::Release {
                item,
                q: 1,
                rid: [9; 16]
            })
            .is_committed());
        assert!(m
            .apply(&W1Op::Release {
                item,
                q: 2,
                rid: [9; 16]
            })
            .is_committed());
        assert!(!m
            .apply(&W1Op::Release {
                item,
                q: 2,
                rid: [9; 16]
            })
            .is_committed());
        assert_eq!(
            m.items[&item],
            ItemState {
                available: 3,
                reserved: 0,
                total: 3
            }
        );
        assert!(!m.apply(&W1Op::Supply { item, q: i64::MAX }).is_committed());
        assert!(m
            .apply(&W1Op::Reserve {
                item,
                q: 1,
                rid: [7; 16]
            })
            .is_committed());
        assert!(m
            .apply(&W1Op::Consume {
                item,
                q: 1,
                rid: [7; 16]
            })
            .is_committed());
        assert_eq!(
            m.items[&item],
            ItemState {
                available: 2,
                reserved: 0,
                total: 2
            }
        );
        m.invariants().unwrap();
        let op = W1Op::Consume {
            item,
            q: 1,
            rid: [7; 16],
        };
        assert_eq!(
            W1Op::decode(&op.encode(), &carolina_core::limits::Limits::v1()).unwrap(),
            op
        );
    }
}
