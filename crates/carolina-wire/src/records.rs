//! Client protocol and receipt records (SPEC-012 §2–§7).
//!
//! Every record is a canonical object; the registered wrapper
//! `{"body":…, "record_kind":…, "record_version":"1"}` is applied by [`crate::registry`].
//! Hash domains: `astra.request.v1` (RequestContentV1 body), `astra.result.v1`
//! (result triple), `astra.receipt.v1` (FinalReceiptV1 body). Construction is acyclic:
//! accepted payload → decision reference → publication/completion → final receipt.

use carolina_core::canon::{decode_set, CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, domains, Hash256};
use carolina_core::ids::*;

// ---------------------------------------------------------------------------
// Read contracts and observation tokens (§6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Visibility {
    LocalSnapshot,
    Causal,
    Certified,
    Serial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SessionGuarantee {
    ReadYourWrites,
    MonotonicReads,
    CausalDependencies,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionScope {
    None,
    ReplicationGroup(ReplicationGroupId),
    CompositeScope(Vec<RecordId>),
}

/// Canonical scope of a read: record ids (empty = operation-defined scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadContractV1 {
    pub visibility: Visibility,
    pub scope: Vec<RecordId>,
    pub session_guarantees: Vec<SessionGuarantee>,
    pub session_scope: SessionScope,
}

impl ReadContractV1 {
    /// The default contract of an operation invocation: the plan's own visibility, no session.
    pub fn operation_default(visibility: Visibility) -> Self {
        ReadContractV1 {
            visibility,
            scope: vec![],
            session_guarantees: vec![],
            session_scope: SessionScope::None,
        }
    }
}

fn sg_label(g: SessionGuarantee) -> &'static str {
    match g {
        SessionGuarantee::ReadYourWrites => "ReadYourWrites",
        SessionGuarantee::MonotonicReads => "MonotonicReads",
        SessionGuarantee::CausalDependencies => "CausalDependencies",
    }
}

impl Canonical for ReadContractV1 {
    fn to_canon(&self) -> CanonValue {
        let ss = match &self.session_scope {
            SessionScope::None => CanonValue::obj().fstr("kind", "none").build(),
            SessionScope::ReplicationGroup(g) => CanonValue::obj()
                .fc("group", g)
                .fstr("kind", "replication_group")
                .build(),
            SessionScope::CompositeScope(r) => CanonValue::obj()
                .fstr("kind", "composite_scope")
                .fset("records", r)
                .build(),
        };
        CanonValue::obj()
            .fset("scope", &self.scope)
            .f(
                "session_guarantees",
                CanonValue::set(
                    self.session_guarantees
                        .iter()
                        .map(|g| CanonValue::str(sg_label(*g)))
                        .collect(),
                )
                .expect("unique"),
            )
            .f("session_scope", ss)
            .fstr(
                "visibility",
                match self.visibility {
                    Visibility::LocalSnapshot => "LocalSnapshot",
                    Visibility::Causal => "Causal",
                    Visibility::Certified => "Certified",
                    Visibility::Serial => "Serial",
                },
            )
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["scope", "session_guarantees", "session_scope", "visibility"])?;
        let visibility = match v.field("visibility")?.as_str()? {
            "LocalSnapshot" => Visibility::LocalSnapshot,
            "Causal" => Visibility::Causal,
            "Certified" => Visibility::Certified,
            "Serial" => Visibility::Serial,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("visibility {k}"),
                ))
            }
        };
        let mut session_guarantees = Vec::new();
        for s in decode_set::<String>(v.field("session_guarantees")?)? {
            session_guarantees.push(match s.as_str() {
                "ReadYourWrites" => SessionGuarantee::ReadYourWrites,
                "MonotonicReads" => SessionGuarantee::MonotonicReads,
                "CausalDependencies" => SessionGuarantee::CausalDependencies,
                k => {
                    return Err(CoreError::new(
                        ErrorCode::NonCanonicalEncoding,
                        format!("session guarantee {k}"),
                    ))
                }
            });
        }
        let ss = v.field("session_scope")?;
        let session_scope = match ss.field("kind")?.as_str()? {
            "none" => SessionScope::None,
            "replication_group" => {
                SessionScope::ReplicationGroup(ReplicationGroupId::from_canon(ss.field("group")?)?)
            }
            "composite_scope" => SessionScope::CompositeScope(decode_set(ss.field("records")?)?),
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("session scope {k}"),
                ))
            }
        };
        Ok(ReadContractV1 {
            visibility,
            scope: decode_set(v.field("scope")?)?,
            session_guarantees,
            session_scope,
        })
    }
}

