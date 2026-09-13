# SPEC-005 — C1/C2 Runtime

**Subtitle:** Semantic Replication, Idempotent Application and Causal Observations
**Status:** Draft 0.2 — proposed implementation contract; unimplemented and unverified
**Date:** 2026-09-09
**Depends on:** [SPEC-001](SPEC-001.md), [SPEC-002](SPEC-002.md), [SPEC-003](SPEC-003.md), [SPEC-004](SPEC-004.md)
**Integrates with:** [SPEC-006](SPEC-006.md), [SPEC-008](SPEC-008.md), [SPEC-009](SPEC-009.md), [SPEC-010](SPEC-010.md)
**Normative registries and protocols:** [SPEC-011](SPEC-011.md) (catalog, typed identity and authority), [SPEC-012](SPEC-012.md) (request identity, client protocol and codecs), [SPEC-013](SPEC-013.md) (security and trust)
**Reference implementation:** Rust stable; initially modules within `carolina-runtime`
**Normative terms:** MUST, MUST NOT, SHOULD and MAY state requirements of this draft.

## 1. Decision and scope

C1 SHALL replicate immutable, normalized semantic effects with stable identity and exactly one logical application at each participating replica. C2 SHALL add explicit dependency tracking and causal visibility. Neither protocol SHALL send physical B+Tree mutations from one replica and interpret them as the operation at another: `counter += 1` is replicated as a normalized increment, not as the origin's final `Put(counter, 17)` or physical journal bytes.

The local `CompiledBatch` of SPEC-002 remains the durability boundary. The receiving runtime translates the semantic effect into local storage mutations and protocol metadata in one atomic batch. Replica-local `VersionStamp` and `JournalLsn` values are never distributed order.

The MVP uses one fixed three-node, fully replicated replication group per supported domain and one local storage boundary for each operation. Physical sharding, partial replication, cross-group atomic operations and online membership changes are outside this protocol's first milestone. Such operations MUST use a separately admitted composition plan, normally SPEC-008. An IDC and a replication group remain distinct concepts even when their MVP placement happens to coincide.

C2 v1 session guarantees are scoped to one replication group. Read-your-writes, monotonic reads and causal dependencies across groups require a separately qualified composite contract and plan. Several group tokens collected by an SDK do not establish that contract. The compiler MUST reject a cross-group session requirement on this template, even when each individual operation otherwise qualifies for C2.

This specification supplies proof obligations and acceptance scenarios. It does not establish a theorem, report a completed implementation, guarantee bounded convergence time or claim a new CRDT algorithm.

## 2. Admission obligations

The runtime MUST load an immutable plan and validate its hashes, operation version, each typed `IdcBinding`, permitted participants and protocol capabilities against SPEC-011 admission evidence before admitting a new operation. The class label alone is insufficient. A stale catalog cache, placement address or larger unrelated generation cannot confer authority.

| Path | Required evidence in the admitted plan |
|---|---|
| C1 | Local invariant preservation; closure under supported concurrent effects; deterministic semantic application; compatible return contract; no unprotected dependency or authority requirement |
| C2 | The C1 obligations for unordered concurrent effects; explicit dependency relation; stable prerequisites; causal visibility and return contract |
| Neither | Arbitrary blind assignment, global exact reads, uniqueness, consumable bounds or revocable prerequisites without an independently verified plan |

Two increments may commute yet violate an upper bound. Two increments with `increment_and_get()` may preserve state yet fail a promised global ordering of returns. Neither fact permits automatic C1 admission. C2 cannot make a concurrent payment revocation safe merely because shipment has observed an earlier payment confirmation. The compiler must constrain revocation or choose another protocol.

If admission evidence is missing, the runtime MUST reject the path. It MAY route to an already published compatible stronger plan. It MUST NOT improvise strengthening that changes results, allows missing dependencies or crosses an unfenced authority boundary.

## 3. Identity and semantic records

