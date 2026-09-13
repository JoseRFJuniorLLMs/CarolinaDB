# SPEC-006 — Escrow Runtime

**Subtitle:** Bounded Resources, Exclusive Rights, Transfers and Recovery
**Status:** Draft 0.2 — proposed protocol; proof and implementation are pending
**Date:** 2026-09-09
**Depends on:** [SPEC-001](SPEC-001.md), [SPEC-002](SPEC-002.md), [SPEC-003](SPEC-003.md), [SPEC-004](SPEC-004.md), [SPEC-005](SPEC-005.md)
**Integrates with:** [SPEC-008](SPEC-008.md), [SPEC-009](SPEC-009.md), [SPEC-010](SPEC-010.md)
**Normative registries and protocols:** [SPEC-011](SPEC-011.md) (catalog and typed authority), [SPEC-012](SPEC-012.md) (request identity and codecs), [SPEC-013](SPEC-013.md) (security and trust)
**Reference implementation:** Rust stable; initially an `carolina-runtime` module
**Normative terms:** MUST, MUST NOT, SHOULD and MAY define requirements of this draft.

## 1. Decision

C3 SHALL execute eligible bounded-resource operations using exclusive logical rights. A successful local consumption atomically commits the business effect, the rights transition, the immutable invocation outcome and replication identity. Rights are durable protocol state, never a cache reconstructed from an arbitrary visible business balance.

The first implementation SHALL use a fixed three-node topology; integer or fixed-decimal resources; one active writer per rights holder; explicit transfers; and barrier-based authority migration. It does not promise that a request succeeds merely because some disconnected node holds unused rights. There is no timeout-based recovery of authority.

This is an implementation specification and model-checking target. Escrow and bounded counters have direct predecessors identified in the repository's research notes. This document does not claim novelty, a completed proof, measured availability or a production implementation.

## 2. Supported resource model and compiler boundary

An admitted `EscrowPlan` MUST identify the invariant, resource key, exact numeric units, bound, consumption/production effects, return contract, authority scope, independent authority/transfer-decision durability policies and all interfering operations. The compiler MUST prove that the resource decomposes into exclusive quantities and that the declared operations preserve the invariant when those quantities are respected. A requested failure-tolerance or availability contract that those policies cannot supply MUST be rejected.

| Invariant | Spendable slack | Effect consuming rights |
|---|---|---|
| `x >= L` | `x - L` | Decrement of x |
| `x <= U` | `U - x` | Increment of x |
| `SUM(x_i) <= U` | `U - SUM(x_i)` in a fixed declared scope | Increase of that scoped sum |
| `SUM(x_i) >= L` | `SUM(x_i) - L` in a fixed declared scope | Decrease of that scoped sum |

The bound and aggregation scope are immutable within a resource generation. A mutable bound, changing group membership, account closure, uniqueness or a second coupled constraint requires a compatible stronger/composite plan or SPEC-009 migration. Independent escrow for two bounds is not automatically a proof that an operation touching both is safe.

Quantities SHALL be nonnegative exact integers in the resource's declared scale. The logical model uses mathematical integers. The implementation SHALL use checked arithmetic in its fixed-width representation and reject out-of-range inputs before state change. Rounding, saturating arithmetic and wrapping arithmetic MUST NOT create rights. Float quantities are outside version 1.

Rights production is admitted only for a semantic operation that demonstrably increases slack. Importing a replica, copying a business balance, receiving a duplicate deposit or seeing unused capacity does not authorize production. Global exact remaining capacity is a read contract requiring its own coordination; escrow does not supply it for free.

## 3. Conservation and business-state interpretation

For one resource generation define the following logical, authoritative quantities:

- `T`: initial spendable capacity plus all committed authorized additions of capacity.
- `C`: quantity permanently consumed by committed business operations.
- `H`: quantity held by active business reservations.
- `U`: sum of usable rights at exclusive logical holders, including any allocator pool.
- `X`: quantity locked in a rights transfer that has not yet been installed as usable rights at its receiver.

The required conservation equation is:

```text
T = C + H + U + X
T >= 0; C >= 0; H >= 0; U >= 0; X >= 0
available_business_capacity = T - C - H = U + X
```

`X` includes both donor-prepared transfers and donor-committed transfers awaiting receiver installation. A receiver's durable `ACCEPTED` entry has no spendable quantity. Once it installs a committed transfer, the quantity belongs to `U` and no longer to `X`, even if the donor has not received the acknowledgment. The equation is evaluated over logical decisions and installation facts in a valid global history/cut; stale node gauges cannot simply be added together.

Replicas contain multiple physical copies of business stock and rights-ledger records. These copies are not separate resources. A logical holder may have several durability replicas but exactly one active spending authority. The verifier counts a holder, reservation, consumption or transfer ID once. It MUST NOT sum `Product.stock` across replicas or count replicated incoming grants as new rights.

This refines the schematic conservation equation in SPEC-001 §23: “already materialized capacity” is not an additional replica-owned balance to add to usable rights. Version 1 uses the explicit non-overlapping categories above.

For an inventory representation with `free`, `reserved` and cumulative `consumed`, the complete logical state satisfies:

```text
free = U + X
reserved = H
consumed = C
free + reserved + consumed = T
```

