# SPEC-001 — Invariant-Compiled Consistency

**Status:** Draft 0.2 — proposed architecture; implementation, qualification and research proof remain open  
**Date:** 2026-09-09  
**Type:** Foundational architecture specification  
**Scope:** observable contracts, conservative compilation, protocol composition, authority, evolution and qualification  
**Reference implementation:** Rust stable; conceptual records do not freeze a Rust or wire ABI  
**Project:** CarolinaDB; earlier research documents and canonical hash domains retain the historical `astra` name  
**Normative terms:** MUST, MUST NOT, SHOULD and MAY express requirements, not implemented capabilities.

## 0. Thesis

CarolinaDB is a proposed database runtime that compiles **versioned observable operation contracts** into execution plans. Contracts declare invariants, permitted effects, reads, results, atomicity, authority, durability and behavior during unavailable communication.

> The compiler selects safe execution plans from a finite, versioned, qualified protocol-template library and optimizes a declared cost function among proven-compatible candidates under explicit assumptions. No globally weakest protocol, total ordering of protocol families, or complete decision procedure is claimed.

The initial deterministic selection heuristic in [SPEC-004](SPEC-004.md) does not guarantee a global optimum even within every possible combination of that finite library. Any later minimum-cost claim MUST identify the completely searched candidate space and cost model. A cost estimate is not a latency measurement.

```text
Versioned schema + invariants + operation/observation contracts
  + topology/authority assumptions + qualified protocol library
  -> deterministic lowering and conservative semantic closure
  -> candidate obligations and joint compatibility analysis
  -> executable plans + evidence + diagnostics
  -> authorized execution, durable outcomes and admissible observations
  -> recovery and explicitly qualified plan/contract transitions
```

The candidate scientific contribution is **observable refinement when heterogeneous safe plans compose, transfer authority, evolve and recover from failure**. Generic consistency selection, escrow and operation dependency analysis are predecessors and engineering building blocks. Novelty and the general refinement result remain to be demonstrated against the [proposal](../PROPOSTA-DE-PESQUISA.md) and [prior-art analysis](../research/consistency-prior-art.md).

This document owns the architectural thesis. SPEC-003 owns language/IR semantics; SPEC-004 compiler selection; SPEC-002 durable storage; SPEC-005–008 protocols; SPEC-009 evolution; SPEC-010 qualification; SPEC-011 catalog/authority and identifier taxonomy; SPEC-012 request/client/encoding contracts; SPEC-013 security; SPEC-014 implementation sequencing. Examples here are explanatory summaries, not alternate definitions of those owners. An unresolved contradiction is a specification defect and blocks the affected capability.

# 1. Problem statement

An invariant over rows does not fully describe an application's correctness. Two implementations may both keep stock nonnegative while returning different confirmations, exposing different read states or losing different acknowledged requests after a crash.

Over-coordination and under-coordination must therefore be judged against the **same full observable contract**. A receipt for a grow-only fact can admit executions that an exact current-global-value result cannot. Two replicas seeing stock 1 cannot both confirm sale 1 unless their confirmations are backed by a valid exclusive resource decomposition or shared coordination. Convergence to stock -1 is still invalid.

Isolation may already be chosen per transaction in existing systems; a new per-operation flag is not the research contribution. The question is whether a restricted contract can generate useful, composable, verifiable plans with fewer application-written protocol mechanisms.

# 2. Design objective

For a closed operation set, invariant set, observable contract, topology and failure model, the compiler MUST determine which **supported candidate plans** satisfy all applicable obligations. It then selects compatible plans using SPEC-004's declared cost policy.

C0 LOCAL, C1 COMMUTATIVE, C2 CAUSAL, C3 ESCROW, C4 CERTIFIED and C5 SERIAL name protocol families. They are not a cost ladder, a semantic total order or a mathematical lattice. Family labels never replace an execution profile or its proof obligations.

An unproven candidate is rejected. The compiler may select an independently safe alternative, including qualified C5, or return `NoSafePlan`. Unsupported semantics and invalid sequential operations cannot be repaired merely by serialization. At runtime an invalidated assumption yields the plan's permitted non-success outcome or a fully qualified transition; it cannot silently alter the contract.

# 3. Non-goals

The initial experiment excludes unrestricted application code, unrestricted SQL mutation, external side effects, automatic business-rule inference, Byzantine consensus, automatic elastic scaling and hardware-specific acceleration. It does not replace EVA's NietzscheDB backend or the established HeraclitusDB services.

SQL read compatibility MAY be added through explicit observation contracts. A new storage engine or commercial database category is not an assumed scientific requirement; the native/runtime-over-existing-storage comparison remains mandatory research work.

# 4. Relation to prior work

The [prior-art analysis](../research/consistency-prior-art.md) is the maintained literature account, including primary sources and their limitations. The architecture MUST NOT claim novelty solely from capabilities already covered by that account.

## 4.1 Invariant Confluence

Use the reachable-state, operation and merge assumptions of the referenced model when applying invariant-confluence reasoning. Preserving invariants under merge is not by itself a theorem about all returned values, authority changes or observation contracts.

## 4.2 RedBlue consistency

Red/blue operation classification is a predecessor. CarolinaDB's finite collection of profiles does not establish a new ordering or lattice by extending the number of labels.

## 4.3 Fine-grained consistency / PoR

