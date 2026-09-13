# SPEC-014 — Implementation Profile & Vertical Slice

**Status:** Draft 0.1 — implementation roadmap; no completed milestone or qualification is claimed
**Implementation status (2026-09-12):** Stage status: MVP-0, MVP-1 and MVP-2 have their exit evidence (Q0, Q1, Q2 PASS; §4 MVP-2 schedules tested); MVP-3 is implemented for the `DEV_LOCAL` loopback profile with Q3-C5, QI-CATALOG and QI-CODEC-CORPUS PASS, but its exit criteria are NOT met because SPEC-013 mTLS/authorization is absent (gate QI NOT_RUN); MVP-4…MVP-8 not started (their FM models pass within bounds only). §5 codec vectors are frozen for enabled kinds only; no benchmark or research result (§6) exists. Table: [docs/AUDIT.md](../docs/AUDIT.md).
**Date:** 2026-09-09
**Depends on:** [SPEC-001](SPEC-001.md)–[SPEC-013](SPEC-013.md)
**Evidence owner:** [SPEC-010](SPEC-010.md)
**Normative terms:** MUST, MUST NOT, SHOULD and MAY state requirements.

## 1. Purpose

This document owns implementation sequencing. Numbered SPECs define contracts, not the order in which every protocol must be built. Historical SPEC-001 M0–M10 and SPEC-002 S0–S12 remain work-package identifiers; the MVP stages below determine integration order and permitted claims.

The first useful slice is a deterministic interpreter and conservative C5 compiler feeding an exact local durable boundary, followed by a fixed three-node catalog and single-IDC C5. Multi-IDC publication comes next; C1/C2 and C3 follow; evolution precedes optional C4. No stage is complete merely because structs, RPC handlers or this specification exist.

Native B+Tree storage remains the selected experimental engine. The research evaluation remains independent of that choice through SPEC-002's `DurableStorageKernel`. Existing EVA/NietzscheDB and shared HeraclitusDB services are never destructive-test targets. A reference backend cannot claim durability without its declared storage fault model.

## 2. Initial distributed profile

Use three isolated processes and data directories, a fixed three-voter SPEC-011 catalog and fixed three-voter C5 authority. Roles may share a test process but retain distinct state machines, durable records and recovery boundaries. Three processes on one machine test process failures; they do not demonstrate machine/region fault tolerance.

Use finite integer/decimal types, explicit operation contracts, registered canonical artifacts, synthetic tenants and namespaces, and an immutable topology/codec manifest. C2 later supports one group per session. Dynamic consensus membership, leases, arbitrary user code, external effects, a SQL frontend and implicit cross-group C2 sessions are disabled.

Client durability, authority durability and transfer-decision durability are independent manifest inputs. The environment must realize every advertised failure domain. Three labels for one disk do not constitute three durable failure domains.

## 3. Stages and exit evidence

| Stage | Deliverable | Required exit evidence and enabled scope |
| --- | --- | --- |
| MVP-0 — Semantic core | SPEC-003 parser, typed IR, reference interpreter, deterministic normalization and canonical artifacts | IR0/IR1 and relevant Q0; explicit results/observations/session scope; positive and negative fixtures; no distributed claim |
| MVP-1 — Conservative compiler | SPEC-004 closure, C5-only candidate generation, checker and EXPLAIN | CC0 and C5 portion of CC1; unsafe/incomplete scopes rejected; canonical plans with obligations. Candidates remain inactive until runtime qualification |
| MVP-2 — Local durable slice | SPEC-002 B+Tree/MVCC/journal/checkpoint, complete CompiledBatch, request CAS/deduplication, exact results and tombstones | Required S0–S8 and Q1/Q2; crash before/after identity binding, decision and result publication; local RequestHome with an explicitly local grant, no mocked distributed durability |
| MVP-3 — Catalog and single-IDC C5 | SPEC-011 catalog; SPEC-012 RPC/codec negotiation and home allocation; SPEC-013 mTLS/authorization; SPEC-008 ordered authority | Applicable QI, single-IDC Q3 and FM-2 subset, deterministic and real-process faults; one final outcome after leader/client failure; no multi-IDC claim |
| MVP-4 — Atomic publication | SPEC-008 sealed participants, prepare/decision/install/publication/completion and coherent reads | Full enabled FM-2 and Q4; install/publication crash schedules; no fractured promised reads; durable prepare and retained gates |
| MVP-5 — C1/C2 | SPEC-005 semantic replication, dotted group frontiers, group sessions and catch-up | C1/C2 Q3 plus affected Q4; C12 acceptance corpus; reject unsupported cross-group sessions |
| MVP-6 — C3 escrow | SPEC-006 conserved allocation, exclusive holders, business/rights atomicity, transfer and independent durability policies | FM-1, C3 Q3 and affected Q4; ESC corpus including permanent evidence loss/frozen rights; no unqualified failover claim |
| MVP-7 — Evolution | SPEC-009 close/drain/transform/install/activate, preserved receipts/tokens, SPEC-011 locks and RequestHome handoff | FM-3 and Q5; EV-01–16 for enabled families; offline issuers block unsafe replacement; no incompatible overlapping admission |
| MVP-8 — Optional C4 | SPEC-007 point/absence/range/predicate evidence, durable reservations and publication | Expanded FM-2, Q6 and affected Q4/Q5; qualified C1/C2/C3/C5 first; compare with C5; C4 may remain disabled |