A replica's causally bounded observation may contain only a subset of committed effects and must identify that cut. It cannot spend from its displayed `free`; admission uses its confirmed `U_holder`. For a raw lower-bound scalar without business reservations, `H = 0` and `x - L = U + X`. A reservation contract must specify whether reserving decreases a `free` field or preserves a separate total-stock field. The runtime MUST NOT silently conflate those schemas.

## 4. Logical persistent structures

Types below are logical schemas from the SPEC-011 identity taxonomy. They are not a Rust memory layout or a finalized ABI. SPEC-012 owns canonical field encodings, discriminants, quantity widths, authenticated evidence envelopes and hash inputs; its cross-platform fixtures MUST pass before compatibility is claimed. Wire records and persisted escrow metadata SHALL have separate format versions.

```rust
struct ResourceRef {
    invariant_id: InvariantId,
    resource_key: CanonicalKey,
    resource_generation: ResourceGeneration,
}

struct HolderRef {
    holder_id: HolderId,          // logical allocation owner, not replica count
    authority_epoch: HolderAuthorityEpoch,
}

struct EscrowPlanV1 {
    format_version: u16,
    plan_hash: PlanHash,
    plan_generation: PlanGeneration,
    schema_hash: SchemaHash,
    contract_hash: ContractHash,
    invariant_id: InvariantId,
    idc_binding: IdcBinding,
    units: ExactNumericDomain,
    bound: CanonicalBound,
    authority_policy_id: Hash256,
    authority_durability: AuthorityDurabilityPolicy,
    transfer_decision_durability: TransferDecisionDurabilityPolicy,
    client_result_durability: DurabilityPolicy,
    allowed_effects: Vec<EscrowEffectRule>,
}

struct HolderStateV1 {
    format_version: u16,
    resource: ResourceRef,
    holder: HolderRef,
    escrow_epoch: EscrowEpoch,
    active_writer: NodeId,
    fence_catalog_generation: CatalogGeneration,
    usable: Quantity,
    reservations: Map<ReservationId, ReservationRecord>,
    outgoing: Map<TransferId, OutgoingTransfer>,
    incoming: Map<TransferId, IncomingTransfer>,
    produced_total: Quantity,
    consumed_total: Quantity,
    causal_context: CausalContextV1,
    state_version: VersionStamp,
}

struct TransferTermsV1 {
    format_version: u16,
    transfer_id: TransferId,
    resource: ResourceRef,
    escrow_epoch: EscrowEpoch,
    donor: HolderRef,
    receiver: HolderRef,
    amount: Quantity,
    plan_hash: PlanHash,
    plan_generation: PlanGeneration,
    idc_binding: IdcBinding,
    authority_durability_policy_hash: Hash256,
    transfer_decision_durability_policy_hash: Hash256,
    terms_hash: Hash256,
    required_context: CausalContextV1,
}

enum DonorState { Prepared, Committed, Aborted }
enum ReceiverState { Accepted, Applied, Aborted }

struct TransferDecisionV1 {
    terms: TransferTermsV1,
    decision: DonorState,          // Committed or Aborted on final wire decision
    decision_origin: OriginId,
    decision_digest: Hash256,
    donor_authority_proof: AuthorityEvidence,
    durability_evidence: TransferDecisionDurabilityEvidence,
}
```

`ResourceGeneration` versions the resource definition. `EscrowEpoch` identifies its conserved allocation-manifest lineage; an ordinary transfer preserves it. `HolderAuthorityEpoch` identifies one holder's exclusive authority incarnation; holder replacement does not implicitly change allocation lineage or mint rights. `IdcBinding` carries separate `IdcId`, `IdcGeneration` and `IdcAuthorityEpoch`. `PlanGeneration`, `CatalogGeneration`, `OriginEpoch`, `StorageEpoch`, `PlacementEpoch` and `MembershipGeneration` retain SPEC-011's separate scopes and validation rules. A higher storage epoch is not a new grant. A higher catalog generation is not proof that an old offline holder stopped spending.

Durable keys SHALL live in protected `ESCROW`, `IDC_META`, `REPLICATION` and `TXN_STATUS` namespaces. Every semantic commit follows SPEC-005's `OriginId`, dot, immutable digest and group-scoped session rules. Each transfer phase has a stable internal phase transaction ID and origin; `TransferId` binds the whole protocol. Protocol-only phases use SPEC-002's internal durable batch identity and MUST NOT invent a client `StableRequestId`. Repeating a phase does not allocate a new operation. Changed transfer terms return internal `IdentityConflict`; changed client content under one `RequestKey` returns SPEC-012 `RequestIdentityMismatch`. Both leave the original record intact.

## 5. Initialization and authority

Creation of a resource generation is a SPEC-011 coordinated catalog operation. It validates the existing business state against the invariant, computes initial slack exactly, and records one immutable genesis allocation manifest with its `EscrowEpoch`. All initial allocations plus any unallocated pool MUST sum to T. Each manifest/grant has a stable identity and is installed once under authenticated SPEC-013 authority evidence. An unallocated pool is itself a logical holder with an exclusive allocator; it is not an extra uncounted source.

