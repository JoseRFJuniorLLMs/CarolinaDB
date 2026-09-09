# SPEC-010 — Qualification

**Status:** Draft qualification specification; no test or benchmark results are claimed.  
**Date:** 2026-09-09  
**Depends on:** [SPEC-001](SPEC-001.md)–[SPEC-009](SPEC-009.md).  
**Primary requirements:** SPEC-001 §§64–75 and 80–92; SPEC-002 §§127–179.  
**Research requirements:** [consistency-prior-art](../research/consistency-prior-art.md), experiments E1–E5, and [PROPOSTA-DE-PESQUISA](../PROPOSTA-DE-PESQUISA.md) §1.

## 1. Purpose

Qualification determines which contracts, protocol combinations, failure schedules, storage formats, and migrations a particular build has evidence to support. A clean final database snapshot is insufficient: a system can restore its counters while invalidating earlier successful reservations.

This specification defines a deterministic simulator, independent reference model, real-process fault campaigns, performance methodology, release gates, and reproducible evidence artifacts. MUST/SHALL requirements apply to the proposed implementation and its qualification runner. The repository currently contains design documents; this SPEC does not turn planned checks into passed checks.

There are three independent claims to evaluate:

1. **Correctness:** admitted executions meet their complete observable contracts under the specified failures.
2. **Usefulness:** the compiler accepts useful operations and permits useful progress, without offloading protocol design to the developer.
3. **Benefit:** reduced coordination outweighs analysis, metadata, transfer, recovery, and migration costs at equivalent guarantees.

Any counterexample to a supported correctness claim blocks that feature's qualification. Inconclusive checking is not a pass. A correct but slower prototype is still a valid measured result.

## 2. Capability manifest and verdicts

Every campaign consumes an immutable manifest:

```text
QualificationManifest {
  manifest_version, source_revision, working_tree_digest
  build_profile, toolchain_id, dependency_lock_digest
  storage_format_versions, compiler_rule_versions, protocol_versions
  supported_contract_fragment, enabled_features
  operation_definitions, schema_hashes, plan_hashes
  durability_profiles, failure_assumptions
  topology, authority_configuration, placement
  resource_limits, timeout_and_retry_policy
  seed_set, schedules, checker_versions, run_budget
  workload_definitions, load_sweep, baseline_configs
}
```

Record dirty source state explicitly. A commit hash alone does not identify a modified working tree. Secret credentials and production data MUST NOT be embedded in artifacts; use synthetic, reproducible data.

Each result has one of these statuses:

| Status | Meaning |
|---|---|
| `PASS` | Named checks completed and found no violation within the recorded scope |
| `FAIL` | A concrete violating history, unsafe acceptance, or reproducible implementation defect |
| `INCONCLUSIVE` | Checker/resource limit, unsupported observation, lost evidence, or ambiguous harness result |
| `NOT_RUN` | Required check has not executed |
| `NOT_APPLICABLE` | Capability intentionally disabled; justification recorded |

An omitted test is `NOT_RUN`, never an implied pass. `PASS` is evidence for the explored state space, not an unrestricted mathematical proof. A feature cannot be enabled for qualification by labelling its mandatory checks `NOT_APPLICABLE`.

## 3. Layered test architecture

```text
typed contracts / workload histories
          |
  independent reference interpreter + history oracle
          |
  deterministic simulator (runtime/protocol adapters)
          |
  local crash runner (real storage kernel)
          |
  real-process distributed fault runner
          |
  reproducible benchmark runner
```

The independent interpreter MUST NOT import the compiler's classification logic or the optimized runtime's merge implementation as its oracle. Shared canonical data types/codecs are acceptable only with separate golden decoding fixtures. Model semantic transitions independently and compare committed effects, exact results, and observable histories.

Run tests appropriate to each implemented milestone. C4 qualification follows a functioning C1/C2/C3/C5 prototype; it is not a prerequisite for starting the formal core. Baseline protocol evolution must be checked before any optimized overlapping migration.

## 4. Reference state and observable history

The slow reference model contains:

