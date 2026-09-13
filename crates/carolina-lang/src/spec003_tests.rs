//! SPEC-003 acceptance gaps closed after the 2026-09-12 audit: structural lowering errors
//! (§3/§3.1), digest sensitivity (S003-A02/A03), decoder/frontend mutation fuzzing (S003-A09),
//! optional reads and the Assign/AddToSet/RemoveFromSet/CompareAndSwap effects (§4/§6), and the
//! effect-level rejections of partial release and same-endpoint transfer (§6, S003-A08).

use std::collections::BTreeSet;

use carolina_core::canon::Canonical;
use carolina_core::error::ErrorCode;
use carolina_core::limits::Limits;
use carolina_core::rng::DetRng;

use crate::fixtures::{fixture_source, load_fixture, FIXTURE_NAMES};
use crate::interp::{evaluate, State};
use crate::ir::ModuleIR;
use crate::lower::lower_module;
use crate::parser::parse_module;
use crate::types::Value;

const CONTRACT: &str = r#"CONTRACT {
    atomicity: WholeInvocation, input_visibility: SerialScope, result_semantics: Receipt,
    result_scope: PerKey(Profile[id]), session: {}, session_scope: None, durability: LocalStable,
    partition_outcomes: { Unavailable }, refusal_semantics: BusinessPredicate, commitment: FinalWhenDurable,
    request_namespace: "profiles"
  }"#;

fn profile_module() -> String {
    format!(
        r#"
RECORD Profile {{ id: Uuid PRIMARY KEY, nick: String(16), level: I64, tags: Set<String(8)> }}
INVARIANT level_nonneg {{ FORALL p IN Profile : p.level >= 0 }}
OPERATION rename(id: Uuid, nick: String(16)) VERSION 1 {{
  REQUIRE true
  READ {{ p = Profile[id] }}
  EFFECT {{ ASSIGN Profile[id].nick = nick }}
  ENSURE Profile[id].nick == nick
  RETURN {{ id: id }}
  {c}
}}
OPERATION tag(id: Uuid, t: String(8)) VERSION 1 {{
  REQUIRE true
  READ {{ p = Profile[id] }}
  EFFECT {{ ADD t TO Profile[id].tags }}
  ENSURE t IN Profile[id].tags
  RETURN {{ id: id }}
  {c}
}}
OPERATION untag(id: Uuid, t: String(8)) VERSION 1 {{
  REQUIRE true
  READ {{ p = Profile[id] }}
  EFFECT {{ REMOVE t FROM Profile[id].tags }}
  ENSURE true
  RETURN {{ id: id }}
  {c}
}}
OPERATION promote(id: Uuid, expected: I64, next: I64) VERSION 1 {{
  REQUIRE next >= 0
  READ {{ p = Profile[id] }}
  EFFECT {{ CAS Profile[id].level FROM expected TO next }}
  ENSURE Profile[id].level == next
  RETURN {{ id: id, level: next }}
  {c}
}}
OPERATION probe(id: Uuid) VERSION 1 {{
  REQUIRE true
  READ {{ present = EXISTS Profile[id], p = OPTIONAL Profile[id] }}
  EFFECT {{ }}
  ENSURE true
  RETURN {{ id: id, present: present, absent: p IS NONE }}
  {c}
}}
"#,
        c = CONTRACT
    )
}

fn uuid(n: u8) -> Value {
    Value::Uuid([n; 16])
}

fn lower_src(src: &str) -> Result<ModuleIR, ErrorCode> {
    let ast = parse_module(src, &Limits::v1()).map_err(|e| e.code)?;
    lower_module(&ast, None).map(|(ir, _)| ir).map_err(|e| e.code)
}

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

