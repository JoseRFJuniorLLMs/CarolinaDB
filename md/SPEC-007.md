# SPEC-007 — Certified Transactions

**Subtitle:** Optimistic Execution, Predicate Validation and Durable Reservations
**Status:** Draft 0.2 — proposed implementation contract; not an implemented or proven protocol
**Implementation status (2026-09-12):** Not implemented (optional SPEC-014 MVP-8; gate Q6-CERTIFICATION NOT_RUN, Q-C09 NOT_APPLICABLE). Only fail-closed placeholders exist: the unqualified C4_CERTIFIED_V1 template (rejected with MissingRuntimeCapability), plan-codec variants that are never constructed and a handshake that refuses a C4 capability; no certifier, read tokens, reservations, SnapshotCut, C4 error codes, metrics or C4-0xx campaign. Table: [docs/AUDIT.md](../docs/AUDIT.md).
**Date:** 2026-09-09
**Type:** Distributed runtime specification
**Depends on:** [SPEC-001](SPEC-001.md), [SPEC-002](SPEC-002.md), [SPEC-003](SPEC-003.md), [SPEC-004](SPEC-004.md)
**Protocol interfaces:** [SPEC-005](SPEC-005.md), [SPEC-006](SPEC-006.md), [SPEC-008](SPEC-008.md), [SPEC-009](SPEC-009.md)
**Qualification:** [SPEC-010](SPEC-010.md)
**Normative registries and protocols:** [SPEC-011](SPEC-011.md) (catalog and typed authority), [SPEC-012](SPEC-012.md) (request identity, receipts and codecs), [SPEC-013](SPEC-013.md) (security and trust)
**Reference implementation:** Rust stable; certification in `carolina-runtime`, outside `carolina-storage`
**Normative terms:** MUST, MUST NOT, SHOULD, SHOULD NOT and MAY express requirements on a conforming implementation.

---

## 0. Decision and scope

C4 permits speculative operation execution followed by authoritative validation of its entire semantic dependency set. A successful speculative calculation is not a commit, a resource grant or a final response.

The correctness-first implementation SHALL use optimistic reads followed by pessimistic, durable reservations during certification. It SHALL validate exact reads, predicate coverage, operation preconditions, generated postconditions and invariant obligations while conflicting commits are excluded. It SHALL retain the reservations until the durable transaction outcome and publication requirements are satisfied.

The atomic transaction decision and publication protocol is defined by SPEC-008, including C4 transactions with several participants. A certifier vote, a local `PrepareBatch` and a quorum-written certification record are each insufficient on their own to make user effects visible.

C4 does not establish a permanent total execution order for every operation in an IDC. Consensus may order certifier metadata requests, while nonconflicting transactions execute concurrently. The first implementation uses conservative conflict detection; invariant-specific reductions are separate optimizations requiring a checked rule and a composition argument.

This specification does not claim that this baseline outperforms C5 or constitutes a new protocol. SPEC-010's equivalent-contract experiment must determine whether C4's additional machinery is useful. C4 is implemented after the qualified C1/C2/C3/C5 baselines and evolution foundations, as ordered by SPEC-014.

## 1. Contract semantics at certification

An operation contract is modeled as:

```text
O(S, args) -> Rejected(reason) | Accepted(S', result, commitments)
```

The relevant obligations are distinct:

| Obligation | Meaning | Required runtime treatment |
|---|---|---|
| Argument precondition | Predicate over immutable arguments | Check before speculative execution; preserve canonical argument identity |
| State precondition | Admission predicate over the state used by the operation | Certify its read dependencies and evaluate before applying effects |
| Postcondition | Predicate relating pre-state, post-state and returned result | Evaluate with the exact certified pre-state and candidate effects |
| State invariant | Predicate that holds in every permitted committed reachable state | Validate the affected invariant closure; reserve all ways concurrent operations could falsify it |
| Final commitment | Promise made by a final response, potentially surviving later state changes | Persist its identity and result; preserve it through replay and plan evolution |