Each active holder MUST have one admitted writer and a durable exclusive writer fence. Local file ownership prevents two processes opening one directory; it does not fence a disconnected copy on another machine. Version 1 uses stable holder placement and explicit barrier handover. A passive replica can retain its holder's records for recovery but cannot spend them.

Offline operation is permitted only while the same admitted holder retains its confirmed rights and durable authority under the supported failure model. Its authority cannot be forcibly reassigned while it is unreachable merely because the control plane elected a new leader. If immediate revocation is required, the plan needs a different admission policy with its own communication/lease assumptions; that policy is outside the baseline.

New business invocations use SPEC-012's `RequestHome = route(RequestKey)` before the home durably CAS-binds the request key to one globally unique `TxnId` and immutable request hash. A C3 plan SHALL select a rights holder at that home or forward to a bound holder without creating a second invocation identity. The request-to-holder binding is durable before its first business decision. A retry at another region, even without a known `TxnId`, may resolve the original exact receipt or wait for the home; it MUST NOT spend a second region's rights while the original outcome is unknown.

### 5.1 Independent durability policies

`AuthorityDurabilityPolicy` owns recoverability of grants, active-writer fences, admission boundaries and every holder transition needed to reconstruct C/H/U/X. `TransferDecisionDurabilityPolicy` owns the unique final COMMITTED/ABORTED decision and enough terms, debit/acceptance evidence and historical artifacts to replay it. `client_result_durability` owns the client's final receipt failure scope. All three are immutable plan inputs with separate policy hashes, configured durability sets and qualification evidence; one policy cannot be inferred from another.

| Policy choice | Required evidence and availability consequence |
|---|---|
| Authority `LocalStable` | Exclusive holder state crosses its local stable barrier before dependent admission; permanent loss can freeze its allocation, and unreachable old authority cannot be replaced |
| Authority `RequiredDurableCopies` | Each specified authority-state copy durably retains every relevant transition before dependent spend/grant; required-copy loss blocks the transition, and copies alone do not fence an old owner |
| Transfer decision `LocalStable` | Final decision and reconstructible terms/debit evidence survive local crash/restart; loss of the sole copy may leave X frozen |
| Transfer decision `QuorumDurable` | The plan names a fixed decision-evidence group and `MembershipGeneration`; its intersecting quorum commits the unique decision and required reconstruction state before export or refund; unavailable quorum blocks completion |

The baseline profile selects both authority and transfer-decision `LocalStable`. `RequiredDurableCopies` and `QuorumDurable` are explicitly gated capabilities, not automatic failover claims: they require SPEC-010 FM-1 plus deterministic and real-process fault qualification under the configured failure scope. A quorum evidence group may preserve a donor decision without granting any replacement writer permission to spend. Promotion still requires the independently qualified authority/fencing protocol in §11.

A plan may combine a `LocalStable` client result with `QuorumDurable` transfer decisions. It MUST report the resulting transfer quorum dependency instead of claiming all C3 work remains disconnected. Final success waits for every policy boundary relevant to its business/rights transition; choosing a cheaper client receipt cannot bypass a stronger authority barrier. An acknowledgment identifies the policy, exact record digest, configured members and durable frontier. Memory receipt, eventual replication, a raw hash or a quorum of arbitrary business replicas is insufficient.

## 6. Business operation transitions

All transitions below occur for the exclusive active holder and require `q > 0` unless an operation explicitly admits zero. The precondition is checked under the same local guards as the commit. The table uses the inventory interpretation from §3.

| Operation | Preconditions | Logical accounting | Business mutation |
|---|---|---|---|
| Direct consume/sell | `U_holder >= q` | `U -= q; C += q` | `free -= q; consumed += q` |
| Produce/restock | Plan proves new slack q | `T += q; U += q` | `free += q` and declared supply evidence |
| Reserve | `U_holder >= q`; new reservation ID | `U -= q; H += q` | `free -= q; reserved += q` |
| Consume reservation | Matching ACTIVE reservation of q | `H -= q; C += q` | `reserved -= q; consumed += q` |
| Release reservation | Matching ACTIVE reservation of q | `H -= q; U += q` | `reserved -= q; free += q` |

Reservation state is `ABSENT -> ACTIVE -> CONSUMED | RELEASED`. Final states never return to ACTIVE under the same ID. Version 1 SHALL use whole-reservation consume/release; partial operations require explicit sub-reservation identities and a later admitted rule. A concurrent consume/release race is decided once by the owner under local atomic validation. A retry of the winning request returns its original result; a different losing request returns the defined terminal-state error.

Business reservations and in-transit rights are different: `H` encumbers capacity for a business promise, whereas `X` only changes placement of otherwise free capacity. A rights transfer MUST NOT fabricate a reservation, consume business inventory or increase T.

An expiration timestamp is not a rights-reclamation algorithm. Version 1 SHALL release a reservation only through a named operation at its authority, with its durable identity and the contract's checked expiry/cancellation predicate. A timeout at a client or remote replica cannot return H to U. Contracts promising automatic expiration require explicit clock assumptions and a specified authoritative expiration operation.

## 7. Atomic local consumption and production

The execute path SHALL:

1. Resolve the durable request-key mapping and exact outcome; verify the plan, typed IDC/resource/allocation/holder bindings, SPEC-011 authority admission and absence of a local fence.
2. Satisfy the client/prerequisite causal context. Validate the declared business precondition and return contract.
3. Acquire guards for the holder/resource, business rows, reservation/transaction keys and local index changes in canonical order. Recheck usable rights, expected versions and fences.
4. Build a single `CompiledBatch` containing full request/operation/schema/contract/plan/IDC identity; business mutations; holder rights/reservation transition; exact terminal result and commitments; origin/dot identity; causal coverage; and semantic outbox record.
5. Commit through SPEC-002 and cross the relevant authority and client-result durability boundaries before returning SPEC-012's `FinalReceiptV1`. Pending remote durability never permits the same invocation to execute elsewhere.

Checks, debit and persistence cannot be separated by an unprotected window. Two workers each seeing `usable = 1` cannot both sell one. Crash recovery yields the complete business/rights transition or none. A disk/fsync error gives no success response; if outcome is uncertain, query the same transaction ID.

Production changes the business value and creates rights only at its designated holder in that same batch. Remote replication applies the business effect and ledger fact idempotently but MUST NOT create usable rights at every replica. Release is a transfer from H back to U and does not increase T.

For operations consuming multiple resources, local co-residence alone is not a proof of separability. When a published plan verifies all constraints and all mutations fit one kernel boundary, the entire debit/effect set MUST be one atomic batch. If holders or storage authorities differ, version 1 routes through SPEC-008's admitted atomic composition or returns `UnsupportedComposition`; committing one side and hoping to compensate is not the same contract.

## 8. Rights transfer protocol

The donor is the sole final decision authority for one transfer. The protocol refines SPEC-001 §24 with explicit durable boundaries. Only the transfer of rights is specified here; a transfer of business quantity between accounts/resources has a different contract and may require multi-IDC atomicity.

```text
donor:    ABSENT -> PREPARED -> COMMITTED
                          \-> ABORTED
receiver: ABSENT -> ACCEPTED -> APPLIED
                          \-> ABORTED (donor's final abort only)
```

### 8.1 PREPARE_TRANSFER

The donor validates exact terms, typed resource/allocation/holder/IDC bindings, destination membership and `usable >= q`. In one durable atomic batch it subtracts q from usable, creates the `PREPARED` outgoing record and records the dependency context justifying these rights. The quantity moves `U -> X`. Only after the configured `AuthorityDurabilityPolicy` barrier for this complete transition may it send `PrepareTransfer(terms)`. A failure before that external acknowledgment is reconciled by transfer identity; it does not authorize a second debit or a guessed refund.

A duplicate prepare with identical terms returns the existing state. Insufficient rights refuses without a transfer record or business mutation. A transfer ID cannot be reused for a different amount, receiver, resource or generation.

### 8.2 ACCEPT_TRANSFER

The receiver verifies SPEC-013 authentication plus the donor's SPEC-011 admitted authority and immutable terms, checks its own admission fence, and ensures it can retain the record. It writes an `ACCEPTED` incoming record and meets the receiver's authority-state durability policy before returning `AcceptTransfer`. Acceptance does not increase usable rights, publish a spend capability or modify business stock. Acceptance includes the terms hash, receiver `HolderAuthorityEpoch` and durability evidence.

If required business dependencies are absent, the receiver MAY persist acceptance and request them, but MUST NOT install usable rights until they are satisfied. If the receiver is fenced, it rejects new acceptance except for an explicitly permitted historical drain/migration.

### 8.3 COMMIT_TRANSFER

After a matching durable acceptance, the donor atomically changes `PREPARED -> COMMITTED` and records an immutable final decision. The reserved debit becomes irrevocable. It MUST cross both the applicable authority-state barrier and `TransferDecisionDurabilityPolicy` before sending `CommitTransfer(decision)`. The final record carries verifiable policy evidence; a local COMMITTED marker alone cannot satisfy a configured quorum decision requirement. Loss of quorum after local commit keeps the debit unavailable and the decision pending required evidence; it cannot authorize abort.

The final record proves that q cannot be spent again by the donor, including after recovery. A sender's memory, a network send or receiver acceptance alone is insufficient evidence. Before receiver installation, the quantity remains X. Committed transfer evidence MUST bind its exact terms, plan, epochs and origin. Authorized authenticated donor evidence is required; a digest supplied by an arbitrary peer is not authority.

### 8.4 INSTALL_TRANSFER

The receiver verifies the committed decision, authentication and complete configured decision-durability evidence against its accepted terms and typed bindings, and waits for the required causal context. In one atomic batch it checks that the transfer is not already APPLIED, adds q to its usable rights, writes the `APPLIED` incoming record and records the transfer's causal/replication identity. The quantity moves `X -> U_receiver`. Only after the receiver's applicable authority durability barrier may it return `TransferApplied` or allow consumption of that grant.

If COMMIT arrives without the receiver's ACCEPTED record, the receiver MUST retrieve/reconcile the durable acceptance and decision evidence; version 1 does not fabricate acceptance from incomplete state. A receiver restored from an old snapshot remains unready until such reconciliation is complete.

A repeated COMMIT returns the existing applied result without crediting again. Receiver installation NEVER modifies T or creates a second copy of the original producer effect. Rights movement leaves total business free capacity unchanged.

