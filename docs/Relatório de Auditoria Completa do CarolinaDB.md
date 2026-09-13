# RELATÓRIO DE AUDITORIA TÉCNICA DO CAROLINADB

**Repositório:** `JoseRFJuniorLLMs/CarolinaDB`  
**Branch auditada:** `main`  
**Commit atual auditado:** `c98e9844de7e12bb33e4c59de46937ea6821ad11`  
**Data da auditoria:** 12 de setembro de 2026

## 1. Conclusão executiva

O CarolinaDB **não é mais um projeto apenas de SPECs**. O repositório atual contém um workspace Rust real com **12 crates**, storage próprio, linguagem/DSL, IR, compilador de consistência, runtime, wire protocol, sistema de qualificação, modelos formais, implementação de consenso, catálogo distribuído, node TCP e CLI. 

A situação real é esta:

| Estágio | Situação real |
|---|---|
| **MVP-0 Semantic Core** | 🟢 Substancialmente implementado |
| **MVP-1 Conservative Compiler** | 🟢 Substancialmente implementado |
| **MVP-2 Local Durable Slice** | 🟢 Substancialmente implementado |
| **MVP-3 Catalog + single-IDC C5** | 🟡 Implementação forte, mas **formalmente incompleta** |
| **MVP-4 Atomic Publication / multi-IDC** | 🔴 Não implementado |
| **MVP-5 C1/C2** | 🔴 Não implementado |
| **MVP-6 C3 Escrow** | 🔴 Não implementado |
| **MVP-7 Evolution** | 🔴 Não implementado |
| **MVP-8 C4** | 🔴 Não implementado |
| **Segurança de produção** | 🔴 mTLS/auth ausentes |
| **Snapshots completos / restore** | 🟡 Parcial |
| **Compaction / GC** | 🔴 Ausente |
| **Qualification pública em CI** | 🔴 CI atual vermelho |
| **Research E1-E5** | 🔴 Ainda sem resultados experimentais completos |
| **Release engineering** | 🟡 Inicial |

A interpretação mais correta, portanto, é:

**CarolinaDB já é um protótipo de pesquisa executável e tecnicamente considerável, com um banco local durável e uma primeira vertical distribuída C5 de três nós. Ainda não é um banco distribuído production-ready.**

Isso é muito diferente de “temos algumas SPECs e um B+Tree”.

---

# 2. Estrutura que realmente existe

O workspace possui estes 12 crates:

| Crate | Papel |
|---|---|
| `carolina-core` | Tipos fundamentais, identidades, hashing, canonicalização, Decimal, key codec |
| `carolina-lang` | DSL, parser, AST, IR, lowering, interpreter |
| `carolina-compiler` | Análise de dependências, obligations, escolha de plano |
| `carolina-storage` | B+Tree, MVCC, journal, buffer pool, recovery, transactions |
| `carolina-wire` | Records, envelopes, negociação, codecs, snapshots |
| `carolina-runtime` | Engine, RequestHome, execução e receipts |
| `carolina-qualify` | Campaigns, oracle, history checker, replay, minimizer |
| `carolina-models` | Modelos formais/exploração explícita |
| `carolina-consensus` | Raft e simulador |
| `carolina-catalog` | Control plane e catálogo replicado |
| `carolina-node` | Processo distribuído, TCP, cluster C5 |
| `carolina-cli` | Interface de comando |

Isso está declarado explicitamente no workspace atual. O projeto também proíbe `unsafe` no nível do workspace e declara Rust 1.89, edition 2021 e Apache-2.0. 

A árvore real confirma que não são crates vazios. Existem implementações de dezenas de milhares de bytes para parser, IR, interpreter, compiler, B+Tree, kernel, engine, Raft, catalog, node e qualification, juntamente com testes e fixtures. 

---

# 3. MVP-0: Semantic Core

## Status: 🟢 praticamente implementado

Aqui o CarolinaDB já tem bastante coisa concreta.

Estão implementados:

**Sistema de identidade tipado**

Existem `RequestKey`, `TxnId`, `IdcBinding`, `PlanRef`, `AuthorityBinding` e a taxonomia de identidade necessária para evitar misturar generation, epoch, authority e request identity.

