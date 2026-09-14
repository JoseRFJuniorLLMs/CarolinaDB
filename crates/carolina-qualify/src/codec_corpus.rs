//! Codec golden corpus (SPEC-012 §12, SPEC-014 §5): frozen canonical bytes and digests for every
//! record kind an enabled feature exchanges or persists, plus negative vectors.
//!
//! `fixtures/codec/<name>.astj` holds the canonical bytes of a deterministic sample instance;
//! `fixtures/codec/manifest.txt` pins `name=<sha256 of the bytes>`. The check decodes each frozen
//! file with the typed decoder, re-encodes it and requires byte identity (decode/encode fixed
//! point), compares digests, and requires every negative vector (unknown field, truncation,
//! type confusion) to be refused with a typed error. Kinds that exist only in the registry
//! without an implemented codec are reported, never counted as frozen.
//!
//! Re-freeze deliberately with `UPDATE_GOLDEN=1 cargo test -p carolina-qualify codec`; a changed
//! digest is a compatibility decision, not a test fix.

use std::collections::BTreeMap;
use std::path::Path;

use carolina_catalog::{
    AuthorityGrant, BootstrapManifest, CatalogCommand, CatalogCommit, CatalogKey, Expected,
    GrantState, Mutation, Predicate, RequestRoute,
};
use carolina_consensus::{Entry, Envelope, Message};
use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::CoreResult;
use carolina_core::hash::{sha256, Hash256};
use carolina_core::ids::*;
use carolina_core::limits::Limits;
use carolina_lang::types::Value;
use carolina_node::protocol::{AdminReply, AdminRequest, NodeCommand, NodeStatus};
use carolina_runtime::engine::{LocalDecisionV1, LocalGrantV1, NamespaceRetirementV1};
use carolina_runtime::home::RequestHomeStateV1;
use carolina_storage::batch::*;
use carolina_storage::testutil::{batch, decision_ref, prepare_batch, receipt, request_key, txn};
use carolina_wire::negotiation::{EndpointRole, LocalCapabilities};
use carolina_wire::records::*;
use carolina_wire::registry::REGISTERED_KINDS;
use carolina_wire::snapshot::{CodecManifest, SnapshotChunkV1, SnapshotKind, SnapshotManifestV1};

pub struct CodecVector {
    pub name: String,
    /// Registered record kind this vector freezes (None for log/admin payloads).
    pub registered_kind: Option<&'static str>,
    pub bytes: Vec<u8>,
    /// Typed decode + re-encode; must reproduce `bytes` exactly.
    pub roundtrip: fn(&[u8]) -> CoreResult<Vec<u8>>,
}

fn rt<T: Canonical>(bytes: &[u8]) -> CoreResult<Vec<u8>> {
    T::decode(bytes, &Limits::v1()).map(|v| v.encode())
}

fn vec_of<T: Canonical>(name: &str, kind: Option<&'static str>, v: &T) -> CodecVector {
    CodecVector {
        name: name.into(),
        registered_kind: kind,
        bytes: v.encode(),
        roundtrip: rt::<T>,
    }
}

fn uuid(b: u8) -> Value {
    Value::Uuid([b; 16])
}

