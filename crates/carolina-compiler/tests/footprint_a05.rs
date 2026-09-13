//! S003-A05 (SPEC-003 §8, SPEC-004 §4/§6): footprint and closure coverage for the cases where a
//! naive "which rows does this statement touch" answer is wrong.
//!
//! Each test states the trap first, then asserts both levels: the selectors the lowering emitted,
//! and what the conservative closure did with them. Where the trap is a concurrency trap the
//! bounded explorer must produce a replayable counterexample, because a footprint that is right on
//! paper but never turns into an obligation buys nothing.
//!
//! Covered: aliasing (two writes that may be the same row), phantom insertion (a row that does not
//! exist yet but an aggregate would have seen), aggregate group movement (a row that changes the
//! GROUP BY key), bound-source changes (the right-hand side of an aggregate bound), reference
//! deletion (the parent of a REFERENCE invariant) and unique absence (a key namespace, not a row).

use carolina_compiler::{compile, CompileInput};
use carolina_core::error::ErrorCode;
use carolina_core::ids::{OperationRef, RecordId};
use carolina_core::limits::Limits;
use carolina_lang::counterexample::{explore_concurrent, replay, Invocation};
use carolina_lang::interp::{evaluate, State};
use carolina_lang::ir::{KeySet, ModuleIR, Purpose};
use carolina_lang::lower::lower_module;
use carolina_lang::parser::parse_module;
use carolina_lang::types::Value;

const CONTRACT: &str = r#"CONTRACT {
    atomicity: WholeInvocation, input_visibility: SerialScope, result_semantics: Receipt,
    result_scope: Global(Member), session: {}, session_scope: None, durability: LocalStable,
    partition_outcomes: { Unavailable }, refusal_semantics: BusinessPredicate,
    commitment: FinalWhenDurable, request_namespace: "teams"
  }"#;

fn source() -> String {
    format!(
        r#"
RECORD Team {{ id: I64 PRIMARY KEY, name: String(16) }}
RECORD Member {{ id: I64 PRIMARY KEY, team: I64, cost: I64, badge: String(16) }}
RECORD Ledger {{ id: I64 PRIMARY KEY, total: I64 }}

INVARIANT member_team_exists {{ REFERENCE m IN Member : m.team -> Team }}
INVARIANT per_team_budget {{ AGGREGATE SUM(m.cost) FOR m IN Member GROUP BY m.team <= 10 }}
INVARIANT total_budget {{ AGGREGATE SUM(m.cost) FOR m IN Member <= Ledger[0].total }}
INVARIANT badge_unique {{ UNIQUE m IN Member : m.badge }}

OPERATION hire(id: I64, team: I64, cost: I64, badge: String(16)) VERSION 1 {{
  REQUIRE cost > 0
  READ {{ t = Team[team] }}
  EFFECT {{ INSERT Member {{ id: id, team: team, cost: cost, badge: badge }} }}
  ENSURE true
  RETURN {{ id: id }}
  {c}
}}
OPERATION move_member(id: I64, team: I64) VERSION 1 {{
  REQUIRE true
  READ {{ m = Member[id], t = Team[team] }}
  EFFECT {{ ASSIGN Member[id].team = team }}
  ENSURE true
  RETURN {{ id: id }}
  {c}
}}
OPERATION disband(team: I64) VERSION 1 {{
  REQUIRE true
  READ {{ t = Team[team] }}
  EFFECT {{ DELETE Team[team] }}
  ENSURE true
  RETURN {{ team: team }}
  {c}
}}
OPERATION raise_budget(amount: I64) VERSION 1 {{
  REQUIRE amount > 0
  READ {{ l = Ledger[0] }}
  EFFECT {{ INCREMENT Ledger[0].total BY amount }}
  ENSURE true
  RETURN {{ amount: amount }}
  {c}
}}
OPERATION move_cost(from: I64, to: I64, amount: I64) VERSION 1 {{
  REQUIRE amount > 0
  READ {{ a = Member[from], b = Member[to] }}
  EFFECT {{ TRANSFER amount FROM Member[from].cost TO Member[to].cost }}
  ENSURE true
  RETURN {{ moved: amount }}
  {c}
}}
"#,
        c = CONTRACT
    )
}

fn module() -> ModuleIR {
    let ast = parse_module(&source(), &Limits::v1()).expect("parses");
    lower_module(&ast, None).expect("lowers").0
}

fn op(m: &ModuleIR, name: &str) -> OperationRef {
    m.operation_by_name(name)
        .unwrap_or_else(|| panic!("operation {name}"))
        .identity
}

fn record(m: &ModuleIR, name: &str) -> RecordId {
    m.records.iter().find(|r| r.name == name).unwrap().id
}

/// Invariants the conservative closure attached to an operation.
fn closure_invariants(name: &str) -> (ModuleIR, Vec<String>, Vec<String>) {
    let m = module();
    let out = compile(&CompileInput::local(m.clone())).expect("compiles");
    let o = op(&m, name);
    let invs = out.closure.op_invariants[&o]
        .iter()
        .map(|i| m.invariant(*i).unwrap().name.clone())
        .collect();
    let recs = out.closure.op_records[&o]
        .iter()
        .map(|r| m.record(*r).unwrap().name.clone())
        .collect();
    (m, invs, recs)
}

