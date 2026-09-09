# SPEC-011 — Catalog, Control Plane & Authority Registry

**Status:** Draft 0.1; implementation and qualification remain open.
**Date:** 2026-09-09
**Depends on:** [SPEC-002](SPEC-002.md), [SPEC-003](SPEC-003.md), [SPEC-004](SPEC-004.md), [SPEC-009](SPEC-009.md).
**Companion contracts:** [SPEC-012](SPEC-012.md) owns request identity and codecs; [SPEC-013](SPEC-013.md) owns authentication and trust.

## 1. Ownership and failure model

This specification owns the durable catalog, shared identity taxonomy, artifact registry, authority grants, admission fencing, and metadata recovery. SPEC-009 owns the migration procedure executed against these records. Runtime authorities own business decisions; a catalog publication is neither a business commit nor proof that an unreachable writer has stopped.

MUST, MUST NOT, SHOULD, and MAY describe the proposed design, not implemented behavior. The v1 control plane is one fixed three-voter Raft replicated state machine, tolerating one unavailable voter while a connected majority has intact durable state. It supplies linearizable catalog transactions and authoritative read barriers. Consensus safety assumes non-Byzantine members, persistent committed state, authenticated identities, and the consensus adapter's correct implementation. Progress additionally needs a communicating majority, eventual message delivery and leader stability. There is no bounded-clock or lease-based semantic revocation assumption.

A replica address, election victory, larger epoch, metadata quorum, checksum, or operator declaration alone MUST NOT revive missing authority or erase acknowledged work. Permanent loss exceeding an authority's durability policy causes affected scopes to remain unavailable pending evidence recovery. There is no `force=true` that reallocates uncertain escrow rights or fabricates transaction outcomes.

The catalog orders metadata, not all data operations. C1/C2/C3 authorities may continue under an already installed offline-capable grant while the catalog is unavailable, only within that grant's exact scope and policy. Its eventual replacement cannot activate until those issuers close. C4/C5 still require their own configured decision quorums.

## 2. Universal identity taxonomy

These are distinct nominal types. Arithmetic, comparison, serialization and lookup MUST retain the complete owner scope. Equal numeric payloads do not make two types interchangeable. Wire widths are owned by SPEC-012; counters MUST reject overflow rather than wrap. Stable IDs are never reused after deletion. Epoch/generation allocators persist their high-water marks with the record that consumes them.

| Type | Owner/scope | Meaning and advancement rule |
|---|---|---|
| `ClusterId` | One cluster genesis | Trust and identity namespace; a replacement cluster receives a new ID |
| `TenantId` | Cluster | Immutable tenant namespace; tenant names may change |
| `CatalogGeneration` | Cluster catalog | One committed catalog state update, including a new durable command result; monotonic revision, not data time |
| `PlanGeneration` | Logical plan lineage | Immutable successor plan publication, paired with `PlanHash`; unrelated lineages are incomparable |
| `IdcGeneration` | `IdcId` | Semantic domain definition, including invariant/dependency membership; changes when that definition changes |
| `IdcAuthorityEpoch` | `IdcId` | Incarnation of decision/admission authority; changes on authority replacement, not ordinary leader election |
| `PlacementEpoch` | Typed placement scope | Physical assignment of participants; does not by itself change semantic authority |
| `MembershipGeneration` | `ReplicationGroupId` | Version of admitted logical replication membership; separate from row/predicate membership and consensus term |
| `ResourceGeneration` | Invariant and canonical resource key | Resource definition/bound-accounting incarnation; change requires full accounting reconciliation |
| `EscrowEpoch` | `ResourceRef` | Conserved allocation-manifest lineage; changes only through closed, reconciled reallocation, never ordinary transfer or leader change |
| `HolderAuthorityEpoch` | `ResourceRef`, `HolderId` | Exclusive spending/transfer authority incarnation; replacement preserves the accounted allocation, not extra capacity |
| `StorageEpoch` | `StorageId` | One local durable storage history; destructive restore/replacement allocates a fresh incarnation |
| `OriginEpoch` | `NodeId` | One semantic origin allocator incarnation; fresh after possible counter rollback or origin replacement |
| `OriginSeq` | `NodeId`, `OriginEpoch` | Monotonic semantic-commit identity counter; relaying preserves the original value |
| `RequestHomeEpoch` | `RequestHomeId` and routing scope | Request identity/admission ownership incarnation; changes only through closed state transfer |
| `LocalCommitSeq` | `StorageId`, `StorageEpoch` | Local MVCC commit sequence; never a remote causality or authority timestamp |
| `SerialPosition` | `IdcBinding` | Ordered semantic position within that authority incarnation; not a consensus term/index or global clock |
| `RequestAllocationSeq` | `RequestHomeId`, allocation `RequestHomeEpoch` | Monotonic counter allocated atomically with the SPEC-012 request binding; never reused |
| `RecordRevision` | One typed protocol record key | Local/protocol CAS revision; not an authority epoch or global commit position |