**Canonical serialization**

Existe encoder/decoder JSON canônico restrito, validação por re-encode, limites e testes negativos.

**Hashing**

Há SHA-256 com domain separation.

**Decimal**

Existe `Decimal(p,s)` com operações verificadas.

**Key encoding**

Existe codec de chaves ordenável.

**Linguagem CarolinaDB**

O parser entende construções como:

`RECORD`, `ENUM`, `INDEX`, `RESOURCE`, `INVARIANT` e `OPERATION`.

Há lexer, parser, AST e lowering.

**IR semântico**

Existem:

`ModuleIR`, `InvariantIR`, `OperationIR`, `EffectIR`, `ContractIR`.

Eles possuem encoding canônico e hashing.

**Reference interpreter**

Há interpreter determinístico, checked arithmetic, avaliação de invariantes e replay.

**Counterexample engine**

Há exploração limitada de execuções concorrentes para encontrar situações onde duas operações aceitas separadamente violariam o contrato quando combinadas.

**Golden corpus**

Existem 7 workloads DSL completos e seus correspondentes golden artifacts.

O STATUS atual registra esses componentes como implementados e testados. 

### O que ainda falta no MVP-0

Há três pontos importantes ainda incompletos.

`S003-A05`, relacionado a aliasing, phantom e aggregate footprints, permanece pendente.

`S003-A09`, fuzzing do parser/semântica, permanece pendente.

`S003-A11`, session scope, aparece apenas parcialmente implementado no lowering.

Portanto eu chamaria o MVP-0 de **90%+ conceitualmente pronto**, mas não de “formalmente fechado”, justamente porque ainda faltam esses testes de fronteira.

---

# 4. MVP-1: Conservative Compiler

## Status: 🟢 implementado para a vertical atual

Essa talvez seja uma das partes intelectualmente mais interessantes do projeto.

Há implementação de:

**Invariant Dependency Components**

O compiler monta closures sobre record footprints e usa union-find para agrupar dependências.

Reconhece relações como:

`RequiresVisible`

`InvalidatesGuard`

`AtomicWith`

**Obligations**

Cada candidato carrega obligations como:

`IR-DEF`

`IR-FOOT`

`DURABILITY`

`AUTHORITY`

`RUNTIME-CAPABILITY`

`IR-OBS`

`SESSION`

`IR-SEQ`

`IR-CONC`

`IR-ATOM`

`IR-COMP`

Cada obrigação pode assumir `Proven`, `Disproven` ou `Unknown`.

**Counterexample generation**

O compiler pode explorar estados pequenos deterministicamente e produzir contraexemplos.

**Protocol library**

C0 e C5 possuem caminhos atualmente utilizáveis.

C1, C2, C3 e C4 aparecem conceitualmente na biblioteca, mas isso **não significa que seus runtimes estejam implementados**.

Essa distinção importa muito.

**Plan selection**

A seleção conservadora prefere C5 como baseline e só escolhe solução mais barata quando as obligations necessárias foram provadas.

**Artifacts**

Há `OperationPlan`, `ConsistencyCertificate`, artifact checker e `EXPLAIN`.

**CLI**

Já existem comandos ligados a compile, explain, check, IR e fixtures.

O próprio STATUS descreve essa vertical como implementada. 

### Minha avaliação

Essa parte já parece mais do que demonstração arquitetural.

O compiler tem uma arquitetura coerente para sustentar a tese principal do CarolinaDB:

**contrato semântico → obligations → protocolo candidato → prova/refutação → plano imutável.**

Isso é o coração do projeto.

---

# 5. MVP-2: Storage e runtime local durável

## Status: 🟢 forte

Aqui também não estamos diante de mock.

### Storage

Existe implementação de:

- páginas de 8 KiB;
- CRC32C;
- MANIFEST A/B;
- journal;
- detecção de torn tail;
- I/O fault injection;
- B+Tree;
- MVCC;
- overflow chains;
- copy-on-write checkpoint;
- buffer pool CLOCK;
- journal redo-first;
- sync/group/unsafe durability modes;
- recuperação;
- verificação estrutural.