Selective ordering and directed dependencies are antecedents. SIEVE, Quelea, Indigo and Hamsaz also constrain any novelty claim based on automatic classification, declarative contracts, reservations, return values or protocol synthesis.

## 4.4 LoRe

Verified safety and selective coordination in a restricted programming model are antecedents, not inventions of this project. A DBMS implementation must demonstrate an additional useful result instead of relying on a difference in packaging.

## 4.5 Event Horizon / Semi-Linearizability

The prior-art account identifies asymmetric dependencies and a cost-directed synthesis agenda in Event Horizon. It also includes semantic evolution work. The proposed composition/evolution result therefore needs a precise construction and comparison, not a general claim that combining these subjects is new.

# 5. Novelty requirement

The research target is a scoped rule/construction such that, for supported contracts and explicit failure assumptions, histories of composed plans and their authority/contract transitions refine the permitted observable histories. Already-issued final commitments must remain explainable after recovery and evolution.

Required evidence includes a precise accepted fragment, formal statements and assumptions, checked protocol models, an implementation correspondence argument, independent observable-history checking and equivalent-contract experiments. A literature comparison MUST distinguish a new result from reproduction of known techniques.

Deterministic semantic lowering and conservative dependency extraction from **declared versioned operation contracts** are required engineering capabilities. No automatic extraction of arbitrary application-code effects is promised. A correct implementation may still fail to establish scientific novelty or the need for a new DBMS.

# 6. Fundamental abstraction: Invariant Dependency Component

An Invariant Dependency Component (IDC) is a conservative semantic closure of interacting data, operations, invariants, observations and authority requirements. Finding the smallest useful closure is a precision objective, not an assumed complete analysis.

IDCs and physical shards are distinct. One IDC may span shards; one shard may host several IDCs. A transfer may instantiate multiple parameterized IDCs with a qualified composite atomicity obligation instead of permanently merging every account into one global component. Unknown overlap widens the closure or prevents compilation.

# 7. Programming model

The accepted language is the restricted DSL of SPEC-003: `RECORD`, `INDEX`, `INVARIANT` and versioned `OPERATION`. Operations explicitly declare `REQUIRE`, `READ`, `EFFECT`, `ENSURE`, `RETURN` and `CONTRACT`. Pure deterministic evaluation and a closed operation set are mandatory.

All business, maintenance, import, repair and administrative writers touching protected state MUST be represented in the same analysis and admission boundary. Rights and authority records are runtime-owned and inaccessible through ordinary user mutation.

# 8. Record declaration

Records have stable identities, checked field types, exactly one primary key and explicit references/indexes. Numeric precision, missing-record behavior, string comparison and canonical unique-key meaning are defined in SPEC-003; no implicit case folding or mathematical unbounded integer is assumed.

# 9. Invariant declaration

Supported shapes include scoped bounds, uniqueness, references, aggregate membership, conservation, monotone facts, state transitions and matching causal prerequisites. Every invariant has an explicit complete scope, deterministic evaluator and version. Initial data MUST satisfy the admitted invariant set before activation.

An undeclared business requirement is outside the guarantee. A global invariant cannot be checked by reading arbitrarily stale fragments and calling their union a valid snapshot.

# 10. Mutation model

Mutations invoke immutable named operation versions with canonical arguments and a complete observable contract. Evaluation constructs a private candidate; only the selected protocol's accepted durable decision can authorize final success.

A reservation receipt promises the accepted reservation, not the current global free quantity. A business rejection must be justified by the observation and refusal semantics declared in its contract. Failed evaluation changes no business state; a final rejection may still require durable deduplication/result persistence.

# 11. Why unrestricted UPDATE is not the primary mutation primitive

An unmodeled update can invalidate invariant closure, result semantics or exclusive rights. It MUST NOT bypass compiled admission. No administrator flag may waive a proof obligation while retaining the same correctness claim.

An optional administrative mutation path must be a registered deterministic operation with a complete footprint, observable contract and qualified plan. Conservative C5 is permissible only after all of its validation, authority and composition obligations hold; otherwise reject the mutation.

# 12. Invariant classes

The recognized IR categories are `LowerBound`, `UpperBound`, `Unique`, `Referential`, `AggregateUpperBound`, `AggregateLowerBound`, `Conservation`, `Monotonic`, `StateTransition`, `CausalPrerequisite` and `Arbitrary` (SPEC-003).

Recognition is not eligibility for every protocol. `Arbitrary` means a typed deterministic supported predicate with a complete evaluator; opaque code or nontermination is rejected. An analyzer may be incomplete even when evaluation is supported, permitting independently safe C5 validation. SPEC-014 limits the implemented fragment at each milestone.

# 13. Operation Effect IR

Effects lower deterministically from declarations into the SPEC-003 effect IR, retaining evaluation order, guard failures, captured reads, exact arithmetic, result dependencies and atomic groups. Normalization MUST preserve accepted and rejected observations, not only final row values.

Business reserve/release effects are separate from generated escrow authority changes. Accepted replicated effects use a specified semantic handler; replicas do not reevaluate an origin guard against arbitrary local state.

# 14. Invariant IR

Invariant IR carries stable identity/version, scope, dependencies, predicate and evaluator version. Canonical artifacts must be deterministic, versioned, bounded and hashable according to SPEC-003. Wire/snapshot compatibility belongs to SPEC-012 and physical key/page formats to SPEC-002; neither is implied by canonical IR JSON.

# 15. Compiler pipeline