The following are logical types, not Rust memory images or a finalized binary ABI. SPEC-011 owns identity types; SPEC-003 owns canonical IR; SPEC-012 owns wire, snapshot, receipt and token codecs, negotiation and bounded decoding. The wire protocol and snapshot format each carry an independent version. SPEC-012's version-1 conformance fixtures MUST fix field order, discriminants, byte order, lengths and hash inputs before interoperability is claimed.

```rust
struct OriginId {                 // exactly the SPEC-002 identity
    node_id: NodeId,
    origin_epoch: OriginEpoch,
    origin_seq: OriginSeq,
}

struct StreamId {
    group: ReplicationGroupId,
    node: NodeId,
    origin_epoch: OriginEpoch,
}

struct Dot { stream: StreamId, sequence: u64 }

struct StreamCoverage {
    prefix: u64,                 // every committed dot 1..=prefix is included
    intervals: Vec<(u64, u64)>,   // sorted, disjoint inclusive ranges above prefix
}

struct CausalContextV1 {
    format_version: u16,
    group: ReplicationGroupId,
    membership_generation: MembershipGeneration,
    streams: SortedMap<StreamId, StreamCoverage>,
}

struct SemanticCommitV1 {
    wire_version: u16,
    origin: OriginId,
    dot: Dot,
    txn_id: TxnId,
    request_key: RequestKey,
    request_hash: RequestHash,
    request_home_epoch: RequestHomeEpoch,
    operation: OperationRef,
    operation_hash: OperationHash,
    schema_hash: SchemaHash,
    contract_hash: ContractHash,
    plan_hash: PlanHash,
    plan_generation: PlanGeneration,
    idc_bindings: Vec<IdcBinding>,
    class: ConsistencyClass,
    dependencies: CausalContextV1,
    effects: CanonicalNormalizedEffects,
    effects_hash: Hash256,
    accepted_result: AcceptedResultV1, // immutable result material; SPEC-012
    durability_policy_id: Hash256,
    semantic_digest: SemanticDigest,
}
```

`origin_seq` is allocated monotonically for the node's origin epoch. `Dot.sequence` is allocated monotonically for its replication-group stream. The counters have separate namespaces even if an MVP allocates identical values. Allocation and the semantic commit are persisted atomically; a sequence promised by a committed record MUST NOT be reused. Abandoned in-memory reservations do not become causal prerequisites. If an allocator makes durable holes, it MUST also publish explicit authenticated no-effect coverage records; the MVP SHOULD avoid such holes by allocating at commit.

`OriginId` identifies a semantic commit. `RequestKey = (TenantId, RequestNamespace, StableRequestId)` identifies the client invocation; `TxnId` identifies its one mapped execution. `Dot` addresses dependency coverage. An `OriginEpoch` changes when its identity could otherwise be reused after destructive restore or replacement. It is different from `StorageEpoch`, `PlanGeneration`, `MembershipGeneration`, `IdcGeneration`, `IdcAuthorityEpoch`, `ResourceGeneration`, `EscrowEpoch` and `HolderAuthorityEpoch`. `IdcBinding` contains exactly `idc_id: IdcId`, `idc_generation: IdcGeneration` and `authority_epoch: IdcAuthorityEpoch`; no bare integer substitutes for any of these fields.

Every new invocation SHALL route to `RequestHome = route(RequestKey)` before allocation of `TxnId`, as defined in SPEC-012. The authenticated home performs one durable CAS from absent to `(RequestKey, globally unique TxnId, immutable request_hash)` before any runtime decision or effects. Concurrent first submissions with matching hashes join that mapping; changed content returns `RequestIdentityMismatch`. An allocated candidate that loses CAS cannot originate an execution. The hash binds the canonical operation/schema/contract identity, arguments, read contract and initial session input; it excludes plan choice and mutable routes.

