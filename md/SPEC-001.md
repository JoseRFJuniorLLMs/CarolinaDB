# SPEC-001 — Invariant-Compiled Consistency

**Status:** Draft 0.1  
**Type:** Foundational architecture specification  
**Scope:** language semantics, invariant compiler, coordination planner, transaction runtime, replication, certification, recovery, verification and benchmarks  
**Reference implementation:** Rust stable  
**Project codename:** intentionally undefined  
**Normative terms:** MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used in the RFC sense.

---

## 0. Thesis

This system is a database in which **consistency is compiled from application invariants and operation semantics**.

The developer does not begin by choosing:

- `READ COMMITTED`;
- `SNAPSHOT ISOLATION`;
- `SERIALIZABLE`;
- eventual consistency;
- causal consistency;
- synchronous replication;
- a globally strong transaction mode.

Instead, the developer declares:

1. the valid state space;
2. the invariants that MUST never be violated;
3. the operations allowed to mutate state;
4. the effects and preconditions of those operations;
5. the ordering constraints that are semantically meaningful.

The database compiler determines the **weakest safe coordination protocol** for each operation and each invariant dependency component.

The core transformation is:

```text
Schema
  +
Invariants
  +
Operation Semantics
  +
Replication Topology
  ↓
Invariant Compiler
  ↓
Dependency Hypergraph
  ↓
Preservation / Commutativity / Authority Analysis
  ↓
Executable Consistency Plan
  ↓
Runtime Protocol
```

The research claim is not that weak consistency is new.

The research claim is:

> **A DBMS can treat coordination strategy as compiled execution machinery derived from declared correctness properties, rather than as a database-wide isolation mode selected manually by the application developer.**

---

# 1. Problem statement

Distributed databases traditionally expose consistency through system-level abstractions.

Examples:

```text
isolation level
replication mode
quorum size
leader placement
transaction boundary
serializable / eventual choice
```

These mechanisms describe **how the database coordinates**.

They do not directly describe **what the application must preserve**.

This causes two symmetric failures.

## 1.1 Over-coordination

Applications often use strong coordination where it is unnecessary.

Example:

```text
likes = likes + 1
```

If the invariant is merely:

```text
likes >= 0
```

concurrent increments do not require a total order.

A serializable transaction can preserve correctness, but it is stronger than necessary.

---

## 1.2 Under-coordination

Applications weaken consistency for latency or availability without an explicit proof that the resulting executions preserve business invariants.

Example:

```text
stock >= 0
```

Two disconnected replicas each see:

```text
stock = 1
```

and independently sell the final item.

The replicas may converge perfectly to:

```text
stock = -1
```

Convergence is not correctness.

---

# 2. Design objective

The DBMS MUST make the following question machine-checkable:

> Given invariant set `I`, operation set `O`, state `S`, topology `T` and concurrent execution relation `C`, what is the minimum coordination required to guarantee that every committed reachable state satisfies `I`?

The system MUST prefer less coordination only when it can justify that choice.

When a weaker execution strategy cannot be proven safe, the compiler MUST fall back to a stronger strategy.

The safe fallback chain is:

```text
C0 LOCAL
   ↓
C1 COMMUTATIVE
   ↓
C2 CAUSAL
   ↓
C3 ESCROW
   ↓
C4 CERTIFIED
   ↓
C5 SERIAL
```

This ordering expresses increasing coordination cost, not semantic superiority.

---

# 3. Non-goals

This project is NOT intended to be:

- another PostgreSQL clone;
- another distributed SQL database;
- another HTAP platform;
- another vector database;
- another graph database;
- another lakehouse;
- another event store;
- another CRDT library;
- another workflow engine;
- another "database for agents";
- a database whose primary novelty is LLM integration;
- a rewrite of HeraclitusDB;
- a rewrite of NietzscheDB.

SQL MAY exist as a read/query compatibility layer.

The mutation model MUST NOT be defined primarily as unrestricted SQL updates.

---

# 4. Relation to prior work

This specification deliberately builds on known research rather than pretending the problem appeared yesterday.

## 4.1 Invariant Confluence

Bailis et al., *Coordination Avoidance in Database Systems*, PVLDB 2014/2015, formalized **I-confluence**: when application transactions and invariants permit coordination-free execution while preserving correctness.

Reference:

https://www.vldb.org/pvldb/vol8/p185-bailis.pdf

This project adopts the central lesson:

> application-level invariants are the correct abstraction for deciding when coordination is necessary.

But this project MUST go beyond a proof-of-concept analysis by making the result a first-class DBMS compilation artifact that controls routing, replication, escrow, certification, failover and runtime execution.

---

## 4.2 RedBlue consistency

Li et al., OSDI 2012, classified operations into strongly coordinated red operations and weakly coordinated blue operations, including generator/shadow decomposition.

Reference:

https://www.usenix.org/conference/osdi12/technical-sessions/presentation/li

This project generalizes the binary red/blue distinction into a compiled protocol lattice and does not require the developer to manually assign operations to the final consistency class.

---

## 4.3 Fine-grained consistency / PoR

Fine-grained consistency research demonstrated that consistency restrictions can be assigned more selectively than one global mode.

Reference:

https://www.usenix.org/conference/atc18/presentation/li-cheng

This project treats those choices as compilation targets, not user-facing configuration primitives.

---

## 4.4 LoRe

LoRe verifies developer-supplied safety properties for local-first software and selectively introduces strong coordination for interactions that can violate invariants.

Reference:

https://arxiv.org/abs/2304.07133

LoRe is a major conceptual predecessor.

This system differs in intended scope:

```text
LoRe:
programming model / local-first application verification

SPEC-001:
general DBMS runtime whose transaction, replication,
partitioning and recovery protocols are generated from
the verified invariant/operation model
```

---

## 4.5 Event Horizon / Semi-Linearizability

CIDR 2026 work on Semi-Linearizability shows that asymmetric dependencies between operations can avoid unnecessary ordering even in mixed-consistency systems.

Reference:

https://www.vldb.org/cidrdb/2026/event-horizon-asymmetric-dependencies-for-fast-geo-distributed-operations.html

The compiler defined here MUST model **directed semantic dependencies**, not merely symmetric conflict.

This means:

```text
A depends on B
```

