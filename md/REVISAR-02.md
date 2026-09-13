# REVISAR-02 — Auditoria de Consistência e Plano de Correção do CarolinaDB

**Projeto:** CarolinaDB  
**Repositório:** `JoseRFJuniorLLMs/CarolinaDB`  
**Data da auditoria:** 2026-09-09  
**Status:** correções arquiteturais pendentes antes do início da implementação distribuída  
**Atualização 2026-09-12:** as caixas abaixo foram reavaliadas contra a árvore de código (auditoria por requisito em [docs/AUDIT.md](../docs/AUDIT.md); evidência de testes em [docs/STATUS.md](../docs/STATUS.md)). Uma caixa marcada `[x]` tem código *e* teste/lint na árvore; uma nota *parcial* explica o que falta. As secções não anotadas (P2, §7, §17, §18, §20) continuam por fazer.  
**Objetivo:** eliminar contradições normativas entre SPECs, congelar os contratos transversais e criar uma sequência de implementação segura para agentes de IA e desenvolvedores humanos.

---

## 0. Resumo executivo

O CarolinaDB evoluiu significativamente em relação à arquitetura anterior. As revisões atuais da `SPEC-001`, `SPEC-002`, `SPEC-005`, `SPEC-006`, `SPEC-007`, `SPEC-008` e as novas `SPEC-011`, `SPEC-012` e `SPEC-013` resolveram quase todos os problemas P0 identificados na auditoria anterior:

- a tese não afirma mais existir um protocolo globalmente “mais fraco”;
- C0–C5 não são tratados como uma ordem total ou lattice;
- a contribuição científica foi reposicionada para **observable refinement** sob composição, evolução, falha e transferência de autoridade;
- a taxonomia de identities/generations/epochs foi formalizada;
- `RequestKey -> RequestHome -> TxnId` foi definido sem dependência circular;
- `CompiledBatch` agora carrega identidade, contrato, plano, IDC bindings e resultado terminal;
- C2 v1 foi corretamente restringido a um único replication group;
- authority durability, transfer-decision durability e client-result durability foram separados;
- o Control Plane ganhou uma SPEC própria;
- request lifecycle, wire format e retry semantics ganharam uma SPEC própria;
- segurança ganhou um threat/trust model normativo.

O conjunto, porém, ainda **não deve ser considerado congelado**. Três blocos permanecem inconsistentes:

1. `SPEC-009` ainda usa a taxonomia antiga (`IdcEpoch`, `idc_epochs[]`, `source_epoch`) e um schema próprio de `FinalReceipt`.
2. `SPEC-010` ainda não incorpora `SPEC-011`–`SPEC-013`, não contém os gates formais `FM-1`, `FM-2`, `FM-3` já referenciados por outras SPECs e usa referências antigas de seções.
3. `SPEC-014` é citada como dona da ordem de implementação, mas não existe.

Além disso, há trabalho de limpeza transversal: nomenclatura `AstraDB`/`CarolinaDB`, README, aliases obsoletos, referências cruzadas e um lint normativo entre SPECs.

**Decisão:** corrigir `SPEC-009`, corrigir `SPEC-010`, criar `SPEC-014`, executar uma limpeza transversal e só então declarar `ARCHITECTURE FREEZE 0.1`.

---

# 1. Prioridades

## P0 — bloqueia implementação distribuída

- [x] Reescrever `SPEC-009` para a taxonomia normativa da `SPEC-011`. *(SPEC-009 Draft 0.2 usa `IdcBinding`/`AuthorityBinding`; `tools/spec_lint.py` PASS.)*
- [x] Eliminar `IdcEpoch` de registros novos/persistentes/wire. *(lint check 1; `carolina-core/src/ids.rs` só define os tipos nominais.)*
- [x] Eliminar schemas duplicados de `FinalReceipt`. *(lint check 2; único dono SPEC-012 §7, codec em `carolina-wire`.)*
- [x] Alinhar migration records aos tipos `IdcBinding`, `IdcGeneration`, `IdcAuthorityEpoch`, `HolderAuthorityEpoch`, `RequestHomeEpoch`, etc. *(SPEC-009 §3; tipos com testes em `carolina-core/src/ids.rs`; os structs `MigrationRecord`/`CloseCertificate` ainda não existem em código — MVP-7.)*
- [x] Atualizar `SPEC-010` para depender também de `SPEC-011`, `SPEC-012`, `SPEC-013` e `SPEC-014`. *(cabeçalho da SPEC-010 v0.2.)*
- [x] Criar os gates formais `FM-1`, `FM-2`, `FM-3`. *(SPEC-010 §16; checkers explícitos em `crates/carolina-models` com controlos negativos, PASS dentro dos limites declarados; fontes TLA+ em `models/`, TLC não executado.)*
- [x] Criar `SPEC-014 — Implementation Profile & Vertical Slice`. *(`md/SPEC-014.md`.)*
- [x] Garantir que nenhuma SPEC normativa use `authority_epoch: u64`, `idcs: Vec<(IdcId,u64)>` ou alias equivalente. *(lint check 1, job `spec-lint` no CI.)*
- [x] Garantir que todo request path use `RequestKey -> RequestHome -> BindIfAbsent -> TxnId`. *(`crates/carolina-runtime/src/home.rs` local e replicada; testes `tests/local_slice.rs`, campanha C5-021 em três processos.)*
- [x] Garantir que nenhum runtime possa criar nova identidade quando o resultado anterior é `OutcomeUnknown`. *(`engine.rs` só resolve por `RequestKey`; regra Q-C05 do checker e controlo negativo QA-02 "unknown treated as abort".)*

