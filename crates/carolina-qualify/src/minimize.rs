//! Schedule minimizer (SPEC-010 §15): shrink the operation list while the failure persists.
//!
//! Delta debugging over the scheduled operations; the fault schedule is kept. Each candidate is
//! re-run and re-checked by the independent oracle, so the minimized schedule is itself evidence.

use crate::local::{Schedule, ScheduledOp};

/// `fails(schedule)` must be a pure function of the schedule (the runner is deterministic).
pub fn minimize<F: FnMut(&Schedule) -> bool>(
    original: &Schedule,
    mut fails: F,
    max_runs: usize,
) -> (Schedule, usize) {
    let mut current = original.clone();
    let mut runs = 0usize;
    if !fails(&current) {
        return (current, 1);
    }
    runs += 1;
    let mut n = 2usize;
    while current.ops.len() >= 2 && runs < max_runs {
        let chunk = current.ops.len().div_ceil(n);
        let mut reduced = false;
        let mut start = 0usize;
        while start < current.ops.len() && runs < max_runs {
            let end = (start + chunk).min(current.ops.len());
            let candidate_ops: Vec<ScheduledOp> = current.ops[..start]
                .iter()
                .chain(current.ops[end..].iter())
                .cloned()
                .collect();
            let candidate = Schedule {
                ops: candidate_ops,
                ..current.clone()
            };
            runs += 1;
            if fails(&candidate) {
                current = candidate;
                n = n.saturating_sub(1).max(2);
                reduced = true;
                break;
            }
            start = end;
        }
        if !reduced {
            if n >= current.ops.len() {
                break;
            }
            n = (n * 2).min(current.ops.len());
        }
    }
    (current, runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::w1::W1Op;

    #[test]
    fn shrinks_to_the_culprit() {
        let item = [1u8; 16];
        let ops: Vec<ScheduledOp> = (0..12)
            .map(|i| ScheduledOp::New {
                req: format!("r{i}"),
                op: W1Op::Supply { item, q: i },
            })
            .collect();
        let s = Schedule {
            seed: 1,
            items: vec![(item, 1)],
            ops,
            fault: None,
        };
        // "fails" whenever the schedule still contains supply(q = 7)
        let (m, runs) = minimize(
            &s,
            |c| {
                c.ops.iter().any(|o| {
                    matches!(
                        o,
                        ScheduledOp::New {
                            op: W1Op::Supply { q: 7, .. },
                            ..
                        }
                    )
                })
            },
            200,
        );
        assert_eq!(m.ops.len(), 1);
        assert!(runs > 1);
    }
}
