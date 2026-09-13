# SPEC-004 — Coordination Compiler

**Status:** Draft 0.2 — proposed implementation contract; rule soundness and runtime qualification pending
**Date:** 2026-09-09
**Depends on:** [SPEC-001](SPEC-001.md), [SPEC-002](SPEC-002.md), [SPEC-003](SPEC-003.md)
**Execution targets:** [SPEC-005](SPEC-005.md), [SPEC-006](SPEC-006.md), [SPEC-007](SPEC-007.md), [SPEC-008](SPEC-008.md)
**Evolution and qualification:** [SPEC-009](SPEC-009.md), [SPEC-010](SPEC-010.md)
**Catalog, identity and security:** [SPEC-011](SPEC-011.md), [SPEC-012](SPEC-012.md), [SPEC-013](SPEC-013.md)
**Scope:** protocol derivation, semantic dependencies, candidate safety, plan certificates, fallback and executable plan obligations
**Reference implementation:** Rust stable; conceptual records are not a frozen ABI
**Normative terms:** MUST, MUST NOT, SHOULD and MAY express requirements.

## 1. Decision and authoritative interpretation

The compiler SHALL select from a finite, versioned library of qualified protocol templates. It may select a plan only after discharging the plan's full obligations for the closed operation set, invariants, observations, topology and failure model. Performance selects among safe candidates; it cannot establish safety.

This document specifies the compiler phases outlined in SPEC-001 §§15–29. SPEC-003 defines operation meaning and canonical IR; SPEC-002 defines the local durable boundary. Runtime SPECs define concrete protocol transitions. A compiler artifact alone does not establish that a runtime implements them correctly.

C0–C5 are **protocol families**, not a semantic total order or a claimed lattice. Selection never uses numeric `max(class)`. Escrow authority, causal visibility, exact result ordering and atomic publication are different obligations. A C3 plan may also require causal scheduling; a C5 plan still needs durability, invariant evaluation, fencing and a correct observation boundary.

The target is a low-coordination safe plan **within the supported template library and explicit assumptions**. This specification claims neither globally minimal coordination nor a complete decision procedure for arbitrary programs. General observable refinement across plans remains a research obligation identified by the proposal.

## 2. Inputs and outputs

```text
CompileInput {
  module_ir: ModuleIR,
  topology: TopologySnapshot,
  active_generation: CatalogGeneration,
  protocol_library: ProtocolLibraryManifest,
  analysis_rules: AnalysisRuleManifest,
  policy: CompilePolicy,
  analysis_budget: DeterministicBudget,
  prior_plans: Vec<PlanRef>
}
TopologySnapshot {
  topology_hash: Hash,
  placements: Vec<Placement>, authority_domains: Vec<AuthorityDomain>,
  durability_policies: Vec<FailureDomainPolicy>,
  node_capabilities: Vec<NodeCapability>
}
CompileOutput {
  plans: Vec<OperationPlan>, idc_templates: Vec<IdcTemplate>,
  interaction_graph: InteractionGraph,
  certificate: ConsistencyCertificate,
  diagnostics: Vec<Diagnostic>, migration_requirements: Vec<MigrationRequirement>
}
```

All versions, rules, capabilities and topology identities are explicit inputs. A placement snapshot is an assumption to validate at admission, not perpetual authority. A live catalog route change cannot silently change a previously hashed plan.

SPEC-011 owns the shared type taxonomy: `CatalogGeneration`, `PlanGeneration`, `IdcGeneration`, `IdcAuthorityEpoch`, `PlacementEpoch`, `MembershipGeneration`, `ResourceGeneration`, `EscrowEpoch`, `HolderAuthorityEpoch`, `StorageEpoch`, `OriginEpoch`, `RequestHomeEpoch`, `LocalCommitSeq` and `SerialPosition` are distinct scoped types. Equal integer values do not make them interchangeable. `IdcBinding { idc_id: IdcId, idc_generation: IdcGeneration, authority_epoch: IdcAuthorityEpoch }` binds a semantic definition and its separate authority incarnation. Untyped `(IdcId, u64)` tuples are forbidden. The compiler names required authority domains; runtime envelopes bind their active values. No cluster-global physical commit counter is introduced.

Compilation is side-effect free. It does not activate plans, grant rights, contact a remote payment provider or modify database records. Output is either a complete candidate artifact or typed failure. SPEC-011 registers immutable artifacts and owns publication/CAS, capability and authority admission; activation follows SPEC-009 and validates existing data, state/authority migration and node capabilities.

## 3. Execution profiles and their partial order