fn row(st: &mut State, m: &ModuleIR, rec: &str, fields: &[(&str, Value)]) {
    st.put_row(m, rec, fields).unwrap();
}

fn i(v: i64) -> Value {
    Value::I64(v)
}

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

/// Aliasing: `move_cost(from, to)` writes two rows of the same record through two parameters. The
/// footprint must carry both selectors (an "it is one record, so one selector" answer would let a
/// plan serialize on a single key), the two writes must sit in one atomic group, and the aliased
/// call must be refused rather than silently self-cancelling.
#[test]
fn aliasing_writes_are_two_selectors_in_one_atomic_group() {
    let m = module();
    let o = m.operation_by_name("move_cost").unwrap();
    let member = record(&m, "Member");
    let writes: Vec<&_> = o
        .footprint
        .writes
        .iter()
        .filter(|w| w.record == member)
        .collect();
    assert_eq!(writes.len(), 2, "both endpoints must appear: {writes:?}");
    for w in &writes {
        assert!(
            matches!(
                w.key_set,
                KeySet::Point {
                    static_key: true,
                    ..
                }
            ),
            "each endpoint is a point selector: {w:?}"
        );
        assert_eq!(w.purpose, Purpose::Write);
    }
    assert_eq!(
        o.footprint.atomic_groups.len(),
        1,
        "one invocation, one atomic group: {:?}",
        o.footprint.atomic_groups
    );
    assert_eq!(o.footprint.atomic_groups[0].len(), o.effects.len());

    // the aliased call is a typed rejection, not a no-op that leaves the sum untouched
    let mut st = State::default();
    row(&mut st, &m, "Team", &[("id", i(1)), ("name", s("a"))]);
    row(
        &mut st,
        &m,
        "Member",
        &[
            ("id", i(1)),
            ("team", i(1)),
            ("cost", i(4)),
            ("badge", s("b1")),
        ],
    );
    row(&mut st, &m, "Ledger", &[("id", i(0)), ("total", i(100))]);
    let aliased = evaluate(&m, op(&m, "move_cost"), &[i(1), i(1), i(2)], &st).unwrap();
    assert_eq!(
        aliased.rejection().expect("aliased transfer rejected").code,
        ErrorCode::UnsupportedEffect
    );
}

/// Phantom insertion: `hire` inserts a row that does not exist yet, so a per-row footprint would
/// miss it entirely. The closure must attach both aggregates to `hire`, and two concurrent hires
/// that each fit the group budget must produce a replayable counterexample.
#[test]
fn phantom_insertion_is_inside_the_aggregate_scope() {
    let (m, invs, recs) = closure_invariants("hire");
    assert!(
        invs.contains(&"per_team_budget".to_string()) && invs.contains(&"total_budget".to_string()),
        "an insert is inside every aggregate over that record: {invs:?}"
    );
    assert!(
        invs.contains(&"badge_unique".to_string()),
        "unique absence protects the whole key namespace, not an existing row: {invs:?}"
    );
    assert!(
        recs.contains(&"Ledger".to_string()),
        "the aggregate bound source belongs to the same closure: {recs:?}"
    );

    let mut st = State::default();
    row(&mut st, &m, "Team", &[("id", i(1)), ("name", s("a"))]);
    row(&mut st, &m, "Ledger", &[("id", i(0)), ("total", i(100))]);
    row(
        &mut st,
        &m,
        "Member",
        &[
            ("id", i(1)),
            ("team", i(1)),
            ("cost", i(4)),
            ("badge", s("b1")),
        ],
    );
    let hire = op(&m, "hire");
    // 4 + 4 = 8 ≤ 10 for either hire alone; together 4 + 4 + 4 = 12 > 10
    let cx = explore_concurrent(
        &m,
        &st,
        &[
            Invocation {
                operation: hire,
                arguments: vec![i(2), i(1), i(4), s("b2")],
            },
            Invocation {
                operation: hire,
                arguments: vec![i(3), i(1), i(4), s("b3")],
            },
        ],
    )
    .unwrap()
    .expect("two separately admissible inserts must violate the group budget together");
    assert!(replay(&m, &cx).unwrap(), "counterexample must replay");
}