`IdcEpoch` is not a normative type and MUST NOT appear in new persistent or wire records. Unqualified `authority_epoch: u64`, `idcs: Vec<(IdcId, u64)>`, and casts between the types above are forbidden at semantic interfaces. A containing record fixes the appropriate nominal epoch type; a heterogeneous authority reference uses a tagged union.

```rust
struct IdcBinding {
    idc_id: IdcId,
    idc_generation: IdcGeneration,
    authority_epoch: IdcAuthorityEpoch,
}
struct PlanRef {
    plan_id: PlanId,
    generation: PlanGeneration,
    hash: PlanHash,
}
enum AuthorityBinding {
    Idc { authority_id: AuthorityId, idc: IdcBinding },
    Holder { resource: ResourceRef, escrow_epoch: EscrowEpoch,
             holder_id: HolderId, epoch: HolderAuthorityEpoch },
    RequestHome { home_id: RequestHomeId, epoch: RequestHomeEpoch },
}
struct RequestKey {
    tenant_id: TenantId,
    request_namespace: RequestNamespace,
    stable_request_id: StableRequestId,
}
```

An IDC split creates new `IdcId`s with explicit predecessor mappings. A change to an existing IDC definition advances `IdcGeneration`; replacing its writer advances `IdcAuthorityEpoch`. A semantic migration may advance both. Updating addresses can advance only `PlacementEpoch`; moving executable authority also requires closure and the appropriate authority epoch. Leader election in an intact authority preserves its identity, outcomes and order.

Composite data participants bind the `ParticipantDescriptor` of SPEC-008: exact typed `idc_bindings`, `PlacementEpoch`, `MembershipGeneration` and descriptor hash. Votes, installation and publication evidence bind that descriptor hash. A data/storage role spanning IDCs MUST NOT introduce a generic authority epoch that hides those bindings. Catalog entry revisions remain `CatalogGeneration`; `RecordRevision` is for record-local state transitions outside that catalog revision domain.

`EscrowEpoch` is not an alias for `HolderAuthorityEpoch`. Transfers move conserved ownership within the same allocation lineage. Resource regeneration creates a new `ResourceRef`; its initial allocation epoch is separately explicit. None of these identifiers authorizes minting capacity. Origin and storage histories remain distinct even if allocated together on bootstrap.

`RequestHome = route(RequestKey)` is resolved before a `TxnId` exists. SPEC-012 owns the home-local durable CAS binding the key to one `TxnId` and immutable `request_hash`. The catalog stores routing scope and home authority, not one global serial decision per application request. C2 v1 session scope is one replication group; the catalog MUST NOT advertise a cross-group session guarantee absent a qualified composite plan.

## 3. Catalog records and keys

Every key is prefixed with `ClusterId`; every tenant-owned key additionally carries `TenantId`. Names are secondary indices, never authority keys. Records include format version, key, immutable identity, revision and lifecycle state. A deleted key retains a tombstone/reuse prohibition.

