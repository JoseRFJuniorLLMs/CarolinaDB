use super::*;
use carolina_catalog::BootstrapManifest;
use carolina_consensus::{Message, Role};
use carolina_lang::types::Value;
use carolina_runtime::engine::make_invoke;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

const ITEM: [u8; 16] = [0x33; 16];

struct Cluster {
    nodes: Vec<Option<NodeCore>>,
    root: PathBuf,
}

impl Cluster {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "carolina-core-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let names = ["alpha", "beta", "gamma"];
        let manifest = BootstrapManifest {
            cluster_id: ClusterId::derive("core-test"),
            voters: names.iter().map(|n| NodeId::derive(n)).collect(),
            trust_root_hash: Hash256([7; 32]),
            security_policy: SecurityPolicyId::derive("dev-local-policy"),
            bootstrap_admin: PrincipalId::derive("bootstrap-admin"),
            security_profile: "DEV_LOCAL".into(),
        };
        let nodes = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                Some(
                    NodeCore::open(
                        NodeConfig {
                            node: (*name).into(),
                            listen: "127.0.0.1:0".into(),
                            peers: vec![],
                            data_dir: root.join(name).to_string_lossy().into_owned(),
                            manifest: manifest.clone(),
                            module: "fixture:inventory_reserve_release".into(),
                            seed: i as u64 + 10,
                            home: "core-test-home".into(),
                            tick_millis: 5,
                        },
                        Arc::new(Connections::default()),
                        BTreeMap::new(),
                    )
                    .unwrap(),
                )
            })
            .collect();
        let mut cluster = Self { nodes, root };
        cluster.elect(0);
        cluster.pump(60, true);
        assert!(cluster.nodes.iter().flatten().all(NodeCore::ready));
        cluster
            .node(0)
            .raft
            .propose(
                NodeCommand::Seed {
                    label: "seed".into(),
                    record: "Item".into(),
                    rows: vec![(
                        Value::Uuid(ITEM),
                        vec![
                            ("id".into(), Value::Uuid(ITEM)),
                            ("available".into(), Value::I64(10)),
                            ("reserved".into(), Value::I64(0)),
                            ("total".into(), Value::I64(10)),
                        ],
                    )],
                }
                .encode(),
            )
            .unwrap();
        cluster.pump(20, true);
        cluster
    }

    fn node(&mut self, index: usize) -> &mut NodeCore {
        self.nodes[index].as_mut().unwrap()
    }

    fn elect(&mut self, index: usize) {
        for _ in 0..30 {
            self.node(index).raft.tick().unwrap();
            if self.node(index).raft.role() == Role::Candidate {
                return;
            }
        }
        panic!("node did not start an election");
    }

    fn pump(&mut self, rounds: usize, duties: bool) {
        for _ in 0..rounds {
            let mut messages = Vec::new();
            for node in self.nodes.iter_mut().flatten() {
                if node.raft.is_leader() {
                    node.raft.tick().unwrap();
                }
                messages.extend(node.raft.take_outbox());
            }
            for message in messages {
                if let Some(node) = self.nodes.iter_mut().flatten().find(|n| n.me == message.to) {
                    node.raft.step(message).unwrap();
                    node.on_role_change();
                    node.apply_committed();
                }
            }
            for node in self.nodes.iter_mut().flatten() {
                if duties {
                    node.leader_duties().unwrap();
                }
                node.serve_ready_resolves();
                assert!(node.failure.is_none(), "{:?}", node.failure);
            }
        }
    }

    fn invoke(&mut self, key: &str, quantity: i64) -> InvokeV1 {
        make_invoke(
            &self.node(0).engine.catalog,
            "tenant-c5",
            key,
            "reserve",
            vec![
                Value::Uuid(ITEM),
                Value::I64(quantity),
                Value::Uuid([1; 16]),
            ],
        )
        .unwrap()
    }

    fn listen(&self, index: usize, conn: u64) -> Receiver<Outbound> {
        self.nodes[index]
            .as_ref()
            .unwrap()
            .conns
            .test_receiver(conn)
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        self.nodes.clear();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The principal the pinned manifest trusts to administer the cluster (SPEC-013 §4).
fn bootstrap_admin(cluster: &Cluster) -> PrincipalId {
    cluster.nodes[0]
        .as_ref()
        .unwrap()
        .cfg
        .manifest
        .bootstrap_admin
}

fn client_reply(receiver: &Receiver<Outbound>) -> (u64, ClientReplyV1) {
    let outbound = receiver.try_recv().expect("expected a client reply");
    assert_eq!(outbound.kind, MessageKind::Reply);
    (
        outbound.stream_id,
        ClientReplyV1::decode(&outbound.payload, &Limits::v1()).unwrap(),
    )
}

fn close_grant(node: &NodeCore, request: &str) -> CatalogCommand {
    CatalogCommand {
        admin_request_id: AdminRequestId::derive(request),
        principal: node.cfg.manifest.bootstrap_admin,
        expected: vec![],
        predicates: vec![Predicate::GrantInState {
            grant: node.grant_id,
            state: GrantState::Active,
        }],
        mutations: vec![Mutation::AdvanceGrantState {
            grant: node.grant_id,
            from: GrantState::Active,
            to: GrantState::Closing,
        }],
    }
}

fn admin_reply(receiver: &Receiver<Outbound>) -> (u64, AdminReply) {
    let outbound = receiver.try_recv().expect("expected an admin reply");
    assert_eq!(outbound.kind, MessageKind::AdminReply);
    (
        outbound.stream_id,
        AdminReply::decode(&outbound.payload, &Limits::v1()).unwrap(),
    )
}

#[test]
fn concurrent_admin_retries_match_command_bytes_before_sharing_the_result() {
    let mut cluster = Cluster::new();
    let admin_principal = bootstrap_admin(&cluster);
    let cmd = close_grant(cluster.node(0), "close-id");
    let mut changed = cmd.clone();
    changed.predicates.clear();
    let receiver = cluster.listen(0, 1);
    let before_generation = cluster.node(0).catalog.generation();
    cluster.node(0).handle_admin(
        Waiter {
            conn: 1,
            stream_id: 1,
            principal: admin_principal,
        },
        AdminRequest::Catalog(cmd.clone()).encode(),
    );
    let proposed_index = cluster.node(0).raft.last_index();
    cluster.node(0).handle_admin(
        Waiter {
            conn: 1,
            stream_id: 2,
            principal: admin_principal,
        },
        AdminRequest::Catalog(changed).encode(),
    );
    assert!(
        matches!(admin_reply(&receiver), (2, AdminReply::Refused { code, .. }) if code == "IdentityConflict")
    );
    cluster.node(0).handle_admin(
        Waiter {
            conn: 1,
            stream_id: 3,
            principal: admin_principal,
        },
        AdminRequest::Catalog(cmd.clone()).encode(),
    );
    assert_eq!(cluster.node(0).raft.last_index(), proposed_index);
    cluster.pump(20, true);
    let first = admin_reply(&receiver);
    let retry = admin_reply(&receiver);
    assert_eq!((first.0, retry.0), (1, 3));
    assert_eq!(first.1.encode(), retry.1.encode());
    assert!(
        matches!(first.1, AdminReply::Committed(commit) if commit.failure.is_none() && commit.command_hash == cmd.command_hash())
    );
    assert_eq!(
        cluster.node(0).catalog.generation().0,
        before_generation.0 + 1
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn seed_retries_share_only_identical_content_and_conflicts_do_not_fail_the_node() {
    let mut cluster = Cluster::new();
    let admin_principal = bootstrap_admin(&cluster);
    let receiver = cluster.listen(0, 1);
    let seed = AdminRequest::Seed {
        label: "seed-identity".into(),
        record: "Item".into(),
        rows: vec![(
            Value::Uuid([0x44; 16]),
            vec![
                ("id".into(), Value::Uuid([0x44; 16])),
                ("available".into(), Value::I64(7)),
                ("reserved".into(), Value::I64(0)),
                ("total".into(), Value::I64(7)),
            ],
        )],
    };
    let mut changed = seed.clone();
    if let AdminRequest::Seed { rows, .. } = &mut changed {
        rows[0].1[1].1 = Value::I64(8);
        rows[0].1[3].1 = Value::I64(8);
    }

    cluster.node(0).handle_admin(
        Waiter {
            conn: 1,
            stream_id: 1,
            principal: admin_principal,
        },
        seed.encode(),
    );
    let proposed_index = cluster.node(0).raft.last_index();
    cluster.node(0).handle_admin(
        Waiter {
            conn: 1,
            stream_id: 2,
            principal: admin_principal,
        },
        changed.encode(),
    );
    assert!(
        matches!(admin_reply(&receiver), (2, AdminReply::Refused { code, .. }) if code == "IdentityConflict")
    );
    cluster.node(0).handle_admin(
        Waiter {
            conn: 1,
            stream_id: 3,
            principal: admin_principal,
        },
        seed.encode(),
    );
    assert_eq!(cluster.node(0).raft.last_index(), proposed_index);
    cluster.pump(20, true);
    assert!(matches!(
        admin_reply(&receiver),
        (1, AdminReply::Seeded { .. })
    ));
    assert!(matches!(
        admin_reply(&receiver),
        (3, AdminReply::Seeded { .. })
    ));

    cluster.node(0).handle_admin(
        Waiter {
            conn: 1,
            stream_id: 4,
            principal: admin_principal,
        },
        changed.encode(),
    );
    cluster.pump(20, true);
    assert!(
        matches!(admin_reply(&receiver), (4, AdminReply::Refused { code, .. }) if code == "IdentityConflict")
    );
    for node in cluster.nodes.iter_mut().flatten() {
        assert!(node.failure.is_none(), "{:?}", node.failure);
        let row = node
            .engine
            .read_row("Item", &Value::Uuid([0x44; 16]))
            .unwrap()
            .unwrap();
        assert_eq!(row["available"], Value::I64(7));
        assert_eq!(row["total"], Value::I64(7));
    }
}

#[test]
fn preceding_grant_closure_refuses_queued_admission_and_preserves_final_retries() {
    let mut cluster = Cluster::new();
    let original = cluster.invoke("before-close", 3);
    let late = cluster.invoke("after-close", 4);
    let receiver = cluster.listen(0, 1);
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 1,
            principal: PrincipalId::derive("dev-local-client"),
        },
        original.encode(),
    );
    cluster.pump(20, true);
    let original_reply = client_reply(&receiver).1;
    assert!(matches!(original_reply, ClientReplyV1::Committed(_)));
    let close = close_grant(cluster.node(0), "close-before-admit");
    cluster
        .node(0)
        .raft
        .propose(NodeCommand::Catalog(close).encode())
        .unwrap();
    // The leader still sees ACTIVE when it queues this Invoke behind the pending closure.
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 2,
            principal: PrincipalId::derive("dev-local-client"),
        },
        late.encode(),
    );
    assert!(cluster
        .node(0)
        .invoke_waiters
        .contains_key(&late.content.request_key));
    cluster.pump(20, true);
    assert!(
        matches!(client_reply(&receiver), (2, ClientReplyV1::Unavailable(r)) if r.code == "AuthorityUnavailable" && !r.possibly_admitted)
    );
    for node in cluster.nodes.iter_mut().flatten() {
        assert!(!node.last_exec.contains_key(&late.content.request_key));
        assert!(matches!(
            node.engine.resolve(&ResolveRequestV1 {
                request_key: late.content.request_key,
                expected_request_hash: late.request_hash,
            }),
            ResolveReplyV1::AbsentAtBarrier { .. }
        ));
        assert_eq!(node.engine.count_rows("Reservation").unwrap(), 1);
    }
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 3,
            principal: PrincipalId::derive("dev-local-client"),
        },
        original.encode(),
    );
    assert_eq!(client_reply(&receiver).1.encode(), original_reply.encode());
    // Replay evaluates historical authority at each Admit, even though the final grant is closed.
    let cfg = cluster.node(1).cfg.clone();
    cluster.nodes[1] = None;
    cluster.nodes[1] =
        Some(NodeCore::open(cfg, Arc::new(Connections::default()), BTreeMap::new()).unwrap());
    cluster.pump(30, true);
    assert!(cluster
        .node(1)
        .decided
        .contains(&original.content.request_key));
    assert_eq!(
        cluster.node(1).last_exec[&original.content.request_key].encode(),
        original_reply.encode()
    );
}

