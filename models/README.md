# Formal model targets (SPEC-010 §16)

| Gate | Model | Executed checker | TLA+ source | Unbounded proof |
|---|---|---|---|---|
| FM-1 | Escrow transfer / authority / rights conservation (SPEC-006 §8, §11, §16) | `crates/carolina-models/src/fm1.rs` | `FM1_Escrow.tla` + `.cfg` | `../lean/Carolina/Escrow.lean` |
| FM-2 | C5 decision, install, publication, completion (SPEC-008 §10, §11, §15) | `crates/carolina-models/src/fm2.rs` | `FM2_Decision.tla` + `.cfg` |
| FM-3 | Migration, fencing, plan evolution (SPEC-009 §7–§8, SPEC-011 §4/§7) | `crates/carolina-models/src/fm3.rs` | `FM3_Migration.tla` + `.cfg` |

## Machine-checked proofs (`lean/`)

The bounded checkers enumerate a finite slice; the Lean development proves the same invariants
for **arbitrary** parameters. FM-1 is done: `Carolina.Escrow.safety` establishes rights
conservation, `CreditImpliesCommit` and `NoRefundAfterCommit` for every total and every list of
transfer quantities, of any length. Run it with `python tools/run_lean.py`, which fails on a
build error, on `sorry` anywhere in the sources, and on any theorem whose axiom set reaches
beyond Lean's own foundations. The toolchain is pinned in `lean/lean-toolchain`; install it with
[elan](https://elan.lean-lang.org). Mathlib is deliberately not a dependency — the two list
lemmas the proofs need are proved in place.

Mechanising FM-1 made one thing explicit that the bounded runs never had to state: conservation
is **not inductive on its own**. It needs the coherence invariant "a message that was sent
agrees with the phase that sent it", because `ReceiverInstall` credits the receiver on the
strength of `commitSent` alone. TLC never had to name that, because it explores states rather
than arguing from a predecessor.

What this is **not**: a proof about the Rust implementation. Lean proves properties of a model,
and the refinement argument from `crates/` to these models remains open (`md/FALTA.md` item 11).
FM-2 and FM-3 are not yet mechanised.

## What is executed

`carolina qualify` runs the Rust explicit-state checkers: an exhaustive breadth-first
exploration of every reachable state under the stated bounds, checking the invariants in every
state, plus the mandatory negative controls (a deliberately broken variant of each model that
MUST produce a counterexample trace). The verdict, the state/transition counts, the bounds and
the control results are written into the campaign verdict; `cargo test -p carolina-models`
runs the same models.

The bounds are explicit and small (FM-1: total 3, two transfers; FM-2: two participants, two
leader epochs; FM-3: two sources, two workers, at most two old-generation admissions). FM-3 models
seven of the ten SPEC-009 phases. A PASS is bounded evidence for the model, not a proof of the
unbounded protocol and not evidence about the Rust implementation of those protocols.

Of the three, only the decision path of FM-2 has any implementation: the single-IDC C5 slice of
SPEC-008 §8 (MVP-3), which is a different machine (`Admit` then `Decision` over one Raft log, no
participants and no publication layer) with no recorded refinement mapping to the model. FM-1
(escrow) and FM-3 (migration) have no implementation at all. The claimed gate `Q3-C5` therefore
consumes FM-2 as *bounded model evidence* beside the deterministic simulator and the real
three-process campaign, never as a proof of the implemented slice; gates Q3 (C1/C2/C3), Q4, Q5 and
Q6 stay `NOT_RUN` even when FM-1/2/3 pass.

## TLA+

`python tools/run_tlc.py` downloads TLA+ tools v1.8.0 when absent, verifies its pinned SHA-256
digest and model-checks all three modules with Java 17 and one worker. Terminal states are valid
for these safety machines, so every `.cfg` explicitly sets `CHECK_DEADLOCK FALSE`; TLC still
explores the complete bounded state graph and checks `Safety` in every reachable state.

The recorded local run on 2026-09-13 completed without an error:

| Model | Generated states | Distinct states | Graph depth |
|---|---:|---:|---:|
| FM-1 | 14,644 | 3,268 | 19 |
| FM-2 | 1,049 | 348 | 21 |
| FM-3 | 13,469 | 2,816 | 20 |

Tool: TLA+ tools v1.8.0, TLC build `2026.09.12.025210` (`867aefb`); jar SHA-256
`db131ddb48e7004d823bef4493df7b35694babe37505b9d9fa5685e7a331f1f1`. CI repeats the same
checksum-verified run. These finite results remain bounded evidence rather than proof of the
unbounded protocols or their Rust implementations.

## Refinement

Implementation events map to model transitions as follows once the protocols exist:
SPEC-006 `PREPARE_TRANSFER/ACCEPT/COMMIT/INSTALL/ABORT` ↔ FM-1 `DonorPrepare/ReceiverAccept/
DonorCommit/ReceiverInstall/DonorAbort`; SPEC-008 `TxnBegin/PrepareVote/DecisionCertificate/
Installed/PublicationCertificate/PublishSeen/CompletionCertificate` ↔ FM-2 `VoteYes/DecideCommit/
Install/IssuePublicationCertificate/Publish/IssueCompletionCertificate`; SPEC-009 phases ↔ FM-3
`phase`. An implementation optimization absent from a model requires a refinement mapping or
stays disabled for the claim (SPEC-010 §16).
