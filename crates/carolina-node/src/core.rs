//! The node's single-threaded core: consensus, catalog and the ordered C5 executor.
//!
//! Events arrive from the transport threads; every state change of the catalog or the engine
//! happens only through committed log entries applied in order, so the three voters converge on
//! byte-identical receipts and rows. Client replies for admitted requests are sent only once the
//! unique `Decision` entry is committed (SPEC-008 §8/§10).

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpListener;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Duration;

use carolina_catalog::{
    AuthorityGrant, Catalog, CatalogCommand, CatalogKey, Expected, GrantState, LogCommand,
    Mutation, Predicate, RequestRoute,
};
use carolina_consensus::{Config, Envelope, FileRaftStorage, Raft, ReadIndex};
use carolina_core::canon::Canonical;
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::Hash256;
use carolina_core::ids::*;
use carolina_core::limits::Limits;
use carolina_runtime::engine::{EngineOptions, LocalEngine};
use carolina_runtime::LocalCatalog;
use carolina_storage::io::NoFaults;
use carolina_storage::kernel::{state_digest, VerifyMode};
use carolina_storage::StoreOptions;
use carolina_wire::envelope::MessageKind;
use carolina_wire::negotiation::EndpointRole;
use carolina_wire::records::*;

use crate::config::NodeConfig;
use crate::protocol::{AdminReply, AdminRequest, NodeCommand, NodeStatus};
use crate::transport::{capabilities, peer_link, serve, ConnId, Connections, Event, Outbound};

const ROUTE_BUCKET: u32 = 0;

struct Waiter {
    conn: ConnId,
    stream_id: u64,
}

struct PendingInvoke {
    request_hash: RequestHash,
    waiters: Vec<Waiter>,
}

struct PendingAdmin {
    command_hash: Hash256,
    waiters: Vec<Waiter>,
}

pub struct NodeCore {
    cfg: NodeConfig,
    me: NodeId,
    raft: Raft<FileRaftStorage>,
    catalog: Catalog,
    engine: LocalEngine,
    manifest_hash: Hash256,
    peer_tx: BTreeMap<NodeId, Sender<Envelope>>,
    conns: Arc<Connections>,
    applied_index: u64,
    decided: BTreeSet<RequestKey>,
    last_exec: BTreeMap<RequestKey, ClientReplyV1>,
    invoke_waiters: BTreeMap<RequestKey, PendingInvoke>,
    admin_waiters: BTreeMap<AdminRequestId, PendingAdmin>,
    seed_waiters: BTreeMap<String, Vec<Waiter>>,
    resolve_waiters: Vec<(u64, Waiter, ResolveRequestV1)>,
    ready_reads_backlog: Vec<ReadIndex>,
    decision_proposals: BTreeMap<RequestKey, u64>,
    was_leader: bool,
    /// (term, log index) of the bootstrap proposal awaiting application.
    bootstrap_pending: Option<(u64, u64)>,
    failure: Option<String>,
    grant_id: GrantId,
    tenant: TenantId,
    namespace: RequestNamespace,
}

fn refusal(code: &str, detail: String) -> ClientReplyV1 {
    ClientReplyV1::Unavailable(RefusalV1 {
        code: code.into(),
        detail,
        possibly_admitted: false,
    })
}

