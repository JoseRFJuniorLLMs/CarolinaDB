# CarolinaDB

> **Invariant-compiled consistency for distributed stateful systems.**

CarolinaDB is a distributed database runtime that compiles **versioned operation contracts, invariants, observation requirements, authority rules, durability policies, and failure assumptions** into qualified execution plans.

Instead of forcing every operation through one global consistency model, CarolinaDB selects from a finite library of protocol families and binds each accepted operation to an immutable, versioned execution plan.

CarolinaDB preserves not only valid final state, but also:

- exact operation results;
- stable request identity;
- durable commitments;
- causal dependencies;
- authority ownership;
- bounded-resource rights;
- distributed transaction decisions;
- retry semantics;
- snapshot semantics;
- recovery evidence;
- plan-evolution guarantees.

The database is designed for systems where correctness is larger than “the rows still satisfy a constraint”.

Typical workloads include inventory and reservations, quotas, credits, permits, distributed allocations, workflow engines, control planes, auditable government systems, financial or administrative state machines, and applications that must preserve previously issued commitments across crashes, retries, partitions, recovery, authority changes, and plan migration.

---

## Status

**Research prototype.** The repository holds the specifications (`md/SPEC-001` … `SPEC-014`) and a Rust
workspace that implements the first local vertical slice of [SPEC-014](md/SPEC-014.md):

| Stage | State |
|---|---|
| MVP-0 semantic core (DSL, typed IR, reference interpreter, canonical artifacts) | implemented, tested |
| MVP-1 conservative compiler (closure, obligations, counterexamples, certificate, checker, EXPLAIN) | implemented, tested |
| MVP-2 local durable slice (B+Tree/MVCC/journal/checkpoint, CompiledBatch, RequestHome, receipts, resolve) | implemented, tested in-process (crash matrix, differential oracle, SPEC-014 §4 schedules) |
| MVP-3 catalog + single-IDC C5 (three-voter Raft, catalog CAS/genesis/grants, `ASTR`/TCP node, ordered execution, real three-process fault campaign) | implemented, tested for the plaintext `DEV_LOCAL` loopback profile (gates Q3-C5, QI-CATALOG, QI-CODEC-CORPUS PASS); the SPEC-014 §3 exit criteria are **not** met because SPEC-013 mTLS/authorization is not implemented (QI-SECURITY NOT_RUN) |
| MVP-4 … MVP-8 (multi-IDC publication, C1/C2, C3, evolution, C4) | not started; their formal models FM-1/2/3 pass within explicit bounds, which is model evidence only |

The only distributed capability is the single-IDC C5 slice above; multi-IDC atomicity, C1/C2, C3 and
evolution do not exist. `present in code != implemented capability != qualified capability`.
The per-deliverable state, test evidence and recorded deviations live in [docs/STATUS.md](docs/STATUS.md);
the implementation and regression audit is [docs/AUDIT.md](docs/AUDIT.md),
and an independent narrative audit is [docs/relatorio-completo.md](docs/relatorio-completo.md).
Public CI evidence for the current working tree does not exist yet. The later run for base commit
`cdfca6a` also stopped at `cargo fmt --all -- --check`; this tree has since been reformatted, tested
locally and still needs a push before those results become public CI evidence.

```bash
cargo test --workspace
cargo run -p carolina-cli -- explain fixtures/dsl/inventory_reserve_release.cdl
cargo run -p carolina-cli -- graph invariants fixtures/dsl/account_transfer.cdl
cargo run -p carolina-cli -- workload ./data-demo
cargo run -p carolina-cli -- qualify --quick --out qualification
```

The last command runs the SPEC-010 campaign and writes a manifest, verdict and report under
`qualification/`. It claims gates Q0, Q1, Q2, FM (bounded models) and Q3-C5 (the single-IDC C5
slice, including a three-process fault campaign); everything else is reported `NOT_RUN`.

---

## Table of Contents