#[test]
fn log_admission_checks_node_and_generation_but_allows_unrelated_catalog_progress() {
    let mut cluster = Cluster::new();
    let me = cluster.node(0).me;
    let generation = cluster.node(0).catalog.generation();
    let invalid = [
        ("unknown-admitter", NodeId::derive("outsider"), generation),
        (
            "future-generation",
            me,
            CatalogGeneration(generation.0 + 100),
        ),
        ("zero-generation", me, CatalogGeneration(0)),
    ];
    let mut refused = Vec::new();
    for (key, admitted_by, catalog_generation) in invalid {
        let inv = cluster.invoke(key, 1);
        refused.push(inv.clone());
        cluster
            .node(0)
            .raft
            .propose(
                NodeCommand::Admit {
                    invoke: inv,
                    principal: PrincipalId::derive("dev-local-client"),
                    admitted_by,
                    catalog_generation,
                }
                .encode(),
            )
            .unwrap();
    }
    let advance = CatalogCommand {
        admin_request_id: AdminRequestId::derive("unrelated-progress"),
        principal: cluster.node(0).cfg.manifest.bootstrap_admin,
        expected: vec![],
        predicates: vec![],
        mutations: vec![],
    };
    cluster
        .node(0)
        .raft
        .propose(NodeCommand::Catalog(advance).encode())
        .unwrap();
    let valid = cluster.invoke("older-valid-generation", 3);
    cluster
        .node(0)
        .raft
        .propose(
            NodeCommand::Admit {
                invoke: valid.clone(),
                principal: PrincipalId::derive("dev-local-client"),
                admitted_by: me,
                catalog_generation: generation,
            }
            .encode(),
        )
        .unwrap();
    cluster.pump(30, true);
    for node in cluster.nodes.iter_mut().flatten() {
        assert!(node.catalog.generation() > generation);
        for inv in &refused {
            assert!(matches!(
                node.engine.resolve(&ResolveRequestV1 {
                    request_key: inv.content.request_key,
                    expected_request_hash: inv.request_hash,
                }),
                ResolveReplyV1::AbsentAtBarrier { .. }
            ));
        }
        assert!(node.decided.contains(&valid.content.request_key));
        assert_eq!(node.engine.count_rows("Reservation").unwrap(), 1);
    }
}

