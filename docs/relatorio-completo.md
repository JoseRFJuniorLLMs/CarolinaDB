# RELATÓRIO COMPLETO DE AUDITORIA RECURSIVA — CarolinaDB

**Repositório:** `JoseRFJuniorLLMs/CarolinaDB`

**Branch auditada:** `main`

**Commit funcional auditado:** `618cd0e42bd42ac5a9218343befd4a3581470b0b`

**Árvore auditada:** branch `main` após as correções consolidadas em 13/09/2026

**Data:** 13 de setembro de 2026
**Arquivo:** `relatorio-completo.md`

---

## 0. Estado verificado no fechamento desta auditoria

Esta seção é o registro mais recente e prevalece quando alguma observação histórica
das seções seguintes descreve um estado anterior da mesma árvore.

Validação local executada em Windows 11 com Rust 1.89.0, a MSRV fixada:

| Verificação | Resultado observado |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --workspace --all-targets --offline --target-dir target -- -D warnings` | PASS |
| `cargo test --workspace --offline --target-dir target` | PASS — 181 testes; 0 falhas; 0 ignorados; repetido na MSRV 1.89.0 |
| `python tools/test_spec_lint.py` | PASS — 2 testes |
| `python tools/spec_lint.py` | PASS — 16 arquivos; 0 erros; 0 warnings |
| `carolina qualify --quick` | PASS para todos os gates reivindicados |
| `cargo audit --file Cargo.lock --no-fetch` | PASS — 25 dependências contra 1.216 advisories em cache; nenhum achado |
| `cargo fuzz check` | PASS — quatro harnesses coverage-guided compilam; execução local bloqueada pelo linker do sanitizer no Windows e delegada ao CI Linux |
| `python tools/run_tlc.py` | PASS — FM-1/2/3; 3.268/348/2.816 estados distintos; nenhuma violação |
| GitHub Actions | PASS — [run 34777875153](https://github.com/JoseRFJuniorLLMs/CarolinaDB/actions/runs/34777875153); oito jobs; Rust 1.89.0 + stable em Linux + Windows |

Bundle retido da campanha rápida:

```text
target/qualification-msrv/final/local-quick/run-34dfbaf40a573267
manifest eefe377be0ff79d6f811f5f01028259d7ca3b2f51a01f616e3fc25ac31b519f3
trace    85731071253a5f21f901242413e009cb0f36e9cdb5ab7f33b1c03426cdbad078
```

Gates reivindicados e aprovados: **Q0, Q1, Q2, FM e Q3-C5**. O resultado
`overall PASS` significa que esse perfil e esses limites passaram. Não converte os
gates não reivindicados em sucesso.

| Gate não reivindicado | Estado | Motivo |
|---|---|---|
| QI | NOT_RUN | QI-SECURITY não executado; mTLS/autorização ausentes |
| Q3 | NOT_RUN | C1/C2/C3 não implementados |
| Q4 | NOT_RUN | publicação atômica multi-IDC não implementada |
| Q5 | NOT_RUN | evolução/migração não implementada |
| Q6 | NOT_RUN | C4 não implementado |
| Q7 | NOT_RUN | avaliação E1–E5 e baselines pendentes |

A auditoria recursiva encontrou e corrigiu falhas que a suíte anterior não exercitava:

- identidade de requisições C5 simultâneas com a mesma chave e conteúdo diferente;
- finalização autônoma de admissões herdadas após troca de líder;
- publicação de `ResolveRequest` somente depois da decisão replicada;
- preservação do hash correto em `OutcomeUnknown` e descarte de read barriers ao perder liderança;
- coalescência de comandos administrativos somente quando os bytes são idênticos;
- validação da autoridade e da geração carregadas pela entrada `Admit`;
- validação recursiva de tipos de chave e de retorno no frontend;
- conversão de falhas semânticas de invariantes em rejeições finais duráveis;
- avaliação somente da closure compilada de invariantes no runtime local;
- restrição integral do perfil `DEV_LOCAL` a endpoints loopback e três voters válidos;
- recusa de campanhas vazias, budgets zero, flags inválidas e replays inconclusivos;
- vínculo obrigatório entre manifest, verdict, schedule e history nos bundles;
- escrita concorrente de bundles em diretórios distintos, sem sobrescrever evidência;
- bloqueio do handle de storage depois de erro numa etapa durável até reabertura/recovery;
- preservação da revisão em retry idempotente de `PutRecord` com `Expected::Absent`;
- repetição segura de `OutcomeUnknown` com a mesma identidade na campanha C5 durante troca de líder.

O achado de identidade administrativa de `LocalEngine::load_rows` foi corrigido nesta
árvore: o hash agora cobre label, record, todas as chaves, nomes de campos e valores.
O nó coalesce somente comandos de seed com bytes idênticos, recusa conteúdo divergente
sob o mesmo label e mantém a máquina de estado saudável quando o conflito já entrou no
log. Há regressões para concorrência, retry exato, conflito e reinício.

O relatório está completo como avaliação da árvore. O CarolinaDB continua incompleto
como visão integral das SPECs e como produto de produção. A árvore auditada está
verde localmente e no CI público para o commit funcional consolidado.

---

## 1. Conclusão executiva

O CarolinaDB já ultrapassou claramente a fase de “coleção de SPECs”. A árvore atual contém um workspace Rust real com 12 crates, DSL própria, IR tipada, compilador conservador de consistência, storage com B+Tree/MVCC/journal/recovery, runtime local, protocolo wire, framework de qualification, modelos formais bounded, Raft, catálogo replicado, processo de nó TCP e CLI.

O projeto, porém, ainda deve ser classificado como **protótipo de pesquisa avançado**, e não como banco distribuído production-ready.

A leitura correta é:

- **MVP-0:** forte e substancialmente implementado.
- **MVP-1:** forte e substancialmente implementado para C0/C5.
- **MVP-2:** forte e executável, com storage local durável e boas campanhas de falha.
- **MVP-3:** existe uma vertical distribuída C5 single-IDC real, mas apenas no perfil `DEV_LOCAL`; o estágio formal não fecha porque SPEC-013 ainda não foi implementada.
- **MVP-4:** não implementado.
- **MVP-5:** não implementado.
- **MVP-6:** não implementado.
- **MVP-7:** não implementado.
- **MVP-8:** não implementado.
- **Segurança de produção:** ausente.
- **Operação de produção:** incompleta.
- **Release engineering:** incompleta.
- **Evidência pública de CI:** verde para o commit funcional `618cd0e`.

Portanto existem dois objetivos possíveis, que não devem ser confundidos:

### Objetivo A — CarolinaDB completo segundo a visão das SPECs

Ainda exige MVP-4 a MVP-8, além de segurança, qualification integral e avaliação científica E1–E5.

### Objetivo B — CarolinaDB C5 single-IDC production-ready

É muito mais próximo e não depende de implementar imediatamente C1/C2, C3, C4 ou multi-IDC. Para esse alvo, os bloqueadores reais são segurança, snapshot/restore, catch-up de réplica, observabilidade, concorrência/storage hardening, release engineering, CI, fuzzing, benchmarks e operação multi-host.

A recomendação deste relatório é **fechar primeiro um perfil C5 production-ready**, em vez de ampliar simultaneamente cinco famílias de protocolo. Fazer o contrário produziria uma quantidade heroica de superfície nova antes de estabilizar a superfície já boa.

---

# 2. Escopo e método da auditoria

A auditoria cobriu:

1. metadados do repositório e branch `main`;
2. workspace raiz e os 12 crates;
3. `README.md`;
4. `SECURITY.md`;
5. `docs/BUILD.md`;
6. `docs/STATUS.md`;
7. `docs/AUDIT.md`;
8. a auditoria narrativa anterior;
9. `md/FALTA.md`;
10. SPEC-001 a SPEC-014;
11. workflow `.github/workflows/ci.yml`;
12. execução pública mais recente do GitHub Actions;
13. logs dos jobs Linux/Windows;
14. buscas por `TODO`, `unimplemented!`, `panic!`, `.unwrap()`, TLS, fuzz, benchmarks, tracing, snapshots, registros futuros e artefatos de release;
15. inspeção pontual de código crítico em transport, consensus, catalog, storage e runtime;
16. comparação entre alegações documentais, estado do código e evidência pública.

A própria auditoria existente `docs/AUDIT.md` registra uma limitação importante: 29 dos 32
runs adversariais planejados foram concluídos. SPEC-013 não recebeu nenhuma das duas lentes e
SPEC-012 recebeu somente a lente de existência do código. Essas duas áreas têm a evidência mais fraca.

Este relatório usa aquela auditoria como insumo, mas não a trata como autoridade infalível.

---

# 3. Estado estrutural do repositório

O workspace declara 12 crates:

| Crate | Função principal |
|---|---|
| `carolina-core` | identidades, canonicalização, hashing, decimal, key codec |
| `carolina-lang` | DSL, parser, AST, lowering, IR, interpreter |
| `carolina-compiler` | closure, obligations, exploração, seleção de plano, certificados |
| `carolina-storage` | page store, B+Tree, MVCC, journal, checkpoint, recovery |
| `carolina-wire` | records, envelopes, negociação, codecs, snapshots |
| `carolina-runtime` | RequestHome, engine, execução e receipts |
| `carolina-qualify` | campaigns, oracle, checker, replay, minimizer |
| `carolina-models` | model checkers explicit-state |
| `carolina-consensus` | Raft e simulador |
| `carolina-catalog` | control plane/catalog state machine |
| `carolina-node` | processo distribuído e transporte TCP |
| `carolina-cli` | CLI |

Pontos positivos do workspace:

- `unsafe_code = "forbid"`;
- Clippy global;
- Apache-2.0;
- `Cargo.lock` versionado;
- dependências externas pequenas;
- separação razoável de responsabilidades;
- documentação normativa extensa;
- testes distribuídos reais com múltiplos processos;
- fault injection no storage;
- corpus golden e codec corpus.

Esse desenho é consideravelmente mais disciplinado do que o normal para um protótipo de pesquisa.

---

# 4. CI público verificado

O workflow atual contém:

- spec lint;
- self-test do spec lint;
- `cargo fmt --all -- --check`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo test --workspace`;
- `carolina qualify --quick`;
- TLC bounded para FM-1/FM-2/FM-3;
- RustSec;
- quatro smoke targets libFuzzer;
- Linux;
- Windows;
- upload de artefato de qualification.

