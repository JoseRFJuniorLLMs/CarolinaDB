Fiz a releitura cruzada das **SPEC-001 a SPEC-010**, da proposta de pesquisa e do material de prior art, em dez passadas recursivas. A conclusão mudou em alguns pontos em relação à minha primeira avaliação.

**O projeto está conceitualmente forte, mas eu ainda não começaria a implementação distribuída.** Há cerca de **5 problemas P0** que precisam ser eliminados antes, porque hoje uma IA implementadora poderia produzir código perfeitamente “conforme” a uma SPEC e incompatível com outra. A humanidade inventou distributed systems justamente para transformar pequenas ambiguidades semânticas em incidentes de madrugada.

## Resultado das 10 interações

| #  | Passada                      | Severidade | Resultado                                                                           |
| -- | ---------------------------- | ---------: | ----------------------------------------------------------------------------------- |
| 1  | Tese + prior art + SPEC-001  |     **P0** | SPEC-001 está semanticamente atrasada em relação às demais                          |
| 2  | DSL + IR + hashes + encoding |     **P1** | IR está melhor do que avaliei inicialmente; falta fechar wire/snapshot              |
| 3  | Storage + durable boundary   |     **P0** | `CompiledBatch` não carrega toda a identidade exigida pelos runtimes                |
| 4  | C1/C2 + causalidade          |     **P0** | contexto causal é essencialmente single-group; sessão multi-group ficou indefinida  |
| 5  | C3 Escrow                    |     **P1** | protocolo está sólido; principal risco é disponibilidade/durabilidade de authority  |
| 6  | C4/C5 + atomic publication   |  **OK/P1** | não encontrei buraco óbvio de atomicidade; complexidade é o risco                   |
| 7  | Plan Evolution               |     **P0** | existe um Control Plane pressuposto por tudo, mas sem SPEC proprietária             |
| 8  | Qualification                |     **P1** | excelente; faltam gates formais obrigatórios para os state machines críticos        |
| 9  | Security + API               |  **P0/P1** | autenticação é citada, mas não existe modelo normativo de segurança/client protocol |
| 10 | IDs, generations e epochs    |     **P0** | `IdcGeneration`, `IdcEpoch`, `authority_epoch` etc. não estão uniformes entre SPECs |

---

# 1. SPEC-001 precisa de uma revisão grande, não pequenos patches

Este é agora o **principal problema documental do projeto**.

A SPEC-001 ainda afirma:

> compiler determines the “weakest safe coordination protocol”

e pergunta pelo “minimum coordination required”, além de apresentar:

```text
C0 -> C1 -> C2 -> C3 -> C4 -> C5
```

como uma cadeia de fallback.

Só que a SPEC-004 posteriormente corrige isso: as famílias **não formam uma ordem total**, planos podem ser incomparáveis e o objetivo é encontrar um plano seguro de baixo custo dentro de uma biblioteca finita de templates, não descobrir um mínimo universal.

E existe mais uma palavra problemática na SPEC-001: ela chama isso de **“protocol lattice”**.

Eu tiraria `lattice`. Matematicamente, lattice exige que os pares possuam meet/join. Isso não foi demonstrado e a própria SPEC-004 admite incomparabilidade.

### Texto que eu colocaria na tese

> **The compiler selects, from a finite qualified protocol-template library, a safe execution plan minimizing the declared cost function among proven-compatible candidates under explicit assumptions. No globally weakest protocol, total ordering of protocol families, or complete decision procedure is claimed.**

Isso deixa a tese muito mais forte cientificamente justamente porque para de prometer um objeto que talvez nem exista.

---

# 2. A seção de “Novelty Requirement” da SPEC-001 está errada perante a própria pesquisa

Esse problema apareceu só quando cruzei novamente SPEC-001 com `PROPOSTA-DE-PESQUISA.md`.

SPEC-001 exige como demonstração de novidade coisas como:

* automatic consistency-class derivation;
* escrow synthesis;
* per-operation coordination;
* static invariant-preservation analysis;
* directed dependencies.