The mandatory phases are deterministic parsing/type checking; semantic lowering; complete footprint and invariant closure; parameterized IDC derivation; sequential definedness/preservation; directed concurrency/observation analysis; supported candidate generation; joint compatibility checking; cost-directed selection; and emission of plans, evidence, diagnostics and transition prerequisites.

Analysis includes representation limits and every interacting writer. A budget limit produces explicit failure/unknown; it cannot yield a partly explored closure with a safe label. Compilation is side-effect free; activation is a separate catalog/evolution procedure.

# 16. Dependency hypergraph

Graph nodes cover data selectors, operations and invariants. Edges additionally capture absence/membership predicates, result reads, authority, causal facts and atomic groups. Proven key separation may split components; two differently named parameters are not proof of disjointness.

# 17. Directed dependency graph

Semantic edges preserve direction and matching instance/key relations. A payment occurrence required by a shipment is not a requirement to wait for all payments. An invalidating refund adds a new exclusion or validation obligation; observing the earlier payment alone does not resolve that interaction.

SPEC-004 defines edge kinds and orientation. Directed execution obligations remain distinct from undirected connectivity used to compute conservative closure.

# 18. Consistency classes

| Family | Necessary obligations in addition to the full contract |
|---|---|
| C0 LOCAL | Fenced exclusive local authority for the complete affected scope; local serialization/validation and qualified failover. Physical co-location alone is insufficient. |
| C1 COMMUTATIVE | Accepted-effect algebra, convergence, invariant/result preservation, checked representation and durable identity/deduplication under the exact replication handler. |
| C2 CAUSAL | Safe unordered concurrency plus complete matching prerequisites, downward-closed visibility and durable session frontiers. v1 guarantees are group-scoped. |
| C3 ESCROW | Conserved resource decomposition; covered producers/consumers/invalidators; atomic rights/effects/outcomes; qualified transfer and authority recovery. |
| C4 CERTIFIED | Complete predicates/conflicts, ordered certification authority, durable reservations and qualified prepare/decision/publication. |
| C5 SERIAL | A durable ordered authority over the complete scope; invariant/guard/result evaluation, fenced alternative writers and atomic publication. |

Every family also needs the promised durability, observation boundary, exact outcome recovery and compatible interactions. C3 may require causal scheduling; C5 may require distributed publication. Family numbers do not authorize composition or rank measured cost. C4 is optional and implemented last.

# 19. Compiler safety rule

```text
Unproven obligation -> reject that candidate
Independent safe compatible alternative available -> may select it
No complete safe candidate -> NoSafePlan
```

Telemetry and an absence of observed failures are not proofs. A certificate must identify rule versions, premises, assumptions, closed operation set and runtime qualification. Runtime validity checks cannot be replaced by trusting a cached family label.

# 20. Preservation analysis

Sequential preservation includes `I(S) AND Pre(S,a) AND Defined(S,a) => I(S')` for an accepted transition. Concurrent accepted guards/results, duplicate effects, reachability, merge semantics and observation commitments require additional obligations.

Pairwise commutativity is usable only under a rule that establishes safety for all admitted histories in its stated fragment; higher-arity requirements cannot be dropped. Supported analyses may include interval/key reasoning, abstract interpretation, set/finite-state rules and compilation-time SMT. SMT is not required in the transaction hot path.

# 21. Commutativity analysis

Two increments may commute as mathematical state functions yet overflow a finite representation or return incompatible exact values. Conversely, effects on distinct rows can interact through an aggregate bound. The compiler MUST analyze accepted effects, guards, observations and invariant closure instead of treating row conflict or effect algebra as complete safety.

# 22. Escrow synthesis

SPEC-004 defines the bounded-resource synthesis fragment and SPEC-006 its runtime accounting. All producers, consumers, bound changes, reservations and invalidators participate in the proof. Supported shapes do not automatically make every operation eligible.

A consume operation can proceed while its holder has valid sufficient usable rights and all durability/observation obligations hold. Lack of local rights is an authority condition, not evidence of global resource exhaustion.

# 23. Escrow metadata

SPEC-006 exclusively defines the ledger and conservation equations. `U` denotes usable rights and `X` rights in transit; the business quantity `free = U + X` does not grant spending authority. Admission uses the holder's usable rights with exact `ResourceGeneration` and `HolderAuthorityEpoch` bindings.

Rights metadata is correctness-critical durable state. Client-result durability, holder-authority durability and transfer-decision durability are separate plan policies. A profile that loses the only authoritative evidence may strand capacity; it must never recreate the lost rights from an estimate.

# 24. Rights transfer protocol

Use the complete SPEC-006 state machine, durable decision authority and idempotent transfer identity. Moving usable rights into in-transit state, recording transfer decisions, importing recipient rights and replaying each step MUST preserve unique spend authority.

No abbreviated prepare/accept/commit exchange in this foundation is an alternative protocol. Timeout is not abort evidence; permanent loss beyond the policy may leave rights unavailable. Transfer authority requires catalog/security authorization as well as ledger consistency.

# 25. Certification

SPEC-007 owns C4's SnapshotCut, predicate evidence, replicated certifier and durable reservations; SPEC-008 owns shared decision/publication requirements. Validation must cover absent keys, ranges, membership changes, authority and interfering writers. Reservations remain effective until the qualified resolution/publication boundary.

