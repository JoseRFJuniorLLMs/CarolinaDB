# SPEC-009 — Plan Evolution

**Status:** Draft specification; implementation and proof obligations remain open.  
**Date:** 2026-09-09  
**Depends on:** [SPEC-003](SPEC-003.md), [SPEC-004](SPEC-004.md), and the runtime contracts in [SPEC-005](SPEC-005.md)–[SPEC-008](SPEC-008.md).  
**Storage boundary:** [SPEC-002](SPEC-002.md), especially §§44, 59–71, 100–109.  
**Research source:** [PROPOSTA-DE-PESQUISA](../PROPOSTA-DE-PESQUISA.md) §1 and [consistency-prior-art](../research/consistency-prior-art.md).

## 1. Purpose and normative scope

This specification defines a conservative transition from an immutable plan generation `G` to `G+1`. It covers protocol replacement, operation/schema changes, IDC split/merge, authority transfer, and placement changes that affect semantic authority.

The observable contract includes final results, permitted reads, durability, dependencies, and resource authority. Preserving an invariant on the final stored rows alone is insufficient. A confirmed reservation cannot disappear because the new merge policy discards a losing operation.

MUST, MUST NOT, SHOULD, and MAY are normative within this proposed design. They do not describe implemented features. The baseline is a barrier and drain protocol. Concurrent execution of incompatible generations is unsupported. A later optimization requires a separate compatibility certificate and qualification evidence.

This refines [SPEC-001](SPEC-001.md) §§39–43: a change labelled `C3 -> C5` is not safe merely because its class number increases. It must first account for every outstanding right and every old writer. Catalog publication is not, by itself, revocation of an offline authority.

## 2. Failure and authority assumptions

Assume non-Byzantine processes, delayed/duplicated/reordered/lost messages, partitions, crash/recovery, and the durable storage guarantees of SPEC-002. The catalog uses an ordered replicated control plane. A successful catalog write meets its configured durable quorum requirement.

Wall-clock time MUST NOT establish revocation in the baseline. There is no bounded-clock lease assumption. A disconnected holder authorized to confirm locally may continue under the old contract until it durably closes that authority. Its migration therefore blocks until the holder is reconciled. An unreachable holder is not an empty holder.

A lost disk may exceed a plan's stated durability model. The migration protocol MUST NOT repair that loss by inventing resource capacity or discarding acknowledged results. Such a boundary enters a safety incident requiring evidence recovery. Readiness and availability MUST reflect this limitation.

## 3. Identities and generation domains

These identifiers have different scopes and MUST NOT be substituted:

| Identity | Scope and purpose |
|---|---|
| `CatalogGeneration` | Ordered catalog publication; monotonic control-plane revision |
| `PlanGeneration` and `PlanHash` | Immutable execution plan identity |
| `OperationId`, `operation_version`, `SchemaHash`, `ContractHash` | Exact operation and observable contract |
| `IdcId`, `IdcEpoch` | Semantic domain and authority incarnation |
| `PlacementEpoch` | Mapping of semantic domain to physical participants |
| `StorageEpoch`, `LocalCommitSeq` | One local storage history; never a cross-node timestamp |
| `OriginId` | Original semantic commit identity from SPEC-002 §102 |
| `MigrationId` | One durable migration procedure; all messages are idempotent by this ID |
| `StableRequestId`, `TxnId` | Logical client request and its execution, bound before effects |

`G+1` below denotes a candidate successor for a migration boundary; unrelated IDCs need not share a global data sequence. The catalog allocates all successor identities before activation. Reused names do not imply compatible hashes.

The records below are logical schema contracts, not a finalized RPC encoding. Persistent and network codecs MUST be deterministic, bounded, independently versioned, and reject unknown mandatory semantics.

## 4. Final outcomes and retained commitments

```text
FinalReceipt {
  receipt_version
  stable_request_id, txn_id, arguments_hash
  operation_id, operation_version
  contract_hash, schema_hash, plan_hash, plan_generation
  idc_epochs[], origin_ids[]
  outcome: COMMITTED | REJECTED
  exact_result_bytes, result_type_hash
  observation_token
  durability_profile, durability_evidence_ref
  decision_ref
}
```

The runtime MUST bind a stable request to one argument digest and one logical outcome. A retry with different arguments returns `RequestIdentityMismatch` before effects. A retry after migration returns the same retained final result, including an application-level rejection when the contract defines it as final. It MUST NOT re-evaluate a final request under `G+1`.