Q0–Q7 and QI are SPEC-010 gates. Each stage enables only features whose applicable checks pass. Future-family cases may be NOT_APPLICABLE only with those capabilities disabled and rationale recorded. Missing required evidence is NOT_RUN or INCONCLUSIVE, never a waived PASS. Later changes rerun affected earlier integration gates.

The interpreter may precede the native engine but cannot replace independent durable recovery evidence. Catalog Raft does not establish runtime C5 authority or multi-IDC atomicity; those are different state machines.

## 4. First end-to-end workload

Use one inventory key and an explicit receipt-returning reserve/release contract from SPEC-003. Declare checked quantities, nonnegative available/reserved values, reservation identity, supply conservation, whole-invocation atomicity and exact rejection meaning. Initially serialize through C5 even if the contract later admits escrow.

```text
declared operation -> typed IR -> closed obligations -> qualified C5 plan
-> authenticated RequestKey -> home durable mapping and plan assignment
-> ordered invariant/effect/result evaluation -> atomic CompiledBatch
-> required decision/publication durability -> persisted FinalReceiptV1
-> lost response -> ResolveRequest -> identical receipt after restart
```

Schedules include concurrent reservation of the last unit, duplicate release, overflow, changed content under one key, crash after allocation, crash after commit before reply, result eviction and retired namespace after restore. MVP-4 adds reserve-plus-debit in two IDCs and a crash after only one installs. MVP-5 adds immutable group-local facts and a rejected cross-group dependency. MVP-6 compares conserved rights at the same receipt contract. MVP-7 changes capacity without erasing accepted reservations.

The independent oracle observes every final reply and promised read. Measure useful progress/starvation separately; refusing every request does not satisfy usefulness.

## 5. Identity, format and infrastructure gates

Before MVP-3 freeze executable schemas/fixtures for every enabled request, binding, receipt, token, certificate and protocol kind under SPEC-012. Include field names, widths, enums, lengths, hash domains, malformed and downgrade cases, snapshots and restore. SPEC-012's header-only negative example is not a complete codec corpus.

Before persistent compatibility claims, publish a content-addressed codec manifest and independent golden byte/digest evidence on supported architectures. Throwaway local formats may evolve explicitly; they must not be called compatible. Typed generations/epochs remain distinct at application interfaces even when underlying widths match.

Every active distributed plan binds operation/contract versions, template, authority and durability models, codecs, backend, security profile and qualification evidence. Missing capability rejects catalog activation; it cannot become a runtime TODO success branch. Local parser/storage development does not need a deployed catalog, but distributed admission requires applicable QI evidence first.

## 6. Research and storage comparison

SPEC-010 E1–E5 govern evaluation. Preserve results, read scope, finality, request retention, durability/failure domains and partition behavior across native storage and existing-storage adapters. An adapter without durable prepare cannot enter a multi-IDC comparison as an equivalent implementation.

Compare competent manual C5/escrow baselines and report where coordination wins. A B+Tree is an engineering choice, not novelty evidence. Equivalent benefit on existing storage favors reconsidering product form under SPEC-002 §180; record that counterpoint rather than weakening a baseline.

## 7. Deliverables and claims

Each completed stage retains source/dirty-tree identity, capability manifest, fixture/model hashes, campaign verdicts, failure bounds, unsupported cases and reproduction commands. Document commands as available only after implementation and verification.

Distributed correctness requires applicable model checking AND deterministic simulation AND real-process fault evidence. Model bounds remain explicit; these gates do not prove the whole implementation or novelty. Production claims also require evidence for the selected SPEC-013 protection/backup profile.

At a failed gate, retain the failure bundle and fix the affected contract/implementation before enabling that capability. Independent work on earlier local stages may continue. No benchmark numbers or proof claims may be invented to mark a milestone complete.