impl NodeCore {
    fn open(
        cfg: NodeConfig,
        conns: Arc<Connections>,
        peer_tx: BTreeMap<NodeId, Sender<Envelope>>,
    ) -> CoreResult<NodeCore> {
        let me = cfg.node_id();
        let manifest_hash = cfg.manifest.manifest_hash();
        if !cfg.manifest.voters.contains(&me) {
            return Err(CoreError::new(
                ErrorCode::InvalidManifest,
                "this node is not a voter of the pinned manifest",
            ));
        }
        let src = cfg.module_source()?;
        let catalog_compiled = Arc::new(LocalCatalog::from_source(&src)?);
        let data = cfg.data_dir();
        std::fs::create_dir_all(&data)?;
        let store_dir = data.join("store");
        let opts = EngineOptions {
            store: StoreOptions {
                faults: Arc::new(NoFaults),
                ..Default::default()
            },
            home_id: cfg.home_id(),
            cluster_id: cfg.manifest.cluster_id,
            pinned_home_epoch: Some(RequestHomeEpoch(1)),
        };
        let engine = if store_dir.join("MANIFEST.A").exists() {
            LocalEngine::open(&store_dir, catalog_compiled, opts)?
        } else {
            LocalEngine::create(&store_dir, catalog_compiled, opts)?
        };
        let storage = FileRaftStorage::open(&data.join("raft"))?;
        let mut rcfg = Config::new(cfg.manifest.voters.clone());
        rcfg.election_ticks_min = 10;
        rcfg.election_ticks_max = 20;
        rcfg.heartbeat_ticks = 3;
        let raft = Raft::new(me, rcfg, storage, cfg.seed)?;
        let tenant = TenantId::derive("tenant-c5");
        let namespace = engine
            .catalog
            .module
            .operations
            .first()
            .map(|o| RequestNamespace::derive(&o.contract.request_namespace))
            .unwrap_or_else(|| RequestNamespace::derive("default"));
        Ok(NodeCore {
            grant_id: GrantId::derive(&format!("{}/home-grant", cfg.home)),
            cfg,
            me,
            raft,
            catalog: Catalog::new(),
            engine,
            manifest_hash,
            peer_tx,
            conns,
            applied_index: 0,
            decided: BTreeSet::new(),
            last_exec: BTreeMap::new(),
            invoke_waiters: BTreeMap::new(),
            admin_waiters: BTreeMap::new(),
            seed_waiters: BTreeMap::new(),
            resolve_waiters: Vec::new(),
            ready_reads_backlog: Vec::new(),
            decision_proposals: BTreeMap::new(),
            was_leader: false,
            bootstrap_pending: None,
            failure: None,
            tenant,
            namespace,
        })
    }

    fn fail(&mut self, reason: String) {
        if self.failure.is_none() {
            eprintln!("[{}] FAIL-CLOSED: {reason}", self.cfg.node);
            self.failure = Some(reason);
        }
    }

    fn ready(&self) -> bool {
        self.failure.is_none()
            && self.catalog.genesis().is_some()
            && self.catalog.admission_allowed(self.grant_id, self.me)
            && self.raft.leader().is_some()
    }

    fn readiness_label(&self) -> String {
        if self.failure.is_some() {
            "Failed".into()
        } else if self.catalog.genesis().is_none() {
            "WaitingGenesis".into()
        } else if !self.catalog.admission_allowed(self.grant_id, self.me) {
            "WaitingGrant".into()
        } else if self.raft.leader().is_none() {
            "NoLeader".into()
        } else {
            "Ready".into()
        }
    }

    fn status(&mut self) -> NodeStatus {
        let digest = state_digest(self.engine.store()).unwrap_or(Hash256::ZERO);
        NodeStatus {
            node: self.me,
            readiness: self.readiness_label(),
            role: format!("{:?}", self.raft.role()),
            term: self.raft.term(),
            leader: self.raft.leader(),
            commit_index: self.raft.commit_index(),
            applied_index: self.applied_index,
            catalog_generation: self.catalog.generation(),
            genesis: self.catalog.genesis().is_some(),
            grant_active: self.catalog.admission_allowed(self.grant_id, self.me),
            state_digest: digest,
            durable_commit_seq: self.engine.store().durable_commit_seq().0,
            failure: self.failure.clone(),
        }
    }

    fn send_out(&mut self) {
        for env in self.raft.take_outbox() {
            if let Some(tx) = self.peer_tx.get(&env.to) {
                let _ = tx.send(env);
            }
        }
    }

    fn reply(&self, w: &Waiter, kind: MessageKind, payload: Vec<u8>) {
        self.conns.reply(
            w.conn,
            Outbound {
                kind,
                stream_id: w.stream_id,
                payload,
            },
        );
    }

    fn reply_client(&self, w: &Waiter, r: &ClientReplyV1) {
        self.reply(w, MessageKind::Reply, r.encode());
    }

    // ---- bootstrap (leader) -------------------------------------------------