`REQUIRE account.status == OPEN` is a state precondition. It is not an instruction that an account must remain open forever. `ENSURE balance >= 0` checks the operation's post-state; a persistent database-wide rule requires an invariant declaration as well. An operation-specific `ENSURE` MUST NOT silently become an invariant assumed for every other writer.

Conversely, a state invariant cannot be dismissed because an operation did not declare it in `ENSURE`. The compiler's complete affected invariant closure is mandatory.

If speculative reads show a false state precondition, a runtime MAY return a retryable observation without certifying it, but MUST NOT label it a final business rejection unless the contract permits that observation strength. A final state-dependent rejection MUST validate the observations that justify it and persist the result under the stable request identity. False immutable argument preconditions can be rejected without distributed state reads.

Arithmetic, comparisons, rounding, result encoding and exceptional outcomes MUST follow SPEC-003. An overflow, missing mandatory witness or unknown evaluator is a typed failure. It is not a successful no-op. C4 cannot make a sequentially invalid operation safe by coordinating it.

## 2. Supported fragment and rejection boundary

C4 v1 SHALL accept only plans with:

- deterministic, executable Effect IR and certification predicates;
- a finite, complete semantic read/write footprint or a conservative finite enclosing domain;
- an authoritative owner for every conflict domain, including absent keys and predicate gaps;
- a complete writer-interference matrix covering all active operation versions;
- fixed participants and epochs for one transaction attempt;
- an explicit read/return contract, durability contract and retry identity;
- storage support for durable prepare, atomic protocol metadata and invisible unresolved work.

Supported initial certification shapes are exact key reads; absent keys; canonical index ranges; uniqueness; referential existence; and bounded aggregate groups whose complete scope can be reserved and evaluated. Aggregate support is a C4 milestone beyond the initial C5 MVP, not a retroactive claim that SPEC-001's MVP contains arbitrary aggregates.

The compiler MUST promote to a supported C5 implementation or reject when C4 cannot cover a predicate. C5 is permissible only if the sequential contract itself is safe and executable. Unbounded traversal without a finite enclosing lock scope, arbitrary user callbacks, opaque external effects, approximate aggregate validation and untracked raw SQL mutation MUST be rejected by C4.

There is no best-effort certification mode. An administrator's request for weaker guarantees does not authorize a path to claim this specification's invariant or final-result guarantees.

## 3. Identity, epochs and membership

The following are conceptual records using SPEC-011's strong types. Their layout is not a frozen wire or disk format. SPEC-012 owns versioned canonical codecs, bounds, receipt formats and golden fixtures, which must pass before compatibility is claimed. Authentication and verification of authority evidence follow SPEC-013 independently of content hashes.

```text
CertificationContext {
    txn_id: TxnId
    request_key: RequestKey; request_hash: RequestHash
    request_home_epoch: RequestHomeEpoch
    operation: OperationRef; operation_hash: OperationHash
    schema_hash: SchemaHash; contract_hash: ContractHash
    plan_hash: PlanHash; plan_generation: PlanGeneration
    idc_memberships: [CertificationMembership]
    decision_authority_id; decision_idc_binding: IdcBinding
    participant_set_hash
    snapshot_cut_id
    read_contract_hash
    effects_digest; result_digest; commitment_digest
}

CertificationMembership {
    idc_binding: IdcBinding
    membership_hash
    placement_epoch: PlacementEpoch
    membership_generation: MembershipGeneration
}
```

`RequestKey = (TenantId, RequestNamespace, StableRequestId)` routes to its SPEC-012 request home before `TxnId` exists. That home durably CAS-creates one mapping to a globally unique `TxnId` and immutable request hash before certification begins. The hash binds the canonical operation/schema/contract identity, arguments and original session/read contract. Placement, plan choice and certification attempts remain separate. A matching retry resolves the original mapping and exact receipt; changed content under one key returns `RequestIdentityMismatch` without effects. An unresolved home cannot be bypassed with a new transaction or certifier.