```text
ExecutionProfile {
  family: C0 | C1 | C2 | C3 | C4 | C5,
  admission: AdmissionProgram,
  visibility: VisibilityProgram,
  authority: AuthorityProgram,
  ordering: OrderingProgram,
  validation: ValidationProgram,
  atomicity: AtomicityProgram,
  durability: DurabilityProgram,
  replication: ReplicationProgram,
  result: ResultProgram,
  recovery: RecoveryContract
}
CandidatePlan {
  template_id: TemplateId, template_version: u32,
  profile: ExecutionProfile, obligations: Vec<Obligation>,
  compatibility_requirements: Vec<CompatibilityRequirement>,
  cost_descriptor: CostDescriptor
}
```

For a fixed contract and failure model, `P ⊑ Q` means a checked relation establishes that every observable history allowed by Q is permitted by P while retaining all mandatory commitments. This is a refinement relation, not a comparison of C labels. Restrictions on liveness/availability also require compatibility when the contract promises them.

Unknown comparison means incomparable for selection. The compiler SHALL NOT infer refinement from a larger label, a larger quorum, an added lock or a new leader. Independent requirements combine by explicit conjunction of their programs and discharged composition obligations. The label in SPEC-002 `CompiledBatch.consistency_class` is a primary family for dispatch/metrics; `plan_hash` binds the full profile. The label alone never authorizes execution.

`CompilePolicy` MAY prefer fewer remote participants, fewer coordination rounds or administrator-required serial semantics. It MUST NOT override a failed obligation. v1 selection uses a deterministic lexicographic cost descriptor supplied by each qualified template, with stable template ID as final tie-breaker. Estimates SHALL be labeled estimates; they are not measured latency or a proof of minimality. Runtime telemetry may guide a new candidate selection but does not expand the safe set.

## 4. Analysis pipeline

The compiler SHALL execute the following deterministic phases:

1. Validate canonical IR, operation identities, runtime evaluator support and complete observation contracts.
2. Add implicit type/representation, identity, index and internal-fact invariants to the declared invariant set.
3. Build conservative footprints and the invariant/operation hypergraph from SPEC-003.
4. Derive parameterized IDC templates and cross-IDC atomic interaction groups.
5. Compute sequential preservation and runtime-definedness obligations for each operation's complete closure.
6. Analyze ordered operation pairs, self-pairs and higher-arity obligations required by each template theorem.
7. Generate directed visibility/authority dependencies, exclusions and observation constraints.
8. Attempt supported algebraic and escrow rules; construct candidate protocol profiles.
9. Verify candidates jointly across the entire interacting operation set; reject unqualified mixed-protocol combinations.
10. Construct complete C5 fallback candidates where executable invariant validation and authority closure exist.
11. Select a deterministic safe candidate set; build routing, admission, validation, replication, result and recovery programs.
12. Emit canonical plans, assumptions, evidence, counterexamples, explanations and migration prerequisites.

The invariant closure is a fixed point: whenever an added invariant, result read, fact dependency or authority obligation exposes a new domain/operation, expand the analysis until no new dependencies occur. Unsupported dynamic footprints widen to a finite declared scope or fail. A compile budget timeout cannot truncate the closure and retain a weak plan.

Adding an operation, including maintenance, import, deletion, admin or repair operations, invalidates the relevant prior compatibility analysis. A closed operation set is part of the certificate. Any raw mutation entry point MUST use the same closure under a fully validated C5 plan; a bypass write is forbidden.

## 5. Proof status, evidence and trust

```text
AnalysisJudgment {
  obligation_id: ObligationId,
  subjects: Vec<SemanticRef>,
  status: Proven | Disproven | Unknown,
  rule_ref: Option<RuleRef>,
  premises: Vec<ObligationId>,
  evidence_ref: Option<EvidenceHash>,
  assumptions: Vec<AssumptionRef>
}
UnknownReason = UnsupportedFragment | UnsupportedComposition | SolverUnknown
              | BudgetExceeded | MissingAuthorityModel | MissingRuntimeCapability
```

`Proven` means a specific accepted rule/checker discharges the stated obligation under enumerated premises. The first implementation SHALL use small versioned rules with a reviewable mathematical statement, supported input fragment, runtime assumptions and test vectors. A claim such as “increments commute” without modeling overflow, guard acceptance and results is not a rule.

SMT is optional at compilation, never required on the transaction hot path. A solver result is usable only with a supported translation and its trust status disclosed. If a solver lacks independently checkable evidence, its exact version and translation become part of the trusted computing base. A returned `unknown`, missing evidence or analysis timeout cannot produce `Proven`.

