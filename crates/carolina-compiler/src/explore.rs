//! Bounded small-state generation and concurrent pair exploration (SPEC-004 §5, §7.2; SPEC-001 §60).
//!
//! Generated states and arguments are deterministic functions of the module and budget.
//! A counterexample found here is replayed by the reference semantics before it becomes
//! `Disproven`. Exhausting the space without a counterexample is `Unknown`, never `Proven`.

use std::collections::BTreeMap;

use carolina_core::decimal::Decimal;
use carolina_core::error::CoreResult;
use carolina_core::ids::{OperationRef, RecordId};
use carolina_lang::counterexample::{explore_concurrent, replay, Counterexample, Invocation};
use carolina_lang::interp::{state_valid, State};
use carolina_lang::ir::ModuleIR;
use carolina_lang::types::{Type, Value};

use crate::input::DeterministicBudget;

fn uuid(n: u8) -> Value {
    let mut a = [0u8; 16];
    a[15] = n;
    Value::Uuid(a)
}

/// Deterministic value pool for a type (bounded by `max_values`).
pub fn value_pool(module: &ModuleIR, ty: &Type, max_values: usize) -> Vec<Value> {
    let v: Vec<Value> = match ty {
        Type::Bool => vec![Value::Bool(false), Value::Bool(true)],
        Type::I64 => vec![Value::I64(0), Value::I64(1), Value::I64(2), Value::I64(4)],
        Type::U64 => vec![Value::U64(0), Value::U64(1), Value::U64(2), Value::U64(4)],
        Type::Uuid => vec![uuid(1), uuid(2), uuid(3)],
        Type::Bytes(_) => vec![Value::Bytes(vec![1]), Value::Bytes(vec![2])],
        Type::String(_) => vec![Value::Str("a".into()), Value::Str("b".into())],
        Type::Decimal(p, s) => [0i128, 1, 2, 4]
            .iter()
            .map(|k| Value::Decimal(Decimal::new(k * 10i128.pow(*s as u32), *p, *s).unwrap()))
            .collect(),
        Type::Enum(e) => {
            let n = module.enum_def(*e).map(|d| d.variants.len()).unwrap_or(1);
            (0..n as u32)
                .map(|variant| Value::Enum {
                    enum_id: *e,
                    variant,
                })
                .collect()
        }
        Type::Option(t) => {
            let mut out = vec![Value::None];
            out.extend(
                value_pool(module, t, max_values)
                    .into_iter()
                    .take(1)
                    .map(|v| Value::Some(Box::new(v))),
            );
            out
        }
        Type::Set(t) => {
            let mut out = vec![Value::Set(Default::default())];
            if let Some(x) = value_pool(module, t, max_values).into_iter().next() {
                out.push(Value::Set([x].into_iter().collect()));
            }
            out
        }
        Type::Tuple(ts) => {
            let pools: Vec<Vec<Value>> = ts.iter().map(|t| value_pool(module, t, 2)).collect();
            cartesian(&pools).into_iter().map(Value::Tuple).collect()
        }
        Type::Row(_) | Type::Struct(_) | Type::Unit => vec![Value::Unit],
    };
    v.into_iter().take(max_values.max(1)).collect()
}

fn cartesian(pools: &[Vec<Value>]) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = vec![vec![]];
    for p in pools {
        let mut next = Vec::new();
        for prefix in &out {
            for v in p {
                let mut c = prefix.clone();
                c.push(v.clone());
                next.push(c);
            }
        }
        out = next;
    }
    out
}

/// Argument vectors for an operation, bounded by the budget (product capped at 64).
pub fn argument_pool(
    module: &ModuleIR,
    op: OperationRef,
    budget: &DeterministicBudget,
) -> CoreResult<Vec<Vec<Value>>> {
    let o = module.operation(op)?;
    let pools: Vec<Vec<Value>> = o
        .parameters
        .iter()
        .map(|p| value_pool(module, &p.ty, budget.max_values_per_param))
        .collect();
    Ok(cartesian(&pools).into_iter().take(64).collect())
}