/// Client-visible observation token wrapper (§6). The group payload (`SessionTokenV1`) is SPEC-005's;
/// v1 stores it as opaque canonical bytes plus scope binding. Certified/serial evidence carry the
/// authority frontier they were captured at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationTokenV1 {
    Group {
        group: ReplicationGroupId,
        membership_generation: MembershipGeneration,
        payload: Vec<u8>,
    },
    Certified {
        authority: AuthorityId,
        idc_bindings: Vec<IdcBinding>,
        frontier: SerialPosition,
    },
    Serial {
        authority: AuthorityId,
        idc_binding: IdcBinding,
        position: SerialPosition,
    },
    Composite {
        payload: Vec<u8>,
    },
}

impl ObservationTokenV1 {
    pub fn kind(&self) -> &'static str {
        match self {
            ObservationTokenV1::Group { .. } => "group",
            ObservationTokenV1::Certified { .. } => "certified",
            ObservationTokenV1::Serial { .. } => "serial",
            ObservationTokenV1::Composite { .. } => "composite",
        }
    }
}

impl Canonical for ObservationTokenV1 {
    fn to_canon(&self) -> CanonValue {
        match self {
            ObservationTokenV1::Group {
                group,
                membership_generation,
                payload,
            } => CanonValue::obj()
                .fc("group", group)
                .fstr("kind", "group")
                .fc("membership_generation", membership_generation)
                .fbytes("payload", payload)
                .build(),
            ObservationTokenV1::Certified {
                authority,
                idc_bindings,
                frontier,
            } => CanonValue::obj()
                .fc("authority", authority)
                .fc("frontier", frontier)
                .fset("idc_bindings", idc_bindings)
                .fstr("kind", "certified")
                .build(),
            ObservationTokenV1::Serial {
                authority,
                idc_binding,
                position,
            } => CanonValue::obj()
                .fc("authority", authority)
                .fc("idc_binding", idc_binding)
                .fstr("kind", "serial")
                .fc("position", position)
                .build(),
            ObservationTokenV1::Composite { payload } => CanonValue::obj()
                .fstr("kind", "composite")
                .fbytes("payload", payload)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "group" => {
                v.expect_fields(&["group", "kind", "membership_generation", "payload"])?;
                ObservationTokenV1::Group {
                    group: ReplicationGroupId::from_canon(v.field("group")?)?,
                    membership_generation: MembershipGeneration::from_canon(
                        v.field("membership_generation")?,
                    )?,
                    payload: v.field("payload")?.as_bytes()?,
                }
            }
            "certified" => {
                v.expect_fields(&["authority", "frontier", "idc_bindings", "kind"])?;
                ObservationTokenV1::Certified {
                    authority: AuthorityId::from_canon(v.field("authority")?)?,
                    idc_bindings: decode_set(v.field("idc_bindings")?)?,
                    frontier: SerialPosition::from_canon(v.field("frontier")?)?,
                }
            }
            "serial" => {
                v.expect_fields(&["authority", "idc_binding", "kind", "position"])?;
                ObservationTokenV1::Serial {
                    authority: AuthorityId::from_canon(v.field("authority")?)?,
                    idc_binding: IdcBinding::from_canon(v.field("idc_binding")?)?,
                    position: SerialPosition::from_canon(v.field("position")?)?,
                }
            }
            "composite" => {
                v.expect_fields(&["kind", "payload"])?;
                ObservationTokenV1::Composite {
                    payload: v.field("payload")?.as_bytes()?,
                }
            }
            k => Err(CoreError::new(
                ErrorCode::UnsupportedSessionScope,
                format!("unsupported token family `{k}`"),
            ))?,
        })
    }
}

// ---------------------------------------------------------------------------
// Request content and identity (§2)
// ---------------------------------------------------------------------------

/// `RequestContentV1`: the complete logical invocation content that `RequestHash` covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestContentV1 {
    pub request_key: RequestKey,
    pub operation: OperationRef,
    pub operation_hash: OperationHash,
    pub schema_hash: SchemaHash,
    pub contract_hash: ContractHash,
    /// Canonical typed value of the argument tuple (`carolina_lang::types::Value` canonical form).
    pub arguments: CanonValue,
    pub read_contract: ReadContractV1,
    pub initial_session: Option<ObservationTokenV1>,
}

impl RequestContentV1 {
    /// `RequestHash = SHA-256(UTF8("astra.request.v1") || 0x00 || canonical(RequestContentV1))`.
    pub fn request_hash(&self) -> RequestHash {
        RequestHash(domain_hash(domains::REQUEST_V1, &self.encode()))
    }
}