Mas a proposta posterior reconhece explicitamente SIEVE, Quelea, Indigo, Hamsaz, LoRe, Event Horizon etc. e delimita a contribuição candidata de forma muito mais estreita:

**preservação de compromissos observáveis na composição de planos heterogêneos, evolução, falhas e mudança de autoridade.**

Então a SPEC-001 atualmente diz, grosso modo:

> “para ser novo precisa fazer X, Y e Z”

enquanto o documento científico diz:

> “X, Y e Z já têm antecedentes; a possível novidade está em outra coisa”.

Isso não pode permanecer.

### Eu mudaria a tese científica oficial para

```text
observable contract
        +
heterogeneous safe plans
        +
composition
        +
authority transfer
        +
plan/contract evolution
        +
crash/recovery
        ↓
observable refinement
```

Esse é o AstraDB interessante.

Não “um banco que escolhe CRDT ou Raft automaticamente”.

---

# 3. “Automatic operation-effect extraction” também deve sair

SPEC-001 coloca isso como requisito de novidade.

Mas SPEC-003 exige explicitamente:

```text
OPERATION ...
READ { ... }
EFFECT { ... }
ENSURE ...
RETURN ...
CONTRACT ...
```

Ou seja, não há propriamente “extração automática de efeitos” de código arbitrário. Existe **lowering/normalização determinística de efeitos declarados**.

Eu trocaria:

```text
Automatic operation-effect extraction
```

por:

```text
Deterministic semantic lowering and conservative
dependency extraction from versioned operation contracts
```

Muito mais preciso.

---

# 4. Retiro parte da minha crítica anterior ao encoding

Aqui a segunda leitura favoreceu as SPECs.

Eu havia sugerido uma SPEC inteira para canonical encoding. **A SPEC-003 já resolveu boa parte disso.**

Ela define:

* canonical JSON;
* ordenação;
* representação de inteiros;
* hex;
* UTF-8;
* normalização;
* domain-separated SHA-256;
* `astra.schema.v1`;
* `astra.operation.v1`;
* `astra.invariant.v1`;
* `astra.contract.v1`;
* golden bytes antes de alegar compatibilidade.

Isso está muito bom.

O que ainda falta é mais específico:

**wire format + snapshots + receipts + protocol records + compatibility negotiation.**

A própria SPEC-005 reconhece que seu wire protocol ainda depende de uma fixture v1 que fixe field order, discriminants, byte order, lengths e inputs de hash.

Portanto, eu criaria:

### SPEC-012 — Protocol Encoding & Compatibility

Não repetiria o canonical IR da SPEC-003.

Ela só possuiria:

```text
wire envelopes
snapshot format
receipt format
session tokens
decision certificates
unknown-field policy
version negotiation
codec downgrade rules
maximum frame sizes
canonical binary fixtures
golden vectors
compatibility matrix
```

---

# 5. Encontrei um problema real no `CompiledBatch`

Este é P0.

SPEC-002 define:

```rust
pub struct CompiledBatch {
    pub txn_id: TxnId,
    pub operation_id: OperationId,
    pub operation_version: u32,

    pub plan_hash: PlanHash,
    pub schema_hash: SchemaHash,

    pub idc_ids: Vec<IdcId>,
    pub consistency_class: ConsistencyClass,

    pub mutations: Vec<StorageMutation>,
    pub protocol_mutations: Vec<ProtocolMutation>,

    pub semantic_digest: [u8; 32],
}
```

Mas acima dele a arquitetura agora exige coisas como:

```text
StableRequestId
request_hash
contract_hash
exact outcome
result bytes/digest
commitments
OriginId
causal information
```

SPEC-005, por exemplo, já inclui `request_hash` e `contract_hash`.

SPEC-009 exige até um `FinalReceipt` completo com stable request, exact result e evidence.

A SPEC-002 tenta esconder isso em:

```rust
SetTxnStatus(...)
```

Só que `...` não é uma especificação. É a tradicional modalidade arquitetural “o programador do futuro se vira”.

### Eu corrigiria `CompiledBatch`

Algo nessa direção:

```rust
CompiledBatch {
    txn_id: TxnId,
    request_key: RequestKey,
    request_hash: RequestHash,

    operation: OperationRef,
    operation_hash: OperationHash,
    contract_hash: ContractHash,
    schema_hash: SchemaHash,

    plan: PlanRef,

    idc_bindings: Vec<IdcBinding>,

    mutations: Vec<StorageMutation>,
    protocol_mutations: Vec<ProtocolMutation>,

    terminal_outcome: Option<TerminalOutcome>,
    semantic_digest: SemanticDigest,
}
```

E `SetTxnStatus(...)` precisa virar uma estrutura real e normativa.

---

# 6. A taxonomia de IDs é hoje o maior risco de bug de implementação

Na última passada apareceu algo muito importante.

SPEC-004 declara explicitamente que:

```text
CatalogGeneration
IdcGeneration
IdcEpoch
EscrowEpoch
StorageEpoch
OriginId.origin_epoch
LocalCommitSeq
```

são tipos diferentes.

Excelente.

Mas SPEC-009 passa a dizer:

```text
IdcId, IdcEpoch
    -> Semantic domain and authority incarnation
```

e deixa `IdcGeneration` desaparecer dessa taxonomia.

Enquanto SPEC-005 faz:

```rust
idcs: Vec<(IdcId, u64)>
```

Ou seja:

> “Aqui está um `u64`. Boa sorte descobrindo qual universo temporal ele representa.”

Isso eu não deixaria uma linha de Rust nascer sem corrigir.

### Crie uma taxonomia única

Eu usaria aproximadamente:

```rust
CatalogGeneration
PlanGeneration

IdcGeneration
IdcAuthorityEpoch

PlacementEpoch
MembershipGeneration

ResourceGeneration
HolderAuthorityEpoch

StorageEpoch
OriginEpoch
RequestHomeEpoch

LocalCommitSeq
SerialPosition
```

E eliminaria o genérico:

```text
IdcEpoch
```

ou lhe daria **uma única definição universal**.

Nada de:

```rust
Vec<(IdcId, u64)>
```

Use:

```rust
struct IdcBinding {
    idc_id: IdcId,
    idc_generation: IdcGeneration,
    authority_epoch: IdcAuthorityEpoch,
}
```

O compilador vai agradecer silenciosamente, que é o máximo de gratidão que se deve esperar de um compilador.

---

# 7. Há uma segunda lacuna de identidade: `StableRequestId -> TxnId`

SPEC-003 estabelece corretamente que um `StableRequestId` deve se vincular exatamente a um `TxnId`.

SPEC-004 também fala no:

> original `StableRequestId`, mapped `TxnId`.

Mas não encontrei uma regra normativa dizendo **como esse mapping nasce**.

Enquanto isso, SPEC-005 determina o `RequestHome` com base em `tenant + TxnId`.

Isso cria uma pergunta desagradável:

```text
Quem escolhe TxnId?

Se RequestHome escolhe TxnId:
    precisamos do RequestHome antes do TxnId.

Se RequestHome é calculado por TxnId:
    precisamos do TxnId antes do RequestHome.
```

Pode ser solucionado facilmente. Só precisa estar escrito.

### Eu preferiria

```text
RequestKey =
    TenantId
    + RequestNamespace
    + StableRequestId
```

E:

```text
RequestHome = route(RequestKey)
```

Depois o RequestHome faz CAS durável:

```text
RequestKey
    -> TxnId
    -> request_hash
```

Ou então `TxnId` é deterministicamente derivado.

Mas escolha uma das duas.

---

# 8. C1/C2 tem uma lacuna multi-group

Esse foi um achado novo.

`CausalContextV1` possui:

```rust
group: ReplicationGroupId
```

e `SessionTokenV1` contém apenas um:

```rust
context: CausalContextV1
```

Isso é perfeitamente bom para uma sessão confinada a um replication group.

Mas o AstraDB possui vários IDCs e grupos.

Imagine:

```text
cliente:
    write A no group G1
    write B no group G2
    read C no G3
```

