# SPEC-003 — Invariant & Effect IR

**Status:** Draft 0.1 — proposed implementation contract; not implemented or proven  
**Date:** 2026-09-09  
**Depends on:** [SPEC-001](SPEC-001.md), [SPEC-002](SPEC-002.md)  
**Consumed by:** [SPEC-004](SPEC-004.md); runtime SPECs 005–008; plan evolution in [SPEC-009](SPEC-009.md)  
**Scope:** restricted DSL, typed AST, deterministic contract IR, effects, footprints, dependency hypergraph and proof obligations  
**Reference implementation:** Rust stable; conceptual types below do not freeze a Rust or network ABI  
**Normative terms:** MUST, MUST NOT, SHOULD and MAY express requirements.

## 1. Decision and authority

Every mutation SHALL compile from a versioned operation contract. The contract includes permitted state changes, results, observations, authority, durability and refusal behavior. A declaration of valid states alone is insufficient to select an execution protocol.

This document makes the language and IR requirements of SPEC-001 §§7–17 concrete. It refines the illustrative examples without treating their omitted fields as implicit safety guarantees. SPEC-002 remains authoritative for local storage and durability. SPEC-004 selects execution plans; the IR SHALL NOT contain developer-selected C0–C5 annotations.

The proposal's narrow research target is observable composition and evolution, not the invention of invariant analysis or escrow. The structures and obligations here are a design to implement and test. They are not a completed soundness proof, a benchmark result or a claim of novelty.

## 2. State, histories and observable outcomes

Let `S` be a finite typed map from `(record_id, primary_key)` to a record. The semantic model additionally contains committed internal facts, request outcomes, causal dependencies and authority state. Physical page layout, journal LSN and replica-local MVCC sequence are outside `S`'s business meaning.

An invocation is `(StableRequestId, OperationId, operation_version, arguments)`. Within its declared request namespace a `StableRequestId` SHALL bind exactly one arguments hash, operation identity and `TxnId`. A retry with the same identity and different content is an identity error, not a new invocation. SPEC-009 specifies retention and migration of this binding.

The sequential evaluator has the following contract:

```text
evaluate(S, arguments, observed_context)
    -> Rejected(reason)
     | Candidate(S', normalized_effects, result, obligations)

commit(candidate, authorized_plan)
    -> durable terminal result
```

`Candidate` is private and MUST NOT be returned as success. An effect is accepted only if its definedness checks, preconditions, postconditions and all affected invariant checks hold under the plan's observation and authority rules. Business rejection changes no business state; a deduplication result MAY still be persisted.

`Unknown` after a transport failure is an unresolved client outcome. It is not evidence of rejection or permission to use another `TxnId`. The original request SHALL be resolved by identity. Once success is final, recovery and later generations MUST preserve its exact result and commitments.

An invariant over an observed snapshot applies only when the observation contract identifies a complete admissible snapshot of its scope. Combining arbitrary stale rows from different nodes is not such a snapshot. Queries used to authorize mutation MUST appear in the operation's read footprint and obligations.

## 3. Accepted language fragment

The initial parser SHALL accept records, indexes, invariants and named operations. Expressions are pure; functions, loops, recursion, arbitrary SQL mutation, network calls, clock reads and random generation are absent. UUIDs, timestamps and external facts needed by an operation arrive as typed, recorded arguments with explicit trust requirements.

The following grammar fixes the structure; `expr`, `predicate`, `path` and `effect` are the typed productions in §§4–7. Braces and keywords are literal. Lists are ordered unless explicitly defined as sets.

```text
module     := (record | index | invariant | operation)*
record     := RECORD name { field ("," field)* }
field      := name ":" type [PRIMARY KEY]
index      := INDEX name ON record_name "(" field_names ")"
invariant  := INVARIANT name { invariant_body }
operation  := OPERATION name "(" parameters ")" VERSION integer {
                REQUIRE predicate
                READ { read_decl* }
                EFFECT { effect* }
                ENSURE predicate
                RETURN result_expr
                CONTRACT { contract_fields }
             }
read_decl  := binding "=" read_expr
read_expr  := path | EXISTS path | SCAN record_name WHERE predicate
```