Each `IdcBinding` contains `IdcId`, semantic `IdcGeneration` and `IdcAuthorityEpoch`; numeric equality cannot substitute one for another. The decision authority is the pinned IDC authority selected in SPEC-008. Catalog, plan, placement, consensus membership, origin and storage generations retain their independent SPEC-011 scopes.

An IDC membership record MUST include the semantic domain instances, their certifier authorities, physical participants and dependency scopes. The membership is sealed before `BEGIN` in SPEC-008. A row creation, deletion or grouping-key change that could alter membership MUST reserve the relevant membership/index domain. A new row does not escape an invariant merely because it was absent when the IDC map was built.

If discovering reads adds a participant or invariant, speculative work MUST restart before any positive vote. After durable reservations or a positive vote, the participant set cannot grow; the attempt must be resolved and retried explicitly. A broad domain reservation or temporary C5 execution may avoid repeated discovery.

Membership hashes and authority epochs are compared independently of local `StorageEpoch` and `VersionStamp`. Neither a storage sequence nor an elapsed lease timer establishes distributed authority.

## 4. Read evidence

### 4.1 Snapshot cut

An optimistic multi-shard read SHALL first obtain a `SnapshotCut` through SPEC-008's coordinated read barrier over its declared footprint. This briefly acquires shared IDC gates in canonical order, waits for ordered/publication frontiers, registers local MVCC snapshots, and then releases the gates. Subsequent speculative computation reads only those pinned snapshots.

Independent local snapshots collected at unrelated times MUST NOT be described as a globally consistent snapshot. A plan that accepts a weaker observational cut must explicitly prove that its guard, returned value and certification program remain valid; C4 v1 does not implement that optimization.

Snapshots retain version, index, membership and decision metadata needed for later validation. An expired or reclaimed snapshot causes `SnapshotExpired`; the runtime does not approximate its contents from current state.

### 4.2 Point token

```text
PointReadToken {
    authority_id; idc_binding: IdcBinding
    storage_epoch: StorageEpoch
    placement_epoch: PlacementEpoch
    key_codec_version; canonical_key
    snapshot_cut_id
    observed: Present(version_stamp, value_digest)
            | Absent(absence_revision)
    key_change_revision
}
```

`key_change_revision` is an authority-maintained monotonic semantic change counter, scoped to its epoch. It advances for every committed logical mutation affecting that key, including deletion and reinsertion with identical bytes. Absence therefore has evidence and is not represented by an unprotected null value.

Tokens from one storage epoch are not comparable to tokens from another. A moved/restored key requires a checked evidence translation or a restart; byte equality does not authorize accepting a stale token.

### 4.3 Range token

```text
RangeReadToken {
    authority_id; idc_binding: IdcBinding
    storage_epoch: StorageEpoch
    placement_epoch: PlacementEpoch
    index_id; index_generation; key_codec_version
    canonical_bounds; endpoint_inclusivity
    predicate_digest
    snapshot_cut_id
    covered_buckets: [(bucket_id, predicate_change_revision)]
    result_digest
    completion: Complete
}
```

The baseline MAY use one revision bucket per index or invariant group. This deliberately causes false conflicts but detects phantoms. Finer interval buckets require a deterministic, complete coverage rule for all intervals and their gaps. Range tokens refer to semantic index domains, never B+Tree page IDs; a page split is not a logical phantom.

Every committed insert, delete, index-key move or qualifying value change MUST advance all covering predicate buckets atomically with the user/index mutation. A change relevant to a residual predicate or aggregate MUST advance the covering bucket even when the index key is unchanged. Deleting a nonmatching row may conservatively invalidate a token.

An empty scan MUST still capture all buckets covering its searched interval. The result rows alone cannot establish range stability. `LIMIT`, pagination and early termination do not establish full predicate coverage; the plan MUST reserve/validate the full logical selection scope or return `IncompletePredicateCoverage`. Cursor continuation must use the same retained snapshot.