- [Why CarolinaDB](#why-carolinadb)
- [Core Model](#core-model)
- [Observable Correctness](#observable-correctness)
- [Consistency Families](#consistency-families)
- [Architecture](#architecture)
- [Invariant Dependency Components](#invariant-dependency-components)
- [Operation Contracts and IR](#operation-contracts-and-ir)
- [Coordination Compiler](#coordination-compiler)
- [Request Identity](#request-identity)
- [Exact Final Receipts](#exact-final-receipts)
- [Storage Kernel](#storage-kernel)
- [C1/C2 Semantic Replication](#c1c2-semantic-replication)
- [C3 Escrow](#c3-escrow)
- [C4 Certified Transactions](#c4-certified-transactions)
- [C5 Serial IDC Runtime](#c5-serial-idc-runtime)
- [Distributed Atomic Publication](#distributed-atomic-publication)
- [Plan Evolution](#plan-evolution)
- [Catalog and Control Plane](#catalog-and-control-plane)
- [Security Model](#security-model)
- [Wire Protocol and Compatibility](#wire-protocol-and-compatibility)
- [Snapshots and Restore](#snapshots-and-restore)
- [Qualification](#qualification)
- [Production v1 Profile](#production-v1-profile)
- [Failure Semantics](#failure-semantics)
- [Non-Goals](#non-goals)
- [Specification Map](#specification-map)
- [Implementation Stages](#implementation-stages)
- [Example](#example)
- [Research Foundations](#research-foundations)
- [Design Principles](#design-principles)

---

# Why CarolinaDB

Traditional databases expose generic isolation or consistency levels such as:

```text
eventual
causal
snapshot
serializable
linearizable
```

These models are useful, but they do not completely describe an application's correctness contract.

Consider:

```text
reserve(item, quantity)
```

The real contract may require:

```text
inventory never becomes negative
+
a successful reservation survives recovery
+
the returned reservation ID remains stable
+
a retry never reserves twice
+
a confirmed reservation survives plan migration
+
an old authority cannot continue spending rights after replacement
```

A database can preserve the final numeric invariant and still violate one or more of those guarantees.

CarolinaDB therefore treats **observable behavior as part of correctness**.

The runtime compiles declared semantics into compatible execution plans, authorizes one of those plans, records the exact plan identity, executes it through the corresponding protocol runtime, and preserves the resulting commitments across retries, failures, recovery, and evolution.

---

# Core Model

The CarolinaDB execution pipeline is:

```text
Versioned schema
    +
Versioned operation contracts
    +
Invariants
    +
Read / result / observation semantics
    +
Authority rules
    +
Durability requirements
    +
Failure assumptions
    +
Topology
    |
    v
Typed semantic IR
    |
    v
Dependency and obligation analysis
    |
    v
Invariant Dependency Components
    |
    v
Finite protocol-template library
    |
    v
Safety and compatibility checks
    |
    v
Cost-aware plan selection
    |
    v
Immutable execution plan
    |
    v
Authorized runtime execution
    |
    v
Durable exact result + evidence
```

The compiler selects only from **supported, versioned, qualified protocol templates**.

It does not assume a globally weakest protocol, a total ordering of consistency classes, or a complete decision procedure for arbitrary distributed programs.

---

# Observable Correctness

CarolinaDB models correctness as more than a predicate over final rows.

An operation contract can include:

- preconditions;
- postconditions;
- invariants;
- exact result semantics;
- rejection semantics;
- read visibility;
- session guarantees;
- whole-invocation atomicity;
- request deduplication;
- durability;
- causal dependencies;
- authority ownership;
- bounded-resource rights;
- migration behavior;
- supported failure behavior.

For a system history `H`, CarolinaDB reasons about its observable projection:

```text
Obs(H)
```

Internal retries, physical page layout, and replica-local sequence numbers are not part of the user-visible contract unless explicitly exposed.

For a plan transition:

```text
Obs(H_G -> G+1) ∈ Allowed(T(C_G, C_G+1), F)
```

where:

- `G` and `G+1` are plan generations;
- `C_G` and `C_G+1` are versioned contracts;
- `T(...)` is the transition contract;
- `F` is the declared failure model.

A migration is therefore not correct merely because the final rows validate.

Previously issued results and commitments remain part of the contract.

---

# Consistency Families

CarolinaDB v1 defines six execution families.

| Family | Name | Primary purpose |
|---|---|---|
| **C0** | Local | Operations confined to one authoritative local boundary |
| **C1** | Commutative | Semantic replication for safely commutative normalized effects |
| **C2** | Causal | C1 plus explicit causal dependencies and causal observations |
| **C3** | Escrow | Bounded-resource execution using exclusive logical rights |
| **C4** | Certified | Optimistic execution followed by authoritative semantic validation |
| **C5** | Serial | Deterministically ordered execution inside an IDC authority |

These families are not a semantic ranking.

For example:

- C3 can preserve a reservation contract without routing every request through one serial authority.
- C2 can be correct for immutable dependent facts but insufficient for a globally exact balance result.
- C4 can provide safe optimistic concurrency only when its complete dependency set can be certified.
- C5 can serialize valid operations, but serialization does not repair an invalid sequential contract.

A plan may combine obligations from multiple families.

---

# Architecture

```text
┌──────────────────────────────────────────────────────────────┐
│                     Client / SDK Layer                       │
│ RequestKey • retries • ResolveRequest • observation tokens   │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               v
┌──────────────────────────────────────────────────────────────┐
│                Authentication / Authorization                │
│       mTLS • principals • tenants • signed evidence          │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               v
┌──────────────────────────────────────────────────────────────┐
│                 Catalog / Control Plane                      │
│ Plans • artifacts • routing • capabilities • authorities    │
│ migrations • typed generations • admission fencing          │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               v
┌──────────────────────────────────────────────────────────────┐
│                  Coordination Compiler                       │
│ IR -> IDC closure -> obligations -> candidate plans          │
│ -> compatibility checks -> selected qualified PlanRef        │
└──────────────────────────────┬───────────────────────────────┘
                               │
              ┌────────────────┼──────────────────┐
              │                │                  │
              v                v                  v
        ┌──────────┐      ┌──────────┐      ┌──────────┐
        │ C1 / C2  │      │    C3    │      │ C4 / C5  │
        │ Semantic │      │  Escrow  │      │ Certified │
        │Replication│     │  Rights  │      │ / Serial  │
        └─────┬────┘      └─────┬────┘      └─────┬────┘
              └─────────────────┼──────────────────┘
                                │
                                v
┌──────────────────────────────────────────────────────────────┐
│               Distributed Decision / Publication            │
│ prepare • decision • install • publication • completion     │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               v
┌──────────────────────────────────────────────────────────────┐
│                    Storage Kernel                            │
│ B+Tree • MVCC • buffer pool • Commit Journal • checkpoints  │
│ CompiledBatch • prepared state • exact crash recovery        │
└──────────────────────────────────────────────────────────────┘
```

---

# Invariant Dependency Components

An **Invariant Dependency Component**, or IDC, is CarolinaDB's semantic coordination boundary.

An IDC is a conservative closure over:

```text
operations
+
invariants
+
records / keys / ranges
+
observations
+
causal dependencies
+
authority requirements
+
resource ownership
```

The core question is:

> Which operations, data, observations, and authorities must be considered together for this contract to remain correct?

An IDC is not the same thing as:

- a physical shard;
- a Raft group;
- a table;
- a storage partition;
- a tenant;
- a transaction.

Those concepts may coincide in a deployment, but they are semantically distinct.

The primary IDC identity is represented through:

```text
IdcBinding {
    idc_id
    idc_generation
    authority_epoch
}
```

A semantic definition change advances `IdcGeneration`.

An authority replacement advances `IdcAuthorityEpoch`.

A physical placement change advances `PlacementEpoch`.

CarolinaDB keeps these identities separate because a placement change is not automatically an authority change, and an authority change is not automatically a semantic-definition change.

---

# Operation Contracts and IR

Applications register versioned operations rather than submitting unrestricted mutation logic directly to the distributed runtime.

The semantic frontend models constructs such as:

```text
RECORD
INDEX
INVARIANT
OPERATION
```

and lowers them into a typed canonical intermediate representation.

An operation may declare:

```text
reads
writes
guards
preconditions
postconditions
results
rejections
durability
observation requirements
session dependencies
resource effects
authority requirements
```

The IR is deterministic and content-addressed.

Important semantic identities include:

```text
SchemaHash
OperationHash
ContractHash
PlanHash
```

Published compiler artifacts are immutable.

This allows plans, receipts, protocol records, snapshots, and migrations to refer to exact contract versions rather than mutable names.

---

# Coordination Compiler

The CarolinaDB compiler evaluates a closed operation set against a finite protocol-template library.

Conceptually:

```text
CompileInput {
    module_ir
    topology
    active_generation
    protocol_library
    analysis_rules
    policy
    analysis_budget
    prior_plans
}
```

The compiler performs:

1. type and semantic validation;
2. read/write/guard dependency extraction;
3. invariant closure;
4. IDC construction;
5. candidate protocol generation;
6. protocol-specific proof-obligation construction;
7. cross-plan compatibility analysis;
8. failure-model compatibility checks;
9. authority and durability checks;
10. cost evaluation among safe candidates;
11. immutable plan generation;
12. diagnostic and `EXPLAIN` generation.

The compiler never uses:

```text
max(C0, C1, C2, C3, C4, C5)
```

as a safety rule.

The family labels do not form a total order.

A plan is selected only after all applicable obligations are satisfied under the exact contract, topology, authority model, failure assumptions, and durability profile.

---

# Request Identity

Client identity is independent from transport attempts, routing, and selected plan.

The canonical request identity is:

```text
RequestKey {
    tenant_id
    request_namespace
    stable_request_id
}
```

The request lifecycle is:

```text
authenticate
    ->
authorize namespace / operation
    ->
route(RequestKey)
    ->
RequestHome durable BindIfAbsent
    ->
TxnId allocation
    ->
plan binding
    ->
execution
```

Routing does not require a `TxnId`.

The RequestHome atomically binds:

```text
RequestKey
+
RequestHash
+
TxnId
```

before effects are dispatched.

If the same `RequestKey` arrives with different semantic content, CarolinaDB returns:

```text
RequestIdentityMismatch
```

before any new effects.

A transport timeout does not mean abort.

Public request outcomes include:

```text
Committed
Rejected
Unavailable
OutcomeUnknown
RequestIdentityMismatch
ResultExpired
IdentityExpired
ProtocolError
```

`OutcomeUnknown` is a client knowledge state.

It is not a transaction decision and never authorizes automatic re-execution under a fresh identity.

Clients resolve the original request through `ResolveRequest(RequestKey)`.

---

# Exact Final Receipts

Every completed invocation is represented by a canonical:

```text
FinalReceiptV1
```

The receipt binds the historical operation result to:

```text
ClusterId
RequestKey
RequestHash
TxnId
OperationRef
OperationHash
SchemaHash
ContractHash
PlanRef
IdcBinding[]
OriginId[]
terminal outcome
exact result bytes
result digest
commitments
observation token
durability evidence
decision evidence
completion evidence
```

Retries return the same canonical receipt payload.

Recovery returns the same canonical receipt payload.

Plan migration preserves the original receipt payload.

Credential rotation may re-sign or re-wrap the payload, but it does not alter the historical semantic receipt.

This prevents a previously successful request from being silently reinterpreted under a new operation version or new consistency plan.

---

# Storage Kernel

CarolinaDB v1 uses a native page-oriented storage engine based on:

```text
B+Tree
+
MVCC
+
explicit buffer pool
+
redo-first Commit Journal
+
atomic CompiledBatch
+
checkpoint and recovery
```

The central durability rule is:

> A committed operation depends on the required journal decision reaching stable storage, not on every modified data page already being flushed.

Dirty pages may be persisted later.

Local storage identities include:

```text
StorageEpoch
VersionStamp
LocalCommitSeq
JournalLsn
```

These values are local to one storage history.

They are never promoted into cross-node semantic timestamps.

`CompiledBatch` is the atomic local boundary for dependent business and protocol state.

It includes the information required to bind:

```text
transaction identity
request identity
operation identity
contract identity
plan identity
IDC bindings
semantic evidence
captured inputs
storage mutations
protocol mutations
terminal outcome
semantic digest
```

Prepared distributed mutations remain invisible until their authoritative transaction decision is installed.

---

# C1/C2 Semantic Replication

C1 replicates **normalized semantic effects**, not physical storage writes.

For example:

```text
counter += 1
```

is replicated as an increment operation.

It is not replicated as:

```text
Put(counter, 17)
```

derived from one replica's local materialized state.

C1 provides:

- immutable semantic commits;
- stable origin identity;
- idempotent logical application;
- deterministic effect application;
- anti-entropy;
- duplicate suppression;
- convergence for qualified commutative contracts.

C2 extends C1 with:

- explicit dependency tracking;
- dotted causal frontiers;
- causal visibility;
- read-your-writes;
- monotonic reads;
- dependency-aware catch-up.

In v1, C2 session guarantees are scoped to **one replication group**.

Cross-group session semantics require a separately qualified composite plan.

CarolinaDB never combines unrelated sequence numbers from different groups and calls the result a causal frontier.

---

# C3 Escrow

C3 handles bounded resources through **exclusive logical rights**.

Typical examples include:

```text
inventory
quota
credit
permits
capacity
allocation slots
```

The core accounting model is:

```text
T = C + H + U + X
```

where:

- `T` = total authorized capacity;
- `C` = committed consumption;
- `H` = held or reserved capacity;
- `U` = currently usable rights;
- `X` = rights in transit.

Only locally owned `U` is spendable.

Rights in `X` remain globally accounted for but cannot be spent by both donor and receiver.

CarolinaDB distinguishes:

```text
ResourceGeneration
EscrowEpoch
HolderAuthorityEpoch
```

because:

- changing the resource definition;
- changing the conserved allocation lineage;
- replacing a holder's authority;

are different operations.

Business values are not spend authority.

If authority evidence is irrecoverably lost, CarolinaDB may freeze uncertain capacity rather than fabricate replacement rights.

---

# C4 Certified Transactions

C4 allows optimistic execution followed by authoritative semantic certification.

An operation first executes against a snapshot and records the dependency evidence required by its contract.

Certification can include:

```text
point reads
absent-key reads
range reads
predicate reads
guards
invariant dependencies
resource interactions
```

A speculative result is not a commit.

The certifier validates all required dependencies and establishes durable reservations where necessary before the transaction enters the distributed decision/publication path.

C4 does not establish a permanent total order for the entire IDC.

Non-conflicting work may execute concurrently.

C4 is intended for workloads where its additional concurrency provides measurable benefit over the simpler C5 baseline.

---

# C5 Serial IDC Runtime

C5 executes an IDC as a replicated, deterministically ordered state machine.

The v1 profile uses a Raft-backed authority for serial IDC execution.

Operations are assigned an explicit semantic position:

```text
SerialPosition
```

This is distinct from:

```text
JournalLsn
VersionStamp
LocalCommitSeq
```

C5 supports contracts requiring:

- exact current results;
- strict invariant serialization;
- authoritative reads;
- deterministic ordering;
- strongly ordered state-machine execution.

The order is scoped to the IDC.

Independent IDCs do not imply one cluster-wide total order.

---

# Distributed Atomic Publication

Transactions spanning multiple physical participants or multiple IDCs use an explicit distributed protocol.

The protocol separates:

```text
prepare
decision
install
publication
completion
```

Representative durable evidence includes:

```text
PrepareVote
DecisionCertificate
PublicationCertificate
PublishSeen
CompletionCertificate
```

Prepared changes are not public.

A participant installing a commit is not, by itself, sufficient evidence for a final client success.

A final client result is returned only when the plan's required decision, durability, publication, and completion conditions are satisfied.

This prevents fractured visibility where one side of a multi-IDC operation appears committed while another required participant remains invisible.

---

# Plan Evolution

CarolinaDB treats plan evolution as an explicit protocol.

Changing a plan is not equivalent to editing a configuration value.

The baseline lifecycle is:

```text
PROPOSED
    ->
COMPILED
    ->
CLOSING
    ->
DRAINING
    ->
VALIDATING
    ->
INSTALLING
    ->
READY_TO_ACTIVATE
    ->
ACTIVE
    ->
RETIRED
```

A migration can cover:

```text
protocol replacement
operation changes
schema changes
IDC split
IDC merge
authority transfer
placement changes
capacity changes
```

The transition preserves:

```text
final results
request bindings
prepared decisions
causal dependencies
resource rights
reservations
transfers
authority evidence
read/session semantics
```

An unreachable authority is not treated as empty.

A timeout does not prove revocation.

Unsafe replacement remains blocked until the required close/drain/reconciliation evidence is available.

Already-issued commitments remain associated with their original request, contract, plan, and authority history.

---

# Catalog and Control Plane

The CarolinaDB v1 control plane is a replicated three-voter catalog.

It owns:

```text
artifact registry
plan registry
IDC definitions
routing
RequestHome ownership
node capabilities
authority grants
admission fences
security policy references
migration state
activation state
typed generations and epochs
```

The catalog provides linearizable metadata transactions and authoritative read barriers.

It does not replace runtime authorities.

A catalog update alone cannot prove that an old disconnected writer has stopped issuing valid decisions.

Important identity domains include:

```text
ClusterId
TenantId
CatalogGeneration
PlanGeneration
IdcGeneration
IdcAuthorityEpoch
PlacementEpoch
MembershipGeneration
ResourceGeneration
EscrowEpoch
HolderAuthorityEpoch
StorageEpoch
OriginEpoch
RequestHomeEpoch
RecordRevision
```

These are nominally distinct.

Equal numeric payloads do not make two epoch types interchangeable.

---

# Security Model

> **Design, not implementation.** None of the mechanisms in this section exists in the code yet.
> The node speaks only the plaintext, loopback-only `DEV_LOCAL` profile, its endpoint roles are
> unauthenticated declarations, and the qualification gate `QI-SECURITY` is `NOT_RUN`. See
> [SECURITY.md](SECURITY.md) and [docs/AUDIT.md](docs/AUDIT.md) (SPEC-013 section).

CarolinaDB v1 assumes authenticated, authorized, non-Byzantine admitted infrastructure members.

The baseline security model includes:

- TLS 1.3 transport protection;
- mutual TLS for trusted service roles;
- client authentication;
- tenant authorization boundaries;
- purpose-scoped credentials;
- signed portable protocol evidence;
- credential activation and revocation;
- key rotation;
- replay protection;
- downgrade protection;
- bounded protocol parsing;
- encrypted storage and backup profiles;
- audit events;
- cluster-bound trust.

Portable signed payloads use a fixed COSE profile with Ed25519 in the v1 security design.

Security identity is bound to:

```text
ClusterId
TenantId
principal
purpose
credential
security policy
```

A cryptographically valid payload from the wrong cluster, tenant, role, or purpose is not valid authority.

CarolinaDB v1 is not a Byzantine-fault-tolerant database.

mTLS, hashes, signatures, and checksums authenticate or protect evidence. They do not transform Raft or escrow into BFT consensus.

---

# Wire Protocol and Compatibility

CarolinaDB defines deterministic canonical runtime records and an explicit versioned wire protocol.

Canonical payload rules include:

```text
UTF-8
sorted unique field names
explicit required fields
canonical integers
lowercase hex bytes
no floating-point ambiguity
explicit null
deterministic collection ordering
duplicate rejection
bounded nesting
bounded collection size
bounded payload size
```

The wire envelope includes:

```text
magic
wire major
wire minor
message kind
flags
payload length
stream ID
per-direction frame sequence
```

v1 message families include:

```text
Hello
HelloAck
Invoke
Reply
ResolveRequest
ResolveReply
Read
ReadReply
ProtocolRecord
SnapshotManifest
SnapshotChunk
```

Unknown mandatory semantic versions fail closed.

A generic serializer may not silently ignore unknown fields.

Historical canonical hash domains retain the `astra.*` namespace for byte stability.

Examples:

```text
astra.request.v1
astra.result.v1
astra.receipt.v1
astra.protocol-record.v1
astra.snapshot-chunk.v1
astra.snapshot-manifest.v1
astra.negotiation.v1
```

`Astra` therefore survives only as a historical protocol/hash namespace.

The product and database name is **CarolinaDB**.

---

# Snapshots and Restore

A CarolinaDB snapshot contains more than user rows.

A recoverable snapshot may include:

```text
user state
request bindings
request tombstones
exact results
prepared transactions
decision evidence
causal frontiers and holes
resource allocations
transfers
authority fences
plan artifacts
schema artifacts
namespace retirement state
protocol metadata
```

Snapshot manifests bind:

```text
cluster identity
tenant scope
source identity
storage epoch
catalog generation
plan references
IDC bindings
membership generation
semantic cut
required codecs
required artifacts
chunks
retention state
unresolved protocol references
authority fences
```

Restore occurs into a closed staging state.

Copied bytes do not grant semantic authority.

A replacement instance receives fresh identities where required and remains unavailable for authoritative admission until the catalog authorizes the reconciled state.

An older backup cannot legitimately resurrect:

```text
retired RequestNamespaces
expired request identities
old rights holders
closed authorities
forgotten prepared decisions
```

---

# Qualification

CarolinaDB treats correctness qualification as part of the system lifecycle.

A feature is not considered production-enabled simply because source code exists.

```text
present in code
!=
implemented capability
!=
qualified capability
!=
production-enabled capability
```

Qualification combines, where applicable:

```text
formal model checking
+
deterministic simulation
+
real-process fault campaigns
+
independent observable-history checking
```

Three formal gates are central to v1. Each one is executed as a bounded explicit-state checker with
mandatory negative controls (`crates/carolina-models`); the TLA+ sources next to them have not been
run through TLC. The bullets below describe the intended scope of each gate, not the current model
bounds, which are recorded in [models/README.md](models/README.md) and in the campaign verdict.

## FM-1 — Escrow / Authority Transfer

Checks:

- conservation of rights;
- transfer recovery;
- holder replacement;
- authority fencing;
- duplicate/reordered transfer messages;
- crash boundaries;
- uncertain evidence handling.

## FM-2 — Decision / Install / Publication

Checks:

- at most one final distributed decision;
- valid prepare-to-commit transitions;
- prepared-state invisibility;
- atomic publication;
- completion semantics;
- authority fencing.

## FM-3 — Migration / Fencing

Checks:

- closure of old authority;
- exclusion of incompatible generations;
- preserved requests/results;
- delayed old messages;
- offline issuers;
- plan activation;
- recovery of migration state.

Qualification verdicts are:

```text
PASS
FAIL
INCONCLUSIVE
NOT_RUN
NOT_APPLICABLE
```

`INCONCLUSIVE` is not `PASS`.

`NOT_RUN` is not `PASS`.

---

# Production v1 Profile

CarolinaDB v1 is intentionally conservative.

Its baseline architecture includes:

```text
Rust stable
native page-oriented B+Tree storage
MVCC
redo-first Commit Journal
explicit buffer pool
atomic CompiledBatch
stable RequestKey identity
RequestHome allocation
exact retained FinalReceiptV1
fixed three-voter control plane
Raft-backed C5 authorities
C1 semantic replication
single-group C2 session guarantees
C3 bounded-resource escrow
C4 certified execution
C5 deterministic IDC execution
distributed prepare / decision / publication
typed authority generations
plan evolution
mTLS
signed protocol evidence
canonical wire protocol
content-addressed codec manifests
semantic snapshots and restore
qualification manifests
```

The production model deliberately avoids implicit behavior.

CarolinaDB does not:

```text
infer abort from timeout
infer authority from cached routing
recreate rights from business values
rewrite old receipts during migration
treat local LSNs as global time
silently downgrade consistency
silently decode unknown semantic fields
expose prepared data as committed state
```

---

# Failure Semantics

CarolinaDB makes failure outcomes explicit.

## Transport Failure

A timeout can produce:

```text
OutcomeUnknown
```

The client resolves the original `RequestKey`.

It does not create a new logical request automatically.

## Catalog Unavailable

An operation may continue only when its installed authority profile explicitly permits the relevant offline behavior.

Cached metadata is not authority.

## Authority Unavailable

An operation may become unavailable even when local user data remains readable.

Availability does not override authority safety.

## Escrow Evidence Lost

Uncertain rights remain frozen.

They are not reconstructed from current row values.

## Distributed Prepare Unresolved

The transaction remains in doubt until the authoritative decision is recovered.

Participants do not infer abort from elapsed time.

## Migration Blocked

Activation remains blocked until required close, drain, validation, install, and authority evidence are available.

The system does not activate an incompatible replacement merely to restore availability.

---

# Non-Goals

CarolinaDB v1 does not attempt to provide:

- unrestricted arbitrary application code inside the semantic compiler;
- automatic invariant discovery from unknown programs;
- unrestricted SQL mutation as the primary programming model;
- Byzantine consensus;
- one global total order across all independent IDCs;
- implicit multi-group C2 sessions;
- timeout-based semantic authority revocation;
- automatic recreation of lost escrow rights;
- magical exactly-once semantics across uncontrolled external side effects;
- automatic business compensation for external APIs;
- a proof that every operation admits a low-coordination plan;
- a consistency-class lattice;
- a complete protocol-synthesis algorithm for arbitrary distributed programs.

---

# Specification Map

CarolinaDB v1 is defined by fourteen core specifications.

| SPEC | Responsibility |
|---|---|
| [`SPEC-001`](md/SPEC-001.md) | Architectural thesis, observable contracts, IDC model and consistency families |
| [`SPEC-002`](md/SPEC-002.md) | B+Tree, MVCC, Commit Journal, durable batch boundary and crash recovery |
| [`SPEC-003`](md/SPEC-003.md) | Restricted DSL, typed semantic IR, effects and invariant dependencies |
| [`SPEC-004`](md/SPEC-004.md) | Coordination compiler, candidate plans, compatibility and plan selection |
| [`SPEC-005`](md/SPEC-005.md) | C1/C2 semantic replication and causal observations |
| [`SPEC-006`](md/SPEC-006.md) | C3 escrow, bounded resources, rights transfer and holder authority |
| [`SPEC-007`](md/SPEC-007.md) | C4 certified transactions and semantic validation |
| [`SPEC-008`](md/SPEC-008.md) | C5 serial IDC execution, distributed decisions and atomic publication |
| [`SPEC-009`](md/SPEC-009.md) | Plan evolution, fencing, drain, transformation and activation |
| [`SPEC-010`](md/SPEC-010.md) | Qualification, formal gates, deterministic simulation and fault campaigns |
| [`SPEC-011`](md/SPEC-011.md) | Catalog, typed identity taxonomy, control plane and authority registry |
| [`SPEC-012`](md/SPEC-012.md) | Request identity, client protocol, wire encoding, receipts and snapshots |
| [`SPEC-013`](md/SPEC-013.md) | Security, authentication, authorization and trust model |
| [`SPEC-014`](md/SPEC-014.md) | Implementation profile, vertical slice and capability sequencing |

---

# Implementation Stages

The v1 implementation model is staged as:

```text
MVP-0  Semantic core
MVP-1  Conservative compiler
MVP-2  Local durable slice
MVP-3  Catalog + single-IDC C5
MVP-4  Distributed atomic publication
MVP-5  C1/C2
MVP-6  C3 escrow
MVP-7  Plan evolution
MVP-8  C4
```

The stages exist to preserve dependency and qualification discipline.

A later stage may depend only on capabilities whose required earlier gates are satisfied.

---

# Example

Suppose an application declares:

```text
inventory >= 0
```

with:

```text
reserve(item, qty)
```

and requires:

```text
a successful reservation survives crash
reservation identity is stable
retry never reserves twice
regions may continue during partition when they hold valid rights
```

Plain asynchronous replication is insufficient.

Two candidate plans might be:

```text
C3 escrow
C5 serial
```

The compiler evaluates only candidates that satisfy the complete contract.

A C3 plan can allocate exclusive regional rights:

```text
Region A: U = 40
Region B: U = 30
Region C: U = 30
```

A successful reservation atomically persists:

```text
business state
+
rights transition
+
request identity
+
exact result
+
receipt evidence
```

If the application later moves the operation to C5, CarolinaDB:

```text
closes old spending authorities
    ->
drains admitted requests
    ->
settles transfers
    ->
captures a complete semantic cut
    ->
validates the transition
    ->
installs the ordered state
    ->
activates the new plan
```

Existing successful reservations retain their original receipt and commitment.

That preservation across execution-model changes is central to CarolinaDB.

---

# Research Foundations

CarolinaDB builds on established research and systems work in:

- invariant confluence;
- RedBlue consistency;
- causal consistency;
- escrow and bounded counters;
- declarative consistency;
- coordination avoidance;
- semantic dependency analysis;
- verified distributed programming;
- transaction certification;
- replicated state machines;
- protocol and schema evolution.

The maintained prior-art analysis is available at:

[`research/consistency-prior-art.md`](research/consistency-prior-art.md)

The research proposal is available at:

[`PROPOSTA-DE-PESQUISA.md`](PROPOSTA-DE-PESQUISA.md)

The scientific focus is not the invention of each individual mechanism.

The broader CarolinaDB design target is the integration of:

```text
versioned observable contracts
+
heterogeneous execution plans
+
exact request/result identity
+
authority transfer
+
durable protocol evidence
+
recovery
+
plan evolution
```

under one explicit runtime model.

---

# Design Principles

CarolinaDB follows ten non-negotiable rules:

1. **Observable results are part of correctness.**
2. **Timeout is not a transaction decision.**
3. **Identity is allocated before execution and survives retry.**
4. **Authority is explicit, typed, and versioned.**
5. **Business state is not authority state.**
6. **Local storage order is not distributed semantic order.**
7. **Prepared state is not public state.**
8. **Migration preserves commitments, not only rows.**
9. **A protocol family name is not a proof.**
10. **A capability is production-enabled only after its required qualification gates pass.**

---

# CarolinaDB in One Sentence

**CarolinaDB is a distributed database runtime that compiles explicit application correctness contracts into qualified execution plans and preserves their results, authority, and commitments across concurrency, failure, recovery, and evolution.**