```text
CatalogEntry<T> {
  key: CatalogKey
  revision: CatalogGeneration
  state: LIVE | TOMBSTONED
  value: T
}
CatalogKey =
  ClusterConfig
  | Artifact(ArtifactKind, ArtifactHash)
  | Operation(TenantId, OperationId, OperationVersion)
  | Plan(TenantId, PlanId, PlanGeneration)
  | ActivePlan(TenantId, SemanticScopeId)
  | Idc(TenantId, IdcId, IdcGeneration)
  | ActiveIdc(TenantId, IdcId)
  | Placement(TenantId, PlacementScopeId)
  | Membership(ReplicationGroupId, MembershipGeneration)
  | Node(NodeId)
  | Authority(AuthorityBinding)
  | RequestRoute(TenantId, RequestNamespace, RoutingBucketId)
  | Namespace(TenantId, RequestNamespace)
  | Migration(MigrationId)
  | ScopeLock(TenantId, SemanticScopeId)
  | RetentionPin(PinId)
  | SecurityPolicy(SecurityPolicyId)
  | AdminCommand(AdminRequestId)
```

Definitions and historical plans are immutable. `ActivePlan`, `ActiveIdc`, placement and route pointers are CAS-updated references to registered immutable state. `ScopeLock` contains the conservative semantic closure from SPEC-004/009, not merely an exact string whose equality might miss overlapping ranges.

```text
AuthorityGrant {
  binding: AuthorityBinding
  grant_id: GrantId
  scope: SemanticScopeManifest
  scope_digest
  plans: SortedSet<PlanRef>
  placement_epoch: PlacementEpoch
  membership: (ReplicationGroupId, MembershipGeneration)
  admitted_nodes: SortedSet<NodeId>
  authority_durability_policy_id
  transfer_decision_durability_policy_id: Optional<PolicyId>
  security_policy_id
  admission_mode: ONLINE_BARRIER | PINNED_OFFLINE
  state: STAGED | ACTIVE | CLOSING | CLOSED | RETIRED
  installed_evidence[], close_evidence[], successor_ref
}
NodeRecord {
  node_id; public_identity_refs[]; credential_status
  software_build_hash; capability_manifest_hash
  storage_histories[]; replication_memberships[]
  readiness_by_scope; observed_catalog_generation
}
```

`AuthorityDurabilityPolicy` and `TransferDecisionDurabilityPolicy` are independent of a client result's durability contract. They identify required replication groups, tolerated failures, required acknowledgements and recovery evidence. A local durable result cannot lower the configured durability of rights ownership or transfer decisions. The runtime protocol defines when each acknowledgement becomes legal.

## 4. Catalog transaction/CAS contract

```text
CatalogCommand {
  admin_request_id: AdminRequestId
  immutable_command_hash
  authenticated_principal: PrincipalId
  expected: [(CatalogKey, ABSENT | ExpectedRevisionAndDigest)]
  predicates: [RegisteredStateMachinePredicate]
  mutation_set: [PutImmutable | AdvancePointer | AdvanceState | Tombstone]
}
CatalogCommit {
  admin_request_id; command_hash; catalog_generation
  changed_key_revisions[]; committed_log_ref; result
}
```

The leader authenticates and authorizes the action under SPEC-013. At deterministic state-machine application, the catalog rechecks all expected revisions, security-policy revision, scope overlap predicates, capabilities and referential integrity against one committed state. Validation and every mutation occur atomically. A new durable command result increments `CatalogGeneration` once and persists with `AdminCommand`; successful target-key changes receive that same revision. A failed CAS changes no target key but retains its idempotent result. A duplicate resolved command does not allocate another revision. Reusing `AdminRequestId` with different bytes returns `IdentityConflict`.

The expected set MUST include every predicate dependency capable of changing the decision. Overlap detection is a state-machine predicate over the semantic scope index, preventing insertion phantoms; checking only previously found locks is insufficient. Unknown overlap conflicts conservatively. No user-provided arbitrary predicate code runs inside consensus.