C4 cannot be enabled by emitting a deterministic predicate alone. Its runtime and applicable formal/fault gates must be qualified, and its complete cost must be compared with C5.

# 26. Transaction envelope

An invocation MUST bind the SPEC-012 `RequestKey`, its `RequestHash`, unique mapped `TxnId`, exact operation/schema/contract/plan identities, concrete `IdcBinding` values and all required authority, causal and observation evidence. Accepted effects and exact outcome data flow into SPEC-002's durable `CompiledBatch` contract.

Wire envelopes, final receipts and terminal outcome records are defined by their owning specifications. Omitted fields in explanatory examples never license omission of identity, generation, result or durability evidence from persistence.

# 27. Executable Operation Plan

The immutable plan contains the full execution profile: admission, visibility, authority, ordering, validation, atomicity, durability, replication, result and recovery programs. It binds analyzed inputs, qualified template/rule versions, topology assumptions, routing selectors and transition requirements.

SPEC-004 owns its canonical representation. A family label is only a dispatch/metrics summary. A hash fixes bytes; it does not authenticate a sender or prove that the bytes implement a correct protocol.

# 28. Plan explanation

EXPLAIN MUST show contract/results, affected invariants and IDCs, candidate eligibility/rejection/unknowns, accepted compatibility rules, declared costs, authority/durability assumptions, partition outcomes and prerequisites for any alternative plan.

For an inventory sale, explain that C1/C2 do not allocate exclusive spending authority; C3 may become eligible with conserved rights; and a C5 alternative requires accounting for every still-authorized writer. Report eligibility as a candidate until rule and runtime evidence permit activation.

# 29. Proof artifact

SPEC-004's consistency certificate records input hashes, rules, premises, candidate rejections, compatibility obligations, assumptions and evidence manifests. Distinguish structural validation, discharged proof obligations and runtime qualification. The artifact is not automatically a proof-assistant theorem.

All safety-relevant assumptions MUST have an enforceable predicate or accepted evidence/model reference. Unknown, stale or missing required evidence makes the affected candidate unavailable.

# 30. Storage architecture

The runtime separates logical state, operation/recovery records and protocol metadata by responsibility, while preserving one atomic local durable boundary for their dependent changes. SPEC-002 defines that boundary through `DurableStorageKernel` and `CompiledBatch`.

The native B+Tree remains the reference storage design. An independent in-memory semantic model and an experimental existing-storage adapter may implement the same interface with explicitly scoped capabilities. The research thesis cannot depend on choosing a particular page structure.

# 31. State Store

SPEC-002 owns local MVCC, typed key encoding, point/range reads, registered snapshots, atomic batches, checksummed journal/pages, recovery and GC. Its native B+Tree choice remains in force. A reference in-memory model does not establish disk durability, and no adapter may advertise a durability capability it has not qualified.

# 32. Operation Log

The log carries recoverable business changes, protocol transitions, exact decision/result identity and replay information under SPEC-002. Append framing, checksums, durable barriers, segment retention and incomplete-tail treatment are mandatory. Required committed history cannot be discarded as an incomplete tail merely because decoding fails.

# 33. Consistency Metadata Store

Authority bindings, rights, causal frontiers, request mappings/results, reservations, decision/publication evidence and migration fences are durable protocol state. Each uses its own typed generation/sequence domains and durability policy; none is disposable cache. Replication and GC must retain everything needed to explain commitments and prevent replay.

# 34. Memory model

Hot caches may hold immutable plans, routing snapshots, MVCC versions, rights views and causal frontiers. Their contents do not grant authority without the owner's admission conditions. Pointer replacement cannot activate a migration or revoke an offline holder. Durable catalog publication, installation and fencing govern new admissions.

# 35. Query execution

Every read has an explicit observation scope and visibility/result contract. A local snapshot, causal observation, serial-scope value and multi-IDC atomic snapshot are different capabilities. A read used to authorize mutation belongs in the operation's dependency footprint.

The runtime MUST keep prepared/staged state outside public visibility. Independent local snapshots do not prove a distributed atomic cut. If no qualified plan satisfies a requested observation, return typed unavailability/unsupported status instead of weakening it.

# 36. Session guarantees

C2 v1 supports read-your-writes, monotonic reads and causal dependencies **within one replication group**, subject to the declared contract and SPEC-005. SPEC-012 owns the group-scoped token encoding and client behavior.

Cross-group session guarantees require a separately qualified composite plan; they are unsupported in the baseline. A client must not drop another group's dependency, reuse a token in the wrong scope, or infer a global guarantee from independently satisfied group contexts. Migration preserves token meaning through a qualified mapping or returns explicit unavailability.

# 37. Replication by consistency class

C1/C2 use SPEC-005's accepted-operation identity, deduplication, semantic application, anti-entropy and group-scoped causal contexts. C3 additionally preserves the SPEC-006 authority ledger. C4 preserves certifier reservations and evidence; C5 preserves ordered authority and decisions under SPEC-008.

Durability is a separate contract dimension for every family. Async delivery does not satisfy a requested replicated-stable final result by itself. Replication membership changes, old-origin delivery and catch-up obey the catalog, codec and evolution contracts.

# 38. Consistency groups are not shards

Physical placement is independent of semantic closure and protocol ownership. A shard may host C1, C3 and C5 state only when every interaction has a checked compatibility rule. Separate locks/logs or a common storage engine do not establish that rule.

# 39. Dynamic plan specialization