Admission refusals such as `MigrationInProgress` and transport timeouts do not create a final business outcome. Replies MUST distinguish them from `REJECTED`. `UNKNOWN` is a client knowledge state; it neither authorizes duplicate execution nor proves abort. A request whose status is unknown remains pinned to the original identity and resolver.

The exact result and deduplication record MUST share the durable decision boundary with the effects, or be deterministically recoverable from that boundary without consulting mutable state. Receipt hashes are integrity/identity checks; they are not independent proofs of semantic correctness or hostile tamper resistance.

Finality concerns the historical promise: a later authorized `cancel` can release a reservation if the contract permits cancellation. Migration cannot rewrite the original success as a failed reservation. External payments and API calls are outside this internal atomicity claim.

## 5. Observable refinement target

Let `H` contain invocations, replies, reads with visibility tokens, migration publications, and permitted failures. Let `Obs(H)` project away internal messages, physical page placement, and retries that do not change the logical request. `Allowed(C, F)` is the set of histories permitted by contract `C` under failure model `F`.

For a fixed contract and alternative execution plan, the target is:

```text
Obs(H_G -> G+1) belongs to Allowed(C, F)
```

For a contract change, a versioned transition specification `T(C_G, C_G+1)` defines which requests execute against each contract and how outstanding obligations survive:

```text
Obs(H_G -> G+1) belongs to Allowed(T(C_G, C_G+1), F)
```

The target is trace inclusion, not equality of replica-local snapshots or physical journal orders. Real-time ordering is checked only where the contract requires it. Liveness needs separately stated delivery, recovery, quorum, and admission fairness assumptions.

Required obligations are:

1. **E1 — Base validity:** the closed source state is reachable and satisfies the old contract/invariants.
2. **E2 — Closure:** all authorities that can issue conflicting old decisions have durably closed at a known boundary.
3. **E3 — Completeness:** every committed result, prepared transaction, causal prerequisite, right, reservation, and transfer is represented exactly once in the migration input.
4. **E4 — State mapping:** the deterministic mapping establishes new invariants and preserves outstanding obligations and results.
5. **E5 — Publication exclusion:** no two incompatible generations can admit new executions for an overlapping semantic scope.
6. **E6 — Recovery:** replay of any migration prefix neither manufactures a decision nor re-enables retired authority.
7. **E7 — Observation:** legal session/read tokens remain interpretable or cause explicit waiting/error without silently weakening the requested guarantee.

A stored certificate enumerates evidence and assumptions for these obligations. It MUST label unproved obligations. This document does not claim that the general composition theorem has been proved.

## 6. Migration boundary and catalog records

The compiler constructs the semantic closure of changed operations, protected keys/ranges, invariants, causal dependencies that require a barrier, outstanding resource issuers, and cross-IDC transactions. Every writer that can invalidate this closure belongs to the boundary, even if it uses another physical shard.

Unbounded or unknown scope selects a conservative enclosing domain. If the enclosing domain cannot be enumerated or fenced, return `UnsupportedMigrationScope`.

```text
MigrationRecord {
  migration_id, record_version, phase
  expected_catalog_generation
  source_plan_hashes[], target_plan_hashes[]
  source_idc_epochs[], target_idc_epochs[]
  source_placement_epoch, target_placement_epoch
  boundary_digest, participant_manifest
  transform_hash, transition_contract_hash
  close_certificates[], drain_digest
  target_install_certificates[]
  activation_decision_ref, retirement_frontiers
}

CloseCertificate {
  migration_id, authority_id, source_epoch
  durable_fence_ref, admitted_request_frontier
  committed_origin_frontier_with_holes
  pending_txn_digest, rights_and_transfer_digest
  final_result_frontier, causal_frontier
}
```

The participant manifest is fixed before closure. Authority grants, membership changes, and migrations touching an overlapping scope serialize through the same catalog compare-and-swap boundary. A holder cannot delegate authority while closing; previously delegated holders must be included.

A vector with holes MUST preserve the holes. Taking a maximum origin sequence is not evidence that all earlier messages arrived. Close certificates reference recoverable records, not merely an unverifiable hash of lost state.

## 7. State machine

```text
PROPOSED -> COMPILED -> CLOSING -> DRAINING -> VALIDATING
         -> INSTALLING -> READY_TO_ACTIVATE -> ACTIVE -> RETIRED

Before ACTIVE: phase may enter BLOCKED(reason)
Before CLOSING: may become CANCELLED
After durable closure: resume old behavior only through a successor generation
After ACTIVE: change requires another forward migration
```

`BLOCKED` retains the prior phase and evidence; it is not a decision to abort business transactions. Retrying the same `MigrationId` resumes that phase. No timer advances it.

