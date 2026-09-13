# Formal model targets (SPEC-010 §16)

| Gate | Model | Executed checker | TLA+ source |
|---|---|---|---|
| FM-1 | Escrow transfer / authority / rights conservation (SPEC-006 §8, §11, §16) | `crates/carolina-models/src/fm1.rs` | `FM1_Escrow.tla` + `.cfg` |
| FM-2 | C5 decision, install, publication, completion (SPEC-008 §10, §11, §15) | `crates/carolina-models/src/fm2.rs` | `FM2_Decision.tla` + `.cfg` |
| FM-3 | Migration, fencing, plan evolution (SPEC-009 §7–§8, SPEC-011 §4/§7) | `crates/carolina-models/src/fm3.rs` | `FM3_Migration.tla` + `.cfg` |

## What is executed

`carolina qualify` runs the Rust explicit-state checkers: an exhaustive breadth-first
exploration of every reachable state under the stated bounds, checking the invariants in every
state, plus the mandatory negative controls (a deliberately broken variant of each model that
MUST produce a counterexample trace). The verdict, the state/transition counts, the bounds and
the control results are written into the campaign verdict; `cargo test -p carolina-models`
runs the same models.

The bounds are explicit and small (FM-1: total 3, two transfers; FM-2: two participants, two
leader epochs; FM-3: two sources, two workers, at most two old-generation admissions). A PASS is
bounded evidence for the model, not a proof of the unbounded protocol and not evidence about the
Rust implementation of those protocols — which does not exist yet (SPEC-014 MVP-3+). Gates Q3–Q6
therefore stay `NOT_RUN` even when FM-1/2/3 pass.

## TLA+

The `.tla` modules state the same machines and invariants for TLC. TLC has not been run in this
repository (no Java/TLA+ toolchain recorded). When it is, record the TLC version, the `.cfg`, the
number of states and the result next to the run; until then these files are documentation of the
model and the Rust checker is the evidence.

```text
java -jar tla2tools.jar -config FM1_Escrow.cfg FM1_Escrow.tla
```

## Refinement

Implementation events map to model transitions as follows once the protocols exist:
SPEC-006 `PREPARE_TRANSFER/ACCEPT/COMMIT/INSTALL/ABORT` ↔ FM-1 `DonorPrepare/ReceiverAccept/
DonorCommit/ReceiverInstall/DonorAbort`; SPEC-008 `TxnBegin/PrepareVote/DecisionCertificate/
Installed/PublicationCertificate/PublishSeen/CompletionCertificate` ↔ FM-2 `VoteYes/DecideCommit/
Install/IssuePublicationCertificate/Publish/IssueCompletionCertificate`; SPEC-009 phases ↔ FM-3
`phase`. An implementation optimization absent from a model requires a refinement mapping or
stays disabled for the claim (SPEC-010 §16).