Only that home may bind the execution authority and originate its first decision. Other nodes may resolve authenticated retained outcomes or forward the unchanged request; they MUST NOT originate another execution when the home or its mapping is unresolved. Moving a request home requires SPEC-011 registry authorization and a SPEC-009 fence/transfer of all retained mappings, outcomes and in-flight identities. `RequestHomeEpoch` is routing authority, not a timestamp or permission to allocate a second `TxnId`. C1 permits independent operations at multiple homes; this conservative MVP may lose availability for one request during home failure while other homes continue.

Replication relays MUST preserve the original origin, transaction, dot, effect bytes and digest. Relaying or locally replaying a record never produces a new origin. A duplicate identity with different canonical contents is an integrity fault, not a last-writer-wins update. Persisted digests detect identity conflicts; hashes alone do not authenticate a peer or prove semantic correctness.

## 4. Context algebra and bounded representation

Context membership means inclusion of a dot, not knowledge of its wall-clock time. `covers(A, B)` means every dot represented by B is represented by A. Join is exact set union followed by canonical interval compaction. Prefixes advance only across actually applied or snapshot-certified contiguous dots; receiving sequence 9 never implies receipt of sequences 1 through 8.

Contexts attached to observations MUST describe a causally closed cut: whenever an included event has dependency D, the cut covers D. C1 may apply independent arrivals out of sequence and retain sparse coverage. C2 may apply any ready event whose dependencies are covered. Neither requires a cluster-wide sequence. A client's program order is represented by its session context; the order of unrelated requests handled by one server is not automatically a semantic dependency.

The runtime SHALL configure finite limits for record bytes, context streams, interval count, dependency-wait bytes, retained inbox bytes and per-peer outstanding requests. Limits and active values MUST be inspectable. It MUST reject or backpressure before overflow. It MUST NOT truncate dependency contexts, replace holes with a maximum sequence or use an approximate summary that can falsely claim coverage. A future compact summary requires its own correctness argument and compatibility version.

## 5. Local origination

For a new C1/C2 invocation:

1. Authenticate and authorize the tenant, request namespace and named operation under SPEC-013. Resolve or durably create SPEC-012's `RequestKey -> (TxnId, request_hash)` mapping at its home; return the retained `FinalReceiptV1` for a matching terminal request and reject changed request content.
2. Validate plan admission and the current local fence. Pin the exact plan and retain it through commit.
3. Verify tenant, group, membership interpretation and token integrity before joining the client session context with operation dependencies. All v1 contexts must name the same group. Wait for local applied coverage. A C1 plan that cannot preserve the requested session contract MUST be rejected or routed to a compatible path; C1 is not permission to drop session dependencies.
4. Choose a durable local snapshot after the wait. Read prerequisite and return-driving values only from that snapshot. Record its required causal context. Validate arguments and generate deterministic normalized effects exactly once for this invocation.
5. Acquire short local application guards over affected keys and relevant protocol keys in canonical order. Recheck plan/fence and the preconditions required at commit. Rebase commutative deltas on the current committed state under these guards; do not overwrite it with a stale snapshot's computed value.
6. Allocate origin and dot. Commit business mutations, local index changes, the complete request/operation/plan/IDC binding, origin/transaction deduplication, immutable `AcceptedResultV1`, applied context and semantic outbox record as one SPEC-002 `CompiledBatch`. Its typed status record must retain exact accepted result bytes/digest and commitments atomically; a later independent dedupe/result-material write is forbidden. The final receipt is completed and persisted only after all required evidence exists under SPEC-012; it is not embedded recursively in the originating semantic digest.
7. Cross the configured local durable journal boundary. Publish all local effects and metadata together, release guards and advertise the immutable record to peers.
8. Return the final receipt only when its declared durability policy is satisfied. A matching retry returns the same outcome; it does not regenerate effects or recompute a newer return value.

Local guards are a storage implementation mechanism to prevent lost updates. They do not establish global serial semantics. SPEC-002's rule against universal same-key abort does not permit unprotected read-modify-write races.