The concrete spelling of contract fields is fixed in §9. `REQUIRE true`, `READ {}`, `ENSURE true` and `RETURN Unit` are explicit where unused. This prevents parser defaults from silently making a final observation promise. Records have exactly one primary key; tuple key types are allowed. All reads implied by effects and predicates are inferred in addition to explicit `READ` declarations.

Frontend sugar matching SPEC-001 examples MAY omit these sections only if the compiler emits the complete expanded contract for review. An unspecified result/visibility contract MUST NOT silently become an exact global read.

### 3.1 Typed AST and names

```text
TypedModule {
  language_version: u32,
  records: Vec<RecordDecl>, indexes: Vec<IndexDecl>,
  invariants: Vec<InvariantDecl>, operations: Vec<OperationDecl>
}
TypedExpr { type_id: TypeId, node: ExprNode, source_span: SourceSpan }
TypedPath { record: RecordId, key: TypedExpr, field: FieldId }
OperationDecl {
  id: OperationId, version: u32, parameters: Vec<Parameter>,
  pre: TypedExpr<Bool>, reads: Vec<ReadBinding>,
  effects: Vec<TypedEffect>, post: TypedExpr<Bool>,
  result: TypedExpr, contract: ContractDecl
}
```

Names resolve within a module namespace; duplicate names, unresolved references and ambiguous bindings are errors. Record, field, invariant and operation IDs are explicit stable catalog identities. A catalog allocation step assigns them before canonical lowering; ID assignment MUST NOT depend on hash-map iteration or host paths. Recompiling an already identified module preserves IDs.

Source spans are diagnostics only and are excluded from semantic hashes. Operation versions are immutable; changing semantics under an existing `(OperationId, version)` is rejected.

## 4. Types and expression semantics

Initial scalar types are `Bool`, `I64`, `U64`, `Uuid`, `Bytes(max_len)`, `String(max_utf8_bytes)`, finite declared enums and `Decimal(precision, scale)` with `1 <= precision <= 38` and `0 <= scale <= precision`. Supported composites are tuples, records, `Option<T>` and finite sets of comparable scalar/tuple values.

Decimals use a checked signed 128-bit coefficient and fixed scale. A value is valid only if its coefficient fits its declared precision. Integer and decimal addition/subtraction use exact checked arithmetic. Overflow, underflow, division by zero and inexact rescaling return typed errors before commitment; wrapping and saturation are forbidden. Initial affine analysis supports constants and multiplication by constants; general multiplication/division MAY be rejected by the frontend or marked unsupported for analysis when the deterministic evaluator supports it.

Finite machine representation is an implicit invariant. `Increment(I64)` is not an unbounded mathematical counter merely because no business upper bound was declared. C1 selection needs a proof that every permitted merged execution remains representable, or a separately specified exact representation. The initial profile has no implicit arbitrary-precision escape hatch.

Strings are valid UTF-8 and compared bytewise. No implicit locale, case folding or Unicode normalization applies. Global username/email uniqueness is uniqueness of the declared canonical key. A future normalizer requires a named versioned function and migration of existing keys; examples SHALL NOT imply that `Alice` and `alice` are equal by default.

`Option<T>` uses explicit `Some`/`None`; predicates use two-valued Boolean logic, not SQL NULL logic. Reading a missing required record returns `MissingRecord`. `EXISTS` returns `false`; optional reads return `None`. An absent row never silently supplies zero.

Pure expression nodes are literals, arguments, bound read values, field access, tuple construction, exact arithmetic, comparisons, Boolean operators and explicit option/set operations. Enum comparisons require the same enum type. Implicit numeric narrowing and cross-scale comparison without exact conversion are forbidden.

## 5. Invariant IR

```text
InvariantIR {
  id: InvariantId, version: u32, scope: ScopeExpr,
  kind: InvariantKind, predicate: PredicateIR,
  dependencies: Vec<DomainSelector>, evaluator_version: u32
}
ScopeExpr = PerKey(KeyExpr) | PerGroup(Vec<KeyExpr>) | Global(RecordSet)
InvariantKind = LowerBound | UpperBound | Unique | Referential
              | AggregateLowerBound | AggregateUpperBound | Conservation
              | Monotonic | StateTransition | CausalPrerequisite | Arbitrary
PredicateIR = BoolExpr(ExprIR)
            | ForAll(record, filter, predicate)
            | Unique(record, filter, key_expr)
            | ExistsReference(child, parent_key, parent_record)
            | Aggregate(op, record, filter, group_by, value, comparison, bound)
            | TransitionPredicate(old_state, new_state, predicate)
            | RequiresFact(effect_selector, fact_selector)
```

