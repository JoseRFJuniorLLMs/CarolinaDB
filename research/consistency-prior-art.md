# Consistência compilada: anterioridade e hipótese de pesquisa para AstraDB

Pesquisa verificada em 2026-09-09. Escopo: invariantes; semântica de operações; síntese de coordenação; composição; observações e evolução. Esta revisão não prova a ausência de um predecessor exato. Não estabelece prioridade científica para a proposta abaixo. Distingue resultados publicados; protótipos; agendas de pesquisa e inferências próprias.

## Parecer

**“Compilar consistência a partir das invariantes e da semântica das operações” não é, isoladamente, contribuição nova.** É uma linha de pesquisa estabelecida, com ferramentas e protocolos. SIEVE; Indigo; Quelea; Hamsaz e LoRe já tornam insustentável anunciar esse slogan como a invenção do terceiro banco. Event Horizon, CIDR 2026, aproxima ainda mais a literatura da formulação proposta: a seção 7 apresenta explicitamente uma agenda de restrições locais + modelo de custo → síntese de protocolo; e levantamento verificado de código para a representação semântica.

A decisão justificável é investir primeiro em um experimento científico delimitado. Não há evidência suficiente nesta revisão para recomendar, já agora, um compromisso irrestrito de três a cinco anos com um novo SGBD.

## Anterioridade que a proposta precisa enfrentar

| Trabalho | O que efetivamente cobre | O que não se deve anunciar como novidade do AstraDB |
|---|---|---|
| CALM / Keeping CALM | Caracterização da execução distribuída consistente sem coordenação pela expressibilidade monotônica, sob o modelo do teorema | “Semântica determina a necessidade de coordenação” |
| Coordination Avoidance / I-confluence | Condição necessária e suficiente, no modelo proposto, para validade por invariantes; disponibilidade transacional; convergência e execução sem coordenação | “O banco coordena somente quando a invariável pode ser violada” |
| Blazes | Análise de propriedades de componentes e suas composições em dataflows; síntese de coordenação | “Compilar pontos de coordenação de um fluxo de dados” |
| SIEVE, ATC 2014 | Invariantes e anotações de merge; análise estática com condições avaliadas em runtime; classificação strong versus causal CRDT | “Decidir dinamicamente a consistência necessária à operação” |
| Indigo, EuroSys 2015 | Explicit Consistency; análise de operações ofensivas e reservas; middleware sobre armazenamento causal | “Invariantes geram reservas distribuídas” |
| Quelea, PLDI 2015 | Contratos de visibilidade por método/transação; classificação automática com soundness; implementação sobre Cassandra | “Consistência declarativa por operação e composição transacional” |
| Bounded Counter, SRDS 2015 | Escrow descentralizado sobre armazenamento existente; cotas numéricas sem coordenação no caminho em que há direitos locais | “Saldo global seguro com consumo local” |
| IPA, PVLDB 2018 | Modifica operações e políticas de resolução para preservar invariantes sob concorrência; mantém semântica original quando não há conflito | “Resolver invariantes automaticamente sem consenso” |
| Hamsaz, POPL 2019 | Objeto sequencial e invariantes → relações de conflito/dependência → protocolos paramétricos; guards; updates; retornos | “Sintetizar replicação segura de uma especificação de objeto” |
| Antidote SQL, 2019 | SQL com semânticas concorrentes para constraints; coordenação para semântica estrita | “SQL com invariantes sob replicação relaxada” |
| LoRe, TOPLAS 2024 | Dataflow reativo; propriedades de segurança fornecidas pelo desenvolvedor; compilador e provas; coordenação seletiva; executável verificado | “DSL de invariantes com compilação comprovada para edge/local-first” |
| No-Op, ECOOP 2025 | Políticas definidas pelo programador priorizam operações; perdedoras tornam-se sem efeito; prova de convergência e invariantes | “Convergência + invariantes sem coordenação” sem discutir perda de efeitos |
| Event Horizon / DeMon, CIDR 2026 | Dependências assimétricas; semi-linearizability; operações fracas reordenáveis; consenso para fortes; causal broadcast; bags entre operações fortes | “Misturar consenso e causalidade segundo as dependências da aplicação” |
| Consistent Updates for Scalable Microservices, POPL 2026 | Consistência de updates visível ao cliente; algoritmos que usam semântica e comutatividade entre versões; impossibilidade para updates semanticamente cegos | “Evolução online precisa de semântica e equivalência observável” |

CALM e I-confluence respondem perguntas relacionadas mas diferentes. CALM trata resultados determinísticos de programas e monotonicidade; I-confluence trata preservação de predicados sobre estados alcançáveis sob merge. Comutatividade dos efeitos não implica ausência de coordenação para qualquer invariante; e preservação de estado não implica retornos idênticos ou promessa final observável.