## P1 — deve ser resolvido antes do primeiro release técnico

- [x] Normalizar nome do projeto: CarolinaDB x Astra. *(lint check 6; `astra.*` mantido só como domínio de hash/protocolo.)*
- [x] Atualizar README. *(secção Status reescrita em 2026-09-12.)*
- [x] Criar matriz de propriedade normativa por SPEC. *(`md/SPEC-OWNERSHIP.md`, usada pelo lint.)*
- [x] Criar lint/check automatizado para referências cruzadas e tipos proibidos. *(`tools/spec_lint.py`, 7 verificações, job de CI.)*
- [ ] Congelar os manifests de codec da `SPEC-012`. *(parcial: `fixtures/codec` congela 52 vetores de 23 kinds habilitados com 208 negativos — `QI-CODEC-CORPUS` PASS; 17 kinds registados de funcionalidades desativadas não têm codec nem vetor; o `CodecManifest` campo-a-campo da SPEC-012 §12 não existe.)*
- [x] Tornar qualification de security/control-plane/wire explícita. *(gates `QI-SECURITY` = NOT_RUN, `QI-CATALOG` = PASS, `QI-CODEC-CORPUS` = PASS em `carolina qualify`; a porta QI só abre com segurança implementada.)*
- [x] Fixar nomes definitivos dos crates e CLI. *(`carolina-*`; binários `carolina` e `carolina-node`.)*

## P2 — pode ocorrer após o primeiro vertical slice

- [ ] Adapter experimental sobre PostgreSQL para experimento E5.
- [ ] Melhorias de ergonomia da DSL.
- [ ] Otimizações de storage.
- [ ] C4 refinado/otimizado.
- [ ] Protocolos cross-group mais avançados.
- [ ] Reconfiguração dinâmica de membership.
- [ ] Perfis criptográficos adicionais.

---

# 2. SPEC-009 v0.2 — Plan Evolution

## 2.1 Problema

A `SPEC-009` ainda representa a geração/autoridade de um IDC através de:

```text
IdcId, IdcEpoch
```

e ainda usa estruturas como:

```text
source_idc_epochs[]
target_idc_epochs[]
source_epoch
idc_epochs[]
```

Isso contradiz a `SPEC-011`, que define tipos distintos:

```text
IdcGeneration
IdcAuthorityEpoch
PlacementEpoch
MembershipGeneration
ResourceGeneration
EscrowEpoch
HolderAuthorityEpoch
RequestHomeEpoch
StorageEpoch
OriginEpoch
```

A `SPEC-011` estabelece ainda que `IdcEpoch` **não é um tipo normativo** e não deve aparecer em novos registros persistentes ou wire.

## 2.2 Correção obrigatória

Substituir toda identidade ambígua por tipos nominais.

### Substituir

```text
IdcId, IdcEpoch
```

por:

```text
IdcBinding {
    idc_id: IdcId
    idc_generation: IdcGeneration
    authority_epoch: IdcAuthorityEpoch
}
```

### Substituir

```text
source_idc_epochs[]
target_idc_epochs[]
```

por:

```text
source_idc_bindings: SortedSet<IdcBinding>
target_idc_bindings: SortedSet<IdcBinding>
```

Quando a migration altera a definição semântica do IDC:

```text
IdcGeneration -> IdcGeneration'
```

Quando altera autoridade:

```text
IdcAuthorityEpoch -> IdcAuthorityEpoch'
```

Quando altera apenas placement:

```text
PlacementEpoch -> PlacementEpoch'
```

Essas mudanças não devem ser inferidas umas das outras.

## 2.3 MigrationRecord

Reescrever o schema conceitual para algo equivalente a:

```text
MigrationRecordV2 {
    migration_id: MigrationId
    record_version: u32
    phase: MigrationPhase
    expected_catalog_generation: CatalogGeneration

    source_plans: SortedSet<PlanRef>
    target_plans: SortedSet<PlanRef>

    source_idc_bindings: SortedSet<IdcBinding>
    target_idc_bindings: SortedSet<IdcBinding>

    source_placements: SortedSet<PlacementBinding>
    target_placements: SortedSet<PlacementBinding>

    source_memberships: SortedSet<ReplicationMembershipBinding>
    target_memberships: SortedSet<ReplicationMembershipBinding>

    affected_authorities: SortedSet<AuthorityBinding>

    boundary_digest: Hash
    participant_manifest: ParticipantManifest

    transform_hash: Hash
    transition_contract_hash: ContractHash

    close_certificates: SortedSet<CloseCertificateRef>
    drain_digest: Hash
    target_install_certificates: SortedSet<InstallCertificateRef>

    activation_decision_ref: ProtocolRecordRef
    retirement_frontiers: SortedSet<RetirementFrontier>
}
```

Nenhum campo deve utilizar um `u64` nu para representar uma geração/epoch sem um tipo nominal.

## 2.4 CloseCertificate

O schema atual possui `source_epoch`.

Trocar por:

```text
CloseCertificateV2 {
    migration_id: MigrationId
    authority: AuthorityBinding
    durable_fence_ref: ProtocolRecordRef
    admitted_request_frontier: RequestFrontier
    committed_origin_frontier_with_holes: OriginCoverage
    pending_txn_digest: Hash
    rights_and_transfer_digest: Optional<Hash>
    final_result_frontier: ResultFrontier
    causal_frontier: Optional<CausalFrontier>
    close_record_ref: ProtocolRecordRef
}
```

Assim a mesma estrutura pode fechar IDC authority, escrow holder authority e request home authority sem inventar `source_epoch: u64`.

## 2.5 FinalReceipt

Remover qualquer schema próprio de `FinalReceipt { ... }` da `SPEC-009`.

Substituir por `FinalReceiptV1` **exatamente como definido na `SPEC-012`**.

Regra normativa:

```text
SPEC-012 is the sole owner of FinalReceiptV1.
Other SPECs may reference it but MUST NOT redefine it.
```

## 2.6 Request identities durante migration

A `SPEC-009` deve passar a usar:

```text
RequestKey
RequestHash
TxnId
RequestHomeEpoch
PlanRef
```

Regra:

```text
A request admitted before migration preserves:
RequestKey
TxnId
RequestHash
admitted PlanRef
original decision authority
exact terminal outcome
```

Nunca:

```text
retry old request -> allocate new TxnId under target plan
```

## 2.7 Session migration

A seção de session/read token deve declarar explicitamente:

```text
C2 v1 is replication-group scoped.
```

Uma migration que muda replication group só pode:

1. preservar o grupo e seu significado;
2. fornecer mapping qualificado da frontier;
3. retornar `SessionFrontierUnavailable`.

Nunca deve “converter” um token tomando apenas máximos de sequence.

## 2.8 Resource migration

Para C3, toda migration deve referenciar os tipos concretos:

```text
ResourceRef
ResourceGeneration
EscrowEpoch
HolderAuthorityEpoch
TransferId
ReservationId
```

A mudança de `IdcAuthorityEpoch` não implica mudança de `EscrowEpoch`.

A mudança de `HolderAuthorityEpoch` não cria novos rights.

## 2.9 Critérios de aceite da SPEC-009 v0.2

- [x] Zero ocorrências normativas de `IdcEpoch`. *(lint)*
- [x] Zero `source_epoch: u64`. *(lint)*
- [x] Zero `idc_epochs[]`. *(lint)*
- [x] Zero schema duplicado de `FinalReceipt`. *(lint)*
- [x] Todo participant usa `IdcBinding`. *(SPEC-009 §3/§5.)*
- [x] Toda authority usa `AuthorityBinding`. *(SPEC-009 §3/§5.)*
- [x] Todo request histórico mantém `RequestKey/RequestHash/TxnId`. *(SPEC-009 §9; no runtime só dentro de uma geração — a migração em si é MVP-7.)*
- [x] Migration de C3 preserva `ResourceGeneration/EscrowEpoch/HolderAuthorityEpoch`. *(SPEC-009 §3; C3 não implementado.)*
- [x] Migration de session explicita single-group C2. *(SPEC-009 §10.)*
- [ ] Todos os records são encodáveis pela `SPEC-012`. *(`migration_record` e `close_certificate` só estão registados por nome; sem codec nem vetor.)*
- [x] Toda activation exige evidência registrada pela `SPEC-011`. *(texto da SPEC-009 §7; a activação em runtime é MVP-7.)*
- [x] Toda autenticação/evidence segue `SPEC-013`. *(texto; SPEC-013 não implementada.)*

---

# 3. SPEC-010 v0.2 — Qualification

## 3.1 Problema

A `SPEC-010` ainda foi escrita como se a arquitetura terminasse em `SPEC-009`.

Ela precisa qualificar também:

- `SPEC-011` — Control Plane e authority registry;
- `SPEC-012` — request identity/client/wire;
- `SPEC-013` — security/trust;
- `SPEC-014` — implementation profile.

Além disso, `SPEC-006` já referencia `FM-1`, mas esse gate ainda não existe.

## 3.2 Atualizar dependências