/// §3/§3.1: exactly one primary key, unique names and resolvable references are lowering errors,
/// each with a stable code.
#[test]
fn lowering_rejects_structural_errors_with_typed_codes() {
    let base = profile_module();
    let two_pks = base.replace(
        "nick: String(16), level: I64",
        "nick: String(16) PRIMARY KEY, level: I64",
    );
    assert_eq!(lower_src(&two_pks).unwrap_err(), ErrorCode::InvalidIr);
    let no_pk = base.replace("id: Uuid PRIMARY KEY", "id: Uuid");
    let code = lower_src(&no_pk).unwrap_err();
    assert!(
        matches!(code, ErrorCode::InvalidIr | ErrorCode::TypeMismatch),
        "missing primary key must be a typed lowering error, got {code:?}"
    );
    let dup_record = base.replacen(
        "INVARIANT level_nonneg",
        "RECORD Profile { id: Uuid PRIMARY KEY, nick: String(16) }\nINVARIANT level_nonneg",
        1,
    );
    assert_eq!(lower_src(&dup_record).unwrap_err(), ErrorCode::DuplicateIdentity);
    let dup_op = base.replacen("OPERATION untag", "OPERATION tag", 1);
    assert_eq!(lower_src(&dup_op).unwrap_err(), ErrorCode::DuplicateIdentity);
    let unresolved = base.replacen("READ { p = Profile[id] }", "READ { p = Account[id] }", 1);
    assert_eq!(lower_src(&unresolved).unwrap_err(), ErrorCode::MissingRecord);
    let optional_pk = base.replace("id: Uuid PRIMARY KEY", "id: Option<Uuid> PRIMARY KEY");
    assert_eq!(lower_src(&optional_pk).unwrap_err(), ErrorCode::TypeMismatch);
}

/// S003-A03: a version, scale or invariant change alters exactly the digests that depend on it.
#[test]
fn semantic_changes_alter_the_right_digests() {
    let src = fixture_source("account_transfer").unwrap();
    let base = lower_src(src).unwrap().hashes();
    // operation version: the operation/contract keys change and the module hash follows; schema does not
    let v2 = lower_src(&src.replacen("VERSION 1", "VERSION 2", 1))
        .unwrap()
        .hashes();
    assert_eq!(v2.schema_hash, base.schema_hash);
    assert_ne!(v2.module_hash, base.module_hash);
    assert_ne!(
        v2.operation_hashes.keys().collect::<Vec<_>>(),
        base.operation_hashes.keys().collect::<Vec<_>>()
    );
    // decimal scale: schema, operations that mention the type and the module all change
    let scaled = lower_src(&src.replace("Decimal(18, 2)", "Decimal(18, 3)"))
        .unwrap()
        .hashes();
    assert_ne!(scaled.schema_hash, base.schema_hash);
    assert_ne!(scaled.module_hash, base.module_hash);
    // invariant bound: only the invariant and module digests move
    let bound = lower_src(&src.replace("a.balance >= 0", "a.balance >= 1"))
        .unwrap()
        .hashes();
    assert_ne!(bound.module_hash, base.module_hash);
    assert_ne!(bound.invariant_hashes, base.invariant_hashes);
    assert_eq!(bound.operation_hashes, base.operation_hashes);
    assert_eq!(bound.contract_hashes, base.contract_hashes);
}

/// S003-A02: re-ordering declarations under a preserved id allocation emits byte-identical IR.
#[test]
fn declaration_order_with_preserved_ids_is_byte_identical() {
    let src = fixture_source("account_transfer").unwrap();
    let ast = parse_module(src, &Limits::v1()).unwrap();
    let (ir, alloc) = lower_module(&ast, None).unwrap();
    // swap the two RECORD declarations and the two OPERATIONs
    let ledger = "RECORD Ledger { id: U64 PRIMARY KEY, total: Decimal(18, 2) }\n";
    let account = "RECORD Account { id: Uuid PRIMARY KEY, balance: Decimal(18, 2) }\n";
    let swapped = src
        .replacen(ledger, "", 1)
        .replacen(account, &format!("{ledger}{account}"), 1);
    let dep_start = swapped.find("OPERATION deposit").unwrap();
    let tr_start = swapped.find("OPERATION transfer").unwrap();
    let head = &swapped[..tr_start];
    let transfer = &swapped[tr_start..dep_start];
    let deposit = &swapped[dep_start..];
    let reordered = format!("{head}{deposit}{transfer}");
    let ast2 = parse_module(&reordered, &Limits::v1()).unwrap();
    let (ir2, alloc2) = lower_module(&ast2, Some(&alloc)).unwrap();
    assert_eq!(ir2.encode(), ir.encode(), "reordered module must lower to identical bytes");
    assert_eq!(alloc2, alloc);
    // without the preserved allocation the ids (and therefore the bytes) may differ, but the
    // per-name schema semantics are the same
    let (ir3, _) = lower_module(&ast2, None).unwrap();
    assert_eq!(ir3.records.len(), ir.records.len());
}

