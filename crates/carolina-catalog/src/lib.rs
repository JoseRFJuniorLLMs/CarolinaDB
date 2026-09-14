//! Control-plane catalog (SPEC-011): a deterministic state machine applied from the consensus
//! log (`carolina-consensus`).
//!
//! * every key carries `ClusterId`; tenant-owned keys carry `TenantId` (§3);
//! * a `CatalogCommand` is one atomic compare-and-set over an expected set of key revisions plus
//!   registered predicates; a successful command increments `CatalogGeneration` once and stamps
//!   every changed key with that revision; a failed CAS changes nothing but retains its
//!   idempotent result under `AdminRequestId`; reusing an `AdminRequestId` with different bytes is
//!   `IdentityConflict` (§4);
//! * genesis is the first committed command: an explicit initialization over empty storage with
//!   an operator-pinned bootstrap manifest whose hash every voter verifies; a second genesis is
//!   refused (§8);
//! * authority grants follow `STAGED -> ACTIVE -> CLOSING -> CLOSED -> RETIRED`; a closed binding
//!   never returns to ACTIVE (§7);
//! * scope locks carry the conservative record closure; overlap is a state-machine predicate
//!   evaluated at application, so two overlapping migrations cannot both acquire ownership (§4);
//! * tombstoned keys refuse re-insertion (retired bindings and namespaces never resurrect).
//!
//! Reads: `get`/`generation` answer from applied state; whether that state is authoritative is the
//! consensus adapter's read barrier (§5) — callers label follower answers `observed_generation`.

use std::collections::{BTreeMap, BTreeSet};

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, Hash256};
use carolina_core::ids::*;
use carolina_core::limits::Limits;

pub const CATALOG_COMMAND_DOMAIN: &str = "astra.catalog-command.v1";
pub const BOOTSTRAP_MANIFEST_DOMAIN: &str = "astra.bootstrap-manifest.v1";

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CatalogKey {
    ClusterConfig,
    Node(NodeId),
    Authority(GrantId),
    RequestRoute(TenantId, RequestNamespace, u32),
    Namespace(TenantId, RequestNamespace),
    Plan(TenantId, PlanId, PlanGeneration),
    ActivePlan(TenantId, SemanticScopeId),
    Migration(MigrationId),
    ScopeLock(TenantId, SemanticScopeId),
    SecurityPolicy(SecurityPolicyId),
    PermissionGrant(GrantId),
    Artifact(String, Hash256),
    AdminCommand(AdminRequestId),
}

impl Canonical for CatalogKey {
    fn to_canon(&self) -> CanonValue {
        let b = CanonValue::obj();
        match self {
            CatalogKey::ClusterConfig => b.fstr("kind", "ClusterConfig").build(),
            CatalogKey::Node(n) => b.fstr("kind", "Node").fc("node", n).build(),
            CatalogKey::Authority(g) => b.fc("grant", g).fstr("kind", "Authority").build(),
            CatalogKey::RequestRoute(t, ns, bucket) => b
                .fu32("bucket", *bucket)
                .fstr("kind", "RequestRoute")
                .fc("namespace", ns)
                .fc("tenant", t)
                .build(),
            CatalogKey::Namespace(t, ns) => b
                .fstr("kind", "Namespace")
                .fc("namespace", ns)
                .fc("tenant", t)
                .build(),
            CatalogKey::Plan(t, p, g) => b
                .fc("generation", g)
                .fstr("kind", "Plan")
                .fc("plan", p)
                .fc("tenant", t)
                .build(),
            CatalogKey::ActivePlan(t, s) => b
                .fstr("kind", "ActivePlan")
                .fc("scope", s)
                .fc("tenant", t)
                .build(),
            CatalogKey::Migration(m) => b.fstr("kind", "Migration").fc("migration", m).build(),
            CatalogKey::ScopeLock(t, s) => b
                .fstr("kind", "ScopeLock")
                .fc("scope", s)
                .fc("tenant", t)
                .build(),
            CatalogKey::SecurityPolicy(p) => {
                b.fstr("kind", "SecurityPolicy").fc("policy", p).build()
            }
            CatalogKey::PermissionGrant(g) => {
                b.fc("grant", g).fstr("kind", "PermissionGrant").build()
            }
            CatalogKey::Artifact(k, h) => b
                .fstr("artifact_kind", k)
                .fc("hash", h)
                .fstr("kind", "Artifact")
                .build(),
            CatalogKey::AdminCommand(a) => b
                .fc("admin_request_id", a)
                .fstr("kind", "AdminCommand")
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "ClusterConfig" => CatalogKey::ClusterConfig,
            "Node" => CatalogKey::Node(NodeId::from_canon(v.field("node")?)?),
            "Authority" => CatalogKey::Authority(GrantId::from_canon(v.field("grant")?)?),
            "RequestRoute" => CatalogKey::RequestRoute(
                TenantId::from_canon(v.field("tenant")?)?,
                RequestNamespace::from_canon(v.field("namespace")?)?,
                v.field("bucket")?.as_u64()? as u32,
            ),
            "Namespace" => CatalogKey::Namespace(
                TenantId::from_canon(v.field("tenant")?)?,
                RequestNamespace::from_canon(v.field("namespace")?)?,
            ),
            "Plan" => CatalogKey::Plan(
                TenantId::from_canon(v.field("tenant")?)?,
                PlanId::from_canon(v.field("plan")?)?,
                PlanGeneration::from_canon(v.field("generation")?)?,
            ),
            "ActivePlan" => CatalogKey::ActivePlan(
                TenantId::from_canon(v.field("tenant")?)?,
                SemanticScopeId::from_canon(v.field("scope")?)?,
            ),
            "Migration" => CatalogKey::Migration(MigrationId::from_canon(v.field("migration")?)?),
            "ScopeLock" => CatalogKey::ScopeLock(
                TenantId::from_canon(v.field("tenant")?)?,
                SemanticScopeId::from_canon(v.field("scope")?)?,
            ),
            "SecurityPolicy" => {
                CatalogKey::SecurityPolicy(SecurityPolicyId::from_canon(v.field("policy")?)?)
            }
            "PermissionGrant" => {
                CatalogKey::PermissionGrant(GrantId::from_canon(v.field("grant")?)?)
            }
            "Artifact" => CatalogKey::Artifact(
                v.field("artifact_kind")?.as_str()?.to_string(),
                Hash256::from_canon(v.field("hash")?)?,
            ),
            "AdminCommand" => {
                CatalogKey::AdminCommand(AdminRequestId::from_canon(v.field("admin_request_id")?)?)
            }
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("catalog key {k}"),
                ))
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    Live,
    Tombstoned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub key: CatalogKey,
    pub revision: CatalogGeneration,
    pub state: EntryState,
    pub value: CanonValue,
}

impl Canonical for EntryState {
    fn to_canon(&self) -> CanonValue {
        CanonValue::Str(
            match self {
                EntryState::Live => "live",
                EntryState::Tombstoned => "tombstoned",
            }
            .into(),
        )
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.as_str()? {
            "live" => EntryState::Live,
            "tombstoned" => EntryState::Tombstoned,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("entry state {k}"),
                ))
            }
        })
    }
}

impl Canonical for CatalogEntry {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("key", &self.key)
            .fstr("kind", "catalog_entry.v1")
            .fc("revision", &self.revision)
            .fc("state", &self.state)
            .f("value", self.value.clone())
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["key", "kind", "revision", "state", "value"])?;
        Ok(CatalogEntry {
            key: CatalogKey::from_canon(v.field("key")?)?,
            revision: CatalogGeneration::from_canon(v.field("revision")?)?,
            state: EntryState::from_canon(v.field("state")?)?,
            value: v.field("value")?.clone(),
        })
    }
}

/// Point-in-time image of the catalog state machine (SPEC-011 §9).
///
/// It exists so a voter can recover its control-plane state without the log prefix that produced
/// it. It is node-local durable state, like the Raft term/vote file: it is not an interchange
/// record and has no registered record kind, because SPEC-012 owns the wire registry and snapshot
/// *transfer* between nodes is not implemented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogSnapshot {
    pub snapshot_version: u32,
    /// Last log index folded into this image.
    pub applied_index: u64,
    pub generation: u64,
    pub genesis: Option<BootstrapManifest>,
    pub entries: Vec<CatalogEntry>,
    pub results: Vec<CatalogCommit>,
}

impl CatalogSnapshot {
    pub const VERSION: u32 = 1;
}