Novo header conceitual:

```text
Depends on: SPEC-001 through SPEC-014,
with capability-specific qualification according to implementation milestone.
```

A SPEC-010 não deve exigir funcionalidades ainda não implementadas em milestones anteriores, mas deve possuir os gates registrados.

---

# 4. Formal-model gates

## FM-1 — Escrow, Authority Transfer & Rights Conservation

Modelar:

```text
T = C + H + U + X
```

Estados:

```text
holder authority
reservation
transfer donor
transfer receiver
authority replacement
crash/recovery
migration fence
```

Schedules mínimos:

```text
PREPARE_TRANSFER
ACCEPT_TRANSFER
COMMIT_TRANSFER
INSTALL_TRANSFER
ABORT_TRANSFER
```

com duplicate messages, reordered messages, crash after every durable boundary, donor loss, receiver loss, authority replacement, stale replica, partition e migration closure.

Invariantes:

```text
no duplicated U
no fabricated T
no double install
no refund after COMMITTED
no late PREPARE resurrection
no spend by passive replica
no spend after authority close
```

Gate:

```text
FM-1 PASS
+
deterministic simulator PASS
+
real-process fault campaign PASS
```

antes de qualificar:

```text
C3 RequiredDurableCopies
C3 QuorumDurable transfer decisions
automatic authority failover
```

## FM-2 — C4/C5 Decision, Install & Publication

Modelar:

```text
TxnBegin
PrepareVote
DecisionCertificate
Installed
PublicationCertificate
PublishSeen
CompletionCertificate
```

Invariantes:

```text
at most one final decision
COMMIT requires all required YES votes
ABORT cannot replace COMMIT
prepared data remains invisible
no partial public transaction
no final success before required completion
stale authority cannot decide
participant cannot infer abort from timeout
```

Schedules:

- coordinator crash;
- leader failover;
- one participant loses install;
- duplicate prepare;
- duplicate commit;
- delayed publication;
- migration during IN_DOUBT;
- GC attempt during prepared transaction.

Gate obrigatório antes de:

```text
distributed atomic C4
distributed atomic C5
multi-IDC atomic publication
```

## FM-3 — Migration, Fencing & Plan Evolution

Modelar:

```text
PROPOSED
COMPILED
CLOSING
DRAINING
VALIDATING
INSTALLING
READY_TO_ACTIVATE
ACTIVE
RETIRED
BLOCKED
```

Invariantes:

```text
no overlapping incompatible admitting authorities
closed authority never reopens under same epoch
old accepted result remains resolvable
offline holder blocks unsafe replacement
no late packet creates new authority
activation requires complete close/drain/install evidence
```

Schedules:

- migration coordinator crash;
- duplicate workers;
- source authority offline;
- source comes back after activation;
- old committed packet arrives late;
- target install incomplete;
- target process restarts;
- RequestHome migration;
- escrow holder migration;
- IDC split/merge.

Gate obrigatório antes de:

```text
plan evolution
authority transfer
IDC split/merge
C3 -> C5
C5 -> C3
C1/C2 -> ordered
```

---

# 5. Qualification de SPEC-011 — Control Plane

Adicionar testes obrigatórios:

- [x] linearizable catalog CAS; *(CAS com revisões/digests sobre o log Raft; testes de `carolina-catalog` e `QI-CATALOG`.)*
- [x] duplicate `AdminRequestId`; *(CAT-02.)*
- [x] same ID + different command -> `IdentityConflict`; *(catálogo; desde 2026-09-12 o node também responde `IdentityConflict` a um pedido admin cujo hash difere do comando comprometido.)*
- [x] overlapping migration locks; *(CAT-01, predicado sem phantoms.)*
- [ ] stale follower absence cannot prove unlock; *(CAT-06 sem teste; leituras só no líder após read barrier.)*
- [x] leader failover retains command results; *(campanha de três processos: o novo líder retoma o bootstrap com ids admin idempotentes; CAT-02.)*
- [ ] compacted watches require fresh snapshot/barrier; *(watches e compaction inexistentes; o log é reproduzido desde o índice 1.)*
- [ ] catalog unavailable + valid `PINNED_OFFLINE` grant; *(não implementado.)*
- [ ] policy update cannot revoke disconnected writer sem closure; *(CAT-04 só no modelo FM-3.)*
- [x] retired authority never returns ACTIVE; *(closed-never-reopens e CAT-16 em `QI-CATALOG`.)*
- [ ] capability removal cannot invalidate active grant silently; *(CAT-07; matriz de capacidades inexistente.)*
- [x] stale node cannot self-authorize from cached catalog; *(C5-009: seguidor/minoria recusa mutações; a admissão é reverificada em ordem de log em cada voter.)*
- [ ] catalog restore does not fabricate semantic authority. *(CAT-09; sem snapshot/restore de catálogo.)*

---

# 6. Qualification de SPEC-012 — Client, Request & Encoding