Deterministic rules use instruction/work budgets, canonical traversal and stable seeds. Reproducible compilation with an external solver requires a pinned, replayable evidence bundle included by hash; wall-clock timing must not silently decide the published output. If evidence differs, the certificate must expose the difference rather than claiming byte-identical reproduction.

A counterexample SHALL replay against the SPEC-003 reference semantics before being labeled `Disproven`. Finite exploration that finds no violation remains `Unknown` outside its explicitly finite complete model. Qualification results and compiler-rule proof results are separate evidence categories; neither substitutes for the other.

## 6. Directed dependencies and conflict analysis

```text
InteractionEdge {
  predecessor: OperationRef, successor: OperationRef,
  key_relation: PredicateIR,
  kind: RequiresVisible | RequiresAuthority | InvalidatesGuard
      | RequiresDrain | ExcludesConcurrent | AtomicWith,
  invariant_refs: Vec<InvariantRef>, obligation_refs: Vec<ObligationId>
}
InteractionGraph { operations: Vec<OperationRef>, edges: Vec<InteractionEdge> }
```

For `RequiresVisible(A,B)`, an instance of B must include the matching committed A occurrence in its accepted causal past. It does not require B to wait for every A ever executed. The key/fact relation identifies which occurrence satisfies the prerequisite. The reverse edge is never inferred automatically.

`InvalidatesGuard(A,B)` means A can make a previously observed B admission predicate false. This is an analysis finding, not a complete protocol. A selected template must discharge it with a suitable dependency, mutual exclusion, preserved authority or revalidation rule. It may require coordinating both operations even though the semantic finding is directed.

`ExcludesConcurrent` denotes a verified requirement that a pair's acceptance windows cannot overlap in the relevant domain; both endpoints participate in the chosen control rule. `RequiresDrain(A,B)` means B needs evidence that all covered A producers are closed and their accepted effects reconciled before B proceeds. A causal token for one A occurrence cannot prove a complete drain.

Example: `PaymentConfirmed(order) -> Ship(order)` can use a matching immutable fact dependency. Adding `Refund(order)` that invalidates a state predicate introduces a new interaction. Sorting refund after one observed payment does not prevent a concurrent shipment. The compiler MUST reanalyze the contract and either preserve the commitment using an explicit transition rule or coordinate the interfering operations.

Syntactic read/write overlap does not prove semantic conflict, and different physical rows do not prove independence. Analysis SHALL include self-concurrency, phantom membership and absence checks, aggregate bounds, invariant bound updates and returned values. Pairwise checks alone are accepted only under a rule whose premises establish safety for arbitrary permitted histories; higher-arity invariants require their own rule or a conservative plan.

## 7. Candidate obligations by protocol family

| Family | Necessary obligations, in addition to the common contract |
| --- | --- |
| C0 LOCAL | Full affected invariant/observation scope has one active fenced local writer; no remote prerequisite is omitted; local execution serializes or validates interfering transitions; failover preserves state, request outcomes and authority. |
| C1 COMMUTATIVE | Accepted normalized effects have proven reorder/duplicate semantics, preserve the reachable-state invariant closure and results, and converge under the exact replication handler. Stable identity/deduplication and representability are proved. |
| C2 CAUSAL | C1-like concurrent safety for unordered effects, or an independently verified causal template; complete matching prerequisites and session dependencies; downward-closed visibility; durable frontier/effect publication. Causality alone is not mutual exclusion. |
| C3 ESCROW | Invariant admits a conserved rights decomposition; all consumers/producers/invalidators are covered; local spending is atomic with effects; unique authority, transfer and epoch recovery are qualified; observation/causal constraints remain enforced. |
| C4 CERTIFIED | Complete conflict/predicate/authority set; deterministic validation at a shared decision boundary; successful overlapping validations cannot jointly violate invariants; atomic prepare/decision/publication; exact result and recovery binding. |
| C5 SERIAL | Complete affected scope and interacting writer set; durable single ordered authority; invariant/guard validation on the authorized serial state; fenced alternatives; atomic composite execution and valid read/result publication. |

Every family also requires the contract's durability boundary, stable request-to-transaction identity, exact terminal result persistence and a qualified recovery path. Asynchronous replication does not automatically meet `ReplicatedStable`.