`SUM` of an empty set is zero and `COUNT` is zero. All aggregate arithmetic is exact and checked. `SUM` combines values of one declared scale; a runtime resource limit is not a successful validation result. Aggregate membership is part of the invariant footprint, including inserts, deletes and updates to filter/group fields. The compiler MUST include bound-source records such as `Department.budget`.

`Unique` rejects two extant qualifying records with the same canonical unique key. Absence checks protect the entire key namespace against phantoms. `Referential` checks parent existence at the accepted observation boundary; deletion of a parent is an interacting operation. Cascades require explicit effects and are not implicit.

`Conservation` compares a scoped sum against a constant or declared conserved quantity. Deposit/restock operations can change the conserved quantity only when the invariant explicitly includes the matching source/sink. A transfer MUST NOT use a weaker interpretation that permits a visible half-transfer.

`Monotonic` and `StateTransition` compare the pre/post logical state of an accepted transition. A grow-only fact may never be revoked by an undeclared deletion path. `CausalPrerequisite` means a matching committed fact must be in the operation's causal past, not merely that another operation type exists in the schema.

`Arbitrary` is a typed deterministic predicate, not executable application code loaded without a model. An unsupported analyzer can conservatively route an evaluable predicate to C5. A predicate with unknown semantics, nontermination, external I/O or no complete affected-scope evaluator SHALL be rejected; serialization cannot manufacture an evaluator.

## 6. Effect IR and evaluation

```text
EffectIR {
  effect_id: u32, guard: ExprIR<Bool>,
  kind: EffectKind, read_dependencies: Vec<BindingId>
}
EffectKind = Assign(path, value)
           | Increment(path, positive_amount) | Decrement(path, positive_amount)
           | Insert(record, key, value) | Delete(record, key)
           | AddToSet(path, value) | RemoveFromSet(path, value)
           | CompareAndSwap(path, expected, value)
           | Reserve(resource, reservation_id, amount)
           | Release(resource, reservation_id, amount)
           | TransferQuantity(source, destination, positive_amount)
           | AdvanceState(path, expected, next)
           | EmitFact(fact_type, fact_key, payload)
```

Effects operate on a private sequential candidate state in source order. Each expression reads explicit pre-state bindings or the current candidate state according to its typed node; these MUST be distinguishable in IR. `ENSURE` sees the resulting candidate state. `RETURN` may use arguments, captured reads and post-state values according to §9. Failed effects roll back the entire candidate.

| Primitive | Required behavior |
| --- | --- |
| Assign | Replace an existing field with a same-type value; infer a write and any expression reads. |
| Increment / Decrement | Apply checked exact arithmetic; amount must be positive; no lost update is permitted in physical lowering. |
| Insert | Fail on existing primary identity unless this is the exact deduplicated invocation; never overwrite. |
| Delete | Require an existing row; tombstone it and atomically maintain indexes; include reference/membership effects. |
| AddToSet | Membership insertion; adding an existing value is a deterministic no-op. |
| RemoveFromSet | Membership removal; absent membership is a deterministic no-op. Concurrent add/remove semantics require an explicit verified plan; no implicit last-writer-wins rule. |
| CompareAndSwap | Compare in the plan-authorized state and reject on mismatch; both comparison and replacement are one transition. |
| Reserve / Release | Bind a declared resource transformation and reservation identity as specified below. |
| TransferQuantity | Checked source decrement and destination increment in one atomic effect group; require distinct targets unless an explicit no-op form is declared. |
| AdvanceState | Require the expected current state and a declared legal transition edge. |
| EmitFact | Insert one immutable internal fact identified by effect and invocation; duplicates cannot create another fact. No network side effect occurs. |