does NOT imply:

```text
B depends on A
```

---

# 5. Novelty requirement

A useful implementation is not automatically a research contribution.

For this project to justify a new DBMS rather than a PostgreSQL extension, the implementation MUST eventually demonstrate all of the following:

1. **Invariant-aware mutation language**
2. **Automatic operation-effect extraction**
3. **Static invariant preservation analysis**
4. **Automatic consistency-class derivation**
5. **Directed operation dependency analysis**
6. **Escrow synthesis for eligible bounded invariants**
7. **Runtime certification generated from the invariant model**
8. **Per-operation coordination rather than database-wide isolation**
9. **Consistency partitioning independent from physical sharding**
10. **Safe dynamic plan upgrades when runtime conditions invalidate a weaker plan**
11. **Machine-readable proof/explanation of why an operation may execute under a given consistency class**
12. **Fail-safe fallback to stronger coordination when proof is incomplete**

If the final implementation merely maps annotations to pre-existing transaction modes, the project has failed its research objective.

---

# 6. Fundamental abstraction: Invariant Dependency Component

The fundamental unit of consistency is the:

```text
Invariant Dependency Component
```

abbreviated:

```text
IDC
```

An IDC is the smallest connected semantic component containing state, operations and invariants whose correctness may interact.

Physical partitioning and consistency partitioning are explicitly separate:

```text
PHYSICAL SHARD
      !=
INVARIANT DEPENDENCY COMPONENT
```

A single shard MAY contain multiple IDCs.

A single IDC MAY span multiple shards.

Example:

```text
Shard 1
 ├─ Customer[1]
 ├─ Customer[2]
 └─ Account[9]

Shard 2
 ├─ Payment[44]
 └─ Ledger[7]
```

An invariant:

```text
Account[9].balance + Ledger[7].reserved >= 0
```

creates one IDC spanning both shards.

---

# 7. Programming model

The minimum language contains:

```text
RECORD
INDEX
INVARIANT
OPERATION
```

Optional later constructs:

```text
CAPABILITY
AUTHORITY
MATERIALIZED
EVENT
POLICY
```

---

# 8. Record declaration

Example:

```text
RECORD Account {
    id: Uuid PRIMARY KEY,
    owner_id: Uuid,
    balance: Decimal(18,2),
    status: AccountStatus
}
```

The compiler MUST know:

- field types;
- primary identity;
- partitioning key if defined;
- mutability;
- uniqueness constraints;
- numeric domains;
- references.

---

# 9. Invariant declaration

Example:

```text
INVARIANT account_non_negative {
    FOR ALL a: Account
    ASSERT a.balance >= 0
}
```

Example:

```text
INVARIANT unique_email {
    UNIQUE User.email
}
```

Example:

```text
INVARIANT department_budget {
    FOR ALL d: Department
    ASSERT SUM(Expense.amount WHERE Expense.department_id == d.id)
           <= d.budget
}
```

Example:

```text
INVARIANT shipment_requires_payment {
    FOR ALL o: Order
    ASSERT o.shipped == true
        IMPLIES o.payment_status == CONFIRMED
}
```

---

# 10. Mutation model

Safe mutation MUST occur through named operations.

Example:

```text
OPERATION withdraw(
    account_id: Uuid,
    amount: Decimal
) {
    REQUIRE amount > 0

    READ
        Account[account_id].balance

    EFFECT
        Account[account_id].balance -= amount

    ENSURE
        Account[account_id].balance >= 0
}
```

Example:

```text
OPERATION deposit(
    account_id: Uuid,
    amount: Decimal
) {
    REQUIRE amount > 0

    EFFECT
        Account[account_id].balance += amount
}
```

Example:

```text
OPERATION transfer(
    source: Uuid,
    destination: Uuid,
    amount: Decimal
) {
    REQUIRE source != destination
    REQUIRE amount > 0

    EFFECT {
        Account[source].balance -= amount
        Account[destination].balance += amount
    }

    ENSURE
        Account[source].balance >= 0
}
```

---

# 11. Why unrestricted UPDATE is not the primary mutation primitive

The following statement:

```sql
UPDATE account
SET balance = balance - 100
WHERE id = ?;
```

does not communicate enough semantics.

The DB sees:

```text
read
write
numeric change
```

It does not necessarily know:

```text
this is withdrawal
this consumes a bounded resource
this operation has a business precondition
this decrement can use escrow
this operation may commute with deposit
this operation conflicts asymmetrically with account closure
```

Therefore the safe API MUST privilege semantic operations.

Ad hoc SQL mutation MAY exist under an explicit mode:

```text
UNCOMPILED_MUTATION
```

Such mutation MUST default to:

```text
C5 SERIAL
```

unless an administrator explicitly accepts weaker semantics.

---

# 12. Invariant classes

Version 1 MUST recognize a decidable subset.

## 12.1 Local predicate

```text
x >= 0
x <= K
enum(x) in Allowed
```

---

## 12.2 Uniqueness

```text
UNIQUE R.field
```

---

## 12.3 Referential

```text
FOREIGN KEY child.parent_id EXISTS parent.id
```

---

## 12.4 Aggregate bound

```text
SUM(x) <= K
SUM(x) >= K
COUNT(x) <= K
COUNT(x) >= K
```

---

## 12.5 Conservation

```text
SUM(Account.balance) == Constant
```

or scoped:

```text
SUM(Position.quantity WHERE portfolio = P) == P.total
```

---

## 12.6 Monotonic set

```text
S(t1) subset_of S(t2)
```

---

## 12.7 State transition invariant

```text
PENDING -> APPROVED -> SETTLED
```

with forbidden reverse transitions unless explicit compensation exists.

---

## 12.8 Causal prerequisite

```text
B requires A
```

Example:

```text
SHIP requires PAYMENT_CONFIRMED
```

---

## 12.9 Arbitrary predicate

```text
ASSERT custom(...)
```

Arbitrary predicates are permitted syntactically but MUST initially compile to conservative execution unless a verifier plugin proves a weaker class.

---

# 13. Operation Effect IR

Each operation MUST compile into an architecture-neutral effect representation.

Initial primitive effects:

```text
Assign
Increment
Decrement
Insert
Delete
AddToSet
RemoveFromSet
CompareAndSwap
Reserve
Release
TransferQuantity
AdvanceState
EmitFact
```