### 8.5 ABORT_TRANSFER

Only the donor may choose final ABORT while its durable state is PREPARED. It atomically writes `ABORTED` and records the refund (`X -> U_donor`), but that refunded quantity remains unavailable to new admissions until both authority-state and transfer-decision durability policies are satisfied. Only then may it transmit final abort or spend the refund. A receiver that accepted but never received a final commit may then mark ABORTED after verifying that evidence. If abort arrives before prepare, it retains an abort tombstone so a late prepare cannot revive it.

Once COMMITTED, ABORT is illegal even if the receiver is unreachable, the client canceled or a deadline expired. Before COMMITTED, an accepted receiver is safe to abort because acceptance confers zero usable rights. Final abort and final commit are mutually exclusive under the donor's exclusive authority and local atomic status check.

### 8.6 Acknowledgment loss, retries and contradictions

Loss of `AcceptTransfer` leaves the donor PREPARED and receiver ACCEPTED. Retry identical prepare/accept or query status. Loss of `CommitTransfer` leaves an irrevocable transit quantity; resend the same final decision. Loss of `TransferApplied` leaves receiver rights usable exactly once and the donor awaiting evidence. The donor MUST NOT return q to usable or initiate a compensating grant.

The donor may retain an outstanding-transfer gauge until it observes APPLIED. That gauge is not authoritative X after the receiver installed. Reconciliation counts the transfer once using the final decision and applied marker.

Contradictory final decisions, a digest mismatch, an amount mismatch or an epoch mismatch are integrity faults. Quarantine the affected resource and retain evidence. Do not choose a winner using timestamp order. `UNKNOWN` means evidence is missing; it never means that the transfer aborted or that the amount is free to allocate again.

## 9. Rebalancing, exhaustion and availability

Rebalancing SHALL consist of ordinary transfers with stable IDs. A controller may use observed demand, starvation and queue depth to request movement. It may not directly rewrite holder balances or lower the semantic class. The controller's sum of stale gauges cannot authorize a grant.

When rights are insufficient, the plan SHALL expose one defined behavior: request a transfer and wait within the request's budget; return `InsufficientLocalRights`; or route to a compatible coordinated allocation path. A response MUST distinguish local rights exhaustion from a proven global stock-out. A global no-stock result needs the plan's corresponding observation/decision evidence.

No automatic fallback may execute the same request in parallel at another holder while its first result is unknown. No background worker may consume held or in-transit quantity as if it were usable. Fairness is a policy to measure, not a safety consequence of conservation. Version 1 provides no guarantee of obtaining remote rights during partition or of recovering rights from permanently lost exclusive authority.

## 10. Replication and causal dependencies

Escrow business effects and ledger facts SHALL use SPEC-005 semantic replication with original `OriginId`, transaction identity and immutable effect bytes. Local physical redo remains internal to SPEC-002.

The active owner's rights transitions are replicated as facts about that logical holder. A passive replica may materialize those facts for audit/recovery. It MUST NOT add the replicated holder's rights into its own spending balance. Transfer installation is the only ordinary path that moves usable rights between different holders.

The causal context of produced rights SHALL include their business production effect. A transfer includes the context needed to justify its amount. The receiver MUST apply those prerequisites before spending incoming rights. A consumption commit includes the grant/production dependencies that make its local business materialization valid. Thus a replica cannot apply a sale dependent on a restock while omitting that restock. These v1 contexts are within one SPEC-005 replication group; cross-group transfers or business prerequisites require a separately qualified composite dependency contract and cannot be encoded by replacing one group token with another.

Remote materialization of a consume or reserve operation updates business state once and records the owner's debit fact. It does not debit a different holder's usable rights. Causal coverage, dedupe and affected business/protocol metadata join the same atomic batch.

Client session observations identify their applied cut. A local or causal inventory observation is not spend authority. The final outcome of `reserve()` is backed by the durable ACTIVE reservation, and cannot later become a silent no-op because replication merged conflicting state. If other operations can revoke or alter that promise, their contract and coordination must be admitted explicitly.

## 11. Crash recovery, bootstrap and failover

After SPEC-002 local recovery, the escrow runtime SHALL enter `RECONCILING_AUTHORITY`. It MUST restore usable rights, reservations, transfer states, outcome/dedupe records, plan/resource generations, admission fences and causal evidence before admitting any spend.

| Recovered state | Safe action |
|---|---|
| Donor PREPARED | Keep q unavailable; retry/status-query or make a durable final abort while exclusive authority is proven |
| Donor COMMITTED | Keep q irrevocably debited; retransmit decision; never reconstruct it as usable |
| Donor ABORTED | Usable refund is already part of its atomic state; replay cannot refund twice |
| Receiver ACCEPTED | Zero new rights; obtain the final donor decision |
| Receiver APPLIED | Credit is already in its atomic state; resend acknowledgment without adding q |
| Receiver ABORTED | Zero credit; retain identity tombstone against late packets |
| Missing/inconsistent authority evidence | Set effective spendable rights to zero and reconcile; do not overwrite stored audit state with a guessed balance |

Normal restart on intact stable storage may resume the same authority only after proving it was never superseded and recovering its durable local fence. A stale restored store or passive replica cannot make that claim from its pages alone.