The runtime may specialize allocation/placement only through qualified transitions of the admitted plan. Telemetry can propose a new candidate; it cannot extend its proof domain. Safety-relevant changes follow SPEC-009/011 even when the family label is unchanged.

# 40. Dynamic strengthening

There is no unconditional runtime strengthening rule. Switching C1/C3/C4 to C5 can change authority, observations, availability and recovery obligations. Such a switch is permitted only through a prequalified transition or a newly compiled candidate with completed admission/drain/fence/install requirements.

On stale authority, unsupported capabilities or lost prerequisites, wait/refuse according to the contract until a safe transition is possible. Relabeling the request with a larger C number cannot resolve missing evidence.

# 41. Schema evolution

Schema, invariant, operation and observation changes are versioned semantic changes. Existing final results remain bound to their original contract. New invariants require validation against the complete closed state, including outstanding resource commitments and prepared work.

SPEC-009 owns barrier/drain evolution; SPEC-011 serializes overlapping catalog, authority and membership changes. A schema hash alone does not encode all generation/authority identity.

# 42. Recompilation protocol

Compile and register the complete candidate/transition contract; lock its semantic closure; durably close old authorities; drain accepted/in-doubt work and rights; validate and transform closed state; install target state; atomically activate through catalog CAS; then retire history only after all pins and anti-replay requirements permit it.

Every durable prefix must have a defined recovery path. An unreachable still-authorized emitter blocks incompatible activation. No clock timeout, catalog increment or deletion of an old fence substitutes for closure evidence.

# 43. Operation versioning

Operation versions are immutable. A retry resolves its original operation/hash/contract and exact result, even after a new version activates. New invocations of old versions require an explicit compatible admission rule; otherwise reject them. A final outcome cannot be recomputed under new semantics.

# 44. Topology

Processes may combine data, gateway, catalog, sequencer and certifier roles. SPEC-014 stages local execution before a three-node control plane and data protocol. Roles are logical authorities, not authorization inferred from process location. No automatic scaling or cloud-specific infrastructure is required for the first experiment.

# 45. Catalog

SPEC-011 owns the ordered replicated catalog, typed generations, immutable artifact registries, placement/membership, capabilities, authority grants/fences, migration ownership, read barriers, recovery and retention. A cached route is not a new authority grant.

Control-plane unavailability may prevent new grants or activation while an already-authorized plan continues within its documented conditions. It cannot create permission to revoke a disconnected spender or admit a conflicting successor.

# 46. Routing

SPEC-012 defines `RequestKey = (TenantId, RequestNamespace, StableRequestId)` and `RequestHome = route(RequestKey)`. Before any effect, the authorized RequestHome durably compare-and-swaps an absent key to one fresh unique `TxnId` and `RequestHash`; retries return that mapping, and mismatched content is rejected.

Plan routing then computes concrete IDC participants from canonical arguments and catalog placement. Request routing never depends on a `TxnId` that has not been allocated. Failover/rehome must retain the mapping and fence the old RequestHome authority.

# 47. Multi-IDC transactions

Whole-invocation atomicity requires a qualified composite plan when one operation spans IDCs. Internal parallel work is allowed; independent final commits for the two halves of a transfer are not.

The initial distributed composition uses SPEC-008's C5 prepare/decision/install/publication protocol. C4 composition is enabled only after its separate qualification. Promised combined reads and final results must wait/help or use a legal prior cut until publication permits observation; local installation alone is not public completion.

# 48. Failure model

The baseline covers non-Byzantine crash/recovery, partitions, lost/delayed/reordered/duplicated messages and storage failures within the chosen qualified durability profile. Permanent loss exceeding its durable witnesses is outside that profile and must be reported without fabricating state or outcomes.

No bounded-clock lease assumption or global wall-clock authority order is implicit. SPEC-013 defines identities and trusted channels; authenticated channels do not make compromised consensus members Byzantine-tolerant.

# 49. Crash recovery

SPEC-002 restores valid durable state and unresolved records; protocol recovery then reconciles request mappings/results, authority bindings, rights, causal holes, reservations, decisions/publication and catalog admission. Resume only after the required readiness and authority evidence holds.

Replay preserves semantic origin identity regardless of local MVCC order. Prepared work remains invisible/in doubt until a valid recorded decision resolves it. Transport timeout cannot invent an abort; a durable final result cannot disappear because its reply was lost.

# 50. Failover safety for escrow

A promoted holder must recover authoritative rights for the exact resource generation and holder authority epoch under SPEC-006/011. A replica lacking that evidence has **zero spendable authority** until reconciliation; this is an admission restriction, not a rewrite of the durable resource ledger.

Authority/transfer durability policies govern whether evidence can be recovered after permanent member loss. Availability can be lost while safety holds. Capacity cannot be reconstructed from stale business `free` alone.

# 51. Network partitions

Progress is conditional on the full plan. C1 needs still-valid admission and local durability; C2 additionally needs its prerequisites; C3 needs usable rights and required witnesses; C4/C5 need their decision/ordering/publication authorities. A contract asking for stronger durability or observations may prevent a nominally local path.

No unconditional availability or fairness promise follows from a family name. Missing authority/dependency and global business rejection are distinct outcomes. Eventual progress assumptions are stated and tested separately from safety.

# 52. Security boundary

SPEC-013 owns client/node/cluster/tenant identity, authorization, authenticated channels, credentials, token integrity, revocation, replay and downgrade defenses, audit and storage/backup security profiles.

