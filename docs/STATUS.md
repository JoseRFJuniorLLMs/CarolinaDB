# CarolinaDB — Implementation Status

**Updated:** 2026-09-13
**Authoritative stage order:** [SPEC-014](../md/SPEC-014.md). This file records what exists in code and what evidence it has. It never upgrades a stage: `present in code != implemented capability != qualified capability != production-enabled capability`.
**Requirement-level audit:** [AUDIT.md](AUDIT.md) classifies every requirement and acceptance row of SPEC-001…014 (and the owner checklists) as implemented / partial / missing against this tree (one auditor per target; the planned adversarial verification pass completed for one target only, which that file states up front); each `md/SPEC-0NN.md` header carries a one-paragraph `Implementation status (2026-09-12)` summary. An independent narrative audit of the same tree is [relatorio-completo.md](relatorio-completo.md).
**Public CI:** the GitHub Actions run for base commit `cdfca6a` **failed** at `cargo fmt --all -- --check`, so it is not evidence for the later checks. The working tree has since been reformatted and verified locally (see "Test evidence"); a new push is needed before any public-CI claim.

## Legend

| Mark | Meaning |
|---|---|
| ✅ | implemented and covered by tests in this repository (`cargo test --workspace`) |
| 🟡 | partially implemented; scope stated in the row |
| ⬜ | not started |
| Q: | qualification verdict per SPEC-010 (`PASS` only for the stated scope; `NOT_RUN` otherwise) |

## Documentation closure (REVISAR-02 §21)

| Item | Status |
|---|---|
| SPEC-009 v0.2 / SPEC-010 v0.2 / SPEC-011–014 present | ✅ (authored by the project owner) |
| Cross-SPEC lint `tools/spec_lint.py` (aliases, schema owners, dangling refs/sections/links, naming residue, FM gates) | ✅ PASS (16 files). `python tools/spec_lint.py --self-test` is the retained negative control: seven crafted bad documents, one per check, each of which must produce an error, plus a clean document that must not. CI runs both |
| Ownership matrix `md/SPEC-OWNERSHIP.md` | ✅ |
| Naming cleanup (`AstraDB`/`astra-*`/`astra <cmd>` → CarolinaDB/`carolina-*`/`carolina`; `astra.*` domains kept) | ✅ |
| README status section | ✅ |
| `ARCHITECTURE-FREEZE-0.1.md` | ⬜ (to be produced once the codec manifest is frozen) |

## MVP-0 — Semantic core (SPEC-003)