#[test]
fn concurrent_changed_content_is_refused_and_exact_retries_share_one_decision() {
    let mut cluster = Cluster::new();
    let inv = cluster.invoke("concurrent", 3);
    let changed = cluster.invoke("concurrent", 4);
    let receiver = cluster.listen(0, 1);
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 1,
            principal: PrincipalId::derive("dev-local-client"),
        },
        inv.encode(),
    );
    let admitted_index = cluster.node(0).raft.last_index();
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 2,
            principal: PrincipalId::derive("dev-local-client"),
        },
        changed.encode(),
    );
    assert!(matches!(
        client_reply(&receiver),
        (2, ClientReplyV1::RequestIdentityMismatch)
    ));
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 3,
            principal: PrincipalId::derive("dev-local-client"),
        },
        inv.encode(),
    );
    assert_eq!(cluster.node(0).raft.last_index(), admitted_index);
    cluster.pump(30, true);
    let first = client_reply(&receiver);
    let retry = client_reply(&receiver);
    assert_eq!((first.0, retry.0), (1, 3));
    assert!(matches!(first.1, ClientReplyV1::Committed(_)));
    assert_eq!(first.1.encode(), retry.1.encode());
    assert!(receiver.try_recv().is_err());
}

#[test]
fn leadership_loss_retains_the_request_hash_in_unknown_reply() {
    let mut cluster = Cluster::new();
    let inv = cluster.invoke("lost-leader", 3);
    let receiver = cluster.listen(0, 1);
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 1,
            principal: PrincipalId::derive("dev-local-client"),
        },
        inv.encode(),
    );
    let from = cluster.node(1).me;
    let leader = cluster.node(0);
    leader.ready_reads_backlog.push(ReadIndex {
        round: 100,
        index: leader.applied_index + 1,
    });
    leader
        .raft
        .step(Envelope {
            from,
            to: leader.me,
            msg: Message::RequestVote {
                term: leader.raft.term() + 1,
                last_log_index: 0,
                last_log_term: 0,
            },
        })
        .unwrap();
    leader.on_role_change();
    assert!(
        leader.ready_reads_backlog.is_empty(),
        "old tenure's read barrier survived step-down"
    );
    match client_reply(&receiver).1 {
        ClientReplyV1::OutcomeUnknown(hint) => {
            assert_eq!(hint.request_hash, inv.request_hash);
            assert_eq!(hint.request_key, inv.content.request_key);
        }
        other => panic!("expected unknown after leadership loss: {other:?}"),
    }
}