A promotion MUST prove through SPEC-011 that the old writer is durably fenced and that all of its relevant decisions, reservations, transfers and acknowledged outcomes satisfy their recorded durability policies and are recovered. Under the baseline's offline-capable authority, an unreachable old writer is not fenced. Promotion therefore blocks, even if a transfer decision is quorum durable. Synchronous copies or a decision quorum improve evidence survival; automatic failover additionally requires a specified and qualified consensus/quorum fencing protocol with exact state-transfer/admission assumptions.

Permanent loss of the sole durable copy of a holder's state creates uncertain C/H/U/X. The runtime MUST NOT estimate remaining rights from another replica's business balance. The affected resource remains blocked unless a valid protocol can recover the missing facts. This is a stated failure limitation, not permission to preserve availability by inventing capacity.

Loss after PREPARED leaves q frozen in X until a valid final decision is recovered or made by proven exclusive authority. Loss after a locally durable final decision may similarly leave evidence or installation unresolved. Diagnostics SHALL distinguish frozen rights, exhausted usable rights and global business exhaustion, and identify which policy's recovery evidence is missing. Declaring stronger receipt durability after the loss cannot reconstruct the missing authority history.

Bootstrap images follow SPEC-005 and MUST include all escrow metadata and retained phase identities at a consistent logical cut. Snapshot copying never activates authority. An image containing usable rights remains passive until a valid handover. Crash before or after activation must not yield two active holders for the same allocation.

## 12. Epoch and plan migration

Changes to resource bounds, numeric units, invariant scopes, IDC membership, operation semantics, writer placement or rights representation require SPEC-009. Version 1 SHALL use an explicit freeze/reconcile/activate barrier:

1. Compile the candidate plan and record its compatibility/refinement obligations.
2. Enumerate every active and delegated holder, allocator pool, reservation owner and in-flight transfer endpoint in the affected scope.
3. Have each holder durably fence new admissions before acknowledging its exact committed boundary, outcomes and C/H/U/transfer ledger state. An offline holder keeps the migration blocked.
4. Reconcile all issued final outcomes and business effects. Resolve PREPARED transfers to durable decisions and install every COMMITTED transfer, or keep the migration blocked. Preserve reservation promises.
5. Compute the one-to-one migration mapping for logical quantities, identities, contexts and authority. Verify both old and new invariants and the observable return contract. No residual “unaccounted” quantity may be assigned.
6. Persist a migration ledger containing old/new generations, drained boundaries, source/destination quantities, retained identity floors and authority evidence.
7. Atomically activate each new authority only under the global barrier decision, with its mapped state and new plan. Retain historical decoders and ledgers for old committed replay and outcome lookup.

Bound reduction MUST account for already consumed and reserved commitments and every outstanding allocation. It cannot revoke a successful reservation silently or rely on disconnected old holders discarding rights. Changing `origin_epoch`, deleting metadata, advancing catalog generation or waiting for a TTL is not a quantity migration.

Fences reject new invocations under the old plan. Old committed operations inside the verified boundary still replay under their original interpretation. A delayed packet from outside that boundary requires evidence/reconciliation and cannot create a new allocation. Retired generation messages return typed rejection without partial mutation.

## 13. Retention, GC and anti-entropy

Transfer/reservation identities and outcomes are correctness state. The runtime SHALL pin their journal/snapshot evidence while any endpoint, retry contract, migration, backup or reconciliation path may need it. SPEC-002's GC horizons and SPEC-005's exact dedupe/retired-epoch rules also apply.

A donor's COMMITTED record can compact only after receiver installation is durably proven and an equivalent retained identity floor/tombstone prevents reuse at both endpoints. ABORTED records require equivalent anti-resurrection protection. ACTIVE reservations and unresolved transfers cannot be removed by age. A terminal reservation may compact only when its outcome and quantity movement remain exactly classifiable for every allowed retry.

Anti-entropy exchanges per-resource authority epochs, final transfer decisions, received/applied transfer identities and immutable ledger digests. Missing facts are fetched through bounded authenticated requests. A complete inventory of ledger copies is not a new allocation or a proof that a missing holder has no remaining rights.

Returning old replicas MUST validate active catalog/migration state and either reconcile exact history or bootstrap as passive members. Retired authority tombstones outlive packets/backups that could otherwise resurrect spending. If an offline member pins required history, the system reports that cost instead of silently pruning it.

## 14. Errors, readiness and resource limits

The public/runtime error taxonomy SHALL distinguish:

```text
InsufficientLocalRights; ProvenGlobalExhausted
UnknownReservation; ReservationAlreadyConsumed; ReservationAlreadyReleased
InvalidQuantity; NumericOverflow; UnsupportedComposition
TransferPending; TransferAlreadyCommitted; TransferAlreadyAborted
IdentityConflict; IdentityExpired; OutcomeUnknown
StaleResourceGeneration; StaleAuthorityEpoch; StalePlan; FencedHolder
DependencyUnavailable; ReconciliationRequired; MigrationBlocked
QueueFull; DiskFull; Corruption; ReadOnly; NotReady
```