No business effect, emitted internal fact or replication record is externally acknowledged before local durable commit. External services are outside this atomicity boundary. An outbox record does not provide exactly-once effects in an unrelated API.

## 6. Durable receive and atomic apply

Receivers use this state machine per semantic identity:

```text
UNSEEN -> RECEIVED_DURABLE -> WAITING_DEPENDENCIES -> READY_TO_APPLY
                                      ^                  |
                                      |                  v
                                      +--------- recheck dependencies
READY_TO_APPLY -> APPLYING -> APPLIED_DURABLE
UNSEEN / RECEIVED_DURABLE -> QUARANTINED on incompatible/corrupt content
```

`WAITING_DEPENDENCIES` and `READY_TO_APPLY` are derived scheduler states; a durable inbox allows their reconstruction. `APPLYING` is not a visible half-commit. A crash resolves it through the storage journal and applied identity record.

On delivery the receiver MUST validate bounded decoding, group identity, allowed source epoch, hashes and historical plan availability. It SHALL atomically store an inbox entry keyed by origin and its digest before returning `ReceivedDurable`. Duplicate receipt returns the existing status. An unknown plan MAY trigger catalog retrieval, but MUST NOT cause execution under the current plan by substitution.

Application SHALL:

1. Verify the record was a committed semantic operation from an admitted origin. For an old generation, require a SPEC-009 historical-replay/drain authorization; a current admission rejection alone does not justify dropping a previously committed effect.
2. Wait until local applied context covers dependencies. A durable receipt acknowledgment is not applied coverage and cannot satisfy a causal read.
3. Acquire local guards for all affected user, index and deduplication keys. Re-read the applied identity while guarded. Validate current protocol state against the pinned historical interpretation.
4. Apply the immutable semantic effects against current local materialization. Do not execute application code, re-run its business choice, generate IDs or consult local time/randomness.
5. Construct one local `CompiledBatch` retaining the original request key/hash, transaction, operation/schema/contract/plan identities and IDC bindings; resulting business/index mutations; origin digest and exact terminal outcome; dot coverage; inbox applied marker; and any outgoing relay cursor required for correctness. Replica-local batch identity does not replace the original invocation identity or result.
6. Commit through SPEC-002. Assign a fresh local `VersionStamp` and journal position. Advance applied acknowledgments only after durable publication of the complete batch.

If two ready effects read the same counter concurrently, guards or an equivalent validated atomic-update primitive MUST ensure the resulting value includes both deltas. A separate dedupe write after business state is forbidden. A duplicate that arrives during application MUST join the same completion or observe its durable outcome; it cannot start another mutation.

The protocol guarantees one logical application within the declared identity-retention contract. It does not claim exactly-once network delivery. An arithmetic exception on a valid admitted remote effect is a plan/runtime consistency failure: quarantine the domain and preserve the record for diagnosis. Silently skipping it or converting it to no-op is forbidden.

## 7. Session and read contracts

```rust
struct SessionTokenV1 {
    version: u16,
    tenant_id: TenantId,
    contract_hash: ContractHash,
    context: CausalContextV1,
    catalog_generation: CatalogGeneration,
    token_id: Hash256,
}

struct CausalObservationV1<T> {
    value: T,
    context: CausalContextV1,
    local_snapshot: VersionStamp,
    plan_hash: PlanHash,
    final_global_value: bool, // MUST be false on this read path
}
```

This is the logical group-context payload of SPEC-012's authenticated token envelope; SPEC-013 owns protection keys, issuer validation and rotation. Session tokens MUST be integrity protected and tenant/group scoped, or validated against server-held session state. A client may request additional waiting but cannot manufacture authorization, satisfied prerequisites or proof of a commit by supplying a token. Explicit application arguments remain distinct from authenticated observations.

