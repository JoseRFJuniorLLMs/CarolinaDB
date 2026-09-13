//! FM-1 — Escrow transfer, authority and rights conservation (SPEC-006 §8, §11, §16; SPEC-010 §16).
//!
//! Logical model: one resource with total `T`; a donor holder and a receiver holder; `N` rights
//! transfers with fixed quantities; spends by the holder that owns usable rights; donor authority
//! close and replacement (a stale passive replica keeps an old view of the donor's usable rights).
//!
//! Durable phases follow SPEC-006 §8:
//! ```text
//! donor:    ABSENT -> PREPARED -> COMMITTED | ABORTED
//! receiver: ABSENT -> ACCEPTED -> APPLIED  | ABORTED (donor's final abort only)
//! ```
//! Messages are derived from durable state and may be delivered any number of times in any order
//! (duplication, reordering, loss and crash-after-durable-boundary-before-send are all covered by
//! "a send is a separate action from the durable transition and a delivered message stays
//! deliverable").
//!
//! Invariants checked in every reachable state:
//! * conservation `T = C + H + U_donor + U_receiver + X` with non-negative terms (H is 0 here:
//!   reservations are covered by the W1 oracle; `X` = quantity of transfers prepared/committed but
//!   not yet applied);
//! * receiver credit implies an irrevocable donor commit (`APPLIED ⇒ donor COMMITTED`);
//! * no refund after COMMITTED (`donor ABORTED ⇒ receiver ≠ APPLIED`);
//! * no double install (applied at most once — encoded by conservation);
//! * no spend by a passive replica and no spend after authority close (spend counters);
//! * no late PREPARE resurrection (an aborted receiver never becomes ACCEPTED).