Se o contrato pede:

```text
ReadYourWrites
MonotonicReads
CausalDependencies
```

a representação atual não explica como transportar causalidade de `G1 + G2` para `G3`.

A SPEC-005 é muito cuidadosa com sessões dentro de seu contexto.

Mas a composição entre grupos ficou faltando.

### Duas soluções válidas

Ou você declara:

```text
C2 v1 session guarantees are group-scoped.
Cross-group session guarantees require a composite plan.
```

Simples e conservador.

Ou muda o token para:

```rust
SessionFrontierV1 {
    groups: SortedMap<
        ReplicationGroupId,
        GroupCausalContext
    >
}
```

Eu prefiro **a primeira para o MVP**.

Muito menos estado vetorial para carregar pelo planeta.

---

# 9. Escrow está melhor do que minha primeira avaliação

Aqui também retiro uma crítica.

Eu havia questionado:

```text
free = U + X
```

porque `X` não é gastável.

Mas a SPEC-006 deixa explicitamente claro que:

* `U` = rights utilizáveis;
* `X` = rights em trânsito;
* business `free` não é spend authority;
* admission consulta `U_holder`;
* observar `free` não autoriza consumo.

O protocolo está semanticamente consistente.

Portanto, **não mudaria isso por necessidade de correção**.

Mudaria talvez o nome em alguma API para evitar confusão humana, mas o modelo está certo.

---

# 10. O problema real do Escrow é outro: durability profile de autoridade

Considere:

```text
donor PREPARED
q saiu de U e entrou em X
```

e o donor sofre perda permanente.

A SPEC corretamente não inventa rights.

Resultado:

```text
q fica congelado
```

Também pode ocorrer depois de uma decisão que só tinha durabilidade local.

Isso preserva safety. A disponibilidade, entretanto, pode morrer com grande dignidade.

A própria SPEC admite essa limitação.

Eu acrescentaria ao plano algo como:

```text
AuthorityDurabilityPolicy
TransferDecisionDurabilityPolicy
```

independente de:

```text
client result durability
```

Porque são coisas diferentes.

Uma aplicação pode aceitar `LocalStable` para certo resultado, mas o cluster pode exigir quorum-durable rights-transfer decisions para tornar failover operacionalmente viável.

**P1, não P0.**

---

# 11. C4/C5: retiro qualquer suspeita de atomicidade simples

Fui especificamente tentar quebrar:

```text
prepare
commit decision
participant install
publication
public read
```

E a SPEC-008 efetivamente fecha a janela.

Ela separa:

```text
DecisionCertificate
Installed
PublicationCertificate
PublishSeen
CompletionCertificate
```

e mantém gates para impedir que public reads observem um participante pós-transação e outro pré-transação.

Isso é uma arquitetura conservadora e cara, mas coerente.

A SPEC-007 também conserva durable reservations até a resolução/publicação.

Não achei um “2PC bug” evidente no desenho.

### O problema é custo

C4 envolve:

```text
optimistic SnapshotCut
+
predicate evidence
+
replicated certifier
+
durable reservations
+
distributed prepare
+
unique decision
+
publication protocol
```

Isso pode ficar mais caro que C5 em vários workloads.

Mas a própria documentação já sabe disso.

SPEC-010 diz que C4 vem **depois** de C1/C2/C3/C5 funcionando.

Então minha recomendação anterior de implementar C4 por último não é uma nova correção.

É simplesmente:

**obedeça a própria SPEC-010.**

---

# 12. Control Plane continua sendo o maior componente ausente

Aqui minha observação anterior ficou ainda mais forte.

SPEC-009 pressupõe:

> “The catalog uses an ordered replicated control plane.”

Esse catálogo precisa controlar:

```text
CatalogGeneration
PlanGeneration
Operation identities
IDC definitions
placements
authority grants
authority epochs
membership
node capabilities
protocol capabilities
migration locks
plan activation
historical plans
fences
retirement
GC horizons
```

E durante migration ele realiza CAS e serializa mudanças de authority.

Só que ninguém é dono completo dessa máquina.