`Reserve` and `Release` are business effects, distinct from compiler-created escrow rights mutations. A `ResourceDecl` binds an available field, reserved field, quantity type and reservation record. Reserve moves `q` from available to reserved and creates a reservation for `q`; release moves an existing unconsumed reservation's quantity back. Partial release is admitted only with an explicit remaining-quantity field and atomic update. The initial implementation MAY support full release only and MUST reject the partial form then. Resource supply, consumption and reservation lifecycle are ordinary declared operations with their own invariant closure.

Escrow rights are generated by a plan and inaccessible to ordinary user writes. Releasing a reservation is not permission to mint protocol rights twice. The runtime will atomically lower business effects and rights changes to SPEC-002 batches.

## 7. Deterministic normalization

The compiler SHALL perform these steps in order:

1. Resolve names and stable identities; type-check all expressions, reads and effects.
2. Expand syntax sugar, inline pure constants and make option/numeric conversions explicit.
3. Lower predicates to versioned IR and extract exact state dependencies.
4. Lower operations preserving source effect order, failure points and returned values.
5. Attach read/write/predicate footprints and atomic effect groups.
6. Emit the complete observation contract and runtime-definedness checks.
7. Canonically sort unordered metadata and encode/hash the module.

Algebraic rewrites MUST preserve rejection behavior and observations. In particular, `x += 1; x -= 1` cannot become a no-op if the first effect can overflow or its intermediate value is observed. Sorting effects by target or coalescing transfers requires a checked equivalence rule. No mathematical-real simplification may replace finite-decimal semantics.

The canonical operation body excludes replica-local values. At execution, arguments and accepted reads create a `NormalizedInvocation` containing literal effect operands, guards' decisions, exact result, source plan identity and captured dependencies. Replication applies these accepted effects through their specified semantic handler; it MUST NOT rerun arbitrary application code or reevaluate an origin guard against an unrelated replica snapshot.

## 8. Footprints, aliasing and IDC templates

```text
DomainSelector {
  record: RecordId,
  key_set: Point(KeyExpr) | Range(lower, upper) | Predicate(PredicateIR) | All,
  fields: FieldSet,
  purpose: ValueRead | Write | Membership | Absence | ResultRead | Authority
}
OperationFootprint {
  reads: Vec<DomainSelector>, writes: Vec<DomainSelector>,
  predicates: Vec<DomainSelector>, facts: Vec<FactSelector>,
  atomic_groups: Vec<EffectIdSet>
}
DependencyHypergraph {
  data_nodes: Vec<DomainSelector>, operation_nodes: Vec<OperationRef>,
  invariant_nodes: Vec<InvariantRef>, edges: Vec<Hyperedge>
}
Hyperedge { members: NodeSet, reason: DependencyReason, witness: SourceRef }
```

Edges represent reads, writes, membership/absence tests, invariants, resource authority, results and atomic effect groups. Directed execution obligations remain separate from the undirected connectivity used for closure; SPEC-004 derives their meaning and orientation.

Two domains MAY be disjoint only with a proven key/filter separation. Different parameters are not proof of different keys; `source != destination` is a usable guard. Unknown aliasing becomes overlap. A dynamic predicate without a sound selector widens to `All` for the relevant record set. Unresolved external targets are a compile error.

Candidate IDC templates are connected components under this conservative semantic closure. A per-key invariant SHOULD yield a parameterized IDC template, then an IDC instance for the concrete key. Co-locating data on one shard never erases an edge. Global uniqueness and aggregate membership can create a global or group-scoped component.

Cross-key operations can instantiate several templates. Their atomic-group edge creates a composite interaction obligation; it need not permanently merge every account into one global component. The compiler MUST either prove the parameterized separation and composite protocol sound, or widen the scope and coordinate it conservatively. “Smallest IDC” in SPEC-001 is a precision objective, not license to omit uncertain dependencies.

## 9. Contract IR and observation semantics

Every operation SHALL lower these explicit fields:

```text
ContractIR {
  atomicity: WholeInvocation,
  observation: {
    input_visibility: LocalSnapshot | CausalContext | CertifiedScope | SerialScope,
    result_semantics: Receipt | SnapshotValue | ExactOrderedValue,
    result_scope: ScopeExpr,
    session: Set<ReadYourWrites | MonotonicReads | CausalDependencies>
  },
  durability: LocalStable | ReplicatedStable(FailureDomainRequirement),
  partition_outcomes: Set<Wait | Unavailable | AuthorityUnavailable>,
  authority_requirements: Vec<AuthorityRequirement>,
  refusal_semantics: BusinessPredicate | MissingAuthority | MissingDependency,
  commitment: FinalWhenDurable,
  request_namespace: NamespaceId
}
```

These are contract semantics; they do not choose C labels. The initial DSL uses these field names and enum names verbatim within `CONTRACT { ... }`. `FailureDomainRequirement` is a named, versioned topology policy; a missing policy is an error. It MUST not be interpreted as a hard-coded quorum inferred from its name.

`Receipt` returns arguments/identities and a commitment derived from accepted effects, such as “reservation R for 3 units accepted.” It MUST NOT imply current global stock. `SnapshotValue` returns an exact value from its identified admissible snapshot and includes that frontier. `ExactOrderedValue` returns the value at a declared ordered point for the full result scope. An increment receipt and increment-and-return-current-global-value are different contracts and may select different plans.

Read-your-writes, monotonic reads and causal dependencies SHALL be retained in session context. They do not imply linearizability. A final business rejection such as `OutOfStock` requires an observation that establishes that predicate for its specified scope. Lack of local rights yields `AuthorityUnavailable`, not `OutOfStock`.

`LocalStable` promises survival of supported local crash/restart, not survival of permanent loss of the only durable replica. `ReplicatedStable(policy)` requires the policy's durable witnesses before a final result. The compiler MUST report an unsatisfiable durability/partition combination; it cannot weaken it to preserve availability. No fairness, starvation freedom or bounded response time is implied without an explicit supported contract extension.

## 10. Canonical artifacts and hashes

```text
ModuleIR {
  ir_version: u32, language_version: u32, key_codec_version: u32,
  records: Vec<RecordIR>, invariants: Vec<InvariantIR>,
  operations: Vec<OperationIR>, contracts: Vec<ContractIR>
}
OperationIR {
  identity: OperationRef, parameters: Vec<Parameter>,
  pre: PredicateIR, reads: Vec<ReadBindingIR>, effects: Vec<EffectIR>,
  post: PredicateIR, result: ExprIR, contract: ContractIR,
  footprint: OperationFootprint
}
```

IR format version 1 SHALL use a restricted canonical JSON encoding for inspectable compiler artifacts. Object keys are unique ASCII field names, sorted lexicographically; there is no insignificant whitespace. Required fields are always present. Booleans/null use JSON literals; all integers, IDs and decimal coefficients are canonical decimal or fixed lowercase hexadecimal strings with type indicated by their containing node. Floats are forbidden. Signed zero is `0`, positive signs and redundant leading zeros are forbidden. Byte arrays are lowercase hex. Strings are valid UTF-8, with only quote/backslash and U+0000–U+001F escaped; control escapes use lowercase `\u00xx`. Invalid Unicode scalars are rejected.

Arrays preserving evaluation order remain ordered. Mathematical sets and maps encoded as arrays SHALL sort by each element's canonical bytes and reject duplicates. Records and declarations sort by stable ID; parameters retain declared order. Each node has a versioned `kind` tag and complete named fields. Unknown required tags/fields or missing required fields fail decoding; v1 has no skippable semantic extension fields.

Hashing uses `SHA-256(UTF8(domain) || 0x00 || canonical_bytes)`, with separate domains `astra.schema.v1`, `astra.operation.v1`, `astra.invariant.v1` and `astra.contract.v1`. Hash inputs exclude the hash field itself, source paths, source spans, timestamps and host-specific metadata. An operation hash includes its full contract and referenced type identities/versions. A schema hash includes all schema declarations and invariant versions; adding an operation also changes the separately emitted module hash `astra.module.v1` and requires reanalysis of compatibility.

Decoders SHALL reencode and compare bytes before accepting a canonical artifact. Compatibility is never inferred from equal human-readable names. Golden bytes and digests must be frozen before persistent compatibility is claimed. The ordered physical key codec remains separately versioned under SPEC-002 §11; JSON artifact encoding is not a B+Tree key codec.