    fn leader_duties(&mut self) -> CoreResult<()> {
        if !self.raft.is_leader() || self.failure.is_some() {
            return Ok(());
        }
        // Execution belongs to the committed log, not to the leader that admitted it. A new
        // leader must finish every inherited admission even when its client never retries.
        self.propose_pending_decisions()?;
        let term = self.raft.term();
        if let Some((t, idx)) = self.bootstrap_pending {
            if t == term && self.applied_index < idx {
                return Ok(()); // the previous bootstrap step is still in flight
            }
        }
        if self.catalog.genesis().is_none() {
            let idx = self
                .raft
                .propose(NodeCommand::Genesis(self.cfg.manifest.clone()).encode())?;
            self.bootstrap_pending = Some((term, idx));
            return Ok(());
        }
        let admin = self.cfg.manifest.bootstrap_admin;
        let home = self.cfg.home_id();
        match self.catalog.grant(self.grant_id) {
            None => {
                let grant = AuthorityGrant {
                    grant_id: self.grant_id,
                    binding: AuthorityBinding::RequestHome {
                        home_id: home,
                        epoch: RequestHomeEpoch(1),
                    },
                    tenant: self.tenant,
                    scope: SemanticScopeId::derive("inventory"),
                    scope_records: self
                        .engine
                        .catalog
                        .module
                        .records
                        .iter()
                        .map(|r| r.id)
                        .collect(),
                    plans: self
                        .engine
                        .catalog
                        .output
                        .plans
                        .iter()
                        .map(|p| p.plan_ref())
                        .collect(),
                    placement_epoch: PlacementEpoch(1),
                    membership: (
                        ReplicationGroupId::derive("voters"),
                        MembershipGeneration(1),
                    ),
                    admitted_nodes: self.cfg.manifest.voters.clone(),
                    authority_durability_policy: "quorum-of-3".into(),
                    security_policy: self.cfg.manifest.security_policy,
                    admission_mode: "ONLINE_BARRIER".into(),
                    state: GrantState::Staged,
                };
                let cmd = CatalogCommand {
                    admin_request_id: AdminRequestId::derive("bootstrap/stage-home-grant"),
                    principal: admin,
                    expected: vec![(CatalogKey::Authority(self.grant_id), Expected::Absent)],
                    predicates: vec![],
                    mutations: vec![Mutation::PutImmutable {
                        key: CatalogKey::Authority(self.grant_id),
                        value: grant.to_canon(),
                    }],
                };
                let idx = self.raft.propose(NodeCommand::Catalog(cmd).encode())?;
                self.bootstrap_pending = Some((term, idx));
            }
            Some(g) if g.state == GrantState::Staged => {
                let cmd = CatalogCommand {
                    admin_request_id: AdminRequestId::derive("bootstrap/activate-home-grant"),
                    principal: admin,
                    expected: vec![],
                    predicates: vec![Predicate::GrantInState {
                        grant: self.grant_id,
                        state: GrantState::Staged,
                    }],
                    mutations: vec![Mutation::AdvanceGrantState {
                        grant: self.grant_id,
                        from: GrantState::Staged,
                        to: GrantState::Active,
                    }],
                };
                let idx = self.raft.propose(NodeCommand::Catalog(cmd).encode())?;
                self.bootstrap_pending = Some((term, idx));
            }
            Some(_) => {
                if self
                    .catalog
                    .route(self.tenant, self.namespace, ROUTE_BUCKET)
                    .is_none()
                {
                    let key = CatalogKey::RequestRoute(self.tenant, self.namespace, ROUTE_BUCKET);
                    let cmd = CatalogCommand {
                        admin_request_id: AdminRequestId::derive("bootstrap/route-home"),
                        principal: admin,
                        expected: vec![(key.clone(), Expected::Absent)],
                        predicates: vec![Predicate::GrantInState {
                            grant: self.grant_id,
                            state: GrantState::Active,
                        }],
                        mutations: vec![Mutation::AdvancePointer {
                            key,
                            value: RequestRoute {
                                home_id: home,
                                home_epoch: RequestHomeEpoch(1),
                                home_grant: self.grant_id,
                            }
                            .to_canon(),
                        }],
                    };
                    let idx = self.raft.propose(NodeCommand::Catalog(cmd).encode())?;
                    self.bootstrap_pending = Some((term, idx));
                }
            }
        }
        Ok(())
    }

    fn propose_pending_decisions(&mut self) -> CoreResult<()> {
        for (key, reply) in &self.last_exec {
            if self.decided.contains(key) {
                continue;
            }
            let receipt = match reply {
                ClientReplyV1::Committed(r) | ClientReplyV1::Rejected(r) => r,
                _ => continue,
            };
            if self.decision_proposals.get(key).is_some_and(|index| {
                self.raft.entry(*index).is_some_and(|entry| {
                    matches!(NodeCommand::decode(&entry.data, &Limits::v1()),
                        Ok(NodeCommand::Decision { request_key, receipt_digest, txn_id, outcome, .. })
                        if request_key == *key && receipt_digest == receipt.receipt_digest().0
                            && txn_id == receipt.txn_id && outcome == receipt.outcome.label())
                })
            }) {
                continue;
            }
            // A previous leader's Decision may already be in the inherited uncommitted suffix.
            let inherited = (self.applied_index + 1..=self.raft.last_index()).find(|index| {
                self.raft.entry(*index).is_some_and(|entry| {
                    matches!(NodeCommand::decode(&entry.data, &Limits::v1()),
                        Ok(NodeCommand::Decision { request_key, receipt_digest, txn_id, outcome, .. })
                        if request_key == *key && receipt_digest == receipt.receipt_digest().0
                            && txn_id == receipt.txn_id && outcome == receipt.outcome.label())
                })
            });
            let index = match inherited {
                Some(index) => index,
                None => self.raft.propose(
                    NodeCommand::Decision {
                        request_key: *key,
                        txn_id: receipt.txn_id,
                        outcome: receipt.outcome.label().into(),
                        receipt_digest: receipt.receipt_digest().0,
                        decided_by: self.me,
                    }
                    .encode(),
                )?,
            };
            self.decision_proposals.insert(*key, index);
        }
        Ok(())
    }