impl Canonical for CatalogSnapshot {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fu64("applied_index", self.applied_index)
            .f(
                "entries",
                CanonValue::Array(self.entries.iter().map(|e| e.to_canon()).collect()),
            )
            .fu64("generation", self.generation)
            .fopt("genesis", &self.genesis)
            .fstr("kind", "catalog_snapshot.v1")
            .f(
                "results",
                CanonValue::Array(self.results.iter().map(|r| r.to_canon()).collect()),
            )
            .fu32("snapshot_version", self.snapshot_version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "applied_index",
            "entries",
            "generation",
            "genesis",
            "kind",
            "results",
            "snapshot_version",
        ])?;
        let snapshot_version = v.field("snapshot_version")?.as_u32()?;
        if snapshot_version != CatalogSnapshot::VERSION {
            return Err(CoreError::new(
                ErrorCode::UnsupportedFormat,
                format!("catalog snapshot version {snapshot_version}"),
            ));
        }
        let genesis = match v.field("genesis")? {
            CanonValue::Null => None,
            g => Some(BootstrapManifest::from_canon(g)?),
        };
        Ok(CatalogSnapshot {
            snapshot_version,
            applied_index: v.field("applied_index")?.as_u64()?,
            generation: v.field("generation")?.as_u64()?,
            genesis,
            entries: v
                .field("entries")?
                .as_array()?
                .iter()
                .map(CatalogEntry::from_canon)
                .collect::<CoreResult<Vec<_>>>()?,
            results: v
                .field("results")?
                .as_array()?
                .iter()
                .map(CatalogCommit::from_canon)
                .collect::<CoreResult<Vec<_>>>()?,
        })
    }
}

impl CatalogEntry {
    pub fn digest(&self) -> Hash256 {
        domain_hash("astra.catalog-entry.v1", &self.value.encode())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantState {
    Staged,
    Active,
    Closing,
    Closed,
    Retired,
}

impl GrantState {
    pub fn label(&self) -> &'static str {
        match self {
            GrantState::Staged => "STAGED",
            GrantState::Active => "ACTIVE",
            GrantState::Closing => "CLOSING",
            GrantState::Closed => "CLOSED",
            GrantState::Retired => "RETIRED",
        }
    }
    pub fn from_label(s: &str) -> CoreResult<Self> {
        Ok(match s {
            "STAGED" => GrantState::Staged,
            "ACTIVE" => GrantState::Active,
            "CLOSING" => GrantState::Closing,
            "CLOSED" => GrantState::Closed,
            "RETIRED" => GrantState::Retired,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("grant state {k}"),
                ))
            }
        })
    }
    /// SPEC-011 §7 lifecycle; a closed binding never returns to ACTIVE.
    pub fn can_advance_to(&self, next: GrantState) -> bool {
        matches!(
            (self, next),
            (GrantState::Staged, GrantState::Active)
                | (GrantState::Active, GrantState::Closing)
                | (GrantState::Closing, GrantState::Closed)
                | (GrantState::Closed, GrantState::Retired)
                | (GrantState::Staged, GrantState::Retired)
        )
    }
}

/// Subset of SPEC-011 §3 `AuthorityGrant` needed by the local/C5 slices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityGrant {
    pub grant_id: GrantId,
    pub binding: AuthorityBinding,
    pub tenant: TenantId,
    pub scope: SemanticScopeId,
    pub scope_records: Vec<RecordId>,
    pub plans: Vec<PlanRef>,
    pub placement_epoch: PlacementEpoch,
    pub membership: (ReplicationGroupId, MembershipGeneration),
    pub admitted_nodes: Vec<NodeId>,
    pub authority_durability_policy: String,
    pub security_policy: SecurityPolicyId,
    pub admission_mode: String,
    pub state: GrantState,
}

impl Canonical for AuthorityGrant {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("admission_mode", &self.admission_mode)
            .fset("admitted_nodes", &self.admitted_nodes)
            .fstr(
                "authority_durability_policy",
                &self.authority_durability_policy,
            )
            .fc("binding", &self.binding)
            .fc("grant_id", &self.grant_id)
            .fc("membership_generation", &self.membership.1)
            .fc("membership_group", &self.membership.0)
            .fc("placement_epoch", &self.placement_epoch)
            .fset("plans", &self.plans)
            .fc("scope", &self.scope)
            .fset("scope_records", &self.scope_records)
            .fc("security_policy", &self.security_policy)
            .fstr("state", self.state.label())
            .fc("tenant", &self.tenant)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(AuthorityGrant {
            grant_id: GrantId::from_canon(v.field("grant_id")?)?,
            binding: AuthorityBinding::from_canon(v.field("binding")?)?,
            tenant: TenantId::from_canon(v.field("tenant")?)?,
            scope: SemanticScopeId::from_canon(v.field("scope")?)?,
            scope_records: carolina_core::canon::decode_set(v.field("scope_records")?)?,
            plans: carolina_core::canon::decode_set(v.field("plans")?)?,
            placement_epoch: PlacementEpoch::from_canon(v.field("placement_epoch")?)?,
            membership: (
                ReplicationGroupId::from_canon(v.field("membership_group")?)?,
                MembershipGeneration::from_canon(v.field("membership_generation")?)?,
            ),
            admitted_nodes: carolina_core::canon::decode_set(v.field("admitted_nodes")?)?,
            authority_durability_policy: v
                .field("authority_durability_policy")?
                .as_str()?
                .to_string(),
            security_policy: SecurityPolicyId::from_canon(v.field("security_policy")?)?,
            admission_mode: v.field("admission_mode")?.as_str()?.to_string(),
            state: GrantState::from_label(v.field("state")?.as_str()?)?,
        })
    }
}

/// An immutable application permission installed by the catalog (SPEC-013 §4).
///
/// The first security slice deliberately supports exact tenant/namespace scopes. A grant cannot
/// expand itself through request arguments and credential rotation can retain the same principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionGrant {
    pub grant_id: GrantId,
    pub principal: PrincipalId,
    pub tenant: TenantId,
    pub namespace: RequestNamespace,
    pub operations: Vec<OperationRef>,
    pub allow_invoke: bool,
    pub allow_resolve: bool,
    pub security_policy: SecurityPolicyId,
    pub active: bool,
}

impl Canonical for PermissionGrant {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fbool("active", self.active)
            .fbool("allow_invoke", self.allow_invoke)
            .fbool("allow_resolve", self.allow_resolve)
            .fc("grant_id", &self.grant_id)
            .fc("namespace", &self.namespace)
            .fset("operations", &self.operations)
            .fc("principal", &self.principal)
            .fc("security_policy", &self.security_policy)
            .fc("tenant", &self.tenant)
            .build()
    }

    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "active",
            "allow_invoke",
            "allow_resolve",
            "grant_id",
            "namespace",
            "operations",
            "principal",
            "security_policy",
            "tenant",
        ])?;
        Ok(PermissionGrant {
            grant_id: GrantId::from_canon(v.field("grant_id")?)?,
            principal: PrincipalId::from_canon(v.field("principal")?)?,
            tenant: TenantId::from_canon(v.field("tenant")?)?,
            namespace: RequestNamespace::from_canon(v.field("namespace")?)?,
            operations: carolina_core::canon::decode_set(v.field("operations")?)?,
            allow_invoke: v.field("allow_invoke")?.as_bool()?,
            allow_resolve: v.field("allow_resolve")?.as_bool()?,
            security_policy: SecurityPolicyId::from_canon(v.field("security_policy")?)?,
            active: v.field("active")?.as_bool()?,
        })
    }
}

/// Route entry: which home (and epoch) owns a request-namespace bucket (SPEC-012 §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestRoute {
    pub home_id: RequestHomeId,
    pub home_epoch: RequestHomeEpoch,
    pub home_grant: GrantId,
}

impl Canonical for RequestRoute {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("home_epoch", &self.home_epoch)
            .fc("home_grant", &self.home_grant)
            .fc("home_id", &self.home_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(RequestRoute {
            home_id: RequestHomeId::from_canon(v.field("home_id")?)?,
            home_epoch: RequestHomeEpoch::from_canon(v.field("home_epoch")?)?,
            home_grant: GrantId::from_canon(v.field("home_grant")?)?,
        })
    }
}

/// Operator-pinned bootstrap manifest (SPEC-011 §8): cluster id, three voters, trust root and
/// bootstrap administrator. Every voter verifies the same manifest hash before applying genesis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapManifest {
    pub cluster_id: ClusterId,
    pub voters: Vec<NodeId>,
    pub trust_root_hash: Hash256,
    pub security_policy: SecurityPolicyId,
    pub bootstrap_admin: PrincipalId,
    pub security_profile: String,
}