A árvore confirma arquivos específicos para `btree.rs`, `buffer.rs`, `journal.rs`, `io.rs`, `kernel.rs`, `batch.rs`, `memkernel.rs` e campaigns de crash. 

### Transaction kernel

O `DurableStorageKernel` já contempla:

`commit`

`commit_protocol`

`prepare`

`commit_prepared`

`abort_prepared`

checkpoint

recovery

verify

Há tratamento de prepared transaction invisível, `IN_DOUBT`, decisões persistentes e idempotência.

### Differential testing

Existe um `MemKernel` independente para comparação.

Isso é uma decisão boa de engenharia porque permite detectar divergência entre armazenamento persistente e modelo de referência.

### Crash testing

O projeto possui fault points nas fronteiras duráveis.

O STATUS registra campanhas verificando propriedades como:

commit confirmado permanece;

operação interrompida é all-or-nothing;

recovery é idempotente;

prepared permanece invisível;

I/O error nunca pode retornar sucesso.



### RequestHome

Também existe o RequestHome local com:

durable state;

epoch;

`BindIfAbsent`;

CAS;

detecção de reutilização da mesma identidade com conteúdo diferente.

### Exact receipts

Já existe persistência de `FinalReceiptV1`.

Depois de perda da resposta, `ResolveRequest` recupera o mesmo resultado.

Também existem:

`OutcomeUnknown`

business rejection terminal;

result tombstone;

namespace retirement;

`IdentityExpired`.

Isso é particularmente importante, porque mostra que o CarolinaDB está implementando **semântica de observação e identidade**, não apenas “gravar linha em disco”.

---

# 6. O que ainda falta no storage

Existem duas lacunas grandes documentadas.

## Snapshot/restore

Existem:

`SnapshotManifestV1`

records wire;

validação dos records.

Mas **não existe ainda export/import completo do Store**.

Logo, snapshot existe enquanto protocolo/formato, mas ainda não como mecanismo operacional completo. 

## Retention / garbage collection

Ainda não existem:

journal segment retention;

MVCC garbage collection.

Hoje versões antigas e segmentos continuam sendo retidos.

Isso não destrói correção, mas impede chamar o storage de produção de longa duração.

Um banco que nunca esquece nada eventualmente descobre uma maneira muito física de explicar a palavra “capacidade”.

---

# 7. Qualification framework

## Status: 🟢 surpreendentemente avançado

Essa área merece destaque.

Já existe um framework próprio de qualification.

Ele registra:

source revision;

working-tree digest;

dirty status;

toolchain;

Cargo.lock digest;

features;

workloads;

seeds;

budgets.

Há estados formais:

`PASS`

`FAIL`

`INCONCLUSIVE`

`NOT_RUN`

`NOT_APPLICABLE`.

O projeto ainda implementa:

histórico observável;

JSONL canônico;

trace digest;

oracle independente W1;

history checker;

deterministic schedules;

fault campaigns;

delta-debugging/minimizer;

replay;

evidence bundles;

CLI de qualification.

Existem comandos equivalentes a:

`qualify`

`simulate`

`replay`

`minimize`

`report`.

O STATUS documenta toda essa infraestrutura. 

Isso é uma excelente direção, porque sistemas distribuídos não ficam corretos pelo simples poder persuasivo de um `assert_eq!`.

---

# 8. Modelos formais

## Status: 🟡 bons artefatos, mas qualification formal incompleta

Existem de fato:

`FM1_Escrow.tla`

`FM2_Decision.tla`

`FM3_Migration.tla`

com respectivos `.cfg`.

Existem também implementações Rust de model checkers bounded:

`fm1.rs`

`fm2.rs`

`fm3.rs`.

A árvore confirma esses artefatos. 

Segundo `STATUS.md`, os checkers Rust exploraram:

| Modelo | Estados |
|---|---:|
| FM-1 | 3.268 |
| FM-2 | 486 |
| FM-3 | 3.392 |

e foram executados com negative controls.

Entretanto:

**TLC ainda não foi executado.**