C2 v1 requires that the contract's session scope and every required causal prerequisite resolve to one replication group. Cross-group `ReadYourWrites`, `MonotonicReads` or `CausalDependencies` require a separately qualified composite plan; no such C2 extension is enabled in the initial profile. Report `UnsupportedSessionScope` before effects if the supported library cannot meet it. A C5 label alone is not evidence that a foreign causal token is understood.

C3 candidate profiles bind SPEC-006's `AuthorityDurabilityPolicy` and `TransferDecisionDurabilityPolicy` separately from client-result durability. Their failure-domain and recovery assumptions are included in the plan hash, compatibility checks, cost descriptor and EXPLAIN. Local-stable client outcomes do not authorize discarding replicated rights decisions or reclaiming rights after permanent evidence loss.

### 7.1 C0 is a locality claim

“The record is on this node” is not proof of C0. Another node may still own rights, admit writes or return a conflicting final result. C0 requires a versioned exclusive authority assumption and admission fencing over every competing writer. Local snapshots alone do not stop write skew across two local rows. A C0 template MUST provide local serialization or complete validation for its invariant scope, even though it introduces no remote coordination during normal execution.

### 7.2 Commutativity is not complete safety

For effects `a` and `b`, equal final state `a(b(S)) = b(a(S))` does not justify both origin-side guards or result values. Selling the last unit twice gives commuting decrements but invalid stock and two incompatible promises. `increment_and_get` can produce incompatible exact results even if an increment-only receipt is safe. Finite numeric overflow and user-visible failures also form part of the proof.

Operation-based templates reason about already accepted effects with stable identities. State-based merge is absent unless a template names a merge operator and proves its laws and reachable-state invariant closure. The compiler cannot substitute a last-writer-wins value for an accepted decrement or reservation.

## 8. Escrow synthesis fragment

The initial analyzer SHALL recognize exact affine one-resource bounds:

```text
x >= L              capacity = x - L
x <= U              capacity = U - x
SUM(x WHERE group=g) >= L(g)
SUM(x WHERE group=g) <= U(g)
```

The resource descriptor contains the invariant ID/version, grouping key, quantity type/scale, stable bound-source identity, producers, consumers, transfer effects, implicit representation limits and authority domain. The compiler derives the sign and amount of each capacity delta. An unknown delta, mutable untracked bound or unknown aggregate membership disables this synthesis rule.

An escrow template SHALL model each available unit as located exactly once among usable authority and non-usable in-flight/reserved authority; committed consumption removes the unit, and legitimate committed production creates it exactly once. The concrete accounting representation and transfer proof belong to SPEC-006. A casual arithmetic sum of several locally observed stock values is not a rights ledger.

All resource-changing operations participate. Bound reduction, reset, deletion and changing a group key can revoke capacity and therefore require a rights reconciliation/coordination path. Restock may produce lower-bound rights only after the underlying supply effect is durably accepted; replay cannot reproduce rights. A representation upper bound can require another authority constraint and prevent a simple C3 plan.

Multi-resource reserve/transfer is not solved by independently spending each resource. The compiler emits a whole-invocation atomic program or falls back to composite C5. Rights availability can satisfy admission for a receipt contract; it cannot establish an exact current global balance return. That observation requires additional verified coordination.

If local rights are insufficient, allowable actions are waiting, a qualified rights acquisition protocol or `AuthorityUnavailable` according to the contract. The compiler MUST NOT label this state `OutOfStock` without a complete authorized observation. C5 fallback cannot reclaim unreachable holders' rights merely by choosing a larger C label.

## 9. Certification synthesis fragment

```text
CertificationProgram {
  program_version: u32,
  scope: ScopeExpr,
  point_reads: Vec<ExpectedPointVersion>,
  predicate_reads: Vec<ExpectedPredicateToken>,
  authority_checks: Vec<AuthorityCheck>,
  invariant_checks: Vec<InvariantEvaluatorRef>,
  observation_checks: Vec<ObservationCheck>,
  decision_domain: AuthorityDomainId,
  reservations: Vec<ConflictReservation>
}
```

C4 SHALL be emitted only when the runtime supports every predicate token and reservation required by the full footprint. Point version equality alone does not certify uniqueness, absence, range sums or referential existence under concurrent deletion. Changes to predicate membership and bound-source values must invalidate or otherwise be covered by the certification rule.

The generated program evaluates candidate effects and invariant deltas relative to an authorized validation state. Two validators independently seeing a valid pre-state do not constitute a shared decision boundary. The template must ensure that accepted reservations/decisions have a compatible serialization or proven invariant-specific equivalent across all participants and other protocol families.