## Request identity tests

- [x] same RequestKey + same content -> same TxnId; *(`reserve_release_receipts_are_exact_and_idempotent`.)*
- [x] same RequestKey + changed args -> `RequestIdentityMismatch`; *(`changed_content_under_one_key_is_an_identity_mismatch`; C5-010.)*
- [x] lost first response -> resolution by RequestKey; *(`crash_after_commit_before_reply_resolves_to_the_identical_receipt`; C5-021.)*
- [x] crash after BindIfAbsent -> no duplicate allocation; *(`crash_after_allocation_reexecutes_under_the_same_txn_id`.)*
- [ ] moving RequestHome preserves bindings; *(parcial: o sucessor Raft continua a mesma home e o contador (C5-021); não há transferência de home entre autoridades — MVP-7.)*
- [ ] old home cannot allocate after closure; *(não há closure de home; um seguidor/minoria recusa alocar, C5-009.)*
- [x] `OutcomeUnknown` never causes auto-new request; *(engine + Q-C05/QA-02.)*
- [x] `ResultExpired` never permits reexecution; *(`result_eviction_and_namespace_retirement_survive_restart`; Q-C13.)*
- [x] retired namespace never permits reuse. *(mesmo teste; `IdentityExpired`.)*

## Wire tests

- [x] canonical reencoding byte-identical; *(`Canonical::decode` re-projeta e compara; o corpus `QI-CODEC-CORPUS` exige ponto fixo.)*
- [x] reject duplicate fields; *(`canon.rs` teste `roundtrip_and_reject_noncanonical`.)*
- [x] reject unknown mandatory semantics; *(`registry.rs decode_checked` fail-closed; `unknown_fields_rejected`; 208 vetores negativos.)*
- [x] reject integer overflow; *(inteiros como strings decimais com verificação de largura, teste `canonical_ints`.)*
- [x] reject noncanonical integer/hex forms; *(`canonical_ints`, `roundtrip_and_reject_noncanonical`; `hex_decode` estrito.)*
- [x] reject oversized lengths before allocation; *(`Limits` aplicados antes da alocação; teste `limits_enforced`.)*
- [ ] golden vectors for every normative record; *(parcial: só os 23 kinds habilitados.)*
- [ ] cross-platform fixtures; *(parcial: CI Linux+Windows configurado; o run público de `c98e984` falhou em `fmt --check` antes dos testes.)*
- [ ] snapshot corruption; *(parcial: `manifest_validates_chunks`; não há export/import de snapshot.)*
- [x] unsupported codec negotiation; *(`negotiation_downgrade_protection`.)*
- [x] downgrade attempt. *(idem, incluindo cluster estrangeiro e DEV_LOCAL vs perfil de produção.)*

---

# 7. Qualification de SPEC-013 — Security

Adicionar campanha:

```text
SEC-1 Authentication
SEC-2 Authorization
SEC-3 Replay
SEC-4 Downgrade
SEC-5 Credential rotation
SEC-6 Revocation
SEC-7 Tenant isolation
SEC-8 Backup/restore
SEC-9 Authority evidence
SEC-10 Parser/DoS limits
```

Casos mínimos:

- wrong tenant credential;
- client tries consensus RPC;
- passive replica forges escrow decision;
- C1 relay submits C5 publication certificate;
- stale/revoked certificate;
- credential rotation during retry;
- revoked principal resolving old request;
- modified COSE payload;
- valid signature with wrong cluster;
- valid signature with wrong purpose;
- token from G1 used at G2;
- downgrade from supported secure profile;
- malformed X.509/COSE/record payload;
- encrypted backup with wrong manifest signer;
- restore from another cluster;
- backup containing rows but missing protocol metadata.

---

# 8. QualificationManifest v2

Adicionar:

```text
QualificationManifestV2 {
    manifest_version
    source_revision
    working_tree_digest
    build_profile
    toolchain_id
    dependency_lock_digest

    storage_format_versions
    compiler_rule_versions
    protocol_versions

    catalog_protocol_version
    request_protocol_version
    wire_protocol_version
    snapshot_protocol_version
    security_profile_id

    supported_contract_fragment
    enabled_capabilities

    operation_definitions
    schema_hashes
    plan_hashes

    authority_durability_profiles
    transfer_decision_durability_profiles
    client_result_durability_profiles

    failure_assumptions
    topology
    authority_configuration
    placement
    membership

    formal_model_artifact_hashes
    formal_model_tool_versions

    resource_limits
    timeout_and_retry_policy

    seed_set
    schedules
    checker_versions
    run_budget

    workload_definitions
    baseline_configs
}
```

---

# 9. SPEC-014 — Implementation Profile & Vertical Slice

Criar:

```text
md/SPEC-014.md
```

Título:

```text
SPEC-014 — Implementation Profile & Vertical Slice
```

Objetivo: impedir que agentes implementem protocolos fora de ordem ou confundam “schema existe” com “capability está pronta”.