`result_digest` detects inconsistent evidence; it does not replace a coverage revision or prove completeness. Revision wraparound MUST cause fenced epoch replacement or a typed resource error; wraparound cannot make an old token current.

### 4.4 Predicate-specific obligations

| Predicate | Required protection |
|---|---|
| `UNIQUE email` | Exact canonical email namespace, including absent slot, plus index generation |
| Child references parent | Parent existence and reference scope; parent deletion must intersect the same domain |
| `SUM(Expense.amount WHERE department=d) <= budget[d]` | Entire group membership/predicate bucket, amount changes, row moves and budget key |
| Conservation across two accounts | Both endpoints and every additional affected conservation scope |
| State transition | Current state and permitted transition; any concurrent competing transition intersects the lock domain |

Protecting only output rows is insufficient for uniqueness, aggregates and referential constraints. The compiler MUST emit the missing negative-space and membership dependencies.

## 5. Certification authority and conflict domains

Each conflict domain SHALL have one active certifier authority implemented by a replicated state machine outside the storage kernel. The initial deployment uses a fixed voting configuration with intersecting consensus quorums. A majority of arbitrary data replicas is not a substitute for this authority's configured quorum.

The authority stores its typed IDC binding, domain map, committed change revisions, durable reservations and transaction outcomes. SPEC-011 owns its registry, admitted capabilities, placement and authority grants; admission MUST additionally prove current leadership through its consensus implementation, not a cached leader address. Data owners accept mutations only with authenticated SPEC-013 evidence for the exact SPEC-008 participant descriptor and required typed bindings. Neither a catalog quorum nor a valid client credential replaces a certifier's vote.

The shared lock model is defined in SPEC-008:

- every C4 write transaction holds an intention-exclusive IDC gate;
- read-dependent keys/predicates hold shared domain reservations;
- writes and invariant-delta predicates hold exclusive domain reservations;
- a C5 operation holds the exclusive root IDC gate, conflicting with all finer reservations;
- a coordinated snapshot briefly holds a shared root IDC gate.

Acquisition is ordered by canonical IDC ID, then domain ID, then interval/key encoding. Multiple references to one lock are combined to the strongest mode before acquisition. Upgrading a lock after lower-ranked locks have been taken is prohibited; restart acquisition with the expanded sorted set.

The implementation MUST reserve a complete invariant conflict scope, not merely a write set. Initial aggregate validation takes an exclusive reservation on the aggregate group. Two expenses in different rows cannot both pass against the same unreserved budget remainder.

Nonconflicting reservations may coexist. Overlap conservatively blocks or rejects one attempt; no wait cycle is permitted. Timeout while waiting without a positive vote can request a coordinator abort. Timeout after a positive vote cannot release that reservation.

## 6. Durable records and state machine

```text
CertificationRecord {
    context
    certification_program_hash
    normalized_read_evidence_digest
    reserved_domains: [(domain, mode)]
    certified_frontier
    validation_outcome
    prepared_storage_tokens
    stable_result_bytes_or_durable_reference
    issued_commitments
}

CertifierTxnState =
    Reserved
  | PreparedYes(certification_record)
  | PreparedNo(reason)
  | CommitObserved(decision_certificate)
  | Published(publication_certificate)
  | AbortObserved(decision_certificate)
```

`Reserved` is replicated before any state may be treated as durably excluded. It binds the request, participant list, exact candidate effect/result digests and conflict footprint. It is not a yes vote. The authority can safely reject incomplete work only through SPEC-008's unique durable decision authority; other participants may already be prepared.

`PreparedYes` is replicated only after validation passes and every local physical participant's exact prepared batch has met the configured replicated durability requirement. Its storage token set binds user changes, index revisions, protocol metadata, immutable result and commitments. A subsequent leader reconstructs all reservations before serving new requests.