The trusted path includes compiler rules/checkers, admitted runtime handlers, catalog authority and durable storage. Nodes validate exact hashes/versions, scoped capabilities, identity, active admission and retained historical replay rules. A valid hash or signature is not a semantic proof; a current authenticated node is not automatically authorized for every tenant/IDC.

# 53. No LLM in correctness path

An LLM may propose contracts, examples or explanations. It MUST NOT decide invariant safety at runtime, waive obligations, issue authority, replace certification or supply unchecked executable semantics. Correctness decisions remain deterministic and attributable to versioned rules and evidence.

# 54. Compiler crate architecture

The first implementation should use a small Rust workspace separating pure language/IR, reference semantics, compiler/plan, storage and runtime/control responsibilities. Causal, escrow, certification and protocol-specific modules are added at their SPEC-014 milestones. Module names and crate boundaries are implementation choices; no crate or CLI is claimed to exist because it is described here.

# 55. Core types

SPEC-011 owns the strongly typed identity/generation taxonomy; SPEC-012 owns request identity. In particular:

```text
IdcBinding {
  idc_id: IdcId,
  idc_generation: IdcGeneration,
  authority_epoch: IdcAuthorityEpoch
}
```

`CatalogGeneration`, `PlanGeneration`, `IdcGeneration`, `IdcAuthorityEpoch`, `PlacementEpoch`, `MembershipGeneration`, `ResourceGeneration`, `HolderAuthorityEpoch`, `StorageEpoch`, `OriginEpoch`, `RequestHomeEpoch`, `LocalCommitSeq` and `SerialPosition` have distinct scopes. No generic epoch or bare `(IdcId, u64)` may replace their types. Equal encoded integers never authorize interchange.

# 56. Invariant representation

Use SPEC-003's complete `InvariantIR`, including scope and evaluator semantics. A category enum is not a proof or executable evaluator. Unknown required invariant representations fail closed.

# 57. Effect representation

Use SPEC-003's `EffectIR` with typed operands, guards, captured reads, atomic groups and definedness checks. The semantic handler named by the plan controls physical lowering; translating concurrent increments into stale snapshot `Put` operations violates the accepted-effect semantics.

# 58. Analysis result

Analysis produces per-obligation judgments and candidate/compatibility evidence under SPEC-004. Results include the affected closure, admitted/rejected/unknown candidates, selected profile, costs and explicit assumptions. There is no `minimum_class` field or numeric maximum over per-invariant families.

# 59. Proof status

`Proven` means a named accepted rule/checker discharged the stated obligation under listed premises. `Disproven` requires a replayable counterexample. `Unknown`, including unsupported analysis, timeout or missing runtime evidence, never means safe.

Bounded model-checking/test success is evidence for its recorded scope, not automatically a proof of an unbounded compiler rule. SPEC-010 qualification verdicts are a different status domain.

# 60. Counterexample generation

The compiler SHOULD emit minimized replayable semantic counterexamples when available. The stock-1/two-sale example must capture separately accepted guards, normalized effects and final confirmations, not merely sequentially reject the second sale and conclude concurrency is safe.

The independent reference semantics must reproduce a counterexample before it is labeled disproven. Failure to find one within a bound remains explicitly limited evidence.

# 61. CLI

Proposed commands cover compile/check, explain, plan/graph inspection, request resolution, authority inspection, migration inspection and qualification/replay. They are implementation deliverables. Compilation and explanation never activate plans automatically; administrative activation requires the catalog/security protocol.

# 62. EXPLAIN CONSISTENCY

Show the whole profile and evaluated candidate set instead of a C0-to-C5 search ladder. A sale example should show the full resource/representation closure, receipt semantics, C1/C2 rejection, C3 rights obligations, C5 authority-transition prerequisites and missing evidence. Costs must include background rights transfer and recovery where relevant.

# 63. Observability

Measure operations by contract/family; admission refusals versus business rejections; causal waits; rights usable/in transit; transfer backlog; prepared/publication backlog; catalog generations; migration duration; recovery and unknown-request resolution. Use bounded metric labels; exact IDs belong in traces. Audit logs do not substitute for the durable protocol records they describe.

# 64. Benchmark philosophy

The experimental question is whether contract-driven plans reduce total coordination cost while preserving **equivalent observable guarantees**. Finality, exact results, failure domains, reads, useful progress and retries must match the comparison. High key-value throughput alone does not answer it.

# 65. Required baselines

SPEC-010 owns baseline selection and equivalence sheets. Include competent strong local/distributed execution, safe weaker execution where equivalent, manual escrow/causal implementations and a runtime over existing storage. Compare relevant prior-work implementations when reproducible; clearly label reimplementations and unavailable conceptual comparisons.

Versions, configurations, durability and workload ports must be recorded at evaluation time. No baseline is claimed installed or measured here.

# 66. Required workloads

Use the SPEC-010 W1–W8 contracts: inventory/reservations, account/ledger, unique namespace, causal order, TPC-C-derived subset, edge quota, mixed-domain allocation and plan/contract evolution. Encode invariants, exact return meanings, rejection behavior and observations for each.

These are synthetic research workloads, not official benchmark compliance or validated application deployments. Include adversarial exact-result, finite-overflow, revocation and cross-domain variants.

# 67. Metrics