## 11. Proof obligations exported to the compiler

```text
Obligation {
  id: ObligationId, kind: ObligationKind,
  subjects: Vec<SemanticRef>, assumptions: Vec<AssumptionRef>,
  formula: FormulaIR, evidence: ProofStatus
}
ProofStatus = Proven(rule_id, rule_version, premises)
            | Disproven(replayable_counterexample)
            | Unknown(reason)
```

The exported obligations SHALL cover:

| ID family | Obligation |
| --- | --- |
| IR-DEF | Defined, terminating typed evaluation, complete effects and captured inputs. |
| IR-SEQ | An accepted sequential transition preserves all invariants in its closure. |
| IR-CONC | Admitted concurrent accepted effects preserve invariants and accepted commitments. |
| IR-OBS | Reads, exact results, failures and session frontiers obey the operation contract. |
| IR-FOOT | Footprints cover reads, writes, predicates, aliases, facts and authority. |
| IR-ATOM | Atomic groups are indivisible at every observation that promises their invariant scope. |
| IR-COMP | Interacting operation versions and protocol plans compose under their joint closure. |

For sequential state predicates, the target includes `I(S) ∧ Pre(S,a) ∧ Defined(S,a) => I(S')`. This implication alone says nothing about concurrently accepted guards or returned results. A theorem/rule for operation-based replication MUST model accepted effect histories and duplicate identities. A state-merge theorem additionally needs a declared merge function and reachable common-ancestor states; no generic merge operator is assumed.

Tests and bounded model exploration produce evidence scoped to their bounds. They MUST NOT be labeled `Proven` for an unbounded formula. A counterexample includes argument values, state, accepted effects, observations and the failing invariant/contract; replay validates it before it can be labeled `Disproven`. Unknown never means safe.

## 12. Required fixture contracts

These examples are semantic fixtures; all omitted contract fields in a fixture file must be explicit when implemented.

| Fixture | Declarations and effects | Required interpretation |
| --- | --- | --- |
| `inventory_sell` | `Product.stock: I64`, `stock >= 0`; require `q > 0`; decrement by `q`; ensure `stock >= 0`; return receipt. | Distinct replicas seeing stock 1 cannot both confirm sale 1 without distinct conserved rights or coordination. Overflow of restock is also checked. |
| `inventory_reserve_release` | `available >= 0`, `reserved >= 0`, `available + reserved = total`; reserve/release use reservation R. | Repeating release R cannot restore stock/rights twice. A source of supply updates `total` atomically. |
| `account_transfer` | Balances nonnegative; conservation over the closed account domain; `source != destination`; `TransferQuantity(source,destination,q)`. | Both balances change in one logical transaction. Cross-shard physical application cannot expose a half-transfer to a promised domain snapshot. |
| `unique_username` | `UNIQUE User.username`; insert user with exact UTF-8 username. | Two inserts under different primary keys still share the unique-key absence predicate. Rename touches old and new keys atomically. |
| `causal_ship` | Immutable `PaymentConfirmed(order,payment)` fact; ship requires its committed identity; emit `ShipmentCreated`. | The matching fact must be in the accepted causal past and visible before shipment. This example has no payment-revocation operation. |
| `causal_refund_extension` | Add refund/cancel that may invalidate shipment's state predicate. | Recompute the whole operation closure; causal ordering alone is not a proof that revoke and ship may execute concurrently. |
| `increment_result` | Compare increment returning receipt with increment returning exact ordered post-value. | Equal effects do not establish equal observation obligations; class selection can differ. |

## 13. Errors and limits

Errors SHALL have a stable code, phase, source span or IR path, relevant identities and a bounded explanation. Codes include `ParseError`, `DuplicateIdentity`, `TypeMismatch`, `MissingRecord`, `NumericOverflow`, `InvalidDecimal`, `UnknownNormalizer`, `UnsupportedEffect`, `UnboundedFootprint`, `UnevaluableInvariant`, `InvalidContract`, `UnsupportedIrVersion`, `NonCanonicalEncoding`, `ResourceLimit` and `IdentityMismatch`.