`covers` and `join` reject different groups or membership interpretations without a certified SPEC-009 mapping. Reusing a G1 token at G2 returns `SessionScopeMismatch`, with no effect or false coverage. Opening an explicitly independent G2 session is permitted but does not carry G1 guarantees. A G1 write followed by a G2 write and G3 read has no cross-group read-your-writes, monotonic-read or causal guarantee under C2 v1. The same restriction applies to operation prerequisites and emitted contexts, not just public reads.

For `READ AT CAUSAL(token)`, the server MUST wait until applied state covers the token, then choose a snapshot whose atomically captured context also covers it. Reading first and attaching a later frontier is forbidden. The result carries that snapshot's context; the client joins it with its prior token within that same group. This implements monotonic reads and read-your-writes across node changes within the group when the required effects are reachable. Concurrent unrelated effects may become visible, and independent chains are not globally ordered.

Default session reads SHALL retain session guarantees. Explicit `AT LOCAL` permits a fresh local observation with its actual context, but MUST NOT silently erase the existing session token or claim it was satisfied. A query that requests an exact current global value, a multidomain consistent snapshot or a final business authorization MUST use a separately verified read/operation plan.

`remaining_stock = 5` observed causally is a statement about a cut. It is not a capability to consume five units. A return labeled final, such as successful creation or reservation under an admitted contract, remains an immutable outcome even as later reads differ.

On an unavailable dependency the runtime waits within configured bounds and returns `DependencyUnavailable` or a typed deadline result with no new execution. If local commit might already have happened, it returns `OutcomeUnknown(txn_id)` and resolves by identity. It MUST NOT report a committed operation as aborted because a client deadline elapsed.

## 8. Transport, acknowledgments and anti-entropy

Version-1 messages SHALL include `Hello`, `Inventory`, `FetchMissing`, `CommitDelivery`, `ReceivedDurable`, `AppliedDurable`, `SnapshotOffer` and typed error responses encoded and negotiated under SPEC-012. Each message binds cluster, tenant, group, typed membership generation, sender identity and format version. SPEC-013 peer authentication/authorization and SPEC-011 authority evidence are required independently of content hashes. Malformed or unsupported mandatory semantics and prohibited codec downgrades fail closed.

`Inventory` describes exact retained origin/dot coverage, applied coverage and snapshot coverage. It may use digests to find mismatches, but a digest mismatch requires exact reconciliation; absence of a mismatch is not an authority grant. Peers request missing records by bounded origin/dot ranges. Delivery is at least once. Retries preserve bytes and IDs. Separate per-peer cursors track received durability and applied durability.

The sender MUST persist the semantic outbox with the original local commit. A background worker recovers it after crash, sends pending records and persists progress. Receiver acknowledgments may be lost without losing data or duplicating effects. Anti-entropy SHALL periodically compare exact coverage and repair dropped deliveries. It MUST handle forwarded records and disconnected writers returning with valid retained commits.

Network messages are not required to be FIFO. The scheduler SHALL permit independent ready effects while a dependency chain waits, subject to fair bounded queues. Permanent disconnection, exhausted local storage or missing irrevocably lost history prevents a general liveness guarantee.

## 9. Durability and failover authority

The plan MUST state the failure scope of a success receipt. Initial supported policies MAY be:

| Policy | Completion condition | Limitation |
|---|---|---|
| LocalDurable | Origin's complete semantic batch has crossed its durable journal barrier | Covers process crash/restart with recoverable stable media; no claim of surviving permanent loss of that only copy |
| RequiredDurableCopies | Origin plus every member in the plan's explicit required-copy set has durably retained the semantic commit | Availability requires that set; interpretation under membership changes belongs to SPEC-009 |

`ReceivedDurable` can satisfy the copy requirement only if recovery can later reconstruct both payload and pending application, including referenced schema/plan artifacts. It cannot satisfy a causal visibility requirement. A late acknowledgment changes durability progress, never the immutable operation outcome. The final receipt is issued only at the policy boundary; a timeout after local commit is unresolved to the client, not permission to cancel or rerun it with a new ID.