```text
ReferenceState {
  logical_records
  operation_contracts_by_version
  committed_requests_and_exact_results
  pending_requests_and_durable_decisions
  causal_relation_and_frontiers
  active_and_closed_authorities
  capacity_ledger: total, consumed, held, usable, in_transit
  prepared_transactions_and_publication_state
  migration_state_and_token_mappings
}
```

The model may explore multiple permitted executions when the contract allows concurrency-dependent results. Comparing one arbitrary sequential order to a causal or commutative contract can produce false failures. Conversely, accepting any final state that satisfies the invariant can miss illegal results.

History events include `Invoke`, `AdmissionRefusal`, `FinalReply`, `UnknownReply`, `ReadReply`, `LocalDurable`, `Prepare`, `Decision`, `Publish`, `Apply`, `AuthorityClose`, `MigrationActivate`, `Crash`, `Restart`, `Partition`, `Heal`, and `StorageFault`. Each records relevant IDs, plan/contract hashes, logical clock/scheduler step, argument/result digest, and evidence references. Preserve exact synthetic arguments/results in replay bundles.

Client histories capture invocation and response intervals with monotonic client-local clocks. Across clients, infer real-time relations only from measured intervals with stated clock assumptions or explicit synchronization. The deterministic simulator has an exact total event schedule. Do not pretend raw wall-clock timestamps from independent hosts prove a total real-time order.

## 5. History checker rules

For each execution, the checker SHALL verify:

| ID | Property |
|---|---|
| Q-C01 | Every final success has its required durable decision/effects and remains resolvable after supported recovery |
| Q-C02 | No committed effect exists without a valid durable decision and matching authority |
| Q-C03 | Stable request retries produce one logical effect and the same retained final result; mismatched arguments cannot execute |
| Q-C04 | Each registered operation's preconditions, postconditions, exact result semantics and allowed rejection conditions hold |
| Q-C05 | Every reachable committed logical state respects applicable invariants; prepared/staged data is excluded from committed visibility |
| Q-C06 | Causal observations include required predecessors; session tokens enforce read-your-writes and monotonic reads |
| Q-C07 | C1 merge converges for equivalent delivered operation sets and preserves both invariants and permitted results |
| Q-C08 | Escrow accounting holds for each resource, with no duplicated usable authority across replicas or epochs |
| Q-C09 | Certified schedules validate all required keys, absent keys, ranges, predicates and relevant interfering writers |
| Q-C10 | C5 histories meet the ordering/read contract within the specified IDC scope; no global linearizability is inferred |
| Q-C11 | Multi-IDC finality and atomic reads never expose a contract-forbidden partial transfer |
| Q-C12 | Migration histories preserve old commitments and never enable incompatible overlapping authority |
| Q-C13 | Recovery, duplicate delivery, GC and stale-node bootstrap cannot resurrect deleted state or retired authority |
| Q-C14 | Unknown/timeouts are not mistaken for aborts or a license to execute a new request |

The checker uses the operation's declared observation model. Apply linearizability checking only to scopes that promise it. Local MVCC snapshots, causal reads, exact global return values, and cross-IDC atomic snapshots are separate claims.

Pending/unknown operations may be completed or left incomplete according to valid histories and known durable evidence. An observed durable commit cannot be removed merely to make the checker find a legal history. A final rejection cannot be treated as pending.

If the checker cannot determine legality within its budget, emit `INCONCLUSIVE` with the explored bound. Retrying with a larger checker budget is a separate recorded run.

## 6. Deterministic simulation contract

All simulated nondeterminism enters through an explicit environment:

```text
SimulationEnvironment {
  seeded_rng
  logical_scheduler
  network_queue
  volatile_memory_per_node
  durable_storage_model_per_node
  clock_source_per_node
  fault_schedule
}
```

Runtime adapters MUST NOT read OS time, random generators, files, sockets, or background threads outside that environment. Scheduling decisions are traceable and replayable.

Network events model duplication, reordering, loss, delay, and directional partitions. Storage models distinguish append-to-process-buffer, write-to-OS, durable barrier, page flush, and torn/short writes. A simulated restart discards volatile state and reconstructs from modeled stable bytes. A fake `fsync` that immediately persists every write cannot test durability boundaries faithfully.