## 9.1 Regra principal

```text
A milestone MAY depend only on capabilities whose mandatory
qualification gates from all previous milestones are PASS.
```

Documentar:

```text
present in code != implemented capability
implemented capability != qualified capability
qualified capability != production-ready capability
```

## 9.2 Milestone P0 — Semantic Core

Implementar:

```text
RECORD
INDEX
INVARIANT
OPERATION
typed AST
IR
canonical normalization
reference interpreter
artifact hashes
golden fixtures
EXPLAIN skeleton
```

Gate:

```text
IR golden tests PASS
reference interpreter PASS
codec fixtures PASS
```

## 9.3 Milestone P1 — Local Storage & Request Identity

Implementar:

```text
B+Tree
buffer pool
MVCC
Commit Journal
CompiledBatch
ProtocolMutation
RequestBinding
TxnStatusRecord
FinalReceipt persistence
crash recovery
```

Gate:

```text
storage crash matrix PASS
request identity matrix PASS
result replay byte-identical
```

## 9.4 Milestone P2 — Control Plane

Implementar:

```text
3-node fixed Raft
CatalogGeneration
artifact registry
PlanRef
IdcBinding
AuthorityBinding
RequestRoute
RequestHome registry
Node capability registry
security bootstrap
```

Gate:

```text
catalog fault matrix PASS
identity taxonomy lint PASS
security bootstrap PASS
```

## 9.5 Milestone P3 — Single-IDC C5

Implementar:

```text
one serial IDC authority
SerialPosition
root X gate
deterministic evaluator
exact result
read barrier
leader failover
```

Gate:

```text
single-IDC C5 history checker PASS
leader failover PASS
exact result retry PASS
```

## 9.6 Milestone P4 — Distributed C5

Implementar:

```text
TxnBegin
prepare
unique decision
install
publication
completion
SnapshotCut
coherent distributed reads
```

Gate:

```text
FM-2 PASS
distributed fault campaign PASS
Q-C11 PASS
```

## 9.7 Milestone P5 — C1/C2

Implementar:

```text
SemanticCommitV1
OriginId
Dot
CausalContextV1
outbox/inbox
dedupe
anti-entropy
single-group sessions
bootstrap
```

Gate:

```text
C1 convergence/invariant PASS
C2 causal/session PASS
cross-group requests rejected correctly
```

## 9.8 Milestone P6 — C3 Escrow

Implementar:

```text
ResourceRef
HolderRef
U/H/X/C/T accounting
reservation state
rights transfer
authority durability profiles
transfer decision durability profiles
```

Gate:

```text
FM-1 PASS
escrow deterministic campaign PASS
real-process escrow fault campaign PASS
```

## 9.9 Milestone P7 — Plan Evolution

Implementar `SPEC-009 v0.2`.

Gate:

```text
FM-3 PASS
migration fault matrix PASS
request/result retention PASS
C3 <-> C5 transition cases PASS
```

## 9.10 Milestone P8 — C4

Somente agora implementar:

```text
SnapshotCut speculation
PointReadToken
RangeReadToken
predicate revisions
certifier
durable reservations
C4 integration with SPEC-008
```

Gate:

```text
FM-2 relevant extensions PASS
C4 fault campaign PASS
equivalent-contract benchmark against C5 published
```

Se C4 não demonstrar vantagem útil, manter capability experimental/desabilitada.

---

# 10. Nomenclatura CarolinaDB / Astra

## 10.1 Problema

Ainda existem:

```text
AstraDB
astra-runtime
astra-storage
astra.*
```

espalhados pelas SPECs e research.

## 10.2 Decisão recomendada

Renomear tudo que seja produto/implementação:

```text
CarolinaDB
carolina-runtime
carolina-storage
carolina-cli
```

Quanto aos hash/protocol domains, escolher uma das duas opções:

### Opção A — limpar tudo agora

```text
carolina.*
```

### Opção B — manter Astra como codename interno permanente

Declarar normativamente:

```text
Product name: CarolinaDB
Permanent protocol codename: Astra
```

e manter apenas `astra.*` para protocol/hash domains explicitamente documentados.

Não deixar `AstraDB` como nome de produto em SPECs novas.

## 10.3 Checklist de rename

- [ ] `SPEC-002` título.
- [ ] todas as ocorrências de `AstraDB SHALL`.
- [ ] `astra-runtime`.
- [ ] `astra-storage`.
- [ ] CLI.
- [ ] research docs.
- [ ] proposta de pesquisa.
- [ ] diagramas.
- [ ] exemplos.
- [ ] artifact domains, conforme decisão.
- [ ] README.

---

# 11. README.md

O README deve conter:

## What it is

```text
CarolinaDB is a research database runtime that compiles versioned
observable operation contracts into qualified execution plans.
```

## What it is not

```text
not a released production DBMS
not a generic automatic consistency oracle
not a claim of a globally weakest coordination protocol
not Byzantine-fault tolerant
not yet benchmark-qualified
```