impl Canonical for BootstrapManifest {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("bootstrap_admin", &self.bootstrap_admin)
            .fc("cluster_id", &self.cluster_id)
            .fc("security_policy", &self.security_policy)
            .fstr("security_profile", &self.security_profile)
            .fc("trust_root_hash", &self.trust_root_hash)
            .fset("voters", &self.voters)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(BootstrapManifest {
            cluster_id: ClusterId::from_canon(v.field("cluster_id")?)?,
            voters: carolina_core::canon::decode_set(v.field("voters")?)?,
            trust_root_hash: Hash256::from_canon(v.field("trust_root_hash")?)?,
            security_policy: SecurityPolicyId::from_canon(v.field("security_policy")?)?,
            bootstrap_admin: PrincipalId::from_canon(v.field("bootstrap_admin")?)?,
            security_profile: v.field("security_profile")?.as_str()?.to_string(),
        })
    }
}

impl BootstrapManifest {
    pub fn manifest_hash(&self) -> Hash256 {
        domain_hash(BOOTSTRAP_MANIFEST_DOMAIN, &self.encode())
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Absent,
    Revision(CatalogGeneration, Hash256),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    /// No live scope lock of `tenant` intersects `records` (phantom-safe: evaluated over the
    /// committed state at application).
    NoScopeOverlap {
        tenant: TenantId,
        records: Vec<RecordId>,
    },
    /// The grant is currently in `state`.
    GrantInState { grant: GrantId, state: GrantState },
    /// The namespace is not retired.
    NamespaceLive {
        tenant: TenantId,
        namespace: RequestNamespace,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    /// Insert an immutable record; the key must be absent and never tombstoned.
    PutImmutable {
        key: CatalogKey,
        value: CanonValue,
    },
    /// Replace a CAS-updated pointer/record.
    AdvancePointer {
        key: CatalogKey,
        value: CanonValue,
    },
    /// Advance a grant's lifecycle state.
    AdvanceGrantState {
        grant: GrantId,
        from: GrantState,
        to: GrantState,
    },
    Tombstone {
        key: CatalogKey,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogCommand {
    pub admin_request_id: AdminRequestId,
    pub principal: PrincipalId,
    pub expected: Vec<(CatalogKey, Expected)>,
    pub predicates: Vec<Predicate>,
    pub mutations: Vec<Mutation>,
}

fn exp_canon(e: &Expected) -> CanonValue {
    match e {
        Expected::Absent => CanonValue::obj().fstr("kind", "absent").build(),
        Expected::Revision(r, d) => CanonValue::obj()
            .fc("digest", d)
            .fstr("kind", "revision")
            .fc("revision", r)
            .build(),
    }
}

fn exp_from(v: &CanonValue) -> CoreResult<Expected> {
    Ok(match v.field("kind")?.as_str()? {
        "absent" => Expected::Absent,
        "revision" => Expected::Revision(
            CatalogGeneration::from_canon(v.field("revision")?)?,
            Hash256::from_canon(v.field("digest")?)?,
        ),
        k => {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                format!("expected {k}"),
            ))
        }
    })
}

impl Canonical for Predicate {
    fn to_canon(&self) -> CanonValue {
        match self {
            Predicate::NoScopeOverlap { tenant, records } => CanonValue::obj()
                .fstr("kind", "NoScopeOverlap")
                .fset("records", records)
                .fc("tenant", tenant)
                .build(),
            Predicate::GrantInState { grant, state } => CanonValue::obj()
                .fc("grant", grant)
                .fstr("kind", "GrantInState")
                .fstr("state", state.label())
                .build(),
            Predicate::NamespaceLive { tenant, namespace } => CanonValue::obj()
                .fstr("kind", "NamespaceLive")
                .fc("namespace", namespace)
                .fc("tenant", tenant)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "NoScopeOverlap" => Predicate::NoScopeOverlap {
                tenant: TenantId::from_canon(v.field("tenant")?)?,
                records: carolina_core::canon::decode_set(v.field("records")?)?,
            },
            "GrantInState" => Predicate::GrantInState {
                grant: GrantId::from_canon(v.field("grant")?)?,
                state: GrantState::from_label(v.field("state")?.as_str()?)?,
            },
            "NamespaceLive" => Predicate::NamespaceLive {
                tenant: TenantId::from_canon(v.field("tenant")?)?,
                namespace: RequestNamespace::from_canon(v.field("namespace")?)?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("predicate {k}"),
                ))
            }
        })
    }
}

impl Canonical for Mutation {
    fn to_canon(&self) -> CanonValue {
        match self {
            Mutation::PutImmutable { key, value } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "PutImmutable")
                .f("value", value.clone())
                .build(),
            Mutation::AdvancePointer { key, value } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "AdvancePointer")
                .f("value", value.clone())
                .build(),
            Mutation::AdvanceGrantState { grant, from, to } => CanonValue::obj()
                .fstr("from", from.label())
                .fc("grant", grant)
                .fstr("kind", "AdvanceGrantState")
                .fstr("to", to.label())
                .build(),
            Mutation::Tombstone { key } => CanonValue::obj()
                .fc("key", key)
                .fstr("kind", "Tombstone")
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "PutImmutable" => Mutation::PutImmutable {
                key: CatalogKey::from_canon(v.field("key")?)?,
                value: v.field("value")?.clone(),
            },
            "AdvancePointer" => Mutation::AdvancePointer {
                key: CatalogKey::from_canon(v.field("key")?)?,
                value: v.field("value")?.clone(),
            },
            "AdvanceGrantState" => Mutation::AdvanceGrantState {
                grant: GrantId::from_canon(v.field("grant")?)?,
                from: GrantState::from_label(v.field("from")?.as_str()?)?,
                to: GrantState::from_label(v.field("to")?.as_str()?)?,
            },
            "Tombstone" => Mutation::Tombstone {
                key: CatalogKey::from_canon(v.field("key")?)?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("mutation {k}"),
                ))
            }
        })
    }
}

impl Canonical for CatalogCommand {
    fn to_canon(&self) -> CanonValue {
        let expected: Vec<CanonValue> = self
            .expected
            .iter()
            .map(|(k, e)| {
                CanonValue::obj()
                    .f("expected", exp_canon(e))
                    .fc("key", k)
                    .build()
            })
            .collect();
        CanonValue::obj()
            .fc("admin_request_id", &self.admin_request_id)
            .f("expected", CanonValue::Array(expected))
            .fvec("mutations", &self.mutations)
            .fvec("predicates", &self.predicates)
            .fc("principal", &self.principal)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "admin_request_id",
            "expected",
            "mutations",
            "predicates",
            "principal",
        ])?;
        let mut expected = Vec::new();
        for e in v.field("expected")?.as_array()? {
            expected.push((
                CatalogKey::from_canon(e.field("key")?)?,
                exp_from(e.field("expected")?)?,
            ));
        }
        Ok(CatalogCommand {
            admin_request_id: AdminRequestId::from_canon(v.field("admin_request_id")?)?,
            principal: PrincipalId::from_canon(v.field("principal")?)?,
            expected,
            predicates: v
                .field("predicates")?
                .as_array()?
                .iter()
                .map(Predicate::from_canon)
                .collect::<CoreResult<Vec<_>>>()?,
            mutations: v
                .field("mutations")?
                .as_array()?
                .iter()
                .map(Mutation::from_canon)
                .collect::<CoreResult<Vec<_>>>()?,
        })
    }
}

impl CatalogCommand {
    /// Immutable command hash: everything except the admin request id (the id names the command).
    pub fn command_hash(&self) -> Hash256 {
        let body = CanonValue::obj()
            .f(
                "expected",
                self.to_canon().field("expected").unwrap().clone(),
            )
            .fvec("mutations", &self.mutations)
            .fvec("predicates", &self.predicates)
            .fc("principal", &self.principal)
            .build();
        domain_hash(CATALOG_COMMAND_DOMAIN, &body.encode())
    }
}

/// What the consensus log carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogCommand {
    Genesis(BootstrapManifest),
    Catalog(CatalogCommand),
}