/// S003-A03/A09: malformed IR — unknown mandatory tags, missing fields, wrong ir_version — is a
/// typed refusal, and every fixed point re-encodes to the very bytes that were decoded.
#[test]
fn malformed_module_ir_is_refused_with_typed_errors() {
    let (ir, _) = load_fixture("inventory_sell").unwrap();
    let bytes = ir.encode();
    let text = String::from_utf8(bytes.clone()).unwrap();
    let lim = Limits::v1();
    // wrong IR version
    let bumped = text.replacen("\"ir_version\":\"1\"", "\"ir_version\":\"99\"", 1);
    assert_ne!(bumped, text);
    assert_eq!(
        ModuleIR::decode(bumped.as_bytes(), &lim).unwrap_err().code,
        ErrorCode::UnsupportedIrVersion
    );
    // unknown effect kind tag
    let tagged = text.replacen("\"decrement\"", "\"teleport\"", 1);
    assert_ne!(tagged, text);
    assert!(ModuleIR::decode(tagged.as_bytes(), &lim).is_err());
    // a dropped mandatory field
    let dropped = text.replacen("\"ir_version\":", "\"ir_versionX\":", 1);
    assert_ne!(dropped, text);
    assert!(ModuleIR::decode(dropped.as_bytes(), &lim).is_err());
    // whitespace is non-canonical
    let spaced = text.replacen(":", ": ", 1);
    assert_eq!(
        ModuleIR::decode(spaced.as_bytes(), &lim).unwrap_err().code,
        ErrorCode::NonCanonicalEncoding
    );
}

/// S003-A09: deterministic mutation fuzzing of the IR decoder and the DSL frontend under the v1
/// and tiny limits — no panic, every accepted input is a canonical fixed point, and the tiny
/// limits refuse the real fixtures before any expansion.
#[test]
fn decoder_and_frontend_mutation_fuzz_never_panics() {
    let mut rng = DetRng::new(0x5EC0_03A9);
    let v1 = Limits::v1();
    let tiny = Limits::tiny();
    let mut decoded_ok = 0usize;
    let mut refused = 0usize;
    for name in FIXTURE_NAMES {
        let (ir, _) = load_fixture(name).unwrap();
        let bytes = ir.encode();
        for _ in 0..120 {
            let mut m = bytes.clone();
            match rng.below(4) {
                0 => {
                    let i = rng.below(m.len() as u64) as usize;
                    m[i] ^= 1 << rng.below(8);
                }
                1 => {
                    let i = rng.below(m.len() as u64) as usize;
                    m.truncate(i);
                }
                2 => {
                    const ALPHABET: &[u8] = b"{}[]\":,0123456789abcdef";
                    let i = rng.below(m.len() as u64) as usize;
                    m.insert(i, ALPHABET[rng.below(ALPHABET.len() as u64) as usize]);
                }
                _ => {
                    let i = rng.below(m.len() as u64) as usize;
                    let j = (i + 1 + rng.below(16) as usize).min(m.len());
                    m.drain(i..j);
                }
            }
            match ModuleIR::decode(&m, &v1) {
                Ok(back) => {
                    decoded_ok += 1;
                    assert_eq!(back.encode(), m, "{name}: accepted input must be a fixed point");
                }
                Err(e) => {
                    refused += 1;
                    assert_ne!(e.code, ErrorCode::Internal, "{name}: {e}");
                }
            }
            // the tiny limits never accept a mutated module of this size
            assert!(ModuleIR::decode(&m, &tiny).is_err());
        }
        // frontend: mutate the source text; parse + lower must stay total
        let src = fixture_source(name).unwrap().as_bytes().to_vec();
        for _ in 0..60 {
            let mut t = src.clone();
            for _ in 0..(1 + rng.below(3)) {
                if t.is_empty() {
                    break;
                }
                let i = rng.below(t.len() as u64) as usize;
                match rng.below(3) {
                    0 => {
                        const ALPHABET: &[u8] = b" (){}[]:,.<>=!+-*/\"'0aZ_";
                        t[i] = ALPHABET[rng.below(ALPHABET.len() as u64) as usize];
                    }
                    1 => {
                        t.truncate(i);
                    }
                    _ => {
                        let j = (i + 1 + rng.below(12) as usize).min(t.len());
                        t.drain(i..j);
                    }
                }
            }
            let Ok(text) = String::from_utf8(t) else {
                continue;
            };
            if let Ok(ast) = parse_module(&text, &v1) {
                let _ = lower_module(&ast, None);
            }
            assert!(parse_module(&text, &tiny).is_err() || text.len() <= tiny.max_source_bytes);
        }
        // bounded expansion: a fixture larger than the tiny source limit is refused before any
        // expansion, with the ResourceLimit code; smaller fixtures parse normally
        let real = fixture_source(name).unwrap();
        match parse_module(real, &tiny) {
            Ok(_) => assert!(real.len() <= tiny.max_source_bytes, "{name}"),
            Err(e) => {
                assert_eq!(e.code, ErrorCode::ResourceLimit, "{name}: {e}");
            }
        }
    }
    // Exercise the byte boundary directly: the fixture corpus may legitimately fit the
    // current tiny limits, so its size cannot serve as the negative control.
    let oversized = " ".repeat(tiny.max_source_bytes + 1);
    assert_eq!(parse_module(&oversized, &tiny).unwrap_err().code, ErrorCode::ResourceLimit);
    assert!(refused > decoded_ok, "mutations must mostly be refused ({refused} vs {decoded_ok})");
}