Portanto os arquivos TLA+ existem, mas não há um resultado registrado de TLC qualificando-os.

O próprio projeto corretamente reconhece que isso é **bounded model evidence**, não uma prova global. 

Essa distinção deve continuar explícita em qualquer paper ou apresentação.

---

# 9. MVP-3: Raft, Catalog e single-IDC C5

## Status: 🟡 muito avançado, mas não fechado

Aqui encontrei uma nuance importante que o README mascara um pouco.

### Raft existe

Há implementação determinística de consenso com:

fixed three-voter membership;

term/vote persistentes;

log persistente;

CRC32C;

torn-tail recovery;

quorum commit;

term no-op;

ReadIndex/read barrier;

sem leases baseados em wall-clock.

### Simulator existe

O cluster simulator suporta:

loss;

duplication;

delay;

directional partition;

crash;

restart.

E verifica invariantes como:

election safety;

log matching;

state-machine safety;

leader completeness.

### Catalog existe

O catálogo implementa:

typed `CatalogKey`;

CAS;

revision;

digest;

scope locks;

catalog generation;

`AdminRequestId`;

idempotência;

`IdentityConflict`;

bootstrap manifest;

grant lifecycle;

routes;

tombstones.

### Node distribuído existe

Há um processo `carolina-node`.

Ele usa `ASTR`/TCP, negociação `Hello/HelloAck`, conexões entre peers e core single-threaded.

### C5 replicado existe

O fluxo atual é aproximadamente:

```text
client Invoke
      ↓
leader
      ↓
Admit no Raft
      ↓
execução determinística em todos os voters
      ↓
Decision
      ↓
commit
      ↓
FinalReceipt
      ↓
reply
```

Followers verificam o digest calculado localmente e devem fail-closed se divergirem.

`ResolveRequest` é atendido após read barrier.

Followers/minority recusam mutation.

Isso tudo é descrito como implementado no status atual. 

### Existem testes com três processos reais

Existe campaign de três processos separados, com diretórios separados e loopback.

O teste inclui:

leader kill;

leader restart;

replay;

majority kill.

Isso já é uma vertical distribuída de verdade.

Mas três processos na mesma máquina **não demonstram tolerância a falha física ou regional**. A própria SPEC-014 faz essa ressalva explicitamente. 

---

# 10. Por que eu não considero MVP-3 completo

A própria SPEC-014 define MVP-3 como:

Catalog + RPC/codec + RequestHome + **SPEC-013 mTLS/authorization** + ordered C5.

E exige QI aplicável. 

Mas o código atual possui apenas:

`DEV_LOCAL`

plaintext transport.

Não há:

mTLS;

authorization real;

credential records completos;

`ENCRYPTED_HOST_V1`.

O `QI-SECURITY` está `NOT_RUN`.

Consequentemente:

**Q3-C5 pode estar qualificado para a vertical DEV_LOCAL, mas o estágio MVP-3 completo, conforme a própria SPEC-014, ainda não fechou.**

Esse é um ponto importante para corrigir no README.

---

# 11. Codec e wire format

## Status: 🟢 para os recursos atualmente habilitados

Há um corpus concreto em `fixtures/codec`.

O status registra:

**52 canonical vectors**

abrangendo:

**23 record kinds**

e:

**208 negative derived vectors**

para casos como:

unknown field;

truncation;

number literal inválido;

whitespace não canônico.

Há `CodecManifest`.

Há fixtures para coisas como:

Invoke;

FinalReceipt;

RequestBinding;

TxnStatus;

CompiledBatch;

ProtocolOnlyBatch;

CatalogCommand;

ConsensusEnvelope;

Hello/HelloAck;

NodeCommand;

ResolveRequest/Reply.



Entretanto, codecs dos recursos ainda desabilitados continuam deliberadamente não congelados, incluindo funcionalidades futuras ligadas a:

C1;

C2;

C3;

C4;

evolution;

session tokens;

parte de snapshots.



Isso faz sentido.

---

# 12. O que NÃO está implementado

Aqui está o pedaço mais importante do relatório.

## MVP-4: Atomic Publication / multi-IDC

