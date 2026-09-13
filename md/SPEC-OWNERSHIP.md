# SPEC-OWNERSHIP — Normative ownership matrix

**Status:** Normative index, maintained with the specifications
**Date:** 2026-09-11
**Rule:** A non-owner SPEC may summarize a contract but MUST NOT define an alternate schema. Restatements are permitted only where listed below and must be marked as such in the restating SPEC. `tools/spec_lint.py` enforces this table.

## 1. Contract ownership

| Contract | Owner | Notes |
|---|---|---|
| Thesis, architecture, consistency families, MVP boundary | [SPEC-001](SPEC-001.md) | Explanatory examples only; no alternate schemas |
| Storage: pages, B+Tree, MVCC, Commit Journal, `CompiledBatch`, `ProtocolMutation`, `TxnStatusRecord`, `DurableStorageKernel` | [SPEC-002](SPEC-002.md) | Physical key/page/journal formats |
| DSL, typed AST, `ModuleIR`, `OperationIR`, `InvariantIR`, `EffectIR`, `ContractIR`, canonical IR JSON, hash domains `astra.schema/operation/invariant/contract/module.v1` | [SPEC-003](SPEC-003.md) | Scalar canonical rules reused by SPEC-012 |
| Compiler analysis, `ExecutionProfile`, `OperationPlan`, `ConsistencyCertificate`, EXPLAIN, domains `astra.plan.v1`, `astra.certificate.v1` | [SPEC-004](SPEC-004.md) | |
| C1/C2: `OriginId` use, `Dot`, `CausalContextV1`, `SemanticCommitV1`, `SessionTokenV1` payload | [SPEC-005](SPEC-005.md) | `OriginId` layout is SPEC-002 §102 |
| C3: `ResourceRef`, `HolderRef`, `EscrowPlanV1`, `HolderStateV1`, `TransferTermsV1`, `TransferDecisionV1`, conservation equation | [SPEC-006](SPEC-006.md) | |
| C4: `CertificationContext`, `PointReadToken`, `RangeReadToken`, `CertificationRecord` | [SPEC-007](SPEC-007.md) | Decision/publication records are SPEC-008 |
| C5 and distributed decision/publication: `IdcAuthority`, `SerialCommand`, `TxnBegin`, `ParticipantDescriptor`, `PrepareVote`, `DecisionCertificate`, `Installed`, `PublicationCertificate`, `PublishSeen`, `CompletionCertificate`, `SnapshotCut` | [SPEC-008](SPEC-008.md) | |
| Plan evolution: `MigrationRecord`, `CloseCertificate`, migration state machine, obligations E1–E7 | [SPEC-009](SPEC-009.md) | Uses `FinalReceiptV1` from SPEC-012 without redefining it |
| Qualification: manifest, verdicts, history checker Q-C01–14, fault matrix Q-F01–22, gates Q0–Q7/QI, formal gates FM-1/FM-2/FM-3 | [SPEC-010](SPEC-010.md) | |
| Catalog, identity taxonomy, `IdcBinding`, `PlanRef`, `AuthorityBinding`, `AuthorityGrant`, `CatalogCommand`, catalog snapshots | [SPEC-011](SPEC-011.md) | `RequestKey` restated from SPEC-012 (allowed copy) |
| Request identity, `RequestKey`, `RequestContentV1`, `RequestBindingV1`, `TxnId` layout, `ClientReplyV1`, `ResolveRequestV1`, `ReadContractV1`, `ObservationTokenV1`, `AcceptedResultV1`, `FinalReceiptV1`, wire envelope, negotiation, snapshots, codec manifest, domains `astra.request/result/receipt/protocol-record/snapshot-chunk/snapshot-manifest/negotiation.v1` | [SPEC-012](SPEC-012.md) | Sole owner of `FinalReceiptV1` |
| Security: principals, `CredentialRecord`, `SecurityPolicy`, TLS/mTLS profile, `SignedPayloadV1` COSE wrapper, backup profiles, audit | [SPEC-013](SPEC-013.md) | |
| Implementation sequencing MVP-0–MVP-8 | [SPEC-014](SPEC-014.md) | Sole owner of stage order and claims |