## Research hypothesis

```text
observable refinement
heterogeneous plans
authority transfer
evolution
recovery
```

## Current status

```text
Specifications: active
Implementation: not started / early
Qualification: not executed
Benchmarks: no claims
```

## Architecture

```text
DSL/Contracts
 -> IR
 -> Compiler
 -> Catalog/Control Plane
 -> C0/C1/C2/C3/C4/C5 Runtime
 -> Storage Kernel
 -> Qualification
```

## SPEC index

Listar `SPEC-001` a `SPEC-014`.

## Research

Links internos para:

```text
PROPOSTA-DE-PESQUISA.md
research/consistency-prior-art.md
```

## No performance claims

Declarar explicitamente que targets ainda não são resultados.

---

# 12. Ownership matrix

Criar `SPEC-OWNERSHIP.md` ou seção equivalente.

| Contract | Owner |
|---|---|
| thesis / architecture | SPEC-001 |
| storage / MVCC / journal / CompiledBatch | SPEC-002 |
| DSL / IR / canonical compiler artifacts | SPEC-003 |
| compiler analysis / plan selection | SPEC-004 |
| C1/C2 | SPEC-005 |
| C3 | SPEC-006 |
| C4 | SPEC-007 |
| C5 / decision / publication | SPEC-008 |
| plan evolution | SPEC-009 |
| qualification | SPEC-010 |
| catalog / identities / authority registry | SPEC-011 |
| request / client / wire / snapshots / receipts | SPEC-012 |
| security / trust | SPEC-013 |
| implementation sequencing | SPEC-014 |

Regra:

```text
A non-owner SPEC may summarize a contract but MUST NOT define an alternate schema.
```

---

# 13. Cross-SPEC lint

Criar:

```text
tools/spec_lint.py
```

ou equivalente.

Ele deve procurar:

## Tipos proibidos

```text
IdcEpoch
authority_epoch: u64
idcs: Vec<(IdcId, u64)>
source_epoch: u64
idc_epochs[]
```

## Schemas duplicados proibidos

```text
FinalReceipt {
RequestBinding {
IdcBinding {
```

fora do owner autorizado.

## Referências inexistentes

Exemplo:

```text
SPEC-010 FM-1
SPEC-014
```

deve falhar se o alvo não existir.

## References de section

Detectar links para headings/seções removidas.

## Owner violations

Exemplo:

```text
SPEC-009 defines FinalReceipt
```

-> FAIL.

---

# 14. Normative glossary

Adicionar, preferencialmente em `SPEC-011` ou arquivo comum:

```text
RequestKey
RequestHash
TxnId
PlanRef
IdcBinding
AuthorityBinding
ResourceRef
HolderRef
OriginId
ProtocolRecordRef
FinalReceiptV1
ObservationTokenV1
```

Cada termo deve ter:

```text
owner
scope
reuse rule
comparison rule
persistence rule
wire owner
```

---

# 15. CompiledBatch final review

Verificar antes do freeze:

```rust
CompiledBatch {
    batch_version
    txn_id
    request_key
    request_hash
    operation
    operation_hash
    contract_hash
    schema_hash
    plan
    idc_bindings
    consistency_class
    origin
    semantic_evidence
    captured_inputs
    mutations
    protocol_mutations
    terminal_outcome
    semantic_digest
}
```

Checklist:

- [x] `RequestKey` sempre presente para business invocation. *(`CompiledBatch.request_key`; `verify()`.)*
- [x] internal protocol batches usam `ProtocolOnlyBatch`. *(`home.rs`, `engine.rs`.)*
- [x] nenhuma fase interna inventa `StableRequestId`. *(a identidade vem só do cliente/`make_invoke`.)*
- [x] `terminal_outcome` não aparece em prepare. *(`kernel.rs prepare` / `batch.rs`.)*
- [ ] composite transaction não cria receipt no participant local. *(MVP-4; transacções compostas inexistentes.)*
- [x] `semantic_digest` exclui evidência posterior que dependa dele. *(`CompiledBatch::semantic_payload`.)*
- [x] `FinalReceiptV1` é byte-idêntico no retry. *(testes locais e campanha C5-010.)*
- [x] CAS de `TxnStatusRecord` é obrigatório. *(`ExpectedRecordRevision`; `protocol_cas_and_status_transitions`.)*
- [x] nenhuma atualização unchecked de protocol state. *(idem; SPEC-002 §69 recusa commit duplicado de txn decidida.)*

---

# 16. C2 single-group contract

Manter explicitamente:

```text
C2 v1 session guarantees are scoped to one ReplicationGroupId.
```

Testar:

```text
write G1
write G2
read G3
```

deve retornar `SessionScopeMismatch` ou exigir composite plan.

Nunca combinar frontiers de grupos distintos tomando apenas máximo de sequence.

---

# 17. Authority durability

Manter:

```text
client_result_durability
authority_durability
transfer_decision_durability
```

Adicionar à qualification:

- [ ] perda do único copy de authority;
- [ ] perda do único donor decision;
- [ ] quorum decision disponível mas holder authority indisponível;
- [ ] receipt local estável + transfer quorum durable;
- [ ] rights nunca são recriados a partir de business balance.

---

# 18. Security profile freeze

Antes do wire/client code:

- [ ] confirmar TLS 1.3;
- [ ] confirmar mTLS role mapping;
- [ ] fixar URI SAN format;
- [ ] fixar COSE profile;
- [ ] fixar Ed25519 profile;
- [ ] definir key IDs;
- [ ] definir rotation;
- [ ] definir retained verification keys;
- [ ] definir tenant separation;
- [ ] definir audit event format;
- [ ] definir encrypted backup profile;
- [ ] definir unsupported threat model.

Evitar criptografia própria.

---

# 19. Canonical protocol codec freeze

A `SPEC-012` deve possuir manifest enumerando cada record:

```text
record_kind
record_version
required fields
optional fields
field types
numeric widths
collection ordering
maximum size
hash domain
canonical fixture
negative fixture
```

Nenhuma ocorrência de:

```text
...
implementation-defined
serde default
unknown ignored field
```

em schemas normativos v1.

---

# 20. Architecture Freeze 0.1

Só declarar freeze quando:

```text
SPEC-001 v0.2 aligned
SPEC-002 v0.2 aligned
SPEC-003 current
SPEC-004 current
SPEC-005 v0.2 aligned
SPEC-006 v0.2 aligned
SPEC-007 v0.2 aligned
SPEC-008 v0.2 aligned
SPEC-009 v0.2 DONE
SPEC-010 v0.2 DONE
SPEC-011 DONE
SPEC-012 DONE
SPEC-013 DONE
SPEC-014 DONE
```

e:

```text
spec_lint PASS
no dangling normative references
no duplicate schema owners
no forbidden epoch aliases
README updated
```

Criar:

```text
ARCHITECTURE-FREEZE-0.1.md
```

contendo:

```text
commit SHA
SPEC hashes
ownership matrix hash
protocol codec manifest hash
known unsupported capabilities
```

---

# 21. Ordem recomendada de execução

Executar nesta sequência:

```text
1. SPEC-009 v0.2
2. SPEC-010 v0.2
3. SPEC-014
4. cross-SPEC lint
5. ownership matrix
6. rename cleanup
7. README
8. protocol codec manifest review
9. architecture freeze
10. implementation P0
```

Não começar C4, C3 ou distributed C5 antes de concluir esses itens.

---

# 22. Não alterar

As seguintes decisões estão corretas e não devem ser revertidas:

- não existe “globally weakest protocol”;
- C0–C5 não formam uma total order;
- C0–C5 não são um mathematical lattice;
- C5 não torna uma operação inválida magicamente segura;
- C2 v1 é group-scoped;
- `OutcomeUnknown` não significa abort;
- timeout não significa abort;
- offline authority não é revogada por incremento de catalog generation;
- business balance não é spend authority;
- receipt final é histórico e imutável;
- authority durability é diferente de client result durability;
- hashes não autenticam authority;
- crash-fault não equivale a Byzantine tolerance;
- local `VersionStamp`/`JournalLsn` não é distributed order;
- prepared data não pode vazar para public reads;
- migration deve preservar compromissos já emitidos;
- C4 deve ser implementado por último;
- benchmark precisa comparar contratos equivalentes;
- `INCONCLUSIVE != PASS`;
- `NOT_RUN != PASS`.

---

# 23. Critério final de “pronto para implementar”

O CarolinaDB estará arquiteturalmente pronto para o primeiro vertical slice quando:

```text
SPEC-009 v0.2 completed
AND
SPEC-010 v0.2 completed
AND
SPEC-014 completed
AND
spec_lint PASS
AND
ownership matrix consistent
AND
README describes actual project status
AND
no normative IdcEpoch remains
AND
no duplicate FinalReceipt schema remains
AND
no missing normative reference remains
```

A partir daí:

```text
DSL
-> Typed IR
-> Reference Interpreter
-> Canonical Artifacts
-> Local Storage
-> Request Identity
-> C5 single IDC
```

---

# 24. Veredito

O CarolinaDB já possui uma base conceitual suficientemente forte para justificar implementação.

As correções restantes não exigem repensar a tese do banco. Elas são principalmente de:

```text
normative consistency
type ownership
qualification closure
implementation sequencing
repository hygiene
```

O principal objetivo desta revisão é impedir que a implementação introduza novamente ambiguidades já resolvidas no desenho.

**Depois de `SPEC-009 v0.2`, `SPEC-010 v0.2` e `SPEC-014`, a arquitetura deve ser congelada e o foco deve mudar de criar novas SPECs para produzir código, modelos formais, fault campaigns e evidência experimental.**