Report useful committed throughput; p50/p95/p99 end-to-end request latency including retries/waits; outcome categories; foreground/background messages/bytes; fsync/ordering rounds; rights starvation/rebalancing; compiler/metadata costs; catch-up/recovery; and migration disruption. Publish the workloads where conventional coordination wins.

# 68. Correctness testing

Benchmarks are invalid if the independent oracle finds a contract/invariant violation. Check complete histories, durable outcomes, authority and legal observation scopes, not just converged rows. A final reservation erased after an upgrade is a failure even if final stock remains nonnegative.

# 69. Jepsen-style testing

SPEC-010 requires deterministic schedules and real-process fault campaigns for enabled distributed features. Include retries, process crashes, partitions, stale authorities, decision/publication gaps, causal holes, storage faults and evolution/GC. Claim actual Jepsen execution only with retained framework configuration and results; a bespoke runner is identified as such.

Only isolated allowlisted test infrastructure may be a fault target. Existing EVA/NietzscheDB and the shared HeraclitusDB memory service are not test targets.

# 70. Model checking

The mandatory applicable formal gates in SPEC-010 are FM-1 (escrow transfer/authority), FM-2 (decision/publication) and FM-3 (evolution/fencing). A distributed correctness claim requires its model-checking evidence, passing deterministic simulation and passing real-process fault campaign.

Each gate records model/tool versions, invariants, fairness assumptions, explored bounds, negative controls and implementation transition mapping. This conjunction does not prove arbitrary Rust code correct. A stage that does not enable the feature may defer that feature's gate; it cannot advertise the feature as qualified.

# 71. Property-based testing

Use independent properties for typed evaluation, normalization equivalence, accepted-effect replay, merge idempotence, rights conservation, deterministic planning and canonical identity. Tests must check semantics and meaningful boundaries rather than mirror the implementation.

# 72. Fuzzing

Fuzz parsers and IR, plan, receipt, wire, snapshot, journal and recovery decoders with explicit resource bounds. Invalid/unknown mandatory semantics fail closed. Use SPEC-012's compatibility/negative corpus and SPEC-002's physical-format requirements.

# 73. Compiler correctness target

**Soundness target:** every admitted execution of the selected compatible plans satisfies the full contract under its supported assumptions, including invariant validity, observations, outcomes, authority, durability and recovery/evolution obligations.

**Precision/cost target:** accept useful contracts and choose lower declared cost among supported compatible safe alternatives. Neither a globally weakest execution nor a complete arbitrary-program analysis is required or claimed. Soundness is mandatory for admitted capabilities; precision is measured research/optimization work.

# 74. Research hypotheses

H1: restricted declared contracts permit useful deterministic lowering and safe plan derivation without manual protocol implementation.

H2: per-IDC plans reduce total coordination cost versus equivalent conservative execution for a meaningful workload region.

H3: conserved escrow authority improves useful partition progress under explicitly measured durability and resource-distribution conditions.

H4: directed dependencies reduce unnecessary ordering under equivalent observable contracts and compatible composition.

H5: supported mixed plans and their authority/contract transitions preserve observable commitments through the tested failure model, with a scoped formal refinement construction.

These are unconfirmed hypotheses. SPEC-010 E1–E5 defines the experiments; neither document status nor zero reported tests supports them.

# 75. Falsification criteria

Reduce scope, prefer a runtime/library or abandon the thesis if useful contracts mostly require conservative execution, annotations amount to hand-written protocols, metadata/background costs erase gains, exact observations remove the benefit, composition/evolution cannot preserve commitments, or the proposed result is already covered by prior work.

A supported counterexample blocks correctness qualification immediately. Equivalent benefit over existing storage challenges the need for a new engine. Report negative and inconclusive experiments; commercial demand and scientific novelty require evidence beyond protocol correctness.

# 76. Why this may require a new DBMS

The compiler must control every protected writer's admission, effects, authority, durability, replication, reads and evolution. This motivates an integrated runtime; it is not an impossibility proof against extensions or middleware.

SPEC-002 retains a native B+Tree behind the durable kernel interface. SPEC-010 E5 compares the same semantics over existing storage. If native storage adds no useful benefit, a layer over existing storage is a valid outcome of the research.

# 77. Difference from NietzscheDB

This experiment addresses transactional contracts and distributed coordination. It introduces no requirement for vector search, geometric graph representation, cognitive memory, GPUs or LLMs. It does not change EVA's NietzscheDB backend or operations.

# 78. Difference from HeraclitusDB

The primary experimental artifact is an executable operation contract with a justified distributed plan. Local event history and durable evidence support recovery; their use does not by itself establish the proposed composition result. No modification of existing HeraclitusDB services is required to revise or validate these specifications.

# 79. Architectural identity

> A conservative compiler and runtime for observable operation contracts, selecting compatible qualified plans and preserving final commitments through composition, authority transfer, evolution and recovery.

This is a design objective and candidate research direction, not an implemented capability claim.

# 80. MVP boundary

[SPEC-014](SPEC-014.md) is the sole implementation-sequencing owner. Start with typed records, checked integer/fixed-decimal semantics, point-scoped bounds and a complete sequential interpreter. Add only explicitly qualified shapes and profiles. C5 provides the first plan/runtime reference; C1/C2 and C3 follow; C4 is last and optional.

The historical milestone IDs M0–M10 below remain traceability labels. Their numeric order is not the implementation schedule; SPEC-014 MVP-0–MVP-8 supersedes that schedule.