## 2. Allowed restatements

| Schema | Owner | Restated in | Why |
|---|---|---|---|
| `IdcBinding` | SPEC-011 | SPEC-001 §55 | Foundational text shows the binding as an explanatory summary and names SPEC-011 as owner |
| `RequestKey` | SPEC-012 | SPEC-011 §2 | Taxonomy table lists the composite key next to its component types; SPEC-011 names SPEC-012 as allocation owner |

## 3. Naming policy

| Name | Status |
|---|---|
| `CarolinaDB` | Product and database name |
| `carolina-*` | Crate names (`carolina-core`, `carolina-lang`, `carolina-compiler`, `carolina-storage`, `carolina-wire`, `carolina-runtime`, `carolina-consensus`, `carolina-catalog`, `carolina-qual`, `carolina-models`, `carolina-cli`, `carolina-server`) |
| `carolina` | CLI binary |
| `astra.*` | Permanent protocol codename for hash domains (byte-stable) |
| `ASTR`, `astra://`, `.astr`, `.astj` | Wire magic, URI SAN scheme, data/journal file extensions |
| `AstraDB`, `astra-*`, `astra <cmd>` | Forbidden; lint fails |

## 4. Forbidden identity aliases

`IdcEpoch`, `authority_epoch: u64`, `idcs: Vec<(IdcId, u64)>`, `source_epoch: u64`, `idc_epochs[]`, `source_idc_epochs`, `target_idc_epochs`. The nominal taxonomy of SPEC-011 §2 is the only accepted form.

## 5. Glossary of cross-cutting terms

| Term | Owner | Scope | Reuse rule | Comparison rule | Persistence rule | Wire owner |
|---|---|---|---|---|---|---|
| `RequestKey` | SPEC-012 | tenant + namespace + stable request id | never reused after retirement | byte equality of all three 128-bit parts | retained with binding/tombstone | SPEC-012 |
| `RequestHash` | SPEC-012 | one request content | immutable | 32-byte equality | with binding | SPEC-012 |
| `TxnId` | SPEC-012 | home id + allocation epoch + allocation seq | never reused | 32-byte equality | with binding and every batch | SPEC-012 |
| `PlanRef` | SPEC-011 | plan lineage | immutable per generation | `(PlanId, PlanGeneration, PlanHash)` all equal | with every batch/receipt | SPEC-012 |
| `IdcBinding` | SPEC-011 | one IDC definition + authority incarnation | new binding on generation/epoch change | all three fields equal; no scalar comparison across bindings | with every batch/receipt | SPEC-012 |
| `AuthorityBinding` | SPEC-011 | tagged IDC/holder/home authority | closed binding never reopens | tag + all fields | grants, fences, certificates | SPEC-012 |
| `ResourceRef` | SPEC-006 | invariant + resource key + resource generation | new ref on regeneration | all fields | escrow ledger | SPEC-012 |
| `HolderRef` | SPEC-006 | holder id + holder authority epoch | new epoch on replacement | both fields | escrow ledger | SPEC-012 |
| `OriginId` | SPEC-002 §102 | node + origin epoch + origin seq | never reused | all fields | semantic commits, receipts | SPEC-012 |
| `ProtocolRecordRef` | SPEC-012 | kind + version + key + payload hash | immutable | all fields | evidence lists | SPEC-012 |
| `FinalReceiptV1` | SPEC-012 | one terminal invocation | byte-identical on every retry | canonical bytes | at RequestHome | SPEC-012 |
| `ObservationTokenV1` | SPEC-012 | one scope (group/certified/serial/composite) | scope-bound | kind + payload | client-held, server-verified | SPEC-012 |