#[test]
fn restarted_successor_finishes_inherited_admission_before_resolve_publishes() {
    let mut cluster = Cluster::new();
    let inv = cluster.invoke("inherited", 3);
    let changed = cluster.invoke("inherited", 4);
    let key = inv.content.request_key;
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 1,
            principal: PrincipalId::derive("dev-local-client"),
        },
        inv.encode(),
    );
    // Model a changed-content admission from another attempt already in the inherited log.
    let leader = cluster.node(0);
    leader
        .raft
        .propose(
            NodeCommand::Admit {
                invoke: changed,
                principal: PrincipalId::derive("dev-local-client"),
                admitted_by: leader.me,
                catalog_generation: leader.catalog.generation(),
            }
            .encode(),
        )
        .unwrap();
    cluster.pump(20, false); // commit and apply Admit, withhold the Decision proposal
    let expected = cluster.node(0).last_exec[&key].encode();
    assert!(!cluster.node(0).decided.contains(&key));
    cluster.nodes[0] = None; // original leader is lost before Decision
    let cfg = cluster.node(1).cfg.clone();
    cluster.nodes[1] = None;
    cluster.nodes[1] =
        Some(NodeCore::open(cfg, Arc::new(Connections::default()), BTreeMap::new()).unwrap());
    cluster.elect(1);
    cluster.pump(30, false); // replay the inherited log under the new committed term
    assert!(cluster.node(1).raft.is_leader());
    assert_eq!(cluster.node(1).last_exec[&key].encode(), expected);
    let req = ResolveRequestV1 {
        request_key: key,
        expected_request_hash: inv.request_hash,
    };
    let receiver = cluster.listen(1, 1);
    cluster.node(1).handle_resolve(
        Waiter {
            conn: 1,
            stream_id: 1,
            principal: PrincipalId::derive("dev-local-client"),
        },
        req.encode(),
    );
    cluster.pump(10, false); // actual ReadIndex quorum, still no Decision
    let outbound = receiver.try_recv().unwrap();
    assert_eq!(outbound.kind, MessageKind::ResolveReply);
    assert!(
        matches!(ResolveReplyV1::decode(&outbound.payload, &Limits::v1()).unwrap(),
        ResolveReplyV1::Unavailable(RefusalV1 { code, possibly_admitted: true, .. }) if code == "DecisionPending")
    );
    cluster.pump(30, true); // no client retry: successor must finish the Decision itself
    assert!(cluster.node(1).decided.contains(&key));
    cluster.node(1).handle_resolve(
        Waiter {
            conn: 1,
            stream_id: 2,
            principal: PrincipalId::derive("dev-local-client"),
        },
        req.encode(),
    );
    cluster.pump(10, true);
    let outbound = receiver.try_recv().unwrap();
    match ResolveReplyV1::decode(&outbound.payload, &Limits::v1()).unwrap() {
        ResolveReplyV1::Terminal(reply) => assert_eq!(reply.encode(), expected),
        other => panic!("expected committed receipt: {other:?}"),
    }
    for node in cluster.nodes.iter_mut().flatten() {
        assert_eq!(node.engine.count_rows("Reservation").unwrap(), 1);
        let row = node
            .engine
            .read_row("Item", &Value::Uuid(ITEM))
            .unwrap()
            .unwrap();
        assert_eq!(row["available"], Value::I64(7));
    }
}

