# AstraDB: contratos de operações e coordenação verificável

**Parecer:** a direção mais promissora é um sistema que compile contratos de operações em planos de coordenação por domínio de invariantes. Entretanto, a formulação genérica já tem antecedentes diretos. Existe uma hipótese de pesquisa que merece um protótipo; ainda não existe evidência suficiente para comprometer três a cinco anos com um banco comercial novo.

A possível contribuição é preservar compromissos observáveis já confirmados quando planos diferentes se compõem, falham, mudam de versão ou atravessam uma redistribuição de dados. Workflows duráveis, integração com agentes e recibos de execução são aplicações ou mecanismos auxiliares. Não constituem, isoladamente, a tese do sistema.

Este parecer considera fontes disponíveis até 9 de setembro de 2026. Distingue documentação, código inspecionado, resultados publicados e arquitetura proposta. Não contém benchmarks próprios nem estimativas numéricas de mercado ou de probabilidade. A classificação de risco de virar extensão é um julgamento qualitativo, sem uma população estatística que permita percentuais defensáveis.

## 1. A propriedade fundamental

Os bancos comerciais oferecem garantias sobre execuções, mas em grande medida deixam para a aplicação a tradução entre essas garantias e suas obrigações de domínio. O banco conhece leituras, escritas, conflitos e restrições declaradas; frequentemente não conhece o significado completo de reservar, consumir, transferir, aprovar ou revogar.

Isso produz dois problemas diferentes: coordenação além do necessário para algumas operações e proteção insuficiente para decisões de negócio que atravessam transações, serviços ou versões de código. O primeiro não autoriza sacrificar o segundo.

**A abstração proposta é um contrato executável sobre os estados alcançáveis e as observações permitidas.** O sistema escolhe, dentre planos disponíveis e verificados, como executar esse contrato sob um modelo de falhas explícito. O contrato deve incluir os valores retornados, visibilidade das leituras, autoridade para produzir efeitos, durabilidade e comportamento diante de falta de comunicação.