| Deliverable | Status | Where |
|---|---|---|
| Typed identity taxonomy (SPEC-011 §2), `RequestKey`, `TxnId` layout (SPEC-012 §3), `IdcBinding`, `PlanRef`, `AuthorityBinding` | ✅ | `crates/carolina-core/src/ids.rs` |
| Restricted canonical JSON encoder/decoder with re-encode check, limits, negative cases | ✅ | `crates/carolina-core/src/canon.rs` |
| Domain-separated SHA-256, all `astra.*` domains | ✅ | `crates/carolina-core/src/hash.rs` |
| Checked `Decimal(p,s)` | ✅ | `crates/carolina-core/src/decimal.rs` |
| Order-preserving key codec v1 (SPEC-002 §11) | ✅ | `crates/carolina-core/src/keycodec.rs` |
| DSL lexer/parser (RECORD/ENUM/INDEX/RESOURCE/INVARIANT/OPERATION, explicit sections) | ✅ | `crates/carolina-lang/src/{lexer,parser}.rs` |
| Stable ID allocation, type check, lowering, footprints | ✅ | `crates/carolina-lang/src/lower.rs` |
| `ModuleIR`/`InvariantIR`/`OperationIR`/`EffectIR`/`ContractIR` canonical encoding + hashes | ✅ | `crates/carolina-lang/src/ir.rs` |
| Reference interpreter (checked arithmetic, full-scope invariants, normalized literal effects, replay) | ✅ | `crates/carolina-lang/src/interp.rs` |
| Bounded concurrent-acceptance counterexample explorer + replay | ✅ | `crates/carolina-lang/src/counterexample.rs` |
| Seven SPEC-003 §12 fixtures + golden IR bytes/hashes | ✅ | `fixtures/dsl/*.cdl`, `fixtures/golden/*` |
| S003-A01, A02, A03, A04, A06, A08, A10 | ✅ tests | `crates/carolina-lang/src/fixtures.rs` |
| S003-A05 (footprint traps), A07 (request identity), A09 (fuzz), A11 (session scope) | ✅ A05 covers aliasing (two point selectors in one atomic group, aliased call refused), phantom insertion (an insert inside every aggregate over the record, two admissible inserts violating the group budget together), aggregate group movement (a row changing its `GROUP BY` key is evaluated against the destination group), bound-source change (writing `Ledger[0].total` joins the closure of the constrained record), reference deletion (deleting the parent of a `REFERENCE` invariant is rejected and interacts with the child's writers) and unique absence (a key namespace, not a row); A11 at lowering and as `composite_session_scope_is_refused_as_unsupported`; A07 end-to-end in the runtime tests; A09 as a deterministic mutation fuzz over the IR decoder and the DSL frontend | `crates/carolina-compiler/tests/footprint_a05.rs`, `crates/carolina-lang/src/spec003_tests.rs` |
| Grouped aggregate bounds (SPEC-003 §5): `AGGREGATE … GROUP BY m.team <= 10` now parses as key + bound. The group-key expression used to consume the comparison, so no grouped bound could be written at all; the key stops below the comparison level and a regression test pins the per-group semantics | ✅ fixed 2026-09-13 | `crates/carolina-lang/src/parser.rs`, test `grouped_aggregate_bound_parses_and_evaluates_per_group` |
| Structural lowering errors (two/zero/optional primary keys, duplicate names, unresolved records), digest sensitivity (version/scale/invariant), byte-identical re-ordering under a preserved id allocation (A02), malformed-IR refusal, OPTIONAL/EXISTS reads, `Assign`/`AddToSet`/`RemoveFromSet`/`CompareAndSwap`, partial-release and same-endpoint-transfer rejections at the effect level | ✅ tests added 2026-09-12 | `crates/carolina-lang/src/spec003_tests.rs` |
| Invariant expressions that fail (overflow inside an aggregate) reject without mutating the input; malformed invariant IR stays an internal error; `state_valid` re-checks disconnected invariants; return wrappers cannot hide rows/sets; keys and set elements need comparable nested types | ✅ | `crates/carolina-lang/tests/semantic_validation.rs` |
| Q0 verdict | Q: PASS for the stated scope via `carolina qualify` (golden corpus, deterministic artifacts, reference evaluator with negative control); see "Qualification system" |

Golden files under `fixtures/golden/` are the IR1 freeze candidates. They MAY be re-frozen with
`UPDATE_GOLDEN=1 cargo test -p carolina-lang` until `ARCHITECTURE-FREEZE-0.1.md` pins their digests.

## MVP-1 — Conservative compiler (SPEC-004)

| Deliverable | Status | Where |
|---|---|---|
| Closure/IDC templates (union-find over record footprints), directed interactions (RequiresVisible, InvalidatesGuard, AtomicWith) | ✅ | `crates/carolina-compiler/src/analysis.rs` |
| Per-candidate obligations IR-DEF, IR-FOOT, DURABILITY, AUTHORITY, RUNTIME-CAPABILITY, IR-OBS, SESSION, IR-SEQ, IR-CONC, IR-ATOM, IR-COMP with `ProofStatus` Proven / Disproven(replayable counterexample) / Unknown | ✅ | `analysis.rs`, `explore.rs` |
| Bounded concurrent-acceptance exploration from generated small states (deterministic budget) | ✅ | `explore.rs` |
| Protocol library manifest (C0 local + C5 serial qualified; C1–C4 present, unqualified) and analysis rule manifest, both hashed into the certificate | ✅ | `library.rs`, `rules.rs` |
| Deterministic selection: C5 baseline, cheaper replacement only with proven obligations (C0 in local topology) | ✅ | `select.rs` |
| Canonical `OperationPlan`, `ConsistencyCertificate`, artifact checker (`check_artifacts`), EXPLAIN | ✅ | `plan.rs`, `select.rs`, `explain.rs` |
| Unsafe/incomplete scopes rejected (`UnsatisfiableDurability`, `UnsupportedSessionScope`, `UnmetObservationContract`, `NoSafePlan`) | ✅ tests | `crates/carolina-compiler/src/select.rs` |
| Artifact checker also verifies the evidence manifest: every `Disproven` judgment's counterexample bytes hash to the listed digest, no dangling entries (S004-A12: forged, dropped or dangling evidence fails) | ✅ | `select.rs` `check_artifacts`, test `checker_rejects_forged_or_missing_counterexample_evidence` |
| Policy (`force_serial` keeps a closure on C5, policy hash in the certificate, no override of failed obligations), deterministic-budget exhaustion (`Unknown(BudgetExceeded)`, never Proven/Disproven, no evidence claimed), unqualified C3/C4 candidates (`MissingRuntimeCapability`), composite session scope (`UnsupportedSessionScope`) | ✅ tests added 2026-09-12 | `crates/carolina-compiler/src/lib.rs` |
| CLI `carolina compile / explain / check / ir / fixtures`, plus `carolina plan <module> [operation]` (canonical `OperationPlan` artifacts) and `carolina graph invariants <module>` (IDC templates, affected records/invariants, interaction edges) | ✅ unit tests for plan/graph and compile→check round-trip with tampering | `crates/carolina-cli/src/main.rs` |
| CC0 / CC1 (C5 portion) verdict | Q: PASS for the stated scope via `carolina qualify` Q0-DETERMINISM (18 plans, tampered artifacts refused); no distributed runtime qualification of any candidate |

Candidates remain inactive until runtime qualification (SPEC-004 §13); the local runtime activates only
plans whose atomicity is one local batch and whose durability is `LocalStable`.

## MVP-2 — Local durable slice (SPEC-002, SPEC-012 local identity)

| Deliverable | Status | Where |
|---|---|---|
| On-disk formats: 8 KiB pages with CRC32C, journal frames, MANIFEST A/B, torn-tail detection | ✅ | `crates/carolina-storage/src/format.rs` |
| Fault-injectable positional I/O (`FaultPoint` at every durable boundary, crash and I/O-error actions) | ✅ | `io.rs` |
| B+Tree with MVCC versions `(key ASC, seq DESC)`, `(key, seq)` separators, overflow chains, copy-on-write persistence at checkpoint, structural verification | ✅ differential tests vs `BTreeMap` oracle incl. small pools and reopen | `btree.rs`, `tests/btree_differential.rs` |
| Buffer pool (CLOCK, never evicts dirty pages, bounded overshoot under dirty pressure) | ✅ | `buffer.rs` |
| Redo-first journal, segments, group/sync/unsafe durability modes, torn-tail truncation only in the last segment, mid-log corruption fails closed | ✅ | `journal.rs`, `tests/kernel.rs` |
| `CompiledBatch` / `ProtocolOnlyBatch` with semantic digest, one mutation per key, protocol record CAS (`ExpectedRecordRevision`), `TxnStatusRecord` phase machine | ✅ | `batch.rs` |
| `DurableStorageKernel`: commit, commit_protocol, prepare/commit_prepared/abort_prepared (prepared state invisible, IN_DOUBT reported, decisions durable and idempotent), snapshots, checkpoint, recovery, verify | ✅ | `kernel.rs`, `tests/kernel.rs` |
| SPEC-002 §69: a transaction that already has a durable decision (INSTALLED/TERMINAL/ABORTED) is never installed twice — a duplicate `commit` is refused with `TxnAlreadyCommitted`/`TxnAlreadyAborted` before anything is journaled (Store and MemKernel) | ✅ | `kernel.rs` `refuse_decided_txn`, `tests/kernel.rs` |
| SPEC-002 §21/§78: key/value caps checked before the journal append (`KeyTooLarge`/`ValueTooLarge` leave no record to replay); one writer per directory enforced on every platform through `File::try_lock` (needs Rust ≥ 1.89) and released with the store | ✅ | `kernel.rs`, `tests/kernel.rs` |
| Manifest page-size mismatch fails open; the buffer pool refuses to flush a dirty page whose LSN is not durable (WAL-before-page) | ✅ unit tests | `format.rs`, `tests/kernel.rs` |
| SPEC-002 §111–§114 fail-closed durability: a durable write that returns early marks the store as requiring recovery (RAII guard), after which every read and every further commit through that handle is refused with `NotReady` until it is reopened; the acknowledged commit survives the reopen, work refused by the failed store never becomes visible, and the reopened store passes a full structural verify | ✅ campaign `io_error_never_yields_success` | `kernel.rs` (`begin_durable_write`), `campaign.rs`, `tests/crash_matrix.rs` |
| Reference `MemKernel` and Store ⇄ MemKernel differential (user rows and txn statuses) | ✅ | `memkernel.rs`, `tests/kernel.rs` |
| Crash campaign: every fault point × n-th occurrence, checking P1 (acked commits present), P2/P3 (crashed step all-or-nothing), P6 (recovery idempotent), P9 (prepared invisible/in doubt); injected I/O error never yields success | ✅ 52 scheduled cases (13 fault points × nth 1..=4); in the standard profile 33 of them actually crash, the rest are unreachable or fall inside `create`, and the campaign asserts that at least two thirds of the reachable cases crashed | `tests/crash_matrix.rs` |
| Local RequestHome: durable `request_home_state`, epoch advance per open, BindIfAbsent with CAS, identity conflict on changed content | ✅ | `crates/carolina-runtime/src/home.rs` |
| Explicitly local `authority_grant` written at create and verified at open (no mocked distributed durability) | ✅ test `opening_without_a_matching_local_grant_is_refused`: a foreign home id, a foreign cluster id and a bare storage directory are each refused with `AuthorityUnavailable` before any request can bind | `crates/carolina-runtime/src/engine.rs`, `tests/local_slice.rs` |
| Request path: admission (schema/operation/contract hashes, typed arguments) → bind → closure state load → interpreter → `CompiledBatch` (binding transition + terminal `TxnStatusRecord` + receipt) → reply | ✅ | `engine.rs` |
| `FinalReceiptV1` persisted at the home, `ResolveRequest` → identical receipt, `OutcomeUnknown` on post-barrier failure, business rejections are final REJECTED receipts | ✅ | `engine.rs`, `tests/local_slice.rs` |
| Result eviction → `ResultTombstoneV1`; namespace retirement → `IdentityExpired` | ✅ | `engine.rs` |
| SPEC-014 §4 schedules: last unit contention, duplicate release, overflow, changed content under one key, crash after allocation, crash after commit before reply, result eviction, retired namespace after restart, checkpoint + reopen with identical receipts | ✅ | `crates/carolina-runtime/tests/local_slice.rs` |
| Invariant scope: an operation whose closure has no invariants is not evaluated against unrelated aggregates over unloaded tables; an aggregate whose SUM overflows is a final `InvariantRejected` receipt (reason names `NumericOverflow`) with no row mutation, byte-identical on retry and resolvable after checkpoint + reopen | ✅ | `crates/carolina-runtime/tests/invariant_scope.rs` |
| CLI `carolina verify <dir>` and `carolina workload <dir>` | ✅ smoke test `workload_then_verify_round_trip`: the workload writes a fresh directory and its retry returns byte-identical receipt bytes, resolve recovers the receipt, verify reopens and structurally checks the directory, and a directory holding no database fails | `crates/carolina-cli/src/main.rs`, `tests/qualification_cli.rs` |
| Snapshots/restore (`SnapshotManifestV1`/chunks) wired into the store | 🟡 wire records and validation only (`crates/carolina-wire/src/snapshot.rs`); no store export/import yet |
| MVCC version reclamation and journal segment retention (SPEC-002 §87–§91, §105, S10) | ✅ at checkpoint, enabled by default (`StoreOptions::reclaim_at_checkpoint`). The horizon is the oldest registered snapshot and never above the durable read point; reclamation is page-local (a leaf drops only versions it can prove superseded inside itself) and tombstones are kept, because one key's versions may span leaves and a clean leaf is not rewritten. The journal keeps the segment holding `checkpoint_lsn` and everything after it, prepared transactions pin it through the existing clamp, and the manifest is published before any file is deleted. Metrics: `mvcc_versions_reclaimed_total`, `journal_segments_reclaimed_total`, `journal_bytes_reclaimed_total`, `journal_retained_bytes`, `oldest_snapshot_seq` | `kernel.rs`, `btree.rs` (`persist_with_gc`), `journal.rs`, tests `checkpoint_reclaims_invisible_versions_and_never_a_registered_snapshot`, `checkpoint_reclaims_journal_segments_but_prepared_work_pins_them` |
| Checkpoint image completeness (defect found while enabling retention): a dirty page whose parent had been evicted was never written, because the copy-on-write walk descended only through resident pages. It was invisible while the journal was replayed from segment 1 and became data loss the moment the journal was truncated. The walk now fetches clean parents while any dirty page remains, and retention only advances when the checkpoint leaves the pool with no dirty page | ✅ | `btree.rs` `persist_page_gc`, `kernel.rs` |
| Q1/Q2 verdicts | Q: PASS for the stated scope (see "Qualification system" below): `carolina qualify` gates Q0, Q1, Q2 PASS; fault model process kill / short write inside one process; OS page-cache loss is not modelled (SPEC-010 §7). |

## Qualification system (SPEC-010)

| Deliverable | Status | Where |
|---|---|---|
| `QualificationManifest` (§2): source revision + working-tree digest over every tracked file, dirty flag, toolchain (rustc from `build.rs`), lock digest, enabled/disabled features with justification, workloads, seeds, budgets | ✅ | `crates/carolina-qualify/src/manifest.rs` |
| Statuses PASS/FAIL/INCONCLUSIVE/NOT_RUN/NOT_APPLICABLE, gate derivation (a NOT_APPLICABLE required check never opens a gate), claimed gates, exit codes 0/1/2/3 | ✅ | `verdict.rs` |
| Observable history (§4): Invoke/AdmissionRefusal/FinalReply/UnknownReply/ExpiredReply/Decision/Crash/Restart/Checkpoint/Resolve/ResultEvicted/NamespaceRetired, canonical JSONL, trace digest | ✅ | `history.rs` |
| Independent W1 oracle (§3/§4/§11): hand-written inventory model, no DSL interpreter import, exact result values | ✅ | `w1.rs` |
| Usefulness measured separately from safety (SPEC-014 §4): `Q2-W1-LOCAL` counts committed invocations and FAILS a campaign that commits none, so refusing or rejecting everything can never satisfy the checker vacuously | ✅ | `runner.rs` |
| History checker (§5): Q-C01, Q-C02, Q-C03, Q-C04, Q-C05, Q-C13, Q-C14 with branch exploration of unresolved unknowns and a budget (INCONCLUSIVE when exceeded); Q-C06–Q-C12 NOT_APPLICABLE with the disabled capability named | ✅ | `checker.rs` |
| Deterministic local schedules over the runtime engine (§6/§7): seeded W1 workload with retries, changed content, eviction, retirement, restarts, checkpoints; crash at every `FaultPoint` × n-th occurrence; every request resolved after final recovery; engine state read back for the oracle | ✅ | `local.rs` |
| Storage campaigns P1–P10 as callable functions shared by tests and the runner | ✅ | `crates/carolina-storage/src/campaign.rs` |
| Minimizer (§15, delta debugging over the schedule, re-checked by the oracle) and bundles (`manifest.json`, `verdict.json`, `history.jsonl`, `schedule.json`, `metrics.json`, `reproduction.md`, `initial-state/`, `contracts/`, `plans/`, `evidence/`) | ✅ | `minimize.rs`, `bundle.rs` |
| Campaign runner (§17) and CLI `carolina qualify / simulate / replay / minimize / report` | ✅ | `runner.rs`, `crates/carolina-cli/src/main.rs` |
| Acceptance of the qualification system (§18): QA-01 (identical trace digest), QA-02 (negative controls: duplicated effect, ack before durable, erased earlier reservation = QA-08, unknown treated as abort), QA-03, QA-04 (budget → INCONCLUSIVE), QA-05 (omitted scenario → NOT_RUN, non-zero exit), QA-07 (unsafe no-fsync refused), QA-10 (test root allowlist) | ✅ | `crates/carolina-qualify/tests/acceptance.rs` |
| Campaign configuration validation: empty seed sets and zero exercise budgets are refused (`ConfigError::InvalidBudget`) before any I/O; the CLI refuses malformed `qualify` options with exit code 2; `replay`/`minimize` never report INCONCLUSIVE as success; a retained bundle cannot lose its trace binding or mix evidence from two runs | ✅ | `tests/acceptance.rs`, `tests/bundle_integrity.rs`, `crates/carolina-cli/tests/qualification_cli.rs` |
| Formal models FM-1/FM-2/FM-3 (§16): explicit-state BFS checkers with exhaustive exploration under stated bounds and mandatory negative controls (each broken variant produces a counterexample trace); TLA+ sources + `.cfg` checked by pinned TLC | ✅ PASS within bounds. Rust checkers: FM-1 3268 states/5 controls; FM-2 486/6; FM-3 3392/5. TLA+ tools v1.8.0: FM-1 3268 distinct states; FM-2 348; FM-3 2816; no error. Bounded model evidence only: the protocols are not implemented, so Q3–Q6 stay NOT_RUN | `crates/carolina-models/src/{fm1,fm2,fm3}.rs`, `models/`, `tools/run_tlc.py` |
| Codec golden corpus (SPEC-012 §12, SPEC-014 §5): `fixtures/codec/` freezes 54 canonical vectors (25 registered record kinds plus node log/admin payloads and consensus envelopes) with digests in `manifest.txt`; every vector must be a decode/encode fixed point and the derived negative vectors (unknown field, truncation, number literal, whitespace) must be refused | ✅ `QI-CODEC-CORPUS` PASS; `snapshot_manifest`/`snapshot_chunk` were frozen on 2026-09-12 (their codecs exist; the store-level export/import does not); re-freeze only with `UPDATE_GOLDEN=1 cargo test -p carolina-qualify codec` | `crates/carolina-qualify/src/codec_corpus.rs`, `fixtures/codec/` |
| QI security (SPEC-013) | ⬜ NOT_RUN — mTLS/authorization not implemented |

Latest campaigns (debug build, this tree, 2026-09-12): `carolina qualify --quick` and the **standard**
budgets (`carolina qualify --out qualification`) both end with gates **Q0 PASS, Q1 PASS, Q2 PASS,
FM PASS, Q3-C5 PASS** (claimed, exit code 0); QI, Q3, Q4, Q5, Q6, Q7 NOT_RUN (not claimed; QI because
`QI-SECURITY` is NOT_RUN — `QI-CATALOG` and `QI-CODEC-CORPUS` PASS). Scope of the standard PASS:
Q0 = 7 golden fixtures, 18 plans deterministic, 7 tampered plans refused, last-unit race found and
replayed with a quiet negative control, 44 compiler counterexamples canonical; Q1 = 18000 B+Tree ops
with 388 splits against reference scans (seeds 1..3, pools 16/64/128, hot key with 3000 versions),
crash matrix 13 fault points × nth 1..=4 (33 crashing cases of 52 scheduled, P1/P2/P3/P6/P9 held),
torn tail / mid-log corruption / injected I/O error, 6 checkpoint-stage crashes; Q2 = prepared
invisibility, epoch change, 300-batch kernel differential, stale plan/schema admission, 160 W1
schedules (seeds 1–4, 40 ops each, 13 fault points × nth 1..=3, checker budget 200000) against the
independent oracle with every receipt resolved identically after recovery, and an identical trace
digest on replay of a crashing schedule; FM-1 3268 states / FM-2 486 states / FM-3 3392 states with
every negative control producing a counterexample; Q3-C5 = consensus simulation (4 seeds, 20 proposals
each, 1066 delivered / 197 dropped / 55 duplicated messages, crash+restart of every voter, isolated
leader, replay determinism) and the three-process campaign (bootstrap through the log, C5-001/009/010/021,
leader kill, restart + replay, majority kill). The quick profile is what CI runs and uses smaller budgets: seeds 1–2, 24 ops per schedule,
crash matrix nth 1..=2, 3000 B+Tree ops per seed. It was re-run on this tree on 2026-09-13, after version reclamation and journal retention were
enabled, and still ends `overall PASS` with exit code 0: 54 W1 schedules in which 780 of 1485
invocations committed (the campaign now fails a run that commits none, SPEC-014 §4), the crash
matrix crashing in 19 of 26 scheduled cases with P1/P2/P3/P6/P9 held, 54 frozen codec vectors over
25 registered kinds with 216 negative vectors refused, and the same gate set as above. The run of
2026-09-13 also exercises log compaction inside `Q3-CONSENSUS-SIM` (two compactions, highest base
43): every voter applies, the leader drops the prefix, the cluster keeps committing across the new
base and a restarted voter comes back on it. The standard-profile figures in this paragraph were
measured on 2026-09-12; reclamation and compaction do not change those counters, but the standard
profile has not been re-run since.

## MVP-3 — Catalog and single-IDC C5 (SPEC-008 §5/§8, SPEC-011, SPEC-012 §3/§9–§10, SPEC-013)

| Deliverable | Status | Where |
|---|---|---|
| Consensus adapter (SPEC-008 §5): deterministic Raft, fixed three-voter membership, persistent term/vote/log (`raft.state` atomic rename, `raft.log` CRC32C frames with torn-tail discard), term no-op, quorum commit, read barrier (`ReadIndex` round acknowledged by a quorum), no wall-clock leases | ✅ | `crates/carolina-consensus/src/{lib,storage}.rs` |
| Deterministic cluster simulator (SPEC-010 §6): seeded scheduler, loss/duplication/delay, directional partitions, crash/restart from modeled durable state; invariants election safety, log matching, state-machine safety, leader completeness checked after every delivery; reproducible traces | ✅ | `crates/carolina-consensus/src/sim.rs` |
| Catalog state machine (SPEC-011 §3–§5, §7–§8): typed `CatalogKey`s, `CatalogCommand` CAS with expected revisions + digests, phantom-safe scope-lock overlap predicate, one `CatalogGeneration` per successful command, idempotent `AdminRequestId` results, `IdentityConflict` on reuse with different bytes, genesis over a pinned bootstrap manifest (hash verified by every voter, second genesis refused, exactly three voters), grant lifecycle `STAGED→ACTIVE→CLOSING→CLOSED→RETIRED` (closed never reopens), request routes, tombstones that refuse re-insertion | ✅ | `crates/carolina-catalog/src/lib.rs` |
| Node process: `ASTR`/TCP transport with Hello/HelloAck negotiation, role-bound frame kinds (`Consensus` only from `Node` peers, `Admin` only from admin endpoints), peer links with reconnection, single-threaded core | ✅ | `crates/carolina-node/src/{transport,core}.rs`, binary `carolina-node` |
| `DEV_LOCAL` hardening: listeners, peers and clients refuse any non-loopback address; `NodeConfig::validate` rejects wrong profiles, a voter set that is not exactly three distinct voters, a node that is not a named voter, zero ticks, missing/duplicate/unknown/self peers, non-loopback or zero-port or reused addresses, before any listener, thread or data directory is created | ✅ | `transport.rs`, `config.rs`, `tests/config_validation.rs` |
| Admission rechecked in log order: an `Admit` entry whose route/grant/catalog generation no longer authorizes it at its log position is refused with `AuthorityUnavailable` on every voter (a catalog transition can revoke an admission the leader already queued); an admin request id bound to different command bytes is answered with `IdentityConflict` instead of the other command's result | ✅ in-process tests (three `NodeCore`s over channels): concurrent changed content refused / exact retries share one decision, leadership loss keeps the request hash in the unknown reply, a restarted successor finishes an inherited admission before resolve publishes | `core.rs`, `core_tests.rs` |
| Single-IDC C5 ordered execution (SPEC-008 §8): leader admits an `Invoke` as an ordered `Admit` entry; every voter executes the same deterministic engine step in log order; the leader proposes the unique `Decision` (receipt digest); the client reply waits for that decision's commit; followers verify their own digest and fail closed on divergence; `ResolveRequest` served by the leader only after a read barrier; a follower/minority refuses mutations | ✅ | `crates/carolina-node/src/core.rs` |
| Replicated RequestHome: pinned home epoch, allocation counter written in the same protocol batch as each binding, so a new leader continues the same `TxnId` sequence | ✅ | `crates/carolina-runtime/src/home.rs` (`open_pinned`) |
| Bootstrap through the log: genesis → home grant STAGED → ACTIVE → request route (idempotent admin ids; a new leader resumes) | ✅ | `core.rs` `leader_duties` |
| Real-process campaign (SPEC-010 §9 subset): three `carolina-node` processes, isolated data directories, loopback; C5-001/009/010/021 and replicated determinism after killing the leader, restarting it and killing a majority | ✅ | `crates/carolina-node/src/campaign.rs`, `tests/three_nodes.rs` |
| SPEC-013 mTLS / authorization / credential records | ⬜ **not implemented** — only the `DEV_LOCAL` plaintext, loopback-only profile exists and the node never advertises `ENCRYPTED_HOST_V1`; no security qualification is claimed. Consequently the SPEC-014 §3 exit criteria of MVP-3 ("SPEC-013 mTLS/authorization … applicable QI") are **not met** even though Q3-C5 passes |
| Catalog snapshots and Raft log compaction (SPEC-011 §9) | ✅ Each voter folds its applied state into `node.snapshot` (catalog image, applied index, decided requests, retained replies) every 64 applied entries, writes it atomically, and only then reports that frontier. Voters piggyback the frontier on `AppendEntriesReply`; the leader keeps the minimum across the membership as the compaction horizon and drops the log prefix up to it. The base (`snapshot_index`/`snapshot_term`) is written to `raft.state` before the entries are removed, so a crash in between leaves redundant entries the loader skips, never a log shorter than the base claims. A restart resumes on the base instead of replaying from index 1. Tested by the simulator (compaction plus restart, a compaction-safety invariant, a leader refusing a follower it cannot serve), by the in-process three-node cluster (image written, prefix dropped, catalog generation and grants recovered from the file) and by the qualification campaign | `crates/carolina-consensus/src/{lib,storage,sim}.rs`, `crates/carolina-node/src/core.rs` |
| Snapshot transfer to a voter that has fallen behind the base, and dynamic membership | ⬜ Compaction is bounded by the slowest voter, so this cannot happen by lag alone; a voter whose durable state is lost must be rebuilt from outside, and the leader counts the refusals (`followers_behind_snapshot`) instead of looping |
| QI verdict | Q: `QI-CATALOG` PASS (CAT-01/02/05/12/15/16 + closed-never-reopens); `QI-CODEC-CORPUS` PASS (52 vectors / 23 kinds; kinds of disabled features are listed as not frozen); `QI-SECURITY` NOT_RUN → gate QI NOT_RUN |
| Q3 verdict | Q: gate **`Q3-C5` PASS** for the single-IDC C5 slice (`Q3-CONSENSUS-SIM` + `Q3-C5-PROCESS` + `QI-CATALOG` + FM-2 model within bounds); gate Q3 (C1/C2/C3 slices) NOT_RUN |

## MVP-4 … MVP-8

⬜ Not started. Multi-IDC publication, C1/C2, C3, evolution and C4 do not exist; their FM
*models* pass within bounds (see above) but no protocol implementation, deterministic simulator
adapter or real-process campaign exists for them.

## Test evidence (this tree)

`cargo test --workspace` on 2026-09-13 (rustc 1.89.0 MSRV, Windows 11): 181 tests, all passing — core 25, lang 32 (27 unit + 5 semantic validation), compiler 22 (16 unit + 6 footprint A05), wire 7, storage 23 (4 unit + 4 B+Tree differential + 3 crash matrix/epoch + 12 kernel), runtime 13 (1 unit + 1 invariant scope + 11 end-to-end), qualify 17 (4 unit incl. the codec corpus + 8 acceptance incl. one quick campaign + 5 bundle integrity), models 4, consensus 10 (incl. compaction plus restart and a leader refusing a follower behind its base), catalog 3, node 19 (11 unit incl. seed identity, poison recovery, a three-node in-process cluster and the snapshot/compaction path + 7 configuration + 1 three real processes), cli 6 (2 unit + 4 CLI); 0 failed, 0 ignored.
`cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean;
`tools/spec_lint.py` PASS (16 files). The three-process campaign and the qualification acceptance
suite run real processes and real fsync and take about two minutes together.

## Release artifacts

| Item | Status |
|---|---|
| `LICENSE` (Apache-2.0, as declared in `Cargo.toml`; owner to confirm) | ✅ file present |
| `.github/workflows/ci.yml` (RustSec audit, spec lint, fmt, clippy `-D warnings`, tests, quick qualification campaign, four libFuzzer smoke targets; Rust 1.89 + stable on Linux + Windows) | 🟡 file present; the public run for base commit `cdfca6a` **failed at `cargo fmt --check`** (later steps skipped). Reformatted in this tree; rerun pending |
| `SECURITY.md` | ✅ |
| `docs/BUILD.md` (build, test, qualification campaign, three-node cluster, troubleshooting) | ✅ |
| SBOM, signed releases, reproducible-build configuration, release manifest | ⬜ |

## Known deviations and decisions

- Fact records are declared `RECORD name IMMUTABLE { … }` (grammar extension for SPEC-003 §6 `EmitFact` targets).
- `RESOURCE` is a module item binding `available`/`reserved` fields and the reservation record (SPEC-003 §6 `ResourceDecl`).
- `REQUIRES op(params) NEEDS Fact(params)`: the fact primary key is built from the listed operation parameters.
- `authority_requirements` may be omitted in `CONTRACT` and then equals the empty list; all other contract fields are mandatory.
- Multiplication is restricted to the affine fragment (one operand is a constant), per SPEC-003 §4.
- Storage persists dirty pages copy-on-write at checkpoint (shadow paging): a dirty page is never rewritten in place and never evicted; the checkpoint rewrites dirty pages under fresh ids bottom-up and publishes the new root through the manifest. Sibling pointers are unused (scans re-seek by key and find the right neighbour through the parent path). Internal separators are full `(key, seq)` positions. Values above 1024 bytes go to immutable overflow chains.
- A `CompiledBatch` carries at most one mutation per logical key (the final per-key effect of one execution); duplicates are refused at `verify()`.
- The first durable `TxnStatusRecord` of a prepared transaction is its decision (INSTALLED/TERMINAL or ABORTED under revision 1): the PREPARED phase lives only in the journal, and the status phase machine accepts any phase as the first durable record.
- Abort of a prepared transaction consumes a local commit sequence (its ABORTED status is installed like a protocol-only batch) so that recovery replays it deterministically.
- The local RequestHome advances its epoch on every open instead of scanning bindings for the highest allocated sequence; a crashed allocation therefore never collides with a new one.
- Rows are stored as canonical `row.v1` values (primary key + typed fields) under `Namespace::User || RecordId || key`; the runtime loads the closure records of an operation in full before evaluation (correctness-first; no partial footprint loading yet).
- New registered record kind `request_home_state` (SPEC-012 owner) for the home's durable epoch/allocation state.
- `LICENSE` holds the Apache-2.0 text because `Cargo.toml` declares that license; the owner has not confirmed the choice.
- The qualification runner's fault model is process kill / short write inside one process (real files, real fsync); it is labelled deterministic because the engine consults no OS time, randomness or threads, and `Q2-DETERMINISM` re-runs a crashing schedule and compares trace digests.
- `carolina qualify` claims only gates Q0, Q1, Q2, FM and Q3-C5; every other gate is reported NOT_RUN and makes the exit code non-zero only when claimed.
- Ordered execution is recorded as two log entries per request (`Admit`, then the admitting node's `Decision` with the receipt digest); the v1 reference path keeps the explicit protocol even though one process holds several roles (SPEC-008 §8).
- Wire `MessageKind`s 12–14 (`Consensus`, `Admin`, `AdminReply`) were added before the codec manifest freeze; they are never accepted on client-role connections.
- The node advertises and accepts only the `DEV_LOCAL` security profile; TLS/mTLS (SPEC-013 §3) needs a maintained TLS implementation, which is a dependency decision left to the owner.
- Voters resume from `node.snapshot` and replay only the entries after it; engine steps stay idempotent by request binding, so replaying the tail never re-executes a decided request. The image is node-local durable state like `raft.state`: it has no registered record kind, because SPEC-012 owns the wire registry and snapshot transfer between nodes is not implemented.
- Typed decoding is strict by construction: `Canonical::decode` re-projects the decoded value to canonical form and requires equality with the input, so unknown fields and lossy projections are refused by every decoder (found by the codec corpus: `ResolveReplyV1` had accepted unknown fields).
- Registered record kinds whose features are disabled (snapshots, C1/C2/C3/C4, evolution, session tokens) have no codec and are reported as not frozen; enabling such a feature requires freezing its vectors first (SPEC-014 §5).
- `Cargo.toml` declares `rust-version = "1.89"`: the portable one-writer lock uses `std::fs::File::try_lock`, stable since Rust 1.89.
- `rust-toolchain.toml` pins local builds to `1.89.0`; CI runs both `1.89.0` and the current stable toolchain on Linux and Windows.
- Four coverage-guided fuzz harnesses type-check under nightly Rust: DSL frontend, canonical artifacts, wire/snapshot protocol and storage formats. The local Windows linker cannot execute the libFuzzer binary because its sanitizer-coverage start/stop symbols are unavailable; the Linux CI smoke run remains the execution gate.
- `cargo audit --file Cargo.lock --no-fetch` scanned 25 locked dependencies against 1216 locally cached RustSec advisories with no vulnerability finding; CI refreshes the advisory database.
- `python tools/run_tlc.py` verified the pinned TLA+ tools v1.8.0 jar and completed all bounded models without an error: FM-1 14,644 generated/3,268 distinct states, FM-2 1,049/348 and FM-3 13,469/2,816. CI repeats the same checksum-verified model check.
- Recovery now classifies an undersized declared journal frame as corruption instead of subtracting the trailer width and panicking; the regression drives `Journal::open` with the malformed on-disk frame.
- The node connection registry recovers a poisoned mutex without a second daemon panic; a regression poisons the registry, registers a connection and delivers a reply.
- A duplicate `commit` of a transaction that already has a durable decision is a typed refusal (SPEC-002 §69), not a second install; the runtime never issues one (retries resolve by `RequestKey`), the guard exists so the kernel boundary holds on its own.
- Key/value caps are enforced at admission, before the journal append, so a refused batch leaves nothing for recovery to replay.
- `CandidateRejection.operation_name` is rendered as `name@version`.
- The `DEV_LOCAL` transport refuses non-loopback addresses on both ends; it remains an unauthenticated test profile (SECURITY.md).