impl Canonical for RequestContentV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .f("arguments", self.arguments.clone())
            .fc("contract_hash", &self.contract_hash)
            .fopt("initial_session", &self.initial_session)
            .fc("operation", &self.operation)
            .fc("operation_hash", &self.operation_hash)
            .fc("read_contract", &self.read_contract)
            .fc("request_key", &self.request_key)
            .fc("schema_hash", &self.schema_hash)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "arguments",
            "contract_hash",
            "initial_session",
            "operation",
            "operation_hash",
            "read_contract",
            "request_key",
            "schema_hash",
        ])?;
        Ok(RequestContentV1 {
            request_key: RequestKey::from_canon(v.field("request_key")?)?,
            operation: OperationRef::from_canon(v.field("operation")?)?,
            operation_hash: OperationHash::from_canon(v.field("operation_hash")?)?,
            schema_hash: SchemaHash::from_canon(v.field("schema_hash")?)?,
            contract_hash: ContractHash::from_canon(v.field("contract_hash")?)?,
            arguments: v.field("arguments")?.clone(),
            read_contract: ReadContractV1::from_canon(v.field("read_contract")?)?,
            initial_session: Option::from_canon(v.field("initial_session")?)?,
        })
    }
}

/// Route hint: verifiable route reference, never permission to allocate locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHint {
    pub home: RequestHomeId,
    pub epoch: RequestHomeEpoch,
}

impl Canonical for RouteHint {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("epoch", &self.epoch)
            .fc("home", &self.home)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["epoch", "home"])?;
        Ok(RouteHint {
            home: RequestHomeId::from_canon(v.field("home")?)?,
            epoch: RequestHomeEpoch::from_canon(v.field("epoch")?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvokeV1 {
    pub content: RequestContentV1,
    pub request_hash: RequestHash,
    pub attempt_id: AttemptId,
    pub deadline_budget_ms: u64,
    pub route_hint: Option<RouteHint>,
    pub accepted_result_codecs: Vec<String>,
}

impl InvokeV1 {
    /// Servers recompute the digest; a client-supplied hash never replaces the check.
    pub fn verify_hash(&self) -> CoreResult<()> {
        if self.content.request_hash() != self.request_hash {
            return Err(CoreError::new(
                ErrorCode::RequestIdentityMismatch,
                "client request_hash does not match canonical content",
            ));
        }
        Ok(())
    }
}

impl Canonical for InvokeV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fset("accepted_result_codecs", &self.accepted_result_codecs)
            .fc("attempt_id", &self.attempt_id)
            .fc("content", &self.content)
            .fu64("deadline_budget_ms", self.deadline_budget_ms)
            .fc("request_hash", &self.request_hash)
            .fopt("route_hint", &self.route_hint)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "accepted_result_codecs",
            "attempt_id",
            "content",
            "deadline_budget_ms",
            "request_hash",
            "route_hint",
        ])?;
        Ok(InvokeV1 {
            content: RequestContentV1::from_canon(v.field("content")?)?,
            request_hash: RequestHash::from_canon(v.field("request_hash")?)?,
            attempt_id: AttemptId::from_canon(v.field("attempt_id")?)?,
            deadline_budget_ms: v.field("deadline_budget_ms")?.as_u64()?,
            route_hint: Option::from_canon(v.field("route_hint")?)?,
            accepted_result_codecs: decode_set(v.field("accepted_result_codecs")?)?,
        })
    }
}

// ---------------------------------------------------------------------------
// Results and receipts (§7)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    Committed,
    Rejected,
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Committed => "COMMITTED",
            Outcome::Rejected => "REJECTED",
        }
    }
    pub fn from_label(s: &str) -> CoreResult<Self> {
        match s {
            "COMMITTED" => Ok(Outcome::Committed),
            "REJECTED" => Ok(Outcome::Rejected),
            k => Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                format!("outcome {k}"),
            )),
        }
    }
}

/// `result_digest = SHA-256("astra.result.v1" || 0 || canonical({result_codec,result_type_hash,exact_result_bytes}))`.
pub fn result_digest(
    result_codec: &str,
    result_type_hash: &Hash256,
    exact_result_bytes: &[u8],
) -> ResultDigest {
    let payload = CanonValue::obj()
        .fbytes("exact_result_bytes", exact_result_bytes)
        .fstr("result_codec", result_codec)
        .fc("result_type_hash", result_type_hash)
        .build();
    ResultDigest(domain_hash(domains::RESULT_V1, &payload.encode()))
}

/// Result material fixed at the unique decision boundary. It references nothing later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedResultV1 {
    pub outcome: Outcome,
    pub result_codec: String,
    pub result_type_hash: Hash256,
    pub exact_result_bytes: Vec<u8>,
    pub result_digest: ResultDigest,
    /// Canonical typed value of the confirmed commitments (operation-versioned meaning).
    pub commitments: CanonValue,
    pub accepted_observation: Option<ObservationTokenV1>,
}