Two migrations touching the same closure cannot both acquire ownership. A plan activation racing a grant, restrictive policy update, namespace retirement or capability withdrawal either serializes compatibly or fails its CAS. The loser rereads and explicitly replans; it MUST NOT drop failed conditions and retry the mutation blindly.

Lost command response is resolved by `AdminRequestId` through a read barrier. A client timeout neither proves failure nor permits a new conflicting command. Catalog command IDs obey durable anti-reexecution retention or retired administrative namespaces just as business request identity does.

## 5. Reads, watches and admission

`read_barrier()` returns an authoritative committed catalog frontier under the consensus adapter. A serving replica waits until it has applied that frontier before returning a current catalog answer. A cached or follower read is explicitly labelled `observed_generation` and cannot independently establish absence of a lock, current revocation, activation success or complete recovery.

Watches deliver revisions with at-least-once semantics. A missed/compacted interval requires a fresh snapshot and barrier. Notifications are hints; runtime admission does not depend on their timely arrival.

Before a new effect, the responsible authority MUST serialize this check with its own durable admission/closure boundary:

1. Validate authenticated caller, tenant, operation permission and the installed security policy.
2. Resolve the request through SPEC-012; existing requests remain bound to their original execution and exact outcome.
3. Match exact plan/schema/contract hashes and `IdcBinding`s; verify the registered grant, placement, membership and declared durability profiles.
4. Verify the node is a permitted executor with the required artifacts, codecs, complete state and protocol readiness.
5. Verify that the local durable fence does not prohibit the invocation. For an ordered authority, obtain its own consensus authority evidence; a cached leader address cannot pass this check.
6. Persist admission identity/evidence atomically with the decision or the durable in-flight record required by the selected protocol.

`ONLINE_BARRIER` obtains a current catalog/policy barrier for admission. `PINNED_OFFLINE` uses an installed grant and policy whose restrictions cannot be declared effective until every authorized issuer has closed. The latter permits continued old operations while a catalog change is pending. An offline grant is explicit availability policy, never a stale-cache approximation of an online grant.

The critical race has only two outcomes: admission precedes the durable close boundary and is included in its admitted-request frontier; or closure precedes admission and the request is refused before effects. A gateway's earlier check does not replace this check at the authoritative runtime/storage boundary. Local fences survive restart and raw-log compaction.

## 6. Artifact publication and capabilities

The registry owns exact immutable schema, operation, contract, invariant, IR, plan, transition, capability and qualification artifacts. Each entry binds kind, format version, domain-separated hash, bounded length, content location, durable availability evidence, dependencies and qualification status. SPEC-003 owns canonical IR hashes; SPEC-012 owns wire/persistent encoding. Same key with different bytes is corruption, not an update.

Registration is side-effect free with respect to business execution. A candidate plan passes `REGISTERED -> VALIDATED -> STAGED -> ACTIVE -> RETIRED`; activation is either genesis initialization over proven empty state or the SPEC-009 activation CAS. `VALIDATED` records explicit supported assumptions and qualification evidence; a hash or a compiler result does not establish proof.

Activation requires durable availability of every referenced artifact and supported codec/runtime capabilities at every required target, plus matching installation evidence. The registry may store bytes in the catalog or immutable replicated blobs; external blob references MUST be pinned and satisfy the declared durable failure model before their catalog pointer commits. A URI to an unverified mutable file is not an artifact.

Nodes announce immutable capability manifests covering protocol families, wire/snapshot versions, numeric/IR features and storage/durability features. Self-announcement does not qualify a feature. The catalog accepts only combinations allowed by the versioned qualification matrix. Unavailable capability evidence blocks activation; the compiler cannot infer compatibility from a higher software version string.

Downgrading/removing a capability required by active grants is rejected or requires closure/migration first. A restarted node with a different build re-enters passive readiness checks. Historical artifacts may remain decodable for recovery even when prohibited for new admission.

## 7. Authority, migration ownership and fencing

Authority lifecycle is `STAGED -> ACTIVE -> CLOSING -> CLOSED -> RETIRED`. Only a staged grant may be cancelled without closure. A closed binding never returns to ACTIVE; resumption uses a fresh authority incarnation and a forward migration. Successors cannot activate simply because a predecessor reached `CLOSING`.