    fn reply_invoke_waiters(&mut self, key: &RequestKey, reply: &ClientReplyV1) {
        if let Some(pending) = self.invoke_waiters.remove(key) {
            let mismatch = matches!(reply,
                ClientReplyV1::Committed(r) | ClientReplyV1::Rejected(r)
                if r.request_hash != pending.request_hash);
            let reply = if mismatch {
                &ClientReplyV1::RequestIdentityMismatch
            } else {
                reply
            };
            for w in pending.waiters {
                self.reply_client(&w, reply);
            }
        }
    }

    // ---- apply --------------------------------------------------------------

    fn apply_committed(&mut self) {
        for e in self.raft.take_committed() {
            if self.failure.is_some() {
                return;
            }
            if e.data.is_empty() {
                self.applied_index = e.index;
                continue;
            }
            let cmd = match NodeCommand::decode(&e.data, &Limits::v1()) {
                Ok(c) => c,
                Err(err) => {
                    self.fail(format!("undecodable log entry {}: {err}", e.index));
                    return;
                }
            };
            if let Err(err) = self.apply_one(e.index, cmd) {
                self.fail(format!("apply of log entry {} failed: {err}", e.index));
                return;
            }
            self.applied_index = e.index;
        }
    }

    fn apply_one(&mut self, index: u64, cmd: NodeCommand) -> CoreResult<()> {
        match cmd {
            NodeCommand::Genesis(m) => {
                self.catalog
                    .apply(index, &LogCommand::Genesis(m), self.manifest_hash)?;
            }
            NodeCommand::Catalog(c) => {
                let id = c.admin_request_id;
                let r = self
                    .catalog
                    .apply(index, &LogCommand::Catalog(c), self.manifest_hash)?;
                if let (Some(commit), Some(pending)) = (r, self.admin_waiters.remove(&id)) {
                    let reply = if commit.command_hash == pending.command_hash {
                        AdminReply::Committed(commit)
                    } else {
                        AdminReply::Refused {
                            code: "IdentityConflict".into(),
                            detail: "admin request id is bound to different command bytes".into(),
                            leader: self.raft.leader(),
                        }
                    };
                    for w in pending.waiters {
                        self.reply(
                            &w,
                            MessageKind::AdminReply,
                            reply.encode(),
                        );
                    }
                }
            }
            NodeCommand::Seed {
                label,
                record,
                rows,
            } => {
                let refs: Vec<(
                    carolina_lang::types::Value,
                    Vec<(&str, carolina_lang::types::Value)>,
                )> = rows
                    .iter()
                    .map(|(k, fs)| {
                        (
                            k.clone(),
                            fs.iter().map(|(n, v)| (n.as_str(), v.clone())).collect(),
                        )
                    })
                    .collect();
                self.engine.load_rows(&label, &record, &refs)?;
                if let Some(ws) = self.seed_waiters.remove(&label) {
                    for w in ws {
                        self.reply(
                            &w,
                            MessageKind::AdminReply,
                            AdminReply::Seeded { log_index: index }.encode(),
                        );
                    }
                }
            }
            NodeCommand::Admit { invoke, admitted_by, catalog_generation } => {
                let key = invoke.content.request_key;
                // Catalog transitions preceding this entry may revoke admission after the
                // leader queued it. Recheck in log order on every voter before any new effect.
                // Previously executed identities only retrieve their original result.
                if !self.last_exec.contains_key(&key)
                    && !self.admission_authorized(&invoke, admitted_by, catalog_generation)
                {
                    if self.invoke_waiters.get(&key).is_some_and(|p| p.request_hash == invoke.request_hash) {
                        self.reply_invoke_waiters(&key, &refusal("AuthorityUnavailable",
                            "route or grant no longer authorizes admission at this log position".into()));
                    }
                    return Ok(());
                }
                // deterministic ordered execution of the admitted request (SPEC-008 §8 steps 5–8)
                let reply = self.engine.invoke(&invoke);
                let terminal = matches!(
                    reply,
                    ClientReplyV1::Committed(_) | ClientReplyV1::Rejected(_)
                );
                if terminal {
                    // A changed-content retry must never erase the receipt needed to verify
                    // the original request's later Decision during live execution or replay.
                    self.last_exec.insert(key, reply.clone());
                }
                if self.raft.is_leader()
                    && ((terminal && self.decided.contains(&key))
                        || (!terminal
                            && self.invoke_waiters.get(&key).is_some_and(|pending| {
                                pending.request_hash == invoke.request_hash
                            })))
                {
                    self.reply_invoke_waiters(&key, &reply);
                }
            }
            NodeCommand::Decision {
                request_key,
                txn_id,
                receipt_digest,
                outcome,
                ..
            } => {
                // every voter verifies its own deterministic result against the unique decision
                let mine = self.engine.resolve(&ResolveRequestV1 {
                    request_key,
                    expected_request_hash: self
                        .last_exec
                        .get(&request_key)
                        .and_then(|r| match r {
                            ClientReplyV1::Committed(x) | ClientReplyV1::Rejected(x) => {
                                Some(x.request_hash)
                            }
                            _ => None,
                        })
                        .unwrap_or_default(),
                });
                let receipt = match mine {
                    ResolveReplyV1::Terminal(b) => match *b {
                        ClientReplyV1::Committed(r) | ClientReplyV1::Rejected(r) => Some(r),
                        _ => None,
                    },
                    _ => None,
                };
                match &receipt {
                    Some(r)
                        if r.txn_id == txn_id
                            && r.receipt_digest().0 == receipt_digest
                            && r.outcome.label() == outcome => {}
                    Some(r) => {
                        return Err(CoreError::new(
                            ErrorCode::Corruption,
                            format!(
                                "divergent execution: my receipt {} vs decided {}",
                                r.receipt_digest(),
                                receipt_digest
                            ),
                        ));
                    }
                    None => {
                        return Err(CoreError::new(
                            ErrorCode::Corruption,
                            "decision for a request this voter never executed",
                        ));
                    }
                }
                self.decided.insert(request_key);
                self.decision_proposals.remove(&request_key);
                let r = receipt.unwrap();
                let reply = match r.outcome {
                    Outcome::Committed => ClientReplyV1::Committed(r),
                    Outcome::Rejected => ClientReplyV1::Rejected(r),
                };
                self.reply_invoke_waiters(&request_key, &reply);
            }
        }
        Ok(())
    }

