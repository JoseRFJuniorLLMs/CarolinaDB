//! Bounded concurrent-acceptance explorer and replayable counterexamples (SPEC-003 §11, §12; SPEC-001 §60).
//!
//! Given a pre-state and a set of invocations, the explorer evaluates every invocation
//! *independently against the same snapshot* (the way replicas would accept them
//! concurrently under a naive commutative/causal plan), then applies all accepted
//! literal effects to the snapshot and checks the invariant closure. A violation, an
//! inapplicable effect or a returned observation that no sequential order can justify is
//! a `Counterexample` that replays deterministically before it may be labelled `Disproven`.
//!
//! Finding no counterexample within the explored bound is **not** a proof.

use carolina_core::canon::CanonValue;
use carolina_core::error::CoreResult;
use carolina_core::ids::OperationRef;

use crate::interp::{apply_effects, check_invariants, evaluate, Candidate, Outcome, State};
use crate::ir::ModuleIR;
use crate::types::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub operation: OperationRef,
    pub arguments: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Applying the accepted effects together violated an invariant.
    InvariantViolated {
        invariant_name: String,
        detail: String,
    },
    /// An accepted effect could not be applied on top of the others (overflow, missing row, duplicate identity).
    EffectInapplicable { detail: String },
    /// Both invocations were accepted but no sequential order of the same invocations accepts both.
    NoSequentialJustification,
}

/// A replayable semantic counterexample (SPEC-003 §11: arguments, state, accepted effects, observations, failing invariant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counterexample {
    pub pre_state: State,
    pub invocations: Vec<Invocation>,
    pub accepted: Vec<Candidate>,
    pub failure: Failure,
}

impl Counterexample {
    pub fn to_canon(&self) -> CanonValue {
        let inv: Vec<CanonValue> = self
            .invocations
            .iter()
            .map(|i| {
                CanonValue::obj()
                    .fvec("arguments", &i.arguments)
                    .fc("operation", &i.operation)
                    .build()
            })
            .collect();
        let acc: Vec<CanonValue> = self
            .accepted
            .iter()
            .zip(&self.invocations)
            .map(|(c, i)| c.normalized_invocation(i.operation, &i.arguments))
            .collect();
        let failure = match &self.failure {
            Failure::InvariantViolated {
                invariant_name,
                detail,
            } => CanonValue::obj()
                .fstr("detail", detail)
                .fstr("invariant", invariant_name)
                .fstr("kind", "invariant_violated")
                .build(),
            Failure::EffectInapplicable { detail } => CanonValue::obj()
                .fstr("detail", detail)
                .fstr("kind", "effect_inapplicable")
                .build(),
            Failure::NoSequentialJustification => CanonValue::obj()
                .fstr("kind", "no_sequential_justification")
                .build(),
        };
        CanonValue::obj()
            .f("accepted", CanonValue::Array(acc))
            .f("failure", failure)
            .f("invocations", CanonValue::Array(inv))
            .fstr("kind", "counterexample.v1")
            .fc("pre_state", &self.pre_state)
            .build()
    }
}

/// Evaluate all invocations against `pre` concurrently (same snapshot) and merge the accepted effects.
/// Returns `Ok(None)` when the concurrent acceptance is consistent, `Ok(Some(cx))` on a counterexample.
pub fn explore_concurrent(
    module: &ModuleIR,
    pre: &State,
    invocations: &[Invocation],
) -> CoreResult<Option<Counterexample>> {
    let mut accepted = Vec::new();
    let mut accepted_invocations = Vec::new();
    for inv in invocations {
        if let Outcome::Accepted(c) = evaluate(module, inv.operation, &inv.arguments, pre)? {
            accepted.push(*c);
            accepted_invocations.push(inv.clone());
        }
    }
    if accepted.len() < 2 {
        return Ok(None);
    }
    // merge: apply every accepted literal effect list to the shared snapshot
    let mut merged = pre.clone();
    for c in &accepted {
        if let Err(e) = apply_effects(module, &mut merged, &c.effects) {
            return Ok(Some(Counterexample {
                pre_state: pre.clone(),
                invocations: accepted_invocations,
                accepted,
                failure: Failure::EffectInapplicable {
                    detail: e.to_string(),
                },
            }));
        }
    }
    let violations = check_invariants(module, pre, &merged)?;
    if let Some(v) = violations.first() {
        let name = module.invariant(v.invariant)?.name.clone();
        return Ok(Some(Counterexample {
            pre_state: pre.clone(),
            invocations: accepted_invocations,
            accepted,
            failure: Failure::InvariantViolated {
                invariant_name: name,
                detail: v.detail.clone(),
            },
        }));
    }
    // observation check: does some sequential order of the accepted invocations accept all of them
    // with the same results? (for two invocations we try both orders)
    if accepted.len() == 2
        && !sequentially_justified(module, pre, &accepted_invocations, &accepted)?
    {
        return Ok(Some(Counterexample {
            pre_state: pre.clone(),
            invocations: accepted_invocations,
            accepted,
            failure: Failure::NoSequentialJustification,
        }));
    }
    Ok(None)
}

fn sequentially_justified(
    module: &ModuleIR,
    pre: &State,
    invs: &[Invocation],
    accepted: &[Candidate],
) -> CoreResult<bool> {
    for order in [[0usize, 1], [1, 0]] {
        let mut st = pre.clone();
        let mut ok = true;
        for &i in &order {
            match evaluate(module, invs[i].operation, &invs[i].arguments, &st)? {
                Outcome::Accepted(c) => {
                    if c.result != accepted[i].result {
                        ok = false;
                        break;
                    }
                    st = c.post_state;
                }
                Outcome::Rejected(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Replay a counterexample against the reference semantics; true when it reproduces the same failure class.
pub fn replay(module: &ModuleIR, cx: &Counterexample) -> CoreResult<bool> {
    match explore_concurrent(module, &cx.pre_state, &cx.invocations)? {
        Some(again) => {
            Ok(std::mem::discriminant(&again.failure) == std::mem::discriminant(&cx.failure))
        }
        None => Ok(false),
    }
}