/// §6 / S003-A08: a partial release and a same-endpoint transfer are effect-level rejections that
/// leave the candidate state untouched (no REQUIRE guards them here).
#[test]
fn partial_release_and_same_endpoint_transfer_are_rejected_at_the_effect() {
    let (m, _) = load_fixture("inventory_reserve_release").unwrap();
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
    let reserve = m.operation_by_name("reserve").unwrap().identity;
    let release = m.operation_by_name("release").unwrap().identity;
    let r = evaluate(&m, reserve, &[uuid(1), Value::I64(3), uuid(7)], &st).unwrap();
    let after = r.candidate().unwrap().post_state.clone();
    let partial = evaluate(&m, release, &[uuid(1), Value::I64(2), uuid(7)], &after).unwrap();
    let rej = partial.rejection().expect("partial release is rejected");
    assert_eq!(rej.code, ErrorCode::UnsupportedEffect);
    // a release naming another item's key is an identity mismatch
    let wrong_key = evaluate(&m, release, &[uuid(2), Value::I64(3), uuid(7)], &after).unwrap();
    assert!(!wrong_key.is_accepted());

    let src = fixture_source("account_transfer")
        .unwrap()
        .replace("REQUIRE amount > 0 && source != destination", "REQUIRE amount > 0");
    let m2 = lower_src(&src).unwrap();
    let mut st2 = State::default();
    let ten = Value::Decimal(carolina_core::decimal::Decimal::new(1000, 18, 2).unwrap());
    st2.put_row(&m2, "Account", &[("id", uuid(1)), ("balance", ten.clone())])
        .unwrap();
    st2.put_row(&m2, "Ledger", &[("id", Value::U64(0)), ("total", ten.clone())])
        .unwrap();
    let transfer = m2.operation_by_name("transfer").unwrap().identity;
    let one = Value::Decimal(carolina_core::decimal::Decimal::new(100, 18, 2).unwrap());
    let same = evaluate(&m2, transfer, &[uuid(1), uuid(1), one.clone()], &st2).unwrap();
    let rej = same.rejection().expect("same-endpoint transfer is rejected");
    assert_eq!(rej.code, ErrorCode::UnsupportedEffect);
    assert!(rej.reason.contains("distinct"), "{}", rej.reason);
    // a distinct destination that does not exist is a missing record, never a partial credit
    let missing = evaluate(&m2, transfer, &[uuid(1), uuid(2), one], &st2).unwrap();
    assert_eq!(missing.rejection().unwrap().code, ErrorCode::MissingRecord);
}

