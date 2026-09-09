# SPEC-008 — Serial IDC Runtime

**Subtitle:** Ordered Execution, Distributed Decisions and Atomic Publication  
**Status:** Draft 0.2 — proposed implementation contract; not an implemented or proven protocol  
**Date:** 2026-09-09  
**Type:** Distributed runtime specification  
**Depends on:** [SPEC-001](SPEC-001.md), [SPEC-002](SPEC-002.md), [SPEC-003](SPEC-003.md), [SPEC-004](SPEC-004.md)  
**Protocol interfaces:** [SPEC-005](SPEC-005.md), [SPEC-006](SPEC-006.md), [SPEC-007](SPEC-007.md), [SPEC-009](SPEC-009.md)  
**Qualification:** [SPEC-010](SPEC-010.md)  
**Normative registries and protocols:** [SPEC-011](SPEC-011.md) (catalog and typed authority), [SPEC-012](SPEC-012.md) (request identity, receipts and codecs), [SPEC-013](SPEC-013.md) (security and trust)  
**Reference implementation:** Rust stable; consensus and sequencing outside `astra-storage`  
**Normative terms:** MUST, MUST NOT, SHOULD, SHOULD NOT and MAY express requirements on a conforming implementation.

---

## 0. Decision

C5 SHALL execute the affected Invariant Dependency Component as a replicated, deterministically ordered state machine. The initial distributed implementation SHALL use a Raft-backed consensus adapter per serial IDC authority. Several authorities MAY share processes and storage nodes; they retain separate logical identity and order.

An operation's semantic order is distinct from local journal order and physical MVCC order. The storage B+Tree SHALL NOT implement leader election, consensus, distributed locking or transaction decision recovery.

Atomic transactions spanning physical participants SHALL use an explicit durable prepare/decision/publication protocol. This includes one IDC spanning multiple shards and several IDCs in one transaction. Atomic local batches alone do not provide distributed atomicity or a coherent distributed read.

This draft selects a conservative blocking baseline. It provides no bounded completion guarantee during arbitrary partitions. Its success criterion is preserving the declared invariant and observable contract through crash, retry, partition and generation changes.

## 1. Guarantees and scope

Within a supported C5 IDC, completed registered operations SHALL be equivalent to a sequential execution of their exact versioned contracts. Final operations submitted through the authority SHALL respect real-time completion-before-invocation order within that IDC. An authoritative read barrier is required to claim that property for reads.

No cluster-wide order is implied across independent IDCs. A multi-IDC operation establishes the necessary order in every affected IDC through the composite protocol below. Independent operations remain unordered unless their contract says otherwise.

Successful mutation means:

```text
valid sequential transition
AND unique replicated transaction decision
AND recoverable participant effects
AND atomic public visibility under the declared read contract
AND durable stable result and issued commitments
```

The contract is not satisfied merely by preventing negative balances while silently changing a returned allocation or canceling a confirmed reservation after failover.

The initial fault model is crash-stop/crash-recovery; message loss, duplication and reordering; arbitrary delay and partition; and the storage failure model of SPEC-002. Byzantine actors, total permanent loss of every configured durable copy and atomic external API effects are outside the guarantee. Hashes/checksums do not turn this into a Byzantine protocol.

## 2. What C5 can and cannot fix

C5 is a candidate fallback when another template's safety is unproven and a supported safe sequential execution exists. Protocol families are not a total order; SPEC-004 selects a safe plan from the qualified finite library under its declared cost function. Serialization cannot repair a contract whose single operation breaks an invariant, returns an unauthorized promise, overflows silently or calls an unknown evaluator.

The compiler MUST reject rather than select C5 if it cannot construct an executable validator and complete semantic boundary. An arbitrary predicate may execute serially only when its deterministic evaluator and necessary state scope are supported and bounded. The statement in SPEC-001 §18 that unsupported invariants use C5 is conditional on those requirements.

An argument precondition concerns input admissibility. A state precondition is evaluated against the serialized pre-state. A postcondition relates that state, the new state and returned result. A declared invariant applies to all reachable committed states regardless of an individual operation's `ENSURE` clause.

For example, `REQUIRE status == OPEN` permits a later legitimate close. It does not establish a permanent `status == OPEN` invariant. Conversely, `ENSURE balance >= 0` on one operation does not cover unrelated uncompiled writers. Every invariant-affecting mutation must enter an authorized path.

Final state-dependent rejection is also an observable decision: it must be based on the specified serialized state and recorded for exact retry. A later increase in inventory cannot change a previously final rejected request into a successful retry under the same identity.

## 3. Boundaries between components

| Component | Owns | Must not infer |
|---|---|---|
| Compiler / SPEC-011 catalog | Contract, invariant closure, plan, typed IDC mapping, authority registry and published generations | Successful execution merely from a plan hash |
| IDC runtime | Admission, semantic order, locks, evaluator and exact result | Distributed durability from a local fsync |
| Consensus adapter | Leadership, committed authority log, quorum evidence and ordered application | Business validity of an opaque command |
| Transaction decision authority | One sealed participant set and one final outcome/publication record | A missing participant response means abort |
| Data authority | Replicated prepared payloads, local install and publication markers | A yes vote is a commit decision |
| Astra storage | Prepare, local atomic batch, journal durability, MVCC and recovery | A local commit is globally published |

Consensus traffic MAY be multiplexed, but one unrelated hot IDC must not automatically establish a global transaction ordering bottleneck. The v1 topology uses three nodes and fixed voting membership; no automatic elastic scaling is required.