If SPEC-007 has not passed qualification, C4 is unavailable even when its analysis shape looks suitable. The compiler reports `MissingRuntimeCapability` and considers another safe candidate. It does not emit a placeholder success path. SPEC-001 permits deferring C4 until after the initial C0/C1/C2/C3/C5 prototype.

## 10. Conservative C5 fallback

The compiler SHALL construct C5 only when all of the following are available:

1. A finite or conservatively widened complete invariant and observation scope.
2. A deterministic, terminating evaluator for every affected invariant, guard, effect and result.
3. A qualified ordered authority covering all interacting writers and required result reads.
4. A checked admission/fencing path for all older or concurrent weak-plan emitters.
5. Atomic local or composite prepare/decision/publication with the required durable witnesses.
6. Explicit rejection/wait/unavailable behavior permitted by the contract.

At execution, C5 evaluates against the authoritative serial state at its assigned position; it checks preconditions, checked arithmetic, postconditions and all affected invariants before accepting. A whole-IDC scan is allowed for an unsupported optimization if complete and bounded by explicit runtime policy. Exceeding the budget returns a non-success outcome; it cannot skip the remaining checks.

An unsupported *analysis* can therefore route to C5. An unevaluable invariant, incomplete effect footprint, unsupported observation promise or unachievable authority transition yields `NoSafePlan`. Serializing an invariant-violating transition does not make it valid. If the declared contract requires an impossible success, compilation fails; runtime rejection is allowed only where the contract permits it and the rejection reason is explicit.

C5 SHALL preserve all already-issued commitments of other families. Before replacing C3/C1 writers it must drain/fence them and reconcile effects/rights under SPEC-009. Bumping a catalog or IDC epoch is insufficient if an offline holder can still legally confirm old requests. Where the supported fault/authority model cannot retire that holder, fallback remains unavailable for the affected scope.

No fallback is permitted to reinterpret previously accepted effects, discard accepted requests, erase a session dependency, reduce promised durability or invent an outcome for `IN_DOUBT` prepared work.

## 11. Joint selection and composition

The unit of compilation is the affected closed module/generation, not one isolated operation. For each potential interacting pair of selected plans, the certificate SHALL reference a supported compatibility rule. This includes the same operation/version against itself and old/new versions permitted during a drain window.

```text
CompatibilityRequirement {
  left: OperationPlanRef, right: OperationPlanRef,
  interaction_scope: ScopeExpr,
  rule: CompatibilityRuleRef,
  shared_authority: Vec<AuthorityDomainId>,
  dependencies: Vec<InteractionEdge>,
  assumptions: Vec<AssumptionRef>
}
```

A compatibility rule specifies effect admission, observation order, authority overlap, identity/deduplication, atomic publication and recovery behavior. Shared physical storage with separate C1 and C5 writers does not imply compatibility. A C5 operation on an aggregate must account for still-authorized C3 consumers of that aggregate.

The initial selection algorithm SHALL start with qualified conservative plans for each semantic closure. It then visits proposed lower-cost replacements in canonical `(IDC template ID, operation ID, template ID)` order. A replacement is accepted only if all common, template and interaction obligations remain proven. Repeat to a deterministic fixed point within a fixed iteration bound. This is a deterministic safe heuristic, not a global optimum.

If individual candidates are safe but their mixture is unsupported, retain or construct a shared conservative plan for the entire interacting closure. If even that closure has no admissible C5 implementation, compilation returns `NoSafePlan`. An operation's family MUST NOT be chosen by taking the numeric maximum of families derived independently for each invariant.

### 11.1 Multi-IDC atomicity

Whole-invocation atomicity is the SPEC-003 baseline. Parallel execution is allowed only for internal work whose publication/result remains one transaction, or for explicitly separate user invocations. A compiler must not silently split a transfer into two independent commits.

For a multi-IDC operation, the plan lists all participants, authority/reservation requirements, global decision identity and publication constraints. v1 uses qualified composite C5; composite C4 may be enabled after its qualification. Participants use SPEC-002 prepare/commit/abort with durable decisions; readers promising the combined invariant scope obtain an admissible cut and cannot expose one committed half while the other remains unresolved. Independent local MVCC snapshots are not a distributed atomic snapshot.

If dynamic arguments expand the participant set beyond the compiled selector, execution rejects or recompiles before any effect. Late discovery cannot extend an already committed prefix. Deadlock avoidance order is a deterministic order of concrete IDC identities; this implementation order is separate from semantic causal dependencies.

## 12. Executable operation plan