The same seed, manifest, scheduler decisions, and starting bytes must reproduce the same trace digest. If a runtime adapter cannot meet deterministic replay, label its run a real-process stress test, not deterministic simulation.

Explore at least:

- bounded exhaustive interleavings for the smallest state machines;
- directed schedules for every protocol's acceptance table;
- seeded randomized histories with crash/restart and partitions;
- schedules that mix two protocol families, then the supported full application;
- migrations concurrent with retries, GC, preparation, delivery gaps and authority transfers.

Publish the number of nodes, operations, states, scheduling steps, values, seeds, and fairness assumptions. Finite exploration does not establish unbounded liveness.

## 7. Storage qualification

Use SPEC-002's independent `BTreeMap<LogicalKey, Vec<Version>>` model for local reads, snapshots, tombstones and version retention. Semantic protocol correctness uses the separate model above.

The crash runner MUST inject at every supported durable boundary from SPEC-002 §132. Required coverage includes journal append fragments, before/after sync, after sync before visibility/ACK, page splits, page flush/torn page, manifest A/B writes and publication, checkpoint stages, PrepareBatch, and CommitPrepared before installation.

| SPEC-002 property | Required evidence |
|---|---|
| P1/P2 | ACK recovery and no commit without durable decision at every journal boundary |
| P3 | Business writes, protocol metadata, dedupe and results recovered atomically |
| P4/P5 | Stable registered snapshots and transaction-local read-your-writes |
| P6 | Repeat redo/reopen yields identical logical digest |
| P7 | Random ordered key/version insertion and splits agree with reference scans |
| P8 | Stable pages never depend on non-durable journal records |
| P9 | In-doubt prepared writes stay invisible until a valid decision |
| P10 | Old storage/protocol epochs never restore current semantic authority |

Separate process-kill tests from machine power-loss or modeled torn-write tests. Killing a process does not by itself flush or erase the host OS page cache. Reports state exactly which failure mechanism was exercised.

Also inject disk full, read/write/sync errors, checksum corruption, unknown mandatory formats, page/root corruption, incomplete journal tail and required mid-log corruption. Recovery may discard a proven incomplete tail; it must not silently truncate a required corrupted commit.

Every recovered database runs the read-only storage and journal verifiers. For pages/checkpoints, verify bounds, reachability, root and allocator consistency as well as row values. Unsafe no-fsync mode is excluded from durability qualification and comparable performance claims.

## 8. Compiler and IR qualification

The golden corpus contains positive, negative, and unknown-analysis cases with declared expected semantics. Tests MUST cover:

1. deterministic parsing, typing, normalization and artifact hashing;
2. checked integer/fixed-decimal arithmetic, overflow, invalid scales and boundary values;
3. read/write/guard/result dependency capture, aliases and dynamic key scopes;
4. absent keys, range predicates and aggregate membership changes;
5. monotone facts versus non-monotone revocation/cancellation;
6. effect commutativity that does not preserve returned values;
7. sequentially unsafe operations that remain unsafe under C5;
8. solver timeout/unknown and uncheckable contracts;
9. adding an interfering operation to a previously safe catalog;
10. deterministic plan checking, tampered certificates and unsupported rule versions.

The independent evaluator enumerates small states and concurrent histories for accepted plans. A found counterexample MUST be attached to a regression case. Finite absence of a counterexample must not be promoted to a general symbolic proof rule.

Fuzz parser, typed IR, plan, certificate and envelope decoders. Malformed input yields bounded typed errors, not panics, infinite analysis, unexpected file access or memory-unsafe behavior.

Record accepted/rejected/unknown counts, analysis time, peak memory, proof-rule usage, plan size and diagnostic quality. A solver timeout is part of coverage statistics; excluding hard inputs would overstate compiler precision.

## 9. Directed distributed fault matrix