## 4. Canonical records

These records are conceptual interfaces using SPEC-011's strong types, not a frozen binary format or public RPC schema. SPEC-012 owns explicit versions, canonical encodings, size bounds, compatibility fixtures and client receipt codecs. SPEC-013 owns authenticated evidence, peer identity, authorization and downgrade protection. All three contracts are admission/release dependencies.

```text
IdcAuthority {
    idc_binding: IdcBinding
    semantic_membership_hash
    authority_id
    placement_epoch: PlacementEpoch
    membership_generation: MembershipGeneration
    consensus_config_id
    voting_members
    physical_participants
    admitted_plan_generations: [PlanGeneration]
    recovery_state
}

SerialOrder {
    idc_binding: IdcBinding
    semantic_position: SerialPosition
}

SerialCommand {
    txn_id: TxnId; request_key: RequestKey; request_hash: RequestHash
    request_home_epoch: RequestHomeEpoch
    operation: OperationRef; operation_hash: OperationHash
    contract_hash: ContractHash; schema_hash: SchemaHash
    plan_hash: PlanHash; plan_generation: PlanGeneration
    idc_bindings: [IdcBinding]
    canonical_arguments
    admission_permit
    sealed_membership_hash; declared_footprint
    causal_dependencies; read_contract_hash
}

SerialExecutionRecord {
    command_digest; serial_order
    validated_prestate_frontier
    effects_digest; result_digest; commitment_digest
    normalized_effects
    stable_result_bytes_or_durable_reference
    participant_set_hash
    decision_reference; publication_reference
}
```

`SerialPosition` is allocated by the IDC authority and scoped to the complete `IdcBinding = (IdcId, IdcGeneration, IdcAuthorityEpoch)`. A consensus term/index pair is evidence for an authority-log entry, not a replacement for `SerialPosition` or `LocalCommitSeq`. Gaps caused by control/abort records are legal and explicit. The complete serial-order identity MUST NOT be reused across destructive restore, IDC generation change or authority replacement; a scalar position is never compared across bindings without a verified transition mapping.

The stable client `RequestKey = (TenantId, RequestNamespace, StableRequestId)` determines `RequestHome = route(RequestKey)` before allocation of `TxnId`. SPEC-012's home durably CAS-creates exactly one mapping to a globally unique transaction and immutable request hash. That hash covers operation/schema/contract identities, canonical arguments and original session/read contract; it excludes plan choice and mutable routing addresses. Result and effect digests bind the chosen execution. Changed content under one request key returns `RequestIdentityMismatch` before work is admitted; an unknown mapping or execution cannot be bypassed by a new transaction identity.

## 5. Authority durability and sequencing

The consensus adapter SHALL expose conceptual operations:

```text
propose(command) -> committed authority-log reference
read_barrier() -> current committed authority frontier
await_applied(frontier)
read_durable_state(key)
snapshot_authority_state()
```

The adapter MUST guarantee a single committed log prefix under its stated quorum configuration and prevent a stale leader from authorizing new effects. Proposal acceptance by one process and local journal flush are not `propose` success.

The authority log SHALL contain sufficient bytes to reconstruct ordered admission, sealed membership, deterministic commands/effects/results, reservations, votes, decisions, publication markers, deduplication and transition fences. A checkpoint must retain the equivalent state and references before its log prefix is compacted.

Followers apply committed records in order and never acknowledge a serial read from a speculative or merely received prefix. A leader must pass an authoritative read barrier before serving a current serial read; wall-clock leases without a separately specified bounded-clock proof are unsupported in v1.

Leader election term and `IdcAuthorityEpoch` are different. Election within the same consensus authority preserves prepared transactions and semantic order. `IdcGeneration` versions the semantic domain; `PlacementEpoch` versions placement; `MembershipGeneration` versions the configured member set; `CatalogGeneration` and `PlanGeneration` have their separate SPEC-011 meanings. Moving authority or changing an IDC boundary requires a SPEC-011 registry decision and SPEC-009 fencing/state transfer; it cannot be implemented by deleting the old group's prepared metadata. Catalog leader changes do not grant runtime leadership or fence offline holders.

## 6. Stable IDC membership

IDC identity represents semantic interaction, not the set of rows currently found by one scan. A scoped domain such as `Department[d] + all Expense rows whose department_id=d` includes future inserts and moves across that group boundary.

Before admission, the runtime SHALL resolve a complete conservative footprint through the catalog's current generation. The membership record contains predicate/index/membership domains needed to prevent rows entering or leaving the operation's dependency set unnoticed.

The transaction seals:

```text
sorted typed IdcBindings (IdcId; IdcGeneration; IdcAuthorityEpoch)
semantic membership hashes
authority IDs; typed placement epochs and membership generations
physical participant groups and durability policy
contract/plan identity
```

Changes to partition keys, grouping fields or references that can alter this membership SHALL execute under a membership-domain fence. Physical relocation is coordinated with SPEC-009. Newly discovered participants after a durable prepare cannot be appended opportunistically to the participant list.

If the complete set cannot be determined, the runtime may lock a conservative enclosing domain and retry discovery under that lock. If no finite supported enclosing scope exists, return `UnsupportedFootprint`. An operation is not safely routed merely because the gateway has found its current write keys.

## 7. Lock hierarchy and deterministic acquisition

The runtime uses a logical lock hierarchy separate from B+Tree page latches:

```text
IDC root gate
    invariant / membership / predicate domain
        canonical key or interval
```

Root modes are shared (`S`), intention-exclusive (`IX`) and exclusive (`X`). `S` is compatible with `S`; `IX` is compatible with `IX`; `X` conflicts with everything; `S` conflicts with `IX`. Domain-level shared reservations coexist; exclusive reservations conflict with overlapping shared or exclusive reservations. Root `X` excludes every descendant mutation/reservation.

The C4 path uses `IX` plus SPEC-007 domain reservations. A C5 operation uses root `X`. The v1 multi-IDC atomic path uses root `X` for each participant IDC. A coordinated snapshot uses root `S` for every requested IDC until its snapshots have been captured.

Acquire by ascending canonical `IdcId`; within an IDC, by domain ID and then canonical interval lower bound/key. Combine duplicate requests into the strongest required mode before acquisition. The lock footprint and participant set must be sealed before acquiring the first lock.

No transaction may hold a later-ranked lock while waiting for an earlier-ranked lock. Lock-mode upgrade is prohibited while holding lower-ranked resources that would violate this order. Expand-and-retry must first obtain an authoritative abort for any distributed attempt with durable protocol state.

Unprepared acquisition waits may be canceled with a recorded abort. Prepared reservations cannot be released on timeout. Queue scheduling SHOULD bound starvation through fair admission and measure it; a fairness implementation may not preempt an irrevocably prepared transaction by forgetting its vote.

The single-IDC serial executor MUST NOT block its consensus apply loop while waiting for another IDC or storage RPC. It persists the queued/reserved state and advances the protocol asynchronously. Control messages and decision recovery remain processable while user operations wait. Unadmitted requests hold no locks and receive no conflicting serial commitment.

## 8. Single-IDC ordered execution

```text
1. Authenticate/authorize under SPEC-013; resolve the durable SPEC-012 RequestKey
   mapping at its home; replay an existing FinalReceiptV1.
2. Verify the generation/admission permit and sealed semantic footprint.
3. Through current consensus authority, enqueue the command and acquire root X.
4. Assign its SerialOrder only at admission to the protected execution slot.
5. Wait for required causal/frontier state and prior protected executions.
6. Read the authoritative committed pre-state across the IDC's participants.
7. Execute deterministic IR; validate state preconditions, postconditions,
   all affected invariants, returned values and issued commitments.
8. Persist the exact normalized effects/result and fixed participant set.
9. Run the prepare/decision/publication protocol in §§9–12.
10. Persist completion, release root X and return the durable original result.
```

An admitted slot can end in a final rejection or abort record; it is not silently reused. A replay applies the stored deterministic transition/result with exact identity. It does not rerun an operation against a later balance or call an external service.

Follower checks SHOULD recompute supported deterministic validation from the recorded pre-state/digests and fail closed on divergence. No wall clock, random generation, locale, platform float behavior or mutable executable code may silently affect result bytes. Required nondeterministic input must be explicitly captured by a supported contract before it enters the ordered command.

A single physical participant MAY combine authority decision and storage commit in an optimized implementation only after proving the same durable decision, deduplication, recovery and visibility obligations. The v1 reference path uses the explicit protocol even when several logical roles share one process.

## 9. Distributed transaction records and authority selection

The protocol applies to C4 and C5. It SHALL use one durable decision authority per transaction, chosen deterministically as the configured authority of the lowest canonical participating IDC ID at admission. This is a participant-scoped coordinator; it is not one global cluster sequencer.

The authority is itself consensus replicated. Its identity and complete `IdcBinding` remain pinned for the transaction, including recovery after leader change. The request home and decision authority have different responsibilities: the home owns the one RequestKey mapping and execution assignment; the decision authority owns the sealed transaction decision and publication. A durable home binding must point to that authority before `TxnBegin`; a missing begin response never authorizes choosing a second decision authority. SPEC-011 and SPEC-009 must preserve a verified forwarding/resolution route if either authority is relocated or retired.

```text
TxnBegin {
    txn_id: TxnId; request_key: RequestKey; request_hash: RequestHash
    request_home_epoch: RequestHomeEpoch
    operation: OperationRef; operation_hash: OperationHash
    contract_hash: ContractHash; schema_hash: SchemaHash
    plan_hash: PlanHash; plan_generation: PlanGeneration
    idc_bindings: [IdcBinding]
    decision_authority_id; decision_idc_binding: IdcBinding
    participant_set: [ParticipantDescriptor]
    participant_set_hash
    membership_hashes; admission_permit
    expected_effects_digest; expected_result_digest
    issued_commitment_digest
}

ParticipantDescriptor {
    participant_id; role: Certifier | SerialAuthority | DataAuthority
    idc_bindings: [IdcBinding]
    placement_epoch: PlacementEpoch
    membership_generation: MembershipGeneration
    consensus_config_id; storage_authorities
    expected_batch_digest
    required_durability
    descriptor_hash
}

PrepareVote {
    txn_id; begin_digest; participant_set_hash
    participant_id; participant_descriptor_hash
    vote: Yes(prepared_tokens, reservation_digest, payload_digest)
        | No(reason)
    replicated_durable_reference
}

DecisionCertificate {
    txn_id; begin_digest; participant_set_hash
    outcome: Commit | Abort(reason)
    all_required_vote_references
    effects_digest; result_digest; commitment_digest
    decision_authority_id; decision_idc_binding: IdcBinding
    committed_authority_log_reference
}
```