impl Canonical for LogCommand {
    fn to_canon(&self) -> CanonValue {
        match self {
            LogCommand::Genesis(m) => CanonValue::obj()
                .fstr("kind", "Genesis")
                .fc("manifest", m)
                .build(),
            LogCommand::Catalog(c) => CanonValue::obj()
                .fc("command", c)
                .fstr("kind", "Catalog")
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "Genesis" => LogCommand::Genesis(BootstrapManifest::from_canon(v.field("manifest")?)?),
            "Catalog" => LogCommand::Catalog(CatalogCommand::from_canon(v.field("command")?)?),
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("log command {k}"),
                ))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogCommit {
    pub admin_request_id: AdminRequestId,
    pub command_hash: Hash256,
    pub catalog_generation: CatalogGeneration,
    pub changed_keys: Vec<CatalogKey>,
    pub committed_log_ref: u64,
    /// `None` = success; `Some(code)` = the CAS/predicate failure that was retained.
    pub failure: Option<String>,
}

impl Canonical for CatalogCommit {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("admin_request_id", &self.admin_request_id)
            .fc("catalog_generation", &self.catalog_generation)
            .fvec("changed_keys", &self.changed_keys)
            .fc("command_hash", &self.command_hash)
            .fu64("committed_log_ref", self.committed_log_ref)
            .f(
                "failure",
                match &self.failure {
                    Some(f) => CanonValue::str(f.as_str()),
                    None => CanonValue::Null,
                },
            )
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(CatalogCommit {
            admin_request_id: AdminRequestId::from_canon(v.field("admin_request_id")?)?,
            command_hash: Hash256::from_canon(v.field("command_hash")?)?,
            catalog_generation: CatalogGeneration::from_canon(v.field("catalog_generation")?)?,
            changed_keys: v
                .field("changed_keys")?
                .as_array()?
                .iter()
                .map(CatalogKey::from_canon)
                .collect::<CoreResult<Vec<_>>>()?,
            committed_log_ref: v.field("committed_log_ref")?.as_u64()?,
            failure: match v.field("failure")? {
                CanonValue::Null => None,
                x => Some(x.as_str()?.to_string()),
            },
        })
    }
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    entries: BTreeMap<CatalogKey, CatalogEntry>,
    generation: u64,
    applied_index: u64,
    genesis: Option<BootstrapManifest>,
    results: BTreeMap<AdminRequestId, CatalogCommit>,
}

impl Catalog {
    pub fn new() -> Catalog {
        Catalog::default()
    }
    pub fn generation(&self) -> CatalogGeneration {
        CatalogGeneration(self.generation)
    }
    pub fn applied_index(&self) -> u64 {
        self.applied_index
    }

    /// Fold the whole state machine into one image (SPEC-011 §9).
    pub fn snapshot(&self) -> CatalogSnapshot {
        CatalogSnapshot {
            snapshot_version: CatalogSnapshot::VERSION,
            applied_index: self.applied_index,
            generation: self.generation,
            genesis: self.genesis.clone(),
            entries: self.entries.values().cloned().collect(),
            results: self.results.values().cloned().collect(),
        }
    }