The first system campaign uses three isolated node processes and separate data directories on a disposable development/test environment. No live EVA/NietzscheDB or shared HeraclitusDB memory service is a fault target. The harness requires an explicit allowlisted test root and validates resolved paths before cleanup.

| ID | Fault/history | Expected safety result |
|---|---|---|
| Q-F01 | Duplicate commit delivery before/after ACK loss and restart | One effect; stable exact result |
| Q-F02 | Causal successor arrives before its predecessor; queue fills | No premature visibility; bounded backpressure |
| Q-F03 | Origin sequence hole plus frontier compaction | Missing dependency remains detectable |
| Q-F04 | Two regions consume the final units concurrently | No negative resource or duplicated usable right |
| Q-F05 | Donor debit durable, receiver ACK lost, retries and donor restart | Transfer cannot recreate donor rights or credit receiver twice |
| Q-F06 | Failover to stale replica during partition | No unsupported authority; unavailable is acceptable |
| Q-F07 | Empty uniqueness scan races with insert into that predicate | Certification/serial path allows at most a valid winner |
| Q-F08 | C4 validates; another class writes same invariant domain | Shared authority/reservation boundary prevents bypass |
| Q-F09 | One participant prepares; coordinator crashes/partitions | Explicit in doubt; no invented commit/abort |
| Q-F10 | Commit decision durable; only one participant installs | Atomic reads wait/help or return a legal earlier cut; no false final ACK |
| Q-F11 | C5 leader fails before/after durable consensus decision | One ordered effect; old leader cannot accept fresh authoritative writes |
| Q-F12 | Plan migration with offline escrow holder | Incompatible activation blocks until closure/reconciliation |
| Q-F13 | New generation retries an old success or returns stale session token | Original result/required visibility preserved |
| Q-F14 | GC/result eviction followed by stale node and ancient request | Bootstrap/refusal or retained result; no resurrection/re-execution |
| Q-F15 | Clock jumps far forward/backward | No extra authority from timeout or lease assumptions absent from the contract |
| Q-F16 | Close/catalog/activation crashes at each durable boundary | At most one incompatible admitting generation |
| Q-F17 | Restore snapshot at different physical versions/placement | Same semantic identities and commitments; local versions not globally compared |

Partition tests include asymmetric links, majority/minority components, isolated rights holders, and a client connected to a stale gateway. Healing requires eventual delivery/recovery assumptions; record backlog limits and replay completion.

A Jepsen-style runner is a deliverable. Claim actual Jepsen execution only when the Jepsen framework and checker have been run and their configuration/artifacts are retained. Equivalent bespoke testing must be identified as such.

## 10. Liveness and resource limits

Safety can be achieved by refusing everything. Qualification must separately measure useful progress:

- local successes while valid rights and durable storage are available;
- operations waiting for causally missing records;
- rights exhaustion versus actual global resource exhaustion;
- starvation duration and per-client/regional success distribution;
- time to resolve in-doubt decisions after communication recovers;
- drain/catch-up time after recovery and migration closure;
- bounded queues, prepared records, pinned journal/snapshots and staging disk.

Liveness tests state eventual synchrony/delivery, surviving quorum, resource supply, and scheduler fairness assumptions. Run limits are experimental budgets, not protocol permissions to abort. An unavailable authority or missing causal prerequisite may legitimately block an operation.

Pressure tests MUST assert bounded memory/disk behavior and typed backpressure. Dropping correctness-critical records to stay within a limit is a failure. Queue drops must cause retryable refusal or refetchable state, never false causal frontier advancement.

## 11. Workload contracts

Each workload package includes a typed schema, registered operations, exact return meanings, invariant set, durability/partition profile, key distribution, initial state, generation script, independent oracle and supported migration.