`ProvenGlobalExhausted` requires explicit global decision evidence. `TransferPending` is not an abort. Errors bind the request/transfer ID, stage and known outcome. A client with an unknown outcome retries the same identity and terms.

Readiness SHALL be scoped to the holder/resource:

```text
RECOVERING_LOCAL -> RECONCILING_AUTHORITY -> READY_TO_SPEND
READY_TO_SPEND -> FENCED / RIGHTS_EXHAUSTED / READ_ONLY_SAFETY / QUARANTINED
```

`RIGHTS_EXHAUSTED` may still accept rights-transfer messages and serve permitted observations. Effective usable rights for unproven authority are zero. A readable business page does not mean the resource is ready to spend.

Finite configurable limits SHALL bound active reservations, outgoing/incoming transfer records, dependency contexts, pending requests, reconciliation memory, per-peer messages and retained bytes. The runtime reserves resources for final decisions, status responses and reconciliation. It MUST refuse new admissions before capacity prevents completion/recovery of durable work. Limit pressure cannot unlock X, release H or delete required dedupe records.

## 15. Observability

Required measurements include usable rights by logical holder; active reservations; outgoing prepared/committed transfers; accepted/applied incoming transfers; rights production/consumption; transfer duration and retries; starvation; local exhaustion; frozen quantity and missing evidence; authority/decision durability waits and policy hashes; reconciliation duration; blocked migrations; retained ledger bytes; and active plan/resource/allocation/authority generations.

Metrics MUST distinguish approximate replica gauges from authoritative reconciled accounting. The tool `carolina rights inspect <resource>` SHOULD show T/C/H/U/X only when its collected cut is complete; otherwise it reports missing holders and partial observations explicitly. It MUST NOT present a sum of stale replicas as a conservation proof.

Traces contain request ID, origin ID, transfer/reservation ID, terms hash, resource/authority generation, durable decision stage and local journal boundary. User values and credentials are omitted by default. High-cardinality resource details use bounded diagnostics. Rebalancing cost and coordination outside the consume fast path must be measured in SPEC-010 evaluations.

## 16. Proof obligations and reference model

The independent model SHALL use a logical set of holders and transfer IDs, arbitrary-precision quantities, explicit durable/volatile state and a reorderable duplicating network. It SHALL distinguish physical ledger replicas from the single logical holder state, and client receipt durability from authority-state and transfer-decision barriers. SPEC-010 FM-1 requires model checking, passing deterministic simulation and a passing real-process fault campaign before any escrow distributed-correctness claim; migration also requires FM-3. Each supported policy combination must be included. Random histories alone do not discharge these gates.

Required obligations are:

1. Conservation `T = C + H + U + X` at every reachable logical protocol transition, with nonnegative terms.
2. A holder cannot spend more than its confirmed exclusive U; no two active writers consume the same holder allocation.
3. One invocation/reservation/transfer phase produces its quantity change at most once, including after retry and recovery.
4. Receiver usable credit implies an irrevocable donor debit and compatible generation/authority evidence.
5. Terminal transfer decisions are mutually exclusive; loss of any acknowledgment cannot restore committed donor rights.
6. Final business outcomes survive every supported recovery/migration and remain explicable under their exact original contract.
7. Every locally visible business/rights state satisfies its causal prerequisites and atomic-storage boundary.

These are release criteria, not proofs furnished by this prose. Safety does not imply partition liveness, absence of starvation or semantic adequacy of the application's invariant set.

## 17. Acceptance scenarios and mandatory schedules

Each named transition SHALL be interrupted before journal append, during append, before fsync, after fsync before publish and after publish before reply. Test ordinary restart, repeated recovery, stale snapshot restore, message permutations, duplication and loss. Continuously check both the accounting equation and immutable client outcomes.