```text
OperationPlan {
  plan_format_version: u32,
  plan_id: PlanId,
  operation: OperationRef,
  operation_hash: Hash, schema_hash: Hash, contract_hash: Hash,
  invariant_set_hash: Hash, module_hash: Hash,
  catalog_generation: CatalogGeneration,
  plan_generation: PlanGeneration,
  idc_templates: Vec<IdcTemplateRef>,
  profile: ExecutionProfile,
  routing: RoutingProgram,
  assumptions: Vec<Assumption>,
  fallback_plan_refs: Vec<PlanRef>,
  migration_requirements: Vec<MigrationRequirement>,
  protocol_capabilities: Vec<ProtocolCapabilityRef>
}
Assumption {
  id: AssumptionId, predicate: AssumptionPredicate,
  enforcement: CompileTimeEvidence | AdmissionCheck | DurableFence | ProtocolInvariant,
  failure_action: Reject | Wait | QuiesceAndTransition(PlanRef)
}
```

There are no unchecked free-text assumptions. Every safety assumption needs an enforceable predicate or accepted proof/model reference. Examples include the exclusive writer epoch, active rights ownership, accepted plan generation, storage readiness, capability version, durable witness policy and required causal frontier.

`RoutingProgram` computes concrete keys, IDC instances and minimum participant sets from canonical arguments and the versioned placement policy. It does not authorize a participant simply because it has a replica. At admission, runtime checks active catalog/IDC generations, plan/schema/contract hashes, operation version, authority and node readiness before persistence.

Execution SHALL bind SPEC-012's original `RequestKey`, immutable `RequestHash`, mapped `TxnId`, canonical arguments and all required captured inputs. Routing to RequestHome precedes transaction allocation; execution resumes the admitted binding rather than deriving a new transaction from the current plan. The result program creates `FinalReceiptV1` with the exact result and frontier/authority/durability evidence. SPEC-002 durably binds the result payload to the decision and business/protocol changes; SPEC-008 completion evidence, where required, gates client finality. A local install ACK alone is not a final client receipt.

Physical lowering MUST apply deltas under the chosen semantic handler and the storage atomic mutation boundary. Producing `Put(snapshot_x + delta)` independently on two writers is not correct C1 merely because the source IR had `Increment`. The generated plan must name the handler and its concurrency/deduplication obligations. Local commit order is neither an IDC serial order nor a causal frontier.

## 13. Certificate, digests and EXPLAIN

```text
ConsistencyCertificate {
  certificate_version: u32,
  compiler_build_hash: Hash,
  input_hashes: CompileInputHashes,
  rule_manifest_hash: Hash,
  protocol_library_hash: Hash,
  plan_refs: Vec<PlanHash>,
  judgments: Vec<AnalysisJudgment>,
  rejected_candidates: Vec<CandidateRejection>,
  compatibility_rules: Vec<CompatibilityRequirement>,
  assumptions: Vec<Assumption>,
  evidence_manifest: Vec<EvidenceRef>
}
```

Plans and certificates SHALL use SPEC-003's canonical artifact encoding with their own format versions and hash domains `astra.plan.v1` and `astra.certificate.v1`. Hash the complete plan payload without its own digest or certificate digest. The certificate refers to computed plan hashes and is then hashed separately, avoiding a circular digest. Human explanations and local file paths are excluded from semantic plan payloads.

The artifact checker validates canonical bytes, identities, hashes, recognized rule/template versions, complete obligation coverage and reference integrity. It MUST distinguish “manifest structurally valid” from “proof obligations checked” and “runtime qualified.” A hash proves byte identity, not theorem correctness or authenticity. Plan signatures are a separate catalog/security mechanism.

`carolina explain operation <name>` SHALL display selected family and all added requirements; affected invariants/IDCs; input visibility and returned-result semantics; each accepted/rejected/unknown candidate; authority and durability assumptions; partition behavior; fallback prerequisites; and unqualified runtime features. `carolina compile`, `carolina check`, `carolina plan` and `carolina graph invariants` are deterministic artifact operations, not automatic activation.

Example output requirements for a sell receipt:

```text
Operation: sell@1
Invariant scope: Product[id].stock >= 0 plus representation limits
C1 rejected: concurrent accepted decrements can exceed capacity
C2 rejected: causal prerequisites do not allocate exclusive spending rights
C3 eligible: exact resource decomposition; rights and effect commit atomically
Result: receipt for this accepted sale; no current-global-stock promise
Availability: may proceed with valid sufficient local rights
On missing rights: acquire rights / wait / AuthorityUnavailable per contract
C5 alternative: only after all interfering authority is fenced/reconciled
Evidence: list rule, runtime qualification and assumption references
```