    /// Rebuild a catalog from an image. Replaying the log after `applied_index` on top of the
    /// result reproduces the state the image was taken from.
    pub fn restore(s: &CatalogSnapshot) -> Catalog {
        Catalog {
            entries: s
                .entries
                .iter()
                .map(|e| (e.key.clone(), e.clone()))
                .collect(),
            generation: s.generation,
            applied_index: s.applied_index,
            genesis: s.genesis.clone(),
            results: s
                .results
                .iter()
                .map(|r| (r.admin_request_id, r.clone()))
                .collect(),
        }
    }
    pub fn genesis(&self) -> Option<&BootstrapManifest> {
        self.genesis.as_ref()
    }
    pub fn cluster_id(&self) -> Option<ClusterId> {
        self.genesis.as_ref().map(|g| g.cluster_id)
    }
    pub fn get(&self, key: &CatalogKey) -> Option<&CatalogEntry> {
        self.entries
            .get(key)
            .filter(|e| e.state == EntryState::Live)
    }
    pub fn get_any(&self, key: &CatalogKey) -> Option<&CatalogEntry> {
        self.entries.get(key)
    }
    pub fn result_of(&self, id: &AdminRequestId) -> Option<&CatalogCommit> {
        self.results.get(id)
    }
    pub fn grant(&self, id: GrantId) -> Option<AuthorityGrant> {
        self.get(&CatalogKey::Authority(id))
            .and_then(|e| AuthorityGrant::from_canon(&e.value).ok())
    }
    pub fn permission_grant(&self, id: GrantId) -> Option<PermissionGrant> {
        self.get(&CatalogKey::PermissionGrant(id))
            .and_then(|e| PermissionGrant::from_canon(&e.value).ok())
            .filter(|grant| grant.grant_id == id)
    }
    /// Bootstrap administration is the only administrative authority in the initial profile.
    pub fn admin_allowed(&self, principal: PrincipalId) -> bool {
        self.genesis
            .as_ref()
            .is_some_and(|m| m.bootstrap_admin == principal)
    }
    pub fn invoke_allowed(
        &self,
        principal: PrincipalId,
        tenant: TenantId,
        namespace: RequestNamespace,
        operation: OperationRef,
    ) -> bool {
        self.live_entries().any(|entry| {
            let CatalogKey::PermissionGrant(key_id) = &entry.key else {
                return false;
            };
            PermissionGrant::from_canon(&entry.value).is_ok_and(|grant| {
                grant.grant_id == *key_id
                    && grant.active
                    && grant.allow_invoke
                    && grant.principal == principal
                    && grant.tenant == tenant
                    && grant.namespace == namespace
                    && grant.operations.contains(&operation)
                    && self
                        .genesis
                        .as_ref()
                        .is_some_and(|m| m.security_policy == grant.security_policy)
            })
        })
    }
    pub fn resolve_allowed(
        &self,
        principal: PrincipalId,
        tenant: TenantId,
        namespace: RequestNamespace,
    ) -> bool {
        self.live_entries().any(|entry| {
            let CatalogKey::PermissionGrant(key_id) = &entry.key else {
                return false;
            };
            PermissionGrant::from_canon(&entry.value).is_ok_and(|grant| {
                grant.grant_id == *key_id
                    && grant.active
                    && grant.allow_resolve
                    && grant.principal == principal
                    && grant.tenant == tenant
                    && grant.namespace == namespace
                    && self
                        .genesis
                        .as_ref()
                        .is_some_and(|m| m.security_policy == grant.security_policy)
            })
        })
    }
    pub fn route(
        &self,
        tenant: TenantId,
        ns: RequestNamespace,
        bucket: u32,
    ) -> Option<RequestRoute> {
        self.get(&CatalogKey::RequestRoute(tenant, ns, bucket))
            .and_then(|e| RequestRoute::from_canon(&e.value).ok())
    }
    /// A node may execute under a grant only while the grant is ACTIVE and the node is admitted.
    pub fn admission_allowed(&self, grant: GrantId, node: NodeId) -> bool {
        match self.grant(grant) {
            Some(g) => g.state == GrantState::Active && g.admitted_nodes.contains(&node),
            None => false,
        }
    }
    pub fn live_entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries
            .values()
            .filter(|e| e.state == EntryState::Live)
    }

    /// Digest of the committed state (for snapshots / CAT-09 comparisons).
    pub fn state_digest(&self) -> Hash256 {
        let mut payload = Vec::new();
        for e in self.entries.values() {
            payload.extend_from_slice(&e.key.encode());
            payload.extend_from_slice(&e.revision.0.to_le_bytes());
            payload.push(if e.state == EntryState::Live { 1 } else { 0 });
            payload.extend_from_slice(&e.value.encode());
        }
        domain_hash("astra.catalog-state.v1", &payload)
    }

    /// Apply one committed log entry. `expected_manifest_hash` is this voter's own pinned
    /// bootstrap manifest hash: a genesis that does not match is refused (the node must not
    /// join a cluster it was not configured for; SPEC-011 §8, CAT-12).
    pub fn apply(
        &mut self,
        log_index: u64,
        command: &LogCommand,
        expected_manifest_hash: Hash256,
    ) -> CoreResult<Option<CatalogCommit>> {
        if log_index <= self.applied_index {
            return Ok(None); // already applied (replay after restart)
        }
        let out = match command {
            LogCommand::Genesis(m) => {
                if self.genesis.is_some() {
                    return Err(CoreError::new(
                        ErrorCode::IdentityConflict,
                        "second genesis under an initialized cluster",
                    ));
                }
                if m.manifest_hash() != expected_manifest_hash {
                    return Err(CoreError::new(
                        ErrorCode::InvalidManifest,
                        "bootstrap manifest hash does not match this voter's pinned manifest",
                    ));
                }
                if m.voters.len() != 3 {
                    return Err(CoreError::new(
                        ErrorCode::InvalidManifest,
                        "the initial profile requires exactly three voters",
                    ));
                }
                self.generation += 1;
                let rev = CatalogGeneration(self.generation);
                self.entries.insert(
                    CatalogKey::ClusterConfig,
                    CatalogEntry {
                        key: CatalogKey::ClusterConfig,
                        revision: rev,
                        state: EntryState::Live,
                        value: m.to_canon(),
                    },
                );
                for n in &m.voters {
                    let value = CanonValue::obj()
                        .fc("node_id", n)
                        .fstr("role", "voter")
                        .build();
                    self.entries.insert(
                        CatalogKey::Node(*n),
                        CatalogEntry {
                            key: CatalogKey::Node(*n),
                            revision: rev,
                            state: EntryState::Live,
                            value,
                        },
                    );
                }
                self.genesis = Some(m.clone());
                None
            }
            LogCommand::Catalog(c) => {
                if self.genesis.is_none() {
                    return Err(CoreError::new(
                        ErrorCode::NotReady,
                        "catalog command before genesis",
                    ));
                }
                Some(self.apply_command(log_index, c))
            }
        };
        self.applied_index = log_index;
        Ok(out)
    }

    fn apply_command(&mut self, log_index: u64, c: &CatalogCommand) -> CatalogCommit {
        let hash = c.command_hash();
        if let Some(prev) = self.results.get(&c.admin_request_id) {
            if prev.command_hash == hash {
                return prev.clone(); // duplicate: same idempotent result, no new revision
            }
            let r = CatalogCommit {
                admin_request_id: c.admin_request_id,
                command_hash: hash,
                catalog_generation: CatalogGeneration(self.generation),
                changed_keys: vec![],
                committed_log_ref: log_index,
                failure: Some("IdentityConflict".into()),
            };
            return r; // not retained as the id's result: the original stays
        }
        let failure = self.validate(c).err();
        let commit = match failure {
            Some(code) => CatalogCommit {
                admin_request_id: c.admin_request_id,
                command_hash: hash,
                catalog_generation: CatalogGeneration(self.generation),
                changed_keys: vec![],
                committed_log_ref: log_index,
                failure: Some(code),
            },
            None => {
                self.generation += 1;
                let rev = CatalogGeneration(self.generation);
                let mut changed = Vec::new();
                for m in &c.mutations {
                    match m {
                        Mutation::PutImmutable { key, value }
                        | Mutation::AdvancePointer { key, value } => {
                            self.entries.insert(
                                key.clone(),
                                CatalogEntry {
                                    key: key.clone(),
                                    revision: rev,
                                    state: EntryState::Live,
                                    value: value.clone(),
                                },
                            );
                            changed.push(key.clone());
                        }
                        Mutation::AdvanceGrantState { grant, to, .. } => {
                            let key = CatalogKey::Authority(*grant);
                            if let Some(e) = self.entries.get_mut(&key) {
                                if let Ok(mut g) = AuthorityGrant::from_canon(&e.value) {
                                    g.state = *to;
                                    e.value = g.to_canon();
                                    e.revision = rev;
                                }
                            }
                            changed.push(key);
                        }
                        Mutation::Tombstone { key } => {
                            let value = self
                                .entries
                                .get(key)
                                .map(|e| e.value.clone())
                                .unwrap_or(CanonValue::Null);
                            self.entries.insert(
                                key.clone(),
                                CatalogEntry {
                                    key: key.clone(),
                                    revision: rev,
                                    state: EntryState::Tombstoned,
                                    value,
                                },
                            );
                            changed.push(key.clone());
                        }
                    }
                }
                let admin_key = CatalogKey::AdminCommand(c.admin_request_id);
                self.entries.insert(
                    admin_key.clone(),
                    CatalogEntry {
                        key: admin_key,
                        revision: rev,
                        state: EntryState::Live,
                        value: CanonValue::obj()
                            .fc("command_hash", &hash)
                            .fu64("log_ref", log_index)
                            .build(),
                    },
                );
                CatalogCommit {
                    admin_request_id: c.admin_request_id,
                    command_hash: hash,
                    catalog_generation: rev,
                    changed_keys: changed,
                    committed_log_ref: log_index,
                    failure: None,
                }
            }
        };
        self.results.insert(c.admin_request_id, commit.clone());
        commit
    }

    /// All expected revisions, predicates and mutation preconditions against the committed state.
    fn validate(&self, c: &CatalogCommand) -> Result<(), String> {
        if !self.admin_allowed(c.principal) {
            return Err("AuthorizationDenied".into());
        }
        for (key, exp) in &c.expected {
            match (self.entries.get(key), exp) {
                (None, Expected::Absent) => {}
                (Some(e), Expected::Absent) if e.state == EntryState::Tombstoned => {
                    return Err("CatalogCasConflict:tombstoned".into())
                }
                (Some(_), Expected::Absent) => return Err("CatalogCasConflict:present".into()),
                (Some(e), Expected::Revision(r, d)) => {
                    if e.state != EntryState::Live || e.revision != *r || e.digest() != *d {
                        return Err("CatalogCasConflict:revision".into());
                    }
                }
                (None, Expected::Revision(..)) => return Err("CatalogCasConflict:absent".into()),
            }
        }
        for p in &c.predicates {
            match p {
                Predicate::NoScopeOverlap { tenant, records } => {
                    let want: BTreeSet<RecordId> = records.iter().copied().collect();
                    for e in self.live_entries() {
                        if let CatalogKey::ScopeLock(t, _) = &e.key {
                            if t == tenant {
                                let held: BTreeSet<RecordId> = e
                                    .value
                                    .field("records")
                                    .ok()
                                    .and_then(|v| {
                                        carolina_core::canon::decode_set::<RecordId>(v).ok()
                                    })
                                    .unwrap_or_default()
                                    .into_iter()
                                    .collect();
                                if !held.is_disjoint(&want) {
                                    return Err("ScopeLocked".into());
                                }
                            }
                        }
                    }
                }
                Predicate::GrantInState { grant, state } => match self.grant(*grant) {
                    Some(g) if g.state == *state => {}
                    Some(_) => return Err("StaleAuthorityBinding".into()),
                    None => return Err("StaleAuthorityBinding:missing".into()),
                },
                Predicate::NamespaceLive { tenant, namespace } => {
                    if let Some(e) = self
                        .entries
                        .get(&CatalogKey::Namespace(*tenant, *namespace))
                    {
                        if e.state == EntryState::Tombstoned
                            || e.value
                                .field("retired")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false)
                        {
                            return Err("NamespaceRetired".into());
                        }
                    }
                }
            }
        }
        for m in &c.mutations {
            match m {
                Mutation::PutImmutable { key, value } => {
                    if let Some(e) = self.entries.get(key) {
                        return Err(if e.state == EntryState::Tombstoned {
                            "ArtifactIdentityConflict:tombstoned".into()
                        } else {
                            "ArtifactIdentityConflict:present".into()
                        });
                    }
                    if let CatalogKey::PermissionGrant(key_id) = key {
                        let grant = PermissionGrant::from_canon(value)
                            .map_err(|_| "InvalidPermissionGrant:encoding".to_string())?;
                        if grant.grant_id != *key_id {
                            return Err("InvalidPermissionGrant:identity".into());
                        }
                        if self.genesis.as_ref().is_none_or(|manifest| {
                            grant.security_policy != manifest.security_policy
                        }) {
                            return Err("InvalidPermissionGrant:security-policy".into());
                        }
                        if grant.operations.is_empty() {
                            return Err("InvalidPermissionGrant:empty-operations".into());
                        }
                    }
                }
                Mutation::AdvancePointer { key, .. } => {
                    if let Some(e) = self.entries.get(key) {
                        if e.state == EntryState::Tombstoned {
                            return Err("CatalogCasConflict:tombstoned".into());
                        }
                    }
                }
                Mutation::AdvanceGrantState { grant, from, to } => match self.grant(*grant) {
                    Some(g) if g.state == *from && from.can_advance_to(*to) => {}
                    Some(g) => {
                        return Err(format!(
                            "StaleAuthorityBinding:{}->{}",
                            g.state.label(),
                            to.label()
                        ))
                    }
                    None => return Err("StaleAuthorityBinding:missing".into()),
                },
                Mutation::Tombstone { key } => {
                    if self.entries.get(key).map(|e| e.state) != Some(EntryState::Live) {
                        return Err("CatalogCasConflict:absent".into());
                    }
                }
            }
        }
        Ok(())
    }

    /// Decode a log entry's bytes.
    pub fn decode_command(bytes: &[u8]) -> CoreResult<LogCommand> {
        LogCommand::decode(bytes, &Limits::v1())
    }
}