impl AcceptedResultV1 {
    pub fn new(
        outcome: Outcome,
        result_codec: &str,
        result_type_hash: Hash256,
        exact_result_bytes: Vec<u8>,
        commitments: CanonValue,
        accepted_observation: Option<ObservationTokenV1>,
    ) -> Self {
        let result_digest = result_digest(result_codec, &result_type_hash, &exact_result_bytes);
        AcceptedResultV1 {
            outcome,
            result_codec: result_codec.into(),
            result_type_hash,
            exact_result_bytes,
            result_digest,
            commitments,
            accepted_observation,
        }
    }
    pub fn verify_digest(&self) -> CoreResult<()> {
        if result_digest(
            &self.result_codec,
            &self.result_type_hash,
            &self.exact_result_bytes,
        ) != self.result_digest
        {
            return Err(CoreError::new(
                ErrorCode::Corruption,
                "result digest mismatch",
            ));
        }
        Ok(())
    }
}

impl Canonical for AcceptedResultV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fopt("accepted_observation", &self.accepted_observation)
            .f("commitments", self.commitments.clone())
            .fbytes("exact_result_bytes", &self.exact_result_bytes)
            .fstr("outcome", self.outcome.label())
            .fstr("result_codec", &self.result_codec)
            .fc("result_digest", &self.result_digest)
            .fc("result_type_hash", &self.result_type_hash)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "accepted_observation",
            "commitments",
            "exact_result_bytes",
            "outcome",
            "result_codec",
            "result_digest",
            "result_type_hash",
        ])?;
        let r = AcceptedResultV1 {
            outcome: Outcome::from_label(v.field("outcome")?.as_str()?)?,
            result_codec: v.field("result_codec")?.as_str()?.to_string(),
            result_type_hash: Hash256::from_canon(v.field("result_type_hash")?)?,
            exact_result_bytes: v.field("exact_result_bytes")?.as_bytes()?,
            result_digest: ResultDigest::from_canon(v.field("result_digest")?)?,
            commitments: v.field("commitments")?.clone(),
            accepted_observation: Option::from_canon(v.field("accepted_observation")?)?,
        };
        r.verify_digest()?;
        Ok(r)
    }
}

/// Durability policy reference carried by receipts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRef {
    pub name: String,
    pub policy_hash: Hash256,
}

impl Canonical for PolicyRef {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("name", &self.name)
            .fc("policy_hash", &self.policy_hash)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["name", "policy_hash"])?;
        Ok(PolicyRef {
            name: v.field("name")?.as_str()?.to_string(),
            policy_hash: Hash256::from_canon(v.field("policy_hash")?)?,
        })
    }
}

/// The canonical exact final receipt (§7). Byte-identical on every retry, recovery and migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalReceiptV1 {
    pub receipt_version: u32,
    pub cluster_id: ClusterId,
    pub request_key: RequestKey,
    pub request_hash: RequestHash,
    pub txn_id: TxnId,
    pub operation: OperationRef,
    pub operation_hash: OperationHash,
    pub schema_hash: SchemaHash,
    pub contract_hash: ContractHash,
    pub plan: PlanRef,
    pub idc_bindings: Vec<IdcBinding>,
    pub origin_ids: Vec<OriginId>,
    pub outcome: Outcome,
    pub result_codec: String,
    pub result_type_hash: Hash256,
    pub exact_result_bytes: Vec<u8>,
    pub result_digest: ResultDigest,
    pub commitments: CanonValue,
    pub observation_token: Option<ObservationTokenV1>,
    pub durability_policy: PolicyRef,
    pub durability_evidence: Vec<ProtocolRecordRef>,
    pub decision_ref: ProtocolRecordRef,
    pub completion_ref: Option<ProtocolRecordRef>,
}

impl FinalReceiptV1 {
    pub const VERSION: u32 = 1;

    /// `ReceiptDigest` hashes the whole receipt body under `astra.receipt.v1`.
    pub fn receipt_digest(&self) -> ReceiptDigest {
        ReceiptDigest(domain_hash(domains::RECEIPT_V1, &self.encode()))
    }

    pub fn accepted_result(&self) -> AcceptedResultV1 {
        AcceptedResultV1 {
            outcome: self.outcome,
            result_codec: self.result_codec.clone(),
            result_type_hash: self.result_type_hash,
            exact_result_bytes: self.exact_result_bytes.clone(),
            result_digest: self.result_digest,
            commitments: self.commitments.clone(),
            accepted_observation: self.observation_token.clone(),
        }
    }

    pub fn verify(&self) -> CoreResult<()> {
        if self.receipt_version != Self::VERSION {
            return Err(CoreError::new(
                ErrorCode::UnsupportedCodec,
                "unsupported receipt version",
            ));
        }
        self.accepted_result().verify_digest()
    }
}