The baseline never deletes a persistent fence to roll back. A cancelled plan that has already closed writers needs a fresh, explicitly published generation, possibly reusing the old operation semantics. This avoids delayed close messages revoking a newly reused old epoch.

## 8. Barrier and drain algorithm

### 8.1 Compile and register

Compile candidate definitions using SPEC-003/004. Check node capabilities and codec support. Produce the scope closure, operation-version compatibility report, deterministic state transform, and explicit transition contract. A catalog compare-and-swap registers the migration and locks overlapping authority changes.

Validation on a live snapshot is only a preflight. It cannot replace validation after the source boundary is closed.

### 8.2 Close admission

Send `CloseAuthority(migration_id, source_epoch, scope)` to every authority in the manifest. Each authority serializes closure with local admission and durable grant issuance:

1. Stop admitting new source-generation requests in scope.
2. Persist the admission fence and a recoverable set/frontier of previously admitted requests.
3. Continue resolving those admitted requests under the old exact plan, including final rejections and prepared decisions.
4. Persist a close certificate before acknowledging closure.

An authority crash before the fence ACK is retried and inspected. After recovery it MUST restore the fence before serving. An old gateway cannot bypass the storage/runtime admission check. An isolated authority not yet closed may still issue valid old decisions; the catalog remains in `CLOSING` and MUST NOT activate the replacement.

### 8.3 Drain and reconcile

Collect all close certificates. Resolve admitted requests and distributed prepares using their original decision authorities. No prepared transaction is aborted merely because a migration wants to finish. If an authoritative abort is selected before a commit decision under SPEC-008, record it normally.

Bring the migration snapshot to the union of closed semantic frontiers and their causal closure. Fetch missing origin records and exact results. Deduplicate by original identity. Settle every resource transfer to a unique terminal disposition; the baseline blocks on unresolved transfers rather than translating in-flight transfer state between epochs.

The drain is complete only when there are no live source-generation business requests, prepared decisions, or unresolved transfers in the boundary. Historical committed envelopes may still require installation at lagging replicas; they remain valid history and cannot create new authority.

### 8.4 Validate and transform

Take a pinned, causally closed source snapshot after drain. It includes user data, reservations, authority state, dedupe/results, causal frontiers, decision references, and retired origin incarnations. Apply a deterministic, total, bounded transform to a staging namespace.

Check old-to-new invariant mapping, identity preservation, numeric overflow, uniqueness/referential integrity, and the outstanding commitment mapping. If the new rule contradicts a confirmed reservation, fail with `CommitmentIncompatible`; do not silently cancel it. A separately authorized domain operation may change the business state before a later migration attempt.

No user-visible target writes occur during validation. The source snapshot and transformation manifest stay pinned until installation/activation is recoverable.

### 8.5 Install target state

Targets install staging data and protocol metadata atomically at their local storage boundary, with `admission_enabled = false`. Installation certificates bind the migration, semantic digest, target epochs, durable snapshot, and exact plan hashes.

The coordinator requires every target participant needed by the new contract and its durability profile to be ready. A target without the complete activation certificate MUST remain closed. Copying source `LocalCommitSeq` as if it were target MVCC time is forbidden; rematerialize with target-local versions and preserve semantic identities.

### 8.6 Activate

The catalog durably commits one `ActivateMigration` record conditioned on the registered source generation, complete close/drain evidence, successful validation, and required target installation certificates. This is the unique metadata activation decision.

Targets persist that decision and enable new admissions only after validating it against their installed state. Publication is asynchronous across physical processes; source authorities are already fenced, and lagging targets remain unavailable until ready. A routing refresh cannot substitute for this admission gate.

Historical requests query status by their original identity. New invocations use the published generation. A client pinning a retired incompatible operation version receives `OperationVersionRetired`; the runtime does not substitute a new contract.

### 8.7 Retire and reclaim

Retirement requires activation recovery at required targets, replica/bootstrap safety, released source snapshots, no unresolved source decisions, and a durable mapping for historical request/session tokens. Retention follows §13. The catalog records why each remaining old artifact is pinned.

## 9. Resource and protocol migration cases

