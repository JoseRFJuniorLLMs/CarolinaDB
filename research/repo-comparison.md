# Evidência arquitetural: NietzscheDB, HeraclitusDB e a hipótese AstraDB

Inspeção em 2026-09-09. Leitura de código e documentação; sem builds, execução de testes, operação de serviços ou verificação de produção. Não é auditoria integral dos repositórios.

## Revisões e método

- NietzscheDB: GitHub `main`, commit `087a301c4fbafa227b00a0411d99c86a29065151` de 2026-07-09. Diretório local não disponível. Árvore remota, README e arquivos centrais lidos via conector GitHub.
- HeraclitusDB: `D:\DEV\HeraclitusDB`, HEAD e GitHub `main` em `74f921f1ad25cf27c399522c5c0d27c8ec084009` de 2026-09-08. O working tree tem muitas alterações indicadas pelo Git; nos arquivos centrais citados abaixo o diff ignorando final de linha é vazio. As referências locais são da cópia inspecionada, não de um binário validado.
- Graphify foi usado primeiro no HeraclitusDB (`graphify-out/graph.json`), seguido de confirmação nos arquivos. O grafo marca `Engine` como stale; as linhas aqui são confirmadas pelo código atual.
- Não tratei números de testes/benchmarks publicados em README como resultados reproduzidos.

## NietzscheDB: a unidade é um nó geométrico com estado cognitivo

`NodeMeta` contém identidade, profundidade, energia, conteúdo, tipo, geração L-system e dimensão de Hausdorff; `Node` agrega `PoincareVector`. `Edge` tem origem, destino, tipo e peso. A geometria e a dinâmica da memória estão no modelo de dados, não somente no marketing.

