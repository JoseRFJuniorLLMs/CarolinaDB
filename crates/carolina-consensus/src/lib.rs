//! Consensus adapter (SPEC-008 §5, SPEC-011 §8): a Raft replicated log with fixed membership.
//!
//! Conceptual operations: `propose(command) -> committed log reference`, `read_barrier()`,
//! `await_applied(frontier)` (the caller drains committed entries), durable persistent state
//! ([`storage::RaftStorage`]). The implementation is deterministic: time is a logical tick, the
//! only randomness is a seeded election timeout, and every message crosses an explicit
//! [`Envelope`] queue so the whole cluster can run inside the deterministic simulator
//! ([`sim`]) or over a real transport.
//!
//! Guarantees within the crash-fault model and a fixed voter set: a single committed log prefix
//! (election safety, log matching, leader completeness), no commit without a quorum of durable
//! acknowledgements, and a stale leader cannot pass a read barrier or commit. Wall-clock leases
//! are not used (SPEC-008 §5). Dynamic membership is unsupported (SPEC-011 §8).

use std::collections::{BTreeMap, BTreeSet};

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::NodeId;
use carolina_core::rng::DetRng;

pub mod sim;
pub mod storage;

pub use storage::{FileRaftStorage, MemRaftStorage, Persistent, RaftStorage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub term: u64,
    pub index: u64,
    /// Empty data marks the leader's term no-op.
    pub data: Vec<u8>,
}

