//! Local engine: the SPEC-014 §4 request path over one durable store.
//!
//! Every reply is derived from durable protocol state: a request is bound before execution, the
//! decision, the terminal receipt and the binding transition land in one `CompiledBatch`, and a
//! lost reply is answered by `resolve` from the persisted receipt — byte-identical after restart.
//! Timeouts and I/O failures after the journal barrier surface as `OutcomeUnknown`, never as an
//! invented abort (SPEC-012 §6).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, Hash256};
use carolina_core::ids::*;
use carolina_core::limits::Limits;
use carolina_lang::interp::{evaluate_scoped, Outcome as EvalOutcome, Stage};
use carolina_lang::types::Value;
use carolina_storage::batch::*;
use carolina_storage::kernel::{
    request_binding_record_key, CheckpointInfo, DurableStorageKernel, Readiness, Store,
    StoreOptions, VerifyMode, VerifyReport,
};
use carolina_wire::records::*;
use carolina_wire::registry::CanonicalRecord;

use crate::catalog::LocalCatalog;
use crate::home::LocalRequestHome;
use crate::state::{decode_row, diff_states, load_records, row_key};

pub const RESULT_CODEC: &str = "canonical-value:1";
pub const LOCAL_GRANT_KIND: &str = "authority_grant";
pub const LOCAL_DECISION_KIND: &str = "local_decision";
pub const NAMESPACE_RETIREMENT_KIND: &str = "namespace_retirement";
pub const LOCAL_DURABILITY_POLICY: &str = "LocalStable";

// ---------------------------------------------------------------------------
// Local protocol records
// ---------------------------------------------------------------------------

/// The explicitly local grant under which this home decides (SPEC-014 §3, MVP-2). It binds the
/// home to one cluster and states its scope; a distributed grant is a different record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalGrantV1 {
    pub grant_id: GrantId,
    pub home_id: RequestHomeId,
    pub cluster_id: ClusterId,
    pub authority_kind: String,
    pub scope: String,
}

impl Canonical for LocalGrantV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("authority_kind", &self.authority_kind)
            .fc("cluster_id", &self.cluster_id)
            .fc("grant_id", &self.grant_id)
            .fc("home_id", &self.home_id)
            .fstr("scope", &self.scope)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "authority_kind",
            "cluster_id",
            "grant_id",
            "home_id",
            "scope",
        ])?;
        Ok(LocalGrantV1 {
            grant_id: GrantId::from_canon(v.field("grant_id")?)?,
            home_id: RequestHomeId::from_canon(v.field("home_id")?)?,
            cluster_id: ClusterId::from_canon(v.field("cluster_id")?)?,
            authority_kind: v.field("authority_kind")?.as_str()?.to_string(),
            scope: v.field("scope")?.as_str()?.to_string(),
        })
    }
}

/// The local decision of one execution (SPEC-002 local decision record). Its reference is the
/// receipt's `decision_ref`; it is computed before the batch digest so the receipt can carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDecisionV1 {
    pub txn_id: TxnId,
    pub request_key: RequestKey,
    pub request_hash: RequestHash,
    pub outcome: Outcome,
    pub invocation_digest: Hash256,
    pub mutation_count: u64,
}

impl Canonical for LocalDecisionV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("invocation_digest", &self.invocation_digest)
            .fu64("mutation_count", self.mutation_count)
            .fstr("outcome", self.outcome.label())
            .fc("request_hash", &self.request_hash)
            .fc("request_key", &self.request_key)
            .fc("txn_id", &self.txn_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "invocation_digest",
            "mutation_count",
            "outcome",
            "request_hash",
            "request_key",
            "txn_id",
        ])?;
        Ok(LocalDecisionV1 {
            txn_id: TxnId::from_canon(v.field("txn_id")?)?,
            request_key: RequestKey::from_canon(v.field("request_key")?)?,
            request_hash: RequestHash::from_canon(v.field("request_hash")?)?,
            outcome: Outcome::from_label(v.field("outcome")?.as_str()?)?,
            invocation_digest: Hash256::from_canon(v.field("invocation_digest")?)?,
            mutation_count: v.field("mutation_count")?.as_u64()?,
        })
    }
}

/// Retirement of a request namespace (SPEC-011): identities under it are expired, never reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceRetirementV1 {
    pub tenant_id: TenantId,
    pub request_namespace: RequestNamespace,
    pub catalog_generation: CatalogGeneration,
    pub reason: String,
}

impl Canonical for NamespaceRetirementV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("catalog_generation", &self.catalog_generation)
            .fstr("reason", &self.reason)
            .fc("request_namespace", &self.request_namespace)
            .fc("tenant_id", &self.tenant_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "catalog_generation",
            "reason",
            "request_namespace",
            "tenant_id",
        ])?;
        Ok(NamespaceRetirementV1 {
            tenant_id: TenantId::from_canon(v.field("tenant_id")?)?,
            request_namespace: RequestNamespace::from_canon(v.field("request_namespace")?)?,
            catalog_generation: CatalogGeneration::from_canon(v.field("catalog_generation")?)?,
            reason: v.field("reason")?.as_str()?.to_string(),
        })
    }
}