/// SPEC-011 §9: a voter folds its control-plane state into a durable image, reports that frontier,
/// and the leader then drops the log prefix every voter has applied. The image is what makes a
/// restart correct without the prefix: the catalog generation, the grants and the admin results
/// come back from the file, and the request receipts still resolve from the engine.
#[test]
fn applied_state_is_snapshotted_and_the_leader_compacts_the_prefix() {
    let mut cluster = Cluster::new();
    let data_dir = cluster.node(0).snapshot_dir.clone();

    // enough traffic to cross the snapshot interval on every voter
    for i in 0..6u8 {
        // a distinct reservation id per request: repeating one is a business rejection, which
        // would still be a decision but would not exercise the committed path
        let inv = make_invoke(
            &cluster.node(0).engine.catalog,
            "tenant-c5",
            &format!("compaction-{i}"),
            "reserve",
            vec![
                Value::Uuid(ITEM),
                Value::I64(1),
                Value::Uuid([0x40 + i; 16]),
            ],
        )
        .unwrap();
        let receiver = cluster.listen(0, 100 + i as u64);
        cluster.node(0).handle_invoke(
            Waiter {
                conn: 100 + i as u64,
                stream_id: 1,
                principal: PrincipalId::derive("dev-local-client"),
            },
            inv.encode(),
        );
        cluster.pump(40, true);
        let (_, reply) = client_reply(&receiver);
        assert!(
            matches!(reply, ClientReplyV1::Committed(_)),
            "request {i} must commit: {reply:?}"
        );
    }
    cluster.pump(40, true);

    let snapshot_path = data_dir.join("node.snapshot");
    assert!(
        snapshot_path.exists(),
        "the applied state must be folded into a durable image"
    );
    let leader_generation = cluster.node(0).catalog.generation();
    let base = cluster.node(0).raft.snapshot_index();
    assert!(
        base > 0,
        "the leader must drop the prefix every voter has applied"
    );
    assert_eq!(cluster.node(0).raft.first_index(), base + 1);
    assert!(cluster.node(0).raft.entry(base).is_none());
    assert!(
        cluster.node(0).raft.compaction_horizon() >= base,
        "the horizon is bounded by the slowest voter, never by the leader alone"
    );
    for i in 0..3 {
        assert!(cluster.node(i).failure.is_none());
    }

    // a restart recovers the control plane from the image instead of the prefix
    let cfg = cluster.node(0).cfg.clone();
    cluster.nodes[0] = None;
    let reopened = NodeCore::open(cfg, Arc::new(Connections::default()), BTreeMap::new()).unwrap();
    assert_eq!(
        reopened.catalog.generation(),
        leader_generation,
        "the catalog must come back at the generation the image recorded"
    );
    assert!(
        reopened.catalog.genesis().is_some(),
        "genesis survives without the log prefix"
    );
    assert_eq!(reopened.raft.snapshot_index(), base);
    assert_eq!(reopened.raft.first_index(), base + 1);
    assert!(
        reopened.applied_index >= base,
        "the restarted voter resumes at the image, not at index 1"
    );
    cluster.nodes[0] = Some(reopened);
}