An exclusive reservation is not an MVCC committed version. Prepared user/index changes remain invisible. Reservation metadata must itself be durable through the authority state machine; where local protocol and business state must change together, they use SPEC-002's `CompiledBatch` atomic boundary.

## 7. Baseline algorithm

### 7.1 Speculate

```text
1. Authenticate/authorize under SPEC-013 and resolve SPEC-012's durable RequestKey mapping; replay an existing final receipt if present.
2. Verify operation/contract/plan compatibility and current admission generation.
3. Compute a conservative semantic footprint and seal membership.
4. Obtain a coordinated SnapshotCut and capture point/range evidence.
5. Execute deterministic IR against that cut plus the private write set.
6. Compute candidate effects, result, commitments and full conflict footprint.
7. If the footprint grew, release snapshots and restart before any vote.
8. Persist BEGIN with the chosen decision authority as specified by SPEC-008.
```

No speculative response is marked final. Tentative output, if exposed, must carry a distinct type that cannot be used as an authority token or a committed reservation.

### 7.2 Certify and prepare

```text
1. Acquire the complete canonical reservation set through its authorities.
2. Quorum-persist Reserved before issuing durable reservation evidence.
3. Resolve older intersecting in-doubt work or remain blocked.
4. Validate admission permit, plan/schema/contract identity, authority epochs,
   sealed membership and required causal frontier.
5. Validate point and predicate revisions against the authoritative committed state.
6. Evaluate all state preconditions, postconditions, affected invariants,
   return constraints and commitment obligations with exact arithmetic.
7. Build exact SPEC-002 batches with the full request/operation/contract/plan/IDC
   identity, user/index/protocol changes, immutable result and commitments.
8. Durably prepare those batches; persist PreparedYes and its exact token set.
9. Submit the vote to SPEC-008's transaction decision authority.
10. Keep all reservations through resolution and publication.
```

A local storage version check in step 5 does not close the race with step 8; the reservation and writer coverage do. Certification and each change-revision update SHALL occur within the same authoritative exclusion protocol.

For v1, any read-version or predicate-revision mismatch rejects the candidate. It cannot be repaired by returning the speculative result with freshly recomputed effects. A later optimization may reexecute the entire operation under reservations and recompute both effects and result; that behavior must preserve retry identity and be explicitly allowed by the contract.

Validation runs only deterministic compiled checks. No solver call, LLM, network API or nondeterministic application callback belongs in this hot path. Solver timeout during compilation and evaluator failure at runtime fail closed.

### 7.3 Decide and publish

SPEC-008 SHALL collect every required vote, record exactly one durable decision, install commits at every physical participant, and record a publication certificate before final success or public committed visibility. C4's `PreparedYes` is not permission for one participant to commit independently.

A `COMMIT` decision is irrevocable even if publication later stalls. The status is `CommittedPendingPublication`, not abort. The certifier retains reservations until publication obligations are met. An `ABORT` decision releases them only after the abort and cleanup state are durable and delayed requests are fenced.

## 8. Cross-IDC composition and mixed classes

Atomic effects across more than one IDC SHALL use SPEC-008's composite transaction protocol. The first implementation takes exclusive root IDC gates in canonical order for the composite transaction, even if individual suboperations could have used C4 domain reservations. This reduces complexity and gives one safe snapshot/decision boundary.

Parallel independent suboperations are allowed only when their contract explicitly defines separate outcomes. The gateway MUST NOT turn a single atomic request into independent successes because its IDCs are physically separate.

C1/C2/C3 writers overlapping a certified dependency must either participate in the same conflict protocol or have a checked interference rule proving that their independent effects cannot invalidate the reads, guards, invariants, returns or issued commitments. A compiler cannot prove that from field-level commutativity alone.

Without that proof, SPEC-009 must freeze and reconcile the affected weaker emitters before C4 admission. An isolated C3 holder cannot be fenced merely by incrementing the catalog epoch. Outstanding rights and prior final confirmations must remain accounted for. Failure to establish a complete writer boundary is `UnfencedWriter`, not automatic strengthening followed by immediate execution.