## Hipótese que ainda merece um experimento

**Hipótese proposta; novidade ainda a confirmar:** um compilador conservador para uma linguagem restrita de transições pode produzir planos de execução por domínio de invariantes e demonstrar refinamento observável ao compor planos diferentes, inclusive durante mudança de contrato e transferência de autoridade. Seu resultado deveria incluir uma justificativa verificável para cada decisão final devolvida ao cliente.

O problema científico central seria a preservação composicional de uma promessa irrevogável, e não a mera conservação do valor de uma linha. A especificação precisa conter estado; invariantes; pré/pós-condições; resultados possíveis; efeitos autorizados; dependências e regras para versões. Uma política concorrente que converta silenciosamente uma reserva já confirmada em no-op viola essa especificação mesmo que o contador final permaneça não negativo.

Possível alvo formal: se o estado inicial satisfaz I; o contrato pertence ao fragmento aceito; cada plano satisfaz suas obrigações; nenhuma escrita contorna o runtime; e as hipóteses de durabilidade/falha do plano valem, então todo histórico observável composto refina o contrato. Em particular, toda resposta marcada final permanece explicável após recuperação e transição de epoch, sem duplicar direitos nem efeitos internos.

Isso precisa ser uma regra/prova nova e útil, não apenas uma frase agregando componentes conhecidos. Pré/pós-condições já existem em LoRe; retornos já estão em Hamsaz; visibilidade está em Quelea; síntese de protocolo é anterior; evolução semântica aparece em POPL 2026. A contribuição só emerge se houver uma construção concreta que resolve uma combinação que os baselines não resolvem com igual garantia.

## Arquitetura mínima para testar a hipótese

1. **Linguagem finita/restrita.** Operações registradas e versionadas; inteiros com limites explícitos; conjuntos; mapas; predicados e recursos. Nada de analisar automaticamente JavaScript arbitrário; inferir intenção de texto; ou aceitar efeitos externos opacos na prova.
2. **IR de contratos.** Identidade/hash de contrato; domínio; entradas; reads/writes semânticos; resultado; pré/pós; recursos consumidos; relações de causalidade; versão do estado e plano.
3. **Compilador conservador.** Classes iniciais: merge comprovado; causal com contexto; escrow de recursos; serialização local de domínio; transação coordenada multidomínio. Timeout/unknown do solver nunca significa seguro. Contraexemplo quando possível; fallback correto quando conhecido; rejeição se o próprio contrato sequencial não preserva I.
4. **Domínios, não flags isoladas.** A análise considera todas as operações e invariantes que podem interferir. Uma operação nova muda o conjunto de execuções possíveis. Dependências cruzadas exigem composição explícita; co-localização; reserva ou coordenação. Não há ganho científico em chamar uma flag weak/strong de compilador.
5. **Plano como autoridade de commit.** Operações semânticas e IDs idempotentes; recibos; dependências; consumo/transferência de direitos e versão do plano fazem parte da unidade durável. As materializações relacionais podem ser convencionais.
6. **Recuperação.** Snapshot inclui direitos; epochs; deduplicação e versões. Reexecutar o mesmo ID não pode consumir outro direito. Uma réplica ressuscitada com snapshot antigo não pode voltar a emitir confirmações válidas.
7. **Mudança de contrato.** Barreira explícita; compatibilidade entre versões provada ou drenagem; fencing para autoridades antigas; ledger de migração dos recursos. Isso precisa fazer parte do protocolo provado.
8. **Consulta.** Separar observação causal; snapshot de domínio; resultado final e snapshot multidomínio. Uma consulta cujo resultado governa uma decisão torna-se parte da operação registrada. SQL analítico sobre projeções pode coexistir, mas não pode emitir decisões que escapam ao contrato.
9. **Aceleração.** SSD/NVMe e CPU são suficientes para a falsificação inicial. CXL/RDMA/GPU não removem dependência causal; indecidibilidade; ou a necessidade de coordenação. Otimizar hardware antes da validação da abstração confundiria a contribuição.

## Armadilhas que devem aparecer no paper