SPEC-004 usa.

SPEC-005 usa.

SPEC-006 usa.

SPEC-007 usa.

SPEC-008 usa.

SPEC-009 usa.

É praticamente o Ministério da Administração Interna do AstraDB e ninguém escreveu a Constituição.

### A próxima SPEC deve ser esta

# SPEC-011 — Catalog, Control Plane & Authority Registry

Ela deveria definir:

```text
CatalogEntry
CatalogGeneration
Catalog transaction/CAS
plan publication
operation/schema registry
IDC registry
node registry
capabilities
membership
placement
authority grants
authority revocation/fencing
historical artifact retention
migration ownership
bootstrap
consensus model
catalog snapshots
catalog recovery
catalog GC
read barriers
authorization of runtime admission
```

**Eu não escreveria código distribuído antes dela.**

---

# 13. SPEC-010 está excelente, mas eu endureceria formal methods

SPEC-010 faz algo raro e muito correto:

```text
PASS != mathematical proof
INCONCLUSIVE != PASS
NOT_RUN != PASS
```

Também exige oracle independente, deterministic simulation, crash injection e equivalência de contratos.

Eu daria **9,5/10** de novo.

Só faria uma alteração.

TLA+/equivalente aparece como `SHOULD`.

Eu criaria três gates obrigatórios:

```text
FM-1 Escrow transfer / authority
FM-2 C4/C5 decision + publication
FM-3 Plan migration / fencing
```

Antes de alegar distributed correctness:

```text
model checked
AND
deterministic simulator passed
AND
real process fault campaign passed
```

Não prova o Rust inteiro, obviamente. Mas ajuda tremendamente a descobrir erro de protocolo antes de ele ganhar 87 structs e um dashboard.

---

# 14. Falta uma SPEC de segurança

SPEC-005 manda:

```text
Authenticate the caller
check permission
authenticated authorized peer channel
```

Bom.

Mas **quem é o caller?**

Quem é o node?

Como uma authority é autenticada?

Como um nó comprometido é revogado?

Como evitar downgrade?

Como rodam certificados?

Como tenants são separados?

Nada disso é possuído normativamente.

### Eu criaria

# SPEC-013 — Security, Identity & Trust Model

Com:

```text
client authentication
node authentication
cluster identity
tenant identity
operation authorization
admin authorization
mTLS profile
certificate lifecycle
key rotation
revocation
protocol downgrade protection
replay protection
session token integrity
authority evidence authentication
secret handling
audit events
at-rest security profile
backup encryption profile
threat model
explicit non-goals
```

E deixar explícito:

```text
malicious/compromised consensus member
    !=
crash-fault member
```

Porque checksum e SHA-256 não expulsam um nó maligno de uma distributed system por força moral.

---

# 15. Falta também fechar o Client Protocol

A semântica do banco depende muito da diferença entre:

```text
Committed
Rejected
Unavailable
OutcomeUnknown
IdentityExpired
```

Isso não pode ficar para um SDK improvisar.

O protocolo cliente deve possuir:

```text
StableRequestId
RequestNamespace
request_hash
TxnId
SessionToken
ReadContract
FinalReceipt
OutcomeUnknown
ResolveRequest
IdentityExpired
Retry rules
```

Eu colocaria isso junto da SPEC-012 ou faria uma SPEC separada.

Isso é **parte da semântica científica**, não ergonomia de API.

---

# 16. Sobre minha crítica à B+Tree: eu a reduziria

Depois de reler a SPEC-002 inteira por blocos, ela está muito mais desacoplada do que parecia na primeira passada.

Ela já tem:

```rust
PageIo
StorageTxn
LocalSnapshot
StorageMutation
CompiledBatch
```

e é obsessivamente clara sobre:

```text
physical MVCC order != semantic distributed order
```

Então **eu não removeria a decisão de construir B+Tree própria**.

Mas criaria uma fronteira limpa:

```text
Astra Semantic Runtime
        |
DurableStorageKernel trait
        |
        +-- Astra B+Tree
        +-- Reference in-memory
        +-- experimental PostgreSQL adapter
```