/// SPEC-008 §19 / SPEC-001 §63: a node publishes what it counts, and the counters distinguish the
/// cases an operator has to tell apart — committed work, refused authorization, the compacted log
/// base and the applied frontier. An unauthorized principal must move the denial counter and
/// nothing else.
#[test]
fn node_status_publishes_operational_counters() {
    let mut cluster = Cluster::new();
    let inv = cluster.invoke("metrics-1", 1);
    let receiver = cluster.listen(0, 70);
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 70,
            stream_id: 1,
            principal: PrincipalId::derive("dev-local-client"),
        },
        inv.encode(),
    );
    cluster.pump(40, true);
    assert!(matches!(
        client_reply(&receiver).1,
        ClientReplyV1::Committed(_)
    ));

    let before = cluster.node(0).status().metrics;
    assert!(before["engine.committed"] >= 1, "{before:?}");
    assert_eq!(before["engine.identity_mismatch"], 0);
    assert_eq!(before["node.authorization_denied"], 0);
    assert_eq!(before["consensus.is_leader"], 1);
    assert!(before["consensus.applied_index"] >= before["consensus.snapshot_index"]);
    assert!(before["storage.commit_total"] >= 1);
    assert_eq!(before["catalog.genesis"], 1);
    assert_eq!(before["catalog.grant_active"], 1);

    // An unknown principal is refused, and only the denial counter moves.
    let stranger = cluster.invoke("metrics-2", 1);
    let receiver = cluster.listen(0, 71);
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 71,
            stream_id: 1,
            principal: PrincipalId::derive("nobody"),
        },
        stranger.encode(),
    );
    cluster.pump(20, true);
    let (_, reply) = client_reply(&receiver);
    assert!(
        matches!(&reply, ClientReplyV1::Unavailable(r) if r.code == "AuthorizationDenied"),
        "{reply:?}"
    );
    let after = cluster.node(0).status().metrics;
    assert_eq!(after["node.authorization_denied"], 1);
    assert_eq!(
        after["engine.invocations"], before["engine.invocations"],
        "a denied request must never reach the engine"
    );

    // A retained result is not an authorization bypass: knowing the exact committed request
    // bytes must not disclose its receipt to a different principal through Invoke.
    let receiver = cluster.listen(0, 72);
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 72,
            stream_id: 1,
            principal: PrincipalId::derive("nobody"),
        },
        inv.encode(),
    );
    let (_, replay) = client_reply(&receiver);
    assert!(
        matches!(&replay, ClientReplyV1::Unavailable(r) if r.code == "AuthorizationDenied"),
        "{replay:?}"
    );
    let after_replay = cluster.node(0).status().metrics;
    assert_eq!(after_replay["node.authorization_denied"], 2);
    assert_eq!(
        after_replay["engine.invocations"], before["engine.invocations"],
        "a denied retained-result replay must never reach the engine"
    );

    // the status survives the codec the admin reply uses
    let status = cluster.node(0).status();
    let bytes = AdminReply::Status(status.clone()).encode();
    match AdminReply::decode(&bytes, &Limits::v1()).unwrap() {
        AdminReply::Status(back) => assert_eq!(back.metrics, status.metrics),
        other => panic!("expected a status reply: {other:?}"),
    }
}