    fn abandon_waiters(&mut self, key: &RequestKey) {
        if let Some(pending) = self.invoke_waiters.remove(key) {
            let hint = ClientReplyV1::OutcomeUnknown(ResolutionHintV1 {
                request_key: *key,
                request_hash: pending.request_hash,
                txn_id: None,
                resolver: Some(RouteHint {
                    home: self.cfg.home_id(),
                    epoch: RequestHomeEpoch(1),
                }),
            });
            for w in pending.waiters {
                self.reply_client(&w, &hint);
            }
        }
    }

    fn on_role_change(&mut self) {
        let leader_now = self.raft.is_leader();
        if self.was_leader && !leader_now {
            // ReadIndex evidence is authority for one leadership tenure only. Discard both
            // already-ready and apply-delayed barriers before this node can lead again.
            self.ready_reads_backlog.clear();
            self.raft.take_ready_reads();
            // every admitted-but-undecided request is unknown to the client; it must resolve
            let keys: Vec<RequestKey> = self.invoke_waiters.keys().copied().collect();
            for k in keys {
                self.abandon_waiters(&k);
            }
            for (_, pending) in std::mem::take(&mut self.admin_waiters) {
                for w in pending.waiters {
                    self.reply(
                        &w,
                        MessageKind::AdminReply,
                        AdminReply::Refused {
                            code: "LeadershipLost".into(),
                            detail: "resolve by admin request id".into(),
                            leader: self.raft.leader(),
                        }
                        .encode(),
                    );
                }
            }
            for (_, ws) in std::mem::take(&mut self.seed_waiters) {
                for w in ws {
                    self.reply(
                        &w,
                        MessageKind::AdminReply,
                        AdminReply::Refused {
                            code: "LeadershipLost".into(),
                            detail: String::new(),
                            leader: self.raft.leader(),
                        }
                        .encode(),
                    );
                }
            }
            for (_, w, req) in std::mem::take(&mut self.resolve_waiters) {
                self.reply(
                    &w,
                    MessageKind::ResolveReply,
                    ResolveReplyV1::Unavailable(RefusalV1 {
                        code: "LeadershipLost".into(),
                        detail: format!("{:?}", req.request_key),
                        possibly_admitted: true,
                    })
                    .encode(),
                );
            }
        }
        self.was_leader = leader_now;
    }