Há uma correção importante na premissa: isolamento já pode ser escolhido por transação em PostgreSQL e em outros sistemas. Consistência, isolamento e consenso também não são sinônimos. A diferença pretendida não é adicionar uma opção por operação. É **derivar obrigações de coordenação de uma especificação e demonstrar que sua composição preserva o contrato**. [PostgreSQL: SET TRANSACTION](https://www.postgresql.org/docs/current/sql-set-transaction.html).

Invariantes, sozinhas, não determinam uma única consistência correta. Duas implementações podem manter saldo não negativo e oferecer leituras, retornos, disponibilidade e justiça de atendimento diferentes. Essas diferenças precisam constar da entrada do compilador, não ser escolhidas silenciosamente pelo otimizador.

## 2. O que já existe e limita a reivindicação de novidade


| Antecedente                                              | O que impede reivindicar como novo                                                                                                                                                                                |
| -------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CALM e invariant confluence                              | Relacionar propriedades dos programas/invariantes à necessidade de coordenação tem fundamento estabelecido. I-confluence depende das operações, do invariante, da alcançabilidade e da semântica de merge. |
| SIEVE, USENIX ATC 2014                                   | Usa informação semântica e análise para identificar operações que exigem consistência mais forte.                                                                                                          |
| Quelea, PLDI 2015                                        | Contratos declarativos com implementação de garantias por operação já são parte da literatura.                                                                                                              |
| Indigo, EuroSys 2015                                     | Preservação de invariantes com coordenação seletiva, incluindo direitos sobre recursos, antecede esta proposta.                                                                                               |
| Hamsaz, POPL 2019                                        | Sintetiza protocolos a partir de objetos e invariantes; operações incluem guardas, atualizações e retornos.                                                                                                   |
| LoRe, TOPLAS 2024                                        | Verificação de propriedades de segurança e coordenação seletiva em programas reativos local-first já combinam compilador, contratos e dataflow.                                                             |
| Event Horizon, CIDR 2026                                 | Explora dependências assimétricas e semi-linearizability. A seção de trabalhos futuros discute síntese de protocolos orientada a custo. Nem essa agenda é inédita.                                         |
| Consistent Updates for Scalable Microservices, POPL 2026 | A evolução semântica de serviços é também uma área com resultados recentes; dizer apenas “upgrade correto” não basta.                                                                                   |

Fontes e leituras técnicas detalhadas estão nas [notas de antecedentes](research/consistency-prior-art.md). Fontes centrais acessíveis: [LoRe](https://arxiv.org/html/2304.07133v2), [Event Horizon](https://vldb.org/cidrdb/papers/2026/p20-arns.pdf) e [Consistent Updates](https://cs.nyu.edu/wies/publ/popl26_consistent_updates.pdf).

O alvo científico teria de ser um resultado mais estreito: uma regra composicional de refinamento observável, com implementação de transições entre planos, que preserve confirmações irrevogáveis para uma classe útil de contratos. Uma revisão bibliográfica não prova que esse resultado está ausente de toda a literatura. Sua originalidade permanece uma obrigação da primeira fase da pesquisa.

**A direção de execução durável enfrenta concorrência ainda mais direta.** DBOS documenta uma biblioteca apoiada em PostgreSQL; Restate integra execução e estado, oferecendo objetos com um escritor por chave e consistência linearizável. Logo, checkpoint, retry, fila e estado junto do workflow não exigem um novo banco por si. [Arquitetura DBOS](https://docs.dbos.dev/architecture), [estado no Restate](https://docs.restate.dev/guides/databases).

O CIDR 2026 publicou *Consistency and Correctness in Data-Oriented Workflow Systems*, que discute garantias para workflows completos. O texto distingue funcionalidades implementadas de garantias de consistência ainda não implementadas na seção 7; não se deve converter a proposta inteira em capacidade comercial disponível. [Paper](https://vldb.org/cidrdb/papers/2026/p9-stonebraker.pdf).

Também há pesquisa recente em especulação para execução durável, com libDSE no OSDI 2026, e um preprint de agosto de 2026 sobre isolamento do ambiente semântico de workflows de IA. Este último é preprint, não resultado que este parecer atribua a uma conferência revisada. Esses trabalhos tornam insuficiente a novidade de “fixar modelo/ferramentas e poder retomar agentes”. [libDSE](https://www.usenix.org/conference/osdi26/presentation/li-tianyu), [BEGIN AI TRANSACTION](https://arxiv.org/abs/2608.05412).

## 3. Dez direções, avaliadas nos quinze critérios

As arquiteturas desta seção são propostas para comparação, não descrições de produtos já construídos. “Moat” significa dificuldade de reprodução técnica sustentada por resultados e operação; uma combinação de componentes conhecidos não constitui automaticamente uma barreira competitiva.

### A. Contratos de operações com coordenação compilada

1. **Problema fundamental:** tornar explícita a relação entre significado das operações, observações e comunicação necessária para preservar compromissos.
2. **Insuficiência dos bancos existentes:** a aplicação normalmente escolhe protocolos, particionamento e isolamento; constraints isoladas não sintetizam uma estratégia distribuída composicional.
3. **Quem tenta:** SIEVE, Quelea, Indigo, Hamsaz, LoRe e DeMon/Event Horizon; CRDTs e escrow são mecanismos antecedentes obrigatórios.
4. **Contribuição científica candidata:** refinamento observável entre planos heterogêneos sob composição, mudança de contrato e falhas. Precisa demonstrar diferença em relação aos antecedentes.
5. **Contribuição de engenharia:** compilador restrito, catálogo versionado, runtime de direitos, explicações de bloqueio, recuperação e migração compatíveis com o contrato.
6. **Arquitetura mínima:** DSL, analisador de dependências, verificador, biblioteca de dois ou três protocolos, runtime replicado e simulador determinístico.
7. **Unidade fundamental:** instância de máquina de estados tipada com contrato versionado; fisicamente, estado, direitos, operações e recibos.
8. **Consistência:** contrato por domínio interdependente; visibilidade causal onde suficiente, escrow onde demonstrado e serialização onde exigida. Nenhuma promessa global de disponibilidade.
9. **Consultas:** projeções relacionais com fronteira de visibilidade explícita; operações retornam resultados compatíveis com o contrato; EXPLAIN mostra por que coordenação é necessária.
10. **Distribuição:** particionamento pelo fechamento das invariantes e dependências; replicação local durável e transferência controlada de direitos entre regiões.
11. **Moat:** teoremas utilizáveis, compilador com boa cobertura, biblioteca de protocolos e histórico público de comportamento sob falhas reais.
12. **Mercado:** hipótese concreta em reservas, quotas, capacidade, direitos de consumo e operações multirregionais; disposição a trocar de banco não validada.
13. **Paper:** forte potencial se houver resultado de composição e avaliação adversarial; fraco se for só seleção entre Raft e CRDT.
14. **Três a cinco anos:** somente após uma fase curta demonstrar novidade, utilidade e vantagem frente a solução manual competente.
15. **Risco de ser feature/camada:** **médio-alto**. Pode terminar justificadamente como runtime sobre um storage existente.

**Decisão: finalista 1 e escolha para o experimento inicial.**

### B. Banco de efeitos duráveis com contratos observáveis

1. **Problema fundamental:** o estado do banco pode estar correto enquanto a execução externa está incompleta, duplicada ou semanticamente desatualizada.
2. **Insuficiência atual:** uma transação não torna atômicos banco, pagamento, ferramenta e intervenção humana; checkpoint tampouco resolve essa fronteira.
3. **Quem tenta:** Temporal, DBOS, Restate, Beldi/Apiary na literatura, libDSE e trabalhos sobre isolamento semântico de workflows.
4. **Contribuição científica candidata:** cálculo de efeitos que componha garantias de execução, autoridade, versões e observações, expondo precisamente os resultados desconhecidos.
5. **Contribuição de engenharia:** ledger de efeitos, conectores com contratos testáveis, deduplicação, reconciliação, versionamento e consulta sobre execuções.
6. **Arquitetura mínima:** scheduler durável, tabelas de workflow/efeito, outbox/inbox, runtime isolado e dois conectores com comportamento de falha especificado.
7. **Unidade fundamental:** ocorrência de efeito identificada, com intenção, autoridade, dependências, resposta e estado de resolução; não apenas um evento genérico.
8. **Consistência:** transição interna atômica; efeito externo condicionado às garantias do destino. `unknown` é um resultado legítimo após falha ambígua.
9. **Consultas:** estado de processos, efeitos pendentes, causalidade operacional e ambiente semântico usado; replay não reenvia efeitos já concluídos.
10. **Distribuição:** ownership por workflow/objeto; filas particionadas; fencing de executores; efeitos entre partições usam protocolo explícito.
11. **Moat:** conectores confiáveis e semântica verificada. A dificuldade é relevante, mas também constitui a especialidade dos concorrentes.
12. **Mercado:** demonstrável para execução durável; casos divulgados por DBOS incluem agentes e automação de laboratórios. São relatos do fornecedor, não validação independente de um mercado para outro banco.
13. **Paper:** possível em composição e tratamento formal de efeitos; uma implementação de workflows com SQL seria pouco original.
14. **Três a cinco anos:** não recomendados como banco independente neste momento; a diferenciação frente a DBOS/Restate é insuficiente.
15. **Risco de ser feature/camada:** **alto**; DBOS é um contraexemplo concreto à alegação de que durabilidade de workflows exige storage novo.

**Decisão: finalista 2, mantida para comparação arquitetural; não recomendada como identidade do AstraDB.** [Casos DBOS](https://dbos.dev/customer-stories).

### C. Banco de incerteza conjunta para computação científica

1. **Problema fundamental:** armazenar um escalar frequentemente apaga dependências, erro de medição, pressupostos e relação entre observação e inferência.
2. **Insuficiência atual:** adicionar uma coluna `confidence` não representa distribuição conjunta, correlações ou efeito de atualizar uma hipótese.
3. **Quem tenta:** ProvSQL, BayesDB, GenSQL e sistemas de consultas probabilísticas como FastPDB; o campo não é novo.
4. **Contribuição científica candidata:** atualização transacional conjunta de observações, modelo e inferência incremental correlacionada com garantias explícitas de aproximação.
5. **Contribuição de engenharia:** armazenamento de fatores, circuitos de inferência, linhagem de versões e execução progressiva com invalidadores corretos.
6. **Arquitetura mínima:** famílias probabilísticas restritas, catálogo de modelos imutáveis, fatores versionados e plano híbrido relacional/probabilístico.
7. **Unidade fundamental:** variável aleatória identificada e fator/dependência associado a uma versão de modelo; observação é uma evidência, não uma verdade inferida.
8. **Consistência:** snapshots coerentes de evidência e modelo; precisão estatística é um contrato separado da consistência transacional.
9. **Consultas:** probabilidades, condicionamento e expectativas com orçamento de erro/tempo; os pressupostos acompanham o resultado.
10. **Distribuição:** corte do grafo de fatores, mensagens de inferência, cache por versão e materializações; partições fortes podem destruir paralelismo.
11. **Moat:** inferência incremental escalável com correlações e garantias verificáveis, mais integrações científicas.
12. **Mercado:** necessidade científica real; hipótese de compra em laboratórios e engenharia, mas demanda por outro DBMS não estabelecida.
13. **Paper:** potencial elevado para algoritmos e semântica de atualização; não basta embutir um probabilistic programming framework.
14. **Três a cinco anos:** justificáveis como programa acadêmico muito especializado; investimento comercial prematuro.
15. **Risco de ser feature/camada:** **alto**. ProvSQL já funciona como extensão PostgreSQL; GenSQL cobre parte relevante da interface probabilística.

**Decisão: finalista 3; melhor alternativa semântica, mas inferior à A em teste inicial de produto.** [ProvSQL](https://provsql.org/docs/user/probabilities.html), [GenSQL](https://arxiv.org/abs/2406.10583).

### D. Banco para memória compartilhada desagregada por CXL

1. **Problema fundamental:** custo e rigidez da localização dos dados quando várias máquinas podem acessar memória compartilhada.
2. **Insuficiência atual:** arquiteturas concebidas para shared-nothing pagam mensagens/cópias; coerência e falhas compartilhadas não se encaixam automaticamente nesse modelo.
3. **Quem tenta:** Tigon, Pasha e pesquisa recente de gerenciamento de memória CXL.
4. **Contribuição científica candidata:** transações e recuperação sob falhas parciais do fabric, com elasticidade e isolamento entre domínios de falha.
5. **Contribuição de engenharia:** allocator, índices relocáveis, persistência correta, NUMA/tiering e controle de topologia.
6. **Arquitetura mínima:** pod pequeno com CXL real, DRAM local, região compartilhada e WAL em armazenamento durável.
7. **Unidade fundamental:** página/objeto versionado endereçável por offset; mantém substancialmente a abstração tradicional de registros.
8. **Consistência:** MVCC/serialização de transações; coerência de cache não substitui isolamento, durabilidade ou replicação.
9. **Consultas:** operadores relacionais conscientes da topologia e do custo de acessos remotos.
10. **Distribuição:** dentro do pod via memória; entre pods via protocolo distribuído convencional.
11. **Moat:** co-design e acesso a hardware real, recuperação validada e gerenciamento de fabric.
12. **Mercado:** fornecedores de nuvem e appliances; entrada depende de hardware e integração com operadores especializados.
13. **Paper:** forte possibilidade em OSDI/FAST/VLDB com solução real para falhas e desempenho.
14. **Três a cinco anos:** plausíveis para laboratório ou fornecedor; não a melhor terceira abstração de banco neste contexto.
15. **Risco de ser feature/engine:** **alto**; pode mudar profundamente a implementação sem criar uma categoria semântica.

**Decisão: eliminada para AstraDB.** Fontes específicas e limites do hardware nas [notas de alternativas](research/hardware-alternatives.md).

### E. Execução próxima dos dados em NVMe, DPU e armazenamento computacional

1. **Problema fundamental:** movimentação de bytes, utilização de CPU e variabilidade de I/O dominam certas cargas.
2. **Insuficiência atual:** pushdown limitado e planos que não representam bem restrições de dispositivos e isolamento entre tenants.
3. **Quem tenta:** fornecedores de SmartNIC/DPU, computational storage, engines com pushdown e pesquisa sobre ZNS/NVMe e RDMA.
4. **Contribuição científica candidata:** placement de operadores com contrato conjunto de recuperação, isolamento e consumo de recursos sob heterogeneidade.
5. **Contribuição de engenharia:** runtime de operadores restritos, drivers, agendamento, DMA seguro e observabilidade de cauda.
6. **Arquitetura mínima:** CPU + SSD NVMe; depois um único caminho de offload medido. Não exigir simultaneamente todas as classes de dispositivos.
7. **Unidade fundamental:** extent/bloco ou segmento colunar com operações permitidas sobre ele.
8. **Consistência:** snapshots e commit continuam necessários; concluir DMA não significa que a transação tornou-se durável.
9. **Consultas:** plano físico decide onde filtrar, descomprimir, agregar e fazer joins suportados.
10. **Distribuição:** storage nodes e compute nodes separados; tráfego reduzido por pushdown; recuperação deve cobrir falha de dispositivo e de host.
11. **Moat:** implementação especializada e relações com fornecedores; forte risco de hardware tornar parte da solução obsoleta.
12. **Mercado:** real em infraestrutura de grande escala; economia depende do workload e do custo total do equipamento.
13. **Paper:** plausível em FAST/OSDI/SOSP/VLDB; novidade deve ser além de “offload de filtro”.
14. **Três a cinco anos:** apenas com parceiro de hardware e carga medida que motive o projeto.
15. **Risco de ser feature/engine:** **muito alto**; interface externa pode permanecer SQL comum.

**Decisão: eliminada.**

### F. Banco que co-otimize consultas, tensores e treinamento/inferência em GPU

1. **Problema fundamental:** cópias e incompatibilidade entre layouts tabulares, tensores, features e estados de modelos.
2. **Insuficiência atual:** pipelines separam etapas e materializam intermediários; alterações nos dados podem invalidar features e inferências.
3. **Quem tenta:** DuckDB e seu ecossistema, engines de GPU, RAPIDS, sistemas de ML e plataformas como Databricks/Snowflake.
4. **Contribuição científica candidata:** consistência entre atualização de dados/modelo e resultados com plano conjunto relacional-tensorial incremental.
5. **Contribuição de engenharia:** layouts compatíveis, fusão de kernels, gestão HBM/DRAM/SSD e versionamento de artefatos.
6. **Arquitetura mínima:** CPU, uma GPU e duas famílias de operadores; inferência determinística restrita e catálogo de versões.
7. **Unidade fundamental:** bloco de dados/tensor com versão e dependências; ainda muito próximo de um engine analítico.
8. **Consistência:** snapshot de dados e modelo; determinismo numérico e tolerância a erro declarados separadamente.
9. **Consultas:** álgebra relacional acrescida de operadores de modelo com custo e comportamento de atualização explícitos.
10. **Distribuição:** pipelines entre workers e particionamento dos tensores; considerar rede, HBM e stragglers.
11. **Moat:** compilação e kernels eficientes em workloads reais; fornecedores estabelecidos possuem vantagens substanciais.
12. **Mercado:** amplo para ML e analytics; isso não prova espaço para substituir o banco existente.
13. **Paper:** bom se houver algoritmo novo de manutenção ou execução; integração de LLM não basta.
14. **Três a cinco anos:** pouco atraentes para terceiro DBMS sem workload exclusivo ou algoritmo diferenciador.
15. **Risco de ser feature/engine:** **muito alto**; compatível com expansão de engines e plataformas existentes.

**Decisão: eliminada.**

### G. Banco local-first com invariantes no edge

1. **Problema fundamental:** trabalhar desconectado sem perder regras que dependem de recursos ou identidades compartilhadas.
2. **Insuficiência atual:** merge convergente não garante validade do domínio, e conflitos resolvidos depois podem invalidar promessas já feitas.
3. **Quem tenta:** Automerge, Yjs, Antidote/CRDTs, Turso Sync e LoRe, com escopos e garantias distintos.
4. **Contribuição científica candidata:** distribuição/revogação de autoridade com orçamento de autonomia e garantias explícitas durante partições.
5. **Contribuição de engenharia:** réplica embutida, compactação causal, anti-entropy, direitos e upgrades de clientes antigos.
6. **Arquitetura mínima:** duas réplicas locais e uma autoridade de direitos; conjuntos e contadores limitados.
7. **Unidade fundamental:** objeto replicado, operação causal e capability de consumo.
8. **Consistência:** causal + preservação de invariantes para operações autorizadas; indisponibilidade ou rejeição quando faltam direitos.
9. **Consultas:** locais com fronteira/pendências explícitas; leituras globais fortes requerem comunicação.
10. **Distribuição:** malha de réplicas e servidor de rendezvous; dispositivos desconectados conservam somente a autoridade concedida.
11. **Moat:** sincronização correta em versões e dispositivos heterogêneos; difícil, mas com ecossistema já estabelecido.
12. **Mercado:** aplicações móveis, offline e colaboração; seria mais fácil vender SDK do que DBMS independente.
13. **Paper:** possível em autoridade/reconfiguração; CRDT com API amigável não é novidade científica.
14. **Três a cinco anos:** potencial como produto de sync; não como projeto separado da direção A.
15. **Risco de ser feature/camada:** **alto**; forte sobreposição com A e tecnologias de sincronização existentes.

**Decisão: absorvida como workload de A, não como segunda identidade.** [Turso Sync](https://docs.turso.tech/sync/usage).

### H. Banco de consultas verificáveis sobre dados confidenciais

1. **Problema fundamental:** cliente precisa verificar correção e completude do resultado sem confiar integralmente no executor.
2. **Insuficiência atual:** checksums e Merkle provam aspectos da integridade; não provam por si só que um join ou agregado foi calculado corretamente e sem omissões.
3. **Quem tenta:** ZKSQL, PoneglyphDB, sistemas de provas de SQL e pesquisa com enclaves e oblivious analytics.
4. **Contribuição científica candidata:** manutenção incremental de provas sob updates e composição de consultas com vazamento precisamente limitado.
5. **Contribuição de engenharia:** compilador de circuitos, commitments, verificador pequeno, gestão de chaves e batching.
6. **Arquitetura mínima:** dados committed, executor não confiável, verificador independente e subconjunto pequeno de SQL.
7. **Unidade fundamental:** relação/segmento comprometido criptograficamente e testemunhas associadas à versão.
8. **Consistência:** prova relativa a snapshot autenticado; atualidade do snapshot e não-equivocação precisam de protocolo adicional.
9. **Consultas:** resultado acompanha prova de soundness/completeness dentro da álgebra suportada; privacidade não é consequência automática da prova.
10. **Distribuição:** provers particionados e composição de provas; metadados autenticados delimitam a visão global.
11. **Moat:** algoritmos de prova e engenharia criptográfica; forte necessidade de auditoria e especialização.
12. **Mercado:** nichos de computação não confiável e dados regulados; migração para outro banco é obstáculo.
13. **Paper:** excelente possibilidade com avanço concreto no custo/expressividade da prova.
14. **Três a cinco anos:** plausíveis para equipe de criptografia; inadequado como evolução natural das capacidades já demonstradas aqui.
15. **Risco de ser feature/camada:** **alto**; o prover pode envolver engines existentes e a tese toca a auditabilidade do HeraclitusDB.

**Decisão: eliminada para este projeto.** [ZKSQL, PVLDB 2023](https://www.vldb.org/pvldb/vol16/p1804-li.pdf), [PoneglyphDB, preprint](https://arxiv.org/abs/2411.15031).

### I. Banco de causalidade científica e intervenções

1. **Problema fundamental:** linhagem do dado explica de onde veio; não identifica, sozinha, o que causou um fenômeno nem o efeito de uma intervenção.
2. **Insuficiência atual:** bancos armazenam fatos e proveniência sem assegurar identificabilidade de consultas contrafactuais.
3. **Quem tenta:** bibliotecas causais, probabilistic programming e sistemas de dados científicos; o campo de causal inference fornece o fundamento.
4. **Contribuição científica candidata:** linguagem que cheque identificabilidade e mantenha estimadores após updates sob hipóteses versionadas.
5. **Contribuição de engenharia:** catálogo de modelos estruturais, intervenções, conjuntos de ajuste e datasets experimentais.
6. **Arquitetura mínima:** dados tabulares, modelo causal restrito, validador de consultas e executor estatístico externo.
7. **Unidade fundamental:** hipótese causal/modelo estrutural mais evidências e intervenções identificadas.
8. **Consistência:** snapshot reprodutível de modelo e dados; validade causal depende de pressupostos científicos externos ao armazenamento.
9. **Consultas:** efeitos de intervenção e contrafactuais quando identificáveis; retornar “não identificável” em vez de inventar resposta.
10. **Distribuição:** execução de estimadores e dados particionados; o grafo causal não equivale à topologia das réplicas.
11. **Moat:** validação metodológica e manutenção de estimadores; pouca vantagem intrínseca em possuir o storage engine.
12. **Mercado:** análise científica e decisão especializada; mais plausível como ferramenta sobre dados existentes.
13. **Paper:** potencial em linguagem/algoritmos; não exige um novo banco para ser relevante.
14. **Três a cinco anos:** somente dentro de uma especialização científica clara e com usuários parceiros.
15. **Risco de ser feature/camada:** **muito alto**; pode integrar a direção C sem justificar outro DBMS.

**Decisão: eliminada como banco independente.**

### J. Banco autoprojetado por índices aprendidos e execução determinística temporal

1. **Problema fundamental:** custo de ajustes físicos e manutenção de desempenho diante de mudança de distribuição e necessidade de replay.
2. **Insuficiência atual:** tuning manual e estimativas frágeis; replay operacional nem sempre é simples ou barato.
3. **Quem tenta:** learned indexes/autotuning, bancos determinísticos, Datomic, XTDB e o próprio HeraclitusDB.
4. **Contribuição científica candidata:** adaptação de layout/indexação com limites de pior caso e replay compatível; a combinação genérica não é novidade.
5. **Contribuição de engenharia:** telemetria, modelos de índice com fallback, migração de layout e rastreamento reprodutível de decisões.
6. **Arquitetura mínima:** engine existente, índice aprendido e executor determinístico para um conjunto limitado de operações.
7. **Unidade fundamental:** registros/eventos e modelo de índice versionado; não altera materialmente a abstração de banco.
8. **Consistência:** a do engine hospedeiro; determinismo não fornece consenso ou disponibilidade automaticamente.
9. **Consultas:** interface convencional; modelos ajudam busca e planejamento, com caminho correto de fallback.
10. **Distribuição:** shards e replicação usuais; determinismo depende de entradas, versões e fontes de não determinismo registradas.
11. **Moat:** robustez sob drift e adversários; índices aprendidos isolados são reproduzíveis por incumbentes.
12. **Mercado:** redução de custo operacional é vendável; não demonstra motivo para migrar de banco.
13. **Paper:** possível para índice ou adaptatividade; fraco como justificativa de nova categoria.
14. **Três a cinco anos:** não recomendados como terceiro banco; risco alto de repetir o HeraclitusDB.
15. **Risco de ser feature/engine:** **muito alto**.

**Decisão: eliminada.** [Datomic](https://docs.datomic.com/glossary.html), [XTDB](https://docs.xtdb.com/intro/what-is-xtdb.html).

## 4. Resultado da eliminação


| Posição | Direção                                  | Por que permanece                                                                                                                                        | Objeção dominante                                                               |
| --------- | ------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| 1         | A — coordenação compilada por contratos | Muda quem assume a obrigação de demonstrar execução distribuída correta; permite um experimento falsificável com aplicações transacionais reais. | Antecedentes muito próximos; pode ser runtime sobre PostgreSQL.                  |
| 2         | B — contratos de efeitos duráveis        | A fronteira entre execução, banco e mundo externo é um problema fundamental.                                                                          | Categoria de durable execution já estabelecida; sobreposição com HeraclitusDB. |
| 3         | C — incerteza conjunta                    | Altera a semântica dos dados e das respostas, não apenas sua velocidade de acesso.                                                                     | ProvSQL/GenSQL; mercado de DBMS independente não demonstrado.                    |

São as três melhores **relativas ao conjunto avaliado**. Isso não significa que três oportunidades comerciais tenham sido validadas. Hardware pode produzir papers tão bons quanto esses finalistas; perdeu na exigência de justificar uma abstração nova e distinta dos projetos anteriores.



Para leigo, eu explicaria assim:

## Que tipo de banco é esse?

Ele seria um **banco de dados transacional distribuído que entende as regras do negócio**.

Não é um banco focado em IA, vetor, grafo ou auditoria. O diferencial é outro:

> **Você diz ao banco quais regras nunca podem ser quebradas, e ele decide sozinho qual nível de coordenação precisa usar para manter essas regras.**

Exemplo simples:

Uma loja tem:

```text
estoque = 10
```

e a regra é:

```text
estoque nunca pode ficar abaixo de zero
```

Num banco tradicional, o programador precisa decidir como fazer isso com lock, transação, serializable, Redis, fila, consenso, etc.

Nesse novo banco, você declararia:

```text
REGRA:
estoque >= 0
```

e:

```text
OPERAÇÃO:
vender(qtd)
```

O próprio banco analisa a operação e escolhe como executá-la de forma segura.

Essa é a essência.

---

# O que ele seria comercialmente?

Eu não venderia como:

> "novo banco distribuído".

Isso não significa nada para 99% das empresas.

Eu venderia como:

> **Banco de dados que impede as regras do seu negócio de serem quebradas, mesmo com milhares de operações simultâneas, múltiplos servidores e falhas de rede.**

Em linguagem de produto:

### **Business-Rules-Native Database**

ou, tecnicamente:

### **Invariant-Compiled Distributed Database**

A categoria que eu tentaria criar seria algo como:

> **Invariant-Native Database**

ou:

> **Correctness-Native Database**

---

# O problema que ele resolve

Imagine banco, companhia aérea, marketplace, fintech ou governo.

Existem regras como:

```text
saldo não pode ficar negativo
```

```text
não vender mais ingressos do que existem
```

```text
não reservar o mesmo quarto para duas pessoas
```

```text
não gastar mais orçamento do que existe
```

```text
um CPF não pode possuir duas inscrições incompatíveis
```

```text
pagamento precisa existir antes do envio
```

```text
um medicamento não pode ser dispensado duas vezes para a mesma autorização
```

Hoje essas regras ficam espalhadas por:

```text
aplicação
+
stored procedure
+
locks
+
Redis
+
Kafka
+
fila
+
microserviços
+
banco
```

E então surge aquela tradicional arquitetura empresarial em que dez serviços precisam concordar sobre uma coisa que deveria ter sido uma regra de três linhas.

Seu banco tentaria colocar essa regra **no coração do sistema**.

---

# O grande diferencial

Hoje o programador normalmente diz:

> "Quero SERIALIZABLE."

ou:

> "Quero eventual consistency."

Seu banco faria o contrário.

O programador diz:

```text
saldo >= 0
```

E o banco responde:

```text
Para esta operação não preciso de consenso.

Para esta preciso apenas de causalidade.

Para esta posso dividir direitos entre servidores.

Para esta preciso certificar antes do commit.

Para esta última realmente preciso serializar.
```

Isso é muito mais interessante.

### Tradicional

```text
PROGRAMADOR
     ↓
escolhe consistência
     ↓
BANCO
```

### Seu banco

```text
PROGRAMADOR
     ↓
declara regra do negócio
     ↓
COMPILADOR
     ↓
descobre consistência necessária
     ↓
BANCO
```

Essa inversão é o produto.

---

# Um exemplo muito fácil

Você tem 1.000 ingressos para um show.

Existem três regiões:

```text
São Paulo
Brasília
Manaus
```

Um banco tradicional distribuído poderia mandar as três regiões consultar um servidor central para não vender ingresso demais.

Seu banco poderia perceber automaticamente:

```text
REGRA:
ingressos >= 0
```

e distribuir:

```text
São Paulo   400 ingressos
Brasília    300
Manaus      300
```

Agora cada região pode vender localmente, até mesmo com problemas de comunicação.

Quando São Paulo chegar perto de zero, pede mais direitos às outras regiões.

Mesmo se a conexão cair:

```text
ninguém consegue vender o mesmo ingresso duas vezes.
```

Isso é o mecanismo de **escrow** que está na SPEC.

Para o cliente, porém, você não vende "escrow".

Você vende:

> **Continue operando mesmo com falha de rede sem violar as regras do negócio.**

Aí o departamento comercial para de parecer uma banca de doutorado. Excelente progresso para todos os envolvidos.

---

# Para quem vender?

Aqui eu vejo mercados realmente grandes.


| Mercado                | Problema                                |
| ---------------------- | --------------------------------------- |
| **Bancos**             | saldo, limite, reservas, liquidação   |
| **Fintechs**           | carteira, crédito, pagamentos          |
| **Marketplaces**       | estoque, pedidos, reservas              |
| **E-commerce**         | overselling e estoque distribuído      |
| **Companhias aéreas** | assentos e reservas                     |
| **Hotéis**            | disponibilidade                         |
| **Ticketing**          | venda simultânea de ingressos          |
| **Telecom**            | quotas e créditos                      |
| **Cloud**              | quotas de CPU/GPU/storage               |
| **Governo**            | orçamento, benefícios, autorizações |
| **ERP**                | estoque, financeiro, compras            |
| **Logística**         | capacidade e alocação                 |
| **Energia**            | quotas, consumo e capacidade            |
| **Gaming**             | moedas, inventários, recursos          |
| **IoT/Edge**           | operações offline com limites         |

---

# Os melhores primeiros clientes

Eu **não começaria tentando substituir PostgreSQL de uma empresa inteira**.

Isso seria uma maneira bastante eficiente de ter zero clientes e muitas apresentações bonitas.

Eu começaria em problemas onde a dor é extremamente clara.

### 1. Fintech

Por exemplo:

```text
saldo
limite
reserva
crédito
```

A proposta:

> "Você declara as regras financeiras e o banco garante que concorrência, retry e falhas não as violem."

Muito forte.

---

### 2. E-commerce / marketplace

Principal problema:

> **overselling**

Dois clientes comprando o último item.

Isso é perfeito para demonstrar a tecnologia.

---

### 3. Reservas

Hotéis, voos, eventos, aluguel.

Regra:

```text
capacidade >= reservas
```

É praticamente o exemplo didático perfeito da arquitetura.

---

### 4. Cloud e GPU

Isso eu acho particularmente interessante.

Imagine um provedor com:

```text
10.000 GPUs
```

distribuídas em datacenters.

Você precisa garantir:

```text
allocated_gpu <= available_gpu
```

sem consultar um coordenador global para tudo.

O banco poderia compilar essa regra em distribuição de direitos.

Isso toca diretamente infraestrutura moderna.

---

### 5. Governo

Também há um campo excelente:

```text
dotação orçamentária
limites financeiros
benefícios
vagas
quotas
estoque público
autorizações
```

Mas eu **não faria dele um produto governamental primeiro**, porque você já está levando o HeraclitusDB exatamente nessa direção.

Eu manteria esse terceiro projeto comercial e internacional.

---

# Quem compra?

O usuário técnico seria:

```text
Backend Engineer
Platform Engineer
Database Engineer
Distributed Systems Engineer
Staff Engineer
Principal Engineer
SRE
Architect
```

Mas quem assina o cheque provavelmente seria:

```text
CTO
VP Engineering
Head of Platform
CIO
Chief Architect
```

Em bancos:

```text
Head of Core Banking
Head of Payments
Head of Infrastructure
```

---

# Qual seria o argumento para um CTO?

Não:

> "Temos invariant-confluence e escrow synthesis."

Essa frase consegue esvaziar uma sala em cerca de 14 segundos.

Seria:

> **Hoje sua equipe precisa escrever código distribuído para impedir saldo negativo, dupla reserva, overselling e outros estados inválidos. Nosso banco transforma essas regras em garantias automáticas de execução.**

E depois:

> **Quando uma operação não precisa de consenso, não usamos consenso. Quando precisa, usamos. O banco decide isso a partir das regras da aplicação.**

Essa segunda frase é bastante poderosa.

---

# Onde está o ganho financeiro?

Porque coordenação distribuída custa:

```text
latência
+
rede
+
servidores
+
complexidade
+
engenheiros
```

Se você consegue executar:

```text
80% localmente
```

e coordenar apenas:

```text
20%
```

em determinada aplicação, existe potencial de reduzir custo e latência.

**Não estou dizendo que serão 80/20.** Isso precisa ser provado por benchmark.

Mas esse é justamente o objetivo da pesquisa.

---

# Ele substituiria PostgreSQL?

No começo, não.

Eu apresentaria como:

> **banco especializado para estados críticos e invariantes distribuídos.**

Uma empresa poderia manter:

```text
PostgreSQL
```

para:

```text
clientes
cadastros
relatórios
CMS
```

e usar seu banco para:

```text
saldos
inventário
quotas
reservas
limites
```

Depois, se o sistema amadurecer, pode assumir muito mais.

---

# E AstraDB?

**Não.**

O nome é bonito, mas está ocupado de forma muito séria.

A DataStax já possui **Astra DB**, um DBaaS baseado em Cassandra, inclusive com oferta serverless e recursos para aplicações de IA. ([DataStax Documentation](https://docs.datastax.com/en/astra-db-classic/faqs.html "https://docs.datastax.com/en/astra-db-classic/faqs.html"))

Então usar `AstraDB` criaria:

```text
confusão de marca
SEO impossível
confusão no GitHub
confusão comercial
potencial problema jurídico
```

Enterraria esse nome.

---

# E InvariantDB?

Também não.

Curiosamente, **InvariantDB também já existe** e em 2026 se apresenta como um banco voltado a reconstrução de decisões, temporalidade e proveniência. ([InvariantDB](https://www.invariantdb.com/docs "https://www.invariantdb.com/docs"))

Pior ainda: o nome parece perfeito para o seu conceito, mas o produto existente trata outra coisa.

Também descartaria:

* `AxiomDB`, já existe. ([Fumadocs](https://docs.axiomdb.squareexp.com/docs "https://docs.axiomdb.squareexp.com/docs"))
* `NomosDB`, já existe como graph database. ([NomosDB](https://nomosdb.com/ "https://nomosdb.com/"))
* `ProofDB`, já tem múltiplos usos. ([Laysense Git](https://git.laysense.com/laysense/proofdb/src/commit/ed70a140a2d6cb2d2108ce7b4e73533a34bb852e "https://git.laysense.com/laysense/proofdb/src/commit/ed70a140a2d6cb2d2108ce7b4e73533a34bb852e"))
* `TelosDB`, também já está sendo usado. ([Yahoo!](https://search.yahoo.co.jp/realtime/search?p=%23telosdb "https://search.yahoo.co.jp/realtime/search?p=%23telosdb"))

A indústria aparentemente não deixou uma única palavra grega sem colocar `DB` no final. Admirável perseverança.

---

# Que nome eu usaria?

Eu **não escolheria definitivamente ainda**, porque nome comercial precisa de busca de marca, domínio, GitHub, crates.io e empresas.

Mas conceitualmente eu procuraria algo que comunique:

```text
regra
garantia
restrição
validade
coordenação
```

e não IA.

Minha preferência seria construir uma marca própria, não uma palavra óbvia + DB.

Por enquanto eu daria ao projeto um **codename técnico**, por exemplo:

### **Project Vela**

E chamaria a categoria de:

> **Invariant-Compiled Database**

Assim você não compromete a marca antes de saber se a arquitetura realmente funciona.

---

# Como eu explicaria em uma frase

### Para leigo

> **É um banco de dados que entende as regras que nunca podem ser quebradas e automaticamente decide como executar as operações sem quebrá-las.**

### Para empresário

> **Evita problemas como saldo negativo, venda de estoque inexistente, dupla reserva e estouro de limites mesmo quando muitos servidores trabalham ao mesmo tempo.**

### Para desenvolvedor

> **Você declara invariantes e operações; o banco compila automaticamente o protocolo de consistência mínimo necessário.**

### Para pesquisador

> **Invariant-compiled distributed DBMS with per-operation/per-IDC coordination synthesis.**

---

# E comparando com seus outros dois?

Ficaria muito fácil explicar:


| Banco              | Pergunta central                                                          |
| ------------------ | ------------------------------------------------------------------------- |
| **NietzscheDB**    | Como representar conhecimento/memória de IA?                             |
| **HeraclitusDB**   | Como preservar e reconstruir exatamente o que aconteceu?                  |
| **Terceiro banco** | Como impedir que operações concorrentes produzam um estado impossível? |

Essa terceira linha é a que eu considero importante.

O produto não é "mais rápido que PostgreSQL".

Não é "mais inteligente".

Não é "tem IA".

É:

> **o banco no qual a regra de negócio passa a determinar o protocolo distribuído.**

Se essa tese funcionar tecnicamente, aí sim você tem algo que pode ser apresentado como **uma categoria nova**, e não simplesmente o terceiro banco curioso do mesmo autor.