A pesquisa já exige comparar com middleware/PostgreSQL, inclusive para descobrir se um banco novo realmente é necessário.

Portanto:

**B+Tree continua. Só não deixe a tese científica depender dela.**

---

# O que eu mudaria agora

Minha prioridade seria esta:

| Prioridade | Correção                                             | Ação                                         |
| ---------- | ---------------------------------------------------- | -------------------------------------------- |
| **P0**     | SPEC-001 contradiz a arquitetura posterior           | **Reescrever SPEC-001**                      |
| **P0**     | IDs/epochs inconsistentes                            | Criar taxonomia normativa única              |
| **P0**     | `StableRequestId -> TxnId -> RequestHome` indefinido | Definir Request Identity Protocol            |
| **P0**     | Control Plane sem SPEC                               | Criar **SPEC-011**                           |
| **P0**     | `CompiledBatch` incompleto                           | Corrigir SPEC-002                            |
| **P0**     | causal session multi-group indefinida                | Restringir MVP ou criar multi-group frontier |
| **P1**     | wire/snapshot ainda não congelados                   | Criar **SPEC-012**                           |
| **P1**     | Security/trust model ausente                         | Criar **SPEC-013**                           |
| **P1**     | formal methods não são release gate                  | Alterar SPEC-010                             |
| **P1**     | rights decision durability                           | Alterar SPEC-006                             |
| **P2**     | Storage backend experimentation                      | Adicionar `DurableStorageKernel`             |
| **P2**     | README quebrado/vazio                                | Arrumar antes de exposição pública           |

---

# Como eu reorganizaria as próximas SPECs

Eu faria agora:

```text
SPEC-011
Catalog, Control Plane & Authority Registry

SPEC-012
Request Identity, Client Protocol,
Wire Encoding & Compatibility

SPEC-013
Security, Authentication & Trust Model

SPEC-014
Implementation Profile & Vertical Slice
```

A SPEC-014 seria especialmente útil para uma IA programadora. Ela diria exatamente:

```text
MVP-0
DSL parser
Typed IR
Sequential reference interpreter
Canonical artifacts

MVP-1
Compiler closure
C5 selection only
EXPLAIN

MVP-2
Local Astra storage
CompiledBatch
crash recovery
request dedup
exact receipts

MVP-3
3-node Control Plane
single-IDC C5
Raft authority

MVP-4
distributed prepare/publication
fault qualification

MVP-5
C1/C2

MVP-6
C3 escrow

MVP-7
plan evolution

MVP-8
C4 certified
```

Isso impede que um agente receba dez documentos, veja `C1/C2/C3/C4/C5/Raft/B+Tree/MVCC/escrow/SMT` e tenha a brilhante ideia de implementar tudo na terça-feira.

---

# Veredito depois das dez passadas

Minha nota anterior global **sobe na qualidade conceitual**, mas minha disposição de começar a implementação distribuída **cai um pouco**.

Eu colocaria assim:

**Arquitetura conceitual: 9/10.**
**Rigor de safety: 9,5/10.**
**Coerência entre SPECs: 7,5/10.**
**Pronta para implementação single-node/IR: sim.**
**Pronta para implementação distributed runtime: ainda não.**
**Potencial de paper: alto, se o resultado for realmente observable refinement/composition/evolution.**
**Risco de virar apenas “consistency compiler sobre storage existente”: continua real e deve ser testado, não escondido.**

O achado mais importante é este:

> **O núcleo científico correto do AstraDB já está nas SPEC-003/004/009/010. A SPEC-001 ainda descreve uma versão anterior e mais simplista da ideia.**

Portanto, antes de criar a SPEC-011, eu faria uma **SPEC-001 v0.2 completamente revisada**, tornando 003/004/009/010 a autoridade conceitual. Depois fecharia IDs e Control Plane. Só então começaria a implementação distribuída.

Essa sequência evita o pior cenário possível: construir corretamente o banco que a SPEC-001 antiga descreve, em vez do banco muito mais interessante que as SPECs posteriores acabaram descobrindo.