/// Deterministic sample instances of every implemented codec.
pub fn vectors() -> Vec<CodecVector> {
    let mut out = Vec::new();
    let limits = Limits::v1();
    let cluster = ClusterId::derive("codec-cluster");
    // ---- SPEC-012 client records
    let cat = carolina_runtime::LocalCatalog::from_source(
        carolina_lang::fixtures::fixture_source("inventory_reserve_release").unwrap(),
    )
    .unwrap();
    let inv = carolina_runtime::engine::make_invoke(
        &cat,
        "tenant-codec",
        "req-1",
        "reserve",
        vec![uuid(1), Value::I64(2), uuid(9)],
    )
    .unwrap();
    out.push(vec_of(
        "request_content",
        Some("request_content"),
        &inv.content,
    ));
    out.push(vec_of("invoke", Some("invoke"), &inv));
    let rc = receipt(1, Outcome::Committed, b"{\"ok\":\"1\"}");
    let rj = receipt(
        2,
        Outcome::Rejected,
        b"{\"code\":\"PostconditionRejected\"}",
    );
    out.push(vec_of("final_receipt", Some("final_receipt"), &rc));
    out.push(vec_of(
        "accepted_result",
        Some("accepted_result"),
        &rc.accepted_result(),
    ));
    let tomb = ResultTombstoneV1 {
        request_key: rc.request_key,
        request_hash: rc.request_hash,
        txn_id: rc.txn_id,
        outcome: rc.outcome,
        receipt_digest: rc.receipt_digest(),
        decision_ref: rc.decision_ref.clone(),
    };
    out.push(vec_of("result_tombstone", Some("result_tombstone"), &tomb));
    let hint = ResolutionHintV1 {
        request_key: rc.request_key,
        request_hash: rc.request_hash,
        txn_id: Some(rc.txn_id),
        resolver: Some(RouteHint {
            home: RequestHomeId::derive("home"),
            epoch: RequestHomeEpoch(1),
        }),
    };
    let replies: Vec<(&str, ClientReplyV1)> = vec![
        (
            "client_reply.committed",
            ClientReplyV1::Committed(rc.clone()),
        ),
        ("client_reply.rejected", ClientReplyV1::Rejected(rj.clone())),
        (
            "client_reply.unavailable",
            ClientReplyV1::Unavailable(RefusalV1 {
                code: "NotLeader".into(),
                detail: "leader unknown".into(),
                possibly_admitted: false,
            }),
        ),
        (
            "client_reply.outcome_unknown",
            ClientReplyV1::OutcomeUnknown(hint.clone()),
        ),
        (
            "client_reply.identity_mismatch",
            ClientReplyV1::RequestIdentityMismatch,
        ),
        (
            "client_reply.result_expired",
            ClientReplyV1::ResultExpired(tomb.clone()),
        ),
        (
            "client_reply.identity_expired",
            ClientReplyV1::IdentityExpired {
                namespace: RequestNamespace::derive("inventory"),
                retirement_ref: rc.decision_ref.clone(),
            },
        ),
        (
            "client_reply.protocol_error",
            ClientReplyV1::ProtocolError {
                code: "MalformedInvoke".into(),
                detail: "frame".into(),
            },
        ),
    ];
    for (n, r) in &replies {
        out.push(vec_of(n, Some("client_reply"), r));
    }
    let resolve_req = ResolveRequestV1 {
        request_key: rc.request_key,
        expected_request_hash: rc.request_hash,
    };
    out.push(vec_of(
        "resolve_request",
        Some("resolve_request"),
        &resolve_req,
    ));
    let resolve_replies: Vec<(&str, ResolveReplyV1)> = vec![
        (
            "resolve_reply.terminal",
            ResolveReplyV1::Terminal(Box::new(ClientReplyV1::Committed(rc.clone()))),
        ),
        (
            "resolve_reply.pending",
            ResolveReplyV1::Pending {
                txn_id: rc.txn_id,
                phase: "ADMITTED".into(),
                original_plan: rc.plan,
                resolver_ref: rc.decision_ref.clone(),
            },
        ),
        (
            "resolve_reply.absent",
            ResolveReplyV1::AbsentAtBarrier {
                catalog_generation: CatalogGeneration(3),
                home: RequestHomeId::derive("home"),
                home_epoch: RequestHomeEpoch(1),
                record_revision: RecordRevision(0),
            },
        ),
        (
            "resolve_reply.unavailable",
            ResolveReplyV1::Unavailable(RefusalV1 {
                code: "NotReady".into(),
                detail: String::new(),
                possibly_admitted: true,
            }),
        ),
    ];
    for (n, r) in &resolve_replies {
        out.push(vec_of(n, Some("resolve_reply"), r));
    }
    let binding = RequestBindingV1 {
        request_key: rc.request_key,
        request_hash: rc.request_hash,
        txn_id: rc.txn_id,
        allocation_home: rc.txn_id.home(),
        allocation_epoch: rc.txn_id.epoch(),
        allocation_seq: rc.txn_id.seq(),
        record_revision: RecordRevision(2),
        state: BindingState::Terminal,
        admitted_plan: Some(rc.plan),
        decision_authority: Some(rc.decision_ref.clone()),
        terminal_receipt: Some(rc.clone()),
        tombstone: None,
    };
    out.push(vec_of(
        "request_binding.terminal",
        Some("request_binding"),
        &binding,
    ));
    let admitted = RequestBindingV1 {
        record_revision: RecordRevision(1),
        state: BindingState::Admitted,
        decision_authority: None,
        terminal_receipt: None,
        ..binding.clone()
    };
    out.push(vec_of(
        "request_binding.admitted",
        Some("request_binding"),
        &admitted,
    ));
    let expired = RequestBindingV1 {
        record_revision: RecordRevision(3),
        state: BindingState::ResultExpired,
        terminal_receipt: None,
        tombstone: Some(tomb.clone()),
        ..binding.clone()
    };
    out.push(vec_of(
        "request_binding.result_expired",
        Some("request_binding"),
        &expired,
    ));
    out.push(vec_of(
        "read_contract",
        Some("read_contract"),
        &ReadContractV1::operation_default(Visibility::Serial),
    ));
    let token = ObservationTokenV1::Serial {
        authority: AuthorityId::derive("authority"),
        idc_binding: IdcBinding {
            idc_id: IdcId::derive("idc"),
            idc_generation: IdcGeneration(1),
            authority_epoch: IdcAuthorityEpoch(1),
        },
        position: SerialPosition(42),
    };
    out.push(vec_of(
        "observation_token.serial",
        Some("observation_token"),
        &token,
    ));
    // ---- negotiation
    let caps = LocalCapabilities::v1(cluster, EndpointRole::Client, &limits, "DEV_LOCAL");
    let hello = caps.hello(vec![1u8; 16]);
    let server = LocalCapabilities::v1(cluster, EndpointRole::Node, &limits, "DEV_LOCAL");
    let ack = server.answer(&hello, vec![2u8; 16]).unwrap();
    out.push(vec_of("hello", Some("hello"), &hello));
    out.push(vec_of("hello_ack", Some("hello_ack"), &ack));
    out.push(vec_of(
        "codec_manifest",
        Some("codec_manifest"),
        &CodecManifest::v1(),
    ));
    // ---- storage
    let cb = batch(
        7,
        &[(carolina_storage::testutil::user_key(1, 1), vec![1, 2, 3])],
        &[carolina_storage::testutil::user_key(1, 2)],
        true,
    );
    out.push(vec_of("compiled_batch", Some("compiled_batch"), &cb));
    let pb = prepare_batch(8, &[(carolina_storage::testutil::user_key(2, 1), vec![9])]);
    out.push(vec_of(
        "compiled_batch.prepared",
        Some("compiled_batch"),
        &pb,
    ));
    let status = match &pb.protocol_mutations[0] {
        ProtocolMutation::SetTxnStatus(t) => t.next.clone(),
        _ => unreachable!(),
    };
    out.push(vec_of("txn_status.prepared", Some("txn_status"), &status));
    let terminal_status = match &cb.protocol_mutations[0] {
        ProtocolMutation::SetTxnStatus(t) => t.next.clone(),
        _ => unreachable!(),
    };
    out.push(vec_of(
        "txn_status.terminal",
        Some("txn_status"),
        &terminal_status,
    ));
    let pob = ProtocolOnlyBatch {
        internal_record_id: ProtocolRecordKey {
            record_kind: "request_binding".into(),
            scope_key: vec![1, 2],
            record_id: vec![3],
        },
        authorizing_evidence: vec![decision_ref(1)],
        writes: vec![ProtocolRecordWrite {
            key: ProtocolRecordKey {
                record_kind: "request_binding".into(),
                scope_key: vec![1, 2],
                record_id: vec![3],
            },
            expected: ExpectedRecordRevision::Exact(RecordRevision(1)),
            next: VersionedProtocolRecord {
                record_kind: "request_binding".into(),
                record_version: 1,
                canonical_payload: admitted.encode(),
            },
        }],
    };
    out.push(vec_of(
        "protocol_only_batch",
        Some("protocol_only_batch"),
        &pob,
    ));
    // ---- runtime records
    let decision = LocalDecisionV1 {
        txn_id: txn(7),
        request_key: request_key(7),
        request_hash: RequestHash(Hash256([7; 32])),
        outcome: Outcome::Committed,
        invocation_digest: Hash256([8; 32]),
        mutation_count: 2,
    };
    out.push(vec_of("local_decision", Some("local_decision"), &decision));
    let retirement = NamespaceRetirementV1 {
        tenant_id: TenantId::derive("tenant"),
        request_namespace: RequestNamespace::derive("inventory"),
        catalog_generation: CatalogGeneration(5),
        reason: "restore".into(),
    };
    out.push(vec_of(
        "namespace_retirement",
        Some("namespace_retirement"),
        &retirement,
    ));
    let grant = LocalGrantV1 {
        grant_id: GrantId::derive("g"),
        home_id: RequestHomeId::derive("home"),
        cluster_id: cluster,
        authority_kind: "local-request-home".into(),
        scope: "single-node".into(),
    };
    out.push(vec_of(
        "authority_grant.local",
        Some("authority_grant"),
        &grant,
    ));
    out.push(vec_of(
        "request_home_state",
        Some("request_home_state"),
        &RequestHomeStateV1 {
            home_id: RequestHomeId::derive("home"),
            epoch: RequestHomeEpoch(2),
            allocated_hint: RequestAllocationSeq(17),
        },
    ));
    // ---- catalog
    let manifest = BootstrapManifest {
        cluster_id: cluster,
        voters: vec![
            NodeId::derive("a"),
            NodeId::derive("b"),
            NodeId::derive("c"),
        ],
        trust_root_hash: Hash256([3; 32]),
        security_policy: SecurityPolicyId::derive("p"),
        bootstrap_admin: PrincipalId::derive("admin"),
        security_profile: "DEV_LOCAL".into(),
    };
    out.push(vec_of("bootstrap_manifest", None, &manifest));
    let tenant = TenantId::derive("tenant");
    let ag = AuthorityGrant {
        grant_id: GrantId::derive("home-grant"),
        binding: AuthorityBinding::RequestHome {
            home_id: RequestHomeId::derive("home"),
            epoch: RequestHomeEpoch(1),
        },
        tenant,
        scope: SemanticScopeId::derive("inventory"),
        scope_records: vec![RecordId(1), RecordId(2)],
        plans: vec![rc.plan],
        placement_epoch: PlacementEpoch(1),
        membership: (
            ReplicationGroupId::derive("voters"),
            MembershipGeneration(1),
        ),
        admitted_nodes: manifest.voters.clone(),
        authority_durability_policy: "quorum-of-3".into(),
        security_policy: manifest.security_policy,
        admission_mode: "ONLINE_BARRIER".into(),
        state: GrantState::Active,
    };
    out.push(vec_of(
        "authority_grant.catalog",
        Some("authority_grant"),
        &ag,
    ));
    let cmd = CatalogCommand {
        admin_request_id: AdminRequestId::derive("cmd-1"),
        principal: PrincipalId::derive("admin"),
        expected: vec![
            (
                CatalogKey::Authority(ag.grant_id),
                Expected::Revision(CatalogGeneration(2), Hash256([4; 32])),
            ),
            (CatalogKey::ClusterConfig, Expected::Absent),
        ],
        predicates: vec![
            Predicate::NoScopeOverlap {
                tenant,
                records: vec![RecordId(1)],
            },
            Predicate::GrantInState {
                grant: ag.grant_id,
                state: GrantState::Staged,
            },
        ],
        mutations: vec![
            Mutation::PutImmutable {
                key: CatalogKey::ScopeLock(tenant, SemanticScopeId::derive("s")),
                value: CanonValue::obj().fset("records", &[RecordId(1)]).build(),
            },
            Mutation::AdvancePointer {
                key: CatalogKey::RequestRoute(tenant, RequestNamespace::derive("inventory"), 0),
                value: RequestRoute {
                    home_id: RequestHomeId::derive("home"),
                    home_epoch: RequestHomeEpoch(1),
                    home_grant: ag.grant_id,
                }
                .to_canon(),
            },
            Mutation::AdvanceGrantState {
                grant: ag.grant_id,
                from: GrantState::Staged,
                to: GrantState::Active,
            },
            Mutation::Tombstone {
                key: CatalogKey::Namespace(tenant, RequestNamespace::derive("old")),
            },
        ],
    };
    out.push(vec_of("catalog_command", Some("catalog_command"), &cmd));
    let commit = CatalogCommit {
        admin_request_id: cmd.admin_request_id,
        command_hash: cmd.command_hash(),
        catalog_generation: CatalogGeneration(3),
        changed_keys: vec![CatalogKey::Authority(ag.grant_id)],
        committed_log_ref: 12,
        failure: None,
    };
    out.push(vec_of("catalog_commit", Some("catalog_commit"), &commit));
    out.push(vec_of(
        "request_route",
        None,
        &RequestRoute {
            home_id: RequestHomeId::derive("home"),
            home_epoch: RequestHomeEpoch(1),
            home_grant: ag.grant_id,
        },
    ));
    // ---- node log and admin payloads
    let node = NodeId::derive("a");
    let cmds: Vec<(&str, NodeCommand)> = vec![
        (
            "node_command.genesis",
            NodeCommand::Genesis(manifest.clone()),
        ),
        ("node_command.catalog", NodeCommand::Catalog(cmd.clone())),
        (
            "node_command.admit",
            NodeCommand::Admit {
                invoke: inv.clone(),
                principal: PrincipalId::derive("dev-local-client"),
                admitted_by: node,
                catalog_generation: CatalogGeneration(3),
            },
        ),
        (
            "node_command.decision",
            NodeCommand::Decision {
                request_key: rc.request_key,
                txn_id: rc.txn_id,
                outcome: "COMMITTED".into(),
                receipt_digest: rc.receipt_digest().0,
                decided_by: node,
            },
        ),
        (
            "node_command.seed",
            NodeCommand::Seed {
                label: "seed".into(),
                record: "Item".into(),
                rows: vec![(
                    uuid(1),
                    vec![("id".into(), uuid(1)), ("available".into(), Value::I64(3))],
                )],
            },
        ),
    ];
    for (n, c) in &cmds {
        out.push(vec_of(n, None, c));
    }
    out.push(vec_of("admin_request.status", None, &AdminRequest::Status));
    out.push(vec_of(
        "admin_request.catalog",
        None,
        &AdminRequest::Catalog(cmd.clone()),
    ));
    let st = NodeStatus {
        node,
        readiness: "Ready".into(),
        role: "Leader".into(),
        term: 2,
        leader: Some(node),
        commit_index: 10,
        applied_index: 10,
        catalog_generation: CatalogGeneration(3),
        genesis: true,
        grant_active: true,
        state_digest: Hash256([5; 32]),
        durable_commit_seq: 9,
        failure: None,
        metrics: BTreeMap::from([
            ("consensus.commit_index".to_string(), 10u64),
            ("engine.committed".to_string(), 7),
            ("storage.commit_total".to_string(), 9),
        ]),
    };
    out.push(vec_of("admin_reply.status", None, &AdminReply::Status(st)));
    out.push(vec_of(
        "admin_reply.committed",
        None,
        &AdminReply::Committed(commit.clone()),
    ));
    out.push(vec_of(
        "admin_reply.refused",
        None,
        &AdminReply::Refused {
            code: "NotLeader".into(),
            detail: String::new(),
            leader: Some(node),
        },
    ));
    // ---- consensus envelope
    let env = Envelope {
        from: NodeId::derive("a"),
        to: NodeId::derive("b"),
        msg: Message::AppendEntries {
            term: 3,
            prev_log_index: 4,
            prev_log_term: 2,
            entries: vec![Entry {
                term: 3,
                index: 5,
                data: b"x".to_vec(),
            }],
            leader_commit: 4,
            read_round: 1,
        },
    };
    out.push(vec_of("consensus_envelope.append_entries", None, &env));
    let vote = Envelope {
        from: NodeId::derive("a"),
        to: NodeId::derive("b"),
        msg: Message::RequestVote {
            term: 3,
            last_log_index: 4,
            last_log_term: 2,
        },
    };
    out.push(vec_of("consensus_envelope.request_vote", None, &vote));

    // SPEC-012 §11 snapshot interchange: the manifest and chunk codecs exist (the store-level
    // export/import does not), so their canonical bytes are frozen like every other enabled codec.
    let chunk = SnapshotChunkV1 {
        snapshot_id: [9u8; 16],
        index: 0,
        records: vec![
            carolina_wire::registry::CanonicalRecord::wrap("result_tombstone", &tomb).unwrap(),
        ],
    };
    let manifest = SnapshotManifestV1 {
        snapshot_id: [9u8; 16],
        snapshot_version: 1,
        cluster_id: ClusterId::derive("corpus-cluster"),
        tenant_scope: vec![TenantId::derive("tenant")],
        kind: SnapshotKind::LocalStorage,
        source_identity: StorageId::derive("corpus-storage"),
        source_storage_epoch: StorageEpoch(1),
        catalog_generation: CatalogGeneration(3),
        plan_refs: vec![],
        idc_bindings: vec![],
        membership_generation: MembershipGeneration(1),
        semantic_cut_with_holes: CanonValue::Null,
        required_codec_manifest: CodecManifest::v1().manifest_hash(),
        required_artifact_refs: vec![],
        request_namespace_retirements: vec![],
        retained_result_horizons: CanonValue::Null,
        unresolved_protocol_refs: vec![],
        authority_fences: vec![],
        chunks: vec![chunk.descriptor()],
    };
    out.push(vec_of(
        "snapshot_manifest",
        Some("snapshot_manifest"),
        &manifest,
    ));
    out.push(vec_of("snapshot_chunk", Some("snapshot_chunk"), &chunk));
    out
}