## 9. Read and result contracts

`AT CERTIFIED` SHALL mean that the requested snapshot's evidence has been certified under the declared semantic footprint, returned values obey the operation's read contract, and no prepared or unpublished effect leaks into it. The API must identify the snapshot/authority frontiers and plan generation. It MUST NOT imply freshness at wall-clock time or cluster-global linearizability unless the contract separately requests and implements those guarantees.

Read-only certification follows the same dependency validation, may use shared reservations and records any promised final decision/result. It cannot serve an `increment_and_get()` exact global result from an arbitrary local increment replica.

A final mutation response uses SPEC-012's `FinalReceiptV1`; this document does not define a competing receipt codec. The durable receipt and its protocol evidence MUST retain:

```text
RequestKey; mapped TxnId; immutable RequestHash
OperationRef; OperationHash; SchemaHash; ContractHash
PlanHash; PlanGeneration; exact IdcBindings
terminal outcome; exact result bytes/digest; issued commitments
decision evidence; publication and completion evidence for committed work
typed read/session evidence and its exact scope
```

The durable response is replayed verbatim for retries; a later account balance, plan or schema does not rewrite it. A receipt hash is an identifier/integrity input, not a proof that the implementation executed the contract correctly.

The typed evidence for a coordinated `SnapshotCut` describes the sealed IDC footprint and required authority/publication frontiers under its strong read contract. It is distinct from SPEC-005's one-group `SessionTokenV1`. Cross-group RYW, monotonic-read or causal dependencies require an explicitly qualified composite session contract and evidence transport; a C4 certificate or SDK token collection does not imply them. An unsupported session scope is rejected before speculative work can become an admission.

## 10. Failure, recovery and retention

| Event | Required behavior |
|---|---|
| Crash before durable `Reserved` | No positive vote exists; retry/status consults the transaction decision authority |
| Crash after `Reserved` | Reconstruct reservations; resolve transaction before conflicting admission |
| Crash after `PreparedYes` | Retain prepared bytes and reservations; recover `IN_DOUBT` until durable decision evidence arrives |
| Lost final response | Same RequestKey and request hash resolve to the original TxnId and exact receipt even if the client never learned TxnId |
| Certifier quorum partition | No new certification through an authority that cannot establish quorum leadership |
| Participant quorum unavailable | Do not fabricate its vote; existing prepared work can remain blocked |
| Commit durable, install incomplete | Retry installation; retain reservations; expose pending publication status |
| Revision/index metadata loss | Fail closed; repair from authoritative replicated state or invalidate evidence with fencing |
| Disk/flush error around a decision | Return unknown/pending when durability is uncertain; reconcile before final outcome |

`UNKNOWN`, `IN_DOUBT`, an elapsed timeout and a missing cache entry are not evidence of abort. Prepared state MUST NOT expire by TTL. A successful authoritative abort decision may be initiated before a final decision exists, including after a client deadline; a participant cannot decide that alone.

GC SHALL retain request-home mappings or authoritative references, prepared batches, reservations, token metadata, immutable results, decision references, publication evidence and all required old plans until the relevant protocol/snapshot/replication/retry horizons pass under SPEC-011. Detailed result retention may be bounded by the advertised retry contract, but an expired identity must remain fenced against reexecution. A request outside that horizon returns SPEC-012 `IdentityExpired`, not a new execution.

Resource exhaustion MUST reject or throttle new admission before destroying unresolved state. Metrics must identify the oldest unresolved transaction and pinned bytes without exposing user values.

## 11. Plan evolution interface

The certifier SHALL implement SPEC-009's conceptual lifecycle:

```text
close_admission(generation, boundary)
enumerate_outstanding(boundary)
resolve_or_transfer(outstanding, transition_id)
install_fence(transition_id, new_authority)
activate_generation(transition_certificate)
retire_generation(verified_horizons)
```