    // ---- client requests ----------------------------------------------------

    fn not_leader(&self) -> ClientReplyV1 {
        refusal(
            "NotLeader",
            self.raft
                .leader()
                .map(|l| carolina_core::hash::hex_encode(&l.0))
                .unwrap_or_else(|| "unknown".into()),
        )
    }

    fn admission_authorized(&self, inv: &InvokeV1, admitted_by: NodeId, generation: CatalogGeneration) -> bool {
        if generation.0 == 0 || generation > self.catalog.generation()
            || !self.cfg.manifest.voters.contains(&admitted_by)
        {
            return false;
        }
        let key = inv.content.request_key;
        let Some(route) = self.catalog.route(key.tenant_id, key.request_namespace, ROUTE_BUCKET) else {
            return false;
        };
        let Some(grant) = self.catalog.grant(route.home_grant) else {
            return false;
        };
        let Some(plan) = self.engine.catalog.plan_for(inv.content.operation) else {
            return false;
        };
        route.home_id == self.cfg.home_id()
            && route.home_epoch == RequestHomeEpoch(1)
            && grant.binding == AuthorityBinding::RequestHome { home_id: route.home_id, epoch: route.home_epoch }
            && grant.tenant == key.tenant_id
            && grant.plans.contains(&plan.plan_ref())
            && self.catalog.admission_allowed(route.home_grant, admitted_by)
    }

    fn handle_invoke(&mut self, w: Waiter, payload: Vec<u8>) {
        let inv = match InvokeV1::decode(&payload, &Limits::v1()) {
            Ok(i) => i,
            Err(e) => {
                self.reply_client(
                    &w,
                    &ClientReplyV1::ProtocolError {
                        code: "MalformedInvoke".into(),
                        detail: e.to_string(),
                    },
                );
                return;
            }
        };
        if self.failure.is_some() {
            self.reply_client(
                &w,
                &refusal("Failed", self.failure.clone().unwrap_or_default()),
            );
            return;
        }
        if !self.raft.is_leader() {
            let r = self.not_leader();
            self.reply_client(&w, &r);
            return;
        }
        if inv.verify_hash().is_err() {
            self.reply_client(&w, &ClientReplyV1::RequestIdentityMismatch);
            return;
        }
        let key = inv.content.request_key;
        if self.decided.contains(&key) {
            let r = self.engine.invoke(&inv);
            self.reply_client(&w, &r);
            return;
        }
        if !self.ready() {
            self.reply_client(&w, &refusal("NotReady", self.readiness_label()));
            return;
        }
        // SPEC-011 §5 admission: route + active grant + admitted node
        if !self.admission_authorized(&inv, self.me, self.catalog.generation()) {
                self.reply_client(
                    &w,
                    &refusal(
                        "AuthorityUnavailable",
                        "no active route/grant for this namespace at this home".into(),
                    ),
                );
                return;
        }
        if let Some(pending) = self.invoke_waiters.get_mut(&key) {
            if pending.request_hash != inv.request_hash {
                self.reply_client(&w, &ClientReplyV1::RequestIdentityMismatch);
            } else {
                pending.waiters.push(w);
            }
            return;
        }
        self.invoke_waiters.insert(
            key,
            PendingInvoke {
                request_hash: inv.request_hash,
                waiters: vec![w],
            },
        );
        let cmd = NodeCommand::Admit {
            invoke: inv,
            admitted_by: self.me,
            catalog_generation: self.catalog.generation(),
        };
        match self.raft.propose(cmd.encode()) {
            Ok(_) => {}
            Err(_) => {
                let r = self.not_leader();
                self.reply_invoke_waiters(&key, &r);
            }
        }
    }