Example:

```text
withdraw(a, n)
```

becomes:

```text
EffectIR {
    op = Decrement
    target = Account[a].balance
    amount = n
}
```

Example:

```text
transfer(a, b, n)
```

becomes:

```text
EffectIR [
    TransferQuantity {
        source = Account[a].balance,
        destination = Account[b].balance,
        amount = n
    }
]
```

Normalizing effects is mandatory because the compiler must reason algebraically about operations.

---

# 14. Invariant IR

Each invariant MUST compile into deterministic IR.

Example:

```text
InvariantIR {
    id: 0x7134,
    kind: LowerBound,
    domain: Account,
    field: balance,
    key_scope: PerPrimaryKey,
    lower_bound: 0
}
```

Example:

```text
InvariantIR {
    id: 0x9912,
    kind: Unique,
    domain: User,
    field: email,
    scope: Global
}
```

Example:

```text
InvariantIR {
    id: 0xA941,
    kind: AggregateUpperBound,
    source: Expense.amount,
    group_by: Expense.department_id,
    bound_source: Department.budget
}
```

IR MUST be:

- deterministic;
- serializable;
- versioned;
- canonical;
- hashable;
- reproducible across architectures.

---

# 15. Compiler pipeline

The compiler MUST implement the following phases.

```text
1. Parse
2. Type check
3. Normalize schema
4. Normalize invariants
5. Lower operations to Effect IR
6. Infer read/write domains
7. Build invariant-operation dependency graph
8. Derive IDC components
9. Analyze monotonicity
10. Analyze commutativity
11. Analyze invariant preservation
12. Analyze directed ordering dependencies
13. Attempt escrow synthesis
14. Select consistency class
15. Build routing plan
16. Build replication plan
17. Build certification predicate
18. Emit executable operation plan
19. Emit human-readable explanation
20. Emit machine-verifiable plan digest
```

---

# 16. Dependency hypergraph

The compiler builds:

```text
G = (D, O, I, E)
```

Where:

- `D` = data domains;
- `O` = operations;
- `I` = invariants;
- `E` = semantic dependencies.

Example:

```text
                      stock >= 0
                          │
                       Product
                      /       \
                   sell      restock
```

A larger system may look like:

```text
Customer
   │
 Order ─── inventory_non_negative ─── Product
   │
 Payment ─── budget_limit ─────────── Ledger
   │
 Shipment
```

Connected components under invariant dependency form candidate IDCs.

---

# 17. Directed dependency graph

Read/write conflict is symmetric.

Semantic dependency often is not.

Example:

```text
close_account
```

may need to observe all withdrawals before completion.

But:

```text
deposit
```

may not need to wait for an unrelated profile update.

The compiler MUST be capable of representing:

```text
A -> B
```

without implying:

```text
B -> A
```

This is necessary to exploit asymmetric ordering opportunities similar to those explored by Semi-Linearizability research.

---

# 18. Consistency classes

## C0 — LOCAL

No distributed coordination.

Requirements:

- affected invariant state is owned locally;
- no remote dependency;
- operation preserves invariants locally;
- failover semantics are covered by replication plan.

Example:

```text
update_non_unique_profile_field
```

---

## C1 — COMMUTATIVE

Concurrent operations may be reordered without violating relevant invariants.

Required proof:

```text
f(g(S)) = g(f(S))
```

or a weaker semantic equivalence accepted by the invariant model.

Examples:

```text
AddToSet
Increment(unbounded_counter)
```

Replication MAY be asynchronous.

Anti-entropy MUST preserve operation identity and idempotence.

---

## C2 — CAUSAL

The operation needs partial order, not total order.

Example:

```text
PaymentConfirmed -> ShipmentCreated
```

Runtime MUST propagate causal dependencies.

Unrelated causal chains MUST NOT be globally serialized.

---

## C3 — ESCROW

Used for decomposable bounded resources.

Example invariant:

```text
stock >= 0
```

Global state:

```text
stock = 1000
```

Rights allocation:

```text
Region A = 400
Region B = 350
Region C = 250
```

Each region can consume its local rights without cross-region coordination.

The sum of outstanding rights MUST never exceed globally available capacity.

---

## C4 — CERTIFIED

Operations may execute optimistically but commit requires validation by the relevant IDC participants.

Certification MAY check:

- read versions;
- write intersection;
- predicate conflicts;
- invariant deltas;
- IDC epoch;
- authority version;
- causal frontier.

C4 is intended for operations whose correctness cannot be guaranteed by purely local or algebraic execution but which do not require permanent total ordering.

---

## C5 — SERIAL

The affected IDC requires a total order.

Possible implementations:

- replicated sequencer;
- Raft group;
- Multi-Paxos group;
- deterministic transaction ordering.

C5 is mandatory when:

- invariant is unsupported;
- proof fails;
- semantic conflict requires total order;
- global uniqueness cannot be decomposed;
- administrator explicitly requests strongest semantics.

---

# 19. Compiler safety rule

The compiler MUST be conservative.

Formally:

```text
UNPROVEN_SAFE(operation, class)
    =>
REJECT(class)
```

The compiler then tries the next stronger class.

The system MUST NOT use statistical evidence such as:

```text
"we have never observed a violation"
```

as proof of safety.

Runtime telemetry MAY optimize placement and allocation.

It MUST NOT silently weaken the semantic class.

---

# 20. Preservation analysis

For operation `O`, invariant `I`, state `S`:

```text
I(S) ∧ Pre(O,S)
    => I(O(S))
```

For concurrent operations `A` and `B`:

```text
I(S)
∧ Pre(A,S)
∧ Pre(B,S)
=>
I(A(B(S)))
∧
I(B(A(S)))
```

For mergeable replicas:

```text
I(S1)
∧ I(S2)
∧ common_ancestor(S0,S1,S2)
=>
I(merge(S1,S2))
```

The compiler SHOULD support:

- interval arithmetic;
- abstract interpretation;
- affine arithmetic;
- monotonicity inference;
- set algebra;
- finite-state transition analysis;
- symbolic bounds;
- key-domain reasoning.

SMT MAY be used during compilation.

SMT MUST NOT be required in the transaction hot path.