impl Canonical for FinalReceiptV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("cluster_id", &self.cluster_id)
            .f("commitments", self.commitments.clone())
            .fopt("completion_ref", &self.completion_ref)
            .fc("contract_hash", &self.contract_hash)
            .fc("decision_ref", &self.decision_ref)
            .fset("durability_evidence", &self.durability_evidence)
            .fc("durability_policy", &self.durability_policy)
            .fbytes("exact_result_bytes", &self.exact_result_bytes)
            .fset("idc_bindings", &self.idc_bindings)
            .fc("operation", &self.operation)
            .fc("operation_hash", &self.operation_hash)
            .fopt("observation_token", &self.observation_token)
            .fset("origin_ids", &self.origin_ids)
            .fstr("outcome", self.outcome.label())
            .fc("plan", &self.plan)
            .fu32("receipt_version", self.receipt_version)
            .fc("request_hash", &self.request_hash)
            .fc("request_key", &self.request_key)
            .fstr("result_codec", &self.result_codec)
            .fc("result_digest", &self.result_digest)
            .fc("result_type_hash", &self.result_type_hash)
            .fc("schema_hash", &self.schema_hash)
            .fc("txn_id", &self.txn_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "cluster_id",
            "commitments",
            "completion_ref",
            "contract_hash",
            "decision_ref",
            "durability_evidence",
            "durability_policy",
            "exact_result_bytes",
            "idc_bindings",
            "operation",
            "operation_hash",
            "observation_token",
            "origin_ids",
            "outcome",
            "plan",
            "receipt_version",
            "request_hash",
            "request_key",
            "result_codec",
            "result_digest",
            "result_type_hash",
            "schema_hash",
            "txn_id",
        ])?;
        let r = FinalReceiptV1 {
            receipt_version: v.field("receipt_version")?.as_u32()?,
            cluster_id: ClusterId::from_canon(v.field("cluster_id")?)?,
            request_key: RequestKey::from_canon(v.field("request_key")?)?,
            request_hash: RequestHash::from_canon(v.field("request_hash")?)?,
            txn_id: TxnId::from_canon(v.field("txn_id")?)?,
            operation: OperationRef::from_canon(v.field("operation")?)?,
            operation_hash: OperationHash::from_canon(v.field("operation_hash")?)?,
            schema_hash: SchemaHash::from_canon(v.field("schema_hash")?)?,
            contract_hash: ContractHash::from_canon(v.field("contract_hash")?)?,
            plan: PlanRef::from_canon(v.field("plan")?)?,
            idc_bindings: decode_set(v.field("idc_bindings")?)?,
            origin_ids: decode_set(v.field("origin_ids")?)?,
            outcome: Outcome::from_label(v.field("outcome")?.as_str()?)?,
            result_codec: v.field("result_codec")?.as_str()?.to_string(),
            result_type_hash: Hash256::from_canon(v.field("result_type_hash")?)?,
            exact_result_bytes: v.field("exact_result_bytes")?.as_bytes()?,
            result_digest: ResultDigest::from_canon(v.field("result_digest")?)?,
            commitments: v.field("commitments")?.clone(),
            observation_token: Option::from_canon(v.field("observation_token")?)?,
            durability_policy: PolicyRef::from_canon(v.field("durability_policy")?)?,
            durability_evidence: decode_set(v.field("durability_evidence")?)?,
            decision_ref: ProtocolRecordRef::from_canon(v.field("decision_ref")?)?,
            completion_ref: Option::from_canon(v.field("completion_ref")?)?,
        };
        r.verify()?;
        Ok(r)
    }
}

// ---------------------------------------------------------------------------
// Request binding (§3) and public replies (§4, §5)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BindingState {
    Bound,
    Admitted,
    Terminal,
    ResultExpired,
}

impl BindingState {
    pub fn label(&self) -> &'static str {
        match self {
            BindingState::Bound => "BOUND",
            BindingState::Admitted => "ADMITTED",
            BindingState::Terminal => "TERMINAL",
            BindingState::ResultExpired => "RESULT_EXPIRED",
        }
    }
    pub fn from_label(s: &str) -> CoreResult<Self> {
        Ok(match s {
            "BOUND" => BindingState::Bound,
            "ADMITTED" => BindingState::Admitted,
            "TERMINAL" => BindingState::Terminal,
            "RESULT_EXPIRED" => BindingState::ResultExpired,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("binding state {k}"),
                ))
            }
        })
    }
    /// Legal forward transitions only (SPEC-002 §42: a retry never rewinds a phase).
    pub fn can_advance_to(&self, next: BindingState) -> bool {
        matches!(
            (self, next),
            (BindingState::Bound, BindingState::Admitted)
                | (BindingState::Bound, BindingState::Terminal)
                | (BindingState::Admitted, BindingState::Terminal)
                | (BindingState::Terminal, BindingState::ResultExpired)
        )
    }
}

/// Retained evidence after exact result bytes are evicted (§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultTombstoneV1 {
    pub request_key: RequestKey,
    pub request_hash: RequestHash,
    pub txn_id: TxnId,
    pub outcome: Outcome,
    pub receipt_digest: ReceiptDigest,
    pub decision_ref: ProtocolRecordRef,
}