/// SPEC-011 §11 acceptance subset as a callable campaign (shared with `carolina-qualify`):
/// CAT-01 (overlapping scope locks), CAT-02 (duplicate admin command resolves to the one committed
/// result), CAT-05 (a staged grant admits nothing), CAT-12 (mismatching/second genesis refused),
/// CAT-15 (route claim CAS admits one mapping), CAT-16 (replayed grant after tombstone refused),
/// plus the closed-never-reopens rule of §7.
pub fn acceptance_campaign() -> Result<Vec<String>, String> {
    let e = |x: CoreError| x.to_string();
    let mut passed = Vec::new();
    let m = BootstrapManifest {
        cluster_id: ClusterId::derive("cluster-q"),
        voters: vec![
            NodeId::derive("n1"),
            NodeId::derive("n2"),
            NodeId::derive("n3"),
        ],
        trust_root_hash: Hash256([1; 32]),
        security_policy: SecurityPolicyId::derive("policy-1"),
        bootstrap_admin: PrincipalId::derive("admin"),
        security_profile: "DEV_LOCAL".into(),
    };
    let tenant = TenantId::derive("t");
    let admin = PrincipalId::derive("admin");
    let mut c = Catalog::new();
    if c.apply(1, &LogCommand::Genesis(m.clone()), Hash256::ZERO)
        .is_ok()
    {
        return Err(
            "CAT-12: a voter accepted a genesis whose manifest hash is not its pinned one".into(),
        );
    }
    c.apply(1, &LogCommand::Genesis(m.clone()), m.manifest_hash())
        .map_err(e)?;
    if c.apply(2, &LogCommand::Genesis(m.clone()), m.manifest_hash())
        .is_ok()
    {
        return Err("CAT-12: second genesis accepted".into());
    }
    passed.push("CAT-12".into());
    let lock = |id: &str, scope: &str, recs: Vec<u64>| {
        let recs: Vec<RecordId> = recs.into_iter().map(RecordId).collect();
        CatalogCommand {
            admin_request_id: AdminRequestId::derive(id),
            principal: admin,
            expected: vec![(
                CatalogKey::ScopeLock(tenant, SemanticScopeId::derive(scope)),
                Expected::Absent,
            )],
            predicates: vec![Predicate::NoScopeOverlap {
                tenant,
                records: recs.clone(),
            }],
            mutations: vec![Mutation::PutImmutable {
                key: CatalogKey::ScopeLock(tenant, SemanticScopeId::derive(scope)),
                value: CanonValue::obj().fset("records", &recs).build(),
            }],
        }
    };
    let first = lock("m1", "a", vec![1, 2]);
    let r = c
        .apply(2, &LogCommand::Catalog(first.clone()), m.manifest_hash())
        .map_err(e)?
        .unwrap();
    if r.failure.is_some() {
        return Err("CAT-01: first lock refused".into());
    }
    let r2 = c
        .apply(
            3,
            &LogCommand::Catalog(lock("m2", "b", vec![2, 3])),
            m.manifest_hash(),
        )
        .map_err(e)?
        .unwrap();
    if r2.failure.as_deref() != Some("ScopeLocked") {
        return Err(format!(
            "CAT-01: overlapping lock not refused: {:?}",
            r2.failure
        ));
    }
    passed.push("CAT-01".into());
    let again = c
        .apply(4, &LogCommand::Catalog(first.clone()), m.manifest_hash())
        .map_err(e)?
        .unwrap();
    if again != r || c.generation() != r.catalog_generation {
        return Err(
            "CAT-02: duplicate admin command did not resolve to the one committed result".into(),
        );
    }
    passed.push("CAT-02".into());
    let gid = GrantId::derive("g");
    let grant = AuthorityGrant {
        grant_id: gid,
        binding: AuthorityBinding::RequestHome {
            home_id: RequestHomeId::derive("h"),
            epoch: RequestHomeEpoch(1),
        },
        tenant,
        scope: SemanticScopeId::derive("inv"),
        scope_records: vec![RecordId(9)],
        plans: vec![],
        placement_epoch: PlacementEpoch(1),
        membership: (ReplicationGroupId::derive("g"), MembershipGeneration(1)),
        admitted_nodes: m.voters.clone(),
        authority_durability_policy: "quorum".into(),
        security_policy: m.security_policy,
        admission_mode: "ONLINE_BARRIER".into(),
        state: GrantState::Staged,
    };
    let put = CatalogCommand {
        admin_request_id: AdminRequestId::derive("stage"),
        principal: admin,
        expected: vec![(CatalogKey::Authority(gid), Expected::Absent)],
        predicates: vec![],
        mutations: vec![Mutation::PutImmutable {
            key: CatalogKey::Authority(gid),
            value: grant.to_canon(),
        }],
    };
    c.apply(5, &LogCommand::Catalog(put), m.manifest_hash())
        .map_err(e)?;
    if c.admission_allowed(gid, m.voters[0]) {
        return Err("CAT-05: a staged grant admitted work".into());
    }
    passed.push("CAT-05".into());
    let adv = |id: &str, from: GrantState, to: GrantState| CatalogCommand {
        admin_request_id: AdminRequestId::derive(id),
        principal: admin,
        expected: vec![],
        predicates: vec![],
        mutations: vec![Mutation::AdvanceGrantState {
            grant: gid,
            from,
            to,
        }],
    };
    c.apply(
        6,
        &LogCommand::Catalog(adv("act", GrantState::Staged, GrantState::Active)),
        m.manifest_hash(),
    )
    .map_err(e)?;
    if !c.admission_allowed(gid, m.voters[0])
        || c.admission_allowed(gid, NodeId::derive("stranger"))
    {
        return Err("CAT-05: active grant admission wrong".into());
    }
    let route = |id: &str, h: &str| CatalogCommand {
        admin_request_id: AdminRequestId::derive(id),
        principal: admin,
        expected: vec![(
            CatalogKey::RequestRoute(tenant, RequestNamespace::derive("ns"), 0),
            Expected::Absent,
        )],
        predicates: vec![],
        mutations: vec![Mutation::AdvancePointer {
            key: CatalogKey::RequestRoute(tenant, RequestNamespace::derive("ns"), 0),
            value: RequestRoute {
                home_id: RequestHomeId::derive(h),
                home_epoch: RequestHomeEpoch(1),
                home_grant: gid,
            }
            .to_canon(),
        }],
    };
    c.apply(
        7,
        &LogCommand::Catalog(route("r1", "home-1")),
        m.manifest_hash(),
    )
    .map_err(e)?;
    let r2 = c
        .apply(
            8,
            &LogCommand::Catalog(route("r2", "home-2")),
            m.manifest_hash(),
        )
        .map_err(e)?
        .unwrap();
    if r2.failure.is_none()
        || c.route(tenant, RequestNamespace::derive("ns"), 0)
            .map(|r| r.home_id)
            != Some(RequestHomeId::derive("home-1"))
    {
        return Err("CAT-15: two homes obtained the same route".into());
    }
    passed.push("CAT-15".into());
    c.apply(
        9,
        &LogCommand::Catalog(adv("closing", GrantState::Active, GrantState::Closing)),
        m.manifest_hash(),
    )
    .map_err(e)?;
    c.apply(
        10,
        &LogCommand::Catalog(adv("closed", GrantState::Closing, GrantState::Closed)),
        m.manifest_hash(),
    )
    .map_err(e)?;
    let reopen = c
        .apply(
            11,
            &LogCommand::Catalog(adv("reopen", GrantState::Closed, GrantState::Active)),
            m.manifest_hash(),
        )
        .map_err(e)?
        .unwrap();
    if reopen.failure.is_none() {
        return Err("SPEC-011 §7: a closed binding returned to ACTIVE".into());
    }
    c.apply(
        12,
        &LogCommand::Catalog(adv("retire", GrantState::Closed, GrantState::Retired)),
        m.manifest_hash(),
    )
    .map_err(e)?;
    let tomb = CatalogCommand {
        admin_request_id: AdminRequestId::derive("tomb"),
        principal: admin,
        expected: vec![],
        predicates: vec![],
        mutations: vec![Mutation::Tombstone {
            key: CatalogKey::Authority(gid),
        }],
    };
    c.apply(13, &LogCommand::Catalog(tomb), m.manifest_hash())
        .map_err(e)?;
    let replay = CatalogCommand {
        admin_request_id: AdminRequestId::derive("replay"),
        principal: admin,
        expected: vec![],
        predicates: vec![],
        mutations: vec![Mutation::PutImmutable {
            key: CatalogKey::Authority(gid),
            value: grant.to_canon(),
        }],
    };
    let rr = c
        .apply(14, &LogCommand::Catalog(replay), m.manifest_hash())
        .map_err(e)?
        .unwrap();
    if rr.failure.is_none() {
        return Err("CAT-16: a retired grant was re-inserted from a replayed record".into());
    }
    passed.push("CAT-16".into());
    passed.push("closed-never-reopens".into());
    Ok(passed)
}

