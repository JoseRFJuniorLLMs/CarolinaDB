//! Deterministic cluster simulator (SPEC-010 §6): seeded scheduler, message queue with delay,
//! duplication, loss and directional partitions, crash/restart from modeled durable state.
//!
//! Invariants checked after every delivered message (Raft safety properties):
//! * election safety — at most one leader per term;
//! * log matching — two logs agreeing on an index/term agree on every preceding entry;
//! * state-machine safety — committed prefixes never diverge across nodes;
//! * leader completeness — every entry committed in an earlier term is in a later leader's log.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use carolina_core::error::CoreResult;
use carolina_core::ids::NodeId;
use carolina_core::rng::DetRng;

use crate::{Config, Entry, Envelope, MemRaftStorage, Raft, Role};

#[derive(Debug, Clone)]
pub struct NetworkModel {
    /// Loss probability per message in 1/1000.
    pub loss_permille: u64,
    /// Duplication probability per message in 1/1000.
    pub dup_permille: u64,
    pub min_delay: u64,
    pub max_delay: u64,
}

impl Default for NetworkModel {
    fn default() -> Self {
        NetworkModel {
            loss_permille: 50,
            dup_permille: 50,
            min_delay: 1,
            max_delay: 4,
        }
    }
}

struct Node {
    raft: Option<Raft<MemRaftStorage>>,
    storage: Option<MemRaftStorage>,
    applied: Vec<Entry>,
}

pub struct Cluster {
    pub cfg: Config,
    nodes: Vec<Node>,
    rng: DetRng,
    seed: u64,
    net: NetworkModel,
    queue: VecDeque<(u64, Envelope)>,
    pub now: u64,
    blocked: BTreeSet<(NodeId, NodeId)>,
    leaders_by_term: BTreeMap<u64, NodeId>,
    pub delivered: u64,
    pub dropped: u64,
    pub duplicated: u64,
}

impl Cluster {
    pub fn new(n: usize, seed: u64, net: NetworkModel) -> Cluster {
        let members: Vec<NodeId> = (0..n)
            .map(|i| NodeId::derive(&format!("voter-{i}")))
            .collect();
        let cfg = Config::new(members.clone());
        let mut nodes = Vec::new();
        for m in &cfg.members {
            let storage = MemRaftStorage::default();
            let raft = Raft::new(*m, cfg.clone(), storage, seed).unwrap();
            nodes.push(Node {
                raft: Some(raft),
                storage: None,
                applied: Vec::new(),
            });
        }
        Cluster {
            cfg,
            nodes,
            rng: DetRng::new(seed),
            seed,
            net,
            queue: VecDeque::new(),
            now: 0,
            blocked: BTreeSet::new(),
            leaders_by_term: BTreeMap::new(),
            delivered: 0,
            dropped: 0,
            duplicated: 0,
        }
    }

    pub fn members(&self) -> &[NodeId] {
        &self.cfg.members
    }

    fn idx(&self, id: NodeId) -> usize {
        self.cfg.members.iter().position(|m| *m == id).unwrap()
    }

    pub fn node(&self, i: usize) -> Option<&Raft<MemRaftStorage>> {
        self.nodes[i].raft.as_ref()
    }
    pub fn node_mut(&mut self, i: usize) -> Option<&mut Raft<MemRaftStorage>> {
        self.nodes[i].raft.as_mut()
    }
    pub fn applied(&self, i: usize) -> &[Entry] {
        &self.nodes[i].applied
    }