C1/C2 peers replicate business facts, not universal writer authority. Promotion MUST establish current catalog membership, admitted source epoch and complete required data/context. A new origin epoch prevents identity collision; it does not erase unknown prior outcomes or authorize replacing lost escrow rights. Restoring an older snapshot MUST create a new storage epoch and enter reconciliation before serving writes or session reads.

An asynchronous lagging replica MUST NOT be advertised as a transparent failover target capable of satisfying all prior sessions. It may serve only contracts for which its actual state is ready. SPEC-006 defines the stricter rule for spend authority; generic replication never credits it.

## 10. Bootstrap and replacement

A bootstrap snapshot SHALL be a complete, causally closed logical image for the group, taken through a registered SPEC-002 snapshot. Its manifest contains:

```text
snapshot_format_version; group; membership_generation
source snapshot identity; schema/plan artifacts and hashes
business/index image; applied causal context
RequestKey-to-TxnId mappings or pinned authoritative mapping references
origin and transaction identity/exact outcome/commitment state
retired-origin floors and replay tombstones
protocol metadata required by the participating classes
retained in-flight inbox/outbox state or explicit replay boundary
chunk lengths/checksums; canonical logical-state digest
```

The source pins all needed MVCC and journal history until snapshot and catch-up consumers release it. Each chunk is bounded and checked. Receivers stage the image while not ready, validate completeness and causal closure, then atomically activate the image and its coverage. Receiving the manifest alone never advances an applied frontier.

The MVP SHALL install snapshots only into a new or fenced replacement store. Replacing live divergent state with a peer snapshot can erase local commits and is forbidden. A populated receiver must first reconcile/export every locally committed effect or use a separate proven merge/migration protocol. Catch-up replays records after the snapshot boundary through ordinary deduplicating apply.

Before READY, validate current catalog/fences, drain catch-up to the declared admission boundary, recover all needed identities and recheck requested session contexts. Copying protocol metadata does not confer active rights. Bootstrap cannot turn a backup into a second live escrow owner.

## 11. Retention and garbage collection

Journal retention SHALL include the minimum requirements of SPEC-002 plus semantic outbox consumers, pending causal dependencies, durable inbox reconstruction, bootstrap snapshots, outcome retry windows, membership transitions and escrow transfer evidence. A physical LSN is not sufficient to represent these horizons; the runtime MUST translate semantic requirements to pinned journal/snapshot resources.

Full applied identity records MAY compact into retained exact coverage plus immutable digest/outcome indexes when every future duplicate can still be safely classified. Receipt outcomes MUST remain replayable through the advertised retry lifetime. After a request identity expires, the system MUST return `IdentityExpired` and require a new explicitly distinct invocation; it MUST NOT silently treat an old ID as new.

For a retired origin, a durable retired-epoch tombstone/floor SHALL outlive any route that could reintroduce its old messages. Pruning these records requires a catalog barrier that excludes every affected holder and requires any later rejoin to bootstrap. Age, peer silence and maximum observed sequence are not safe GC evidence. Offline admitted members either pin history or pass through SPEC-009's explicit retirement protocol.

Monotonic-set membership itself is live business state. Removal/tombstone GC has no generic authorization in C1; a remove operation needs its own verified merge semantics and anti-resurrection horizon.

## 12. Plan transitions and readiness

Generation transition SHALL use SPEC-009. The baseline is barrier-first: each relevant origin/authority holder durably fences new admissions before acknowledging its drained committed boundary. A catalog update or timeout does not fence an offline holder. The transition remains blocked until its state and final outcomes are reconciled.

Historical commits within the certified drain boundary continue to replay under their exact old interpretation. The fence prohibits new invocations; it does not erase previously committed facts. New state, context mapping, retained outcomes, writer epochs and plan activation MUST become recoverably consistent before the new generation serves requests.

Readiness SHALL be scoped by group and capability:

```text
RECOVERING_LOCAL -> WAITING_PROTOCOL_RECONCILIATION -> CATCHING_UP
                -> READY_C1 / READY_C2
any state -> FENCED / READ_ONLY_SAFETY / QUARANTINED
```

`READY_C2` still admits a request only after checking that particular session context. Dependency absence is not necessarily whole-node failure. Corrupt required history, inconsistent duplicate digests, unsafe plan interpretation and unproven authority prohibit affected writes. Read-only safety may expose only readable snapshots whose declared guarantees can actually be satisfied.

## 13. Errors, backpressure and observability

Errors SHALL distinguish `StalePlan`, `StaleSchema`, `StaleGeneration`, `FencedOrigin`, `UnknownPlan`, `UnsupportedWireVersion`, `RequestIdentityMismatch`, `IdentityConflict`, `IdentityExpired`, `SessionScopeMismatch`, `DependencyUnavailable`, `DependencyLimit`, `QueueFull`, `OutcomeUnknown`, `NotReady`, `Corruption`, `DiskFull` and `ReadOnly`. `RequestIdentityMismatch` is the SPEC-012 client outcome for changed content under one request key; `IdentityConflict` also covers conflicting internal origin/dot records. Error payloads bind identity, stage and whether local commit is known committed, known absent or unresolved. Retry/resolve guidance MUST preserve the original `RequestKey` and request hash whenever outcome could exist, including when the client has not received its mapped `TxnId`.

The runtime MUST expose durable received/applied lag, missing dependency count, causal wait duration, inbox/outbox bytes, duplicate deliveries, identity conflicts, context intervals, replication bytes, snapshot pins, GC blocking reasons, outcome retention, readiness and active generations. Traces bind `TxnId`, `OriginId`, dot, plan hash, group, local version and phase. Business values and authorization secrets MUST NOT be logged by default. High-cardinality identity detail belongs in bounded diagnostic views, not unrestricted metric labels.

Backpressure MUST reserve enough disk/control capacity to finish already durable operations, reconcile dependencies and communicate progress. A full queue may refuse new work, but MUST NOT discard committed records, advance coverage falsely or acknowledge memory-only receipt. Fairness and wait bounds are qualification measurements; no throughput or latency target is implied here.

## 14. Acceptance scenarios and fault schedules

Every scenario SHALL check an independent reference history and the contract's returned outcomes, not just converged scalar values. Crash each local atomic transition before append, during append, before durable barrier, after barrier before publication and after publication before acknowledgment. Repeat recovery, deliver duplicates, and verify stable results.