fn versioned<T: Canonical>(kind: &str, body: &T) -> CoreResult<VersionedProtocolRecord> {
    let rec = CanonicalRecord::wrap(kind, body)?;
    Ok(VersionedProtocolRecord {
        record_kind: rec.record_kind,
        record_version: rec.record_version,
        canonical_payload: body.encode(),
    })
}

fn grant_key(home: RequestHomeId) -> ProtocolRecordKey {
    ProtocolRecordKey {
        record_kind: LOCAL_GRANT_KIND.into(),
        scope_key: home.0.to_vec(),
        record_id: b"local".to_vec(),
    }
}

fn retirement_key(tenant: TenantId, ns: RequestNamespace) -> ProtocolRecordKey {
    let mut scope = Vec::with_capacity(32);
    scope.extend_from_slice(&tenant.0);
    scope.extend_from_slice(&ns.0);
    ProtocolRecordKey {
        record_kind: NAMESPACE_RETIREMENT_KIND.into(),
        scope_key: scope,
        record_id: vec![],
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct EngineOptions {
    pub store: StoreOptions,
    pub home_id: RequestHomeId,
    pub cluster_id: ClusterId,
    /// `Some(epoch)`: the home epoch is pinned by an external authority (ordered/replicated
    /// execution) and allocation continues from the durable counter; `None`: local mode, the
    /// epoch advances on every open.
    pub pinned_home_epoch: Option<RequestHomeEpoch>,
}

impl EngineOptions {
    pub fn local(store: StoreOptions) -> EngineOptions {
        EngineOptions {
            store,
            home_id: RequestHomeId::derive("local-home"),
            cluster_id: ClusterId::derive("local-cluster"),
            pinned_home_epoch: None,
        }
    }
}

pub struct LocalEngine {
    /// Shared, immutable compiled catalog (compile once, open many data directories).
    pub catalog: Arc<LocalCatalog>,
    store: Store,
    home: LocalRequestHome,
    cluster_id: ClusterId,
    grant_ref: ProtocolRecordRef,
}

enum Phase {
    BeforeBinding,
    AfterBinding,
}

impl LocalEngine {
    /// Create a new data directory: grant, then home. Nothing else is written.
    pub fn create(dir: &Path, catalog: Arc<LocalCatalog>, opts: EngineOptions) -> CoreResult<Self> {
        let mut store = Store::create(dir, opts.store.clone())?;
        let grant = LocalGrantV1 {
            grant_id: GrantId::derive("local-grant"),
            home_id: opts.home_id,
            cluster_id: opts.cluster_id,
            authority_kind: "local-request-home".into(),
            scope: "single-node; no replication; no multi-IDC atomicity".into(),
        };
        let key = grant_key(opts.home_id);
        store.commit_protocol(ProtocolOnlyBatch {
            internal_record_id: key.clone(),
            authorizing_evidence: vec![],
            writes: vec![ProtocolRecordWrite {
                key,
                expected: ExpectedRecordRevision::Absent,
                next: versioned(LOCAL_GRANT_KIND, &grant)?,
            }],
        })?;
        Self::finish_open(store, catalog, opts)
    }

    /// Open an existing directory: recovery, grant verification, home epoch advance.
    pub fn open(dir: &Path, catalog: Arc<LocalCatalog>, opts: EngineOptions) -> CoreResult<Self> {
        let store = Store::open(dir, opts.store.clone())?;
        Self::finish_open(store, catalog, opts)
    }

    fn finish_open(
        mut store: Store,
        catalog: Arc<LocalCatalog>,
        opts: EngineOptions,
    ) -> CoreResult<Self> {
        if store.readiness() != Readiness::Ready {
            return Err(CoreError::new(
                ErrorCode::NotReady,
                format!("store readiness {:?}", store.readiness()),
            ));
        }
        let key = grant_key(opts.home_id);
        let grant = match store.read_protocol_record(&key)? {
            Some((_, rec)) => LocalGrantV1::decode(&rec.canonical_payload, &Limits::v1())?,
            None => {
                return Err(CoreError::new(
                    ErrorCode::AuthorityUnavailable,
                    "no local grant for this request home",
                ))
            }
        };
        if grant.home_id != opts.home_id || grant.cluster_id != opts.cluster_id {
            return Err(CoreError::new(
                ErrorCode::AuthorityUnavailable,
                "local grant does not match home/cluster",
            ));
        }
        let grant_ref =
            CanonicalRecord::wrap(LOCAL_GRANT_KIND, &grant)?.reference(key.logical_key().0);
        let home = match opts.pinned_home_epoch {
            Some(epoch) => LocalRequestHome::open_pinned(&mut store, opts.home_id, epoch)?,
            None => LocalRequestHome::open(&mut store, opts.home_id)?,
        };
        Ok(LocalEngine {
            catalog,
            store,
            home,
            cluster_id: opts.cluster_id,
            grant_ref,
        })
    }

    pub fn home_id(&self) -> RequestHomeId {
        self.home.home_id
    }
    pub fn home_epoch(&self) -> RequestHomeEpoch {
        self.home.epoch
    }
    pub fn cluster_id(&self) -> ClusterId {
        self.cluster_id
    }
    pub fn store(&mut self) -> &mut Store {
        &mut self.store
    }
    pub fn checkpoint(&mut self) -> CoreResult<CheckpointInfo> {
        self.store.checkpoint()
    }
    pub fn verify(&mut self, mode: VerifyMode) -> CoreResult<VerifyReport> {
        self.store.verify(mode)
    }

    // ---- request path ------------------------------------------------------

    /// Serve one client request (SPEC-012 §5). Never panics on client input.
    pub fn invoke(&mut self, inv: &InvokeV1) -> ClientReplyV1 {
        match self.try_invoke(inv) {
            Ok(r) => r,
            Err((phase, e)) => Self::error_reply(inv, phase, e, self.home.home_id, self.home.epoch),
        }
    }

    fn error_reply(
        inv: &InvokeV1,
        phase: Phase,
        e: CoreError,
        home: RequestHomeId,
        epoch: RequestHomeEpoch,
    ) -> ClientReplyV1 {
        match e.code {
            ErrorCode::IdentityConflict | ErrorCode::RequestIdentityMismatch => {
                return ClientReplyV1::RequestIdentityMismatch
            }
            ErrorCode::NamespaceRetired | ErrorCode::IdentityExpired => {}
            _ => {}
        }
        let durable_uncertain = matches!(
            e.code,
            ErrorCode::Io
                | ErrorCode::DiskFull
                | ErrorCode::Corruption
                | ErrorCode::ChecksumMismatch
                | ErrorCode::Internal
                | ErrorCode::OutcomeUnknown
                | ErrorCode::Unavailable
        );
        match phase {
            Phase::AfterBinding if durable_uncertain => {
                ClientReplyV1::OutcomeUnknown(ResolutionHintV1 {
                    request_key: inv.content.request_key,
                    request_hash: inv.request_hash,
                    txn_id: None,
                    resolver: Some(RouteHint { home, epoch }),
                })
            }
            Phase::AfterBinding => ClientReplyV1::Unavailable(RefusalV1 {
                code: format!("{:?}", e.code),
                detail: e.message,
                possibly_admitted: true,
            }),
            Phase::BeforeBinding if durable_uncertain => {
                // the binding commit itself may have reached the journal: resolve, do not re-invoke blindly
                ClientReplyV1::OutcomeUnknown(ResolutionHintV1 {
                    request_key: inv.content.request_key,
                    request_hash: inv.request_hash,
                    txn_id: None,
                    resolver: Some(RouteHint { home, epoch }),
                })
            }
            Phase::BeforeBinding => ClientReplyV1::Unavailable(RefusalV1 {
                code: format!("{:?}", e.code),
                detail: e.message,
                possibly_admitted: false,
            }),
        }
    }

    fn retirement(&mut self, key: &RequestKey) -> CoreResult<Option<ProtocolRecordRef>> {
        let rk = retirement_key(key.tenant_id, key.request_namespace);
        match self.store.read_protocol_record(&rk)? {
            Some((_, rec)) => {
                let r = NamespaceRetirementV1::decode(&rec.canonical_payload, &Limits::v1())?;
                Ok(Some(
                    CanonicalRecord::wrap(NAMESPACE_RETIREMENT_KIND, &r)?
                        .reference(rk.logical_key().0),
                ))
            }
            None => Ok(None),
        }
    }

    fn reply_from_receipt(r: &FinalReceiptV1) -> ClientReplyV1 {
        match r.outcome {
            Outcome::Committed => ClientReplyV1::Committed(r.clone()),
            Outcome::Rejected => ClientReplyV1::Rejected(r.clone()),
        }
    }

    fn try_invoke(&mut self, inv: &InvokeV1) -> Result<ClientReplyV1, (Phase, CoreError)> {
        let pre = |e: CoreError| (Phase::BeforeBinding, e);
        let post = |e: CoreError| (Phase::AfterBinding, e);
        if inv.verify_hash().is_err() {
            return Ok(ClientReplyV1::RequestIdentityMismatch);
        }
        let content = &inv.content;
        let key = content.request_key;
        if let Some(retirement_ref) = self.retirement(&key).map_err(pre)? {
            return Ok(ClientReplyV1::IdentityExpired {
                namespace: key.request_namespace,
                retirement_ref,
            });
        }
        if self.store.readiness() != Readiness::Ready {
            return Err(pre(CoreError::new(
                ErrorCode::NotReady,
                format!("{:?}", self.store.readiness()),
            )));
        }
        // ---- admission: identity of the declared operation, plan, schema and contract ----
        let op = self
            .catalog
            .module
            .operation(content.operation)
            .map_err(pre)?
            .clone();
        let plan = self
            .catalog
            .plan_for(op.identity)
            .ok_or_else(|| pre(CoreError::new(ErrorCode::NoSafePlan, "no active plan")))?
            .clone();
        if content.schema_hash != self.catalog.hashes.schema_hash {
            return Err(pre(CoreError::new(
                ErrorCode::StaleSchema,
                "request schema hash is not the active schema",
            )));
        }
        if content.operation_hash != plan.operation_hash
            || content.contract_hash != plan.contract_hash
        {
            return Err(pre(CoreError::new(
                ErrorCode::StalePlan,
                "request operation/contract hash is not the active version",
            )));
        }
        let args = match Value::from_canon(&content.arguments).map_err(pre)? {
            Value::Tuple(v) => v,
            _ => {
                return Err(pre(CoreError::new(
                    ErrorCode::TypeMismatch,
                    "arguments must be a tuple",
                )))
            }
        };
        if args.len() != op.parameters.len() {
            return Err(pre(CoreError::new(
                ErrorCode::TypeMismatch,
                "argument count mismatch",
            )));
        }
        for (a, p) in args.iter().zip(&op.parameters) {
            a.check_type(&p.ty).map_err(pre)?;
        }
        // ---- durable identity binding (SPEC-012 §3) ----
        let plan_ref = plan.plan_ref();
        let (rev, binding) = self
            .home
            .bind_if_absent(&mut self.store, &key, inv.request_hash, plan_ref)
            .map_err(pre)?;
        match binding.state {
            BindingState::Terminal => {
                let r = binding.terminal_receipt.as_ref().ok_or_else(|| {
                    post(CoreError::new(
                        ErrorCode::Corruption,
                        "terminal binding without receipt",
                    ))
                })?;
                return Ok(Self::reply_from_receipt(r));
            }
            BindingState::ResultExpired => {
                let t = binding.tombstone.clone().ok_or_else(|| {
                    post(CoreError::new(
                        ErrorCode::Corruption,
                        "expired binding without tombstone",
                    ))
                })?;
                return Ok(ClientReplyV1::ResultExpired(t));
            }
            BindingState::Bound | BindingState::Admitted => {}
        }
        // a durable decision may already exist for this txn (binding lag is impossible in one
        // batch, but the check keeps the invariant "one execution per key" independent of that)
        if let Some(st) = self.store.txn_status(binding.txn_id).map_err(post)? {
            if let Some(t) = &st.terminal_outcome {
                return Ok(Self::reply_from_receipt(t.receipt()));
            }
            if st.phase == TxnPhase::Prepared {
                return Err(post(CoreError::new(
                    ErrorCode::TxnInDoubt,
                    "execution is prepared and undecided",
                )));
            }
        }
        // ---- ordered evaluation over the closure of the operation ----
        let records = self.catalog.closure_records(op.identity);
        let snap = self.store.snapshot();
        let pre_state = load_records(&mut self.store, snap, &records);
        self.store.release_snapshot(snap);
        let pre_state = pre_state.map_err(post)?;
        let invariants = self
            .catalog
            .output
            .closure
            .op_invariants
            .get(&op.identity)
            .ok_or_else(|| {
                post(CoreError::new(
                    ErrorCode::InvalidIr,
                    "missing invariant closure",
                ))
            })?;
        let eval = evaluate_scoped(
            &self.catalog.module,
            op.identity,
            &args,
            &pre_state,
            invariants,
        )
        .map_err(post)?;
        let result_type_hash = domain_hash("astra.type.v1", &op.result.ty.encode());
        let idc_bindings = Self::idc_bindings(&plan, &args);
        let (outcome, result_bytes, commitments, mutations, captured, invocation_digest) =
            match eval {
                EvalOutcome::Accepted(c) => {
                    let mutations =
                        diff_states(&records, &pre_state, &c.post_state).map_err(post)?;
                    let digest = c.invocation_digest(op.identity, &args);
                    let commitments = CanonValue::obj()
                        .fc("invocation_digest", &digest)
                        .fu64("mutations", mutations.len() as u64)
                        .build();
                    (
                        Outcome::Committed,
                        c.result.encode(),
                        commitments,
                        mutations,
                        c.normalized_invocation(op.identity, &args).encode(),
                        digest,
                    )
                }
                EvalOutcome::Rejected(r) => {
                    if matches!(r.stage, Stage::Definedness) {
                        return Err(post(CoreError::new(r.code, r.reason)));
                    }
                    let body = CanonValue::obj()
                        .fstr("code", &format!("{:?}", r.code))
                        .fstr("reason", &r.reason)
                        .fstr("stage", &format!("{:?}", r.stage))
                        .build();
                    (
                        Outcome::Rejected,
                        body.encode(),
                        CanonValue::obj().build(),
                        vec![],
                        vec![],
                        Hash256::ZERO,
                    )
                }
            };
        let decision = LocalDecisionV1 {
            txn_id: binding.txn_id,
            request_key: key,
            request_hash: inv.request_hash,
            outcome,
            invocation_digest,
            mutation_count: mutations.len() as u64,
        };
        let decision_ref = CanonicalRecord::wrap(LOCAL_DECISION_KIND, &decision)
            .map_err(post)?
            .reference(binding.txn_id.0.to_vec());
        let ar = AcceptedResultV1::new(
            outcome,
            RESULT_CODEC,
            result_type_hash,
            result_bytes,
            commitments,
            None,
        );
        let receipt = FinalReceiptV1 {
            receipt_version: FinalReceiptV1::VERSION,
            cluster_id: self.cluster_id,
            request_key: key,
            request_hash: inv.request_hash,
            txn_id: binding.txn_id,
            operation: op.identity,
            operation_hash: plan.operation_hash,
            schema_hash: plan.schema_hash,
            contract_hash: plan.contract_hash,
            plan: plan_ref,
            idc_bindings: idc_bindings.clone(),
            origin_ids: vec![],
            outcome,
            result_codec: ar.result_codec.clone(),
            result_type_hash,
            exact_result_bytes: ar.exact_result_bytes.clone(),
            result_digest: ar.result_digest,
            commitments: ar.commitments.clone(),
            observation_token: None,
            durability_policy: PolicyRef {
                name: LOCAL_DURABILITY_POLICY.into(),
                policy_hash: domain_hash("astra.policy.v1", LOCAL_DURABILITY_POLICY.as_bytes()),
            },
            durability_evidence: vec![self.grant_ref.clone()],
            decision_ref: decision_ref.clone(),
            completion_ref: None,
        };
        let terminal = match outcome {
            Outcome::Committed => TerminalOutcome::Committed(receipt.clone()),
            Outcome::Rejected => TerminalOutcome::Rejected(receipt.clone()),
        };
        let terminal_binding = RequestBindingV1 {
            record_revision: RecordRevision(rev.0 + 1),
            state: BindingState::Terminal,
            admitted_plan: Some(plan_ref),
            decision_authority: Some(self.grant_ref.clone()),
            terminal_receipt: Some(receipt.clone()),
            tombstone: None,
            ..binding.clone()
        };
        let bkey = request_binding_record_key(&key);
        let protocol_mutations = vec![
            ProtocolMutation::PutRecord(ProtocolRecordWrite {
                key: bkey,
                expected: ExpectedRecordRevision::Exact(rev),
                next: versioned("request_binding", &terminal_binding).map_err(post)?,
            }),
            ProtocolMutation::SetTxnStatus(TxnStatusTransition {
                txn_id: binding.txn_id,
                request_key: key,
                request_hash: inv.request_hash,
                expected_revision: ExpectedRecordRevision::Absent,
                next: TxnStatusRecord {
                    revision: RecordRevision(1),
                    phase: TxnPhase::Terminal,
                    plan: plan_ref,
                    idc_bindings: idc_bindings.clone(),
                    prepared_digest: None,
                    decision_ref: Some(decision_ref),
                    accepted_result: Some(ar),
                    terminal_outcome: Some(terminal.clone()),
                },
            }),
        ];
        let batch = CompiledBatch {
            batch_version: BATCH_VERSION,
            txn_id: binding.txn_id,
            request_key: key,
            request_hash: inv.request_hash,
            operation: op.identity,
            operation_hash: plan.operation_hash,
            contract_hash: plan.contract_hash,
            schema_hash: plan.schema_hash,
            plan: plan_ref,
            idc_bindings,
            consistency_class: plan.profile.family,
            origin: None,
            semantic_evidence: vec![],
            captured_inputs: captured,
            mutations,
            protocol_mutations,
            terminal_outcome: Some(terminal),
            semantic_digest: SemanticDigest::default(),
        }
        .seal();
        self.store.commit(batch).map_err(post)?;
        Ok(Self::reply_from_receipt(&receipt))
    }

    /// One binding per IDC template instance the plan routes through (local generation/epoch 1).
    fn idc_bindings(plan: &carolina_compiler::OperationPlan, args: &[Value]) -> Vec<IdcBinding> {
        let mut out: Vec<IdcBinding> = plan
            .routing
            .instance_keys
            .iter()
            .map(|(name, idx)| {
                let label = match idx.and_then(|i| args.get(i as usize)) {
                    Some(v) => format!("{name}/{}", carolina_core::hash::hex_encode(&v.encode())),
                    None => name.clone(),
                };
                IdcBinding {
                    idc_id: IdcId::derive(&label),
                    idc_generation: IdcGeneration(1),
                    authority_epoch: IdcAuthorityEpoch(1),
                }
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Resolve a lost reply from durable state (SPEC-012 §6).
    pub fn resolve(&mut self, req: &ResolveRequestV1) -> ResolveReplyV1 {
        match self.try_resolve(req) {
            Ok(r) => r,
            Err(e) => ResolveReplyV1::Unavailable(RefusalV1 {
                code: format!("{:?}", e.code),
                detail: e.message,
                possibly_admitted: true,
            }),
        }
    }

    fn try_resolve(&mut self, req: &ResolveRequestV1) -> CoreResult<ResolveReplyV1> {
        let key = req.request_key;
        if let Some(retirement_ref) = self.retirement(&key)? {
            return Ok(ResolveReplyV1::Terminal(Box::new(
                ClientReplyV1::IdentityExpired {
                    namespace: key.request_namespace,
                    retirement_ref,
                },
            )));
        }
        let ev = self.store.resolve(&key)?;
        let b = match ev.binding {
            None => {
                return Ok(ResolveReplyV1::AbsentAtBarrier {
                    catalog_generation: self.catalog.generation,
                    home: self.home.home_id,
                    home_epoch: self.home.epoch,
                    record_revision: RecordRevision(0),
                })
            }
            Some(b) => b,
        };
        if b.request_hash != req.expected_request_hash {
            return Ok(ResolveReplyV1::Terminal(Box::new(
                ClientReplyV1::RequestIdentityMismatch,
            )));
        }
        match b.state {
            BindingState::Terminal => {
                let r = b.terminal_receipt.as_ref().ok_or_else(|| {
                    CoreError::new(ErrorCode::Corruption, "terminal binding without receipt")
                })?;
                Ok(ResolveReplyV1::Terminal(Box::new(
                    Self::reply_from_receipt(r),
                )))
            }
            BindingState::ResultExpired => {
                let t = b.tombstone.clone().ok_or_else(|| {
                    CoreError::new(ErrorCode::Corruption, "expired binding without tombstone")
                })?;
                Ok(ResolveReplyV1::Terminal(Box::new(
                    ClientReplyV1::ResultExpired(t),
                )))
            }
            BindingState::Bound | BindingState::Admitted => {
                if let Some(st) = &ev.status {
                    if let Some(t) = &st.terminal_outcome {
                        return Ok(ResolveReplyV1::Terminal(Box::new(
                            Self::reply_from_receipt(t.receipt()),
                        )));
                    }
                }
                let phase = match &ev.status {
                    Some(st) => st.phase.label().to_string(),
                    None => b.state.label().to_string(),
                };
                let original_plan = b.admitted_plan.ok_or_else(|| {
                    CoreError::new(ErrorCode::Corruption, "admitted binding without plan")
                })?;
                let resolver_ref = CanonicalRecord::wrap("request_binding", &b)?
                    .reference(request_binding_record_key(&key).logical_key().0);
                Ok(ResolveReplyV1::Pending {
                    txn_id: b.txn_id,
                    phase,
                    original_plan,
                    resolver_ref,
                })
            }
        }
    }

    // ---- administrative protocol transitions -----------------------------

    /// Evict a retained result (SPEC-012 §7): the receipt is replaced by a tombstone that keeps
    /// the outcome, the receipt digest and the decision reference.
    pub fn evict_result(&mut self, key: &RequestKey) -> CoreResult<ResultTombstoneV1> {
        let (rev, b) = LocalRequestHome::lookup(&mut self.store, key)?
            .ok_or_else(|| CoreError::new(ErrorCode::MissingRecord, "no binding"))?;
        let r = match (&b.state, &b.terminal_receipt) {
            (BindingState::Terminal, Some(r)) => r.clone(),
            (BindingState::ResultExpired, _) => {
                return b.tombstone.clone().ok_or_else(|| {
                    CoreError::new(ErrorCode::Corruption, "expired binding without tombstone")
                })
            }
            _ => {
                return Err(CoreError::new(
                    ErrorCode::TxnInDoubt,
                    "only terminal results can be evicted",
                ))
            }
        };
        let tombstone = ResultTombstoneV1 {
            request_key: *key,
            request_hash: r.request_hash,
            txn_id: r.txn_id,
            outcome: r.outcome,
            receipt_digest: r.receipt_digest(),
            decision_ref: r.decision_ref.clone(),
        };
        let next = RequestBindingV1 {
            record_revision: RecordRevision(rev.0 + 1),
            state: BindingState::ResultExpired,
            terminal_receipt: None,
            tombstone: Some(tombstone.clone()),
            ..b
        };
        let bkey = request_binding_record_key(key);
        self.store.commit_protocol(ProtocolOnlyBatch {
            internal_record_id: bkey.clone(),
            authorizing_evidence: vec![self.grant_ref.clone()],
            writes: vec![ProtocolRecordWrite {
                key: bkey,
                expected: ExpectedRecordRevision::Exact(rev),
                next: versioned("request_binding", &next)?,
            }],
        })?;
        Ok(tombstone)
    }

    /// Retire a request namespace (SPEC-011): every identity under it becomes `IdentityExpired`.
    pub fn retire_namespace(
        &mut self,
        tenant: TenantId,
        ns: RequestNamespace,
        reason: &str,
    ) -> CoreResult<ProtocolRecordRef> {
        let rk = retirement_key(tenant, ns);
        let rec = NamespaceRetirementV1 {
            tenant_id: tenant,
            request_namespace: ns,
            catalog_generation: self.catalog.generation,
            reason: reason.into(),
        };
        self.store.commit_protocol(ProtocolOnlyBatch {
            internal_record_id: rk.clone(),
            authorizing_evidence: vec![self.grant_ref.clone()],
            writes: vec![ProtocolRecordWrite {
                key: rk.clone(),
                expected: ExpectedRecordRevision::Absent,
                next: versioned(NAMESPACE_RETIREMENT_KIND, &rec)?,
            }],
        })?;
        Ok(CanonicalRecord::wrap(NAMESPACE_RETIREMENT_KIND, &rec)?.reference(rk.logical_key().0))
    }

    /// Administrative bulk load of rows (typed against the schema) as one committed batch under
    /// a synthetic admin request. Used to seed fixtures and experiments; it is not a client path.
    pub fn load_rows(
        &mut self,
        label: &str,
        record_name: &str,
        rows: &[(Value, Vec<(&str, Value)>)],
    ) -> CoreResult<FinalReceiptV1> {
        let rec = self
            .catalog
            .module
            .record_by_name(record_name)
            .ok_or_else(|| CoreError::new(ErrorCode::MissingRecord, record_name.to_string()))?
            .clone();
        let mut mutations = Vec::new();
        for (key, fields) in rows {
            let mut row = BTreeMap::new();
            for (n, v) in fields {
                let f = rec.fields.iter().find(|f| f.name == *n).ok_or_else(|| {
                    CoreError::new(ErrorCode::MissingRecord, format!("field {n}"))
                })?;
                v.check_type(&f.ty)?;
                row.insert(f.id, v.clone());
            }
            let lk = row_key(rec.id, key)?;
            mutations.push(StorageMutation::Put {
                key: lk,
                value: crate::state::encode_row(key, &row),
                semantic_meta: SemanticMeta::default(),
            });
        }
        mutations.sort_by(|a, b| a.key().cmp(b.key()));
        let key = RequestKey {
            tenant_id: TenantId::derive("admin"),
            request_namespace: RequestNamespace::derive("admin-load"),
            stable_request_id: StableRequestId::derive(label),
        };
        let content = CanonValue::obj()
            .fstr("kind", "admin-load")
            .fstr("label", label)
            .fstr("record", record_name)
            .fu64("rows", rows.len() as u64)
            .build();
        let request_hash = RequestHash(domain_hash("astra.request.v1", &content.encode()));
        let plan_ref = PlanRef {
            plan_id: PlanId::derive("admin-load"),
            generation: PlanGeneration(1),
            hash: PlanHash(Hash256::ZERO),
        };
        let (rev, binding) =
            self.home
                .bind_if_absent(&mut self.store, &key, request_hash, plan_ref)?;
        if let Some(r) = &binding.terminal_receipt {
            return Ok(r.clone());
        }
        let op = OperationRef {
            operation_id: OperationId(0),
            version: 0,
        };
        let decision = LocalDecisionV1 {
            txn_id: binding.txn_id,
            request_key: key,
            request_hash,
            outcome: Outcome::Committed,
            invocation_digest: Hash256::ZERO,
            mutation_count: mutations.len() as u64,
        };
        let decision_ref = CanonicalRecord::wrap(LOCAL_DECISION_KIND, &decision)?
            .reference(binding.txn_id.0.to_vec());
        let ar = AcceptedResultV1::new(
            Outcome::Committed,
            RESULT_CODEC,
            Hash256::ZERO,
            content.encode(),
            CanonValue::obj().build(),
            None,
        );
        let receipt = FinalReceiptV1 {
            receipt_version: FinalReceiptV1::VERSION,
            cluster_id: self.cluster_id,
            request_key: key,
            request_hash,
            txn_id: binding.txn_id,
            operation: op,
            operation_hash: OperationHash(Hash256::ZERO),
            schema_hash: self.catalog.hashes.schema_hash,
            contract_hash: ContractHash(Hash256::ZERO),
            plan: plan_ref,
            idc_bindings: vec![],
            origin_ids: vec![],
            outcome: Outcome::Committed,
            result_codec: ar.result_codec.clone(),
            result_type_hash: Hash256::ZERO,
            exact_result_bytes: ar.exact_result_bytes.clone(),
            result_digest: ar.result_digest,
            commitments: ar.commitments.clone(),
            observation_token: None,
            durability_policy: PolicyRef {
                name: LOCAL_DURABILITY_POLICY.into(),
                policy_hash: domain_hash("astra.policy.v1", LOCAL_DURABILITY_POLICY.as_bytes()),
            },
            durability_evidence: vec![self.grant_ref.clone()],
            decision_ref: decision_ref.clone(),
            completion_ref: None,
        };
        let terminal_binding = RequestBindingV1 {
            record_revision: RecordRevision(rev.0 + 1),
            state: BindingState::Terminal,
            admitted_plan: Some(plan_ref),
            decision_authority: Some(self.grant_ref.clone()),
            terminal_receipt: Some(receipt.clone()),
            tombstone: None,
            ..binding.clone()
        };
        let batch = CompiledBatch {
            batch_version: BATCH_VERSION,
            txn_id: binding.txn_id,
            request_key: key,
            request_hash,
            operation: op,
            operation_hash: OperationHash(Hash256::ZERO),
            contract_hash: ContractHash(Hash256::ZERO),
            schema_hash: self.catalog.hashes.schema_hash,
            plan: plan_ref,
            idc_bindings: vec![],
            consistency_class: ConsistencyClass::C0Local,
            origin: None,
            semantic_evidence: vec![],
            captured_inputs: vec![],
            mutations,
            protocol_mutations: vec![
                ProtocolMutation::PutRecord(ProtocolRecordWrite {
                    key: request_binding_record_key(&key),
                    expected: ExpectedRecordRevision::Exact(rev),
                    next: versioned("request_binding", &terminal_binding)?,
                }),
                ProtocolMutation::SetTxnStatus(TxnStatusTransition {
                    txn_id: binding.txn_id,
                    request_key: key,
                    request_hash,
                    expected_revision: ExpectedRecordRevision::Absent,
                    next: TxnStatusRecord {
                        revision: RecordRevision(1),
                        phase: TxnPhase::Terminal,
                        plan: plan_ref,
                        idc_bindings: vec![],
                        prepared_digest: None,
                        decision_ref: Some(decision_ref),
                        accepted_result: Some(ar),
                        terminal_outcome: Some(TerminalOutcome::Committed(receipt.clone())),
                    },
                }),
            ],
            terminal_outcome: Some(TerminalOutcome::Committed(receipt.clone())),
            semantic_digest: SemanticDigest::default(),
        }
        .seal();
        self.store.commit(batch)?;
        Ok(receipt)
    }

    /// Read one row by record name and primary key (field name → value), at the durable snapshot.
    pub fn read_row(
        &mut self,
        record_name: &str,
        key: &Value,
    ) -> CoreResult<Option<BTreeMap<String, Value>>> {
        let rec = self
            .catalog
            .module
            .record_by_name(record_name)
            .ok_or_else(|| CoreError::new(ErrorCode::MissingRecord, record_name.to_string()))?
            .clone();
        let lk = row_key(rec.id, key)?;
        let snap = self.store.snapshot();
        let got = self.store.get(&lk, snap);
        self.store.release_snapshot(snap);
        match got? {
            Some(bytes) => {
                let (_, row) = decode_row(&bytes)?;
                let mut out = BTreeMap::new();
                for (fid, v) in row {
                    if let Some(f) = rec.fields.iter().find(|f| f.id == fid) {
                        out.insert(f.name.clone(), v);
                    }
                }
                Ok(Some(out))
            }
            None => Ok(None),
        }
    }

    /// Every row of a record at the durable snapshot (field name → value), in key order.
    pub fn dump_record(
        &mut self,
        record_name: &str,
    ) -> CoreResult<Vec<(Value, BTreeMap<String, Value>)>> {
        let rec = self
            .catalog
            .module
            .record_by_name(record_name)
            .ok_or_else(|| CoreError::new(ErrorCode::MissingRecord, record_name.to_string()))?
            .clone();
        let snap = self.store.snapshot();
        let st = load_records(&mut self.store, snap, &[rec.id]);
        self.store.release_snapshot(snap);
        let st = st?;
        let mut out = Vec::new();
        if let Some(t) = st.rows.get(&rec.id) {
            for (k, row) in t {
                let mut named = BTreeMap::new();
                for (fid, v) in row {
                    if let Some(f) = rec.fields.iter().find(|f| f.id == *fid) {
                        named.insert(f.name.clone(), v.clone());
                    }
                }
                out.push((k.clone(), named));
            }
        }
        Ok(out)
    }

    /// Number of rows of a record at the durable snapshot.
    pub fn count_rows(&mut self, record_name: &str) -> CoreResult<usize> {
        let rec = self
            .catalog
            .module
            .record_by_name(record_name)
            .ok_or_else(|| CoreError::new(ErrorCode::MissingRecord, record_name.to_string()))?
            .id;
        let snap = self.store.snapshot();
        let st = load_records(&mut self.store, snap, &[rec]);
        self.store.release_snapshot(snap);
        Ok(st?.rows.get(&rec).map(|t| t.len()).unwrap_or(0))
    }
}

/// Build a client request for `operation` of `catalog` with the given typed arguments.
pub fn make_invoke(
    catalog: &LocalCatalog,
    tenant: &str,
    stable_request_id: &str,
    operation: &str,
    args: Vec<Value>,
) -> CoreResult<InvokeV1> {
    let op = catalog
        .module
        .operation_by_name(operation)
        .ok_or_else(|| CoreError::new(ErrorCode::MissingRecord, operation.to_string()))?;
    let plan = catalog
        .plan_for(op.identity)
        .ok_or_else(|| CoreError::new(ErrorCode::NoSafePlan, operation.to_string()))?;
    let content = RequestContentV1 {
        request_key: RequestKey {
            tenant_id: TenantId::derive(tenant),
            request_namespace: RequestNamespace::derive(&op.contract.request_namespace),
            stable_request_id: StableRequestId::derive(stable_request_id),
        },
        operation: op.identity,
        operation_hash: plan.operation_hash,
        schema_hash: catalog.hashes.schema_hash,
        contract_hash: plan.contract_hash,
        arguments: Value::Tuple(args).to_canon(),
        read_contract: ReadContractV1::operation_default(Visibility::Serial),
        initial_session: None,
    };
    let request_hash = content.request_hash();
    Ok(InvokeV1 {
        content,
        request_hash,
        attempt_id: AttemptId::derive(&format!("{tenant}/{stable_request_id}/attempt")),
        deadline_budget_ms: 1_000,
        route_hint: None,
        accepted_result_codecs: vec![RESULT_CODEC.into()],
    })
}