An analysis `Unknown` is a structured result for SPEC-004, not a frontend type error. An unknown IR node is an error, not an analyzable opaque operation. Runtime definedness/precondition errors abort the entire candidate. Diagnostic counterexamples may contain user values only in explicitly requested fixture/debug output.

The compiler SHALL receive explicit resource limits for input bytes, declaration count, expression depth, set cardinality, expansion nodes and analysis work. Limits are versioned inputs to reproducibility tests. Exceeding a frontend/decoder bound rejects compilation; exceeding an analysis bound yields `Unknown` only when complete deterministic execution/validation remains available. No partial module or unchecked plan is publishable.

Initial limitations are finite typed records, closed operation sets per generation, no arbitrary trusted application code, no external side effects and no general automatic invariant inference. Range predicates may widen to entire scopes; this can reduce coordination savings. No theorem is claimed for the full language before its formal model and checker exist.

## 14. Acceptance criteria and milestones

| Acceptance ID | Required executable evidence |
| --- | --- |
| S003-A01 | The seven fixture families lower to expected typed IR or explicit documented rejection; source ordering and failures are preserved. |
| S003-A02 | Equivalent formatting/declaration insertion order with preserved IDs emits byte-identical IR and hashes on supported architectures. |
| S003-A03 | Distinct scale, operation version, invariant, contract or result semantics changes the appropriate digest. Malformed/unknown mandatory IR fails closed. |
| S003-A04 | Overflow, absent rows, duplicate primary keys and invalid enum transitions leave no business effects; decimal replay is exact. |
| S003-A05 | Footprint tests cover aliasing, unique absence, phantom insertion, aggregate group movement, bound-source changes and reference deletion. |
| S003-A06 | Sell/sell from stock 1 emits the accepted-effect counterexample; arbitrary set add/remove and revoked payment do not receive unsound C1/C2 evidence. |
| S003-A07 | Same request identity returns the stored exact result; different arguments are rejected; unknown outcome never creates a replacement request implicitly. |
| S003-A08 | Transfer and reserve/release reference evaluations preserve conservation and atomic groups, including all declared rejection cases. |
| S003-A09 | Fuzzed decoders and bounded input expansion cannot bypass type, footprint or resource checks. |
| S003-A10 | Receipt, causal snapshot and exact ordered result remain observably distinct in IR and interpreter histories. |

Milestone `IR0` delivers grammar, typed AST and a slow sequential interpreter (A01, A04, A08). `IR1` delivers canonical codecs and golden fixtures (A02, A03, A09). `IR2` delivers footprint/IDC templates and obligation export (A05, A06, A10). `IR3` integrates request/result bindings with SPEC-002 and runtime conformance (A07). These refine SPEC-001 M0/M1; IR3 depends on the relevant storage/runtime milestones and is not an implementation claim.

## 15. Traceability and open work

| Source | Requirement carried here |
| --- | --- |
| SPEC-001 §§7–14, 55–57, 80, 82 | Restricted records/operations; invariant/effect families; typed IR and MVP boundaries. |
| SPEC-001 §§15–21, 47, 58–60, 73 | Conservative dependency analysis; explicit proof status; cross-IDC composition; soundness over precision. |
| SPEC-001 §§26–29, 35–36, 41–43 | Exact identities; operation versions; observations; deterministic artifacts. |
| SPEC-002 §§7–11, 41–44, 69–71, 93–109 | Separate semantic/physical identity; atomic batches; deduplication; global uniqueness; protocol metadata. |
| [Proposal §§1–2 and direction A](../PROPOSTA-DE-PESQUISA.md#1-a-propriedade-fundamental) | Results, visibility, authority and durability are contract inputs; composition/evolution remain research obligations. |
| [Prior-art notes](../research/consistency-prior-art.md#arquitetura-mínima-para-testar-a-hipótese) | Complete operation contracts and observable commitments; no claimed novelty from a compiler flag. |

Open work before compatibility is declared: freeze parser/IR fixtures; define the separate ordered tuple-key codec; implement the interpreter; prove the supported normalization rules; establish the complete request-retention contract in SPEC-009; and qualify each analysis rule under SPEC-010. None of these may be replaced with an unverified runtime heuristic.
