use carolina_core::error::ErrorCode;
use carolina_core::limits::Limits;
use carolina_lang::interp::{evaluate, state_valid, State};
use carolina_lang::ir::{BindingId, ExprNodeIR, ModuleIR, PredicateIR};
use carolina_lang::lower::lower_module;
use carolina_lang::parser::parse_module;
use carolina_lang::types::Value;

fn source(invariant: &str, result: &str) -> String {
    format!(
        r#"
RECORD Counter {{ id: I64 PRIMARY KEY, value: I64 }}
RECORD Limit {{ id: I64 PRIMARY KEY, value: I64 }}
{invariant}
OPERATION bump(key: I64) VERSION 1 {{
  REQUIRE true
  READ {{ c = Counter[key], rows = SCAN a IN Counter WHERE true }}
  EFFECT {{ INCREMENT Counter[key].value BY 1 }}
  ENSURE true
  RETURN {result}
  CONTRACT {{
    atomicity: WholeInvocation, input_visibility: SerialScope,
    result_semantics: Receipt, result_scope: PerKey(Counter[key]),
    session: {{}}, session_scope: None, durability: LocalStable,
    partition_outcomes: {{ Unavailable }}, refusal_semantics: BusinessPredicate,
    commitment: FinalWhenDurable, request_namespace: "semantic-validation"
  }}
}}
"#
    )
}

fn module(invariant: &str) -> ModuleIR {
    let ast = parse_module(&source(invariant, "Unit"), &Limits::v1()).unwrap();
    lower_module(&ast, None).unwrap().0
}

fn counter(state: &mut State, module: &ModuleIR, id: i64, value: i64) {
    state
        .put_row(
            module,
            "Counter",
            &[("id", Value::I64(id)), ("value", Value::I64(value))],
        )
        .unwrap();
}

#[test]
fn invariant_expression_failures_reject_without_mutating_the_input() {
    let cases = [
        (
            "AGGREGATE SUM(a.value) FOR a IN Counter <= 9223372036854775807",
            vec![i64::MAX, 0],
            1,
        ),
        (
            "AGGREGATE SUM(a.value + 1) FOR a IN Counter <= 9223372036854775807",
            vec![i64::MAX - 1],
            0,
        ),
        (
            "AGGREGATE COUNT(a.value) FOR a IN Counter WHERE a.value + 1 > 0 <= 3",
            vec![i64::MAX - 1],
            0,
        ),
        ("UNIQUE a IN Counter : a.value + 1", vec![i64::MAX - 1], 0),
        (
            "FORALL a IN Counter WHERE a.value + 1 > 0 : true",
            vec![i64::MAX - 1],
            0,
        ),
        (
            "FORALL a IN Counter WHERE Limit[a.value].value > 0 : true",
            vec![0],
            0,
        ),
    ];
    for (body, initial, key) in cases {
        let module = module(&format!("INVARIANT checked {{ {body} }}"));
        let mut state = State::default();
        for (id, value) in initial.into_iter().enumerate() {
            counter(&mut state, &module, id as i64, value);
        }
        state
            .put_row(
                &module,
                "Limit",
                &[("id", Value::I64(0)), ("value", Value::I64(1))],
            )
            .unwrap();
        assert!(state_valid(&module, &state).unwrap().is_empty(), "{body}");
        let original = state.clone();
        let result = evaluate(
            &module,
            module.operation_by_name("bump").unwrap().identity,
            &[Value::I64(key)],
            &state,
        )
        .unwrap_or_else(|error| panic!("{body}: leaked technical error {error}"));
        let rejection = result.rejection().expect("must reject undefined invariant");
        assert_eq!(rejection.code, ErrorCode::InvariantRejected, "{body}");
        assert!(rejection.reason.contains("evaluation error"), "{body}");
        assert_eq!(state, original, "{body}");
    }
}

#[test]
fn malformed_invariant_ir_remains_an_internal_error() {
    for invariant in [
        "INVARIANT checked { FORALL a IN Counter : a.value >= 0 }",
        "INVARIANT checked { AGGREGATE SUM(a.value) FOR a IN Counter <= 10 }",
    ] {
        let mut module = module(invariant);
        let expr = match &mut module.invariants[0].predicate {
            PredicateIR::ForAll { predicate, .. } => predicate,
            PredicateIR::Aggregate { bound, .. } => bound,
            _ => unreachable!(),
        };
        expr.node = ExprNodeIR::Binding(BindingId(u32::MAX));
        let mut state = State::default();
        counter(&mut state, &module, 0, 1);
        let error = evaluate(
            &module,
            module.operation_by_name("bump").unwrap().identity,
            &[Value::I64(0)],
            &state,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidIr);
    }
}

#[test]
fn reference_evaluation_still_checks_disconnected_invariants() {
    let module = module("INVARIANT limit_exists { AGGREGATE SUM(l.value) FOR l IN Limit == 1 }");
    let mut state = State::default();
    counter(&mut state, &module, 0, 0);
    let result = evaluate(
        &module,
        module.operation_by_name("bump").unwrap().identity,
        &[Value::I64(0)],
        &state,
    )
    .unwrap();
    assert_eq!(
        result.rejection().unwrap().code,
        ErrorCode::InvariantRejected
    );
}

#[test]
fn return_wrappers_cannot_hide_rows_or_sets() {
    for result in [
        "{ row: c }",
        "(c, key)",
        "Some(c)",
        "{ nested: (Some(c), key) }",
        "{ rows: rows }",
        "Some(rows)",
        "{ nested: (rows, key) }",
    ] {
        let ast = parse_module(&source("", result), &Limits::v1()).unwrap();
        assert_eq!(
            lower_module(&ast, None).unwrap_err().code,
            ErrorCode::InvalidContract,
            "{result}"
        );
    }
    for result in [
        "{ value: c.value, count: SIZE(rows) }",
        "{ nested: (Some(c.value), key) }",
    ] {
        let ast = parse_module(&source("", result), &Limits::v1()).unwrap();
        lower_module(&ast, None).unwrap();
    }
}

#[test]
fn keys_and_set_elements_require_comparable_nested_types() {
    for source in [
        "RECORD Bad { id: Tuple(I64, Set<I64>) PRIMARY KEY }",
        "RECORD Bad { id: Tuple(I64, Option<Set<I64>>) PRIMARY KEY }",
        "RECORD Bad { id: I64 PRIMARY KEY, values: Set<Tuple(I64, Set<I64>)> }",
        "RECORD Bad { id: I64 PRIMARY KEY, values: Set<Option<Set<I64>>> }",
    ] {
        let ast = parse_module(source, &Limits::v1()).unwrap();
        assert_eq!(
            lower_module(&ast, None).unwrap_err().code,
            ErrorCode::TypeMismatch,
            "{source}"
        );
    }
    let ast = parse_module(
        "RECORD Good { id: Tuple(I64, Option<I64>) PRIMARY KEY, values: Set<Tuple(I64, Option<I64>)> }",
        &Limits::v1(),
    )
    .unwrap();
    lower_module(&ast, None).unwrap();
}