| Change | Mandatory treatment |
|---|---|
| C3 to C5 | Close every old rights issuer/holder; account for consumption, reservations and transfers; initialize ordered state from the complete semantic cut |
| C5 to C3 | Stop the old sequencer's admissions; drain decisions; allocate exclusive rights once from validated remaining capacity |
| C1/C2 to an ordered path | Include all closed origin prefixes and causal dependencies; do not discard old asynchronous commits merely because the target plan is ordered |
| Increase capacity | Treat additional capacity as an authorized business delta or explicit transition rule; create rights only once |
| Decrease capacity | Revoke/reconcile outstanding usable rights; preserve reservations and successful consumption; reject a bound incompatible with those obligations |
| IDC split | Prove no invariant needs atomic treatment across the proposed split, or retain an explicit composite coordinator |
| IDC merge | Lock/fence the union of source domains and resolve cross-domain work before allocating merged authority |
| Placement move | Preserve logical identity, decisions, dedupe, frontiers and rights; storage shard boundaries do not define semantic ownership |

For escrow, the source ledger MUST use one consistent accounting model such as `T = C + H + U + X` from SPEC-006. Staging a copy of this ledger does not make those copied rights spendable. Target activation transfers usability after old usability is closed.

Example: `T=10`, committed consumption `C=4`, held reservations `H=3`, usable rights `U=3`, no transfers. A new total of `6` conflicts with `C+H=7` and is rejected. A new total of `8` permits at most one usable right after reconciliation. A disconnected old holder of the three rights prevents either change from activating.

## 10. Sessions, reads and old messages

Read/session tokens bind semantic frontiers, relevant contracts and IDC epochs. They never consist solely of a foreign `JournalLsn`. A successor snapshot MUST dominate retained dependencies after mapping old IDC identities to successors.

If the mapping is not ready, return `SessionFrontierUnavailable` or wait subject to the request deadline. A deadline response is not permission to return a weaker read. The baseline may block all in-scope reads during activation; serving pinned old snapshots is optional only when the requested contract allows them.

An atomic multi-IDC read continues to use SPEC-008's decision/visibility protocol during migration. Combining independently current per-IDC snapshots is not automatically an atomic snapshot.

Messages are handled by purpose:

| Message | After source admission closure |
|---|---|
| New invocation under retired authority | Reject before effects |
| Retry/status of an existing request | Resolve original identity/result |
| Already committed semantic envelope | Validate origin/history and install idempotently or answer already included in migrated base |
| Delayed rights grant or transfer | Resolve against the closed transfer manifest; never grant fresh target rights |
| Prepared decision | Resolve using original durable authority and identity; block migration if unresolved |
| Unknown plan/mandatory codec | Quarantine/fail closed; no guessed interpretation |

No historical replay path may execute arbitrary old application code or create a fresh request.

## 11. Failure recovery table

| Failure boundary | Required recovery |
|---|---|
| Candidate compilation fails | Old plan remains active; publish diagnostics only |
| Some close ACKs missing | Keep migration blocked; do not infer fencing from silence |
| Authority restarts after fence fsync | Restore fence, then reconcile admitted requests and return certificate |
| Migration coordinator fails | Successor resumes the same durable MigrationRecord; duplicated phase messages are idempotent |
| Target installs but activation absent | Target remains staged/closed; source stays in its recorded phase |
| Activation commits but reply is lost | Query catalog by MigrationId; never create a competing activation |
| One target misses activation | Target remains unavailable until decision and matching installed state are recovered |
| Source replica returns after retirement | Fence and bootstrap it; it cannot resume old authority or resurrect state |
| Corrupted required history | Fail closed and report storage/recovery error; do not skip commitments |

An operator's desire to restore availability is not evidence that old acknowledged work did not occur.

## 12. Compatibility specialization and deferred work

The baseline supports disjoint migrations concurrently only when the compiler proves disjoint semantic closures. Authority-grant operations remain serialized for each affected closure.

Future overlap of `G` and `G+1` requires a compatibility certificate covering mixed-version operation pairs, observations, resource accounting, all phase interleavings, and retirement. Matching invariants or matching field layouts is insufficient. Unknown analysis falls back to barrier/drain, not speculative overlap.

Live schema repair, arbitrary transformation code, automatic forced recovery after permanent loss, leases, Byzantine fencing, and unbounded offline upgrade are outside v1. Deadlines MAY bound how long an administrative request waits; they cannot force a successful unsafe transition.

## 13. Retention and bounded resources

Migration registers pins with MVCC GC and journal consumers. Retain everything needed by active snapshots, replica catch-up, backups, unresolved decisions, close certificates, dedupe, final-result retry policy, and token translation. Use semantic-origin horizons and local journal cursors in their own domains.

Exact results may have a declared finite retention period, but expiry cannot permit re-execution. After result eviction, retain an anti-reexecution tombstone or a safely retired request namespace. A retry returns `ResultExpired` with available outcome evidence. If neither a tombstone nor namespace rejection can be guaranteed, keep the record. Arbitrary ancient IDs cannot be treated as new simply to save disk.