impl Canonical for ResultTombstoneV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("decision_ref", &self.decision_ref)
            .fstr("outcome", self.outcome.label())
            .fc("receipt_digest", &self.receipt_digest)
            .fc("request_hash", &self.request_hash)
            .fc("request_key", &self.request_key)
            .fc("txn_id", &self.txn_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "decision_ref",
            "outcome",
            "receipt_digest",
            "request_hash",
            "request_key",
            "txn_id",
        ])?;
        Ok(ResultTombstoneV1 {
            request_key: RequestKey::from_canon(v.field("request_key")?)?,
            request_hash: RequestHash::from_canon(v.field("request_hash")?)?,
            txn_id: TxnId::from_canon(v.field("txn_id")?)?,
            outcome: Outcome::from_label(v.field("outcome")?.as_str()?)?,
            receipt_digest: ReceiptDigest::from_canon(v.field("receipt_digest")?)?,
            decision_ref: ProtocolRecordRef::from_canon(v.field("decision_ref")?)?,
        })
    }
}

/// The durable RequestHome binding (§3). `txn_id` is the exact concatenation of home, epoch and seq.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestBindingV1 {
    pub request_key: RequestKey,
    pub request_hash: RequestHash,
    pub txn_id: TxnId,
    pub allocation_home: RequestHomeId,
    pub allocation_epoch: RequestHomeEpoch,
    pub allocation_seq: RequestAllocationSeq,
    pub record_revision: RecordRevision,
    pub state: BindingState,
    pub admitted_plan: Option<PlanRef>,
    pub decision_authority: Option<ProtocolRecordRef>,
    pub terminal_receipt: Option<FinalReceiptV1>,
    pub tombstone: Option<ResultTombstoneV1>,
}

impl RequestBindingV1 {
    pub fn verify(&self) -> CoreResult<()> {
        let expected = TxnId::new(
            self.allocation_home,
            self.allocation_epoch,
            self.allocation_seq,
        );
        if expected != self.txn_id {
            return Err(CoreError::new(
                ErrorCode::Corruption,
                "TxnId does not equal home||epoch||seq",
            ));
        }
        match self.state {
            BindingState::Terminal if self.terminal_receipt.is_none() => Err(CoreError::new(
                ErrorCode::Corruption,
                "TERMINAL binding without receipt",
            )),
            BindingState::ResultExpired if self.tombstone.is_none() => Err(CoreError::new(
                ErrorCode::Corruption,
                "RESULT_EXPIRED binding without tombstone",
            )),
            BindingState::Bound | BindingState::Admitted if self.terminal_receipt.is_some() => {
                Err(CoreError::new(
                    ErrorCode::Corruption,
                    "non-terminal binding carries a receipt",
                ))
            }
            _ => Ok(()),
        }
    }
}

impl Canonical for RequestBindingV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fopt("admitted_plan", &self.admitted_plan)
            .fc("allocation_epoch", &self.allocation_epoch)
            .fc("allocation_home", &self.allocation_home)
            .fc("allocation_seq", &self.allocation_seq)
            .fopt("decision_authority", &self.decision_authority)
            .fc("record_revision", &self.record_revision)
            .fc("request_hash", &self.request_hash)
            .fc("request_key", &self.request_key)
            .fstr("state", self.state.label())
            .fopt("terminal_receipt", &self.terminal_receipt)
            .fopt("tombstone", &self.tombstone)
            .fc("txn_id", &self.txn_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "admitted_plan",
            "allocation_epoch",
            "allocation_home",
            "allocation_seq",
            "decision_authority",
            "record_revision",
            "request_hash",
            "request_key",
            "state",
            "terminal_receipt",
            "tombstone",
            "txn_id",
        ])?;
        let b = RequestBindingV1 {
            request_key: RequestKey::from_canon(v.field("request_key")?)?,
            request_hash: RequestHash::from_canon(v.field("request_hash")?)?,
            txn_id: TxnId::from_canon(v.field("txn_id")?)?,
            allocation_home: RequestHomeId::from_canon(v.field("allocation_home")?)?,
            allocation_epoch: RequestHomeEpoch::from_canon(v.field("allocation_epoch")?)?,
            allocation_seq: RequestAllocationSeq::from_canon(v.field("allocation_seq")?)?,
            record_revision: RecordRevision::from_canon(v.field("record_revision")?)?,
            state: BindingState::from_label(v.field("state")?.as_str()?)?,
            admitted_plan: Option::from_canon(v.field("admitted_plan")?)?,
            decision_authority: Option::from_canon(v.field("decision_authority")?)?,
            terminal_receipt: Option::from_canon(v.field("terminal_receipt")?)?,
            tombstone: Option::from_canon(v.field("tombstone")?)?,
        };
        b.verify()?;
        Ok(b)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusalV1 {
    pub code: String,
    pub detail: String,
    /// True when an execution may already have been admitted (the client must resolve, not re-invoke).
    pub possibly_admitted: bool,
}

