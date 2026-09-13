//! Semantic fixture families of SPEC-003 §12, embedded from `fixtures/dsl/*.cdl`.

use carolina_core::error::CoreResult;
use carolina_core::limits::Limits;

use crate::ir::ModuleIR;
use crate::lower::{lower_module, IdAllocation};
use crate::parser::parse_module;

pub const FIXTURE_NAMES: [&str; 7] = [
    "inventory_sell",
    "inventory_reserve_release",
    "account_transfer",
    "unique_username",
    "causal_ship",
    "causal_refund_extension",
    "increment_result",
];

pub fn fixture_source(name: &str) -> Option<&'static str> {
    Some(match name {
        "inventory_sell" => include_str!("../../../fixtures/dsl/inventory_sell.cdl"),
        "inventory_reserve_release" => {
            include_str!("../../../fixtures/dsl/inventory_reserve_release.cdl")
        }
        "account_transfer" => include_str!("../../../fixtures/dsl/account_transfer.cdl"),
        "unique_username" => include_str!("../../../fixtures/dsl/unique_username.cdl"),
        "causal_ship" => include_str!("../../../fixtures/dsl/causal_ship.cdl"),
        "causal_refund_extension" => {
            include_str!("../../../fixtures/dsl/causal_refund_extension.cdl")
        }
        "increment_result" => include_str!("../../../fixtures/dsl/increment_result.cdl"),
        _ => return None,
    })
}

pub fn load_fixture(name: &str) -> CoreResult<(ModuleIR, IdAllocation)> {
    let src = fixture_source(name).ok_or_else(|| {
        carolina_core::error::CoreError::new(
            carolina_core::error::ErrorCode::MissingRecord,
            format!("unknown fixture {name}"),
        )
    })?;
    let ast = parse_module(src, &Limits::v1())?;
    lower_module(&ast, None)
}