    pub fn leader(&self) -> Option<usize> {
        let mut leaders: Vec<(u64, usize)> = self
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                n.raft
                    .as_ref()
                    .filter(|r| r.is_leader())
                    .map(|r| (r.term(), i))
            })
            .collect();
        leaders.sort();
        leaders.last().map(|(_, i)| *i)
    }

    /// Crash: volatile state lost; durable Raft state kept.
    pub fn crash(&mut self, i: usize) {
        if let Some(r) = self.nodes[i].raft.take() {
            self.nodes[i].storage = Some(r.into_storage());
        }
    }

    pub fn restart(&mut self, i: usize) -> CoreResult<()> {
        if self.nodes[i].raft.is_some() {
            return Ok(());
        }
        let storage = self.nodes[i].storage.take().unwrap_or_default();
        let me = self.cfg.members[i];
        let mut raft = Raft::new(me, self.cfg.clone(), storage, self.seed ^ self.now)?;
        // the application re-applies from its durably applied frontier (here: kept in memory as
        // the simulated durable state machine)
        raft.set_applied(self.nodes[i].applied.len() as u64);
        self.nodes[i].raft = Some(raft);
        Ok(())
    }

    pub fn is_up(&self, i: usize) -> bool {
        self.nodes[i].raft.is_some()
    }

    /// Directional partition: messages from `a` to `b` are dropped.
    pub fn block(&mut self, a: usize, b: usize) {
        self.blocked
            .insert((self.cfg.members[a], self.cfg.members[b]));
    }
    pub fn heal(&mut self) {
        self.blocked.clear();
    }
    /// Isolate node `i` in both directions.
    pub fn isolate(&mut self, i: usize) {
        for j in 0..self.nodes.len() {
            if j != i {
                self.block(i, j);
                self.block(j, i);
            }
        }
    }

    fn enqueue(&mut self, env: Envelope) {
        if self.blocked.contains(&(env.from, env.to)) {
            self.dropped += 1;
            return;
        }
        if self.rng.below(1000) < self.net.loss_permille {
            self.dropped += 1;
            return;
        }
        let copies = if self.rng.below(1000) < self.net.dup_permille {
            self.duplicated += 1;
            2
        } else {
            1
        };
        for _ in 0..copies {
            let delay =
                self.net.min_delay + self.rng.below(self.net.max_delay - self.net.min_delay + 1);
            self.queue.push_back((self.now + delay, env.clone()));
        }
    }

    fn drain_outboxes(&mut self) {
        let mut all = Vec::new();
        for n in self.nodes.iter_mut() {
            if let Some(r) = n.raft.as_mut() {
                all.extend(r.take_outbox());
            }
        }
        for e in all {
            self.enqueue(e);
        }
    }

    fn apply_committed(&mut self) {
        for n in self.nodes.iter_mut() {
            if let Some(r) = n.raft.as_mut() {
                for e in r.take_committed() {
                    n.applied.push(e);
                }
                // the simulated state machine is durable the moment it returns, so it reports its
                // applied frontier back: that is what lets a leader compact (SPEC-011 §9)
                if let Some(last) = n.applied.last() {
                    r.set_applied(last.index);
                }
            }
        }
    }

    /// One logical tick: every live node ticks, then every message due now is delivered in a
    /// seeded random order.
    pub fn step(&mut self) -> Result<(), String> {
        self.now += 1;
        for n in self.nodes.iter_mut() {
            if let Some(r) = n.raft.as_mut() {
                r.tick().map_err(|e| e.to_string())?;
            }
        }
        self.drain_outboxes();
        let mut due: Vec<Envelope> = Vec::new();
        let mut rest = VecDeque::new();
        while let Some((at, env)) = self.queue.pop_front() {
            if at <= self.now {
                due.push(env);
            } else {
                rest.push_back((at, env));
            }
        }
        self.queue = rest;
        // shuffle deterministically
        for i in (1..due.len()).rev() {
            let j = self.rng.below(i as u64 + 1) as usize;
            due.swap(i, j);
        }
        for env in due {
            let to = self.idx(env.to);
            if let Some(r) = self.nodes[to].raft.as_mut() {
                r.step(env).map_err(|e| e.to_string())?;
                self.delivered += 1;
            } else {
                self.dropped += 1; // crashed node: message lost
            }
            self.drain_outboxes();
            self.apply_committed();
            self.check_invariants()?;
        }
        self.apply_committed();
        self.check_invariants()
    }

    pub fn run(&mut self, ticks: u64) -> Result<(), String> {
        for _ in 0..ticks {
            self.step()?;
        }
        Ok(())
    }

    /// Run until a leader exists and has committed its term no-op, or `max_ticks` elapse.
    pub fn run_until_leader(&mut self, max_ticks: u64) -> Result<Option<usize>, String> {
        for _ in 0..max_ticks {
            self.step()?;
            if let Some(l) = self.leader() {
                let r = self.nodes[l].raft.as_ref().unwrap();
                if r.commit_index() > 0
                    && r.entry(r.commit_index()).map(|e| e.term) == Some(r.term())
                {
                    return Ok(Some(l));
                }
            }
        }
        Ok(None)
    }

    /// Propose through the current leader (if any); returns the log index.
    pub fn propose(&mut self, data: Vec<u8>) -> Option<u64> {
        let l = self.leader()?;
        let r = self.nodes[l].raft.as_mut()?;
        r.propose(data).ok()
    }

    /// Whether every live node applied at least `index` entries.
    pub fn all_applied(&self, index: u64) -> bool {
        self.nodes
            .iter()
            .filter(|n| n.raft.is_some())
            .all(|n| n.applied.len() as u64 >= index)
    }

    pub fn check_invariants(&mut self) -> Result<(), String> {
        // election safety
        for n in &self.nodes {
            if let Some(r) = &n.raft {
                if r.role() == Role::Leader {
                    match self.leaders_by_term.get(&r.term()) {
                        Some(other) if *other != r.me() => {
                            return Err(format!(
                                "election safety: two leaders in term {}",
                                r.term()
                            ));
                        }
                        _ => {
                            self.leaders_by_term.insert(r.term(), r.me());
                        }
                    }
                }
            }
        }
        // log matching across live nodes
        let live: Vec<&Raft<MemRaftStorage>> =
            self.nodes.iter().filter_map(|n| n.raft.as_ref()).collect();
        for a in 0..live.len() {
            for b in (a + 1)..live.len() {
                let (ra, rb) = (live[a], live[b]);
                // a compacted node no longer stores its prefix: compare only what both still hold
                let first = ra.first_index().max(rb.first_index());
                let common = ra.last_index().min(rb.last_index());
                let mut agreed_upto = 0;
                for i in (first..=common).rev() {
                    let (ea, eb) = match (ra.entry(i), rb.entry(i)) {
                        (Some(ea), Some(eb)) => (ea, eb),
                        _ => continue,
                    };
                    if ea.term == eb.term {
                        agreed_upto = i;
                        break;
                    }
                }
                for i in first..=agreed_upto {
                    if ra.entry(i) != rb.entry(i) {
                        return Err(format!("log matching violated at index {i}"));
                    }
                }
            }
        }
        // state machine safety: applied sequences are prefix-consistent
        let mut longest: Option<&Vec<Entry>> = None;
        for n in &self.nodes {
            if longest.map(|l| n.applied.len() > l.len()).unwrap_or(true) {
                longest = Some(&n.applied);
            }
        }
        if let Some(l) = longest {
            for n in &self.nodes {
                for (i, e) in n.applied.iter().enumerate() {
                    if l[i] != *e {
                        return Err(format!(
                            "state machine safety violated at applied index {}",
                            i + 1
                        ));
                    }
                }
            }
            // leader completeness: every applied (committed) entry is in the current leader's log
            if let Some(li) = self.leader() {
                let r = self.nodes[li].raft.as_ref().unwrap();
                for e in l {
                    // entries the leader compacted are covered by its applied state, which is
                    // exactly what the snapshot base asserts; below the base there is nothing to
                    // compare against
                    if e.index <= r.snapshot_index() {
                        continue;
                    }
                    if r.entry(e.index) != Some(e) {
                        return Err(format!(
                            "leader completeness violated: leader lacks committed index {}",
                            e.index
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Deterministic-simulation campaign (SPEC-010 §6): for every seed, elections and replication
/// under loss/duplication/delay with crash/restart of each voter, an isolated minority leader
/// (no commit, no read barrier, stale entry never applied), and reproducibility of the trace.
pub fn campaign(
    seeds: &[u64],
    proposals: u64,
) -> Result<std::collections::BTreeMap<String, u64>, String> {
    let mut m = std::collections::BTreeMap::new();
    let cmd = |i: u64| format!("cmd-{i}").into_bytes();
    for seed in seeds {
        let mut c = Cluster::new(3, *seed, NetworkModel::default());
        c.run_until_leader(600)?
            .ok_or_else(|| format!("seed {seed}: no leader within 600 ticks"))?;
        let mut last = 0;
        for i in 1..=proposals {
            let mut tries = 0;
            loop {
                if let Some(idx) = c.propose(cmd(i)) {
                    last = idx;
                    break;
                }
                c.run(1)?;
                tries += 1;
                if tries > 2000 {
                    return Err(format!("seed {seed}: could not propose {i}"));
                }
            }
            c.run(2)?;
            if i.is_multiple_of(7) {
                let victim = (i as usize) % 3;
                c.crash(victim);
                c.run(25)?;
                c.restart(victim).map_err(|e| e.to_string())?;
            }
        }
        let mut ok = false;
        for _ in 0..3000 {
            c.run(1)?;
            if c.all_applied(last) {
                ok = true;
                break;
            }
        }
        if !ok {
            return Err(format!("seed {seed}: not all voters applied index {last}"));
        }
        let applied = c.applied(0).iter().filter(|e| !e.data.is_empty()).count();
        if applied != proposals as usize {
            return Err(format!(
                "seed {seed}: {applied} commands applied, expected {proposals}"
            ));
        }
        *m.entry("delivered".to_string()).or_insert(0) += c.delivered;
        *m.entry("dropped".to_string()).or_insert(0) += c.dropped;
        *m.entry("duplicated".to_string()).or_insert(0) += c.duplicated;
        *m.entry("ticks".to_string()).or_insert(0) += c.now;
        // isolated leader: no commit, no read barrier; the stale entry never applies anywhere
        let mut c = Cluster::new(
            3,
            seed ^ 0x5151,
            NetworkModel {
                loss_permille: 0,
                dup_permille: 0,
                min_delay: 1,
                max_delay: 2,
            },
        );
        let l = c
            .run_until_leader(600)?
            .ok_or_else(|| format!("seed {seed}: no leader"))?;
        c.isolate(l);
        let commit_before = c.node(l).unwrap().commit_index();
        let _ = c.node_mut(l).unwrap().propose(b"stale".to_vec());
        c.run(80)?;
        if c.node(l).unwrap().commit_index() != commit_before {
            return Err(format!(
                "seed {seed}: isolated leader advanced its commit index"
            ));
        }
        let round = c.node_mut(l).unwrap().read_barrier();
        c.run(30)?;
        if let Ok(r) = round {
            if c.node_mut(l)
                .unwrap()
                .take_ready_reads()
                .iter()
                .any(|x| x.round == r)
            {
                return Err(format!(
                    "seed {seed}: isolated leader passed a read barrier"
                ));
            }
        }
        c.heal();
        let mut idx = None;
        for _ in 0..600 {
            c.run(1)?;
            match idx {
                None => idx = c.propose(b"majority".to_vec()),
                Some(i) if c.all_applied(i) => break,
                _ => {}
            }
        }
        for i in 0..3 {
            if c.applied(i).iter().any(|e| e.data == b"stale") {
                return Err(format!("seed {seed}: voter {i} applied the stale entry"));
            }
        }
        *m.entry("isolation_cases".to_string()).or_insert(0) += 1;
        let run = |s: u64| {
            let mut c = Cluster::new(3, s, NetworkModel::default());
            let _ = c.run_until_leader(300);
            for i in 1..=5u64 {
                let mut n = 0;
                while c.propose(cmd(i)).is_none() && n < 500 {
                    let _ = c.run(1);
                    n += 1;
                }
                let _ = c.run(3);
            }
            let _ = c.run(60);
            (c.applied(0).to_vec(), c.delivered, c.dropped, c.now)
        };
        if run(*seed) != run(*seed) {
            return Err(format!("seed {seed}: the simulation is not reproducible"));
        }
    }
    m.insert("seeds".to_string(), seeds.len() as u64);
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(i: u64) -> Vec<u8> {
        format!("cmd-{i}").into_bytes()
    }

    #[test]
    fn campaign_runs() {
        let m = campaign(&[21, 22], 20).unwrap();
        assert_eq!(m["seeds"], 2);
    }

    #[test]
    fn elects_and_replicates_under_loss_and_duplication() {
        for seed in 1..=6u64 {
            let mut c = Cluster::new(3, seed, NetworkModel::default());
            let l = c.run_until_leader(400).unwrap().expect("a leader");
            assert!(c.node(l).unwrap().is_leader());
            let mut last = 0;
            for i in 1..=40u64 {
                loop {
                    if let Some(idx) = c.propose(cmd(i)) {
                        last = idx;
                        break;
                    }
                    c.run(1).unwrap();
                }
                c.run(2).unwrap();
            }
            let mut ok = false;
            for _ in 0..600 {
                c.run(1).unwrap();
                if c.all_applied(last) {
                    ok = true;
                    break;
                }
            }
            assert!(ok, "seed {seed}: not all nodes applied {last}");
            let a: Vec<&[u8]> = c
                .applied(0)
                .iter()
                .map(|e| e.data.as_slice())
                .filter(|d| !d.is_empty())
                .collect();
            assert_eq!(a.len(), 40);
            assert_eq!(a[0], b"cmd-1");
            assert_eq!(a[39], b"cmd-40");
        }
    }

    #[test]
    fn minority_leader_cannot_commit_and_loses_to_the_majority() {
        let mut c = Cluster::new(
            3,
            42,
            NetworkModel {
                loss_permille: 0,
                dup_permille: 0,
                min_delay: 1,
                max_delay: 2,
            },
        );
        let l = c.run_until_leader(300).unwrap().unwrap();
        // isolate the leader: it keeps believing it leads for a while but cannot commit
        c.isolate(l);
        let stale_idx = c.node_mut(l).unwrap().propose(b"stale".to_vec()).unwrap();
        let commit_before = c.node(l).unwrap().commit_index();
        c.run(60).unwrap();
        assert_eq!(
            c.node(l).unwrap().commit_index(),
            commit_before,
            "isolated leader must not commit"
        );
        let l2 = c.leader();
        assert!(
            l2.is_some() && l2 != Some(l),
            "majority must elect a new leader"
        );
        let idx = loop {
            if let Some(i) = c.propose(b"majority".to_vec()) {
                break i;
            }
            c.run(1).unwrap();
        };
        c.run(40).unwrap();
        // the stale leader's read barrier can never complete
        assert!(
            c.node_mut(l).unwrap().read_barrier().is_err()
                || c.node(l).unwrap().role() != Role::Leader
                || c.node_mut(l).unwrap().take_ready_reads().is_empty()
        );
        c.heal();
        c.run(80).unwrap();
        assert!(c.all_applied(idx));
        // the stale entry was overwritten, never applied anywhere
        for i in 0..3 {
            assert!(
                !c.applied(i).iter().any(|e| e.data == b"stale"),
                "node {i} applied the stale entry"
            );
        }
        assert!(
            c.node(l)
                .unwrap()
                .entry(stale_idx)
                .map(|e| e.data.as_slice())
                != Some(b"stale")
        );
    }

    #[test]
    fn crashes_and_restarts_keep_committed_entries() {
        for seed in [7u64, 8, 9] {
            let mut c = Cluster::new(
                3,
                seed,
                NetworkModel {
                    loss_permille: 20,
                    dup_permille: 20,
                    min_delay: 1,
                    max_delay: 3,
                },
            );
            c.run_until_leader(300).unwrap().unwrap();
            let mut last = 0;
            for i in 1..=30u64 {
                loop {
                    if let Some(idx) = c.propose(cmd(i)) {
                        last = idx;
                        break;
                    }
                    c.run(1).unwrap();
                }
                c.run(2).unwrap();
                if i.is_multiple_of(7) {
                    let victim = (i as usize) % 3;
                    c.crash(victim);
                    c.run(25).unwrap();
                    c.restart(victim).unwrap();
                }
            }
            let mut ok = false;
            for _ in 0..800 {
                c.run(1).unwrap();
                if c.all_applied(last) {
                    ok = true;
                    break;
                }
            }
            assert!(ok, "seed {seed}");
            let a: Vec<&[u8]> = c
                .applied(1)
                .iter()
                .map(|e| e.data.as_slice())
                .filter(|d| !d.is_empty())
                .collect();
            assert_eq!(a.len(), 30, "seed {seed}");
        }
    }

    /// SPEC-011 §9 / SPEC-008 §5: once every voter has durably applied a prefix, the leader may
    /// drop it. The cluster keeps electing, replicating and committing across the new base, a
    /// restarted voter comes back on that base instead of replaying from index 1, and a leader
    /// never serves entries it no longer holds.
    #[test]
    fn compaction_drops_the_applied_prefix_and_survives_restart() {
        let mut c = Cluster::new(
            3,
            41,
            NetworkModel {
                loss_permille: 10,
                dup_permille: 10,
                min_delay: 1,
                max_delay: 3,
            },
        );
        let leader = c.run_until_leader(300).unwrap().unwrap();
        let mut last = 0;
        for i in 1..=20u64 {
            loop {
                if let Some(idx) = c.propose(cmd(i)) {
                    last = idx;
                    break;
                }
                c.run(1).unwrap();
            }
            c.run(2).unwrap();
        }
        let mut ok = false;
        for _ in 0..800 {
            c.run(1).unwrap();
            if c.all_applied(last) {
                ok = true;
                break;
            }
        }
        assert!(ok, "every voter must apply before anything may be compacted");
        // the leader learns each voter's applied index from their replies
        c.run(40).unwrap();
        let leader = c.leader().unwrap_or(leader);
        let horizon = c.node(leader).unwrap().compaction_horizon();
        assert!(
            horizon >= last,
            "horizon {horizon} must cover the applied prefix {last}"
        );
        let base = c.node_mut(leader).unwrap().compact(horizon).unwrap();
        assert!(base >= last, "compaction must reach the horizon");
        {
            let r = c.node(leader).unwrap();
            assert_eq!(r.first_index(), base + 1);
            assert_eq!(r.snapshot_index(), base);
            assert!(r.entry(base).is_none(), "the prefix is gone");
            assert!(r.last_index() >= base);
        }
        c.check_invariants().unwrap();

        // the cluster keeps working across the base
        for i in 21..=30u64 {
            loop {
                if let Some(idx) = c.propose(cmd(i)) {
                    last = idx;
                    break;
                }
                c.run(1).unwrap();
            }
            c.run(2).unwrap();
        }
        let mut ok = false;
        for _ in 0..800 {
            c.run(1).unwrap();
            if c.all_applied(last) {
                ok = true;
                break;
            }
        }
        assert!(ok, "commits must continue after compaction");
        c.check_invariants().unwrap();

        // a restart resumes on the compacted base, not from index 1
        c.crash(leader);
        c.run(30).unwrap();
        c.restart(leader).unwrap();
        c.run(60).unwrap();
        assert_eq!(c.node(leader).unwrap().snapshot_index(), base);
        assert_eq!(c.node(leader).unwrap().first_index(), base + 1);
        c.check_invariants().unwrap();
        let applied: Vec<&[u8]> = c
            .applied(leader)
            .iter()
            .map(|e| e.data.as_slice())
            .filter(|d| !d.is_empty())
            .collect();
        assert_eq!(applied.len(), 30, "no applied entry may be lost");
    }

    /// A voter that falls behind the leader's snapshot cannot be repaired by the log: v1 has no
    /// snapshot transfer, so the leader refuses to serve it and says so.
    #[test]
    fn a_voter_behind_the_snapshot_is_refused_not_silently_broken() {
        let mut c = Cluster::new(3, 43, NetworkModel::default());
        let leader = c.run_until_leader(300).unwrap().unwrap();
        let mut last = 0;
        for i in 1..=10u64 {
            loop {
                if let Some(idx) = c.propose(cmd(i)) {
                    last = idx;
                    break;
                }
                c.run(1).unwrap();
            }
            c.run(2).unwrap();
        }
        for _ in 0..800 {
            c.run(1).unwrap();
            if c.all_applied(last) {
                break;
            }
        }
        c.run(40).unwrap();
        let leader = c.leader().unwrap_or(leader);
        let base = {
            let horizon = c.node(leader).unwrap().compaction_horizon();
            c.node_mut(leader).unwrap().compact(horizon).unwrap()
        };
        assert!(base > 0);
        // pretend a follower asked for an index the leader no longer has
        let follower = (0..3).find(|i| *i != leader).unwrap();
        let peer = c.members()[follower];
        {
            let r = c.node_mut(leader).unwrap();
            let before = r.followers_behind_snapshot;
            // the leader only sends on its heartbeat, so give it a few ticks; next_index stays
            // where it was put because no reply is delivered in this loop
            for _ in 0..20 {
                r.force_next_index(peer, 1);
                r.tick().unwrap();
            }
            assert!(
                r.followers_behind_snapshot > before,
                "the leader must report a follower it cannot serve"
            );
        }
        c.check_invariants().unwrap();
    }

    #[test]
    fn read_barrier_completes_only_with_a_quorum() {
        let mut c = Cluster::new(
            3,
            5,
            NetworkModel {
                loss_permille: 0,
                dup_permille: 0,
                min_delay: 1,
                max_delay: 1,
            },
        );
        let l = c.run_until_leader(300).unwrap().unwrap();
        let round = c.node_mut(l).unwrap().read_barrier().unwrap();
        c.run(5).unwrap();
        let ready = c.node_mut(l).unwrap().take_ready_reads();
        assert!(
            ready.iter().any(|r| r.round == round),
            "barrier must complete with a quorum"
        );
        c.isolate(l);
        let round2 = c.node_mut(l).unwrap().read_barrier().unwrap();
        c.run(30).unwrap();
        let ready = c.node_mut(l).unwrap().take_ready_reads();
        assert!(
            !ready.iter().any(|r| r.round == round2),
            "isolated leader must not pass a barrier"
        );
    }

    #[test]
    fn same_seed_reproduces_the_same_history() {
        let run = |seed: u64| {
            let mut c = Cluster::new(3, seed, NetworkModel::default());
            c.run_until_leader(300).unwrap();
            for i in 1..=10u64 {
                while c.propose(cmd(i)).is_none() {
                    c.run(1).unwrap();
                }
                c.run(3).unwrap();
            }
            c.run(100).unwrap();
            (
                c.applied(0).to_vec(),
                c.delivered,
                c.dropped,
                c.duplicated,
                c.now,
            )
        };
        assert_eq!(run(11), run(11));
    }
}