# 81. MVP topology

Begin with an in-memory semantic reference and a single local storage process. The first distributed experiment uses three isolated node processes with independent data directories and declared durability domains. It needs no Kubernetes, GPU, CXL, cloud dependency or dashboard. Co-located test processes do not establish independent machine-loss durability.

# 82. Milestone M0 — Formal core

Historical work package: grammar, typed AST/IR, explicit contracts, reference interpreter and canonical artifacts. Delivered through SPEC-014 MVP-0. Exit requires executable positive/negative fixtures, deterministic bytes and explicit unsupported semantics.

# 83. Milestone M1 — Static analyzer

Historical work package: conservative semantic closure, sequential validity, candidate obligations, evidence and explanations. MVP-1 begins with C5 selection only and typed rejection. Later protocol derivation follows the feature milestones; an eligible template is not active before runtime qualification.

# 84. Milestone M2 — Single-node runtime

Historical work package: native storage, compiled admission, atomic business/protocol/request/outcome persistence and crash recovery. Delivered through MVP-2. Mutation already follows plans; recovered retries return the same exact outcome.

# 85. Milestone M3 — C1 replication

Historical work package delivered in MVP-5, after the C5 distributed reference and publication boundary. Qualify accepted-effect identity, convergence, anti-entropy, exact receipt replay and bounded overflow/observation exclusions.

# 86. Milestone M4 — C2 causal execution

Historical work package delivered in MVP-5. Qualify group-scoped prerequisites, holes, sessions, catch-up and durable publication. Cross-group session guarantees remain disabled without a separate composite plan.

# 87. Milestone M5 — C3 escrow

Historical work package delivered in MVP-6. Qualify resource synthesis, consumption/production, reservation lifecycle, transfer, holder fencing and distinct durability policies. FM-1 and both simulation/process fault evidence are mandatory for a distributed claim.

# 88. Milestone M6 — C5 serial IDC

Historical work package moved **before C1/C2/C3**: MVP-3 establishes the three-node catalog and single-IDC C5 authority; MVP-4 establishes multi-IDC decision/publication. Ordered execution still validates all contract obligations and does not admit unsupported sequential semantics.

# 89. Milestone M7 — Multi-class operation planner

Historical work package integrated during MVP-5/6 and requalified during MVP-7. Each enabled interacting mixture requires explicit compatibility and observation evidence. Merely running independent families side by side is not a composition result.

# 90. Milestone M8 — C4 certification

Historical work package delivered last in MVP-8. Enable only if complete predicate certification, durable reservations and decision/publication pass applicable gates and measured workloads justify its cost versus C5. It may remain disabled indefinitely.

# 91. Milestone M9 — Dynamic plan generation

Historical work package delivered in MVP-7, before optional C4. Implement SPEC-009 barrier/drain, immutable transitions, close/install/activate recovery, retained request outcomes and session mapping. FM-3 is mandatory; overlapping incompatible generations remain unsupported.

# 92. Milestone M10 — Evaluation

Each MVP stage produces evidence; comparative research evaluation follows the applicable SPEC-010 gates. Deliver supported capability manifests, failures/unknowns, equivalent-contract baselines, composition/evolution histories, performance distributions and storage-comparison results. Never invent benchmark numbers, fixture hashes or completed proofs.

# 93. Minimum paper contribution

A paper must identify a precise new result beyond known invariant analysis, escrow, per-operation consistency or directed dependencies. The preferred candidate is a useful compositional observable-refinement construction covering explicit authority, evolution and failure behavior in a stated fragment.

Proof/argument, limits, prior-art comparison and reproducible experiments must support that result. Six protocol labels, a native engine and a working prototype alone do not establish it.

# 94. Definition of success

A developer can declare inventory/reservation rules, effects and receipt/read semantics. The compiler explains eligible plans, rejects unsupported promises and derives a qualified plan. The runtime permits useful work under its stated authority/durability conditions, preserves exact final results through retries and faults, and carries commitments through a qualified transition.

Success is measured by executable evidence and equivalent-contract benefit. Preserving stock nonnegative by rejecting every request is insufficient evidence of usefulness.

# 95. Definition of failure

The intended abstraction fails when developers still implement the correctness-critical protocol manually, accepted plans violate declared observations or commitments, or gains depend on quietly weakening the comparison contract. Safe rejection of an unsupported contract is expected compiler behavior; broad rejection may refute usefulness rather than soundness.

# 96. Final invariant of the project

```text
NO ADMITTED EXECUTION WITHOUT ITS FULL CONTRACT OBLIGATIONS SATISFIED
NO FINAL COMMITMENT ERASED BY RETRY, RECOVERY OR PLAN EVOLUTION
```

Optimization is limited to compatible qualified candidates under explicit assumptions. If evidence is missing, the capability remains unavailable.

# References

- [Research proposal](../PROPOSTA-DE-PESQUISA.md), especially §§1–2 and direction A: observable contracts and conditional research scope.
- [Prior-art analysis](../research/consistency-prior-art.md): maintained primary-source bibliography, predecessors, research hypotheses and falsification experiments.
- [Review input](REVISAR.md): cross-specification findings motivating Draft 0.2; retained as review evidence, not rewritten as implementation status.
- [Implementation profile](SPEC-014.md): authoritative staged delivery and evidence requirements.

# End of SPEC-001