A certificate is verified evidence from trusted protocol participants under the crash-fault model. Its digest alone is not authoritative. Receivers verify SPEC-013 sender/authority authentication and the exact SPEC-011 admission plus committed authority record. Votes from heterogeneous participant roles bind the complete descriptor hash; a bare integer `authority_epoch` is not an acceptable replacement for its typed IDC, placement and membership fields. It is not a mathematical correctness proof or a Byzantine quorum certificate.

One role may represent several local shards only when its yes vote explicitly covers every exact durable prepared payload. Data and certifier voting sets are explicit; a catalog majority or a random set of reachable replicas is never substituted for a missing required participant.

## 10. Prepare and unique decision protocol

### 10.1 Begin

The coordinator SHALL first quorum-persist `TxnBegin` with the complete immutable participant set. No participant may issue a durable yes vote without the matching begin evidence. Repeated begin with identical fields returns its existing record; changed request, participant or effect identity is rejected.

The coordinator's execution-dedup state and `TxnBegin` are updated atomically in its replicated state machine after verifying the existing request-home mapping and its one execution assignment. This does not allocate a second RequestKey mapping. A caller that loses the begin response uses SPEC-012 `ResolveRequest` with its original RequestKey, even if it never learned `TxnId`; it cannot safely create an unrelated replacement request.

### 10.2 Prepare participants

The coordinator obtains all logical locks in the order from §7 and asks each participant to prepare its exact portion. Each participant SHALL:

```text
1. Verify BEGIN, identity, participant membership, epochs and admission permit.
2. Resolve any existing outcome for this identity before accepting a new prepare.
3. Verify the required authority/order/certification evidence and reservations.
4. Validate the exact batch digest and deterministic local obligations.
5. Persist PrepareBatch with the complete SPEC-002 request/operation/contract/
   plan/IDC identity, user changes, indexes, revisions, exact terminal result,
   issued commitments and protocol changes as applicable.
6. Meet the configured replication durability barrier for the prepared payload.
7. Persist a Yes vote/reservation state in its authority group before replying Yes.
```

For a replicated data authority, one node's local fsync is insufficient if failover may promote another node without that payload. A yes vote requires enough durable replicated payload/state to survive the declared number of tolerated failures. `SPEC-002::prepare` supplies local durability only; the upper data authority supplies the replicated durability evidence.

Failed validation produces a durable `No` vote or a coordinator-visible rejection. A participant with an earlier durable yes cannot replace it with no to reclaim resources. Prepared data remains invisible.

### 10.3 Choose one outcome

The decision authority implements a compare-and-set state machine:

```text
Begun -> DecidedCommit  only with verified Yes from every required participant
Begun -> DecidedAbort   on rejection or cancellation while no decision exists
DecidedCommit -> DecidedCommit  for identical duplicate resolution
DecidedAbort  -> DecidedAbort   for identical duplicate resolution
```

No other final transition is permitted. A commit requires exact matching begin, participant-set, epoch, payload, effect and result digests. A no vote, missing mandatory participant or unsupported version forbids commit.

Only the decision authority may choose abort. It may do so following a coordinator deadline while the transaction is still undecided, even if some or all participants have voted yes, provided its quorum commits the unique abort first. A participant's local deadline cannot stand in for that decision.

Coordinator failure is recovered by the same replicated authority: inspect the durable state, gather matching votes if useful, then complete the unique decision. A temporary absence of commit in a stale follower is not an abort certificate. If the decision authority's quorum is unavailable, participants remain in doubt.

### 10.4 Install the decision

On `Commit`, each participant verifies the certificate and calls SPEC-002's `commit_prepared` idempotently. Its complete user/index/protocol batch becomes recoverable locally, tagged with the transaction's semantic publication dependency. On `Abort`, it durably records the terminal abort and releases prepared payloads only after the corresponding safety requirements.

Installing the same committed transaction again must not create another debit, result or serial position. A missing prepared payload under an otherwise valid commit is a recovery fault requiring replicated repair; it is never justification to convert commit to abort.

Delayed prepare messages arriving after abort must hit the retained terminal identity and return that outcome. They cannot resurrect a transaction that has already released its locks.

## 11. Global atomic publication

Physical installs may complete at different times. The runtime SHALL therefore enforce a publication layer above SPEC-002's local MVCC visibility rule. All public C4/C5 reads and invariant-evaluating writers must obey it.

```text
Installed {
    txn_id; decision_digest; participant_id; participant_descriptor_hash
    prepared_batch_digest
    installed_frontier
    replicated_durable_reference
}

PublicationCertificate {
    txn_id; decision_digest; participant_set_hash
    complete_installed_references
    result_digest; commitment_digest
    publication_authority_log_reference
}

PublishSeen {
    txn_id; publication_digest; participant_id; participant_descriptor_hash
    durable_publication_frontier
}

CompletionCertificate {
    txn_id; publication_digest
    complete_publish_seen_references
    completion_authority_log_reference
}
```

The algorithm is:

```text
1. Every required physical/data participant commits the exact prepared batch
   and quorum-persists Installed. Its user versions remain publication-gated.
2. The decision authority verifies Installed from the complete sealed set.
3. It quorum-persists PublicationCertificate exactly once.
4. Each participant durably installs that certificate and its semantic visibility
   marker, then responds PublishSeen after its required replicated barrier.
5. The decision authority verifies every required PublishSeen and quorum-persists
   CompletionCertificate.
6. Participants use completion evidence to release transaction reservations.
7. The gateway may return SPEC-012 FinalReceiptV1 with the durable exact result,
   commitments, decision/publication/completion evidence and scoped session evidence.
```