| ID | Workload | Operations and required obligations |
|---|---|---|
| W1 | Inventory/reservations | restock, reserve, sell, release, transfer; nonnegative free/held units, conservation, successful reservation identity |
| W2 | Account/ledger | deposit, withdraw, transfer, hold/release; checked amounts, configured lower bound, conservation of internal transfer, exact final status |
| W3 | Unique namespace | register, rename, delete; uniqueness across concurrent empty scans, deletion/reuse semantics, retry identity |
| W4 | Causal order | create, confirm-payment fact, ship; predecessor visibility; cancel/refund tested separately because revocation may need coordination |
| W5 | TPC-C-derived subset | explicit NewOrder/Payment/StockLevel contracts and invariants; all semantic changes documented |
| W6 | Edge quota | disconnected authorized consumption, exhaustion, transfer, restart and reconnect |
| W7 | Mixed-domain allocation | reserve capacity plus debit budget with atomic finality, combining supported C1/C2/C3/C5 components |
| W8 | Plan/contract evolution | capacity increase/decrease, added interfering operation, IDC split/merge and placement move with retained client commitments |

W5 is not an official TPC-C result. Do not report `tpmC` or compliance without a separate compliant implementation and audit. Domain examples are synthetic correctness workloads, not validated financial or government applications.

For every C1 candidate, include an adversarial variant with an exact state-dependent return or finite-width overflow. For every monotone causal fact, include a revocable variant. The compiler must either derive additional obligations or reject/fallback with a valid contract explanation.

## 12. Baselines and semantic equivalence

These are evaluation candidates inherited from project research, not a statement that their latest versions have been installed or tested:

| Baseline | Purpose |
|---|---|
| PostgreSQL SERIALIZABLE with correct retry/constraint logic | Strong local transaction reference; no implied multiregion availability |
| Correct weaker PostgreSQL isolation where equivalent | Avoid artificially over-coordinating the comparison |
| Strong distributed SQL deployment | Compare equivalent replica/failure/read/commit semantics |
| Hand-written escrow/causal protocol | Measure value and cost of automatic derivation against competent manual design |
| CRDT/eventual implementation | Eligible convergent contracts only; not an invalid bounded-inventory baseline |
| Compiler/runtime over embedded/existing storage | Test whether custom storage is necessary for measured benefit |
| Reproducible LoRe/Hamsaz/DeMon/Antidote-related implementation | Prior-work comparison with exact scope and implementation provenance |

Pin version, revision, license, configuration, hardware and workload port before running. A reimplementation is labelled as a reimplementation and independently checked. Missing runnable prior work remains a conceptual comparison, not a fabricated data point.

For each paired experiment, an equivalence sheet MUST compare:

```text
final versus provisional ACK
returned value semantics
invariants and rejection conditions
local/multi-node durability and number of failure domains
read/session guarantees and atomic visibility scope
retry/result retention
partition behavior and allowed admission refusals
background rights transfer/replication cost
```

If these differ materially, separate the experiment or explain the difference without claiming a like-for-like speedup. A local-fsync acceptance compared with a quorum-durable final result is not an equivalent commit latency comparison.

## 13. Performance protocol

Correctness checks remain active during benchmark runs. When online checking itself cannot be kept off the critical path, report checker overhead and also validate retained complete histories offline. Any invariant/contract violation invalidates that run's performance result.

Record CPU, RAM, storage, filesystem, OS/kernel, sync mode, network topology/latency/loss, node placement, replica count, dataset size versus memory, compiler/build profile and background services. Avoid comparing a warm in-memory candidate against a cold disk-bound baseline without identifying that condition.

Before measured runs, publish the load sweep, warm-up policy, run length, number of independent seeds/repetitions, resource budget, and overload stopping condition. These are experiment parameters, not promised throughput targets. Include both offered load and completed useful work to avoid hiding saturation behind client blocking.

Measure:

- p50/p95/p99 end-to-end latency, from logical request invocation through final outcome, including retries/waits;
- per-attempt latency separately from logical-request latency;
- successful, business-rejected, admission-refused, timed-out and unresolved request counts;
- throughput of successful logical operations and the total offered load;
- foreground/background cross-region messages and bytes, consensus rounds and fsync count;
- rights initialization, transfer, rebalancing, starvation and stranded capacity;
- compiler time/memory, plan bytes, metadata bytes per operation and retained evidence;
- recovery time, catch-up time, unavailable duration and migration phase durations;
- storage metrics from SPEC-002, including write amplification and checkpoint interference.