**Não existe.**

A SPEC prevê:

sealed participants;

prepare;

decision;

install;

publication;

completion;

coherent reads;

retained gates;

multi-IDC crash recovery.

Esse é o estágio que transforma C5 single-IDC numa arquitetura realmente distribuída entre domínios de consistência.

Ainda não está implementado. 

---

# 13. MVP-5: C1/C2

## Status: 🔴 não implementado

Ainda faltam os runtimes de semantic replication.

Isso inclui:

dotted group frontiers;

group sessions;

catch-up;

C1;

C2;

rejeição segura de cross-group sessions não suportadas.

A presença dessas famílias no compiler **não significa implementação runtime**.

A SPEC-014 coloca tudo isso no MVP-5. 

---

# 14. MVP-6: C3 Escrow

## Status: 🔴 não implementado

Ainda faltam:

conserved allocations;

exclusive holders;

rights;

escrow epochs;

rights transfer;

business/rights atomicity;

transfer decision durability;

independent durability policies;

failover qualification.

FM-1 existe enquanto modelo bounded, mas isso não substitui o runtime.

A própria documentação atual confirma que C3 runtime ainda não existe. 

---

# 15. MVP-7: Plan Evolution

## Status: 🔴 não implementado

Ainda falta implementar de ponta a ponta:

close;

drain;

transform;

install;

activate;

preservação de receipts antigos;

preservação de tokens;

locks;

RequestHome handoff;

generation fencing;

offline issuer handling;

incompatible-plan exclusion.

FM-3 é um modelo formal bounded, não implementação.

Esse estágio continua futuro. 

---

# 16. MVP-8: C4 Certified Transactions

## Status: 🔴 não implementado

Ainda faltam:

point evidence;

absence evidence;

range evidence;

predicate evidence;

durable reservations;

publication;

integração dessa evidência com o runtime.

A própria SPEC coloca C4 por último e permite inclusive que permaneça desabilitado se não justificar a complexidade. 

Essa escolha é sensata.

---

# 17. Segurança

## Status: 🔴 principal bloqueio atual

Este é, na minha avaliação, o maior bloqueio imediato.

O projeto já criou a SPEC-013.

Mas o runtime distribuído atual ainda trabalha em `DEV_LOCAL`.

Antes de considerar o cluster utilizável fora de laboratório, faltam no mínimo:

mTLS;

identidade dos nós;

credenciais;

autorização do control plane;

autorização do cliente;

revogação;

binding de segurança ao capability manifest;

qualification adversarial do transporte.

O próprio projeto registra `QI-SECURITY NOT_RUN`. 

Sem isso, **não exponha `carolina-node` em rede não confiável**.

---

# 18. Consensus e catalog ainda têm limitações operacionais

Atualmente:

membership é fixo;

existem exatamente três voters;

não há dynamic membership;

não há Raft log compaction;

não há catalog snapshots;

no restart o log é replayado desde o índice 1.



Para protótipo de pesquisa isso é aceitável.

Para operação prolongada, não.

---

# 19. CI: encontrei um problema real

Existe um workflow de CI bem montado.

Ele executa:

spec lint;

`cargo fmt --check`;

`cargo clippy --workspace --all-targets -- -D warnings`;

`cargo test --workspace`;

`carolina qualify --quick`;

e faz upload do bundle de qualification.

Há matriz Linux + Windows. 

### Só que o CI atual está vermelho

A execução pública atual terminou com:

**conclusion: failure**. 

Inspecionei os jobs.

Tanto Linux quanto Windows falharam em:

```text
cargo fmt --all -- --check
```

Por isso:

`clippy`

`cargo test`

e `qualification campaign`

foram pulados.

Os diffs de rustfmt atingem arquivos como:

`carolina-cli/src/main.rs`;

`carolina-cli/tests/qualification_cli.rs`;

`carolina-lang/src/spec003_tests.rs`;

`carolina-node/src/core.rs`;

`carolina-qualify/src/runner.rs`;

`carolina-qualify/tests/acceptance.rs`;

`carolina-storage/tests/kernel.rs`.