/// Negative vectors derived from a frozen vector: every one must fail to decode.
pub fn negative_vectors(v: &CodecVector) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    // unknown field injected at the top level (decoders reject unknown fields)
    let text = String::from_utf8_lossy(&v.bytes).to_string();
    if let Some(rest) = text.strip_prefix('{') {
        out.push((
            "unknown_field".into(),
            format!("{{\"__zz_unknown\":\"1\",{rest}").into_bytes(),
        ));
    }
    // truncated
    if v.bytes.len() > 8 {
        out.push(("truncated".into(), v.bytes[..v.bytes.len() / 2].to_vec()));
    }
    // JSON number instead of canonical decimal string (forbidden by SPEC-003 §10)
    if let Some(pos) = text.find("\":\"") {
        let mut mutated = text.clone();
        // replace the first string value with a bare number
        if let Some(end) = mutated[pos + 3..].find('"') {
            mutated.replace_range(pos + 2..pos + 3 + end + 1, "0");
            out.push(("number_literal".into(), mutated.into_bytes()));
        }
    }
    // whitespace (non-canonical)
    out.push(("whitespace".into(), format!(" {text}").into_bytes()));
    out
}

#[derive(Debug, Clone, Default)]
pub struct CorpusStats {
    pub frozen: usize,
    pub kinds_frozen: Vec<String>,
    pub kinds_without_codec: Vec<String>,
    pub negatives: usize,
}