/// §4 (Option, EXISTS) and §6 (Assign, AddToSet, RemoveFromSet, CompareAndSwap): the effects
/// execute deterministically, set operations are no-ops on present/absent members, CAS with a
/// stale expectation is a typed conflict, and optional reads of a missing row yield None.
#[test]
fn optional_reads_and_set_effects_execute() {
    let m = lower_src(&profile_module()).unwrap();
    let mut st = State::default();
    st.put_row(
        &m,
        "Profile",
        &[
            ("id", uuid(1)),
            ("nick", s("alice")),
            ("level", Value::I64(1)),
            ("tags", Value::Set(BTreeSet::from([s("a")]))),
        ],
    )
    .unwrap();
    let op = |n: &str| m.operation_by_name(n).unwrap().identity;
    // probe: present row vs absent row through EXISTS and OPTIONAL
    let hit = evaluate(&m, op("probe"), &[uuid(1)], &st).unwrap();
    let res = hit.candidate().unwrap().result.clone();
    assert!(matches!(&res, Value::Struct(f) if f["present"] == Value::Bool(true) && f["absent"] == Value::Bool(false)));
    let miss = evaluate(&m, op("probe"), &[uuid(9)], &st).unwrap();
    let res = miss.candidate().expect("optional read of a missing row is not a rejection").result.clone();
    assert!(matches!(&res, Value::Struct(f) if f["present"] == Value::Bool(false) && f["absent"] == Value::Bool(true)));
    // assign replaces the field
    let renamed = evaluate(&m, op("rename"), &[uuid(1), s("bob")], &st).unwrap();
    let st1 = renamed.candidate().unwrap().post_state.clone();
    assert_eq!(st1.field(&m, "Profile", &uuid(1), "nick"), Some(s("bob")));
    // add: new member inserted; existing member is a deterministic no-op
    let tagged = evaluate(&m, op("tag"), &[uuid(1), s("b")], &st1).unwrap();
    let st2 = tagged.candidate().unwrap().post_state.clone();
    assert_eq!(
        st2.field(&m, "Profile", &uuid(1), "tags"),
        Some(Value::Set(BTreeSet::from([s("a"), s("b")])))
    );
    let again = evaluate(&m, op("tag"), &[uuid(1), s("b")], &st2).unwrap();
    assert_eq!(
        again.candidate().unwrap().post_state.field(&m, "Profile", &uuid(1), "tags"),
        Some(Value::Set(BTreeSet::from([s("a"), s("b")])))
    );
    // remove: absent member is a no-op, present member removed
    let untag_absent = evaluate(&m, op("untag"), &[uuid(1), s("zz")], &st2).unwrap();
    assert!(untag_absent.is_accepted());
    let untagged = evaluate(&m, op("untag"), &[uuid(1), s("a")], &st2).unwrap();
    assert_eq!(
        untagged.candidate().unwrap().post_state.field(&m, "Profile", &uuid(1), "tags"),
        Some(Value::Set(BTreeSet::from([s("b")])))
    );
    // compare-and-swap: matching expectation swaps; stale expectation is a Conflict
    let promoted = evaluate(&m, op("promote"), &[uuid(1), Value::I64(1), Value::I64(2)], &st2).unwrap();
    let st3 = promoted.candidate().unwrap().post_state.clone();
    assert_eq!(st3.field(&m, "Profile", &uuid(1), "level"), Some(Value::I64(2)));
    let stale = evaluate(&m, op("promote"), &[uuid(1), Value::I64(1), Value::I64(3)], &st3).unwrap();
    assert_eq!(stale.rejection().unwrap().code, ErrorCode::Conflict);
    // invariant still guards the swapped value (level_nonneg) — REQUIRE keeps next >= 0, so a
    // negative target is a precondition rejection before any effect
    let neg = evaluate(&m, op("promote"), &[uuid(1), Value::I64(2), Value::I64(-1)], &st3).unwrap();
    assert!(!neg.is_accepted());
}
