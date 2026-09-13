use std::sync::Arc;

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::limits::Limits;
use carolina_lang::types::Value;
use carolina_runtime::engine::{make_invoke, EngineOptions, LocalEngine};
use carolina_runtime::LocalCatalog;
use carolina_storage::testutil::temp_dir;
use carolina_storage::StoreOptions;
use carolina_wire::records::{ClientReplyV1, ResolveReplyV1, ResolveRequestV1};

fn source() -> String {
    let mut source = String::from(
        r#"
RECORD Counter { id: I64 PRIMARY KEY, value: I64 }
RECORD Account { id: I64 PRIMARY KEY, balance: I64 }
RECORD Ledger { id: I64 PRIMARY KEY, total: I64 }
INVARIANT total_bound { AGGREGATE SUM(a.balance) FOR a IN Account <= Ledger[0].total }
"#,
    );
    for (name, record, field) in [
        ("bump", "Counter", "value"),
        ("deposit", "Account", "balance"),
    ] {
        source.push_str(&format!(
            r#"
OPERATION {name}(key: I64) VERSION 1 {{
  REQUIRE true
  READ {{}}
  EFFECT {{ INCREMENT {record}[key].{field} BY 1 }}
  ENSURE true
  RETURN Unit
  CONTRACT {{
    atomicity: WholeInvocation, input_visibility: SerialScope,
    result_semantics: Receipt, result_scope: PerKey({record}[key]),
    session: {{}}, session_scope: None, durability: LocalStable,
    partition_outcomes: {{ Unavailable }}, refusal_semantics: BusinessPredicate,
    commitment: FinalWhenDurable, request_namespace: "invariant-scope"
  }}
}}
"#
        ));
    }
    source
}

fn options() -> EngineOptions {
    EngineOptions::local(StoreOptions::default())
}

#[test]
fn independent_closures_and_undefined_invariants_produce_durable_receipts() {
    let dir = temp_dir("rt-invariant-scope");
    let catalog = Arc::new(LocalCatalog::from_source(&source()).unwrap());
    let mut engine = LocalEngine::create(&dir, catalog.clone(), options()).unwrap();
    for (record, field, values) in [
        ("Counter", "value", vec![0]),
        ("Account", "balance", vec![i64::MAX, 0]),
        ("Ledger", "total", vec![i64::MAX]),
    ] {
        let rows: Vec<_> = values
            .into_iter()
            .enumerate()
            .map(|(id, value)| {
                (
                    Value::I64(id as i64),
                    vec![("id", Value::I64(id as i64)), (field, Value::I64(value))],
                )
            })
            .collect();
        engine.load_rows(record, record, &rows).unwrap();
    }
    let bump = make_invoke(&catalog, "tenant", "bump", "bump", vec![Value::I64(0)]).unwrap();
    let operation = catalog.module.operation_by_name("bump").unwrap().identity;
    assert_eq!(catalog.closure_records(operation).len(), 1);
    assert!(catalog.output.closure.op_invariants[&operation].is_empty());
    let bump_reply = engine.invoke(&bump);
    assert!(
        matches!(bump_reply, ClientReplyV1::Committed(_)),
        "an unrelated aggregate must not be evaluated against unloaded tables: {bump_reply:?}"
    );
    assert_eq!(
        engine.read_row("Counter", &Value::I64(0)).unwrap().unwrap()["value"],
        Value::I64(1)
    );

    // Every row update fits I64, but their SUM overflows. This must be a final business
    // rejection, with no row mutation and a receipt that resolves after checkpoint/restart.
    let deposit = make_invoke(
        &catalog,
        "tenant",
        "overflow",
        "deposit",
        vec![Value::I64(1)],
    )
    .unwrap();
    let receipt = match engine.invoke(&deposit) {
        ClientReplyV1::Rejected(receipt) => receipt,
        other => panic!("expected final rejection for aggregate overflow: {other:?}"),
    };
    let result = CanonValue::decode(&receipt.exact_result_bytes, &Limits::v1()).unwrap();
    assert_eq!(
        result.field("code").unwrap().as_str().unwrap(),
        "InvariantRejected"
    );
    assert!(result
        .field("reason")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("NumericOverflow"));
    assert_eq!(
        engine.read_row("Account", &Value::I64(1)).unwrap().unwrap()["balance"],
        Value::I64(0)
    );
    assert_eq!(
        engine.invoke(&deposit).encode(),
        ClientReplyV1::Rejected(receipt.clone()).encode()
    );
    engine.checkpoint().unwrap();
    drop(engine);

    let mut engine = LocalEngine::open(&dir, catalog, options()).unwrap();
    match engine.resolve(&ResolveRequestV1 {
        request_key: deposit.content.request_key,
        expected_request_hash: deposit.request_hash,
    }) {
        ResolveReplyV1::Terminal(reply) => {
            assert_eq!(reply.encode(), ClientReplyV1::Rejected(receipt).encode());
        }
        other => panic!("rejection did not survive restart: {other:?}"),
    }
    assert_eq!(engine.invoke(&bump).encode(), bump_reply.encode());
    assert_eq!(
        engine.read_row("Counter", &Value::I64(0)).unwrap().unwrap()["value"],
        Value::I64(1)
    );
    assert_eq!(
        engine.read_row("Account", &Value::I64(1)).unwrap().unwrap()["balance"],
        Value::I64(0)
    );
    drop(engine);
    std::fs::remove_dir_all(dir).unwrap();
}