Isso é fácil de corrigir:

```bash
cargo fmt --all
```

e commit.

Mas há uma consequência metodológica importante:

**hoje não existe um GitHub Actions verde demonstrando publicamente que a árvore atual passa clippy, testes e qualification.**

O `STATUS.md` afirma que isso passou localmente em 11/09/2026, mas o CI público da árvore atual ainda não confirmou essa alegação. 

Portanto eu classifico:

**test evidence local registrada: sim**

**independent public CI evidence da árvore atual: ainda não**

Essa diferença precisa aparecer em qualquer avaliação séria.

---

# 20. Problema grave de documentação desatualizada

Encontrei quatro fontes de verdade competindo entre si.

Isso precisa ser corrigido imediatamente.

## README

O README diz corretamente que MVP-0 a MVP-3 existem.

Mas logo depois afirma:

> No distributed capability exists yet, no SPEC-010 qualification campaign has run, and no formal model gate has executed.

Isso contradiz:

o próprio quadro anterior;

o próprio restante do README;

`docs/STATUS.md`;

a existência de `carolina-node`;

o three-process campaign;

o framework `carolina-qualify`.



Essa frase deve ser removida ou atualizada.

---

# 21. `md/FALTA,md` está completamente obsoleto

Esse arquivo atualmente afirma que:

não existe Cargo.toml;

não existe workspace;

não existem crates;

não existe storage;

não existe distributed layer;

não existem formal models;

não existe spec lint;

não existe CI;

não existe LICENSE.



A árvore atual demonstra exatamente o contrário. 

Esse arquivo precisa ser:

removido;

ou movido para algo como `docs/history/`;

ou renomeado para deixar claro que é uma auditoria histórica.

Hoje ele prejudica seriamente a credibilidade do repositório.

Um pesquisador entrando no GitHub pode abrir esse arquivo e concluir que o projeto é praticamente fictício, enquanto há um cluster Raft de três nós algumas pastas ao lado. Excelente forma de transformar trabalho real em dúvida desnecessária.

---

# 22. `REVISAR-02.md` também está envelhecido

O arquivo ainda contém como pendências:

criar SPEC-014;

criar FM-1/FM-2/FM-3;

criar lint;

atualizar README;

criar ownership matrix;

normalizar nomes.



Essas coisas já existem.

Portanto as caixas `[ ]` desse documento não representam o estado atual.

Novamente, ele deveria ser marcado claramente como:

**historical audit, resolved items not maintained**

ou atualizado.

---

# 23. SPEC-014 também tem um pequeno problema de status

A SPEC-014 ainda abre dizendo:

> no completed milestone or qualification is claimed

Mas agora `STATUS.md` registra Q0/Q1/Q2/FM/Q3-C5.



Como roadmap normativo, isso não destrói a SPEC.

Mas esse cabeçalho factual deveria ser atualizado para evitar contradição.

---

# 24. ARCHITECTURE FREEZE ainda não existe

`docs/STATUS.md` registra:

`ARCHITECTURE-FREEZE-0.1.md` ainda não produzido.



Eu não congelaria a arquitetura antes de:

CI verde;

resolver inconsistências documentais;

fechar o conjunto atual de codec vectors;

decidir a interface de security profile do MVP-3.

Depois disso, faz sentido criar o freeze.

---

# 25. Research / paper

O projeto possui uma base de pesquisa bastante maior que a maioria dos repositórios desse tipo.

Existem:

prior art;

repo comparison;

proposal;

SPECs;

formal models.

Mas a parte experimental científica ainda está incompleta.

SPEC-010 define E1-E5, incluindo principalmente:

| Experimento | Pergunta |
|---|---|
| E1 | Quão expressivo é o fragmento? |
| E2 | Há vantagem real contra protocolos equivalentes? |
| E3 | Observable refinement é preservado em composição/evolução? |
| E4 | Onde a vantagem desaparece? |
| E5 | Possuir o storage próprio realmente importa? |

Em particular, E5 exige comparar o engine nativo com implementação sobre storage existente. 

Não encontrei resultados experimentais consolidados desses E1-E5 no repositório atual.