These names describe required responsibilities, not a stable RPC API.

Closing admission blocks new invocations under the old generation. A transaction admitted before closure has an immutable permit; it may complete only within the explicitly supported drain protocol. Retries/status for existing identities and replay of historically final operations remain legal after closure.

A barrier acknowledgement MUST account for every durable reservation, positive vote, unresolved prepare, committed-but-unpublished result and already-issued commitment. Rehoming a certifier requires a verified transfer of that state and fencing of its previous admission/execution authority. A process-local pointer swap is insufficient.

New invariants must validate both present state and live obligations such as confirmed reservations. The migration cannot make an accepted booking disappear to satisfy a new capacity bound. If refinement cannot be established, the transition blocks or is rejected; old final commitments remain effective.

## 12. Errors and observability

Required typed errors/statuses include:

```text
RequestIdentityMismatch; IdentityConflict; UnsupportedCertification; UnsafeSequentialContract
StalePlan; StaleSchema; StaleAuthority; MembershipChanged
SnapshotExpired; VersionConflict; PredicateConflict
IncompletePredicateCoverage; MissingInvariantWitness; UnfencedWriter
PreconditionRejected; PostconditionRejected; InvariantRejected
ArithmeticError; ReservationConflict; AuthorityUnavailable
TxnInDoubt; CommittedPendingPublication; IdentityExpired; SessionScopeMismatch
ProtocolCorruption; ResourceExhausted
```

Each response must separate final business rejection, retryable conflict, unavailable authority, pending durable outcome and corrupt state. A transport timeout MUST NOT masquerade as `PreconditionRejected`.

Required metrics:

```text
certify_attempt_total{operation,result}
certify_conflict_total{kind}
certify_duration_seconds{phase}
certify_read_tokens; certify_predicate_buckets
certify_reservations{mode}; certify_reservation_wait_seconds
certify_prepared_in_doubt; certify_oldest_in_doubt_seconds
certify_retained_bytes; certify_publication_wait_seconds
certify_epoch_reject_total; certify_membership_restart_total
certify_fallback_total{reason,target_class}
```

High-cardinality transaction/key values belong in opt-in traces, not metric labels. Diagnostic inspection SHALL show epochs, participant set, read evidence, conflict domains, durable vote/decision references and retention reason. Payload values remain redacted by default.

## 13. Acceptance scenarios

These are required test cases, not results already achieved. SPEC-010 owns execution and evidence publication.

| ID | Scenario | Acceptance condition |
|---|---|---|
| C4-001 | Two withdrawals read the same final unit | At most one conflicting candidate commits; retries cannot spend twice |
| C4-002 | Two expenses update different rows in one budget group | Aggregate-domain protection prevents write skew |
| C4-003 | Unique-key scan is empty; concurrent registration inserts into its gap | At most one registration succeeds; the empty range token detects the phantom |
| C4-004 | A row changes into or out of a filtered aggregate without changing its index key | Predicate revision invalidates the old certification evidence |
| C4-005 | Delete/reinsert identical value; key moves across index buckets | ABA does not validate an old token; every relevant old/new bucket is covered |
| C4-006 | Index rebuild/page split/range pagination during reads | Page movement does not corrupt tokens; changed index generation or incomplete coverage rejects safely |
| C4-007 | Crash at each point from `Reserved` to final publication | No lost positive vote, no expired reservation, no partial public commit |
| C4-008 | Duplicate prepare/commit; same ID with changed payload | Exact duplicates replay once; changed payload returns `IdentityConflict` |
| C4-009 | Certifier failover and minority old leader | New leader restores exclusions; old leader cannot authorize conflicting writes |
| C4-010 | Lost coordinator and arbitrary timeout after all yes votes | Transaction remains resolvable; no unilateral timeout-abort |
| C4-011 | Multi-IDC transfer with one participant install delayed | Final success and coherent reads wait for SPEC-008 publication |
| C4-012 | C3 holder remains disconnected during C4 strengthening | Admission blocks unless a proved composition rule covers all outstanding authority |
| C4-013 | Old-plan prepared transaction crosses generation change | Exact decision/result preserved or transfer blocks; no implicit loss of commitment |
| C4-014 | State guard differs from persistent invariant | Guard is checked at its specified observation point; all declared invariants still apply |
| C4-015 | Snapshot/token horizon expires or revision approaches overflow | Explicit retry/fencing; no accidental evidence reuse |
| C4-016 | Incompatible result contract or unsafe sequential operation | Compiler/runtime rejects; extra coordination does not bless the operation |
| C4-017 | Race request-home CAS and certification at two ingress nodes; lose reply before client learns TxnId | One request mapping/execution and exact receipt; changed request content returns RequestIdentityMismatch |
| C4-018 | Swap IDC generation and authority epoch; restore a token across placement/storage changes; forge certifier evidence | Typed binding, catalog admission and authentication reject without releasing another transaction's reservations |
| C4-019 | Request group-spanning session guarantees using one C2 token or an unqualified composite read plan | Scope rejects before certification; a strong snapshot alone does not claim an unspecified cross-group session |
| C4-020 | Encode receipt/prepare with unknown mandatory semantics or downgrade after preparation | SPEC-012 fails closed while retaining historical result, prepared bytes and reservations for compatible resolution |

