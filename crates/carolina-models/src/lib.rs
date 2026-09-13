//! Formal model targets (SPEC-010 §16) as explicit-state model checkers.
//!
//! Each model is a finite state machine with a duplicating, reordering network (a message once
//! sent may be delivered any number of times, in any order, or never), durable state that
//! survives every crash, and explicit bounds. `explore` performs a breadth-first exhaustive search
//! of the reachable states, checks the invariants in every state and returns the first violating
//! trace. The TLA+ sources under `models/` state the same machines and are checked separately by
//! `tools/run_tlc.py`; `models/README.md` records the pinned tool, bounds and state counts.
//!
//! * [`fm1`] — Escrow transfer / authority / rights conservation
//! * [`fm2`] — C5 decision authority, prepare/decision/install/publication/completion
//! * [`fm3`] — Migration, fencing and plan evolution
//!
//! Negative controls (SPEC-010 §16) are part of every model: a deliberately broken variant MUST
//! produce a counterexample, otherwise the checker has not established useful evidence.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub mod fm1;
pub mod fm2;
pub mod fm3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace {
    pub steps: Vec<String>,
    pub final_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelReport {
    pub model: String,
    pub bounds: String,
    pub states: usize,
    pub transitions: usize,
    pub max_depth: usize,
    /// Exploration stopped at the state budget: the verdict is only for the explored prefix.
    pub truncated: bool,
    pub violation: Option<(String, Trace)>,
}

impl ModelReport {
    pub fn passed(&self) -> bool {
        self.violation.is_none() && !self.truncated
    }
    pub fn summary(&self) -> String {
        match &self.violation {
            Some((inv, t)) => format!(
                "{}: VIOLATION of `{}` after {} steps ({} states explored): {} => {}",
                self.model,
                inv,
                t.steps.len(),
                self.states,
                t.steps.join(" ; "),
                t.final_state
            ),
            None => format!(
                "{}: no violation in {} states / {} transitions, max depth {}{} [{}]",
                self.model,
                self.states,
                self.transitions,
                self.max_depth,
                if self.truncated {
                    " (TRUNCATED at budget)"
                } else {
                    ""
                },
                self.bounds
            ),
        }
    }
}

/// Exhaustive breadth-first exploration. `next` enumerates every enabled action with its label;
/// `invariant` names the violated property. Explores at most `max_states` states.
pub fn explore<S, N, I, D>(
    model: &str,
    bounds: &str,
    init: S,
    next: N,
    invariant: I,
    describe: D,
    max_states: usize,
) -> ModelReport
where
    S: Clone + Eq + Hash,
    N: Fn(&S) -> Vec<(String, S)>,
    I: Fn(&S) -> Result<(), String>,
    D: Fn(&S) -> String,
{
    let mut states: Vec<S> = vec![init.clone()];
    let mut index: HashMap<S, usize> = HashMap::new();
    index.insert(init, 0);
    let mut parent: Vec<Option<(usize, String)>> = vec![None];
    let mut depth: Vec<usize> = vec![0];
    let mut queue: VecDeque<usize> = VecDeque::new();
    queue.push_back(0);
    let mut transitions = 0usize;
    let mut max_depth = 0usize;
    let mut truncated = false;
    let trace_of = |i: usize, parent: &Vec<Option<(usize, String)>>, states: &Vec<S>| -> Trace {
        let mut steps = Vec::new();
        let mut cur = i;
        while let Some((p, label)) = &parent[cur] {
            steps.push(label.clone());
            cur = *p;
        }
        steps.reverse();
        Trace {
            steps,
            final_state: describe(&states[i]),
        }
    };
    if let Err(inv) = invariant(&states[0]) {
        return ModelReport {
            model: model.into(),
            bounds: bounds.into(),
            states: 1,
            transitions: 0,
            max_depth: 0,
            truncated: false,
            violation: Some((inv, trace_of(0, &parent, &states))),
        };
    }
    while let Some(i) = queue.pop_front() {
        let current = states[i].clone();
        let d = depth[i];
        for (label, s) in next(&current) {
            transitions += 1;
            if index.contains_key(&s) {
                continue;
            }
            let j = states.len();
            states.push(s.clone());
            index.insert(s, j);
            parent.push(Some((i, label)));
            depth.push(d + 1);
            max_depth = max_depth.max(d + 1);
            if let Err(inv) = invariant(&states[j]) {
                return ModelReport {
                    model: model.into(),
                    bounds: bounds.into(),
                    states: states.len(),
                    transitions,
                    max_depth,
                    truncated: false,
                    violation: Some((inv, trace_of(j, &parent, &states))),
                };
            }
            if states.len() >= max_states {
                truncated = true;
                queue.clear();
                break;
            }
            queue.push_back(j);
        }
        if truncated {
            break;
        }
    }
    ModelReport {
        model: model.into(),
        bounds: bounds.into(),
        states: states.len(),
        transitions,
        max_depth,
        truncated,
        violation: None,
    }
}

/// A model run plus its negative controls, as the qualification runner consumes it.
#[derive(Debug, Clone)]
pub struct GateEvidence {
    pub gate: &'static str,
    pub positive: ModelReport,
    /// (control name, report) — every report must carry a violation.
    pub controls: Vec<(String, ModelReport)>,
}

impl GateEvidence {
    /// PASS iff the positive model has no violation, is not truncated, and every negative control
    /// found one.
    pub fn verdict(&self) -> Result<String, String> {
        if let Some((inv, t)) = &self.positive.violation {
            return Err(format!(
                "{}: `{}` violated after {}",
                self.gate,
                inv,
                t.steps.join(" ; ")
            ));
        }
        if self.positive.truncated {
            return Err(format!(
                "{}: exploration truncated at {} states; no completed verdict",
                self.gate, self.positive.states
            ));
        }
        for (name, r) in &self.controls {
            if r.violation.is_none() {
                return Err(format!(
                    "{}: negative control `{name}` produced no counterexample ({} states)",
                    self.gate, r.states
                ));
            }
        }
        Ok(format!(
            "{} states, {} transitions, depth {}; {} negative controls each produced a counterexample [{}]",
            self.positive.states,
            self.positive.transitions,
            self.positive.max_depth,
            self.controls.len(),
            self.positive.bounds
        ))
    }
}

pub fn all_gates(max_states: usize) -> Vec<GateEvidence> {
    vec![
        fm1::evidence(max_states),
        fm2::evidence(max_states),
        fm3::evidence(max_states),
    ]
}