An offline node older than the safe retention floor must rebootstrap. Admission MUST reject old identities/authority even after raw journal reclamation. Storage pressure causes backpressure or a blocked migration; it never causes deletion of in-doubt evidence.

## 14. Errors, diagnostics and observability

Required typed errors include `MigrationInProgress`, `UnsupportedMigrationScope`, `UnfencedAuthority`, `UnresolvedPrepared`, `UnresolvedTransfer`, `CommitmentIncompatible`, `StaleCatalogGeneration`, `TargetNotReady`, `OperationVersionRetired`, `SessionFrontierUnavailable`, `RequestIdentityMismatch`, and `ResultExpired`. Every error includes the migration/request identity and whether any final decision is known.

Proposed diagnostic interface:

```text
astra plan diff <source> <candidate>
astra migration inspect <migration_id>
astra migration resume <migration_id>
astra txn status <txn_id>
```

`inspect` is read-only. `resume` only re-enters the recorded protocol; it has no force-success flag. Reports include current phase, scope, missing authorities, pinned data, unresolved work, target readiness, and proof status. Do not log raw client arguments/results by default.

Measure phase duration, blocked authority count, source drain backlog, unresolved transfers/prepares, staging bytes, retention bytes, rejected stale invocations, preserved-result retries, and unavailable read duration. High-cardinality request/IDC identifiers belong in traces, not unbounded metric labels.

## 15. Acceptance scenarios

| ID | Schedule | Required outcome |
|---|---|---|
| EV-01 | C3 holder partitions with usable rights; request C5 migration | Migration cannot activate; old legitimate consumption remains accounted for |
| EV-02 | Crash holder after durable close but before close reply | Recovery keeps closure; retry returns same frontier/certificate |
| EV-03 | Final reservation succeeds; tighten capacity below consumption plus held reservations | Candidate rejected without erasing success |
| EV-04 | Crash during source/target snapshot installation | At most one generation has admission authority; staging cannot be spent |
| EV-05 | Activation durable; coordinator/client response lost | One activation on retry; exact earlier business receipts remain resolvable |
| EV-06 | Delayed source-generation invocation after activation | Reject without mutation; distinguish valid committed replay |
| EV-07 | Old committed C2 envelope arrives after source snapshot copy | Drain detects missing dependency or target recognizes inclusion; no lost commit |
| EV-08 | Prepared cross-IDC transfer during closure; decision authority partitions | Remain blocked/in doubt; neither invent abort nor expose half transfer |
| EV-09 | Source origin sequence has a hole | No contiguous frontier advance across the missing decision |
| EV-10 | Split IDC with a previously hidden cross-domain invariant | Compiler rejects split or retains composite coordination |
| EV-11 | Retry known request with changed arguments under new schema | RequestIdentityMismatch; no new effects |
| EV-12 | Old causal session token after IDC merge | Mapped dependency-satisfying read or explicit unavailability; no silent local downgrade |
| EV-13 | Journal/result GC followed by ancient retry and stale replica return | No duplicate execution or authority resurrection |
| EV-14 | Cancel after one authority durably closed | Resume through successor generation; never remove/reuse the old fence |

Each schedule MUST run in the deterministic simulator and, where supported, against real processes with crash/restart. The oracle checks receipts and authority accounting throughout the history, not only final row values.

## 16. Implementation gates and traceability

1. **E0:** implement immutable migration records, scope locking and deterministic transforms; test schema/identity mismatch without network protocols.
2. **E1:** model close/drain/activate under partitions and coordinator crashes; demonstrate EV-01/02/04/05/14.
3. **E2:** integrate source fences, dedupe/results and storage pins; pass all supported local crash boundaries.
4. **E3:** integrate C1/C2/C3/C5 transitions and cross-IDC decision drain; pass EV-01–14.
5. **E4:** evaluate disruption and preserved observations with SPEC-010. No online overlap optimization before baseline qualification.

| Requirement origin | Resolution here |
|---|---|
| SPEC-001 §§39–43 and 52 | Explicit authority closure and immutable version admission rules |
| SPEC-002 §§44, 59–71, 108–109 | Staging, exact durable decisions, old history replay and retained identities |
| Proposal §1: observable contracts | FinalReceipt, transition contract and E1–E7 obligations |
| Research: offline rights and evolution | Unreachable holders block incompatible migration |
| Research: narrower compositional contribution | Trace-refinement target; proof remains a stated deliverable |

Success means every migration prefix has a defined recovery path and every final response remains explainable under its recorded contract. Performance and scientific novelty remain subjects for measurement and proof.