| ID | Schedule | Required result |
|---|---|---|
| C12-001 | Two origins increment the same counter while partitioned; reconnect with permutations and duplicate delivery | Every committed delta appears once at each caught-up replica; supported invariants hold throughout |
| C12-002 | Deliver the same origin concurrently to two apply workers; crash between intended business and dedupe writes | One atomic application or none; no separate visible business/dedupe state |
| C12-003 | Deliver one identity with different effects, outcome or request hash | Reject/quarantine; original result stays unchanged |
| C12-004 | Deliver shipment before its payment dependency; allow an unrelated group/chain to run | Shipment invisible until dependency applied; unrelated ready work progresses |
| C12-005 | Deliver dot 9 while 7 is absent; encode/decode and compact coverage; request dependency 7 | No false coverage; exact holes survive crash and snapshot |
| C12-006 | Client writes at A, loses reply, retries at B with same ID; later sends a session read at C | At most one invocation; same outcome on resolution; read waits or returns typed unavailability until context covered |
| C12-007 | Race snapshot acquisition with applied-frontier advancement | Every returned context belongs to the chosen snapshot and is causally closed |
| C12-008 | Drop every delivery acknowledgment; crash sender; anti-entropy repair | Outbox recovers; duplicates remain harmless; final copy durability is never inferred from send completion |
| C12-009 | Bootstrap fresh node; interrupt at every chunk/activation step; replay snapshot-covered origins | No partial READY image and no double application |
| C12-010 | Attempt snapshot replacement while destination has unknown-to-source local commits | Refuse replacement until reconciliation; no lost acknowledged effect |
| C12-011 | Keep admitted peer offline; try journal/identity GC and replay old packet after rejoin | History pinned or peer explicitly fenced/bootstrapped; old message never becomes a new effect |
| C12-012 | Freeze generation while one holder is partitioned; deliver old committed records during drain | Migration blocks for missing holder; valid historical commits retained; new old-generation invocation rejected after local fence |
| C12-013 | Saturate context/inbox limits and inject disk full/fsync errors | Typed backpressure; no premature success, dropped obligations or partial batch |
| C12-014 | Promote a lagging replica; present a prior session token and escrow metadata snapshot | Causal read waits/fails truthfully; no implicit spend authority |
| C12-015 | Compile bounded concurrent decrement or global `increment_and_get` through C1 | Admission rejected unless an explicit matching proof/contract exists |
| C12-016 | Concurrent first submissions of one RequestKey at different ingress nodes; crash before/after home CAS | One recoverable mapping and at most one execution; retry without known TxnId resolves the same exact receipt |
| C12-017 | Write G1; supply its token to G2/G3 or compile a cross-group RYW/monotonic/causal contract | Typed scope rejection before effects unless a separately qualified composite plan owns the full requirement; no token truncation or implicit cross-group coverage |
| C12-018 | Reuse one request key with changed canonical arguments/contract/session input; replay its old receipt after migration | RequestIdentityMismatch for changed content; matching retries retain original result and commitments |
| C12-019 | Forge a token/peer identity or replay records with an IDC generation substituted for authority epoch; negotiate unknown mandatory codecs | Authentication, typed-binding or compatibility validation rejects before apply; hashes and numeric equality confer no authority |

## 15. Milestones and traceability

| Gate | Deliverable and exit criterion |
|---|---|
| R0 | SPEC-011/012/013 contracts available; freeze wire/context fixtures and a slow dot-set reference model; C12-003/005/019 pass |
| R1 | Semantic outbox/inbox and atomic deduplicating apply atop SPEC-002 integrated recovery; C12-001/002/008 pass |
| R2 | Causal scheduler, authenticated group sessions, request-home CAS and snapshot-context capture; C12-004/006/007/015–018 pass |
| R3 | Bootstrap, retention and bounded resource handling; C12-009/010/011/013 pass |
| R4 | Generation/failover integration and complete fault qualification; C12-012/014 plus all preceding tests pass before a distributed-correctness claim |

Gates are required future evidence. None is marked complete by publication of this draft. SPEC-010 owns reproducible simulator seeds, harness versions and result artifacts. Report convergence/availability only under stated communication, durability and scheduling assumptions; include stalled dependencies and background coordination costs.

| Source | Refinement in this document |
|---|---|
| SPEC-001 §§18–21, 26–29, 35–37 | Admission, semantic records, deterministic effects and explicit session observations (§§2–7) |
| SPEC-001 §§42–43, 48–52, 68–72, 85–86 | Fences, replay, failures and acceptance gates (§§9–15) |
| SPEC-002 §§7–9, 41–44, 66–71, 95–96, 100–105 | Separate physical/semantic identity and atomic durable apply (§§3–6, 8–11) |
| SPEC-002 §§88–90, 107–118 | Retention, generations and readiness (§§10–13) |
| [Research proposal](../PROPOSTA-DE-PESQUISA.md) §§1–2 and direction A | Observable contracts and retained final outcomes (§§2, 5, 7, 9, 12) |
| [Prior-art analysis](../research/consistency-prior-art.md), “Armadilhas” and E2–E4 | Commutativity is insufficient; honest liveness/durability comparisons (§§2, 7–9, 14–15) |

The defining invariant is: **visible semantic effects, their durable identity and their claimed causal coverage advance together, under the exact admitted contract.**