impl Canonical for RefusalV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("code", &self.code)
            .fstr("detail", &self.detail)
            .fbool("possibly_admitted", self.possibly_admitted)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["code", "detail", "possibly_admitted"])?;
        Ok(RefusalV1 {
            code: v.field("code")?.as_str()?.to_string(),
            detail: v.field("detail")?.as_str()?.to_string(),
            possibly_admitted: v.field("possibly_admitted")?.as_bool()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionHintV1 {
    pub request_key: RequestKey,
    pub request_hash: RequestHash,
    pub txn_id: Option<TxnId>,
    pub resolver: Option<RouteHint>,
}

impl Canonical for ResolutionHintV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("request_hash", &self.request_hash)
            .fc("request_key", &self.request_key)
            .fopt("resolver", &self.resolver)
            .fopt("txn_id", &self.txn_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["request_hash", "request_key", "resolver", "txn_id"])?;
        Ok(ResolutionHintV1 {
            request_key: RequestKey::from_canon(v.field("request_key")?)?,
            request_hash: RequestHash::from_canon(v.field("request_hash")?)?,
            txn_id: Option::from_canon(v.field("txn_id")?)?,
            resolver: Option::from_canon(v.field("resolver")?)?,
        })
    }
}

/// Public outcomes (§4). `OutcomeUnknown` is a client knowledge state, never a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientReplyV1 {
    Committed(FinalReceiptV1),
    Rejected(FinalReceiptV1),
    Unavailable(RefusalV1),
    OutcomeUnknown(ResolutionHintV1),
    RequestIdentityMismatch,
    ResultExpired(ResultTombstoneV1),
    IdentityExpired {
        namespace: RequestNamespace,
        retirement_ref: ProtocolRecordRef,
    },
    ProtocolError {
        code: String,
        detail: String,
    },
}

impl ClientReplyV1 {
    pub fn kind(&self) -> &'static str {
        match self {
            ClientReplyV1::Committed(_) => "Committed",
            ClientReplyV1::Rejected(_) => "Rejected",
            ClientReplyV1::Unavailable(_) => "Unavailable",
            ClientReplyV1::OutcomeUnknown(_) => "OutcomeUnknown",
            ClientReplyV1::RequestIdentityMismatch => "RequestIdentityMismatch",
            ClientReplyV1::ResultExpired(_) => "ResultExpired",
            ClientReplyV1::IdentityExpired { .. } => "IdentityExpired",
            ClientReplyV1::ProtocolError { .. } => "ProtocolError",
        }
    }
    /// Whether the client may re-send the same RequestKey/content (never a new identity).
    pub fn retry_same_identity(&self) -> bool {
        matches!(
            self,
            ClientReplyV1::Unavailable(_) | ClientReplyV1::OutcomeUnknown(_)
        )
    }
    pub fn receipt(&self) -> Option<&FinalReceiptV1> {
        match self {
            ClientReplyV1::Committed(r) | ClientReplyV1::Rejected(r) => Some(r),
            _ => None,
        }
    }
}