- **Safety não é liveness.** Saldo nunca negativo pode ser garantido negando todos os pedidos. Medir disponibilidade útil; sucesso quando recursos existem; starvation; justiça e taxa de rejeições espúrias.
- **Provar preservação não prova especificação correta.** Uma regra de negócio omitida continua omitida. LLM pode ajudar a escrever uma proposta de contrato; não é parte confiável da decisão de segurança.
- **Inteiros e semântica de linguagem.** Prova em inteiros matemáticos não autoriza overflow silencioso no executável. Floats; relógio; randomização; ordenação e exceções exigem semântica explícita.
- **Composição.** Domínios seguros separadamente podem produzir comportamento inválido conjuntamente. Transferência entre orçamento e estoque precisa de atomicidade ou de uma especificação de reserva/compensação diferente; não basta somar duas provas locais.
- **Observações.** `increment()` sem retorno e `increment_and_get()` são contratos distintos. `remaining_budget()` causal não é uma autorização para gastar. Um retorno exato e global pode trazer coordenação de volta.
- **Causalidade não é causalidade inferencial.** Relógios e dependências de execução não sustentam inferência causal de dados científicos.
- **Escrow.** Direitos previamente alocados escondem coordenação fora do caminho rápido. Rebalanceamento; hot spots; esgotamento e falhas são resultados centrais, não detalhes dispensáveis do benchmark.
- **Evolução.** Alterar limite de orçamento enquanto réplicas desconectadas conservam direitos antigos pode invalidar a prova. TTL não basta sem hipóteses de relógio. Revogação imediata e execução indefinidamente offline entram em tensão.
- **Efeitos externos.** Recibo não torna uma API de pagamentos atômica com o banco. Exactly-once externo exige cooperação da contraparte; idempotência; protocolo transacional ou resolução de resultado incerto. Registrar intenção e resultado não elimina a janela de falha.
- **Prova criptográfica.** Hashes/Merkle comprovam compromissos e adulteração segundo um modelo de ameaça; não provam automaticamente correção semântica; completude de resultado; ou honestidade do executor.
- **Desvio de rota.** Se UPDATE ad hoc consegue mudar campos protegidos sem passar pela mesma validação, a garantia do sistema cai. O runtime deve bloquear ou coordenar operações desconhecidas.

## Experimentos refutáveis e critérios para encerrar

**E1 — fragmento útil.** Desenvolvedores independentes conseguem expressar pelo menos alguns workloads reais de reservas/cotas/alocação sem escrever manualmente o protocolo distribuído. Publicar contratos rejeitados; tempo de verificação; quantidade de anotações; casos de timeout e contraexemplos.

**E2 — vantagem por semântica, não por enfraquecimento.** Comparar implementações com o mesmo significado de confirmação; durabilidade; falhas toleradas; réplicas e condição de sucesso. Medir p50/p95/p99; throughput; mensagens; bytes; energia quando instrumentada; coordenação movida para background; taxa de rejeição. Não comparar confirmação final do baseline com aceitação provisória do AstraDB.

**E3 — composição.** Mistura de merge causal + escrow + domínio coordenado deve preservar a especificação observável sob reordenação; perdas; duplicações; crash/restart; resharding e atualização concorrente. Buscar histórias mínimas que falsifiquem a regra de composição.

**E4 — ponto em que o ganho desaparece.** Variar skew; razão entre leituras finais e observações causais; invariantes cruzadas; tamanho do domínio; falta de direitos; churn de versão e partições. Reportar a região em que consenso convencional é melhor.

**E5 — custo da integração.** Comparar a implementação nativa contra um compilador/middleware usando PostgreSQL local e contra escrow manual bem projetado. Se a vantagem sobrevive sem uma engine nova, o produto inicial deve ser essa camada. Se os casos valiosos caem quase sempre no fallback, a tese de uma nova categoria perde força.

Baselines necessários: PostgreSQL com isolamento/locks corretos; implementação manual de escrow; Hamsaz/LoRe em tarefas reproduzíveis ou reimplementações explicitamente identificadas; DeMon onde a semântica é equivalente; Antidote/CRDT para os fragmentos compatíveis. Medir esforço de implementação e erros encontrados separadamente do desempenho.

## A pergunta PostgreSQL

Não existe prova de impossibilidade computacional de reproduzir essa lógica em uma extensão PostgreSQL: um sistema suficientemente extensível consegue hospedar um runtime arbitrário. Constraints; stored procedures; locks; isolamento por transação; ledger de direitos e processos auxiliares cobrem grande parte do comportamento. O argumento honesto é arquitetural e econômico.

Se o diferencial exige que o compilador controle admissão; autorização de recursos; commit; dependências; replicação; recuperação e mudança de epoch para todos os escritores, PostgreSQL pode continuar sendo o armazenamento local, mas o sistema que fornece a garantia distribuída passa a ser outro runtime. Deve-se medir se trocar a engine agrega valor. Uma feature de isolamento por operação, sozinha, não exige novo banco: PostgreSQL já permite escolher isolamento por transação.

## Fontes primárias chave