If any required rule or runtime qualification is missing, EXPLAIN must say “candidate,” not “safe active plan.” The sample is not evidence that the rule has already been implemented.

## 14. Plan evolution and runtime assumption failure

Runtime specialization within a proven family may change placement or rights allocation only through the template's qualified state transition. A safety-relevant placement change is not an in-memory pointer update. Telemetry cannot authorize weaker execution.

For any advertised fallback, the compiler emits `MigrationRequirement` records identifying admission barriers, old operation producers, outstanding rights/transfers, prepared work, request outcomes, causal frontiers, exact-result obligations and new authority installation. SPEC-009 owns their durable state machine and observable refinement rules.

A safe transition generally requires freezing affected admission, closing/draining or fencing **all** old emitters, resolving/reconciling their committed and in-doubt work, validating destination state, installing new authority and publishing the new generation. Compatible dual execution is allowed only under an explicit checked compatibility rule. A temporarily stronger-looking protocol is not permission to bypass this process.

At runtime, failed assumptions produce the specified typed non-success state. The runtime MAY choose a precompiled fallback only after its admission and transition prerequisites hold. Otherwise it waits or refuses service. It MUST NOT relabel the same unsafe request from C3 to C5 and execute it immediately.

## 15. Required derivation scenarios

| Scenario | Required compiler outcome |
| --- | --- |
| Local profile assignment under proven exclusive ownership | C0 may be eligible with local invariant validation and failover fencing; mere co-location is insufficient. |
| Grow-only internal facts with receipt results | C1 may be eligible after set/fact algebra, identity, full operation closure and runtime proof obligations pass. |
| Stock 1, two concurrent sale-1 receipts | Reject naive C1/C2 with a replayable counterexample; consider conserved C3 rights or complete C5. Never promise two sales. |
| Restock plus sale on finite I64 stock | Check both business lower bound and representational upper bound; unconstrained commutative increment is not automatically safe. |
| Inventory reserve/release with reservation identity | Include reservation lifecycle, stock conservation, replay and rights-production obligations; duplicate release cannot create capacity. |
| Account transfer across two IDCs | Preserve source bound and sum conservation in one composite transaction; reject independent commit plans and fractured promised snapshots. |
| Register/rename unique username | Include canonical unique-key absence/membership and every registering/renaming/deleting operation. Initial fallback uses ordered authority over the namespace; a local unique index is insufficient. |
| Payment fact then shipment | C2 may be eligible for the matching immutable committed fact; do not add reverse dependencies or global ordering of unrelated orders. |
| Refund/cancel added to shipment contract | Recompute closure and expose new invalidation/exclusion obligations. Retaining the old causal-only certificate is forbidden. |
| Exact post-balance result added to receipt operation | Recompute observation obligations; a state-preserving weak plan cannot inherit the stronger result promise. |
| Analyzer cannot prove a deterministic custom predicate | C5 evaluates the complete predicate at the serialized acceptance boundary, or compilation fails if no complete evaluator/scope exists. |
| C3 to C5 while an old rights holder is disconnected | Refuse activation until a sound retire/drain/rights treatment satisfies SPEC-009; a catalog epoch increment alone is insufficient. |

## 16. Error and resource behavior

Compile errors include `InvalidIr`, `IncompleteContract`, `InvalidSequentialContract`, `UnenforceableInvariant`, `IncompleteFootprint`, `UnsupportedComposition`, `UnsatisfiableDurability`, `UnmetObservationContract`, `MissingRuntimeCapability`, `InvalidEvidence`, `AnalysisLimit` and `NoSafePlan`. Each includes affected operations/invariants, failed obligations and an explanation of any available conservative alternative.

Candidate rejection and `Unknown` are recorded even when compilation succeeds through another candidate. Input parse/type failures do not produce fallback plans. A solver or rule budget failure may use a independently safe C5 candidate, but cannot reuse a partial dependency closure. Limits on graph nodes, generated candidates, symbolic expansions, composite participants and certificate size are explicit versioned inputs and bounded during every phase.

Relevant runtime failures include `StalePlan`, `StaleSchema`, `StaleEpoch`, `IdentityMismatch`, `AuthorityUnavailable`, `MissingDependency`, `Conflict`, `InvariantRejected`, `NotReady`, `TxnInDoubt` and `UnknownOutcome`. Runtime SPECs define wire-level mappings. No success may follow a failed durable barrier. A result that may already be committed is resolved by stable identity rather than blindly retried as a new request.