impl Canonical for Entry {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fbytes("data", &self.data)
            .fu64("index", self.index)
            .fu64("term", self.term)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["data", "index", "term"])?;
        Ok(Entry {
            term: v.field("term")?.as_u64()?,
            index: v.field("index")?.as_u64()?,
            data: v.field("data")?.as_bytes()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    RequestVote {
        term: u64,
        last_log_index: u64,
        last_log_term: u64,
    },
    RequestVoteReply {
        term: u64,
        granted: bool,
    },
    AppendEntries {
        term: u64,
        prev_log_index: u64,
        prev_log_term: u64,
        entries: Vec<Entry>,
        leader_commit: u64,
        /// Read-barrier round this heartbeat belongs to (0 = none).
        read_round: u64,
    },
    AppendEntriesReply {
        term: u64,
        success: bool,
        /// On success: highest index now matching; on failure: the follower's last index (hint).
        match_index: u64,
        read_round: u64,
        /// Highest index this voter has durably applied. The leader keeps the minimum across the
        /// membership as the compaction horizon: no entry at or below it can still be needed.
        applied_index: u64,
    },
}

impl Canonical for Message {
    fn to_canon(&self) -> CanonValue {
        match self {
            Message::RequestVote {
                term,
                last_log_index,
                last_log_term,
            } => CanonValue::obj()
                .fstr("kind", "RequestVote")
                .fu64("last_log_index", *last_log_index)
                .fu64("last_log_term", *last_log_term)
                .fu64("term", *term)
                .build(),
            Message::RequestVoteReply { term, granted } => CanonValue::obj()
                .fbool("granted", *granted)
                .fstr("kind", "RequestVoteReply")
                .fu64("term", *term)
                .build(),
            Message::AppendEntries {
                term,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
                read_round,
            } => CanonValue::obj()
                .fvec("entries", entries)
                .fstr("kind", "AppendEntries")
                .fu64("leader_commit", *leader_commit)
                .fu64("prev_log_index", *prev_log_index)
                .fu64("prev_log_term", *prev_log_term)
                .fu64("read_round", *read_round)
                .fu64("term", *term)
                .build(),
            Message::AppendEntriesReply {
                term,
                success,
                match_index,
                read_round,
                applied_index,
            } => CanonValue::obj()
                .fu64("applied_index", *applied_index)
                .fstr("kind", "AppendEntriesReply")
                .fu64("match_index", *match_index)
                .fu64("read_round", *read_round)
                .fbool("success", *success)
                .fu64("term", *term)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let u = |f: &str| v.field(f)?.as_u64();
        Ok(match v.field("kind")?.as_str()? {
            "RequestVote" => Message::RequestVote {
                term: u("term")?,
                last_log_index: u("last_log_index")?,
                last_log_term: u("last_log_term")?,
            },
            "RequestVoteReply" => Message::RequestVoteReply {
                term: u("term")?,
                granted: v.field("granted")?.as_bool()?,
            },
            "AppendEntries" => Message::AppendEntries {
                term: u("term")?,
                prev_log_index: u("prev_log_index")?,
                prev_log_term: u("prev_log_term")?,
                entries: v
                    .field("entries")?
                    .as_array()?
                    .iter()
                    .map(Entry::from_canon)
                    .collect::<CoreResult<Vec<_>>>()?,
                leader_commit: u("leader_commit")?,
                read_round: u("read_round")?,
            },
            "AppendEntriesReply" => Message::AppendEntriesReply {
                term: u("term")?,
                success: v.field("success")?.as_bool()?,
                match_index: u("match_index")?,
                read_round: u("read_round")?,
                applied_index: u("applied_index")?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("raft message {k}"),
                ))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub from: NodeId,
    pub to: NodeId,
    pub msg: Message,
}

impl Canonical for Envelope {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("from", &self.from)
            .fc("msg", &self.msg)
            .fc("to", &self.to)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["from", "msg", "to"])?;
        Ok(Envelope {
            from: NodeId::from_canon(v.field("from")?)?,
            to: NodeId::from_canon(v.field("to")?)?,
            msg: Message::from_canon(v.field("msg")?)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Fixed, sorted voter set (SPEC-011 §8: three voters in the initial profile).
    pub members: Vec<NodeId>,
    pub election_ticks_min: u32,
    pub election_ticks_max: u32,
    pub heartbeat_ticks: u32,
}

impl Config {
    pub fn new(mut members: Vec<NodeId>) -> Config {
        members.sort();
        members.dedup();
        Config {
            members,
            election_ticks_min: 10,
            election_ticks_max: 20,
            heartbeat_ticks: 3,
        }
    }
    pub fn quorum(&self) -> usize {
        self.members.len() / 2 + 1
    }
}

/// A completed read barrier: the log index that is guaranteed committed at the time of the
/// request. The caller may serve a current read once it has applied up to that index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadIndex {
    pub round: u64,
    pub index: u64,
}

pub struct Raft<S: RaftStorage> {
    me: NodeId,
    cfg: Config,
    storage: S,
    term: u64,
    voted_for: Option<NodeId>,
    log: Vec<Entry>,
    commit_index: u64,
    last_delivered: u64,
    role: Role,
    leader: Option<NodeId>,
    votes: BTreeSet<NodeId>,
    next_index: BTreeMap<NodeId, u64>,
    match_index: BTreeMap<NodeId, u64>,
    election_elapsed: u32,
    heartbeat_elapsed: u32,
    election_timeout: u32,
    rng: DetRng,
    outbox: Vec<Envelope>,
    read_round: u64,
    /// Log base after compaction: entries up to and including this index are covered by the
    /// applied state and no longer stored (SPEC-011 §9).
    snapshot_index: u64,
    snapshot_term: u64,
    /// Highest index this voter has durably applied, and the leader's view of the others.
    applied: u64,
    peer_applied: BTreeMap<NodeId, u64>,
    /// Times this leader could not serve a follower because the entries it needs were compacted.
    pub followers_behind_snapshot: u64,
    pending_reads: Vec<(u64, u64, BTreeSet<NodeId>)>, // (round, index, acks)
    ready_reads: Vec<ReadIndex>,
    /// Leader terms observed (for diagnostics/tests).
    pub became_leader_terms: Vec<u64>,
}

impl<S: RaftStorage> Raft<S> {
    pub fn new(me: NodeId, cfg: Config, storage: S, seed: u64) -> CoreResult<Self> {
        if !cfg.members.contains(&me) {
            return Err(CoreError::new(
                ErrorCode::InvalidManifest,
                "node is not a configured voter",
            ));
        }
        let p = storage.load()?;
        let mut r = Raft {
            me,
            cfg,
            storage,
            term: p.term,
            voted_for: p.voted_for,
            log: p.entries,
            commit_index: 0,
            snapshot_index: p.snapshot_index,
            snapshot_term: p.snapshot_term,
            applied: p.snapshot_index,
            peer_applied: BTreeMap::new(),
            followers_behind_snapshot: 0,
            last_delivered: p.snapshot_index,
            role: Role::Follower,
            leader: None,
            votes: BTreeSet::new(),
            next_index: BTreeMap::new(),
            match_index: BTreeMap::new(),
            election_elapsed: 0,
            heartbeat_elapsed: 0,
            election_timeout: 0,
            rng: DetRng::new(seed ^ u64::from_le_bytes(me.0[..8].try_into().unwrap())),
            outbox: Vec::new(),
            read_round: 0,
            pending_reads: Vec::new(),
            ready_reads: Vec::new(),
            became_leader_terms: Vec::new(),
        };
        r.reset_election_timeout();
        Ok(r)
    }

    pub fn me(&self) -> NodeId {
        self.me
    }
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn term(&self) -> u64 {
        self.term
    }
    pub fn leader(&self) -> Option<NodeId> {
        self.leader
    }
    pub fn is_leader(&self) -> bool {
        self.role == Role::Leader
    }
    pub fn commit_index(&self) -> u64 {
        self.commit_index
    }
    pub fn last_index(&self) -> u64 {
        self.log.last().map(|e| e.index).unwrap_or(self.snapshot_index)
    }

    /// First index still stored; entries below it are covered by the applied-state snapshot.
    pub fn first_index(&self) -> u64 {
        self.snapshot_index + 1
    }

    /// Last index covered by the snapshot (0 when nothing has been compacted).
    pub fn snapshot_index(&self) -> u64 {
        self.snapshot_index
    }

    /// Position of `index` inside the stored log.
    fn log_pos(&self, index: u64) -> Option<usize> {
        if index <= self.snapshot_index {
            return None;
        }
        usize::try_from(index - self.snapshot_index - 1).ok()
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }
    pub fn into_storage(self) -> S {
        self.storage
    }
    pub fn log_len(&self) -> usize {
        self.log.len()
    }
    pub fn entry(&self, index: u64) -> Option<&Entry> {
        if index == 0 || index > self.last_index() {
            return None;
        }
        self.log.get(self.log_pos(index)?)
    }
    fn last_term(&self) -> u64 {
        self.log.last().map(|e| e.term).unwrap_or(self.snapshot_term)
    }
    fn term_at(&self, index: u64) -> u64 {
        if index == self.snapshot_index {
            return self.snapshot_term;
        }
        self.entry(index).map(|e| e.term).unwrap_or(0)
    }

    fn reset_election_timeout(&mut self) {
        let span = (self.cfg.election_ticks_max - self.cfg.election_ticks_min).max(1) as u64;
        self.election_timeout = self.cfg.election_ticks_min + self.rng.below(span) as u32;
        self.election_elapsed = 0;
    }

    fn send(&mut self, to: NodeId, msg: Message) {
        self.outbox.push(Envelope {
            from: self.me,
            to,
            msg,
        });
    }

    /// Messages produced since the last drain, in order.
    pub fn take_outbox(&mut self) -> Vec<Envelope> {
        std::mem::take(&mut self.outbox)
    }

    /// Newly committed entries in log order (each entry delivered exactly once per process life;
    /// after a restart the caller must re-apply from its own applied frontier).
    pub fn take_committed(&mut self) -> Vec<Entry> {
        let mut out = Vec::new();
        while self.last_delivered < self.commit_index {
            let next = self.last_delivered + 1;
            match self.entry(next) {
                Some(e) => out.push(e.clone()),
                None => break,
            }
            self.last_delivered = next;
        }
        out
    }

    /// After a restart the application tells the log which prefix it already applied durably.
    /// Record how far the state machine has durably applied. Lagging behind the truth is safe:
    /// it only holds the compaction horizon back.
    pub fn set_applied(&mut self, applied: u64) {
        self.last_delivered = applied.min(self.last_index()).max(self.snapshot_index);
        self.applied = self.applied.max(applied.min(self.last_index()));
    }

    /// Test hook: pretend a follower asked for `next`, to exercise the refusal path of a peer
    /// whose entries this leader has compacted away.
    #[cfg(test)]
    pub fn force_next_index(&mut self, peer: NodeId, next: u64) {
        self.next_index.insert(peer, next);
    }

    /// Highest index every voter has durably applied, as far as this node knows. A member that
    /// has never answered counts as zero, so a leader that has just been elected compacts nothing
    /// until it has heard from the others.
    pub fn compaction_horizon(&self) -> u64 {
        let mut h = self.applied;
        for m in &self.cfg.members {
            if *m == self.me {
                continue;
            }
            h = h.min(self.peer_applied.get(m).copied().unwrap_or(0));
        }
        h.min(self.commit_index)
    }

    /// Drop the log prefix every voter has applied, up to `up_to`. Returns the new base. The term
    /// of the base must still be known, so the prefix is never cut past what this node stores.
    pub fn compact(&mut self, up_to: u64) -> CoreResult<u64> {
        let target = up_to.min(self.compaction_horizon()).min(self.last_index());
        if target <= self.snapshot_index {
            return Ok(self.snapshot_index);
        }
        let term = self.term_at(target);
        if term == 0 {
            return Ok(self.snapshot_index);
        }
        self.storage.compact_to(target, term)?;
        let keep = self.log_pos(target + 1).unwrap_or(self.log.len());
        self.log.drain(..keep.min(self.log.len()));
        self.snapshot_index = target;
        self.snapshot_term = term;
        self.last_delivered = self.last_delivered.max(target);
        self.applied = self.applied.max(target);
        Ok(target)
    }

    pub fn take_ready_reads(&mut self) -> Vec<ReadIndex> {
        std::mem::take(&mut self.ready_reads)
    }

    fn persist_term_vote(&mut self) -> CoreResult<()> {
        self.storage.save_term_vote(self.term, self.voted_for)
    }

    fn become_follower(&mut self, term: u64, leader: Option<NodeId>) -> CoreResult<()> {
        if term > self.term {
            self.term = term;
            self.voted_for = None;
            self.persist_term_vote()?;
        }
        self.role = Role::Follower;
        self.leader = leader;
        self.votes.clear();
        self.pending_reads.clear();
        self.reset_election_timeout();
        Ok(())
    }

    fn become_candidate(&mut self) -> CoreResult<()> {
        self.term += 1;
        self.voted_for = Some(self.me);
        self.persist_term_vote()?;
        self.role = Role::Candidate;
        self.leader = None;
        self.votes.clear();
        self.votes.insert(self.me);
        self.reset_election_timeout();
        let (li, lt) = (self.last_index(), self.last_term());
        let term = self.term;
        for m in self.cfg.members.clone() {
            if m != self.me {
                self.send(
                    m,
                    Message::RequestVote {
                        term,
                        last_log_index: li,
                        last_log_term: lt,
                    },
                );
            }
        }
        if self.votes.len() >= self.cfg.quorum() {
            self.become_leader()?;
        }
        Ok(())
    }

    fn become_leader(&mut self) -> CoreResult<()> {
        self.role = Role::Leader;
        self.leader = Some(self.me);
        self.became_leader_terms.push(self.term);
        let next = self.last_index() + 1;
        for m in self.cfg.members.clone() {
            self.next_index.insert(m, next);
            self.match_index.insert(m, 0);
        }
        self.match_index.insert(self.me, self.last_index());
        // a leader commits entries of its own term only: append the term no-op (Raft §5.4.2)
        self.append_local(Vec::new())?;
        self.heartbeat_elapsed = self.cfg.heartbeat_ticks; // replicate immediately
        self.broadcast_append(0);
        Ok(())
    }

    fn append_local(&mut self, data: Vec<u8>) -> CoreResult<u64> {
        let e = Entry {
            term: self.term,
            index: self.last_index() + 1,
            data,
        };
        self.storage.append(std::slice::from_ref(&e))?;
        self.log.push(e);
        let li = self.last_index();
        self.match_index.insert(self.me, li);
        Ok(li)
    }

    /// Leader-only: append a command; returns its log index. The command is committed once a
    /// quorum has durably acknowledged it (observe via `take_committed`).
    pub fn propose(&mut self, data: Vec<u8>) -> CoreResult<u64> {
        if self.role != Role::Leader {
            return Err(CoreError::new(
                ErrorCode::AuthorityUnavailable,
                "not the leader",
            ));
        }
        if data.is_empty() {
            return Err(CoreError::new(ErrorCode::ProtocolError, "empty command"));
        }
        let idx = self.append_local(data)?;
        self.broadcast_append(0);
        Ok(idx)
    }

    /// Leader-only read barrier (SPEC-008 §5): a heartbeat round must be acknowledged by a quorum
    /// before the returned index counts as an authoritative committed frontier.
    pub fn read_barrier(&mut self) -> CoreResult<u64> {
        if self.role != Role::Leader {
            return Err(CoreError::new(
                ErrorCode::AuthorityUnavailable,
                "not the leader",
            ));
        }
        // a leader must have committed an entry of its own term first
        if self.term_at(self.commit_index) != self.term {
            return Err(CoreError::new(
                ErrorCode::NotReady,
                "leader has not committed an entry of its term yet",
            ));
        }
        self.read_round += 1;
        let round = self.read_round;
        let mut acks = BTreeSet::new();
        acks.insert(self.me);
        self.pending_reads.push((round, self.commit_index, acks));
        self.broadcast_append(round);
        Ok(round)
    }

    fn broadcast_append(&mut self, read_round: u64) {
        for m in self.cfg.members.clone() {
            if m != self.me {
                self.send_append(m, read_round);
            }
        }
        self.heartbeat_elapsed = 0;
    }

    fn send_append(&mut self, to: NodeId, read_round: u64) {
        let next = *self.next_index.get(&to).unwrap_or(&(self.snapshot_index + 1));
        if next <= self.snapshot_index {
            // The follower needs entries this leader compacted. v1 has no snapshot transfer, so
            // this fails closed and loudly instead of sending a log the follower cannot splice
            // (SPEC-011 §9: a lagging member is rebuilt, never silently repaired).
            self.followers_behind_snapshot += 1;
            return;
        }
        let prev_log_index = next.saturating_sub(1);
        let prev_log_term = self.term_at(prev_log_index);
        let entries: Vec<Entry> = match self.log_pos(next) {
            Some(pos) if next <= self.last_index() => self.log[pos..].to_vec(),
            _ => Vec::new(),
        };
        let msg = Message::AppendEntries {
            term: self.term,
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit: self.commit_index,
            read_round,
        };
        self.send(to, msg);
    }

    /// Logical clock tick: elections and heartbeats.
    pub fn tick(&mut self) -> CoreResult<()> {
        match self.role {
            Role::Leader => {
                self.heartbeat_elapsed += 1;
                if self.heartbeat_elapsed >= self.cfg.heartbeat_ticks {
                    self.broadcast_append(0);
                }
            }
            Role::Follower | Role::Candidate => {
                self.election_elapsed += 1;
                if self.election_elapsed >= self.election_timeout {
                    self.become_candidate()?;
                }
            }
        }
        Ok(())
    }

    pub fn step(&mut self, env: Envelope) -> CoreResult<()> {
        if env.to != self.me || !self.cfg.members.contains(&env.from) {
            return Ok(()); // not for us / not a voter: ignore
        }
        let from = env.from;
        match env.msg {
            Message::RequestVote {
                term,
                last_log_index,
                last_log_term,
            } => {
                if term > self.term {
                    self.become_follower(term, None)?;
                }
                let up_to_date = last_log_term > self.last_term()
                    || (last_log_term == self.last_term() && last_log_index >= self.last_index());
                let grant = term == self.term
                    && up_to_date
                    && (self.voted_for.is_none() || self.voted_for == Some(from));
                if grant {
                    self.voted_for = Some(from);
                    self.persist_term_vote()?;
                    self.reset_election_timeout();
                }
                let t = self.term;
                self.send(
                    from,
                    Message::RequestVoteReply {
                        term: t,
                        granted: grant,
                    },
                );
            }
            Message::RequestVoteReply { term, granted } => {
                if term > self.term {
                    self.become_follower(term, None)?;
                    return Ok(());
                }
                if self.role == Role::Candidate && term == self.term && granted {
                    self.votes.insert(from);
                    if self.votes.len() >= self.cfg.quorum() {
                        self.become_leader()?;
                    }
                }
            }
            Message::AppendEntries {
                term,
                prev_log_index,
                prev_log_term,
                entries,
                leader_commit,
                read_round,
            } => {
                if term < self.term {
                    let t = self.term;
                    self.send(
                        from,
                        Message::AppendEntriesReply {
                            term: t,
                            success: false,
                            match_index: self.last_index(),
                            read_round,
                            applied_index: self.applied,
                        },
                    );
                    return Ok(());
                }
                if term > self.term || self.role != Role::Follower {
                    self.become_follower(term, Some(from))?;
                } else {
                    self.leader = Some(from);
                    self.reset_election_timeout();
                }
                // log matching
                if prev_log_index > 0
                    && (prev_log_index > self.last_index()
                        || self.term_at(prev_log_index) != prev_log_term)
                {
                    let hint = self.last_index().min(prev_log_index.saturating_sub(1));
                    let t = self.term;
                    self.send(
                        from,
                        Message::AppendEntriesReply {
                            term: t,
                            success: false,
                            match_index: hint,
                            read_round,
                            applied_index: self.applied,
                        },
                    );
                    return Ok(());
                }
                // append new entries, truncating conflicts (never below commit_index)
                let mut first_new: Option<usize> = None;
                for (i, e) in entries.iter().enumerate() {
                    match self.entry(e.index) {
                        Some(mine) if mine.term == e.term => continue,
                        Some(_) => {
                            if e.index <= self.commit_index {
                                return Err(CoreError::new(
                                    ErrorCode::Corruption,
                                    "leader conflicts with a committed entry",
                                ));
                            }
                            self.storage.truncate_from(e.index)?;
                            if let Some(pos) = self.log_pos(e.index) {
                                self.log.truncate(pos);
                            }
                            first_new = Some(i);
                            break;
                        }
                        None => {
                            first_new = Some(i);
                            break;
                        }
                    }
                }
                if let Some(i) = first_new {
                    let new = &entries[i..];
                    self.storage.append(new)?;
                    self.log.extend_from_slice(new);
                }
                let last_new = if entries.is_empty() {
                    prev_log_index
                } else {
                    entries.last().unwrap().index
                };
                if leader_commit > self.commit_index {
                    self.commit_index = leader_commit.min(last_new).max(self.commit_index);
                }
                let t = self.term;
                self.send(
                    from,
                    Message::AppendEntriesReply {
                        term: t,
                        success: true,
                        match_index: last_new,
                        read_round,
                        applied_index: self.applied,
                    },
                );
            }
            Message::AppendEntriesReply {
                term,
                success,
                match_index,
                read_round,
                applied_index,
            } => {
                let known = self.peer_applied.entry(from).or_insert(0);
                *known = (*known).max(applied_index);
                if term > self.term {
                    self.become_follower(term, None)?;
                    return Ok(());
                }
                if self.role != Role::Leader || term != self.term {
                    return Ok(());
                }
                if success {
                    let cur = *self.match_index.get(&from).unwrap_or(&0);
                    if match_index > cur {
                        self.match_index.insert(from, match_index);
                    }
                    self.next_index.insert(from, match_index + 1);
                    self.advance_commit();
                    if read_round > 0 {
                        let quorum = self.cfg.quorum();
                        let mut done = Vec::new();
                        for (round, index, acks) in self.pending_reads.iter_mut() {
                            if *round <= read_round {
                                acks.insert(from);
                                if acks.len() >= quorum {
                                    done.push(ReadIndex {
                                        round: *round,
                                        index: *index,
                                    });
                                }
                            }
                        }
                        self.pending_reads
                            .retain(|(r, _, _)| !done.iter().any(|d| d.round == *r));
                        self.ready_reads.extend(done);
                    }
                    if *self.next_index.get(&from).unwrap_or(&1) <= self.last_index() {
                        self.send_append(from, 0);
                    }
                } else {
                    let next = (match_index + 1).max(1);
                    self.next_index.insert(from, next);
                    self.send_append(from, 0);
                }
            }
        }
        Ok(())
    }

    fn advance_commit(&mut self) {
        let mut indices: Vec<u64> = self
            .cfg
            .members
            .iter()
            .map(|m| *self.match_index.get(m).unwrap_or(&0))
            .collect();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        let q = indices[self.cfg.quorum() - 1];
        // only entries of the current term are committed by counting (Raft §5.4.2)
        if q > self.commit_index && self.term_at(q) == self.term {
            self.commit_index = q;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrip() {
        let e = Envelope {
            from: NodeId::derive("a"),
            to: NodeId::derive("b"),
            msg: Message::AppendEntries {
                term: 3,
                prev_log_index: 2,
                prev_log_term: 1,
                entries: vec![Entry {
                    term: 3,
                    index: 3,
                    data: b"x".to_vec(),
                }],
                leader_commit: 2,
                read_round: 7,
            },
        };
        let back = Envelope::decode(&e.encode(), &carolina_core::limits::Limits::v1()).unwrap();
        assert_eq!(back, e);
    }
}