Evidência: [model.rs:415](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-graph/src/model.rs#L415), [Node:513](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-graph/src/model.rs#L513), [Edge:598](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-graph/src/model.rs#L598).

O agregador `NietzscheDB<V>` reúne `GraphStorage` em RocksDB, `GraphWal`, adjacência em memória, backend vetorial e cache LRU. `insert_node` valida esquema; escreve WAL; escreve nó e índices de metadados em RocksDB; atualiza vector store. O WAL é recuperação das mutações; o armazenamento mantém nós e arestas materializados. `GraphWal::append` faz flush e sync_data.

Evidência: [db.rs:185](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-graph/src/db.rs#L185), [insert_node:646](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-graph/src/db.rs#L646), [wal.rs:128](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-graph/src/wal.rs#L128).

`Transaction` retém `&mut NietzscheDB`, acumula operações, grava marcadores TxBegin/TxCommitted e depois aplica operações. Isso comprova um protocolo implementado de commit/recuperação; não autoriza inferir isolamento distribuído ou ACID completo de todas as superfícies a partir do comentário que usa “ACID”.

Evidência: [transaction.rs:130](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-graph/src/transaction.rs#L130), [commit:194](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-graph/src/transaction.rs#L194).

Há NQL com AST de grafos, operações geométricas, BEGIN/COMMIT/ROLLBACK e muitas extensões. O cluster descreve registro de peers, hashing consistente e consistência eventual sem Raft. O servidor realmente inicializa heartbeat e gossip. A presença de tipos e funções CRDT não comprova correção algébrica nem replicação completa dos dados: a inspeção não seguiu todos os consumidores de `GraphDelta`.

Evidência: [ast.rs:10](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-query/src/ast.rs#L10), [cluster/lib.rs:13](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-cluster/src/lib.rs#L13), [main.rs:1612](https://github.com/JoseRFJuniorLLMs/NietzscheDB/blob/087a301c4fbafa227b00a0411d99c86a29065151/crates/nietzsche-server/src/main.rs#L1612).

## HeraclitusDB: a unidade é evidência imutável e as estruturas são projeções

`Episode` é explicitamente a unidade de verdade: id, HLC, agente, sessão, tipo, conteúdo, embedding opcional, atributos, pais de proveniência e intervalo de validade. O registro canônico HRKL v6 inclui LSN, HLC, metadados opacos e episódio, com codec versionado para identidade criptográfica.

Evidência local: `crates/heraclitus-core/src/event.rs:74-95`; `crates/heraclitus-log/src/v6/canonical.rs:217-235`.

`View` recebe `(lsn, event)` e possui watermark, checkpoint, reset e hash de estado opcional. `ViewRegistry::catch_up` restaura snapshots e aplica a cauda; reconstrução desde o início é possível. `Engine` reúne log, memtable, índices de vetor/texto/grafo/atributos, grafo temporal, resolução de entidades, ativação e controles regulatórios.

Evidência local: `crates/heraclitus-views/src/lib.rs:64-100,252`; `crates/heraclitus-server/src/engine.rs:132-185`.

Na configuração replicada, `Engine::append_internal` encaminha as escritas para `ReplRouter`; caso contrário, escreve no log e indexa. O wiring do cluster instancia FileRaftLog e EpisodeStateMachine, usa openraft e TCP ou gRPC. `submit_episode` chama `raft.client_write`. Replica bytes de episódios e reconstrói índices localmente. Não encontrei síntese de coordenação a partir de operações/invariantes; o caminho observado decide pela configuração de replicação.

Evidência local: `crates/heraclitus-server/src/engine.rs:2363-2388`; `crates/heraclitus-server/src/cluster.rs:128-169`; `crates/heraclitus-raft/src/consensus.rs:54-64,807-819`.

SQL analítico usa DataFusion e Arrow sobre uma tabela `events` derivada do log, com seleção por AS OF LSN. HUME contém primitivas físicas reais, mas seu próprio módulo informa que ainda não estão ligadas ao caminho vivo de query, que segue DataFusion/Arrow. Não confundir um módulo de pesquisa com o executor em produção.

Evidência local: `crates/heraclitus-analytics/src/lib.rs:1-25`; `crates/hume-kernel/src/lib.rs:16-22`.

### Sobreposição direta com a expressão “semântica de execução”

O HeraclitusDB já tem uma `ConsistencyVirtualMachine` (H-VM): instruções Upsert, Delete e SplitShard; estado é um fold determinístico de instruções em ordem canônica. `Engine::hvm_upsert` e `hvm_delete` gravam bytecode no mesmo log, pelo mesmo caminho de Raft quando habilitado. O ledger é replayado sob demanda. Isso é determinismo da interpretação de uma ordem recebida; não é um compilador que prova quais ordenações são necessárias.

Evidência local: `crates/heraclitus-core/src/vm/interpreter.rs:1-12,24-44,87-121`; `crates/heraclitus-server/src/engine.rs:859-913`.

Também há Case Management event-sourced com comandos identificados, `expected_revision`, idempotência, estados de caso, tarefas e política versionada de prazos. O motor toma um lock por shard do case_id antes de reconstruir estado, comparar revisão e apendar evento. São semânticas de domínio implementadas manualmente sobre a infraestrutura canônica.

Evidência local: `crates/heraclitus-case/src/lib.rs:1-39,66-105`; `crates/heraclitus-server/src/engine.rs:6193-6254`.

## Diferença que AstraDB precisaria sustentar

| Questão | NietzscheDB observado | HeraclitusDB observado | Hipótese AstraDB |
|---|---|---|---|
| Objeto central | Nó/aresta com geometria e dinâmica cognitiva | Episódio imutável ordenado; registro canônico | Operação tipada com contrato e testemunho da decisão de coordenação |
| Papel do log | Recuperar mutações | Fonte canônica de verdade/proveniência | Durabilidade local e fatos necessários à recuperação do protocolo compilado |
| Correção principal | Estrutura e regras da memória | Integridade, replay e preservação histórica | Preservação declarada de invariantes sob concorrência, falhas e composição de protocolos |
| Coordenação | Infraestrutura específica de cluster; merge manual | Raft configurado e verificações/locks de domínio escritos à mão | Obrigações de ordem, direitos ou exclusão derivadas pelo compilador |
| Consulta distintiva | Navegar/procurar/raciocinar sobre memória | Consultar estado e origem em um instante | Explicar por que uma operação pode confirmar localmente ou deve coordenar |

Reutilizar RocksDB, log, Raft, Arrow, hashes ou bibliotecas não torna a pesquisa repetição. A repetição ocorreria se o resultado fosse apenas outra combinação dessas ferramentas com workflows e agentes. A diferença científica é a relação verificável entre semântica declarada e protocolo executado, incluindo a interação entre operações, fronteiras entre protocolos e evolução dos contratos.

Compilação deve ser explicitamente restrita a uma linguagem analisável. Não há evidência aqui de que seja possível inferir invariantes de negócio de código arbitrário, escolher sempre o mínimo global de coordenação, abolir CAP, ou assegurar efeitos externos exactly-once sem cooperação do destino.

## Limitações e observação de rigor

- “Não encontrei compilador de coordenação” é resultado desta inspeção focalizada, não prova de ausência em todos os arquivos ou branches.
- O estado operacional/uso governamental é declaração do usuário; não foi verificado nesta tarefa.
- No NietzscheDB, `crdt.rs:86` desempata energia igual escolhendo o primeiro argumento. Se dois estados com a mesma energia têm conteúdo/peer distintos, trocar os argumentos muda o resultado de `merge_node`; portanto o comentário “commutative” não basta. Não foi executado teste, mas o contraexemplo segue diretamente da função. Isso reforça a necessidade de separar alegação de propriedade de prova/validação, não é uma auditoria global do projeto.

URLs estáveis HeraclitusDB: substituir `<path>` em `https://github.com/JoseRFJuniorLLMs/HeraclitusDB/blob/74f921f1ad25cf27c399522c5c0d27c8ec084009/<path>#L<linha>`.