/// Non-cryptographic authorization slice of SPEC-013. Transport authentication remains a
/// separate qualification requirement; these checks prove that an authenticated principal value
/// cannot cross the catalog or tenant/namespace permission boundaries.
pub fn authorization_campaign() -> Result<Vec<String>, String> {
    let manifest = BootstrapManifest {
        cluster_id: ClusterId::derive("cluster-authz"),
        voters: vec![
            NodeId::derive("n1"),
            NodeId::derive("n2"),
            NodeId::derive("n3"),
        ],
        trust_root_hash: Hash256([9; 32]),
        security_policy: SecurityPolicyId::derive("policy-authz"),
        bootstrap_admin: PrincipalId::derive("admin-authz"),
        security_profile: "DEV_LOCAL".into(),
    };
    let mut catalog = Catalog::new();
    catalog
        .apply(
            1,
            &LogCommand::Genesis(manifest.clone()),
            manifest.manifest_hash(),
        )
        .map_err(|e| e.to_string())?;

    let attacker = CatalogCommand {
        admin_request_id: AdminRequestId::derive("authz/attacker"),
        principal: PrincipalId::derive("attacker"),
        expected: vec![],
        predicates: vec![],
        mutations: vec![Mutation::PutImmutable {
            key: CatalogKey::Artifact("forged".into(), Hash256([3; 32])),
            value: CanonValue::str("forged"),
        }],
    };
    let denied = catalog
        .apply(2, &LogCommand::Catalog(attacker), manifest.manifest_hash())
        .map_err(|e| e.to_string())?
        .ok_or("authorization command produced no result")?;
    if denied.failure.as_deref() != Some("AuthorizationDenied")
        || catalog
            .get(&CatalogKey::Artifact("forged".into(), Hash256([3; 32])))
            .is_some()
    {
        return Err("SEC-02/14: unauthorized catalog mutation was not denied atomically".into());
    }

    let tenant = TenantId::derive("tenant-authz");
    let namespace = RequestNamespace::derive("namespace-authz");
    let operation = OperationRef {
        operation_id: OperationId(17),
        version: 1,
    };
    let client = PrincipalId::derive("client-authz");
    let grant_id = GrantId::derive("permission-authz");
    let permission = PermissionGrant {
        grant_id,
        principal: client,
        tenant,
        namespace,
        operations: vec![operation],
        allow_invoke: true,
        allow_resolve: true,
        security_policy: manifest.security_policy,
        active: true,
    };
    let mismatched_id = GrantId::derive("permission-authz-mismatched-key");
    let malformed = CatalogCommand {
        admin_request_id: AdminRequestId::derive("authz/malformed-grant"),
        principal: manifest.bootstrap_admin,
        expected: vec![(CatalogKey::PermissionGrant(mismatched_id), Expected::Absent)],
        predicates: vec![],
        mutations: vec![Mutation::PutImmutable {
            key: CatalogKey::PermissionGrant(mismatched_id),
            value: permission.to_canon(),
        }],
    };
    let malformed_result = catalog
        .apply(3, &LogCommand::Catalog(malformed), manifest.manifest_hash())
        .map_err(|e| e.to_string())?
        .ok_or("malformed permission command produced no result")?;
    if malformed_result.failure.as_deref() != Some("InvalidPermissionGrant:identity")
        || catalog.permission_grant(mismatched_id).is_some()
    {
        return Err("SEC-14: mismatched permission identity was not denied atomically".into());
    }
    let install = CatalogCommand {
        admin_request_id: AdminRequestId::derive("authz/install"),
        principal: manifest.bootstrap_admin,
        expected: vec![(CatalogKey::PermissionGrant(grant_id), Expected::Absent)],
        predicates: vec![],
        mutations: vec![Mutation::PutImmutable {
            key: CatalogKey::PermissionGrant(grant_id),
            value: permission.to_canon(),
        }],
    };
    let installed = catalog
        .apply(4, &LogCommand::Catalog(install), manifest.manifest_hash())
        .map_err(|e| e.to_string())?
        .ok_or("permission install produced no result")?;
    if installed.failure.is_some() {
        return Err(format!(
            "permission install failed: {:?}",
            installed.failure
        ));
    }
    if !catalog.invoke_allowed(client, tenant, namespace, operation)
        || !catalog.resolve_allowed(client, tenant, namespace)
        || catalog.invoke_allowed(
            client,
            TenantId::derive("tenant-other"),
            namespace,
            operation,
        )
        || catalog.invoke_allowed(
            client,
            tenant,
            RequestNamespace::derive("namespace-other"),
            operation,
        )
        || catalog.invoke_allowed(
            client,
            tenant,
            namespace,
            OperationRef {
                operation_id: OperationId(18),
                version: 1,
            },
        )
        || catalog.invoke_allowed(
            client,
            tenant,
            namespace,
            OperationRef {
                operation_id: operation.operation_id,
                version: 2,
            },
        )
        || catalog.resolve_allowed(PrincipalId::derive("other-client"), tenant, namespace)
    {
        return Err("SEC-02: exact tenant namespace or operation boundary was not enforced".into());
    }

    let revoke = CatalogCommand {
        admin_request_id: AdminRequestId::derive("authz/revoke"),
        principal: manifest.bootstrap_admin,
        expected: vec![],
        predicates: vec![],
        mutations: vec![Mutation::Tombstone {
            key: CatalogKey::PermissionGrant(grant_id),
        }],
    };
    catalog
        .apply(5, &LogCommand::Catalog(revoke), manifest.manifest_hash())
        .map_err(|e| e.to_string())?;
    if catalog.invoke_allowed(client, tenant, namespace, operation)
        || catalog.resolve_allowed(client, tenant, namespace)
    {
        return Err("SEC-09: a tombstoned permission still authorized new disclosure".into());
    }

    Ok(vec![
        "SEC-02 tenant/namespace isolation".into(),
        "SEC-09 current authorization on resolve".into(),
        "SEC-14 privileged mutation authorization".into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceptance_campaign_passes() {
        let p = acceptance_campaign().unwrap();
        assert!(p.contains(&"CAT-15".to_string()));
    }

    #[test]
    fn authorization_campaign_passes() {
        let passed = authorization_campaign().unwrap();
        assert_eq!(passed.len(), 3);
    }

    fn manifest() -> BootstrapManifest {
        BootstrapManifest {
            cluster_id: ClusterId::derive("cluster-a"),
            voters: vec![
                NodeId::derive("n1"),
                NodeId::derive("n2"),
                NodeId::derive("n3"),
            ],
            trust_root_hash: Hash256([1; 32]),
            security_policy: SecurityPolicyId::derive("policy-1"),
            bootstrap_admin: PrincipalId::derive("admin"),
            security_profile: "DEV_LOCAL".into(),
        }
    }

    fn tenant() -> TenantId {
        TenantId::derive("t")
    }

    fn lock_cmd(id: &str, scope: &str, records: Vec<u64>) -> CatalogCommand {
        let recs: Vec<RecordId> = records.into_iter().map(RecordId).collect();
        CatalogCommand {
            admin_request_id: AdminRequestId::derive(id),
            principal: PrincipalId::derive("admin"),
            expected: vec![(
                CatalogKey::ScopeLock(tenant(), SemanticScopeId::derive(scope)),
                Expected::Absent,
            )],
            predicates: vec![Predicate::NoScopeOverlap {
                tenant: tenant(),
                records: recs.clone(),
            }],
            mutations: vec![Mutation::PutImmutable {
                key: CatalogKey::ScopeLock(tenant(), SemanticScopeId::derive(scope)),
                value: CanonValue::obj().fset("records", &recs).build(),
            }],
        }
    }

    #[test]
    fn genesis_then_cas_generation_and_idempotence() {
        let m = manifest();
        let mut c = Catalog::new();
        assert!(
            c.apply(1, &LogCommand::Genesis(m.clone()), Hash256::ZERO)
                .is_err(),
            "CAT-12: mismatching manifest refused"
        );
        c.apply(1, &LogCommand::Genesis(m.clone()), m.manifest_hash())
            .unwrap();
        assert_eq!(c.generation(), CatalogGeneration(1));
        assert!(
            c.apply(2, &LogCommand::Genesis(m.clone()), m.manifest_hash())
                .is_err(),
            "CAT-12: second genesis refused"
        );
        let cmd = lock_cmd("m1", "scope-a", vec![1, 2]);
        let r = c
            .apply(2, &LogCommand::Catalog(cmd.clone()), m.manifest_hash())
            .unwrap()
            .unwrap();
        assert!(r.failure.is_none());
        assert_eq!(r.catalog_generation, CatalogGeneration(2));
        // CAT-02: same command again (reply lost) → the same committed result, no new revision
        let again = c
            .apply(3, &LogCommand::Catalog(cmd.clone()), m.manifest_hash())
            .unwrap()
            .unwrap();
        assert_eq!(again, r);
        assert_eq!(c.generation(), CatalogGeneration(2));
        // same id, different bytes → IdentityConflict; the original result stays
        let mut other = cmd.clone();
        other.mutations.push(Mutation::Tombstone {
            key: CatalogKey::ClusterConfig,
        });
        let conflict = c
            .apply(4, &LogCommand::Catalog(other), m.manifest_hash())
            .unwrap()
            .unwrap();
        assert_eq!(conflict.failure.as_deref(), Some("IdentityConflict"));
        assert_eq!(c.result_of(&cmd.admin_request_id).unwrap(), &r);
        // CAT-01: an overlapping migration cannot take a second lock (phantom-safe predicate)
        let r2 = c
            .apply(
                5,
                &LogCommand::Catalog(lock_cmd("m2", "scope-b", vec![2, 3])),
                m.manifest_hash(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(r2.failure.as_deref(), Some("ScopeLocked"));
        assert_eq!(
            c.generation(),
            CatalogGeneration(2),
            "a failed CAS allocates no revision"
        );
        let r3 = c
            .apply(
                6,
                &LogCommand::Catalog(lock_cmd("m3", "scope-c", vec![7])),
                m.manifest_hash(),
            )
            .unwrap()
            .unwrap();
        assert!(r3.failure.is_none());
        // replay after restart: already-applied indices are no-ops
        assert!(c
            .apply(
                6,
                &LogCommand::Catalog(lock_cmd("m3", "scope-c", vec![7])),
                m.manifest_hash()
            )
            .unwrap()
            .is_none());
    }

    #[test]
    fn grant_lifecycle_routes_and_tombstones() {
        let m = manifest();
        let mut c = Catalog::new();
        c.apply(1, &LogCommand::Genesis(m.clone()), m.manifest_hash())
            .unwrap();
        let gid = GrantId::derive("home-grant");
        let home = RequestHomeId::derive("home-1");
        let grant = AuthorityGrant {
            grant_id: gid,
            binding: AuthorityBinding::RequestHome {
                home_id: home,
                epoch: RequestHomeEpoch(1),
            },
            tenant: tenant(),
            scope: SemanticScopeId::derive("inventory"),
            scope_records: vec![RecordId(1), RecordId(2)],
            plans: vec![],
            placement_epoch: PlacementEpoch(1),
            membership: (ReplicationGroupId::derive("g"), MembershipGeneration(1)),
            admitted_nodes: m.voters.clone(),
            authority_durability_policy: "quorum".into(),
            security_policy: m.security_policy,
            admission_mode: "ONLINE_BARRIER".into(),
            state: GrantState::Staged,
        };
        let stage = CatalogCommand {
            admin_request_id: AdminRequestId::derive("stage"),
            principal: PrincipalId::derive("admin"),
            expected: vec![(CatalogKey::Authority(gid), Expected::Absent)],
            predicates: vec![],
            mutations: vec![Mutation::PutImmutable {
                key: CatalogKey::Authority(gid),
                value: grant.to_canon(),
            }],
        };
        c.apply(2, &LogCommand::Catalog(stage), m.manifest_hash())
            .unwrap();
        // CAT-05: staged grant admits nothing
        assert!(!c.admission_allowed(gid, m.voters[0]));
        // CLOSED before ACTIVE is illegal
        let bad = CatalogCommand {
            admin_request_id: AdminRequestId::derive("bad"),
            principal: PrincipalId::derive("admin"),
            expected: vec![],
            predicates: vec![],
            mutations: vec![Mutation::AdvanceGrantState {
                grant: gid,
                from: GrantState::Staged,
                to: GrantState::Closed,
            }],
        };
        assert!(c
            .apply(3, &LogCommand::Catalog(bad), m.manifest_hash())
            .unwrap()
            .unwrap()
            .failure
            .is_some());
        let activate = CatalogCommand {
            admin_request_id: AdminRequestId::derive("activate"),
            principal: PrincipalId::derive("admin"),
            expected: vec![],
            predicates: vec![Predicate::GrantInState {
                grant: gid,
                state: GrantState::Staged,
            }],
            mutations: vec![Mutation::AdvanceGrantState {
                grant: gid,
                from: GrantState::Staged,
                to: GrantState::Active,
            }],
        };
        assert!(c
            .apply(4, &LogCommand::Catalog(activate), m.manifest_hash())
            .unwrap()
            .unwrap()
            .failure
            .is_none());
        assert!(c.admission_allowed(gid, m.voters[0]));
        assert!(!c.admission_allowed(gid, NodeId::derive("stranger")));
        // CAT-15: two homes race the first route claim; one CAS wins
        let route = |id: &str, h: &str| CatalogCommand {
            admin_request_id: AdminRequestId::derive(id),
            principal: PrincipalId::derive("admin"),
            expected: vec![(
                CatalogKey::RequestRoute(tenant(), RequestNamespace::derive("inventory"), 0),
                Expected::Absent,
            )],
            predicates: vec![],
            mutations: vec![Mutation::AdvancePointer {
                key: CatalogKey::RequestRoute(tenant(), RequestNamespace::derive("inventory"), 0),
                value: RequestRoute {
                    home_id: RequestHomeId::derive(h),
                    home_epoch: RequestHomeEpoch(1),
                    home_grant: gid,
                }
                .to_canon(),
            }],
        };
        assert!(c
            .apply(
                5,
                &LogCommand::Catalog(route("r1", "home-1")),
                m.manifest_hash()
            )
            .unwrap()
            .unwrap()
            .failure
            .is_none());
        assert_eq!(
            c.apply(
                6,
                &LogCommand::Catalog(route("r2", "home-2")),
                m.manifest_hash()
            )
            .unwrap()
            .unwrap()
            .failure
            .as_deref(),
            Some("CatalogCasConflict:present")
        );
        assert_eq!(
            c.route(tenant(), RequestNamespace::derive("inventory"), 0)
                .unwrap()
                .home_id,
            RequestHomeId::derive("home-1")
        );
        // close, retire, tombstone; CAT-16: a replayed old grant cannot be re-inserted
        for (id, from, to) in [
            ("closing", GrantState::Active, GrantState::Closing),
            ("closed", GrantState::Closing, GrantState::Closed),
            ("retired", GrantState::Closed, GrantState::Retired),
        ] {
            let cmd = CatalogCommand {
                admin_request_id: AdminRequestId::derive(id),
                principal: PrincipalId::derive("admin"),
                expected: vec![],
                predicates: vec![],
                mutations: vec![Mutation::AdvanceGrantState {
                    grant: gid,
                    from,
                    to,
                }],
            };
            assert!(c
                .apply(
                    10 + from as u64,
                    &LogCommand::Catalog(cmd),
                    m.manifest_hash()
                )
                .unwrap()
                .unwrap()
                .failure
                .is_none());
        }
        assert!(!c.admission_allowed(gid, m.voters[0]));
        let reopen = CatalogCommand {
            admin_request_id: AdminRequestId::derive("reopen"),
            principal: PrincipalId::derive("admin"),
            expected: vec![],
            predicates: vec![],
            mutations: vec![Mutation::AdvanceGrantState {
                grant: gid,
                from: GrantState::Retired,
                to: GrantState::Active,
            }],
        };
        assert!(
            c.apply(20, &LogCommand::Catalog(reopen), m.manifest_hash())
                .unwrap()
                .unwrap()
                .failure
                .is_some(),
            "a closed binding never returns to ACTIVE"
        );
        let tomb = CatalogCommand {
            admin_request_id: AdminRequestId::derive("tomb"),
            principal: PrincipalId::derive("admin"),
            expected: vec![],
            predicates: vec![],
            mutations: vec![Mutation::Tombstone {
                key: CatalogKey::Authority(gid),
            }],
        };
        assert!(c
            .apply(21, &LogCommand::Catalog(tomb), m.manifest_hash())
            .unwrap()
            .unwrap()
            .failure
            .is_none());
        let replay_old = CatalogCommand {
            admin_request_id: AdminRequestId::derive("replay-old-grant"),
            principal: PrincipalId::derive("admin"),
            expected: vec![],
            predicates: vec![],
            mutations: vec![Mutation::PutImmutable {
                key: CatalogKey::Authority(gid),
                value: grant.to_canon(),
            }],
        };
        assert_eq!(
            c.apply(22, &LogCommand::Catalog(replay_old), m.manifest_hash())
                .unwrap()
                .unwrap()
                .failure
                .as_deref(),
            Some("ArtifactIdentityConflict:tombstoned")
        );
        // canonical roundtrip of a command
        let cmd = lock_cmd("rt", "scope-x", vec![1]);
        assert_eq!(
            CatalogCommand::decode(&cmd.encode(), &Limits::v1()).unwrap(),
            cmd
        );
        let d1 = c.state_digest();
        assert_eq!(d1, c.clone().state_digest());
    }
}