Use a measurement method that records requests delayed by overload; do not omit queued time. Report distributions across independent runs, sample counts, variance and the interval-estimation method. Raw percentiles cannot be reconstructed by averaging node percentiles. Keep mergeable histograms or raw samples with documented units.

Sensitivity sweeps vary skew/hot keys, number/size of IDCs, cross-IDC fraction, causal dependency width, exact/strong read fraction, amount and distribution of rights, operation mix, finite resource supply, replica count, network delay/loss, version churn and dataset/cache ratio. Publish where conventional coordination wins.

No numerical throughput/latency claim is a release target until a reproducible baseline justifies it. Metrics of zero violations are correctness requirements, not measured facts in this document.

## 14. Research experiments and stopping rules

| Experiment | Question | Evidence and possible refutation |
|---|---|---|
| E1 | Is the language useful? | Accepted/rejected real-shaped contracts, annotations and author effort; manual protocol work still dominant weakens the thesis |
| E2 | Does semantic derivation reduce cost at equal guarantees? | Equivalence sheets plus latency/message/background-cost curves; benefit caused only by weaker finality is invalid |
| E3 | Does composition preserve observable promises through failures/upgrades? | Minimized histories, invariant/result checker and migration model; a supported counterexample refutes that rule |
| E4 | Where does the benefit disappear? | Sensitivity sweeps and starvation/strong-read costs; broad collapse to C5 narrows the useful fragment |
| E5 | Does owning storage matter? | Equivalent runtime over existing storage and native kernel; comparable benefit without custom pages favors a runtime/library product |

SPEC-001 H1–H5 are tested through E1/E2/E4, E2, E2/E4, E2/E3, and E3 respectively. The proposal's narrower refinement hypothesis additionally requires E3 during generation/authority transition, beyond steady-state invariant checking.

Novelty review is separate from test success. Reproducing a known protocol correctly is not proof of a new scientific contribution. A paper needs a precisely scoped statement, literature comparison, proof/argument and reproducible experiments. A native B+Tree is an engineering decision that may be revisited under SPEC-002 §180.

## 15. Failure bundles and minimization

Every `FAIL` or `INCONCLUSIVE` emits a self-contained bundle:

```text
qualification/<campaign_id>/<run_id>/
  manifest.json
  verdict.json
  history.jsonl
  schedule.json
  initial-state/
  contracts/
  plans/
  evidence/
  metrics.json
  reproduction.md
```

`evidence/` contains required journal/page fragments, modeled storage events, certificates and protocol logs from synthetic runs. Redaction must preserve enough identity and causal information to reproduce; otherwise mark the external bundle incomplete and retain the full local test evidence.

The minimizer shrinks operation count, keys/values, messages and faults while preserving a failing schedule. Its output is rechecked with the independent oracle. Record both original and minimized artifacts. A minimized counterexample becomes a regression with the affected obligation/spec IDs.

Proposed CLI (implementation deliverable, not currently available commands):

```text
astra qualify --manifest <file>
astra simulate --manifest <file> --seed <seed>
astra replay --bundle <path>
astra minimize --bundle <path>
astra report --campaign <path>
```

The runner returns distinct process exit codes for pass, failure, inconclusive and invalid configuration. Do not overload timeout with pass. Store schema versions for evidence so later tools can read earlier campaigns or fail explicitly.

## 16. Formal model targets

Minimum models cover escrow conservation/transfer, local durability barriers, prepare/decision/publication, C4 validation reservations, C5 authority failover, and plan close/drain/activate. Each model states initial conditions, transitions, invariants, fairness assumptions and explored bounds.

Link implementation events to model transitions. If implementation introduces an optimization absent from the model, either show a refinement mapping or disable that optimization for the claim. Model checking is evidence within bounds; an unrestricted proof is a separate deliverable.

Negative controls are mandatory: deliberately remove donor fencing, skip range validation, acknowledge before sync, discard a causal hole, or activate with an offline holder. The relevant checker must find a violation. A checker that passes both the correct and deliberately broken protocol has not established useful evidence.

