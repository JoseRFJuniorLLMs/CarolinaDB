//! FM-2 — C5 decision authority, prepare / unique decision / install / publication / completion
//! (SPEC-008 §10, §11, §15; SPEC-010 §16).
//!
//! One replicated decision authority (compare-and-set state `Begun → DecidedCommit | DecidedAbort`)
//! with leader epochs (a stale leader may still hold an old epoch), `N` participants, and a
//! duplicating/reordering network of PREPARE, votes, COMMIT/ABORT, INSTALLED, PUBLICATION and
//! PUBLISH-SEEN messages. Participants' prepared payload is durable and invisible until published.
//!
//! Invariants checked in every reachable state:
//! * at most one final decision; ABORT never replaces COMMIT;
//! * COMMIT requires a durable YES vote from every participant;
//! * prepared data remains invisible: a participant's data is public only after the publication
//!   certificate, which requires INSTALLED from every participant (no partial public transaction);
//! * no final success before required completion (every participant PUBLISH-SEEN);
//! * a stale leader cannot decide;
//! * a participant never leaves PREPARED without a decision (cannot infer abort from a timeout);
//! * a committed transaction never loses a prepared payload to GC before install.

use crate::{explore, GateEvidence, ModelReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Decision {
    Begun,
    Commit,
    Abort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Idle,
    Prepared,
    VotedNo,
    Installed,
    Published,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Participant {
    pub phase: Phase,
    pub payload_present: bool,
    pub prepare_sent: bool,
    pub vote_sent: bool,
    pub installed_sent: bool,
    pub seen_sent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct State {
    pub decision: Decision,
    pub ever_commit: bool,
    pub ever_abort: bool,
    pub decided_by_epoch: u8,
    /// A decision was taken by a leader whose epoch was already superseded (must stay false).
    pub stale_decision: bool,
    pub leader_epoch: u8,
    /// A stale leader from epoch 1 still exists while the current epoch is 2.
    pub stale_leader_alive: bool,
    pub commit_sent: bool,
    pub abort_sent: bool,
    pub publication_sent: bool,
    pub completion_sent: bool,
    pub success_reported: bool,
    pub participants: Vec<Participant>,
    pub timeout_aborts: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Commit decided with a missing YES vote.
    CommitWithoutAllVotes,
    /// A participant aborts on its own timeout while undecided.
    ParticipantAbortsOnTimeout,
    /// A stale leader may still decide.
    StaleLeaderDecides,
    /// Participants publish as soon as they install (no publication certificate gate).
    PublishOnInstall,
    /// GC of a prepared payload while undecided.
    GcPreparedPayload,
    /// ABORT may replace an existing COMMIT (no CAS).
    AbortAfterCommit,
}

pub fn invariant(s: &State) -> Result<(), String> {
    if s.ever_commit && s.ever_abort {
        return Err("more than one final decision".into());
    }
    if s.decision == Decision::Commit
        && s.participants
            .iter()
            .any(|p| !p.vote_sent || p.phase == Phase::VotedNo || p.phase == Phase::Idle)
    {
        return Err("COMMIT without YES from every participant".into());
    }
    if s.stale_decision {
        return Err("stale authority decided".into());
    }
    let any_published = s.participants.iter().any(|p| p.phase == Phase::Published);
    if any_published {
        if s.decision != Decision::Commit {
            return Err("published data without a commit decision".into());
        }
        if !s.publication_sent {
            return Err("published without a publication certificate".into());
        }
        if s.participants
            .iter()
            .any(|p| !matches!(p.phase, Phase::Installed | Phase::Published))
        {
            return Err("partial public transaction: a participant published while another is not installed".into());
        }
    }
    if s.publication_sent
        && s.participants
            .iter()
            .any(|p| !matches!(p.phase, Phase::Installed | Phase::Published))
    {
        return Err("publication certificate without INSTALLED from every participant".into());
    }
    if s.success_reported && !s.completion_sent {
        return Err("final success before completion evidence".into());
    }
    if s.completion_sent && s.participants.iter().any(|p| p.phase != Phase::Published) {
        return Err("completion certificate without PUBLISH-SEEN from every participant".into());
    }
    if s.timeout_aborts > 0 {
        return Err("participant inferred abort from a timeout".into());
    }
    for (i, p) in s.participants.iter().enumerate() {
        if p.phase == Phase::Aborted && s.decision != Decision::Abort {
            return Err(format!(
                "participant {i} aborted without a durable abort decision"
            ));
        }
        if s.decision == Decision::Commit && p.phase == Phase::Prepared && !p.payload_present {
            return Err(format!(
                "participant {i}: committed transaction lost its prepared payload"
            ));
        }
    }
    Ok(())
}

pub fn next(s: &State, control: Option<Control>) -> Vec<(String, State)> {
    let mut out = Vec::new();
    let n = s.participants.len();
    for i in 0..n {
        let p = s.participants[i];
        if !p.prepare_sent {
            let mut ns = s.clone();
            ns.participants[i].prepare_sent = true;
            out.push((format!("SendPrepare({i})"), ns));
        }
        if p.prepare_sent && p.phase == Phase::Idle {
            let mut ns = s.clone();
            ns.participants[i].phase = Phase::Prepared;
            ns.participants[i].payload_present = true;
            out.push((format!("VoteYes({i})"), ns));
            let mut ns = s.clone();
            ns.participants[i].phase = Phase::VotedNo;
            out.push((format!("VoteNo({i})"), ns));
        }
        if matches!(p.phase, Phase::Prepared | Phase::VotedNo) && !p.vote_sent {
            let mut ns = s.clone();
            ns.participants[i].vote_sent = true;
            out.push((format!("SendVote({i})"), ns));
        }
        if s.commit_sent && p.phase == Phase::Prepared && p.payload_present {
            let mut ns = s.clone();
            ns.participants[i].phase = Phase::Installed;
            out.push((format!("Install({i})"), ns));
        }
        if p.phase == Phase::Installed && !p.installed_sent {
            let mut ns = s.clone();
            ns.participants[i].installed_sent = true;
            out.push((format!("SendInstalled({i})"), ns));
        }
        let may_publish = p.phase == Phase::Installed
            && (s.publication_sent || control == Some(Control::PublishOnInstall));
        if may_publish {
            let mut ns = s.clone();
            ns.participants[i].phase = Phase::Published;
            out.push((format!("Publish({i})"), ns));
        }
        if p.phase == Phase::Published && !p.seen_sent {
            let mut ns = s.clone();
            ns.participants[i].seen_sent = true;
            out.push((format!("SendPublishSeen({i})"), ns));
        }
        if s.abort_sent && matches!(p.phase, Phase::Prepared | Phase::VotedNo | Phase::Idle) {
            let mut ns = s.clone();
            ns.participants[i].phase = Phase::Aborted;
            ns.participants[i].payload_present = false;
            out.push((format!("ParticipantAbort({i})"), ns));
        }
        if control == Some(Control::ParticipantAbortsOnTimeout)
            && p.phase == Phase::Prepared
            && s.decision == Decision::Begun
        {
            let mut ns = s.clone();
            ns.participants[i].phase = Phase::Aborted;
            ns.participants[i].payload_present = false;
            ns.timeout_aborts += 1;
            out.push((format!("TimeoutAbort({i})"), ns));
        }
        if control == Some(Control::GcPreparedPayload)
            && p.phase == Phase::Prepared
            && p.payload_present
            && s.decision == Decision::Begun
        {
            let mut ns = s.clone();
            ns.participants[i].payload_present = false;
            out.push((format!("GcPrepared({i})"), ns));
        }
    }
    // decisions by the current leader through the replicated CAS
    let all_yes = s
        .participants
        .iter()
        .all(|p| p.phase == Phase::Prepared && p.vote_sent)
        || s.participants.iter().all(|p| {
            matches!(
                p.phase,
                Phase::Prepared | Phase::Installed | Phase::Published
            ) && p.vote_sent
        });
    let some_no = s
        .participants
        .iter()
        .any(|p| p.phase == Phase::VotedNo && p.vote_sent);
    let commit_allowed = s.decision == Decision::Begun
        && (all_yes
            || (control == Some(Control::CommitWithoutAllVotes)
                && s.participants
                    .iter()
                    .any(|p| p.phase == Phase::Prepared && p.vote_sent)));
    if commit_allowed {
        let mut ns = s.clone();
        ns.decision = Decision::Commit;
        ns.ever_commit = true;
        ns.decided_by_epoch = s.leader_epoch;
        out.push(("DecideCommit".into(), ns));
    }
    // abort while undecided: on a NO vote or a coordinator deadline (authority's choice)
    if s.decision == Decision::Begun {
        let mut ns = s.clone();
        ns.decision = Decision::Abort;
        ns.ever_abort = true;
        ns.decided_by_epoch = s.leader_epoch;
        out.push((
            if some_no {
                "DecideAbort(no-vote)".into()
            } else {
                "DecideAbort(deadline)".into()
            },
            ns,
        ));
    }
    if control == Some(Control::AbortAfterCommit) && s.decision == Decision::Commit {
        let mut ns = s.clone();
        ns.decision = Decision::Abort;
        ns.ever_abort = true;
        out.push(("AbortReplacesCommit".into(), ns));
    }
    if control == Some(Control::StaleLeaderDecides)
        && s.stale_leader_alive
        && s.decision == Decision::Begun
    {
        let mut ns = s.clone();
        ns.decision = Decision::Commit;
        ns.ever_commit = true;
        ns.decided_by_epoch = 1;
        ns.stale_decision = true;
        out.push(("StaleLeaderDecidesCommit".into(), ns));
    }
    if s.decision == Decision::Commit && !s.commit_sent {
        let mut ns = s.clone();
        ns.commit_sent = true;
        out.push(("SendCommit".into(), ns));
    }
    if s.decision == Decision::Abort && !s.abort_sent {
        let mut ns = s.clone();
        ns.abort_sent = true;
        out.push(("SendAbort".into(), ns));
    }
    // publication certificate: commit + INSTALLED from the complete sealed set
    if s.decision == Decision::Commit
        && !s.publication_sent
        && s.participants.iter().all(|p| p.installed_sent)
    {
        let mut ns = s.clone();
        ns.publication_sent = true;
        out.push(("IssuePublicationCertificate".into(), ns));
    }
    if s.publication_sent && !s.completion_sent && s.participants.iter().all(|p| p.seen_sent) {
        let mut ns = s.clone();
        ns.completion_sent = true;
        out.push(("IssueCompletionCertificate".into(), ns));
    }
    if !s.success_reported
        && (s.completion_sent || control == Some(Control::PublishOnInstall) && s.publication_sent)
    {
        let mut ns = s.clone();
        ns.success_reported = true;
        out.push(("ReportFinalSuccess".into(), ns));
    }
    // leader failover: the replicated decision state survives; the old leader may linger
    if s.leader_epoch == 1 {
        let mut ns = s.clone();
        ns.leader_epoch = 2;
        ns.stale_leader_alive = true;
        out.push(("LeaderFailover".into(), ns));
    }
    out
}

pub fn initial(participants: usize) -> State {
    State {
        decision: Decision::Begun,
        ever_commit: false,
        ever_abort: false,
        decided_by_epoch: 0,
        stale_decision: false,
        leader_epoch: 1,
        stale_leader_alive: false,
        commit_sent: false,
        abort_sent: false,
        publication_sent: false,
        completion_sent: false,
        success_reported: false,
        participants: vec![
            Participant {
                phase: Phase::Idle,
                payload_present: false,
                prepare_sent: false,
                vote_sent: false,
                installed_sent: false,
                seen_sent: false,
            };
            participants
        ],
        timeout_aborts: 0,
    }
}

fn describe(s: &State) -> String {
    format!(
        "decision={:?} epoch={} pub={} done={} success={} participants={:?}",
        s.decision,
        s.leader_epoch,
        s.publication_sent,
        s.completion_sent,
        s.success_reported,
        s.participants
            .iter()
            .map(|p| format!("{:?}{}", p.phase, if p.payload_present { "+" } else { "" }))
            .collect::<Vec<_>>()
    )
}

pub fn run(participants: usize, control: Option<Control>, max_states: usize) -> ModelReport {
    let bounds = format!(
        "participants={participants}, leader epochs 1..2 with lingering stale leader, duplicating/reordering network{}",
        control.map(|c| format!(", control={c:?}")).unwrap_or_default()
    );
    explore(
        "FM-2",
        &bounds,
        initial(participants),
        |s| next(s, control),
        invariant,
        describe,
        max_states,
    )
}

pub fn evidence(max_states: usize) -> GateEvidence {
    GateEvidence {
        gate: "FM-2",
        positive: run(2, None, max_states),
        controls: [
            Control::CommitWithoutAllVotes,
            Control::ParticipantAbortsOnTimeout,
            Control::StaleLeaderDecides,
            Control::PublishOnInstall,
            Control::GcPreparedPayload,
            Control::AbortAfterCommit,
        ]
        .into_iter()
        .map(|c| (format!("{c:?}"), run(2, Some(c), max_states)))
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
        assert!(ev.verdict().is_ok(), "{:?}", ev.verdict());
    }
}