---

# 21. Commutativity analysis

The compiler MUST distinguish:

```text
syntactic conflict
```

from:

```text
semantic conflict
```

Two writes to the same logical value may commute.

Example:

```text
counter += 1
counter += 1
```

Conversely, operations on distinct rows may conflict through an aggregate invariant.

Example:

```text
Expense[A] += 100
Expense[B] += 100
```

if both participate in:

```text
SUM(Expense) <= Budget
```

Therefore row-level conflict detection alone is insufficient.

---

# 22. Escrow synthesis

Escrow is a first-class compiler target.

Supported initial invariant shapes:

```text
x >= L
x <= U
SUM(x_i) <= U
SUM(x_i) >= L
```

The compiler determines whether an operation consumes or produces rights.

Example:

```text
withdraw(account, amount)
```

with:

```text
balance >= 0
```

becomes:

```text
consume_rights(account.balance, amount)
```

A deposit produces rights:

```text
produce_rights(account.balance, amount)
```

---

# 23. Escrow metadata

Each bounded resource maintains:

```text
EscrowState {
    invariant_id
    resource_id
    epoch
    global_bound
    local_rights
    delegated_rights
    consumed_rights
}
```

Conservation rule:

```text
available_global
=
sum(local_rights)
+
reserved_in_transfer
+
already_materialized_capacity
```

The exact representation MUST be proven not to create rights during retry, replay or failover.

---

# 24. Rights transfer protocol

Rights transfer MUST be idempotent.

Protocol:

```text
A -> PREPARE_TRANSFER(id, amount, B)

A:
    moves amount from usable to reserved

B -> ACCEPT_TRANSFER(id)

A -> COMMIT_TRANSFER(id)

B:
    materializes rights exactly once
```

Crash recovery MUST resolve:

```text
PREPARED
ACCEPTED
COMMITTED
ABORTED
```

Duplicate messages MUST NOT duplicate authority.

---

# 25. Certification

C4 uses a generated certification predicate.

Example:

```text
CERTIFY withdraw(a,n) {
    assert epoch == expected_epoch
    assert version(Account[a]) == read_version
    assert post_balance >= 0
}
```

More generally:

```text
CertificationProgram {
    operation_id
    invariant_ids
    expected_epoch
    conflict_domains
    version_predicates
    invariant_delta_predicates
}
```

The certification program MUST be deterministic and canonical.

---

# 26. Transaction envelope

Every mutation execution MUST produce:

```text
TxnEnvelope {
    txn_id
    operation_id
    operation_version
    schema_hash
    plan_hash
    arguments_hash
    idc_ids
    consistency_class
    causal_dependencies
    authority_tokens
    read_versions
    normalized_effect_digest
    epoch
}
```

Replication works on compiled transactional envelopes and normalized effects, not arbitrary application code.

---

# 27. Executable Operation Plan

Compilation output:

```text
OperationPlan {
    operation_id
    operation_version

    invariants: [...]
    idcs: [...]

    class: C0 | C1 | C2 | C3 | C4 | C5

    routing
    authority_requirements
    causal_requirements
    escrow_program
    certification_program
    replication_program
    recovery_program

    proof_summary
}
```

---

# 28. Plan explanation

Every plan MUST be explainable.

Command:

```text
EXPLAIN CONSISTENCY withdraw;
```

Example result:

```text
Operation: withdraw
Class: C3 ESCROW

Reason:
  withdraw decreases Account.balance

Affected invariant:
  account_non_negative:
      Account.balance >= 0

Static result:
  concurrent withdrawals can violate the invariant

Escrow result:
  the invariant is decomposable as consumable rights

Coordination:
  no remote coordination while local rights >= requested amount

Fallback:
  request additional rights
  if unavailable, coordinate with current rights authority

Global serialization:
  not required
```

This explanation is part of the product, not debugging decoration.

---

# 29. Proof artifact

The compiler MUST emit a machine-readable artifact:

```text
ConsistencyProof {
    compiler_version
    schema_hash
    operation_hash
    invariant_hashes
    analysis_rules
    selected_class
    rejected_weaker_classes
    assumptions
    plan_hash
}
```

This is not necessarily a formal proof assistant theorem in v1.

It is a deterministic certificate showing why the compiler chose the protocol.

Future versions MAY emit proof objects checkable by an independent verifier.

---

# 30. Storage architecture

The reference implementation SHOULD separate:

```text
1. State Store
2. Operation Log
3. Consistency Metadata Store
```

Unlike HeraclitusDB, the operation log is not required to be the ontological source of all truth.

Its role here is:

- durability;
- replication;
- recovery;
- protocol reconstruction.

---

# 31. State Store

Initial engine requirements:

- MVCC;
- versioned keys;
- point lookup;
- ordered range scan;
- snapshots;
- atomic batches;
- deterministic recovery;
- checksum per page/block;
- background compaction or page reclamation.

Candidate physical engines:

```text
B+Tree
Bw-Tree-like
LSM
copy-on-write B-tree
```

SPEC-002 will choose the reference storage engine after workload analysis.

SPEC-001 intentionally does not allow storage fashion to dictate consistency semantics.

---

# 32. Operation Log

Record:

```text
CommittedOperation {
    commit_id
    txn_envelope
    normalized_effects
    certification_record
}
```

The log MUST support:

- append;
- checksum;
- crash-safe framing;
- replay;
- truncation of incomplete tail;
- segment rotation;
- replication cursor.

---

# 33. Consistency Metadata Store

Stores protocol-critical data:

```text
IDC epochs
escrow rights
causal frontiers
owner leases
certifier versions
sequencer positions
plan versions
schema versions
```

This metadata MUST be replicated according to its own correctness requirements.

It MUST NOT be treated as disposable cache.

---

# 34. Memory model

Hot memory contains:

```text
active MVCC versions
IDC routing table
operation plans
escrow rights
causal frontier
certification index
lease state
recent operation ids
```

Plans MUST be immutable after publication.

Plan replacement MUST use atomic generation swap.

---

# 35. Query execution

Read-only queries are separate from mutation semantics.

Initial query model MAY support:

```text
GET
SCAN
FILTER
PROJECT
AGGREGATE
JOIN
```

SQL compatibility MAY be added later.