The `MigrationRecord` and immutable participant manifest of SPEC-009 are catalog-owned records. A coordinator is a worker, not the owner of an independent decision log. A durable worker claim includes a migration revision and fresh claim ID. Recovery workers can replace a claim by catalog CAS; stale workers cannot advance phases because their expected claim/revision fails. Phase commands at participants are idempotent by migration ID, phase and immutable terms. Duplicate workers cannot bypass an already installed close fence.

Closure evidence binds the exact `AuthorityBinding`, migration ID, recoverable admitted-request set/frontier, durable fence, pending decisions/publications, rights transfers, exact outcomes and relevant semantic-origin coverage. An epoch increase without this evidence is not revocation. An administrative node/credential revocation blocks new authenticated access where observed but cannot prove closure of an offline semantic writer.

Activation is one catalog CAS over source active pointers, locks, migration phase, all closure/drain/validation/install evidence and target capability/policy revisions. It publishes target active pointers and marks predecessor bindings closed/retiring atomically. Target processes persist activation against their installed state before opening admission. The publication may reach different targets at different times; closed sources and unready targets prevent conflicting execution.

No separate placement, membership, request-home or security procedure may circumvent this serialization. Physical addresses can change within an intact authority only when identity, durable state and its consensus membership remain valid. Data copied onto a new disk is passive until these checks succeed.

## 8. Membership and bootstrap

The initial profile has fixed three-voter membership. A leader election within that configuration preserves the committed catalog and does not allocate new semantic authority epochs. Dynamic voting reconfiguration is unsupported until a separate qualified consensus reconfiguration procedure defines intersecting transition quorums, snapshot transfer and old-voter fencing. Merely publishing a new voter list is forbidden.

Node enrollment requires an authenticated administrator, a registered immutable `NodeId`, proof of the configured key identity, expected `ClusterId`, admitted build/capability manifest and passive state. Readiness follows snapshot/log recovery, current catalog barrier, restoration of all fences, artifact availability and per-scope state verification. Enrollment confers no writer/holder authority.

Genesis is a single explicit, idempotent initialization command over empty catalog storage and an operator-pinned bootstrap manifest containing cluster ID, three voter identities, trust roots, policy IDs and bootstrap administrator identity. All voters verify the same manifest hash. The first durable quorum establishes genesis before any grant becomes active. Competing manifests for initialized storage are rejected; discovery or a network partition cannot auto-create a second cluster.

Bootstrap authority is consumed by that genesis identity and disabled for normal administration. Losing the bootstrap response resolves by its manifest/command ID. Starting three independent empty stores with matching friendly cluster names is not cluster recovery.

## 9. Catalog snapshots, restore and GC

```text
CatalogSnapshotManifest {
  cluster_id; snapshot_id; format_version
  catalog_generation; committed_consensus_boundary
  consensus_configuration; entry_set_digest
  allocator_high_water_marks; immutable_artifact_manifest
  active_and_retired_authority_bindings
  migration_locks_and_phases; request_route_history
  security_policy_and_key_history; pins_and_retirement_floors
}
```

A snapshot is a consistent committed cut; it contains equivalent state for every compacted record needed by recovery. Local installation is atomic under SPEC-002, then consensus log suffix application resumes. Downloading/validating a snapshot never enables admission. Unknown mandatory formats, missing blobs, inconsistent digests or incomplete fences quarantine the node.

Restoring a stale backup into a live cluster begins passively with new storage/origin incarnations and reconciles against its authoritative current catalog and decision histories. An isolated old backup cannot elect itself into replacement authority. If no safe authoritative quorum or equivalent recoverable history exists, the affected cluster remains unavailable; a separately initialized cluster has a different `ClusterId` and cannot claim the old outcomes or rights were preserved.