A SPEC-014 ainda manda realizar essa comparação com baselines competentes e observa corretamente que **um B+Tree próprio é uma escolha de engenharia, não evidência de novidade científica**. 

Esse trecho está intelectualmente correto.

### O experimento que mais importa

Para paper, eu colocaria E5 entre os mais importantes:

**CarolinaDB native storage**

versus

**mesmo coordination compiler sobre PostgreSQL**

versus

**PostgreSQL + protocolo manual competente**

versus

**C5 convencional**.

Se o compiler gerar benefício independentemente do storage, o resultado científico continua interessante, mas sugere que o produto talvez devesse ser runtime/compiler e não DBMS completo.

A própria SPEC já reconhece essa possibilidade. Isso é uma virtude, não uma fraqueza.

---

# 26. Release engineering

Já existem:

`LICENSE`;

`SECURITY.md`;

`.gitignore`;

Cargo workspace;

Cargo.lock;

rust-toolchain;

GitHub Actions.

Mas ainda faltam:

SBOM;

artefatos assinados;

release manifest;

build documentation consolidada;

pacotes/binários de release;

supply-chain provenance;

release reproducível formalmente demonstrado.

O próprio status marca SBOM, signed releases e `docs/build` como ausentes. 

Também não existem releases publicadas no GitHub neste momento. 

---

# 27. Histórico Git

O repositório público possui vários commits recentes, todos ainda denominados genericamente:

`first commit`.

Isso não é um problema de funcionamento, mas prejudica auditabilidade técnica.

Para um projeto cuja proposta envolve evidência, proveniência e qualification, seria melhor começar a usar commits semanticamente úteis:

```text
storage: implement journal recovery barrier

consensus: persist term/vote before reply

qualify: add Q3-C5 leader crash campaign

security: add certificate-bound node identity
```

Proveniência de software também conta.

---

# 28. O que o CarolinaDB consegue fazer hoje

Em termos simples, o CarolinaDB atualmente consegue receber uma definição de domínio e operações em DSL, transformá-la em IR tipada, analisar relações e invariantes, produzir um plano conservador, executar esse plano sobre armazenamento durável local e preservar identidade e resultado da operação através de falhas.

Ele também já consegue executar uma vertical C5 serializada através de um cluster Raft fixo de três nós, mantendo RequestHome, catálogo e receipts, além de realizar simulações e campanhas de crash/recovery.

Portanto não estamos falando apenas de um “compilador de invariantes”.

Hoje existe algo semelhante a:

```text
DSL
 ↓
typed IR
 ↓
semantic analysis
 ↓
coordination compiler
 ↓
ConsistencyCertificate
 ↓
OperationPlan
 ↓
RequestHome
 ↓
C5 runtime
 ↓
DurableStorageKernel
 ↓
FinalReceipt
 ↓
Resolve / Replay / Qualification
```

Esse pipeline é real na árvore atual.

---

# 29. O que ele NÃO consegue fazer hoje

Ele ainda não deve ser apresentado como banco multi-região de produção.

Não possui atualmente:

transporte seguro de produção;

multi-IDC atomic publication;

runtime C1/C2;

runtime C3 escrow;

plan migration/evolution;

C4 certified transactions;

dynamic cluster membership;

Raft log compaction;

catalog snapshots completos;

storage snapshot/export/restore completo;

MVCC GC;

journal retention;

TLC qualification registrada;

fuzzing completo da DSL;

CI público verde;

evidência experimental E1-E5;

release assinado/SBOM.

---

# 30. Comparação entre documentação e realidade

| Afirmação | Realidade auditada |
|---|---|
| “Não existe código” | ❌ completamente falso hoje |
| “Não existe distributed capability” | ❌ falso, existe C5 single-IDC de 3 nós |
| “MVP-0 implementado” | ✅ |
| “MVP-1 implementado” | ✅ |
| “MVP-2 implementado” | ✅ |
| “MVP-3 completo” | ⚠️ não, falta security/QI |
| “Q3 completo” | ❌ apenas `Q3-C5` |
| “FM completos” | ⚠️ bounded Rust models sim; TLC não |
| “snapshot implementado” | ⚠️ records sim, export/import não |
| “C1-C4 disponíveis” | ❌ compiler conhece famílias; runtimes não |
| “production ready” | ❌ definitivamente não |
| “research prototype executável” | ✅ |
| “há CI” | ✅ |
| “CI passa” | ❌ árvore pública atual está vermelha |