Reads MUST expose consistency explicitly where relevant.

Example:

```text
READ Account[123]
AT LOCAL
```

```text
READ Account[123]
AT CAUSAL session
```

```text
READ Account[123]
AT CERTIFIED
```

The compiler MAY infer the weakest read strength required by an operation.

---

# 36. Session guarantees

Runtime MUST support at least:

```text
read-your-writes
monotonic reads
causal dependencies
```

These MUST NOT accidentally imply global linearizability.

---

# 37. Replication by consistency class

## C0

Replication policy is a durability choice.

Mutation itself requires no distributed coordination.

---

## C1

Operation-based replication preferred.

Requirements:

```text
idempotent op id
deduplication
deterministic application
anti-entropy
```

---

## C2

Replica messages carry causal metadata.

Candidate representation:

```text
version vector
dotted version vector
hybrid dependency summary
```

The exact scheme is deferred.

---

## C3

Data plus authority/rights metadata are replicated.

Failover MUST NOT create extra rights.

A replica without confirmed rights MUST refuse bounded-resource consumption.

---

## C4

Relevant IDC participants certify.

Certification set MAY differ from physical replica set.

---

## C5

A strongly ordered replicated log is used for the IDC.

Global cluster-wide consensus MUST NOT be the default if only one IDC requires serialization.

---

# 38. Consistency groups are not shards

Traditional design often creates:

```text
shard -> consensus group
```

This system SHOULD permit:

```text
IDC -> consistency protocol
```

independent of:

```text
storage shard
```

This is a central architectural distinction.

Example:

```text
Shard A:
  IDC-1 -> C1
  IDC-2 -> C3
  IDC-3 -> C5
```

---

# 39. Dynamic plan specialization

Static compilation determines the safe protocol family.

Runtime MAY specialize within that safe family.

Example:

```text
C3 ESCROW
```

may change rights allocation based on demand.

Example:

```text
C4 CERTIFIED
```

may co-locate certifiers based on observed access patterns.

Runtime MUST NOT change:

```text
C5 -> C1
```

based solely on telemetry.

Weakening class requires recompilation and proof.

---

# 40. Dynamic strengthening

Runtime MAY temporarily strengthen consistency without recompilation.

Examples:

```text
C1 -> C4
C3 -> C5
C4 -> C5
```

Reasons:

- topology uncertainty;
- stale authority;
- failed lease renewal;
- partition ambiguity;
- incompatible plan versions;
- compiler/runtime version mismatch.

Safety wins over availability.

---

# 41. Schema evolution

Schema and invariant evolution are protocol changes.

Every published schema has:

```text
schema_version
schema_hash
invariant_set_hash
```

Changing an invariant MAY change:

- IDC composition;
- consistency class;
- rights allocation;
- routing;
- certification program;
- replication group.

Therefore invariant migration MUST be transactional at the metadata layer.

---

# 42. Recompilation protocol

Changing schema/invariant set:

```text
1. Compile candidate generation G+1
2. Validate all existing data against new invariants
3. Compute IDC changes
4. Compute required protocol migrations
5. Freeze affected semantic boundary if necessary
6. Drain incompatible operations
7. Transfer / reconcile rights
8. Establish new certifiers or sequencers
9. Publish G+1
10. Retire G after all old envelopes complete
```

---

# 43. Operation versioning

Operations are immutable by version.

Example:

```text
withdraw@1
withdraw@2
```

A transaction envelope references exact version.

Old operation versions MAY remain executable while compatible with current invariants.

Otherwise they MUST be rejected.

---

# 44. Topology

Reference cluster roles:

```text
Data Node
Compiler / Catalog Node
IDC Coordinator
Certifier
Sequencer
Gateway
```

A physical process MAY implement multiple roles.

No role is assumed globally centralized.

---

# 45. Catalog

The catalog stores:

```text
schemas
operation definitions
invariants
compiled plans
IDC map
plan generations
node capabilities
```

Catalog publication MUST use strong consistency.

This metadata volume is expected to be far smaller than user data.

A small strongly replicated control plane is acceptable.

---

# 46. Routing

Gateway receives:

```text
operation + arguments
```

It loads the immutable operation plan and computes:

```text
affected keys
affected IDCs
owners
required protocol
```

Then it dispatches directly to the minimum participant set.

No universal distributed transaction coordinator SHOULD sit on every mutation path.

---

# 47. Multi-IDC transactions

An operation MAY touch multiple IDCs.

The compiler builds an IDC interaction graph.

If components are independent and effects are separable:

```text
parallel execution
```

MAY be allowed.

If atomicity spans components:

```text
composite certification
```

or:

```text
stronger temporary coordination
```

is required.

Version 1 SHOULD conservatively promote cross-IDC atomic mutations to C4 or C5.

---

# 48. Failure model

Initial system assumes:

```text
crash-stop / crash-recovery
network partitions
message duplication
message reordering
delayed messages
process restart
disk torn write at tail
```

Byzantine faults are explicitly out of scope for SPEC-001.

---

# 49. Crash recovery

Recovery order:

```text
1. Recover local operation log
2. Discard incomplete tail
3. Restore latest valid state checkpoint
4. Replay committed operations
5. Restore consistency metadata
6. Reconcile IDC epoch
7. Reconcile rights
8. Reconcile causal frontier
9. Rejoin replication groups
10. Resume operations only after authority is proven
```

---

# 50. Failover safety for escrow

Escrow is the most dangerous subsystem in crash recovery.

A promoted replica MUST prove its rights state belongs to the active epoch.

If it cannot prove:

```text
rights(epoch = current)
```

then:

```text
available_rights = 0
```

until reconciliation.

This may reduce availability.

It MUST NOT invent capacity.

---

# 51. Network partitions

Behavior is determined by compiled class.

## C1

Continue locally if operation remains semantically safe.

## C2

Continue if causal prerequisites are locally satisfied.

## C3

Continue while sufficient local rights exist.

## C4

Continue only if required certification quorum remains reachable.

## C5

Continue only on the side that retains ordering authority.

This creates **semantic partition tolerance** rather than one cluster-wide behavior.

---

# 52. Security boundary

The invariant compiler is part of the trusted computing base.

Compiler output MUST be deterministic.

Production builds SHOULD support:

```text
reproducible compiler artifact
signed plan generation
plan hash verification
```

Nodes MUST reject an envelope whose:

```text
plan_hash
schema_hash
operation_version
```

do not match the active generation.

---

# 53. No LLM in correctness path

An LLM MAY:

- propose an invariant;
- explain a compiler result;
- generate schema scaffolding;
- suggest operation definitions.

An LLM MUST NOT:

- decide at runtime whether an invariant is preserved;
- approve a weaker consistency mode;
- generate authority tokens;
- replace certification logic;
- participate in the trusted correctness path.

Correctness MUST remain deterministic.

---

# 54. Compiler crate architecture

Reference Rust workspace:

```text
crates/
  core/
  syntax/
  ir/
  invariant/
  effect/
  analysis/
  compiler/
  plan/
  runtime/
  storage/
  log/
  causal/
  escrow/
  certify/
  serial/
  replication/
  catalog/
  server/
  client/
  cli/
```

The project SHOULD resist premature crate proliferation.

The first compiling milestone MAY combine modules.

---

# 55. Core types

Indicative Rust types:

```rust
pub struct InvariantId(pub u128);
pub struct OperationId(pub u128);
pub struct IdcId(pub u128);
pub struct PlanHash(pub [u8; 32]);
pub struct Epoch(pub u64);

pub enum ConsistencyClass {
    Local,
    Commutative,
    Causal,
    Escrow,
    Certified,
    Serial,
}
```

---

# 56. Invariant representation

```rust
pub enum InvariantKind {
    LowerBound,
    UpperBound,
    Unique,
    Referential,
    AggregateUpperBound,
    AggregateLowerBound,
    Conservation,
    Monotonic,
    StateTransition,
    CausalPrerequisite,
    Arbitrary,
}
```

---

# 57. Effect representation

```rust
pub enum Effect {
    Assign { target: Path, value: Expr },
    Increment { target: Path, amount: Expr },
    Decrement { target: Path, amount: Expr },
    Insert { target: Domain, value: Expr },
    Delete { target: Path },
    AddToSet { target: Path, value: Expr },
    RemoveFromSet { target: Path, value: Expr },
    CompareAndSwap { target: Path, expected: Expr, value: Expr },
    Reserve { resource: Path, amount: Expr },
    Release { resource: Path, amount: Expr },
    TransferQuantity { from: Path, to: Path, amount: Expr },
    AdvanceState { target: Path, from: Expr, to: Expr },
}
```

---

# 58. Analysis result

```rust
pub struct AnalysisResult {
    pub invariant: InvariantId,
    pub operation: OperationId,
    pub preserves_locally: ProofStatus,
    pub commutative_with: Vec<OperationId>,
    pub causal_dependencies: Vec<OperationId>,
    pub escrow_candidate: Option<EscrowPlan>,
    pub minimum_class: ConsistencyClass,
    pub assumptions: Vec<Assumption>,
}
```

---

# 59. Proof status

```rust
pub enum ProofStatus {
    Proven,
    Disproven(CounterExample),
    Unknown,
}
```

Critical rule:

```text
Unknown
```

MUST NOT be interpreted as:

```text
probably safe
```

---

# 60. Counterexample generation

When possible, the compiler SHOULD emit a counterexample.

Example:

```text
Invariant:
    stock >= 0

Operations:
    sell(1)
    sell(1)

Counterexample:
    initial stock = 1

Replica A:
    stock 1 -> 0

Replica B:
    stock 1 -> 0

Naive merge of decrements:
    stock = -1

Result:
    C1 COMMUTATIVE rejected
```

Counterexamples are valuable both for developers and research evaluation.

---

# 61. CLI

Initial CLI:

```text
db compile schema.icc
db check
db explain operation withdraw
db graph invariants
db plan
db run
db status
db rights
db certify inspect <txn>
db replay
db verify
```

---

# 62. EXPLAIN CONSISTENCY

Example:

```text
$ db explain operation sell

Operation:
  sell(product_id, qty)

Touched invariant:
  inventory_non_negative

Attempted classes:

C0 LOCAL
  rejected:
  inventory authority may exist on multiple regions

C1 COMMUTATIVE
  rejected:
  concurrent decrements can exceed available inventory

C2 CAUSAL
  rejected:
  causal ordering alone does not prevent over-consumption

C3 ESCROW
  accepted:
  invariant is a decomposable lower bound
  sell consumes quantity rights

Result:
  C3 ESCROW
```

---

# 63. Observability

Metrics MUST be semantic, not only physical.

Examples:

```text
operations_total{operation,class}
coordination_avoided_total
coordination_required_total
rights_available{idc,node}
rights_transfer_total
certification_abort_total
serial_queue_depth{idc}
causal_wait_total
plan_generation
consistency_upgrade_total
```

---

# 64. Benchmark philosophy

The project MUST NOT claim success merely because raw key-value throughput is high.

The central experimental question is:

> How much coordination can the compiler safely eliminate for invariant-rich workloads compared with a strong baseline, while preserving exactly the same application invariants?

---

# 65. Required baselines

At minimum:

```text
PostgreSQL SERIALIZABLE
PostgreSQL weaker isolation where semantically comparable
CockroachDB / strongly serializable distributed SQL
a simple eventual/CRDT baseline for eligible workloads
hand-written optimized protocol where feasible
```

Research comparisons SHOULD include conceptual or implementation comparison with:

```text
I-confluence prototype lineage
RedBlue-style classification
LoRe-style selective coordination
Semi-Linearizability / Event Horizon
```

Exact systems depend on reproducible availability at evaluation time.

---

# 66. Required workloads

## 66.1 TPC-C derived

Focus:

```text
NewOrder
Payment
StockLevel
```

Invariants MUST be explicitly encoded.

---

## 66.2 Inventory

Operations:

```text
restock
sell
reserve
release
transfer_stock
```

Invariant:

```text
available >= 0
reserved >= 0
available + reserved = total where applicable
```

---

## 66.3 Banking / ledger

Operations:

```text
deposit
withdraw
transfer
reserve
release
```

Invariants:

```text
balance >= overdraft_limit
conservation where applicable
```

---

## 66.4 Unique namespace

Operations:

```text
register_username
rename_username
delete_username
```