use crate::{explore, GateEvidence, ModelReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Donor {
    Absent,
    Prepared,
    Committed,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Receiver {
    Absent,
    Accepted,
    Applied,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Transfer {
    pub q: u8,
    pub donor: Donor,
    pub receiver: Receiver,
    pub prepare_sent: bool,
    pub accept_sent: bool,
    pub commit_sent: bool,
    pub abort_sent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct State {
    pub total: u8,
    pub u_donor: u8,
    pub u_receiver: u8,
    pub consumed: u8,
    pub transfers: Vec<Transfer>,
    /// Donor authority closed (migration fence / replacement).
    pub donor_closed: bool,
    /// A passive replica's stale view of the donor's usable rights (never authoritative).
    pub replica_view: u8,
    pub replica_spends: u8,
    pub spends_after_close: u8,
    /// Receiver tombstones revived by a late PREPARE (must stay 0).
    pub revivals: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Donor may ABORT (refund) after COMMITTED.
    RefundAfterCommit,
    /// Receiver credits a duplicate COMMIT again (no idempotent install).
    DoubleInstall,
    /// The stale passive replica may spend from its own view (no fencing).
    NoReplicaFence,
    /// The donor keeps spending after its authority was closed.
    SpendAfterClose,
    /// A late PREPARE revives an aborted receiver record.
    LatePrepareResurrection,
}

fn x_of(t: &Transfer) -> u8 {
    match (t.donor, t.receiver) {
        (Donor::Prepared | Donor::Committed, r) if r != Receiver::Applied => t.q,
        _ => 0,
    }
}

pub fn invariant(s: &State) -> Result<(), String> {
    let x: u32 = s.transfers.iter().map(|t| x_of(t) as u32).sum();
    let sum = s.consumed as u32 + s.u_donor as u32 + s.u_receiver as u32 + x;
    if sum != s.total as u32 {
        return Err(format!(
            "conservation T = C + H + U + X ({} != {} + 0 + {} + {} + {})",
            s.total, s.consumed, s.u_donor, s.u_receiver, x
        ));
    }
    for (i, t) in s.transfers.iter().enumerate() {
        if t.receiver == Receiver::Applied && t.donor != Donor::Committed {
            return Err(format!(
                "transfer {i}: receiver credit without irrevocable donor commit"
            ));
        }
        if t.donor == Donor::Aborted && t.receiver == Receiver::Applied {
            return Err(format!("transfer {i}: refund after committed install"));
        }
    }
    if s.replica_spends > 0 {
        return Err("spend by passive replica".into());
    }
    if s.spends_after_close > 0 {
        return Err("spend after authority close".into());
    }
    if s.revivals > 0 {
        return Err("late PREPARE resurrected an aborted receiver record".into());
    }
    Ok(())
}

pub fn next(s: &State, control: Option<Control>) -> Vec<(String, State)> {
    let mut out = Vec::new();
    let n = s.transfers.len();
    for i in 0..n {
        let t = s.transfers[i];
        // donor PREPARE: usable >= q, authority open, durable in one batch
        if t.donor == Donor::Absent && !s.donor_closed && s.u_donor >= t.q {
            let mut ns = s.clone();
            ns.u_donor -= t.q;
            ns.transfers[i].donor = Donor::Prepared;
            out.push((format!("DonorPrepare({i})"), ns));
        }
        if t.donor == Donor::Prepared && !t.prepare_sent {
            let mut ns = s.clone();
            ns.transfers[i].prepare_sent = true;
            out.push((format!("SendPrepare({i})"), ns));
        }
        // receiver ACCEPT on a delivered PREPARE (may be delivered again later: no removal)
        if t.prepare_sent && t.receiver == Receiver::Absent {
            let mut ns = s.clone();
            ns.transfers[i].receiver = Receiver::Accepted;
            out.push((format!("ReceiverAccept({i})"), ns));
        }
        if control == Some(Control::LatePrepareResurrection)
            && t.prepare_sent
            && t.receiver == Receiver::Aborted
        {
            let mut ns = s.clone();
            ns.transfers[i].receiver = Receiver::Accepted;
            ns.revivals += 1;
            out.push((format!("LatePrepareRevives({i})"), ns));
        }
        if t.receiver == Receiver::Accepted && !t.accept_sent {
            let mut ns = s.clone();
            ns.transfers[i].accept_sent = true;
            out.push((format!("SendAccept({i})"), ns));
        }
        // donor COMMIT only after a durable acceptance was delivered
        if t.accept_sent && t.donor == Donor::Prepared {
            let mut ns = s.clone();
            ns.transfers[i].donor = Donor::Committed;
            out.push((format!("DonorCommit({i})"), ns));
        }
        if t.donor == Donor::Committed && !t.commit_sent {
            let mut ns = s.clone();
            ns.transfers[i].commit_sent = true;
            out.push((format!("SendCommit({i})"), ns));
        }
        // donor ABORT only from PREPARED (refund X -> U_donor)
        let may_abort = t.donor == Donor::Prepared
            || (control == Some(Control::RefundAfterCommit) && t.donor == Donor::Committed);
        if may_abort {
            let mut ns = s.clone();
            ns.transfers[i].donor = Donor::Aborted;
            ns.u_donor += t.q;
            out.push((format!("DonorAbort({i})"), ns));
        }
        if t.donor == Donor::Aborted && !t.abort_sent {
            let mut ns = s.clone();
            ns.transfers[i].abort_sent = true;
            out.push((format!("SendAbort({i})"), ns));
        }
        // receiver INSTALL on a delivered COMMIT with a durable ACCEPTED record; idempotent
        let may_install = t.commit_sent
            && (t.receiver == Receiver::Accepted
                || (control == Some(Control::DoubleInstall) && t.receiver == Receiver::Applied));
        if may_install {
            let mut ns = s.clone();
            ns.transfers[i].receiver = Receiver::Applied;
            ns.u_receiver += t.q;
            out.push((format!("ReceiverInstall({i})"), ns));
        }
        // receiver ABORT on the donor's final abort; tombstone against late prepare
        if t.abort_sent && matches!(t.receiver, Receiver::Absent | Receiver::Accepted) {
            let mut ns = s.clone();
            ns.transfers[i].receiver = Receiver::Aborted;
            out.push((format!("ReceiverAbort({i})"), ns));
        }
    }
    // spends by the authoritative holders
    if s.u_donor > 0 && (!s.donor_closed || control == Some(Control::SpendAfterClose)) {
        let mut ns = s.clone();
        ns.u_donor -= 1;
        ns.consumed += 1;
        if s.donor_closed {
            ns.spends_after_close += 1;
        }
        out.push(("SpendDonor".into(), ns));
    }
    if s.u_receiver > 0 {
        let mut ns = s.clone();
        ns.u_receiver -= 1;
        ns.consumed += 1;
        out.push(("SpendReceiver".into(), ns));
    }
    // authority close (migration fence / replacement); the passive replica keeps its stale view
    if !s.donor_closed {
        let mut ns = s.clone();
        ns.donor_closed = true;
        ns.replica_view = s.u_donor;
        out.push(("CloseDonorAuthority".into(), ns));
    }
    if control == Some(Control::NoReplicaFence) && s.donor_closed && s.replica_view > 0 {
        let mut ns = s.clone();
        ns.replica_view -= 1;
        ns.consumed += 1;
        ns.replica_spends += 1;
        out.push(("StaleReplicaSpend".into(), ns));
    }
    out
}

pub fn initial(total: u8, quantities: &[u8]) -> State {
    State {
        total,
        u_donor: total,
        u_receiver: 0,
        consumed: 0,
        transfers: quantities
            .iter()
            .map(|q| Transfer {
                q: *q,
                donor: Donor::Absent,
                receiver: Receiver::Absent,
                prepare_sent: false,
                accept_sent: false,
                commit_sent: false,
                abort_sent: false,
            })
            .collect(),
        donor_closed: false,
        replica_view: 0,
        replica_spends: 0,
        spends_after_close: 0,
        revivals: 0,
    }
}

fn describe(s: &State) -> String {
    format!(
        "U_d={} U_r={} C={} closed={} transfers={:?}",
        s.u_donor,
        s.u_receiver,
        s.consumed,
        s.donor_closed,
        s.transfers
            .iter()
            .map(|t| format!("{:?}/{:?}", t.donor, t.receiver))
            .collect::<Vec<_>>()
    )
}

pub fn run(
    total: u8,
    quantities: &[u8],
    control: Option<Control>,
    max_states: usize,
) -> ModelReport {
    let bounds =
        format!(
        "T={total}, transfers={:?}, duplicating/reordering network, donor close + stale replica{}",
        quantities,
        control.map(|c| format!(", control={c:?}")).unwrap_or_default()
    );
    explore(
        "FM-1",
        &bounds,
        initial(total, quantities),
        |s| next(s, control),
        invariant,
        describe,
        max_states,
    )
}

pub fn evidence(max_states: usize) -> GateEvidence {
    let q = [1u8, 2];
    GateEvidence {
        gate: "FM-1",
        positive: run(3, &q, None, max_states),
        controls: [
            Control::RefundAfterCommit,
            Control::DoubleInstall,
            Control::NoReplicaFence,
            Control::SpendAfterClose,
            Control::LatePrepareResurrection,
        ]
        .into_iter()
        .map(|c| (format!("{c:?}"), run(3, &q, Some(c), max_states)))
        .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_model_holds_and_controls_fail() {
        let ev = evidence(2_000_000);
        assert!(ev.positive.violation.is_none(), "{}", ev.positive.summary());
        assert!(!ev.positive.truncated);
        for (name, r) in &ev.controls {
            assert!(
                r.violation.is_some(),
                "control {name} found nothing: {}",
                r.summary()
            );
        }
        assert!(ev.verdict().is_ok());
    }

    #[test]
    fn late_prepare_control_needs_the_tombstone() {
        // the abort tombstone is terminal: a late PREPARE must not revive the receiver record
        let r = run(
            2,
            &[1, 1],
            Some(Control::LatePrepareResurrection),
            2_000_000,
        );
        assert!(r.violation.is_some(), "{}", r.summary());
        assert!(r
            .violation
            .as_ref()
            .unwrap()
            .1
            .steps
            .iter()
            .any(|s| s.starts_with("LatePrepareRevives")));
    }
}