impl Canonical for ClientReplyV1 {
    fn to_canon(&self) -> CanonValue {
        match self {
            ClientReplyV1::Committed(r) => CanonValue::obj()
                .fstr("kind", "Committed")
                .fc("receipt", r)
                .build(),
            ClientReplyV1::Rejected(r) => CanonValue::obj()
                .fstr("kind", "Rejected")
                .fc("receipt", r)
                .build(),
            ClientReplyV1::Unavailable(x) => CanonValue::obj()
                .fstr("kind", "Unavailable")
                .fc("refusal", x)
                .build(),
            ClientReplyV1::OutcomeUnknown(h) => CanonValue::obj()
                .fc("hint", h)
                .fstr("kind", "OutcomeUnknown")
                .build(),
            ClientReplyV1::RequestIdentityMismatch => CanonValue::obj()
                .fstr("kind", "RequestIdentityMismatch")
                .build(),
            ClientReplyV1::ResultExpired(t) => CanonValue::obj()
                .fstr("kind", "ResultExpired")
                .fc("tombstone", t)
                .build(),
            ClientReplyV1::IdentityExpired {
                namespace,
                retirement_ref,
            } => CanonValue::obj()
                .fstr("kind", "IdentityExpired")
                .fc("namespace", namespace)
                .fc("retirement_ref", retirement_ref)
                .build(),
            ClientReplyV1::ProtocolError { code, detail } => CanonValue::obj()
                .fstr("code", code)
                .fstr("detail", detail)
                .fstr("kind", "ProtocolError")
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "Committed" => {
                v.expect_fields(&["kind", "receipt"])?;
                ClientReplyV1::Committed(FinalReceiptV1::from_canon(v.field("receipt")?)?)
            }
            "Rejected" => {
                v.expect_fields(&["kind", "receipt"])?;
                ClientReplyV1::Rejected(FinalReceiptV1::from_canon(v.field("receipt")?)?)
            }
            "Unavailable" => {
                v.expect_fields(&["kind", "refusal"])?;
                ClientReplyV1::Unavailable(RefusalV1::from_canon(v.field("refusal")?)?)
            }
            "OutcomeUnknown" => {
                v.expect_fields(&["hint", "kind"])?;
                ClientReplyV1::OutcomeUnknown(ResolutionHintV1::from_canon(v.field("hint")?)?)
            }
            "RequestIdentityMismatch" => {
                v.expect_fields(&["kind"])?;
                ClientReplyV1::RequestIdentityMismatch
            }
            "ResultExpired" => {
                v.expect_fields(&["kind", "tombstone"])?;
                ClientReplyV1::ResultExpired(ResultTombstoneV1::from_canon(v.field("tombstone")?)?)
            }
            "IdentityExpired" => {
                v.expect_fields(&["kind", "namespace", "retirement_ref"])?;
                ClientReplyV1::IdentityExpired {
                    namespace: RequestNamespace::from_canon(v.field("namespace")?)?,
                    retirement_ref: ProtocolRecordRef::from_canon(v.field("retirement_ref")?)?,
                }
            }
            "ProtocolError" => {
                v.expect_fields(&["code", "detail", "kind"])?;
                ClientReplyV1::ProtocolError {
                    code: v.field("code")?.as_str()?.to_string(),
                    detail: v.field("detail")?.as_str()?.to_string(),
                }
            }
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("reply kind {k}"),
                ))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveRequestV1 {
    pub request_key: RequestKey,
    pub expected_request_hash: RequestHash,
}

impl Canonical for ResolveRequestV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("expected_request_hash", &self.expected_request_hash)
            .fc("request_key", &self.request_key)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["expected_request_hash", "request_key"])?;
        Ok(ResolveRequestV1 {
            request_key: RequestKey::from_canon(v.field("request_key")?)?,
            expected_request_hash: RequestHash::from_canon(v.field("expected_request_hash")?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveReplyV1 {
    Terminal(Box<ClientReplyV1>),
    Pending {
        txn_id: TxnId,
        phase: String,
        original_plan: PlanRef,
        resolver_ref: ProtocolRecordRef,
    },
    AbsentAtBarrier {
        catalog_generation: CatalogGeneration,
        home: RequestHomeId,
        home_epoch: RequestHomeEpoch,
        record_revision: RecordRevision,
    },
    Unavailable(RefusalV1),
}

impl Canonical for ResolveReplyV1 {
    fn to_canon(&self) -> CanonValue {
        match self {
            ResolveReplyV1::Terminal(r) => CanonValue::obj()
                .fstr("kind", "Terminal")
                .fc("reply", r.as_ref())
                .build(),
            ResolveReplyV1::Pending {
                txn_id,
                phase,
                original_plan,
                resolver_ref,
            } => CanonValue::obj()
                .fstr("kind", "Pending")
                .fc("original_plan", original_plan)
                .fstr("phase", phase)
                .fc("resolver_ref", resolver_ref)
                .fc("txn_id", txn_id)
                .build(),
            ResolveReplyV1::AbsentAtBarrier {
                catalog_generation,
                home,
                home_epoch,
                record_revision,
            } => CanonValue::obj()
                .fc("catalog_generation", catalog_generation)
                .fc("home", home)
                .fc("home_epoch", home_epoch)
                .fstr("kind", "AbsentAtBarrier")
                .fc("record_revision", record_revision)
                .build(),
            ResolveReplyV1::Unavailable(r) => CanonValue::obj()
                .fstr("kind", "Unavailable")
                .fc("refusal", r)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "Terminal" => {
                ResolveReplyV1::Terminal(Box::new(ClientReplyV1::from_canon(v.field("reply")?)?))
            }
            "Pending" => ResolveReplyV1::Pending {
                txn_id: TxnId::from_canon(v.field("txn_id")?)?,
                phase: v.field("phase")?.as_str()?.to_string(),
                original_plan: PlanRef::from_canon(v.field("original_plan")?)?,
                resolver_ref: ProtocolRecordRef::from_canon(v.field("resolver_ref")?)?,
            },
            "AbsentAtBarrier" => ResolveReplyV1::AbsentAtBarrier {
                catalog_generation: CatalogGeneration::from_canon(v.field("catalog_generation")?)?,
                home: RequestHomeId::from_canon(v.field("home")?)?,
                home_epoch: RequestHomeEpoch::from_canon(v.field("home_epoch")?)?,
                record_revision: RecordRevision::from_canon(v.field("record_revision")?)?,
            },
            "Unavailable" => {
                ResolveReplyV1::Unavailable(RefusalV1::from_canon(v.field("refusal")?)?)
            }
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("resolve reply kind {k}"),
                ))
            }
        })
    }
}