| ID | Schedule | Required result |
|---|---|---|
| ESC-001 | Genesis stock 10; allocate A=4/B=3/C=3; partition; concurrent consumes including excess requests | Sum of final consumes <=10; no negative usable quantity; local shortages distinguished from global exhaustion |
| ESC-002 | A has usable=1; two local workers sell 1; race and crash at all commit boundaries | At most one success; business/debit/outcome/origin metadata all recover together |
| ESC-003 | Commit restock then duplicate its replication to every node and restart | T increases once; rights created only at designated holder; replicated stock copies do not multiply capacity |
| ESC-004 | Donor PREPARE q; crash before/after barrier; duplicate prepare; receiver ACCEPT; lose accept reply | Debit appears once; receiver has zero usable grant; retry or donor final abort remains safe |
| ESC-005 | Race donor abort with accept/commit worker; reorder final packets | Exactly one durable final decision; abort refunds once or commit remains irrevocable |
| ESC-006 | Donor COMMIT q; drop all delivery/receiver-applied replies; receiver installs and spends; donor retries | Receiver credits once; donor never refunds; conservation counts X as zero after install despite stale donor gauge |
| ESC-007 | Deliver committed transfer before its producer/restock context | Receiver cannot spend until dependencies applied; causal business invariant remains valid |
| ESC-008 | Restore donor PREPARED/receiver ACCEPTED from crash; make donor unreachable | Receiver does not credit; q stays unavailable; no timeout resolves unknown decision |
| ESC-009 | Reserve q; concurrently consume and release same reservation; lose response and retry | Exactly one terminal reservation transition; no release after consume or duplicate production |
| ESC-010 | Clone passive snapshot with usable rights; start it beside live owner; change catalog epoch while owner offline | Clone cannot spend; catalog change does not fence old writer; handover/migration blocks |
| ESC-011 | Lose primary holder's sole durable media; present lagging business/rights snapshot | Effective spendable zero; no reconstructed capacity or false transparent failover claim |
| ESC-012 | Freeze bound change with one holder offline and one transfer in progress | New activation blocks until holders fenced and transfer resolved; old successful reservations retained |
| ESC-013 | Drain all holders; migrate; replay old committed sale plus new stale-generation invocation | Historical commit dedupes/replays under valid boundary; fresh stale invocation rejected; no new grant |
| ESC-014 | GC terminal transfers then replay ancient prepare/commit/abort and expired request IDs | Retained identity evidence prevents resurrection/double credit; expired request never becomes fresh execution |
| ESC-015 | Same transfer ID with changed q/receiver/epoch or contradictory final decision | Reject/quarantine; original ledger remains unchanged |
| ESC-016 | Fill pending queues/disk; inject fsync errors during debit and final transfer decision | Backpressure and unresolved outcomes are explicit; no success or rights creation from failed persistence |
| ESC-017 | Consumer requires two resources at different holders; one side unavailable | No independent partial commit under an atomic contract; use admitted SPEC-008 composition or reject |
| ESC-018 | Retry an unknown sale at disconnected region with free rights under the same TxnId | Forward/wait/resolve original binding; no second consumption at another home |
| ESC-019 | Test quantity extremes, decimal scales, upper/lower and aggregate bounds | Exact arithmetic and correct sign mapping; overflow/unsupported coupled constraints rejected |
| ESC-020 | Client result LocalStable with transfer decision QuorumDurable; lose quorum after donor local COMMITTED/ABORTED | No exported commit, receiver credit or spendable abort refund before required evidence; client policy cannot weaken transfer barrier |
| ESC-021 | Permanently lose donor after PREPARED or only locally durable final decision | Missing evidence leaves rights frozen with honest failure scope; no timeout minting or business-balance reconstruction |
| ESC-022 | Quorum decision evidence survives donor loss; old offline holder is unfenced | Evidence remains resolvable but no replacement spend authority until valid fence/state transfer; decision durability alone is not promotion |
| ESC-023 | Substitute ResourceGeneration/IdcAuthorityEpoch for HolderAuthorityEpoch or alter EscrowEpoch during transfer | Typed binding or lineage rejection before mutation; holder replacement never creates a fresh allocation |
| ESC-024 | Crash home CAS/holder binding; retry without TxnId at another region; send G1 prerequisites to G2 | One durable request execution binding; changed request content rejects; unsupported cross-group dependency rejects before rights mutation |

## 18. Milestone gates and source traceability

| Gate | Exit evidence required |
|---|---|
| E0 — Model | SPEC-011/012/013 interfaces; canonical type fixtures; explicit accounting reference model; FM-1 exploration covering ESC-004/005/006/008/015/020–023 |
| E1 — Local resource | SPEC-002 integrated durability plus consume/produce/reservations; ESC-002/003/009/016/019 pass |
| E2 — Distributed rights | Stable authority, full transfer protocol, causal business apply and request-home integration; ESC-001/004–008/018 pass |
| E3 — Recovery and evolution | Passive bootstrap, fail-closed promotion, GC and SPEC-009 migration ledger; ESC-010–015 pass |
| E4 — Qualification | FM-1 and applicable FM-3 evidence plus deterministic/real-process fault campaigns for each admitted durability profile and atomic composition; all scenarios including ESC-017/020–024 pass before an escrow correctness claim |

No gate is complete solely because this document exists. Performance experiments follow the correctness gates and SHALL compare identical outcome semantics, failure tolerance, rights allocation and durability. Report denied operations while capacity exists elsewhere, transfer/rebalance overhead, offline-holder blockage and storage retention alongside throughput and latency.

| Source | Refinement in this specification |
|---|---|
| SPEC-001 §§18, 22–24, 39, 50–51, 66.2–66.3, 87 | Resource equations, allocation, transfer, failure boundaries and milestones (§§2–12, 16–18) |
| SPEC-001 §§36, 42–43, 47–49 | Sessions, multi-resource atomicity and generation drain (§§7, 10–12) |
| SPEC-002 §§41–44, 66–71, 97, 100–109, 173, 178–179 | Atomic business/rights/outcome batches, semantic replication, replay and identities (§§4, 7–8, 10–13) |
| [Research proposal](../PROPOSTA-DE-PESQUISA.md) §§1–2, directions A and G, inventory example | Observable confirmation and bounded disconnected autonomy (§§1–3, 9–12) |
| [Prior-art analysis](../research/consistency-prior-art.md), “Hipótese”, “Armadilhas”, E2–E4 | Preserved final outcomes; no TTL revocation; distinction between safety/liveness and relocated coordination (§§3, 9, 11–18) |

The defining invariant is: **every usable right has exactly one valid logical owner and recoverable provenance; neither a retry, a copied replica, a timeout nor a new epoch can mint another one.**