A execução pública verificada para o commit funcional auditado é:

- **Run:** [34777875153](https://github.com/JoseRFJuniorLLMs/CarolinaDB/actions/runs/34777875153)
- **Commit:** `618cd0e42bd42ac5a9218343befd4a3581470b0b`
- **Resultado:** `success`

Todos os oito jobs passaram. As quatro combinações Rust 1.89.0/stable × Linux/Windows
executaram fmt, Clippy com warnings negados, a suíte do workspace e a campanha rápida.
O Linux também executou os quatro fuzzers; jobs independentes verificaram RustSec,
spec lint e os três modelos TLA+.

O run anterior `34777132566` revelou uma corrida na própria campanha C5: durante troca
de líder, `OutcomeUnknown` era tratado como resultado final pelo harness. O contrato wire
o classifica como repetível com a mesma identidade. O executor foi corrigido, passou três
vezes consecutivas localmente e depois passou nas quatro matrizes públicas.

Essa evidência fecha o bloqueador de CI da auditoria. Ela não muda os gates `NOT_RUN`,
nem transforma o protótipo em release candidate ou produto de produção.

---

# 5. Política de toolchain

Durante a auditoria, `docs/BUILD.md` dizia:

```text
Rust 1.85 or newer
```

enquanto o `Cargo.toml` raiz declarava:

```toml
rust-version = "1.89"
```

Além disso:

```toml
[toolchain]
channel = "stable"
```

não fixa uma versão exata de Rust.

O primeiro problema foi corrigido nesta árvore: `docs/BUILD.md` agora declara 1.89+
e explica que `File::try_lock` fixa o mínimo real. Permanecem dois pontos de política:

1. CI pode mudar de comportamento quando `stable` muda;
2. builds não são estritamente reproduzíveis em relação ao compilador.

Na execução pública auditada, o runner utilizou Rust 1.98.1.

### Recomendação

Escolher uma política explícita:

**Opção A — MSRV + stable**

- `rust-version = "1.89"`;
- CI testa `1.89` e `stable`;
- BUILD diz exatamente 1.89+.

**Opção B — toolchain congelado**

- `rust-toolchain.toml` fixa a versão exata;
- CI usa aquela versão;
- atualização de toolchain vira PR explícito.

Para um projeto que enfatiza determinismo, artefatos canônicos e qualification, a opção B é a mais coerente para releases.

---

# 6. MVP-0 — Semantic Core

## Estado

**Forte.**

Existem:

- identidades tipadas;
- canonical encoding;
- SHA-256 com domain separation;
- `Decimal(p,s)`;
- key codec ordenável;
- DSL restrita;
- parser;
- AST;
- lowering;
- IR;
- reference interpreter;
- counterexample explorer;
- golden fixtures.

A cobertura documental atual indica que problemas antigos de footprint foram ampliados em 13/09, incluindo:

- aliasing;
- phantom insertion;
- aggregate group movement;
- bound-source change;
- reference deletion;
- unique absence.

Também houve correção de parsing de aggregate bound com `GROUP BY`.

## O que falta

### P1 — campanhas longas de fuzzing

Quatro harnesses `cargo-fuzz`/libFuzzer agora cobrem o frontend DSL, decodificadores
canônicos/IR, wire/snapshot e formatos de storage. Todos compilam com nightly. A execução local
no Windows não inicia porque o linker não fornece os símbolos de início/fim de
sanitizer-coverage; o CI Linux executa 256 entradas por alvo. Ainda faltam corpus persistente,
campanhas longas retidas e orçamento/digestos registrados para qualificação de release.

### P1 — cobertura mensurável

Não foi encontrado pipeline de `cargo llvm-cov` ou equivalente.

Adicionar cobertura não “prova correção”, mas ajuda a detectar grandes regiões que a qualification não está exercitando.

### P2 — architecture freeze

`ARCHITECTURE-FREEZE-0.1.md` ainda não existe.

Não deve ser produzido antes de:

- CI verde;
- toolchain definido;
- codec manifest atual estável;
- decisão de security profile;
- snapshot/export contract fechado.

---

# 7. MVP-1 — Conservative Compiler

## Estado

**Forte para C0/C5.**

A arquitetura:

```text
semântica
→ closure
→ obligations
→ candidatos
→ prova/refutação/unknown
→ seleção conservadora
→ certificate
```

é coerente e é provavelmente a parte mais diferenciadora do projeto.

Existem:

- IDC templates;
- relações direcionadas;
- obligations;
- `Proven/Disproven/Unknown`;
- counterexamples;
- protocol library;
- artifact checker;
- certificate;
- EXPLAIN;
- CLI.

## O que falta

Segundo a própria árvore:

- síntese real para C3;
- certificação C4;
- atomicidade composta multi-IDC;
- `MigrationRequirement` completo;
- relation/refinement formal;
- golden EXPLAIN;
- simulação de planos mistos;
- `Range`/`Predicate` key sets;
- hypergraph exportado com razão/testemunha;
- regras completas de evolução de operação.

### Recomendação

Não expandir C1–C4 antes de congelar e endurecer a vertical C5.

O compiler já pode ser o produto de pesquisa mesmo com a library real inicialmente restrita a C0/C5, desde que isso esteja explicitamente documentado.

---

# 8. MVP-2 — Storage local

## Estado

**Tecnicamente forte para um protótipo.**

Existem:

- páginas;
- CRC32C;
- MANIFEST A/B;
- torn-tail handling;
- fault injection;
- B+Tree;
- MVCC;
- overflow;
- buffer pool;
- redo-first journal;
- checkpoint;
- prepared transactions;
- recovery;
- structural verify;
- MemKernel;
- differential testing;
- crash matrix.

### Correção importante em relação à auditoria anterior

A auditoria narrativa anterior dizia que MVCC GC e journal retention não existiam.

Isso ficou desatualizado.

A árvore de 13/09 registra:

- version reclamation no checkpoint;
- horizonte por oldest registered snapshot;
- prepared work pinning;
- retenção/truncagem de segmentos de journal;
- correção de bug em páginas dirty sob parent evicted.

Portanto o relatório atual **não classifica mais MVCC GC/journal retention como ausentes**.

---

# 9. O que ainda falta no storage

Este é um dos principais blocos de trabalho.

## P0/P1 — concorrência real

O kernel continua essencialmente single-threaded.

Faltam:

- latching;
- política clara de lock ordering;
- concorrência reader/writer;
- testes de race/deadlock;
- modelo de concorrência do buffer pool;
- contention benchmarks.

Sem isso, throughput do storage fica limitado e as garantias de thread safety ainda não são uma propriedade operacional amplamente testada.

## P1 — group commit

Existe durability mode, mas falta um mecanismo de group commit maduro para amortizar fsync.

## P1 — reutilização de páginas

Falta free-list/page allocator com reutilização.

O arquivo tende a crescer porque páginas liberadas/overflow reclamado não voltam plenamente ao pool de páginas reutilizáveis.

## P1 — compaction física

GC lógico e journal retention não substituem compactação física completa.

É preciso definir:

- page reuse;
- compaction;
- rewrite;
- thresholds;
- background/foreground policy;
- crash safety durante compaction.

## P1 — transaction object com read-your-writes

A própria SPEC-002 ainda exige objeto de transação com RYW.

## P1 — `ExpectedVersion`

CAS de chave de usuário ainda não está implementado de ponta a ponta.

Isso afeta a codificação de `CompiledBatch`, portanto deve ser fechado antes de congelar permanentemente o formato.

## P1 — read-set capture

Necessário especialmente para C4 e diagnósticos de serialização.

## P1 — journal consumers e dedupe semântico

Ainda faltam consumers formais e dedupe por `OriginId` para as famílias futuras.

## P1 — observabilidade do storage

Faltam:

- métricas;
- tracing;
- diagnósticos;
- inspect de page tree;
- inspect de journal;
- checkpoint stats;
- reclamation stats;
- fsync latency;
- cache hit/miss;
- page split/merge;
- corruption diagnostics.

## P1 — modelo formal específico do storage

Existem modelos para escrow/decision/migration, mas falta modelagem mais direta de invariantes do kernel de storage.

## P1 — benchmarks

Não há harness de benchmark maduro nem `benches/`/Criterion identificado.

Precisam existir pelo menos:

- point read;
- point write;
- batch;
- fsync;
- checkpoint;
- recovery;
- B+Tree split;
- hot key;
- MVCC scan;
- read snapshot;
- compaction/reclamation;
- group commit;
- contention.

---

# 10. Snapshot, export, import e backup

## Estado

**Parcial.**

Existem:

- `SnapshotManifestV1`;
- `SnapshotChunkV1`;
- canonical codecs;
- validation;
- golden vectors.

Mas ainda não existe um fluxo operacional completo:

```text
Store
→ export snapshot
→ chunks
→ persistent bundle
→ transfer/copy
→ import into empty Store
→ verify
→ reopen
→ same observable state
```

Além disso, a própria árvore registra um blocker normativo: os chunks carregam `CanonicalRecord`, mas ainda falta um record kind apropriado para linha física/lógica de storage.

## P0 para produção

Implementar:

1. export;
2. import;
3. checksum manifest;
4. incremental/full backup strategy;
5. restore para store vazio;
6. restore para versão compatível;
7. teste de corrupção;
8. teste de chunk ausente/reordenado;
9. retenção;
10. restore drill automatizado.

Sem restore validado, backup é apenas decoração administrativa.

## Segurança

Para produção ainda faltam:

- backup encryption;
- key management;
- rotação;
- manifest signing ou autenticação equivalente.

---

# 11. Consensus / Raft

## Estado

Existe Raft real com:

- membership fixa;
- persistent vote/term/log;
- quorum commit;
- read barrier;
- simulator;
- partitions;
- loss;
- duplication;
- crash/restart.

Também há três processos reais em loopback.

Isso é bom.

## Lacuna crítica: snapshot transfer

O código atual reconhece explicitamente:

```text
v1 has no snapshot transfer
```

Um voter que ficou para trás do snapshot do leader não consegue ser reparado pela replicação normal do log.

O comportamento atual é fail-closed, o que é correto para segurança, mas insuficiente para operação de longo prazo.

### P0/P1

Implementar:

- install snapshot;
- snapshot metadata;
- transfer chunking;
- resumable transfer;
- checksum;
- follower catch-up;
- atomic activation;
- restart during install;
- corrupted snapshot rejection;
- old snapshot rejection;
- membership/epoch binding.

## Catalog snapshot vs snapshot end-to-end

O catálogo possui funções de snapshot/restore de state machine, mas isso **não equivale** a um pipeline completo:

```text
catalog state
→ durable Raft snapshot
→ log compaction
→ transfer to lagging follower
→ install
→ resume replication
```

Esse pipeline ainda precisa ser fechado.

## Dynamic membership

Ainda não existe.

O perfil atual exige exatamente três voters.

Isso pode ser aceitável para v0.1, desde que seja assumido como limitação formal.

---

# 12. Multi-host real

A campanha de três processos usa:

- mesma máquina;
- loopback;
- diretórios separados.

Isso valida muita coisa, mas não valida:

- perda de host;
- NIC;
- latência real;
- clock differences;
- filesystem differences;
- network reorder fora do simulator;
- MTU;
- congestion;
- asymmetric packet loss real;
- TLS handshake;
- DNS/service discovery;
- machine reboot.

## P1

Criar uma campanha multi-host real, ainda que pequena:

- 3 VMs/containers em hosts distintos;
- kill -9;
- reboot;
- network partition;
- delay;
- packet loss;
- disk full;
- restart;
- follower rebuild por snapshot;
- leader replacement;
- restore from backup.

---

# 13. MVP-3 — C5 single-IDC

## Estado

A vertical é real.

Há:

- Raft;
- catalog;
- grants;
- routes;
- ordered `Admit`;
- deterministic execution;
- `Decision`;
- exact receipt;
- `ResolveRequest`;
- leader failover;
- replicated RequestHome;
- real-process campaign.

## Por que MVP-3 ainda não fecha

A própria SPEC-014 inclui SPEC-013 security no critério de saída.

Hoje:

```text
QI-CATALOG = PASS
QI-CODEC-CORPUS = PASS
QI-SECURITY = NOT_RUN
```

Logo:

```text
Q3-C5 = PASS para DEV_LOCAL
MVP-3 production profile = NÃO FECHADO
```

Essa distinção está corretamente presente no README atual.

---

# 14. Segurança — principal bloqueador de produção

`SECURITY.md` é honesto: o projeto é protótipo de pesquisa e não deve ser exposto a entrada não confiável.

O transporte atual:

- é plaintext;
- usa loopback;
- aceita roles declaradas;
- não autentica identidades.

O próprio `transport.rs` diz que role é declaração, não identidade autenticada.

## P0 — implementar SPEC-013 de verdade

Faltam no mínimo:

### Transporte

- TLS 1.3;
- mTLS node-to-node;
- client TLS;
- hostname/node identity validation;
- certificate rotation;
- expiry handling;
- revocation;
- downgrade prevention.

### Identidade

- node principal;
- admin principal;
- client principal;
- tenant principal;
- credential records;
- key IDs;
- rotation epochs.

### Autorização

- grants;
- admin command authorization;
- operation authorization;
- tenant isolation;
- catalog mutation authorization;
- route/grant ownership checks ligados à identidade autenticada.

### Integridade

- signed/authenticated control-plane payloads quando necessário;
- binding de negotiated capability à sessão autenticada.

### Auditoria

- security audit records;
- auth success/failure;
- admin mutations;
- credential changes;
- grant changes;
- denied operations;
- revocation events.

### Backup

- encryption;
- key rotation;
- secure restore.

### Qualification

Criar `QI-AUTHZ` e fechar `QI-SECURITY`.

---

# 15. Panic por poison em transport — corrigido

O `Connections` registry deixou de usar `self.writers.lock().unwrap()`. Um mutex poisoned agora
recupera o estado protegido com `PoisonError::into_inner`, evitando que uma falha anterior cause
um segundo panic no daemon. A regressão envenena deliberadamente o registry e confirma que uma
conexão ainda pode ser registrada, receber resposta e ser removida.

A regra recomendada não é “zero unwrap em todo Rust”, e sim:

**nenhum panic acionável por input remoto, estado de disco corrompido, falha recuperável de thread ou condição operacional prevista.**

---

# 16. MVP-4 — publicação atômica multi-IDC

## Estado

**Não implementado.**

Faltam, entre outros:

- `TxnBegin`;
- participants sealed;
- prepare;
- `PrepareVote`;
- decision;
- `DecisionCertificate`;
- install;
- publication;
- `PublicationCertificate`;
- `PublishSeen`;
- completion;
- `CompletionCertificate`;
- coherent snapshot cut;
- recovery multi-IDC;
- retained gates.

Isso é um projeto grande por si só.

### Recomendação

Não tornar MVP-4 blocker do primeiro release C5 single-IDC.

Fechar C5 primeiro.

---

# 17. MVP-5 — C1/C2

## Estado

**Não implementado em runtime.**

Busca por `SemanticCommitV1` encontra SPECs e documentação, mas não runtime correspondente.

Faltam:

- `SemanticCommitV1`;
- `CausalContextV1`;
- `SessionTokenV1`;
- durable outbox;
- durable inbox;
- receive state machine;
- causal scheduler;
- anti-entropy;
- bootstrap;
- group frontier;
- session rules;
- C12 corpus/campaign.

A presença de C1/C2 na library do compiler não deve ser confundida com capability executável.

---

# 18. MVP-6 — C3 escrow

## Estado

**Não implementado em runtime.**

Faltam:

- rights ledger;
- U/H/X states;
- holders;
- fences;
- transfer protocol;
- durable reservation;
- quarantine;
- failover;
- independent durability policy;
- recovery states;
- metrics;
- ESC corpus.

FM-1 é evidência de modelo bounded, não implementação.

---

# 19. MVP-7 — evolução

## Estado

**Não implementado.**

Faltam:

- close;
- drain;
- reconcile;
- transform;
- staging;
- install;
- activate;
- retire;
- generation fencing;
- RequestHome handoff;
- preservation of old receipts;
- preservation of retry semantics;
- old issuer handling;
- evolution schedules.

FM-3 não substitui o runtime.

---

# 20. MVP-8 — C4

## Estado

**Não implementado.**

Faltam:

- point evidence;
- absence evidence;
- range evidence;
- predicate evidence;
- durable reservations;
- generated validator;
- publication;
- read-set integration;
- C4 campaign.

A própria arquitetura permite deixar C4 desabilitado se o custo não justificar o ganho. Isso é razoável.

---

# 21. Qualification

## Ponto forte

O framework de qualification é um dos melhores elementos do projeto.

Existem:

- manifest;
- source revision binding;
- working-tree digest;
- toolchain;
- lock digest;
- PASS/FAIL/INCONCLUSIVE/NOT_RUN;
- observable history;
- W1 oracle independente;
- checker;
- schedules;
- fault injection;
- minimizer;
- replay;
- evidence bundles;
- codec corpus;
- model checkers.

Isso é muito mais sério do que um conjunto casual de unit tests.

## Lacunas

### CI público verde

Fechado no [run 34777875153](https://github.com/JoseRFJuniorLLMs/CarolinaDB/actions/runs/34777875153)
para o commit funcional `618cd0e`.

### P1 — fuzz coverage-guided de longa duração

Os quatro harnesses coverage-guided existem, compilam e passaram no smoke Linux público.
Falta executar e reter campanhas longas.

### P1 — workloads W2–W8

Ainda ausentes.

### P1 — performance protocol

Faltam:

- baseline;
- hardware manifest;
- warmup;
- repetitions;
- confidence interval;
- regression threshold;
- equivalence sheet;
- comparable competitors/modes.

### TLC bounded executado

O runner `tools/run_tlc.py` fixa TLA+ tools v1.8.0 por SHA-256. Em 13/09/2026 os três
modelos concluíram sem erro: FM-1 3.268 estados distintos, FM-2 348 e FM-3 2.816. Isso continua
sendo evidência limitada pelos bounds:

O projeto deve continuar dizendo:

```text
bounded model evidence
```

e não “formal proof”.

### P1 — lacunas da verificação adversarial

A própria `docs/AUDIT.md` informa que 29 de 32 runs foram concluídos. Faltam as duas lentes de
SPEC-013 e a lente de testes de SPEC-012.

Isso deve virar uma campanha reprodutível, não depender de uma sessão manual.

---

# 22. Testes

`docs/STATUS.md` declara localmente, em 13/09:

- 181 testes;
- 0 failed;
- 0 ignored;
- clippy clean;
- fmt clean;
- spec lint PASS.

O GitHub Actions confirmou essas verificações para o commit funcional publicado.

Portanto a hierarquia correta de confiança é:

1. CI do commit publicado;
2. bundle de qualification ligado ao commit;
3. log reprodutível;
4. declaração documental.

O item 1 está verde no run `34777875153`.

---

# 23. Power-loss e durability model

`SECURITY.md` reconhece que as campanhas atuais modelam:

- process kill;
- short write;
- I/O failures internos.

Mas não modelam perda de page cache em power failure.

## P1 para storage production

Adicionar testes em ambiente que permita:

- VM power cut;
- forced reboot;
- filesystem/barrier variation;
- fsync fault;
- device full;
- rename durability;
- directory fsync semantics;
- Windows/Linux differences.

O projeto precisa declarar exatamente o que significa “durable” por plataforma.

---

# 24. Observabilidade

Não foi encontrado stack real de tracing/metrics production profile.

Faltam:

- structured logs;
- trace IDs;
- request key;
- txn ID;
- raft term/index;
- catalog generation;
- grant/authority epoch;
- latency histograms;
- queue depth;
- fsync latency;
- checkpoint duration;
- recovery duration;
- follower lag;
- snapshot progress;
- rejected admissions;
- auth failures;
- corruption counters;
- resource pressure.

Não é necessário adotar Prometheus especificamente, mas é necessário fornecer um contrato de observabilidade.

---

# 25. Dependency e supply-chain security

O workspace possui poucas dependências externas, o que é excelente.

O pipeline agora contém RustSec, e a auditoria local do lockfile não encontrou advisories.
Continuam faltando:

- `cargo deny` ou equivalente;
- policy de license allowlist;
- dependency review;
- provenance;
- SBOM;
- artifact signature.

A baixa quantidade de dependências reduz risco, não elimina o problema.

---

# 26. Release engineering

A própria árvore assume que faltam:

- SBOM;
- signed releases;
- reproducible builds;
- release manifest.

Também não foi identificado um pipeline completo de release.

## P0/P1

Criar release que produza:

- `carolina`;
- `carolina-node`;
- checksums;
- SBOM CycloneDX/SPDX;
- source revision;
- Rust toolchain;
- Cargo.lock digest;
- qualification bundle digest;
- signature;
- supported platform list.

## Versionamento

Definir explicitamente:

- wire compatibility;
- storage format compatibility;
- catalog snapshot compatibility;
- downgrade policy;
- upgrade policy.

---

# 27. Deployment

`docs/BUILD.md` cobre build e DEV_LOCAL, mas ainda não há perfil operacional de produção.

Não foi encontrado Dockerfile no repositório.

Docker não é obrigatório, mas é necessário ter pelo menos um caminho suportado de deployment:

- binary tarball/package;
- systemd unit ou Windows Service;
- directories;
- ownership;
- ports;
- TLS paths;
- cert rotation;
- data directory;
- backup directory;
- log directory;
- ulimits;
- graceful shutdown;
- upgrade;
- rollback.

---

# 28. Upgrade e rollback

Esse é um buraco operacional importante.

Antes de produção precisam existir testes para:

```text
vN start
→ write
→ shutdown
→ upgrade vN+1
→ reopen
→ verify
→ workload
→ rollback quando permitido
```

Também:

- old binary + new data format;
- new binary + old data format;
- mixed-version cluster;
- rolling upgrade;
- incompatible wire version;
- incompatible catalog generation;
- interrupted upgrade.

Sem isso, “versioned protocol” fica mais forte no documento do que na operação.

---

# 29. Commit discipline e rastreabilidade

O histórico anterior contém muitos commits repetidos como:

```text
first commit
```

Isso é ruim para:

- bisect;
- release notes;
- audit;
- blame;
- regression isolation;
- scientific reproducibility.

As correções desta auditoria passaram a usar mensagens semânticas. A disciplina deve ser
mantida nos próximos commits.

## Recomendação

Adotar mensagens semânticas, por exemplo:

```text
storage: reclaim mvcc versions at checkpoint
consensus: reject follower behind compacted snapshot
catalog: add deterministic snapshot image
ci: enforce qualification quick profile
docs: sync public CI evidence
```

Para um projeto cujo argumento central envolve proveniência e evidência, a história Git não deveria parecer um amnésico repetindo a mesma frase.

---

# 30. Documentação: inconsistências atuais

## 30.1 README/STATUS e evidência pública

README e STATUS registram o commit funcional verificado:

```text
618cd0e42bd42ac5a9218343befd4a3581470b0b
```

e o [run público verde 34777875153](https://github.com/JoseRFJuniorLLMs/CarolinaDB/actions/runs/34777875153).

## 30.2 BUILD vs Cargo.toml

O mismatch foi corrigido: BUILD e Cargo.toml dizem Rust 1.89+. O
`rust-toolchain.toml` agora fixa `1.89.0`, e o CI testa `1.89.0` e `stable` em Linux e
Windows.

## 30.3 Auditoria narrativa antiga

O snapshot narrativo antigo `docs/Relatório de Auditoria Completa do CarolinaDB.md`
foi substituído por este relatório, que contém o estado e as evidências atuais.

## 30.4 `md/FALTA.md`

O arquivo foi renomeado de `FALTA,md` para `FALTA.md`; o conteúdo continua sendo o
backlog curto, enquanto `docs/AUDIT.md` mantém a matriz detalhada.

---

# 31. O que não deve ser tratado como bug

Algumas limitações são escolhas conscientes do estágio atual:

- fixed three-voter membership;
- loopback-only DEV_LOCAL;
- C1–C4 desabilitados;
- bounded model checking;
- C4 opcional;
- single-IDC C5 como primeira vertical.

Elas viram problema apenas quando o projeto se apresenta como algo além do escopo declarado.

O erro seria ligar mais features sem fechar a vertical atual.

---

# 32. Prioridades reais

## P0 — antes de qualquer claim de release

1. **CI verde no commit publicado.**
2. **Corrigir MSRV/toolchain e documentação.**
3. **Implementar SPEC-013 para um perfil não-DEV_LOCAL.**
4. **Fechar snapshot export/import e restore.**
5. **Criar follower catch-up por snapshot.**
6. **Definir release artifact reproduzível e assinado.**
7. **Atualizar README/STATUS/AUDIT para o commit real.**

## P1 — hardening para C5 production-ready

8. latching/concurrency;
9. group commit;
10. page free-list/reuse;
11. compaction física;
12. transaction RYW;
13. `ExpectedVersion`;
14. read-set;
15. metrics/tracing;
16. diagnostics;
17. cargo-audit/cargo-deny;
18. fuzzing coverage-guided;
19. code coverage;
20. benchmarks + regression gates;
21. multi-host campaign;
22. power-loss campaign;
23. upgrade/rollback;
24. backup/restore drills;
25. TLC execution;
26. completar as lentes restantes de SPEC-012 e SPEC-013.

## P2 — completar a visão científica

27. MVP-4 multi-IDC;
28. MVP-5 C1/C2;
29. MVP-6 C3;
30. MVP-7 evolution;
31. MVP-8 C4;
32. W2–W8;
33. E1–E5;
34. paper com resultados comparativos.

---

# 33. Sequência recomendada de implementação

A sequência de menor risco é:

```text
PASSO 1
CI / toolchain / docs / audit sync

PASSO 2
SPEC-013 security
mTLS + principals + authz + audit records

PASSO 3
snapshot/restore local
backup + import + corruption tests

PASSO 4
Raft snapshot + install snapshot
log compaction + lagging follower rebuild

PASSO 5
storage hardening
concurrency + group commit + free-list + diagnostics

PASSO 6
observability + operational profile

PASSO 7
fuzz + dependency scanning + coverage + benchmark gates

PASSO 8
multi-host + power-loss + soak + upgrade/rollback

PASSO 9
signed/reproducible release

PASSO 10
somente depois: MVP-4

PASSO 11
MVP-5

PASSO 12
MVP-6

PASSO 13
MVP-7

PASSO 14
MVP-8 se ainda justificar o custo
```

---

# 34. Gate proposto: CarolinaDB C5 Production Candidate

Antes de usar a expressão “production candidate”, exigir todos:

- [x] GitHub Actions verde no commit funcional.
- [x] Linux e Windows nas matrizes suportadas pelo workflow.
- [x] MSRV coerente.
- [x] toolchain reproduzível.
- [x] zero formatter/clippy failures localmente.
- [x] qualification quick PASS no CI.
- [ ] standard qualification PASS em release.
- [ ] `QI-SECURITY PASS`.
- [ ] mTLS.
- [ ] authn/authz.
- [ ] audit records.
- [ ] snapshot export/import.
- [ ] restore test.
- [ ] encrypted backup.
- [ ] Raft snapshot transfer.
- [ ] follower catch-up.
- [ ] log compaction operacional.
- [ ] graceful shutdown/restart.
- [ ] multi-host failover.
- [ ] power-loss evidence.
- [ ] campanhas longas de fuzzing coverage-guided (harnesses e smoke CI presentes).
- [x] dependency/advisory scan público (RustSec).
- [ ] SBOM.
- [ ] signed artifacts.
- [ ] benchmark baseline.
- [ ] performance regression gate.
- [ ] observability.
- [ ] upgrade test.
- [ ] rollback policy.
- [ ] storage format compatibility policy.
- [ ] wire compatibility policy.
- [ ] no known remotely-triggerable panic.
- [ ] README/STATUS/AUDIT sincronizados com o tag.

Quando esses itens estiverem fechados, o CarolinaDB poderá ser apresentado honestamente como:

> **banco distribuído C5 single-IDC, com consistência compilada, identity-safe retries, durable receipts, replicated control plane e qualification reprodutível.**

Isso já seria uma proposta técnica bastante incomum sem precisar fingir que C1–C4 estão prontos.

---

# 35. Gate proposto: CarolinaDB Research Complete

Para dizer que a visão inteira está implementada:

- [ ] MVP-4 completo;
- [ ] MVP-5 completo;
- [ ] MVP-6 completo;
- [ ] MVP-7 completo;
- [ ] MVP-8 decidido e, se habilitado, completo;
- [ ] Q0–Q7 conforme aplicabilidade;
- [ ] QI integral;
- [ ] W1–W8;
- [ ] E1–E5;
- [x] TLC bounded registrado;
- [ ] paper com baselines equivalentes;
- [ ] failure bundles públicos reproduzíveis;
- [ ] resultados de performance e limites negativos publicados.

Esse é um alvo de pesquisa muito maior do que “production-ready C5”.

---

# 36. Notas específicas sobre claims

Claims seguros hoje:

- “workspace Rust real com 12 crates”;
- “storage próprio com B+Tree/MVCC/journal/recovery”;
- “compiler conservador de consistência”;
- “exact receipts e stable request identity”;
- “vertical C5 single-IDC de três voters”;
- “campanha de três processos em loopback”;
- “framework de qualification”;
- “model checking bounded”.

Claims que **não** devem ser usados hoje:

- “production-ready”;
- “secure distributed database”;
- “multi-IDC transactions implemented”;
- “C1/C2 runtime implemented”;
- “C3 escrow implemented”;
- “C4 certified transactions implemented”;
- “online evolution implemented”;
- “formally verified” sem qualificar que os modelos são bounded;
- “CI green” sem identificar o commit/run;
- “backup/restore complete”;
- “multi-host fault tolerant proven”.

---

# 37. Achados de maior valor desta auditoria

Os achados que mais mudam a prioridade prática são:

### 1. O storage avançou além da auditoria anterior

MVCC reclamation e journal retention foram implementados em 13/09.

Portanto não vale gastar energia repetindo uma pendência já fechada.

### 2. O CI público encontrou uma corrida e ficou verde após a correção

O run `34777132566` expôs o tratamento incorreto de `OutcomeUnknown` no harness C5.
Após a correção, o run `34777875153` passou nos oito jobs.

### 3. O mismatch de Rust 1.85 vs 1.89 foi corrigido

BUILD e Cargo.toml agora concordam em 1.89+. Falta decidir se releases fixam o
compilador exato ou se CI cobre explicitamente MSRV + stable.

### 4. Toolchain reproduzível

A MSRV foi fixada em Rust 1.89.0; o CI mantém uma segunda perna em `stable` para detectar
regressões futuras sem usar essa versão flutuante como artefato de release.

### 5. Snapshot de state machine não é snapshot distribuído operacional

Existe material de snapshot, mas falta install/catch-up end-to-end.

### 6. Fuzz coverage-guided precisa de campanha longa

Os harnesses libFuzzer existem, compilam e passaram no smoke Linux público. Faltam campanhas
longas com corpus e digests retidos.

### 7. Restam duas lacunas na verificação adversarial da auditoria

Vinte e nove dos 32 runs foram concluídos; faltam as duas lentes de SPEC-013 e a lente de testes de
SPEC-012. O processo completo deve ser reprodutível.

### 8. O caminho curto para produto não é MVP-4…8

É fechar **C5 production profile**.

---

# 38. Veredito final

O CarolinaDB é tecnicamente sério.

Não é um README cercado de arquivos vazios. Existem componentes reais, testes reais, storage real, fault injection, um compilador coerente, qualification e uma vertical Raft/C5 que já passa muito além do nível de toy database.

Ao mesmo tempo, ainda existe uma distância clara entre:

```text
research prototype avançado
```

e:

```text
distributed database production-ready
```

Essa distância hoje é dominada por engenharia operacional, não pela falta de novas ideias:

- security;
- snapshot/restore;
- replica catch-up;
- concurrency;
- observability;
- release;
- fuzz;
- benchmarks;
- CI;
- multi-host;
- upgrade/rollback.

Isso é uma notícia melhor do que parece.

O coração intelectual do CarolinaDB já existe.

O próximo salto não deveria ser inventar mais mecanismos. Deveria ser tornar o mecanismo existente **difícil de quebrar, fácil de operar, verificável por terceiros e publicamente reproduzível**.

---

# 39. Checklist final resumido

## Imediato

- [x] `cargo fmt --all -- --check` local.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` local.
- [x] `cargo test --workspace` local — 181 testes na MSRV 1.89.0.
- [x] `carolina qualify --quick` local com bundle retido.
- [x] commit local da árvore auditada.
- [x] push da árvore auditada e confirmação do CI — run `34777875153`.
- [x] corrigir BUILD para `rust-version = 1.89`.
- [x] pin de toolchain e matriz MSRV + stable.
- [x] atualizar README/STATUS para o run verde `34777875153`.
- [x] renomear `FALTA,md` para `FALTA.md`.
- [x] substituir a auditoria narrativa antiga pelo relatório atual.
- [x] `cargo audit` local e RustSec no CI.
- [x] quatro harnesses `cargo-fuzz` e smoke no CI.
- [x] TLC bounded FM-1/FM-2/FM-3 com runner e checksum fixados.

## C5 production profile

- [ ] SPEC-013.
- [ ] mTLS.
- [ ] authn/authz.
- [ ] audit records.
- [ ] backup encryption.
- [ ] snapshot export/import.
- [ ] restore.
- [ ] Raft install snapshot.
- [ ] follower catch-up.
- [ ] log compaction end-to-end.
- [ ] observability.
- [ ] concurrency/latching.
- [ ] group commit.
- [ ] free-list.
- [ ] RYW.
- [ ] ExpectedVersion.
- [ ] read-set.
- [ ] multi-host.
- [ ] power-loss tests.
- [ ] campanhas longas de fuzz coverage-guided.
- [ ] coverage.
- [ ] cargo-deny/license policy (`cargo audit` já integrado).
- [ ] benchmarks.
- [ ] upgrade/rollback.
- [ ] SBOM.
- [ ] signed release.
- [ ] reproducible build.

## Full research vision

- [ ] MVP-4.
- [ ] MVP-5.
- [ ] MVP-6.
- [ ] MVP-7.
- [ ] MVP-8 se justificado.
- [ ] W2–W8.
- [ ] E1–E5.
- [x] TLC bounded.
- [ ] paper/baselines.

---

**Fim do relatório.**