Tests global uniqueness behavior.

---

## 66.5 Causal workflow

Operations:

```text
create_order
confirm_payment
ship
cancel
refund
```

Tests asymmetric dependencies and partial ordering.

---

## 66.6 Edge / disconnected workload

Operations continue under partition according to compiled authority.

This absorbs the strongest useful part of the local-first direction without turning the system into a sync SDK.

---

# 67. Metrics

Measure:

```text
throughput
p50 latency
p95 latency
p99 latency
cross-region messages per operation
consensus rounds per operation
bytes coordinated per operation
availability during partition
abort rate
rights starvation
rights rebalance cost
recovery time
compiler time
plan size
number of operations per consistency class
coordination avoided relative to serial baseline
```

---

# 68. Correctness testing

Performance tests are invalid unless accompanied by invariant checking.

Every benchmark MUST continuously verify:

```text
all declared invariants
```

The test harness MUST fail immediately on violation.

---

# 69. Jepsen-style testing

A distributed correctness harness SHOULD inject:

```text
process crash
network partition
packet delay
message duplication
leader failure
clock skew
disk restart
rolling restart
catalog generation change
rights transfer crash
```

After every execution:

```text
invariants == satisfied
```

must hold for all committed states according to declared semantics.

---

# 70. Model checking

Small protocol models SHOULD be written in:

```text
TLA+
```

or an equivalent formal specification framework.

Minimum protocols to model:

```text
escrow transfer
C4 certification
IDC epoch transition
schema-plan generation transition
C5 failover
```

---

# 71. Property-based testing

Rust implementation SHOULD use property-based testing for:

```text
effect normalization
commutativity
merge idempotence
rights conservation
operation replay
plan determinism
canonical hashing
```

---

# 72. Fuzzing

Fuzz:

```text
parser
IR decoder
operation log decoder
plan decoder
network messages
recovery records
```

Invalid input MUST fail closed.

---

# 73. Compiler correctness target

Two distinct properties:

## Soundness

If the compiler selects class `C`, executions permitted by `C` MUST preserve declared invariants under the supported failure model.

## Precision

The compiler SHOULD avoid choosing stronger coordination when a weaker supported class can be proven safe.

Soundness is mandatory.

Precision is an optimization/research objective.

---

# 74. Research hypotheses

The project should be evaluated against explicit hypotheses.

### H1

Application invariants and normalized operation effects permit automatic derivation of weaker safe coordination for a meaningful fraction of transactional operations.

### H2

Per-IDC consistency reduces cross-region coordination relative to database-wide serializable execution.

### H3

Escrow synthesis materially improves availability during partitions for bounded-resource invariants.

### H4

Directed operation dependencies avoid coordination that symmetric conflict models would impose.

### H5

A fail-safe compiler can provide these reductions without increasing invariant violations relative to a strongly serializable baseline.

---

# 75. Falsification criteria

The research direction SHOULD be abandoned or reduced to a library if experiments show any of the following:

1. Most real operations compile to C5.
2. Developers must provide so many annotations that manual protocol design is simpler.
3. Static analysis cannot derive materially better plans than straightforward conflict analysis.
4. Runtime metadata cost erases coordination savings.
5. Escrow/causal/certification protocols dominate implementation complexity without broad workload benefit.
6. PostgreSQL plus a thin compiler extension achieves essentially identical semantics and performance.
7. Correctness requires unrestricted application code in the trusted analysis path.
8. Plan migration is too disruptive for production use.

This section is intentional. A research system must be able to fail its thesis.

---

# 76. Why this may require a new DBMS

A PostgreSQL extension can implement:

```text
custom types
triggers
functions
new indexes
background workers
foreign data wrappers
```

But the architecture in this SPEC requires the compiler to control:

```text
transaction routing
replication mode
coordination participants
causal metadata
escrow authority
certification
consensus scope
failover behavior
schema migration semantics
recovery behavior
```

on a per-operation and per-IDC basis.

If PostgreSQL remains the authority over:

```text
WAL
MVCC
transaction manager
replication semantics
lock manager
commit path
```

then the compiler cannot fully implement the architecture without fighting the host DBMS.

The decisive test is:

> If implementing the model requires replacing the transaction manager, replication semantics, routing and commit protocol, it is no longer merely a PostgreSQL extension.

---

# 77. Difference from NietzscheDB

NietzscheDB centers on:

```text
knowledge
graph structure
multi-manifold geometry
cognitive memory
reasoning
semantic activation
```

Its primary question is approximately:

```text
How should machine knowledge be represented,
organized, traversed and evolved?
```

SPEC-001 does none of this.

It has no required:

```text
embedding
HNSW
Poincaré geometry
GNN
ACT-R
semantic memory
LLM
```

The abstraction is distributed correctness, not cognition.

---

# 78. Difference from HeraclitusDB

HeraclitusDB centers on an immutable canonical event history from which derived views can be reconstructed, with strong emphasis on:

```text
auditability
provenance
replay
temporal inspection
integrity
multi-model retrieval
government/security workloads
```

SPEC-001 asks a different question:

```text
Before an operation commits,
what coordination is mathematically necessary
to preserve the declared application invariants?
```

HeraclitusDB may later consume or record decisions generated by this system.

That does not make them the same architecture.

The primary object here is not:

```text
immutable historical event
```

It is:

```text
compiled invariant-preserving operation
```

---

# 79. Architectural identity

The architecture can be summarized in one sentence:

> **A semantic transaction compiler that turns application invariants into the minimum safe distributed coordination protocol.**

If implementation drifts away from that sentence, it should be reconsidered.

---

# 80. MVP boundary

The first prototype MUST be deliberately small.

Support only:

```text
key-value / typed records
integer and fixed-decimal numeric fields
lower/upper bounds
uniqueness
simple referential constraints
monotonic sets
causal prerequisites

operations:
  assign
  increment
  decrement
  insert
  add-to-set
  compare-and-swap

classes:
  C0
  C1
  C2
  C3
  C5
```

C4 certification MAY enter after the first correctness prototype.

---

# 81. MVP topology

Three nodes.

```text
Node A
Node B
Node C
```

No automatic elastic scaling.

No Kubernetes dependency.

No cloud-specific dependency.

No GPU.