/// SPEC-008 §5 / SPEC-011 §7: a cold restart of the **whole** cluster must not bootstrap twice.
///
/// A voter that restarts before its first snapshot rebuilds the catalog by replaying the log, so
/// it starts with no genesis. If the leader elected out of that restart asks the catalog what to
/// bootstrap before it has applied its own log, it sees an empty catalog and proposes a second
/// genesis; every voter then refuses that entry and fails closed — permanently, because the
/// duplicate is durable and every later restart replays it. The process campaign restarts one
/// voter at a time, which a live majority carries and catches up as a follower, so it cannot
/// reach this case.
///
/// The test build snapshots every four entries, so the pre-snapshot window is reached by removing
/// the image; the assertion that nothing was compacted first is what makes that equivalent.
#[test]
fn a_cold_restart_of_every_voter_does_not_bootstrap_twice() {
    let mut cluster = Cluster::new();
    let generation = cluster.node(0).catalog.generation();
    let log_end = cluster.node(0).raft.last_index();

    let cfgs: Vec<NodeConfig> = (0..3).map(|i| cluster.node(i).cfg.clone()).collect();
    for i in 0..3 {
        assert_eq!(
            cluster.node(i).raft.snapshot_index(),
            0,
            "the whole log must still be present for the image to be removable"
        );
        let image = cluster.node(i).snapshot_dir.join("node.snapshot");
        cluster.nodes[i] = None;
        let _ = std::fs::remove_file(&image);
    }

    for (i, cfg) in cfgs.into_iter().enumerate() {
        let node = NodeCore::open(cfg, Arc::new(Connections::default()), BTreeMap::new()).unwrap();
        assert!(
            node.catalog.genesis().is_none(),
            "a voter with no image starts with an empty catalog; that is what makes an \
             un-caught-up leader dangerous"
        );
        assert_eq!(node.applied_index, 0);
        cluster.nodes[i] = Some(node);
    }

    // pump() asserts on every round that no node has failed closed
    cluster.elect(1);
    cluster.pump(80, true);

    for i in 0..3 {
        assert!(cluster.node(i).failure.is_none());
        assert!(
            cluster.node(i).catalog.genesis().is_some(),
            "voter {i} must replay the genesis it already had"
        );
        assert_eq!(
            cluster.node(i).catalog.generation(),
            generation,
            "voter {i}: bootstrap must not run a second time"
        );
    }
    assert_eq!(
        cluster.node(1).raft.last_index(),
        log_end + 1,
        "the only new entry is the term entry the election appended"
    );
    assert!(cluster.nodes.iter().flatten().all(NodeCore::ready));
}