    fn handle_resolve(&mut self, w: Waiter, payload: Vec<u8>) {
        let req = match ResolveRequestV1::decode(&payload, &Limits::v1()) {
            Ok(r) => r,
            Err(e) => {
                self.reply(
                    &w,
                    MessageKind::ResolveReply,
                    ResolveReplyV1::Unavailable(RefusalV1 {
                        code: "MalformedResolve".into(),
                        detail: e.to_string(),
                        possibly_admitted: false,
                    })
                    .encode(),
                );
                return;
            }
        };
        if !self.raft.is_leader() || self.failure.is_some() {
            let r = self.not_leader();
            let refusal = match r {
                ClientReplyV1::Unavailable(x) => x,
                _ => unreachable!(),
            };
            self.reply(
                &w,
                MessageKind::ResolveReply,
                ResolveReplyV1::Unavailable(refusal).encode(),
            );
            return;
        }
        // a current answer needs an authoritative read barrier (SPEC-011 §5, SPEC-012 §5)
        match self.raft.read_barrier() {
            Ok(round) => self.resolve_waiters.push((round, w, req)),
            Err(e) => {
                self.reply(
                    &w,
                    MessageKind::ResolveReply,
                    ResolveReplyV1::Unavailable(RefusalV1 {
                        code: "NotReady".into(),
                        detail: e.to_string(),
                        possibly_admitted: true,
                    })
                    .encode(),
                );
            }
        }
    }

    fn serve_ready_resolves(&mut self) {
        if !self.raft.is_leader() || self.failure.is_some() {
            return;
        }
        let mut ready = std::mem::take(&mut self.ready_reads_backlog);
        ready.extend(self.raft.take_ready_reads());
        if ready.is_empty() {
            return;
        }
        let max_round = ready.iter().map(|r| r.round).max().unwrap_or(0);
        let max_index = ready.iter().map(|r| r.index).max().unwrap_or(0);
        if self.applied_index < max_index {
            // the barrier index is committed but not yet applied here: serve after applying
            self.ready_reads_backlog = ready;
            return;
        }
        let pending = std::mem::take(&mut self.resolve_waiters);
        for (round, w, req) in pending {
            if round <= max_round {
                let reply = self.resolve_at_barrier(&req);
                self.reply(&w, MessageKind::ResolveReply, reply.encode());
            } else {
                self.resolve_waiters.push((round, w, req));
            }
        }
    }

    fn resolve_at_barrier(&mut self, req: &ResolveRequestV1) -> ResolveReplyV1 {
        let reply = self.engine.resolve(req);
        if !self.decided.contains(&req.request_key)
            && matches!(&reply, ResolveReplyV1::Terminal(r)
                if matches!(r.as_ref(), ClientReplyV1::Committed(_) | ClientReplyV1::Rejected(_)))
        {
            // The local engine persists execution at Admit. Only the replicated Decision
            // publishes that receipt; a read quorum alone cannot substitute for its commit.
            return ResolveReplyV1::Unavailable(RefusalV1 {
                code: "DecisionPending".into(),
                detail: "execution is durable; the replicated decision is not yet applied".into(),
                possibly_admitted: true,
            });
        }
        reply
    }

    fn handle_admin(&mut self, w: Waiter, payload: Vec<u8>) {
        let req = match AdminRequest::decode(&payload, &Limits::v1()) {
            Ok(r) => r,
            Err(e) => {
                self.reply(
                    &w,
                    MessageKind::AdminReply,
                    AdminReply::Refused {
                        code: "MalformedAdmin".into(),
                        detail: e.to_string(),
                        leader: None,
                    }
                    .encode(),
                );
                return;
            }
        };
        match req {
            AdminRequest::Status => {
                let s = self.status();
                self.reply(&w, MessageKind::AdminReply, AdminReply::Status(s).encode());
            }
            AdminRequest::Catalog(cmd) => {
                if !self.raft.is_leader() || self.failure.is_some() {
                    self.reply(
                        &w,
                        MessageKind::AdminReply,
                        AdminReply::Refused {
                            code: "NotLeader".into(),
                            detail: String::new(),
                            leader: self.raft.leader(),
                        }
                        .encode(),
                    );
                    return;
                }
                if let Some(prev) = self.catalog.result_of(&cmd.admin_request_id) {
                    if prev.command_hash == cmd.command_hash() {
                        let p = prev.clone();
                        self.reply(
                            &w,
                            MessageKind::AdminReply,
                            AdminReply::Committed(p).encode(),
                        );
                        return;
                    }
                }
                let id = cmd.admin_request_id;
                let command_hash = cmd.command_hash();
                if let Some(pending) = self.admin_waiters.get_mut(&id) {
                    if pending.command_hash == command_hash {
                        pending.waiters.push(w);
                    } else {
                        self.reply(&w, MessageKind::AdminReply, AdminReply::Refused {
                            code: "IdentityConflict".into(),
                            detail: "admin request id has different command bytes in flight".into(),
                            leader: self.raft.leader(),
                        }.encode());
                    }
                    return;
                }
                match self.raft.propose(NodeCommand::Catalog(cmd).encode()) {
                    Ok(_) => { self.admin_waiters.insert(id, PendingAdmin { command_hash, waiters: vec![w] }); }
                    Err(e) => self.reply(
                        &w,
                        MessageKind::AdminReply,
                        AdminReply::Refused {
                            code: "NotLeader".into(),
                            detail: e.to_string(),
                            leader: self.raft.leader(),
                        }
                        .encode(),
                    ),
                }
            }
            AdminRequest::Seed {
                label,
                record,
                rows,
            } => {
                if !self.raft.is_leader() || self.failure.is_some() {
                    self.reply(
                        &w,
                        MessageKind::AdminReply,
                        AdminReply::Refused {
                            code: "NotLeader".into(),
                            detail: String::new(),
                            leader: self.raft.leader(),
                        }
                        .encode(),
                    );
                    return;
                }
                match self.raft.propose(
                    NodeCommand::Seed {
                        label: label.clone(),
                        record,
                        rows,
                    }
                    .encode(),
                ) {
                    Ok(_) => self.seed_waiters.entry(label).or_default().push(w),
                    Err(e) => self.reply(
                        &w,
                        MessageKind::AdminReply,
                        AdminReply::Refused {
                            code: "NotLeader".into(),
                            detail: e.to_string(),
                            leader: self.raft.leader(),
                        }
                        .encode(),
                    ),
                }
            }
        }
    }
}