/// Deterministic small states: each record gets up to `max_rows_per_record` rows keyed from the
/// key pool; non-key fields enumerate per-row values from the field pool. Only invariant-valid
/// states are returned. The enumeration is capped at `max_states`.
pub fn state_pool(
    module: &ModuleIR,
    records: &[RecordId],
    budget: &DeterministicBudget,
    max_states: usize,
) -> CoreResult<Vec<State>> {
    // Build per-record row candidates: for each key, the list of possible rows.
    struct RecPlan {
        record: RecordId,
        rows_per_key: Vec<(Value, Vec<BTreeMap<carolina_core::ids::FieldId, Value>>)>,
    }
    let mut plans = Vec::new();
    for rid in records {
        let rec = module.record(*rid)?;
        let pk = rec.fields.iter().find(|f| f.id == rec.primary_key).unwrap();
        let keys = value_pool(module, &pk.ty, budget.max_rows_per_record);
        let mut rows_per_key = Vec::new();
        for k in keys {
            let mut field_pools: Vec<(carolina_core::ids::FieldId, Vec<Value>)> = Vec::new();
            for f in &rec.fields {
                if f.id == rec.primary_key {
                    continue;
                }
                // key-typed reference fields (Uuid/U64/tuples) use the key pool so references can resolve
                let pool = value_pool(module, &f.ty, 4);
                field_pools.push((f.id, pool));
            }
            let combos = cartesian(
                &field_pools
                    .iter()
                    .map(|(_, p)| p.clone())
                    .collect::<Vec<_>>(),
            );
            let rows: Vec<BTreeMap<_, _>> = combos
                .into_iter()
                .take(16)
                .map(|vals| {
                    let mut row = BTreeMap::new();
                    row.insert(rec.primary_key, k.clone());
                    for ((fid, _), v) in field_pools.iter().zip(vals) {
                        row.insert(*fid, v);
                    }
                    row
                })
                .collect();
            rows_per_key.push((k, rows));
        }
        plans.push(RecPlan {
            record: *rid,
            rows_per_key,
        });
    }
    // Enumerate: for each record, for each key: absent or one of its rows.
    type Slot = (
        RecordId,
        Value,
        Vec<BTreeMap<carolina_core::ids::FieldId, Value>>,
    );
    let mut slots: Vec<Slot> = Vec::new();
    for p in &plans {
        for (k, rows) in &p.rows_per_key {
            slots.push((p.record, k.clone(), rows.clone()));
        }
    }
    let mut states = Vec::new();
    let mut counters = vec![0usize; slots.len()]; // 0 = absent, i>0 = rows[i-1]
    let mut enumerated = 0usize;
    loop {
        enumerated += 1;
        if enumerated > 20_000 {
            break;
        }
        let mut st = State::default();
        for (i, (rid, k, rows)) in slots.iter().enumerate() {
            let c = counters[i];
            if c > 0 {
                st.insert(*rid, k.clone(), rows[c - 1].clone());
            }
        }
        if state_valid(module, &st)?.is_empty() {
            states.push(st);
            if states.len() >= max_states {
                break;
            }
        }
        // increment mixed radix
        let mut i = 0;
        loop {
            if i >= slots.len() {
                return Ok(states);
            }
            counters[i] += 1;
            if counters[i] <= slots[i].2.len() {
                break;
            }
            counters[i] = 0;
            i += 1;
        }
    }
    Ok(states)
}

/// Result of exploring one ordered operation pair.
pub struct PairExploration {
    pub counterexample: Option<Counterexample>,
    pub explored: u64,
    pub budget_exceeded: bool,
}

/// Explore concurrent acceptance of `(a, b)` over generated states and arguments.
pub fn explore_pair(
    module: &ModuleIR,
    a: OperationRef,
    b: OperationRef,
    records: &[RecordId],
    budget: &DeterministicBudget,
    steps: &mut u64,
) -> CoreResult<PairExploration> {
    let states = state_pool(module, records, budget, 48)?;
    let args_a = argument_pool(module, a, budget)?;
    let args_b = argument_pool(module, b, budget)?;
    let mut explored = 0u64;
    for st in &states {
        for aa in &args_a {
            for ab in &args_b {
                *steps += 1;
                explored += 1;
                if *steps > budget.max_steps {
                    return Ok(PairExploration {
                        counterexample: None,
                        explored,
                        budget_exceeded: true,
                    });
                }
                let invs = vec![
                    Invocation {
                        operation: a,
                        arguments: aa.clone(),
                    },
                    Invocation {
                        operation: b,
                        arguments: ab.clone(),
                    },
                ];
                if let Some(cx) = explore_concurrent(module, st, &invs)? {
                    if replay(module, &cx)? {
                        return Ok(PairExploration {
                            counterexample: Some(cx),
                            explored,
                            budget_exceeded: false,
                        });
                    }
                }
            }
        }
    }
    Ok(PairExploration {
        counterexample: None,
        explored,
        budget_exceeded: false,
    })
}