The coordinator retains protocol progress and retries missing install/publication messages after failures. It cannot acknowledge success after just step 3 if the declared read contract requires every participant to be ready to serve the completed transaction.

Prepared or committed-but-unpublished versions MUST NOT escape through a raw storage read, secondary index, aggregate, change feed or a reader using only local `durable_commit_seq`. Each such version carries its transaction publication dependency or resides behind a semantic published frontier that proves the equivalent property.

All transaction gates remain held until the completion certificate permits release. This prevents a public read on one participant from observing newly published effects while another participant is still presenting the old state within the same coherent read operation. A participant recovering from an earlier snapshot restores pending publication and gate state before becoming ready.

Two-phase commit provides the unique decision; this extra publication protocol plus the read barriers below supplies the public observation boundary. Removing either boundary requires a separate proof and acceptance campaign.

For an abort, the coordinator gathers durable cleanup acknowledgement from all affected participants before compacting the transaction record. It may return a final aborted outcome once that unique decision and stable result are durable, because no participant can legally publish a commit. Unavailable cleanup still pins state; it does not delay knowing the final abort or justify forgetting its identity.

## 12. Safe reads and snapshot cuts

### 12.1 Coherent IDC and multi-IDC reads

The v1 public C4/C5 read path SHALL:

```text
1. Resolve and seal the complete requested semantic/physical footprint.
2. Acquire root S gates for every requested IDC in canonical order.
3. Obtain a current read barrier from each authority and wait for application.
4. Resolve required publication/decision dependencies, causal/session frontiers
   and migration fences, or block/return Unavailable without partial results.
5. While holding every S gate, register one retained LocalSnapshot per physical
   participant and record the corresponding semantic publication frontiers.
6. Bind them into a SnapshotCut; release the logical S gates.
7. Read only the registered snapshots and apply semantic visibility filtering.
```

The gate interval prevents any intersecting C4/C5 writer from modifying the cut while local snapshots are collected. A snapshot may therefore be old, but it is one coherent cut. The system does not need a cluster-global physical timestamp or require equal `LocalCommitSeq` values at different replicas.

```text
SnapshotCut {
    cut_id; read_contract_hash
    idc_bindings: [IdcBinding]
    idc_membership_hashes
    authority_frontiers
    local_snapshots: [SnapshotParticipant]
    publication_frontiers
    required_scoped_session_evidence
    retention_guards
}

SnapshotParticipant {
    storage_authority; participant_descriptor_hash
    storage_epoch: StorageEpoch; visible_seq: LocalCommitSeq
}
```

The complete query footprint must be known or safely enclosed before snapshot capture. For joins, scans and dynamic discovery, an uncovered new IDC requires a restart with the enlarged sorted gate set. Pagination must remain on the same cut. A resource limit may expire the query with a typed error; it cannot silently continue on a new snapshot.

Read-your-writes and monotonic session reads use authenticated returned evidence bound to the strong read contract and its exact IDC footprint. A server unable to reach that evidence's required frontier waits, routes to an eligible authority or returns unavailable. It never drops the session token to return stale data. The qualified evidence type and transport belong to SPEC-012; a runtime capability may claim only the scopes it implements.

This coordinated multi-IDC snapshot does not enlarge SPEC-005's group-scoped `SessionTokenV1`. A sequence of C2 operations in G1/G2 followed by a read in G3 has no implicit cross-group guarantee. Supporting that session requires an explicitly qualified composite contract with complete dependency transport, authority/publication evidence and scope conversion; absent that capability the compiler/runtime returns `SessionScopeMismatch` or `UnsupportedComposition` before effects. A collection of unrelated tokens or the C5 class label alone is insufficient.

### 12.2 Atomicity example

```text
Before T: A.balance = 100; B.balance = 0
T:        A -= 100; B += 100
```

A coherent read of `{A, B}` may return the pre-T or post-T state, or another valid state resulting from later operations. It MUST NOT return the torn state `A=0, B=0` caused solely by one T participant installing earlier.

Independent reads in different requests may straddle T in time; the API does not pretend they form one snapshot. A read operation that needs a joint assertion must request one joint cut. Within such a request, no partial rows, streaming page or aggregate is returned before the cut is established.

### 12.3 Local observations and replicas

`LocalSnapshot` alone is a storage primitive, not a C5 serial read or a multi-IDC snapshot. In v1, public authoritative C4/C5 reads use the gates above, including a one-IDC point read. This may require communication under failure; offline strong reads are not promised.

A separate local projection may expose an explicitly weaker observation under SPEC-005. Its one-group token must identify its causal/publication closure and scope. It cannot be used as a certified business decision, mislabel independent local snapshots as an atomic multi-IDC cut, infer cross-group session guarantees, or expose unresolved/unpublished transaction fragments as final state.

A lagging follower can serve the coherent path only after an authoritative barrier and application through the required frontier. A stale leader or a replica restored from a previous storage epoch must not claim current authority because it still has readable pages.

## 13. Multi-IDC ordering

For a composite transaction, the coordinator acquires root `X` gates across all IDCs in canonical order and records each reservation in its authority group. Once all gates are held, it obtains a stable pre-state, assigns the corresponding per-IDC execution positions, evaluates the whole operation, and executes the shared prepare/decision/publication protocol.