Pins cover active/historical plans, migrations, request homes and retired namespaces, exact results/tombstones, prepared decisions/publications, transfer ledgers, origin coverage, snapshots, replica bootstrap and backup dependencies. The catalog records owner, reason, protected identity range, evidence frontier and release condition for each pin. A pin is released by a verified state transition, not an elapsed worker timeout.

GC first commits a reclaim plan conditioned on every relevant pin/revision and retention horizon. It then deletes only objects named in that plan. Concurrent new pins cannot name already reclaiming data; they must establish a newer recoverable cut or fail. Interrupted deletion is idempotent and cannot remove live references. Hash tombstones, allocator high-water marks, retired authority/namespace floors and equivalent anti-reexecution records survive raw-log deletion.

Finite result retention is permitted only with preserved non-reexecution identity under SPEC-012. Old nodes below a retirement floor must rebootstrap. Old backups remain unusable as active writers even after their referenced raw logs are reclaimed. Storage pressure yields backpressure or explicit unavailability, not deletion of unresolved authority evidence.

## 10. Required errors and diagnostics

Errors include `StaleCatalogGeneration`, `CatalogCasConflict`, `CatalogUnavailable`, `ScopeLocked`, `CapabilityUnsupported`, `ArtifactUnavailable`, `ArtifactIdentityConflict`, `UnfencedAuthority`, `AuthorityStateIncomplete`, `TargetNotReady`, `StaleAuthorityBinding`, `NamespaceRetired`, `UnsupportedMembershipChange`, and `RecoveryEvidenceMissing`. They distinguish whether a command is known final, uncommitted or outcome-unknown and identify the safe resolver.

Inspection shows catalog frontier, applied lag, each grant and durable fence, missing closure/install evidence, policy activation state, worker claim, artifact qualifications and retention pins. Metrics use bounded labels; high-cardinality IDs belong in authenticated traces. Secret material and business arguments/results are omitted by default.

## 11. Acceptance and release gates

| ID | Schedule | Required result |
|---|---|---|
| CAT-01 | Two overlapping migrations pass concurrent preflight | At most one obtains the semantic scope lock; no predicate phantom |
| CAT-02 | Catalog commit succeeds, reply lost, leader crashes | Same admin command resolves to the one committed result |
| CAT-03 | Old gateway races holder close | Invocation is in the recoverable admitted set or refused before effects |
| CAT-04 | Offline holder has rights; majority publishes proposed replacement | Replacement remains inactive until closure/reconciliation |
| CAT-05 | Target has all data but no activation evidence | Target cannot admit new work |
| CAT-06 | Stale catalog follower reports no migration lock | Authoritative CAS/barrier prevents conflicting activation |
| CAT-07 | Capability withdrawal races plan activation | One valid serial order; no active unsupported target |
| CAT-08 | Same `IdcGeneration` numeric value used as authority epoch | Typed interface/codec schema rejects substitution |
| CAT-09 | Restore old catalog snapshot after rights transfer and request completion | No authority resurrection or request reexecution |
| CAT-10 | Worker replaced while its old phase message is delayed | Same migration resumes; stale claim cannot advance state |
| CAT-11 | GC release races new snapshot/request-history pin | Atomic reclaim conditions preserve a recoverable cut |
| CAT-12 | Bootstrap manifests disagree or initialized voter loses contact | No second genesis under the same cluster identity |
| CAT-13 | Authority disk loss exceeds configured durability | Missing rights/outcomes remain frozen and explicitly unavailable |
| CAT-14 | Change node certificate while retaining intact authority | Identity rotates without minting rights or resetting epochs |
| CAT-15 | Two homes receive first invocation with same request key | SPEC-012 route/CAS admits one mapping; route epoch alone cannot fork it |
| CAT-16 | Old signed grant replayed after tombstone/log GC | Retired binding is refused; historical decision lookup remains distinct |

Before distributed-correctness claims, the catalog/migration fencing state machines require SPEC-010's mandatory formal gate, deterministic simulation and real-process fault campaigns. Model assumptions, explored bounds, unsupported transitions and non-passing results are retained with the qualification artifact. A working three-node happy path does not satisfy this gate.