/// Run the node until the process is terminated.
pub fn run_node(cfg: NodeConfig) -> CoreResult<()> {
    cfg.validate()?;
    let listener = TcpListener::bind(cfg.listen_addr()?)?;
    let cluster = cfg.manifest.cluster_id;
    let conns = Arc::new(Connections::default());
    let (events_tx, events_rx) = channel::<Event>();
    // peer links
    let mut peer_tx = BTreeMap::new();
    for p in &cfg.peers {
        let id = NodeId::derive(&p.node);
        if id == cfg.node_id() {
            continue;
        }
        let addr: std::net::SocketAddr = p.addr.parse().map_err(|_| {
            CoreError::new(
                ErrorCode::InvalidManifest,
                format!("peer address {}", p.addr),
            )
        })?;
        let (tx, rx) = channel::<Envelope>();
        peer_tx.insert(id, tx);
        let caps = capabilities(cluster, EndpointRole::Node);
        let seed = cfg.seed ^ u64::from_le_bytes(id.0[..8].try_into().unwrap());
        std::thread::spawn(move || peer_link(addr, caps, rx, seed));
    }
    let mut core = NodeCore::open(cfg.clone(), conns.clone(), peer_tx)?;
    core.engine.verify(VerifyMode::Quick)?;
    {
        let caps = capabilities(cluster, EndpointRole::Node);
        let tx = events_tx.clone();
        let seed = cfg.seed;
        std::thread::spawn(move || serve(listener, caps, conns, tx, seed));
    }
    eprintln!(
        "[{}] listening on {} as {}",
        cfg.node,
        cfg.listen,
        carolina_core::hash::hex_encode(&cfg.node_id().0[..4])
    );
    // logical ticks come from a dedicated timer thread so that client/peer traffic never starves
    // elections and heartbeats
    {
        let tx = events_tx.clone();
        let tick = Duration::from_millis(cfg.tick_millis.max(5));
        std::thread::spawn(move || loop {
            std::thread::sleep(tick);
            if tx.send(Event::Tick).is_err() {
                break;
            }
        });
    }
    loop {
        let ev = match events_rx.recv() {
            Ok(ev) => ev,
            Err(_) => Event::Shutdown,
        };
        match ev {
            Event::Tick => {
                core.raft.tick()?;
            }
            Event::Peer(env) => {
                core.raft.step(env)?;
            }
            Event::Client {
                conn,
                kind,
                stream_id,
                payload,
                ..
            } => {
                let w = Waiter { conn, stream_id };
                match kind {
                    MessageKind::Invoke => core.handle_invoke(w, payload),
                    MessageKind::ResolveRequest => core.handle_resolve(w, payload),
                    MessageKind::Admin => core.handle_admin(w, payload),
                    _ => {}
                }
            }
            Event::Disconnected(_) => {}
            Event::Shutdown => return Ok(()),
        }
        core.on_role_change();
        core.send_out();
        core.apply_committed();
        core.send_out();
        core.leader_duties()?;
        core.send_out();
        core.serve_ready_resolves();
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