/// Text rendering of module hashes as frozen in `fixtures/golden/<name>.hashes.txt`.
pub fn render_hashes(h: &crate::ir::ModuleHashes) -> String {
    let mut hashes = format!(
        "schema_hash={}\nmodule_hash={}\n",
        h.schema_hash, h.module_hash
    );
    for (r, oh) in &h.operation_hashes {
        hashes.push_str(&format!(
            "operation_hash[{}@{}]={}\n",
            r.operation_id, r.version, oh
        ));
    }
    for (r, ch) in &h.contract_hashes {
        hashes.push_str(&format!(
            "contract_hash[{}@{}]={}\n",
            r.operation_id, r.version, ch
        ));
    }
    for (i, ih) in &h.invariant_hashes {
        hashes.push_str(&format!("invariant_hash[{}]={}\n", i, ih));
    }
    hashes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::counterexample::{explore_concurrent, replay, Failure, Invocation};
    use crate::interp::{evaluate, State};
    use crate::ir::{InvariantKind, ResultSemantics};
    use crate::types::Value;
    use carolina_core::canon::Canonical;
    use carolina_core::decimal::Decimal;
    use carolina_core::error::ErrorCode;
    use carolina_core::ids::OperationRef;
    use std::path::PathBuf;

    fn uuid(n: u8) -> Value {
        let mut a = [0u8; 16];
        a[15] = n;
        Value::Uuid(a)
    }

    fn op(m: &ModuleIR, name: &str) -> OperationRef {
        m.operation_by_name(name).unwrap().identity
    }

    fn golden_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden")
    }

    /// S003-A01/A02/A03: every fixture lowers; canonical bytes and hashes are frozen as golden files.
    #[test]
    fn all_fixtures_lower_and_match_golden() {
        let update = std::env::var("UPDATE_GOLDEN").is_ok();
        for name in FIXTURE_NAMES {
            let (ir, _) = load_fixture(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            let bytes = ir.encode();
            let hashes = render_hashes(&ir.hashes());
            let dir = golden_dir();
            let ir_path = dir.join(format!("{name}.module.json"));
            let hash_path = dir.join(format!("{name}.hashes.txt"));
            if update || !ir_path.exists() {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&ir_path, &bytes).unwrap();
                std::fs::write(&hash_path, &hashes).unwrap();
            }
            let golden = std::fs::read(&ir_path).unwrap();
            assert_eq!(
                golden, bytes,
                "golden IR bytes changed for {name}; set UPDATE_GOLDEN=1 to re-freeze"
            );
            let golden_h = std::fs::read_to_string(&hash_path)
                .unwrap()
                .replace("\r\n", "\n");
            assert_eq!(golden_h, hashes, "golden hashes changed for {name}");
            // decode of golden bytes is byte-identical after re-encoding
            let back = ModuleIR::decode(&golden, &Limits::v1()).unwrap();
            assert_eq!(back.encode(), golden);
        }
    }

    /// S003-A04: overflow, absent rows, duplicate keys and invalid transitions leave no business effects.
    #[test]
    fn inventory_sell_semantics() {
        let (m, _) = load_fixture("inventory_sell").unwrap();
        let mut st = State::default();
        st.put_row(&m, "Product", &[("id", uuid(1)), ("stock", Value::I64(1))])
            .unwrap();
        let sell = op(&m, "sell");
        let ok = evaluate(&m, sell, &[uuid(1), Value::I64(1)], &st).unwrap();
        let c = ok.candidate().expect("accepted");
        assert_eq!(
            st.field(&m, "Product", &uuid(1), "stock"),
            Some(Value::I64(1)),
            "pre-state untouched"
        );
        assert_eq!(
            c.post_state.field(&m, "Product", &uuid(1), "stock"),
            Some(Value::I64(0))
        );
        let too_many = evaluate(&m, sell, &[uuid(1), Value::I64(2)], &st).unwrap();
        assert_eq!(
            too_many.rejection().unwrap().code,
            ErrorCode::PostconditionRejected
        );
        let absent = evaluate(&m, sell, &[uuid(9), Value::I64(1)], &st).unwrap();
        assert_eq!(absent.rejection().unwrap().code, ErrorCode::MissingRecord);
        let nonpos = evaluate(&m, sell, &[uuid(1), Value::I64(0)], &st).unwrap();
        assert_eq!(
            nonpos.rejection().unwrap().code,
            ErrorCode::PreconditionRejected
        );
        // restock overflow on finite I64 is a typed rejection, never a wrap
        let mut big = State::default();
        big.put_row(
            &m,
            "Product",
            &[("id", uuid(1)), ("stock", Value::I64(i64::MAX))],
        )
        .unwrap();
        let over = evaluate(&m, op(&m, "restock"), &[uuid(1), Value::I64(1)], &big).unwrap();
        assert_eq!(over.rejection().unwrap().code, ErrorCode::NumericOverflow);
    }

    /// S003-A06: sell/sell from stock 1 yields a replayable accepted-effect counterexample.
    #[test]
    fn sell_sell_stock_one_counterexample() {
        let (m, _) = load_fixture("inventory_sell").unwrap();
        let mut st = State::default();
        st.put_row(&m, "Product", &[("id", uuid(1)), ("stock", Value::I64(1))])
            .unwrap();
        let sell = op(&m, "sell");
        let invs = vec![
            Invocation {
                operation: sell,
                arguments: vec![uuid(1), Value::I64(1)],
            },
            Invocation {
                operation: sell,
                arguments: vec![uuid(1), Value::I64(1)],
            },
        ];
        let cx = explore_concurrent(&m, &st, &invs)
            .unwrap()
            .expect("counterexample expected");
        assert!(
            matches!(cx.failure, Failure::InvariantViolated { ref invariant_name, .. } if invariant_name == "stock_nonneg")
        );
        assert_eq!(cx.accepted.len(), 2, "both sales were separately accepted");
        assert!(
            replay(&m, &cx).unwrap(),
            "counterexample must replay before it is Disproven"
        );
        let bytes = cx.to_canon().encode();
        assert!(bytes.len() > 100);
        // two restocks from any state are consistent (no counterexample within this bound; not a proof)
        let restock = op(&m, "restock");
        let invs = vec![
            Invocation {
                operation: restock,
                arguments: vec![uuid(1), Value::I64(1)],
            },
            Invocation {
                operation: restock,
                arguments: vec![uuid(1), Value::I64(2)],
            },
        ];
        assert!(explore_concurrent(&m, &st, &invs).unwrap().is_none());
    }

    /// S003-A08: reserve/release preserve conservation; duplicate release is rejected.
    #[test]
    fn reserve_release_semantics() {
        let (m, _) = load_fixture("inventory_reserve_release").unwrap();
        assert_eq!(
            m.invariants
                .iter()
                .find(|i| i.name == "supply_conserved")
                .unwrap()
                .kind,
            InvariantKind::Conservation
        );
        let mut st = State::default();
        st.put_row(
            &m,
            "Item",
            &[
                ("id", uuid(1)),
                ("available", Value::I64(5)),
                ("reserved", Value::I64(0)),
                ("total", Value::I64(5)),
            ],
        )
        .unwrap();
        let reserve = op(&m, "reserve");
        let release = op(&m, "release");
        let consume = op(&m, "consume");
        let r = evaluate(&m, reserve, &[uuid(1), Value::I64(3), uuid(7)], &st).unwrap();
        let after = r.candidate().expect("reserve accepted").post_state.clone();
        assert_eq!(
            after.field(&m, "Item", &uuid(1), "available"),
            Some(Value::I64(2))
        );
        assert_eq!(
            after.field(&m, "Item", &uuid(1), "reserved"),
            Some(Value::I64(3))
        );
        // duplicate reservation id
        let dup = evaluate(&m, reserve, &[uuid(1), Value::I64(1), uuid(7)], &after).unwrap();
        assert_eq!(dup.rejection().unwrap().code, ErrorCode::DuplicateIdentity);
        // over-reserve rejected by ENSURE/invariant
        let over = evaluate(&m, reserve, &[uuid(1), Value::I64(3), uuid(8)], &after).unwrap();
        assert!(!over.is_accepted());
        // release once ok
        let rel = evaluate(&m, release, &[uuid(1), Value::I64(3), uuid(7)], &after).unwrap();
        let released = rel
            .candidate()
            .expect("release accepted")
            .post_state
            .clone();
        assert_eq!(
            released.field(&m, "Item", &uuid(1), "available"),
            Some(Value::I64(5))
        );
        // release twice cannot restore stock twice
        let again = evaluate(&m, release, &[uuid(1), Value::I64(3), uuid(7)], &released).unwrap();
        assert_eq!(again.rejection().unwrap().code, ErrorCode::Conflict);
        // consume path preserves conservation
        let c = evaluate(&m, consume, &[uuid(1), Value::I64(3), uuid(7)], &after).unwrap();
        let consumed = c.candidate().expect("consume accepted").post_state.clone();
        assert_eq!(
            consumed.field(&m, "Item", &uuid(1), "total"),
            Some(Value::I64(2))
        );
        // release after consume is rejected (terminal state)
        let bad = evaluate(&m, release, &[uuid(1), Value::I64(3), uuid(7)], &consumed).unwrap();
        assert!(!bad.is_accepted());
        // concurrent double-release from the same snapshot is a counterexample
        let invs = vec![
            Invocation {
                operation: release,
                arguments: vec![uuid(1), Value::I64(3), uuid(7)],
            },
            Invocation {
                operation: release,
                arguments: vec![uuid(1), Value::I64(3), uuid(7)],
            },
        ];
        let cx = explore_concurrent(&m, &after, &invs)
            .unwrap()
            .expect("double release counterexample");
        assert!(matches!(cx.failure, Failure::EffectInapplicable { .. }));
    }

    /// S003-A08: transfer conservation, source != destination, exact decimal replay.
    #[test]
    fn account_transfer_semantics() {
        let (m, _) = load_fixture("account_transfer").unwrap();
        let d = |s: &str| Value::Decimal(Decimal::parse(s, 18, 2).unwrap());
        let mut st = State::default();
        st.put_row(&m, "Account", &[("id", uuid(1)), ("balance", d("100.00"))])
            .unwrap();
        st.put_row(&m, "Account", &[("id", uuid(2)), ("balance", d("0.00"))])
            .unwrap();
        st.put_row(
            &m,
            "Ledger",
            &[("id", Value::U64(0)), ("total", d("100.00"))],
        )
        .unwrap();
        let transfer = op(&m, "transfer");
        let ok = evaluate(&m, transfer, &[uuid(1), uuid(2), d("40.50")], &st).unwrap();
        let post = ok.candidate().expect("accepted").post_state.clone();
        assert_eq!(
            post.field(&m, "Account", &uuid(1), "balance"),
            Some(d("59.50"))
        );
        assert_eq!(
            post.field(&m, "Account", &uuid(2), "balance"),
            Some(d("40.50"))
        );
        let same = evaluate(&m, transfer, &[uuid(1), uuid(1), d("1.00")], &st).unwrap();
        assert_eq!(
            same.rejection().unwrap().code,
            ErrorCode::PreconditionRejected
        );
        let over = evaluate(&m, transfer, &[uuid(1), uuid(2), d("100.01")], &st).unwrap();
        assert_eq!(
            over.rejection().unwrap().code,
            ErrorCode::PostconditionRejected
        );
        // deposit without ledger update would break conservation; deposit updates both
        let dep = evaluate(&m, op(&m, "deposit"), &[uuid(2), d("1.25")], &post).unwrap();
        assert!(dep.is_accepted());
        // concurrent overdraft: two transfers each valid alone
        let invs = vec![
            Invocation {
                operation: transfer,
                arguments: vec![uuid(1), uuid(2), d("60.00")],
            },
            Invocation {
                operation: transfer,
                arguments: vec![uuid(1), uuid(2), d("60.00")],
            },
        ];
        let cx = explore_concurrent(&m, &st, &invs)
            .unwrap()
            .expect("overdraft counterexample");
        assert!(
            matches!(cx.failure, Failure::InvariantViolated { ref invariant_name, .. } if invariant_name == "balance_nonneg")
        );
    }

    /// S003-A05: unique absence predicate spans different primary keys.
    #[test]
    fn unique_username_semantics() {
        let (m, _) = load_fixture("unique_username").unwrap();
        let st = State::default();
        let register = op(&m, "register");
        let a = evaluate(&m, register, &[uuid(1), Value::Str("alice".into())], &st).unwrap();
        let post = a.candidate().expect("accepted").post_state.clone();
        let b = evaluate(&m, register, &[uuid(2), Value::Str("alice".into())], &post).unwrap();
        assert_eq!(b.rejection().unwrap().code, ErrorCode::InvariantRejected);
        // case matters: bytewise comparison, no folding
        let c = evaluate(&m, register, &[uuid(2), Value::Str("Alice".into())], &post).unwrap();
        assert!(c.is_accepted());
        // concurrent registration of the same name under different ids: counterexample
        let invs = vec![
            Invocation {
                operation: register,
                arguments: vec![uuid(1), Value::Str("bob".into())],
            },
            Invocation {
                operation: register,
                arguments: vec![uuid(2), Value::Str("bob".into())],
            },
        ];
        let cx = explore_concurrent(&m, &st, &invs)
            .unwrap()
            .expect("unique counterexample");
        assert!(
            matches!(cx.failure, Failure::InvariantViolated { ref invariant_name, .. } if invariant_name == "unique_username")
        );
        // duplicate primary key insert
        let dup = evaluate(&m, register, &[uuid(1), Value::Str("zed".into())], &post).unwrap();
        assert_eq!(dup.rejection().unwrap().code, ErrorCode::DuplicateIdentity);
    }

    /// causal_ship: the fact must be in the causal past; the transition must be legal.
    #[test]
    fn causal_ship_semantics() {
        let (m, _) = load_fixture("causal_ship").unwrap();
        let created = Value::Enum {
            enum_id: m.enums[0].id,
            variant: 0,
        };
        let mut st = State::default();
        st.put_row(&m, "Order", &[("id", uuid(1)), ("state", created)])
            .unwrap();
        let ship = op(&m, "ship");
        let no_fact = evaluate(&m, ship, &[uuid(1), uuid(5)], &st).unwrap();
        assert_eq!(
            no_fact.rejection().unwrap().code,
            ErrorCode::MissingDependency
        );
        let pay = evaluate(&m, op(&m, "confirm_payment"), &[uuid(1), uuid(3)], &st).unwrap();
        let paid = pay.candidate().expect("accepted").post_state.clone();
        let shipped = evaluate(&m, ship, &[uuid(1), uuid(5)], &paid).unwrap();
        let after = shipped
            .candidate()
            .expect("ship accepted")
            .post_state
            .clone();
        // shipping twice: duplicate fact and illegal transition
        let twice = evaluate(&m, ship, &[uuid(1), uuid(6)], &after).unwrap();
        assert!(!twice.is_accepted());
        // concurrent payment + ship from the unpaid snapshot: ship is rejected alone, so no counterexample
        let invs = vec![
            Invocation {
                operation: op(&m, "confirm_payment"),
                arguments: vec![uuid(1), uuid(3)],
            },
            Invocation {
                operation: ship,
                arguments: vec![uuid(1), uuid(5)],
            },
        ];
        assert!(explore_concurrent(&m, &st, &invs).unwrap().is_none());
    }

    /// causal_refund_extension: ship and refund each accepted alone cannot be justified sequentially.
    #[test]
    fn refund_extension_counterexample() {
        let (m, _) = load_fixture("causal_refund_extension").unwrap();
        let created = Value::Enum {
            enum_id: m.enums[0].id,
            variant: 0,
        };
        let mut st = State::default();
        st.put_row(&m, "Order", &[("id", uuid(1)), ("state", created)])
            .unwrap();
        let paid = evaluate(&m, op(&m, "confirm_payment"), &[uuid(1), uuid(3)], &st)
            .unwrap()
            .candidate()
            .unwrap()
            .post_state
            .clone();
        let invs = vec![
            Invocation {
                operation: op(&m, "ship"),
                arguments: vec![uuid(1), uuid(5)],
            },
            Invocation {
                operation: op(&m, "refund"),
                arguments: vec![uuid(1)],
            },
        ];
        let cx = explore_concurrent(&m, &paid, &invs)
            .unwrap()
            .expect("ship/refund counterexample");
        assert_eq!(cx.failure, Failure::NoSequentialJustification);
    }

    /// S003-A10: receipt and exact ordered result are observably distinct.
    #[test]
    fn increment_result_contracts_differ() {
        let (m, _) = load_fixture("increment_result").unwrap();
        let bump = m.operation_by_name("bump").unwrap();
        let get = m.operation_by_name("bump_and_get").unwrap();
        assert_eq!(bump.contract.result_semantics, ResultSemantics::Receipt);
        assert_eq!(
            get.contract.result_semantics,
            ResultSemantics::ExactOrderedValue
        );
        assert_eq!(bump.effects.len(), get.effects.len());
        let h = m.hashes();
        assert_ne!(
            h.contract_hashes[&bump.identity],
            h.contract_hashes[&get.identity]
        );
        // two concurrent bump_and_get from the same snapshot both return value 1: no sequential order justifies it
        let mut st = State::default();
        st.put_row(&m, "Counter", &[("id", uuid(1)), ("value", Value::I64(0))])
            .unwrap();
        let invs = vec![
            Invocation {
                operation: get.identity,
                arguments: vec![uuid(1)],
            },
            Invocation {
                operation: get.identity,
                arguments: vec![uuid(1)],
            },
        ];
        let cx = explore_concurrent(&m, &st, &invs)
            .unwrap()
            .expect("increment_and_get counterexample");
        assert_eq!(cx.failure, Failure::NoSequentialJustification);
        // plain receipts are consistent under concurrency within this bound
        let invs = vec![
            Invocation {
                operation: bump.identity,
                arguments: vec![uuid(1)],
            },
            Invocation {
                operation: bump.identity,
                arguments: vec![uuid(1)],
            },
        ];
        assert!(explore_concurrent(&m, &st, &invs).unwrap().is_none());
    }

    /// S003-A07 (interpreter part): identical invocation → identical normalized invocation digest.
    #[test]
    fn normalized_invocation_is_deterministic() {
        let (m, _) = load_fixture("inventory_sell").unwrap();
        let mut st = State::default();
        st.put_row(&m, "Product", &[("id", uuid(1)), ("stock", Value::I64(3))])
            .unwrap();
        let sell = op(&m, "sell");
        let a = evaluate(&m, sell, &[uuid(1), Value::I64(1)], &st).unwrap();
        let b = evaluate(&m, sell, &[uuid(1), Value::I64(1)], &st).unwrap();
        let (ca, cb) = (a.candidate().unwrap(), b.candidate().unwrap());
        assert_eq!(
            ca.invocation_digest(sell, &[uuid(1), Value::I64(1)]),
            cb.invocation_digest(sell, &[uuid(1), Value::I64(1)])
        );
        assert_ne!(
            ca.invocation_digest(sell, &[uuid(1), Value::I64(1)]),
            ca.invocation_digest(sell, &[uuid(1), Value::I64(2)])
        );
        // replay of applied effects yields the identical post-state and digest
        let mut replayed = st.clone();
        crate::interp::apply_effects(&m, &mut replayed, &ca.effects).unwrap();
        assert_eq!(replayed.digest(), ca.post_state.digest());
    }
}