---

# 31. Ordem exata que eu seguiria agora

1. **Corrigir imediatamente o `cargo fmt` e obter um CI verde real**, com Linux e Windows executando fmt, clippy, todos os testes e `qualify --quick`.

2. **Limpar a documentação**, especialmente README, `FALTA,md`, `REVISAR.md`, `REVISAR-02.md`, SPEC-014 header e STATUS, deixando uma única fonte operacional de verdade.

3. **Fechar SPEC-013 no código**: mTLS, node identity, client/admin authorization, credentials, revocation e QI-SECURITY. Isso fecha de verdade o MVP-3.

4. **Criar `ARCHITECTURE-FREEZE-0.1.md`** depois de segurança e codec manifest estabilizados.

5. **Completar infraestrutura operacional local**: snapshot export/import, MVCC GC, journal retention, Raft snapshot/compaction e catalog snapshot.

6. **Implementar MVP-4 antes de qualquer outra família**, porque atomic publication multi-IDC é a base para as garantias distribuídas mais fortes.

7. **Depois seguir a própria SPEC-014**: C1/C2 → C3 → Evolution → C4.

8. **Executar E1-E5 de verdade**, principalmente PostgreSQL adapter E5, baselines manuais, throughput/latency, partition behavior e starvation.

9. **Fechar release engineering** com SBOM, signed artifacts, reproducible builds, release manifest e uma primeira versão `0.1.0-alpha`.

---

# 32. Avaliação técnica final

### Arquitetura

**Forte.**

Há separação razoavelmente limpa entre:

semântica;

compiler;

storage;

runtime;

wire;

consensus;

control plane;

qualification.

### Originalidade potencial

**Alta o suficiente para justificar pesquisa séria.**

O diferencial não é “mais um banco Rust”.

O diferencial é:

**compilar invariantes, observabilidade, autoridade, durabilidade e semântica de falha em planos de coordenação qualificados.**

Isso é o que precisa ser demonstrado experimentalmente.

### Storage

**Avançado para estágio de pesquisa.**

B+Tree/MVCC/journal/crash recovery já são reais.

### Distributed systems

**Promissor, porém ainda estreito.**

C5 single-IDC é um começo bom.

Multi-IDC ainda é o divisor de águas.

### Formal methods

**Muito acima da média de um protótipo inicial**, mas não confundir bounded checking com prova geral.

### Segurança

**Insuficiente para produção.**

É hoje um dos principais blockers.

### Testing

A infraestrutura é muito boa.

O CI público vermelho tira credibilidade desnecessariamente e deveria ser corrigido antes de divulgar o repositório.

### Documentação

As SPECs são extensas e ambiciosas.

O problema não é falta de documentação.

O problema é **documentação histórica permanecendo ao lado da documentação atual sem indicar que envelheceu**.

### Production readiness

**Não.**

E o próprio projeto não deveria fingir que é.

### Research prototype readiness

**Sim.**

Já passou claramente do estágio de design-only.

---

# 33. Veredito

A leitura que eu faria para um pesquisador, engenheiro de banco ou possível colaborador é:

> **CarolinaDB já implementa seu semantic core, coordination compiler, storage kernel durável, request identity/exact receipts, qualification framework e uma primeira vertical C5 distribuída em três nós. O próximo grande marco não é “começar a implementar o banco”, porque isso já aconteceu. O próximo marco é fechar segurança e qualification do MVP-3, estabilizar a arquitetura e então construir atomic publication multi-IDC.**

Esse é o estado real.

E há outra conclusão importante:

**o arquivo `FALTA,md` está mais atrasado que o projeto por várias eras geológicas. Não use esse arquivo para planejar o trabalho atual.**

O CarolinaDB já atravessou aquela fase.