Small executable models SHALL enumerate duplicate/reordered messages, leader changes, crash recovery and publication races. SPEC-010 FM-2 requires a checked decision/publication model, passing deterministic simulation and passing real-process fault campaign before a distributed C4 correctness claim; evolution additionally requires FM-3. The safety target is an obligation: every admitted observable history refines its contract under the stated failure model. Passing a finite test suite is evidence, not a universal proof.

## 14. Milestones

| Milestone | Deliverable | Exit criterion |
|---|---|---|
| C4-A | SPEC-011/012/013 interfaces, canonical tokens and conservative domain compiler | C4-002–006, C4-014–020 pass against the relevant reference models |
| C4-B | Single-IDC reservations and generated validator | Concurrent histories preserve exact results, guards and invariants; C4-001/008 pass |
| C4-C | Replicated authority, durable prepare and recovery | C4-007/009/010 pass under deterministic crash/reorder campaigns |
| C4-D | SPEC-008 composite publication and safe reads | FM-2 model/simulator/real-process gates and C4-011 pass with independent participant failures |
| C4-E | Mixed-class and SPEC-009 transitions | C4-012/013 pass without revoking final commitments |
| C4-F | Evaluation against equivalent C5/manual certification | Publish contention, aborts, useful successes and full coordination cost; retain C4 only if justified |

## 15. Source traceability

| Source requirement | This specification |
|---|---|
| SPEC-001 §§18, 25 — optimistic generated certification | §§0–7 |
| SPEC-001 §§20–21 — preservation and semantic conflict | §§1–2, 4–5 |
| SPEC-001 §§26–29 — identity, plan and proof artifacts | §§3, 6, 9 |
| SPEC-001 §§35–38, 47 — reads, IDC participants and composition | §§4, 8–9; SPEC-008 |
| SPEC-001 §§39–43, 49–51 — evolution and failure safety | §§10–11 |
| SPEC-001 §§68–73, 90 — correctness and C4 falsification | §§13–14 |
| SPEC-002 §§38–40, 59–71, 98 — opt-in evidence and prepared boundary | §§4–7, 10 |
| SPEC-002 §§88–89, 105–109 — retention and generation drain | §§10–11 |
| Research proposal §1 and prior-art notes: observables, guards, final commitments | §§1, 8–11, 13 |

Research context is taken from the repository's [proposal](../PROPOSTA-DE-PESQUISA.md) and [prior-art notes](../research/consistency-prior-art.md). This draft specifies implementation obligations; it adds no claim of novelty, completed formal proof or measured performance.