The ordering is equivalent to conservative strict two-phase locking over those IDC gates. Locks remain held through publication completion. This prevents serialization cycles between transactions that touch overlapping IDC sets and ensures one atomic contract governs the result.

Queued, unadmitted local operations cannot hold a higher-ranked lock that causes the apply loop to wait on a lower-ranked IDC. Composite admission must be scheduled as a lock request rather than an already-executing user command. Consensus decision/recovery records remain applicable while a reservation blocks new user execution.

A multi-IDC contract may explicitly describe separate reservations/compensation instead of atomic effects. Such a contract has different success/failure outcomes and must be compiled as that contract. The runtime MUST NOT introduce compensation to disguise a partially committed atomic request.

The v1 path may coordinate unrelated participants more than an optimal protocol. That cost is measured in SPEC-010. It does not justify weakening atomicity or returned promises.

## 14. Mixed-class execution

C5 is not an automatic fence against C1/C2/C3 authority still active elsewhere. All writers capable of invalidating its state, guards, return contract or obligations must be included in the compiled compatibility matrix.

A weaker writer may remain independent only with a checked interference rule that covers the ordered operation's full observable contract. Otherwise, the affected domain must first enter SPEC-009's freeze/drain/reconciliation transition, including every semantic emitter and outstanding escrow grant.

An epoch increase at the sequencer cannot revoke an isolated holder's locally issued rights or final confirmations. If a disconnected writer has not installed the required fence and no separately proved offline-authority bound makes it safe, strengthening blocks. The runtime may continue unrelated safe IDCs.

Replayed historical final effects retain their original identity, commitments and provenance. They are not admitted as new old-plan invocations, nor arbitrarily rejected as stale in a way that erases already-issued commitments.

## 15. Failure and recovery behavior

| Failure window | Required outcome |
|---|---|
| Before durable BEGIN | No legitimate participant yes exists; stable-ID status may be unknown |
| After BEGIN, before all votes | Decision authority can recover and commit only with all yes, or durably abort |
| Participant crashes after yes | Recover prepared bytes/reservations before admission; resolve through the pinned decision authority |
| Decision authority loses its quorum | No new decision; prepared participants retain locks and data indefinitely if necessary |
| Decision durable, reply lost | Replay exact decision/result; never execute the request under a new identity automatically |
| Commit decided, one data authority unavailable | Commit remains final; installation/publication waits; no heuristic abort |
| Publication durable, some markers not installed | Restore/retry markers; gates remain until completion evidence |
| Completion durable, response lost | Same-ID retry returns the original successful response |
| Minority stale leader | Cannot produce new valid ordered admissions, votes or serial reads |
| Disk full/fsync error | No unsupported durable acknowledgement; pending outcome is reconciled before final response |
| Required payload or protocol history corrupt | Fail closed; restore verified replicated state; never guess transaction outcome |

Startup order SHALL be:

```text
recover local storage and exact prepared/committed state
restore consensus authority state and durable configuration
reconstruct request identities, reservations, votes and sealed memberships
reconcile current admission fences and authority epochs
resolve decisions through the original authority or verified successor
replay missing committed installs and publication markers idempotently
restore retained result/commitment records and read frontiers
advertise READY only for scopes whose authority and visibility are proven
```

The system MAY advertise readiness by IDC rather than wait for every unrelated component. A scope with unresolved intersecting work must block unsafe mutations/reads. Health output distinguishes locally recovered storage from a usable serial authority.

`UNKNOWN` is a lack of conclusive outcome information. `IN_DOUBT` is durable prepared work without known decision. `CommittedPendingPublication` means commit is irrevocable but its public completion is unfinished. None means aborted.

## 16. Retry, replay and garbage collection

An exact duplicate command, prepare, decision, installation or publication message SHALL return the existing phase/outcome and MUST NOT apply effects twice. Identity mismatches are protocol errors even if the apparent final values happen to match.

Request records retain the original RequestKey mapping, immutable request hash, exact result bytes or a durable content reference, commitments and SPEC-012 FinalReceiptV1 evidence. Recovery and plan changes return the original versioned result. A new application request may choose a new identity only when the caller intentionally requests a separate execution; the server cannot manufacture one to hide an unknown outcome. Protocol-only phase records use SPEC-002 internal identities and do not invent client request keys.

GC must satisfy every relevant horizon:

```text
active snapshot cuts and semantic dependency closure
unresolved prepares and reservations
decision/install/publication/completion recovery
replication and backup consumers
advertised retry/result horizon
plan/authority migration references and issued commitments
```

Prepared work never expires by TTL. A decision authority may compact completed transaction details only after required participant acknowledgement and equivalent durable deduplication/outcome state exists. A lagging participant must recover from an authoritative checkpoint containing that state before new admission.

Detailed old results may be retired only within the advertised retention contract and SPEC-011 retention decision. Expired identities remain non-reusable through durable epoch/client-sequence fences or retained tombstones. A late request returns SPEC-012 `IdentityExpired`; it is not treated as a fresh debit. Commitments whose obligations remain live cannot be discarded merely because their response retention elapsed.

Backpressure must reject new transactions before it threatens retention of in-doubt state. Operators may restore communication, disk capacity or verified replica state; a force-abort command without an authoritative unique decision is outside this specification and MUST NOT be offered as a safe repair.

## 17. Reconfiguration and plan evolution

V1 supports fixed consensus membership. Adding/removing voting members requires the consensus adapter's verified joint-configuration procedure or an equivalent specified quorum-intersection mechanism. Copying an old log to a fresh independent majority is not reconfiguration.