/// Aggregate group movement: `move_member` writes one field, but the row leaves one GROUP BY
/// bucket and joins another, so the invariant must be evaluated over both groups.
#[test]
fn group_movement_keeps_both_groups_in_scope() {
    let (m, invs, _) = closure_invariants("move_member");
    assert!(
        invs.contains(&"per_team_budget".to_string()),
        "moving the GROUP BY key is an aggregate change: {invs:?}"
    );

    let mut st = State::default();
    for t in 1..=3 {
        row(&mut st, &m, "Team", &[("id", i(t)), ("name", s("t"))]);
    }
    row(&mut st, &m, "Ledger", &[("id", i(0)), ("total", i(100))]);
    // team 2 already holds 7; moving a 6 into it overflows the group, moving it alone does not
    row(
        &mut st,
        &m,
        "Member",
        &[
            ("id", i(1)),
            ("team", i(1)),
            ("cost", i(6)),
            ("badge", s("b1")),
        ],
    );
    row(
        &mut st,
        &m,
        "Member",
        &[
            ("id", i(2)),
            ("team", i(2)),
            ("cost", i(7)),
            ("badge", s("b2")),
        ],
    );
    let moved = evaluate(&m, op(&m, "move_member"), &[i(1), i(2)], &st).unwrap();
    assert!(
        !moved.is_accepted(),
        "the destination group must be evaluated, not only the row"
    );
    // the same row into an empty team is fine: the rejection above is about the destination group,
    // not about the row or the total (the total is 13 either way, well under Ledger[0].total)
    let ok = evaluate(&m, op(&m, "move_member"), &[i(1), i(3)], &st).unwrap();
    assert!(
        ok.is_accepted(),
        "moving into a group with room is legal: {ok:?}"
    );
    let after = ok.candidate().unwrap().post_state.clone();
    assert_eq!(after.field(&m, "Member", &i(1), "team"), Some(i(3)));
}

/// Bound-source change: `raise_budget` never touches Member, yet it changes the right-hand side of
/// `total_budget`. It must land in the same closure as the operations the bound constrains.
#[test]
fn aggregate_bound_source_shares_the_closure() {
    let (m, invs, recs) = closure_invariants("raise_budget");
    assert!(
        invs.contains(&"total_budget".to_string()),
        "writing the bound source is writing the invariant: {invs:?}"
    );
    assert!(
        recs.contains(&"Member".to_string()),
        "the constrained record travels with the bound: {recs:?}"
    );
    let out = compile(&CompileInput::local(m.clone())).unwrap();
    let raise = op(&m, "raise_budget");
    let hire = op(&m, "hire");
    assert!(
        out.closure.op_closure[&raise].contains(&hire),
        "raise_budget and hire interact through total_budget"
    );
    assert_eq!(
        out.closure.record_template[&record(&m, "Ledger")],
        out.closure.record_template[&record(&m, "Member")],
        "bound source and constrained record share one IDC template"
    );
}

/// Reference deletion: deleting the parent of a REFERENCE invariant is an interacting operation
/// even though it never writes the child record.
#[test]
fn reference_deletion_is_an_interacting_write() {
    let (m, invs, recs) = closure_invariants("disband");
    assert!(
        invs.contains(&"member_team_exists".to_string()),
        "deleting a parent is inside the referential invariant: {invs:?}"
    );
    assert!(
        recs.contains(&"Member".to_string()),
        "the child record is in the closure of the parent delete: {recs:?}"
    );
    let out = compile(&CompileInput::local(m.clone())).unwrap();
    assert!(
        out.closure.op_closure[&op(&m, "disband")].contains(&op(&m, "hire")),
        "hiring into a team and disbanding it interact"
    );

    let mut st = State::default();
    row(&mut st, &m, "Team", &[("id", i(1)), ("name", s("a"))]);
    row(&mut st, &m, "Ledger", &[("id", i(0)), ("total", i(100))]);
    row(
        &mut st,
        &m,
        "Member",
        &[
            ("id", i(1)),
            ("team", i(1)),
            ("cost", i(1)),
            ("badge", s("b1")),
        ],
    );
    let orphaned = evaluate(&m, op(&m, "disband"), &[i(1)], &st).unwrap();
    assert_eq!(
        orphaned.rejection().expect("orphaning is rejected").code,
        ErrorCode::InvariantRejected
    );
    // a team with no members may be disbanded
    let mut empty = State::default();
    row(&mut empty, &m, "Team", &[("id", i(9)), ("name", s("z"))]);
    row(&mut empty, &m, "Ledger", &[("id", i(0)), ("total", i(100))]);
    assert!(evaluate(&m, op(&m, "disband"), &[i(9)], &empty)
        .unwrap()
        .is_accepted());
}

/// Unique absence: two inserts with the same badge are each admissible alone. The uniqueness
/// invariant is global over the key namespace, so the pair must be a counterexample.
#[test]
fn unique_absence_is_a_namespace_not_a_row() {
    let m = module();
    let mut st = State::default();
    row(&mut st, &m, "Team", &[("id", i(1)), ("name", s("a"))]);
    row(&mut st, &m, "Ledger", &[("id", i(0)), ("total", i(100))]);
    let hire = op(&m, "hire");
    let cx = explore_concurrent(
        &m,
        &st,
        &[
            Invocation {
                operation: hire,
                arguments: vec![i(1), i(1), i(1), s("same")],
            },
            Invocation {
                operation: hire,
                arguments: vec![i(2), i(1), i(1), s("same")],
            },
        ],
    )
    .unwrap()
    .expect("two rows with the same unique key must be a counterexample");
    assert!(replay(&m, &cx).unwrap());
}