No CXL.

No vector search.

No dashboard before correctness.

The point is to prove the semantic compiler, not decorate it.

---

# 82. Milestone M0 — Formal core

Deliver:

```text
language grammar
typed AST
Invariant IR
Effect IR
formal state model
formal operation model
IDC definition
consistency class definitions
```

Exit criterion:

A set of hand-written examples can be lowered deterministically into IR.

---

# 83. Milestone M1 — Static analyzer

Implement:

```text
dependency graph
locality analysis
commutativity analysis
simple preservation proofs
counterexample generation
```

Exit criterion:

Compiler correctly distinguishes safe/unsafe coordination-free examples.

---

# 84. Milestone M2 — Single-node runtime

Implement:

```text
state store
operation log
operation executor
plan executor
recovery
```

Even though distributed protocols are not active yet, mutation MUST already go through compiled plans.

---

# 85. Milestone M3 — C1 replication

Implement:

```text
operation IDs
idempotent replication
anti-entropy
commutative application
```

Exit criterion:

Concurrent replicated operations converge and preserve supported invariants.

---

# 86. Milestone M4 — C2 causal execution

Implement:

```text
causal dependency metadata
causal wait
session frontier
partial-order replication
```

---

# 87. Milestone M5 — C3 escrow

Implement:

```text
rights allocation
rights consumption
rights production
rights transfer
epoch safety
crash recovery
```

This milestone is the first major research-quality checkpoint.

---

# 88. Milestone M6 — C5 serial IDC

Implement:

```text
per-IDC replicated sequencer
strong ordering
failover
```

No global sequencer unless one invariant genuinely creates one global IDC.

---

# 89. Milestone M7 — Multi-class operation planner

One application MUST simultaneously run operations compiled to:

```text
C1
C2
C3
C5
```

against related data.

This is a key demonstration.

---

# 90. Milestone M8 — C4 certification

Implement generated certification for cases lying between escrow/causal execution and total serialization.

Research question:

```text
Can invariant-specific certification shrink the serializable conflict domain
enough to justify C4 as a distinct class?
```

If not, C4 SHOULD be removed rather than preserved for architectural vanity.

---

# 91. Milestone M9 — Dynamic plan generation

Implement:

```text
schema generation
plan generation
safe publication
old-plan draining
IDC epoch migration
```

---

# 92. Milestone M10 — Evaluation

Required output:

```text
correctness report
fault-injection report
TPC-C-derived evaluation
inventory benchmark
banking benchmark
causal workflow benchmark
partition availability study
coordination-cost study
comparison to strong baseline
comparison to manually optimized baseline
```

No invented benchmark numbers.

---

# 93. Minimum paper contribution

A publishable research paper SHOULD NOT be:

> "We built a database that supports six consistency modes."

That is not enough.

A strong paper needs at least one new result such as:

```text
1. a new compiler algorithm deriving protocol choice from invariant/effect IR;
2. a novel directed dependency analysis that safely reduces coordination;
3. automatic escrow synthesis for a broader invariant class;
4. a new invariant-specific certification algorithm;
5. a new decomposition algorithm for IDC construction;
6. a correctness proof plus evidence that the generated plans approach hand-tuned protocols.
```

The DBMS is the experimental vehicle.

The algorithmic result is the paper.

---

# 94. Definition of success

The project succeeds if a developer can write:

```text
INVARIANT stock_non_negative {
    Product.stock >= 0
}

OPERATION sell(id, qty) {
    Product[id].stock -= qty
}
```

and the system can correctly answer:

```text
This operation cannot run as naive eventual consistency.

It does not require a global serializable transaction.

The invariant is decomposable.

Compile as ESCROW.

Allocate quantity rights per replica.

Permit disconnected execution while local rights remain.

Refuse or coordinate when rights are exhausted.

Preserve stock >= 0 under crash, retry and partition.
```

without the developer implementing that distributed protocol manually.

That is the first real target.

---

# 95. Definition of failure

The project fails if the developer still has to write:

```text
use_serializable = false
use_crdt = true
use_escrow = true
leader = us-east
quorum = 2
causal = false
```

The purpose of the compiler is precisely to derive those implementation choices from semantics wherever possible.

---

# 96. Final invariant of the project

The system itself has one architectural invariant:

```text
NO WEAKER EXECUTION WITHOUT A PROOF OBLIGATION BEING SATISFIED
```

Performance is optimized below that line.

Never above it.

---

# References

1. Peter Bailis, Alan Fekete, Michael J. Franklin, Ali Ghodsi, Joseph M. Hellerstein, Ion Stoica. **Coordination Avoidance in Database Systems.** PVLDB 8(3), 2014/2015.  
   https://www.vldb.org/pvldb/vol8/p185-bailis.pdf

2. Cheng Li, Daniel Porto, Allen Clement, Johannes Gehrke, Nuno Preguiça, Rodrigo Rodrigues. **Making Geo-Replicated Systems Fast as Possible, Consistent when Necessary.** OSDI 2012.  
   https://www.usenix.org/conference/osdi12/technical-sessions/presentation/li

3. Cheng Li, Nuno Preguiça, Rodrigo Rodrigues. **Fine-grained consistency for geo-replicated systems.** USENIX ATC 2018.  
   https://www.usenix.org/conference/atc18/presentation/li-cheng

4. Julian Haas, Ragnar Mogk, Elena Yanakieva, Annette Bieniusa, Mira Mezini. **LoRe: A Programming Model for Verifiably Safe Local-First Software.** 2023.  
   https://arxiv.org/abs/2304.07133

5. Jonathan Arns, Harald Ng, Kyriakos Psarakis, Asterios Katsifodimos, Paris Carbone. **Event Horizon: Asymmetric Dependencies for Fast Geo-Distributed Operations.** CIDR 2026.  
   https://www.vldb.org/cidrdb/2026/event-horizon-asymmetric-dependencies-for-fast-geo-distributed-operations.html

6. Michael Stonebraker, Xinjing Zhou, Peter Kraft, Qian Li. **Consistency and Correctness in Data-Oriented Workflow Systems.** CIDR 2026.  
   https://www.vldb.org/cidrdb/2026/consistency-and-correctness-in-data-oriented-workflow-systems.html

---

# End of SPEC-001