IDC split/merge, physical resharding, sequencer placement change, schema changes and protocol strengthening use SPEC-009. The runtime SHALL provide:

```text
close_admission(generation, boundary)
enumerate_outstanding(boundary)
capture_authority_state(transition_id)
install_transfer_and_fence(transition_id, destination)
activate_generation(transition_certificate)
retire_authority(verified_horizons)
```

These are responsibilities, not a versioned transport API. A transfer contains semantic order/frontiers, prepared reservations, exact votes and participant sets, durable decisions, pending publication/completion, results, deduplication and live commitments. The receiver verifies the transfer digest and the prior authority's fencing evidence before producing new outcomes.

A transition MUST distinguish new old-generation invocation from retry/status/historical replay. New invocation is rejected after admission closure. Already admitted attempts may finish under an explicit drain permit; their final result and commitments cannot be rewritten. The old code/plan/decoders remain retained for required recovery.

Before activating new authority, every old semantic emitter must have installed the required fence or be accounted for by a proved compatible transition. Offline escrow holders cannot be assumed fenced by catalog quorum alone. When a split/merge touches an in-doubt composite transaction, the transition either drains it or transfers its entire decision-resolution and publication obligations without creating a second decision authority.

An old committed transaction received during recovery is interpreted under its recorded contract/generation and verified transfer rules. It is not rejected merely because new admission now requires a later generation. Safety means preserving old commitments as well as enforcing the new contract for new work.

## 18. Explicit unsupported behavior

A conforming v1 runtime SHALL reject or block:

- mutation through SQL, storage access or a background writer that bypasses the protected semantic boundary;
- unsafe sequential contracts, unsupported predicates and unknown mandatory format/plan semantics;
- dynamic participant expansion after durable preparation;
- multi-IDC atomic queries assembled from unrelated local snapshots;
- final responses based only on local fsync, a received log entry, a yes vote or a partial publication;
- heuristic or timeout-based participant abort after preparation;
- reconfiguration without quorum continuity and complete outstanding-state transfer;
- immediate revocation of disconnected authority without a proved fencing/authority mechanism;
- an atomicity claim spanning an external HTTP/payment/device effect without a separately specified cooperative protocol;
- arbitrary user code, floating-point nondeterminism or runtime LLM decisions in the correctness path.

These boundaries describe unavailable implementations. They are not alternate modes that may continue returning the same final-guarantee label.

## 19. Errors, status and observability

Required typed outcomes include:

```text
RequestIdentityMismatch; IdentityConflict; IdentityExpired; SessionScopeMismatch
UnsupportedFootprint; UnsafeSequentialContract; UnsupportedEvaluator
PreconditionRejected; PostconditionRejected; InvariantRejected; ArithmeticError
StalePlan; StaleSchema; StaleAuthority; MembershipChanged; UnfencedWriter
NotLeader; QuorumUnavailable; ParticipantUnavailable; ReadBarrierUnavailable
TxnUnknown; TxnInDoubt; CommittedPendingPublication
SnapshotExpired; PublicationRequired; ProtocolCorruption; ResourceExhausted
```

Final rejection, final abort, pending outcome and transport error must be distinct. Error details identify the failing authority/phase and safe retry identity; user values and secrets are redacted by default.

Required metrics:

```text
serial_operations_total{operation,outcome}
serial_queue_depth; serial_queue_wait_seconds
serial_authority_leader_changes_total
serial_applied_position; serial_commit_lag
idc_gate_wait_seconds{mode}; idc_gate_holders{mode}
distributed_txn_total{class,outcome}
distributed_txn_phase_seconds{phase}
distributed_txn_participants
distributed_txn_in_doubt; distributed_txn_oldest_in_doubt_seconds
distributed_txn_retained_bytes
publication_pending; publication_wait_seconds
serial_read_barrier_seconds; snapshot_cut_restart_total
authority_fence_reject_total; protocol_identity_conflict_total
```

Diagnostic inspection must show the exact participant set, per-participant prepare/install/publication phase, decision authority, epochs, serial positions, pinned retention and stable result digest. High-cardinality transaction IDs belong in traces or inspection output, not default metric labels.

## 20. Acceptance scenarios

These are release obligations. No execution or pass result is claimed by this document.