## 17. Qualification gates

| Gate | Dependencies | Exit criteria |
|---|---|---|
| Q0 — Formal core | SPEC-003/004 | Golden IR corpus, deterministic artifacts, explicit unknown/reject behavior, reference evaluator |
| Q1 — Local durability | SPEC-002 S0–S6 | P1–P8 campaigns and storage/journal verifiers pass for supported formats |
| Q2 — Local prepared/metadata | SPEC-002 S7–S8 | P9/P10 and atomic business/protocol/result persistence pass |
| Q3 — Initial protocols | SPEC-005/006/008 single-IDC | C1/C2/C3/C5 fault acceptance and recovery pass; capabilities explicitly scoped |
| Q4 — Composition | SPEC-008 multi-IDC and SPEC-004 composition | Mixed workloads, authority interaction and atomic read/finality checks pass |
| Q5 — Evolution | SPEC-009 | EV-01–EV-14 pass on enabled features with real-process and deterministic evidence |
| Q6 — Optional certification | SPEC-007 | Key/range/predicate validation and durable reservations qualified with Q4/Q5 rerun where affected |
| Q7 — Evaluation | Prior relevant gates | E1–E5 report, pinned baselines, equivalent guarantees, failures and limitations published |

A feature is disabled until its applicable gates pass; C4 can remain disabled while the initial prototype progresses. Each gate stores explicit test IDs and evidence references. After a code change, rerun affected checks and required integration gates; repeat unrelated campaigns only when dependency changes or failures justify them.

This is research-prototype qualification. Production authentication, authorization, encryption, tenant isolation, operational backup/restore policy, service packaging and compatibility support require later specifications and evidence. Absence of these capabilities must not be hidden behind a passing protocol suite.

## 18. Acceptance of the qualification system

| ID | Scenario | Required result |
|---|---|---|
| QA-01 | Replay identical manifest/seed/schedule | Identical deterministic trace digest |
| QA-02 | Inject each negative control from §16 | Relevant checker produces a minimized failure |
| QA-03 | Unknown request commits after lost response | Checker accepts only histories consistent with the durable commit |
| QA-04 | Checker budget exhausts | INCONCLUSIVE; feature gate does not pass |
| QA-05 | Required scenario omitted | NOT_RUN; no silent green report |
| QA-06 | Local-only ACK compared with quorum-durable ACK | Equivalence validator rejects like-for-like comparison |
| QA-07 | Unsafe no-fsync mode selected | Durability gate rejects configuration |
| QA-08 | Result invariant passes but an earlier final reservation is erased | Observable-history checker fails |
| QA-09 | Client queues under overload | End-to-end latency includes queue/retry/wait time |
| QA-10 | Fault target points outside allowlisted test environment | Runner rejects before mutation |

Acceptance tables in SPEC-003–009 and storage scenarios SPEC-002 §§173–179 are inputs to the campaign registry. Their presence in Markdown is not evidence they have run.

## 19. Deliverables and traceability

Deliver the reference interpreter, simulator adapters, crash runner, distributed harness, checker/minimizer, workload packages, baseline equivalence sheets, and evidence/report schemas before publishing comparative claims.

| Source | Required coverage |
|---|---|
| SPEC-001 §§64–75 | Workloads, compiler soundness/precision, faults, hypotheses and falsification |
| SPEC-002 §§127–179 | Verification tools, crash points, P1–P10 and storage benchmarks |
| SPEC-003/004 | Typed contracts, deterministic derivation, conservative acceptance and obligations |
| SPEC-005/006 | Delivery/causality, rights accounting, dedupe, authority recovery |
| SPEC-007/008 | Certification, consensus, distributed decision and atomic observation |
| SPEC-009 | Preserved receipts and authority across generation transitions |
| Research E1–E5 | Useful fragment, honest comparisons, composition and storage necessity |

The final campaign report leads with supported capabilities and remaining failures/unknowns, then states exact guarantees, tested failure bounds, evidence, performance and limitations. It must be possible to reproduce a verdict without trusting the prose claim that the database is correct.
