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
    let cmd = close_grant(cluster.node(0), "close-id");
    let mut changed = cmd.clone();
    changed.predicates.clear();
    let receiver = cluster.listen(0, 1);
    let before_generation = cluster.node(0).catalog.generation();
    cluster.node(0).handle_admin(
        Waiter {
            conn: 1,
            stream_id: 1,
        },
        AdminRequest::Catalog(cmd.clone()).encode(),
    );
    let proposed_index = cluster.node(0).raft.last_index();
    cluster.node(0).handle_admin(
        Waiter {
            conn: 1,
            stream_id: 2,
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
fn preceding_grant_closure_refuses_queued_admission_and_preserves_final_retries() {
    let mut cluster = Cluster::new();
    let original = cluster.invoke("before-close", 3);
    let late = cluster.invoke("after-close", 4);
    let receiver = cluster.listen(0, 1);
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 1,
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
        },
        inv.encode(),
    );
    let admitted_index = cluster.node(0).raft.last_index();
    cluster.node(0).handle_invoke(
        Waiter {
            conn: 1,
            stream_id: 2,
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