| ID | Scenario | Acceptance condition |
|---|---|---|
| C5-001 | Concurrent operations in one IDC | History matches the sequential reference contract, including results and final rejection |
| C5-002 | Independent IDCs on one shard | No unintended global semantic order; physical local batching does not change contracts |
| C5-003 | One IDC spans two physical shards | Atomic decision and publication cover both; local success alone is insufficient |
| C5-004 | Multi-IDC transfer with reversed request key order | Canonical gate order avoids deadlock and serialization cycles |
| C5-005 | All participants vote yes; coordinator crashes | One replicated authority resolves once; no participant timeout-abort |
| C5-006 | Crash at every prepare/decision/install/publication/completion edge | No lost acknowledged result, phantom decision or partial public transaction |
| C5-007 | Debit installed; credit participant partitioned | No coherent read returns the torn transfer; commit status remains pending publication |
| C5-008 | Read collects snapshots while a distributed commit is published | Gate-protected cut contains all or none of that transaction's relevant effects |
| C5-009 | Minority stale leader serves mutation/read requests | Cannot authorize new mutation or current serial read |
| C5-010 | Duplicate messages and changed payload under one ID | Exact duplicates are idempotent; mismatches fail without another effect |
| C5-011 | Client disconnects after completion, then schema evolves | Retry returns the original durable result and commitments |
| C5-012 | Dynamic predicate inserts new row or moves a grouping key | Membership/domain fences include the new dependency or reject/restart safely |
| C5-013 | Prepared transaction blocks IDC split/merge or authority move | Drain/verified transfer retains one decision authority and every obligation |
| C5-014 | Consensus configuration changes during prepared work | Quorum continuity and payload/decision retention survive allowed failures |
| C5-015 | Offline C3 holder during strengthening to C5 | No admission until complete proof/fence/reconciliation; prior grants remain accounted for |
| C5-016 | Follower restore, GC and late retry | No stale authority or duplicate execution; expired identity rejects explicitly |
| C5-017 | Unsafe sequential operation with strongest requested class | Rejected; serial order cannot legalize the invalid transition |
| C5-018 | Failure of one data copy after yes | Prepared payload/decision remains available within declared failure tolerance |
| C5-019 | Query discovers new IDC, paginates or exceeds retention | Restart before results or explicit expiry; no mixed-cut output |
| C5-020 | Control/recovery record arrives while user lock request waits | Authority state machine makes protocol progress without apply-loop deadlock |
| C5-021 | Two ingress nodes first submit one RequestKey; crash after home mapping/assignment but before BEGIN reply | One mapping and decision authority; ResolveRequest works without client-known TxnId and returns the exact original receipt |
| C5-022 | Substitute IDC generation for authority epoch or replay a vote under a changed participant descriptor | Typed binding/hash/authentication checks reject before decision or install; numeric equality grants no authority |
| C5-023 | Present C2 G1/G2 tokens for unqualified cross-group strong-session read | Scope rejects or separately qualified complete composite evidence is required; no partial context or implicit C2 guarantee |
| C5-024 | Upgrade/downgrade codecs with prepared and unpublished work; forged peer/certificate | SPEC-012/013 reject unknown mandatory semantics and unauthenticated authority while preserving all old recovery/publication obligations |

The model checking target includes unique outcome, no visible undecided effect, publication closure, serializability of gate-protected operations, stable final result, fencing and retention. SPEC-010 FM-2 requires model checking plus passing deterministic simulation and real-process fault campaign before a distributed C4/C5 correctness claim; authority/plan evolution also requires FM-3. The simulator must preserve independent local journal and consensus durability events so it can detect mistaken conflation of those boundaries.

Liveness is conditional on eventual communication, available required quorums/storage, fair scheduling and resolution of preceding reservations. Availability, starvation and useful successful operations must be measured separately from safety.

## 21. Milestones

| Milestone | Deliverable | Exit criterion |
|---|---|---|
| C5-A | Deterministic single-IDC executor and explicit ordered/result records | C5-001/017 pass against the sequential model |
| C5-B | SPEC-011/012/013 integration, fixed three-node consensus authority and storage adapter | C5-009/010/018/021/022/024 pass with independent durability faults |
| C5-C | Multi-shard prepare/unique decision recovery | C5-003/005/006 pass; no heuristic abort path exists |
| C5-D | Publication layer and coherent read cuts | FM-2 model/simulator/real-process gates and C5-007/008/019/023 pass under arbitrary install delays |
| C5-E | Composite gates and mixed-class integration | C5-002/004/012/015/020 pass |
| C5-F | SPEC-009 evolution and retention | C5-011/013/014/016 pass or unsupported reconfiguration fails closed |
| C5-G | Baseline evaluation | Publish equivalent-contract latency, throughput, queueing, failure availability and coordination costs |

The first serial milestone corresponds to SPEC-001 M6; composite/publication behavior is required before exposing an atomic distributed API. The strong baseline must work before claiming benefits from C4 or dynamic strengthening. Performance targets follow measurements; no target TPS or latency is asserted here.

## 22. Source traceability and clarifications

| Source requirement | This specification |
|---|---|
| SPEC-001 §§6, 18, 37–38 — per-IDC serial order independent from shards | §§0–6, 8, 13 |
| SPEC-001 §§20, 26–29 — preservation, envelope and exact plan | §§1–4, 8–10 |
| SPEC-001 §§35–36, 47 — reads/session guarantees and multi-IDC atomicity | §§9–13 |
| SPEC-001 §§39–43, 49–51 — evolution, recovery and partitions | §§14–17 |
| SPEC-001 §§68–73, 88 — correctness and serial milestone | §§20–21 |
| SPEC-002 §§7–9, 98–99 — physical/semantic ordering boundary | §§0, 3–5 |
| SPEC-002 §§53–71 — local durability, prepare and unknown outcomes | §§9–11, 15–16 |
| SPEC-002 §§88–89, 105–109 — GC and generation drain | §§16–17 |
| Proposal §1; prior-art notes on composition, observations and evolution | §§1–2, 11–17, 20 |

This draft refines SPEC-001 and SPEC-002 consistently: local MVCC visibility is additionally gated by distributed publication; serial fallback requires a valid executable sequential contract; and old final outcomes remain recoverable after their admission generation closes. SPEC-011 owns registry authority, SPEC-012 owns identity/encoding/client contracts and SPEC-013 owns trust; none removes the installation/publication/completion gates defined here.

Research context is taken from the repository's [proposal](../PROPOSTA-DE-PESQUISA.md) and [prior-art notes](../research/consistency-prior-art.md). The design above is a proposed baseline and proof obligation, not a new theorem or a benchmark result.