1. [Coordination Avoidance in Database Systems — PVLDB 8(3), 2014; VLDB 2015](https://www.vldb.org/pvldb/vol8/p185-bailis.pdf). PDF lido, especialmente modelo e condições do teorema.
2. [Keeping CALM: When Distributed Consistency is Easy](https://arxiv.org/abs/1901.01930). Autores; versão CACM 2020.
3. [Blazes — relatório técnico dos autores; ICDE 2014](https://www2.eecs.berkeley.edu/Pubs/TechRpts/2013/EECS-2013-133.html).
4. [SIEVE: Automating the Choice of Consistency Levels in Replicated Systems — USENIX ATC 2014](https://www.usenix.org/conference/atc14/technical-sessions/presentation/li_cheng_2). Página oficial e conteúdo dos proceedings consultados.
5. [Putting Consistency Back into Eventual Consistency / Indigo — EuroSys 2015](https://www.lip6.fr/Marc.Shapiro/papers/2015/putting-consistency-back-EuroSys-2015.pdf). Conteúdo indexado primário consultado; fetch direto do PDF apresentou erro nesta sessão.
6. [Quelea: Declarative Programming over Eventually Consistent Data Stores — PLDI 2015](https://kcsrk.info/papers/quelea-long.pdf). PDF lido; seção 4.6 soundness; seção 6 transações; seção 9 limitações de tempo real.
7. [Extending Eventually Consistent Cloud Databases for Enforcing Numeric Invariants — SRDS 2015](https://arxiv.org/abs/1503.09052).
8. [IPA: Invariant-Preserving Applications for Weakly Consistent Replicated Databases — PVLDB 12(4), 2018](https://www.vldb.org/pvldb/vol12/p404-balegas.pdf). PDF lido; mudanças de semântica concorrente são explícitas.
9. [Hamsaz: Replication Coordination Analysis and Synthesis — POPL 2019](https://mohsenlesani.github.io/companion/popl19/POPL19.pdf). PDF lido; well-coordination; guards/update/retv; síntese de protocolos.
10. [Antidote SQL: Relaxed When Possible, Strict When Necessary — 2019](https://arxiv.org/abs/1902.03576).
11. [LoRe: A Programming Model for Verifiably Safe Local-First Software — TOPLAS 2024](https://arxiv.org/html/2304.07133v2). Texto completo consultado; precondições; dataflow e coordenação.
12. [Ensuring Convergence and Invariants Without Coordination / No-Op — ECOOP 2025](https://drops.dagstuhl.de/storage/00lipics/lipics-vol333-ecoop2025/LIPIcs.ECOOP.2025.4/LIPIcs.ECOOP.2025.4.pdf). PDF lido; políticas de prioridade tornam operações sem efeito.
13. [Event Horizon: Asymmetric Dependencies for Fast Geo-Distributed Operations — CIDR 2026](https://vldb.org/cidrdb/papers/2026/p20-arns.pdf). PDF lido; seções 2–4 definem SL/DeMon; **seção 7 explicita síntese de protocolos por custo e verified lifting como agenda**.
14. [Consistent Updates for Scalable Microservices — POPL 2026](https://cs.nyu.edu/wies/publ/popl26_consistent_updates.pdf). PDF lido; escopo assume workers isomórficos; clientes sequenciais; um update de cada vez; não muda interface do banco; composição de redes de serviços fica para pesquisa futura.
15. [The LAW Theorem: Local Reads and Linearizable Asynchronous Replication — PVLDB 18(9), 2025](https://dse.in.tum.de/wp-content/uploads/2025/11/lara-vldb-2025.pdf). PDF lido; impossibilidade de leituras locais sob hipóteses precisas de registro linearizável assíncrono tolerante a crash; propõe almost-local reads.

Fontes suplementares:

- [Bodega: Localized Linearizable Reads at Anywhere Anytime via Roster Leases — OSDI 2026](https://www.usenix.org/conference/osdi26/presentation/hu-guanzhou). Página oficial aberta; demonstra alternativa com leases e responder-covering quorum. Não contradiz LAW sob suas hipóteses diferentes.
- [Cure: Strong Semantics Meets High Availability and Low Latency — ICDCS 2016](https://www.lip6.fr/Marc.Shapiro/papers/Cure-final-ICDCS16.pdf). Fonte primária indexada; causalidade transacional e CRDTs.
- [PostgreSQL 18: Transaction Isolation](https://www.postgresql.org/docs/current/transaction-iso.html) e [Data Consistency Checks at the Application Level](https://www.postgresql.org/docs/current/applevel-consistency.html). Ambas abertas; evitam o falso contraponto de que PostgreSQL só dispõe de um isolamento global.

Não foram transportados resultados numéricos de desempenho das fontes para previsões do AstraDB. A recomendação é condicional à demonstração e aos experimentos propostos.