/// Compare (or, with `update`, freeze) the corpus under `<root>/fixtures/codec`.
pub fn check(root: &Path, update: bool) -> Result<CorpusStats, String> {
    let dir = root.join("fixtures").join("codec");
    let vectors = vectors();
    let manifest_path = dir.join("manifest.txt");
    let existing: BTreeMap<String, String> = std::fs::read_to_string(&manifest_path)
        .map(|t| {
            t.lines()
                .filter_map(|l| l.split_once('='))
                .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut manifest = String::new();
    let mut stats = CorpusStats::default();
    for v in &vectors {
        let digest = sha256(&v.bytes);
        let path = dir.join(format!("{}.astj", v.name));
        if update || !path.exists() {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(&path, &v.bytes).map_err(|e| e.to_string())?;
        }
        let frozen = std::fs::read(&path).map_err(|e| format!("{}: {e}", v.name))?;
        if frozen != v.bytes {
            return Err(format!("{}: sample bytes differ from the frozen corpus (a codec change; re-freeze deliberately)", v.name));
        }
        let back = (v.roundtrip)(&frozen)
            .map_err(|e| format!("{}: typed decode of the frozen bytes failed: {e}", v.name))?;
        if back != frozen {
            return Err(format!("{}: decode/encode is not a fixed point", v.name));
        }
        if !update {
            match existing.get(&v.name) {
                Some(d) if *d == digest.to_string() => {}
                Some(d) => {
                    return Err(format!(
                        "{}: digest {} differs from the manifest {d}",
                        v.name, digest
                    ))
                }
                None => {
                    return Err(format!(
                        "{}: not in manifest.txt (re-freeze deliberately)",
                        v.name
                    ))
                }
            }
        }
        for (label, bytes) in negative_vectors(v) {
            if (v.roundtrip)(&bytes).is_ok() {
                return Err(format!(
                    "{}: negative vector `{label}` was accepted",
                    v.name
                ));
            }
            stats.negatives += 1;
        }
        manifest.push_str(&format!("{}={}\n", v.name, digest));
        stats.frozen += 1;
        if let Some(k) = v.registered_kind {
            if !stats.kinds_frozen.contains(&k.to_string()) {
                stats.kinds_frozen.push(k.to_string());
            }
        }
    }
    if update {
        std::fs::write(&manifest_path, &manifest).map_err(|e| e.to_string())?;
    }
    for (k, _, _) in REGISTERED_KINDS {
        if !stats.kinds_frozen.iter().any(|x| x == k) {
            stats.kinds_without_codec.push(k.to_string());
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_corpus_is_frozen() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let update = std::env::var("UPDATE_GOLDEN").is_ok();
        let s = check(&root, update).unwrap();
        assert!(s.frozen >= 40, "{}", s.frozen);
        assert!(s.negatives >= 100);
        assert!(s.kinds_frozen.iter().any(|k| k == "final_receipt"));
        eprintln!(
            "frozen {} vectors; kinds without an implemented codec: {:?}",
            s.frozen, s.kinds_without_codec
        );
    }
}