## 17. Acceptance criteria and milestones

| Acceptance ID | Required executable evidence |
| --- | --- |
| S004-A01 | Equivalent canonical inputs and fixed rule/evidence manifests produce identical plans/certificates; no host-order or wall-clock selection differences. |
| S004-A02 | Every selected profile has complete discharged common/template/composition obligations; deleting a required premise makes checking fail. |
| S004-A03 | Sell/sell stock-1 counterexample rejects C1/C2; finite-width overflow is also included in safety analysis. |
| S004-A04 | Co-located rows with a competing remote writer do not qualify for C0; local write skew requires local validation/order. |
| S004-A05 | Asymmetric payment-to-shipment dependencies retain their direction and instance matching; refund extension invalidates the prior certificate. |
| S004-A06 | Global uniqueness, range membership and aggregate bound updates cannot pass a point-read-only certification program. |
| S004-A07 | A safe isolated C1/C3 plan plus unsafe C5 interaction cannot be accepted by numeric maximum or independent plan selection. |
| S004-A08 | Unsupported/timeout analysis uses a fully validated C5 path or returns `NoSafePlan`; unevaluable predicates never become automatic success. |
| S004-A09 | Cross-IDC transfer has one durable decision and valid combined observation; all generated partial-commit plans are rejected. |
| S004-A10 | Receipt and exact ordered result contracts yield distinct obligations; lack of rights cannot return final global `OutOfStock`. |
| S004-A11 | C3-to-C5 transition cannot activate with unretired spending authority; stale plans and missing assumptions fail admission. |
| S004-A12 | Plan/certificate corruption, unknown mandatory rule versions and forged/incomplete evidence fail checking; the hash/checker distinction is exposed. |
| S004-A13 | Golden EXPLAIN fixtures show candidate rejections, unknowns, scope, observations, durability and migration prerequisites accurately. |
| S004-A14 | Deterministic simulation exercises the generated mixed-family plans with crash, duplicate, delayed and partitioned histories under SPEC-010 and checks exact final outcomes. |
| S004-A15 | Cross-group sessions and foreign prerequisite tokens select a qualified explicit composite plan or `UnsupportedSessionScope`; no numeric-family fallback erases dependencies. |
| S004-A16 | Equal numeric IDC definition/authority generations cannot be substituted; catalog/capability and independent C3 durability changes invalidate affected admission assumptions. |

Milestone `CC0` implements pure input validation, closure and rule/certificate models (A01, A02, A12). `CC1` implements conservative C0/C5 templates and the reference validator (A04, A08, A09). `CC2` adds qualified C1/C2/C3 derivation, directed dependencies and joint selection (A03, A05, A07, A10). `CC3` enables C4 only after SPEC-007 qualification (A06). `CC4` integrates explanations, evolution obligations and adversarial qualification (A11, A13, A14). These refine SPEC-001 M1/M7/M8/M9; completion requires evidence, not the presence of these documents.

## 18. Traceability and unresolved research

| Source | Requirement carried here |
| --- | --- |
| SPEC-001 §§2, 15–29, 58–60, 73 | Conservative pipeline, incomparable families, directed dependencies, explicit evidence and soundness over precision. |
| SPEC-001 §§35–47, 49–53 | Observations, identity, protocol composition, catalog generations, failover and no LLM in the correctness path. |
| SPEC-001 §§80–92 | Restricted initial prototype; C4 deferral; mixed-family and migration milestones. |
| SPEC-002 §§38–44, 53–71, 93–109, 123–125 | Compiler owns semantic conflicts; storage owns atomic durable batches/prepares; exact identity and protocol metadata survive recovery. |
| SPEC-003 §§5–11 | Typed deterministic contracts, complete footprints, semantic effects, observations and proof obligations. |
| [Proposal §§1–2 and direction A](../PROPOSTA-DE-PESQUISA.md#1-a-propriedade-fundamental) | Preserve observable commitments across composition and evolution; choose among verified plans rather than claiming a unique weakest consistency. |
| [Prior-art notes](../research/consistency-prior-art.md#hipótese-que-ainda-merece-um-experimento) | Prior art prevents novelty claims from generic synthesis; negative cases and falsification remain part of evaluation. |

Open obligations include proofs for each supported effect/history rule, useful completeness of dependency extraction, safe mixed-protocol bridges, independent evidence checking, the cost of exact observations and the observable-refinement construction of SPEC-009. Until a rule or bridge is qualified, the relevant candidate stays unavailable. This restriction is intentional: safe refusal is an implementable outcome; an invented proof is not.
