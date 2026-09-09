# SPEC-002 — Astra Storage Kernel

**Subtitle:** Page Store, MVCC, Commit Journal, Atomic Batches and Crash Recovery
**Status:** Draft 0.2 — proposed boundary; formats and runtime qualification pending
**Type:** Foundational implementation specification
**Depends on:** `SPEC-001 — Invariant-Compiled Consistency`
**Shared contracts:** [SPEC-011](SPEC-011.md) identities/catalog; [SPEC-012](SPEC-012.md) request/receipt/encoding; [SPEC-013](SPEC-013.md) trust profiles
**Reference implementation:** Rust stable
**Scope:** local storage kernel and durable boundary consumed by the distributed consistency runtime
**Out of scope:** protocol synthesis, C0–C5 selection, global routing, Raft, SQL optimizer, vector/graph/AI features

---

## 0. Decision

AstraDB SHALL use a **page-oriented ordered storage engine based on a B+Tree, MVCC, an explicit buffer pool and a redo-first Commit Journal**.

The storage structure is deliberately conventional. AstraDB's research claim is not “a better tree”. The storage kernel exists to provide a deterministic, inspectable substrate on which `SPEC-001` can safely execute different consistency plans.

```text
                         Astra Semantic Runtime
                                 │
                                 │ CompiledBatch
                                 │ PlanContext
                                 ▼
                    ┌──────────────────────────┐
                    │   Transaction Boundary   │
                    │ prepare / commit / abort │
                    └────────────┬─────────────┘
                                 │
                   ┌─────────────┴─────────────┐
                   ▼                           ▼
          ┌──────────────────┐        ┌──────────────────┐
          │  Commit Journal  │        │    MVCC State    │
          │ redo + semantics │        │      B+Tree      │
          └────────┬─────────┘        └────────┬─────────┘
                   │                           │
                   │ durable_lsn               │ dirty pages
                   ▼                           ▼
             ┌───────────┐              ┌─────────────┐
             │ fsync /   │              │ Buffer Pool │
             │ group sync│              └──────┬──────┘
             └───────────┘                     │
                                               ▼
                                          Data Pages
```

The central durability rule is:

> **No acknowledged commit may depend on a modified data page having reached stable storage. It depends on the corresponding Commit Journal decision having reached the required durability boundary.**

Data pages MAY be flushed later.

---

## 1. Why this engine

`SPEC-001` requires the local kernel to support:

- local transactions;
- commutative replicated effects;
- causal application;
- escrow metadata;
- optimistic certification;
- strongly ordered IDC execution;
- prepared distributed mutations;
- exact crash recovery;
- deterministic replay of normalized effects;
- atomic plan/schema metadata changes;
- consistent local snapshots;
- point and range reads.

The minimum substrate is therefore:

```text
ordered keys
+
MVCC
+
atomic batches
+
redo logging
+
prepared transactions
+
predictable range scans
+
explicit durability
```

A page-oriented B+Tree provides this without making compaction policy the center of the project.

---

## 2. Non-goals

SPEC-002 SHALL NOT turn AstraDB into:

- another LSM research project;
- another immutable event store;
- another HTAP engine;
- another vector database;
- another graph database;
- another lakehouse;
- another workflow engine;
- another distributed SQL clone;
- a CXL-only or GPU-dependent system.

The storage kernel is infrastructure, not the scientific identity.

---

## 3. Reference influences

The design follows established ideas from modern page-oriented and log-decoupled database systems.

FoundationDB demonstrates separation between durable transaction logging and later storage-server materialization, while retaining an ordered transactional key-value abstraction.

Reference:

https://apple.github.io/foundationdb/architecture.html

FoundationDB's Redwood and SSD engines demonstrate that B-tree-family storage remains viable for modern SSD-backed distributed systems.

Reference:

https://apple.github.io/foundationdb/configuration.html

LeanStore demonstrates that a modern buffer manager can retain near in-memory behavior for hot working sets while scaling beyond DRAM.

Reference:

https://db.in.tum.de/~leis/papers/leanstore.pdf

Umbra demonstrates a modern page-oriented DBMS architecture using an efficient buffer manager.

Reference:

https://umbra.db.in.tum.de/

AstraDB adopts classic WAL/redo principles but intentionally avoids ordinary undo recovery by ensuring uncommitted mutations are never published into committed MVCC state.

---

# Part I — Persistent state model

## 4. Fundamental physical abstraction

The storage kernel exposes an **ordered versioned keyspace**.

The physical kernel does not understand:

```text
Customer
Account
Order
Invoice
```

It understands:

```text
LogicalKey
VersionStamp
Value
Mutation
Snapshot
```

Conceptually:

```text
LogicalKey -> Value@VersionStamp
```

One logical key MAY have multiple MVCC versions.

---

## 5. Logical keys

```rust
pub struct LogicalKey(pub Vec<u8>);
```

A `LogicalKey` is an opaque canonical byte string produced by the semantic layer.

The B+Tree MUST compare logical keys lexicographically.

Business meaning MUST NOT leak into page comparison code.

---

## 6. Physical ordering

Leaf entries are ordered by:

```text
(logical_key ASC, local_commit_seq DESC)
```

Example:

```text
Account/42 @ 109
Account/42 @ 103
Account/42 @ 91
Account/43 @ 112
Account/43 @ 88
```

A snapshot with `visible_seq = 105` reads:

```text
Account/42 @ 103
```

This representation is the MVP MVCC layout.

---

## 7. No cluster-global physical version

AstraDB SHALL NOT create one global physical commit counter.

Each local storage authority maintains:

```rust
pub struct LocalCommitSeq(pub u64);
```

`LocalCommitSeq` is monotonic only within one storage epoch.

It is not:

- global time;
- causal time;
- global serial order;
- an IDC serial position;
- an escrow generation.

---

## 8. Storage epoch

```rust
pub struct StorageEpoch(pub u64);

pub struct VersionStamp {
    pub epoch: StorageEpoch,
    pub seq: LocalCommitSeq,
}
```

A new destructive restore, replacement or reinitialization MUST create a new storage epoch.

Old-epoch authority MUST NOT silently become valid authority in the new epoch.

---

## 9. Semantic order is separate

`SPEC-001` may attach:

```text
CausalStamp
EscrowEpoch
IdcAuthorityEpoch
SerialPosition
IdcGeneration
PlanGeneration
```

These are semantic metadata.

They MUST NOT be overloaded into `LocalCommitSeq`.

```text
physical MVCC order
        !=
semantic distributed order
```

This separation is foundational.

---

## 10. Namespaces

Initial logical namespaces:

```text
USER
CATALOG
IDC_META
ESCROW
TXN_STATUS
PLAN
REPLICATION
SYSTEM
```

Reserved namespaces MUST NOT be writable through ordinary user operations.

---

## 11. Key codec

The semantic layer SHALL define canonical mem-comparable encoding for at least:

```text
u64
i64
UUID
fixed bytes
UTF-8 string
tuple
```

The exact tuple codec is a separate sub-specification.

The storage engine receives already-canonical bytes.

---

# Part II — Page store

## 12. Page size

The first implementation SHALL use:

```text
8 KiB
```

pages.

The page size is recorded in the manifest.

Opening a database under a mismatched page-size interpretation MUST fail.

Supporting multiple page sizes before benchmarks is explicitly out of scope.

---

## 13. Page identifiers

```rust
pub struct PageId(pub u64);
```

Page `0` is reserved.

The MVP page file uses:

```text
offset = page_id * page_size
```

---

## 14. Page types

```rust
pub enum PageType {
    Internal,
    Leaf,
    FreeList,
    Meta,
}
```

---

## 15. Page header

Conceptual persistent header:

```text
PageHeader {
    magic
    format_version
    page_type
    flags

    page_id
    page_generation

    page_lsn

    item_count
    free_start
    free_end

    left_sibling
    right_sibling

    checksum_crc32c
}
```

All byte offsets SHALL be frozen in format fixtures before compatibility is claimed.

---

## 16. Checksums

Every page MUST carry CRC32C.

CRC32C is accidental-corruption detection, not hostile tamper evidence.

AstraDB SHALL NOT claim cryptographic integrity from a checksum.

---

## 17. Leaf layout

Leaves SHALL use slotted pages.

```text
┌────────────────────────────────────────────┐
│ header                                     │
├────────────────────────────────────────────┤
│ slot array →                               │
│                                            │
│                 free space                 │
│                                            │
│                         ← variable records │
└────────────────────────────────────────────┘
```

---

## 18. Leaf record

Conceptual:

```rust
pub struct LeafRecord {
    pub logical_key: Vec<u8>,
    pub version: VersionStamp,
    pub txn_id: TxnId,
    pub origin: OriginId,
    pub flags: RecordFlags,
    pub semantic_meta: SemanticMeta,
    pub value: Vec<u8>,
}
```

Prepared but undecided mutations MUST NOT appear as normal visible leaf versions.

---

## 19. Record flags

Initial flags:

```text
TOMBSTONE
SYSTEM
HAS_CAUSAL_META
HAS_SERIAL_META
HAS_ESCROW_META
```

---

## 20. Internal pages

Internal pages store:

```text
(separator_key, child_page_id)
```

The engine MUST support:

- binary search;
- child split;
- root split;
- range navigation;
- crash-safe root publication.

Aggressive online merge/redistribution MAY be deferred.

---

## 21. Large values

MVP MAY cap one value at:

```text
1 MiB
```

The limit MUST be explicit.

Future versions MAY add overflow/blob pages.

The storage engine is not an object store.

---

## 22. Prefix compression

Prefix compression is optional after baseline correctness.

It MUST NOT be an MVP dependency.

---

# Part III — Buffer manager and I/O

## 23. Explicit buffer pool

AstraDB SHALL use an explicit buffer pool rather than `mmap` as the default mutable page path.

Reasons:

- explicit dirty ownership;
- explicit WAL-before-page enforcement;
- bounded memory;
- predictable eviction;
- explicit I/O scheduling;
- easier crash reasoning;
- portable path before Linux specialization.

---

## 24. Buffer frame

Conceptual:

```rust
pub struct Frame {
    pub page_id: PageId,
    pub generation: u64,
    pub pin_count: AtomicU32,
    pub dirty: AtomicBool,
    pub page_lsn: AtomicU64,
    pub latch: PageLatch,
    pub bytes: AlignedPage,
}
```

---

## 25. Buffer API

```text
pin(page_id)
allocate(page_type)
mark_dirty(page_lsn)
unpin()
flush(page_id)
flush_up_to(lsn)
evict_one()
```

Pinned pages MUST NOT be evicted.

---

## 26. Replacement policy

MVP SHALL use CLOCK or an equally simple policy.

Do not start with learned cache replacement.

Later benchmarking MAY compare more advanced policies.

---

## 27. Latching

The correctness-first implementation MAY use a per-frame reader/writer latch.

The first benchmarkable version MUST NOT retain one process-wide B+Tree mutex.

Optimistic page reads MAY be added later.

---

## 28. I/O abstraction

```rust
pub trait PageIo {
    fn read_page(&self, id: PageId, dst: &mut [u8]) -> Result<()>;
    fn write_page(&self, id: PageId, src: &[u8]) -> Result<()>;
    fn sync_data(&self) -> Result<()>;
}
```

Journal I/O SHALL use a separate sequential interface.

---

## 29. Portable baseline

The default path SHALL use portable positional file I/O.

Linux MAY later provide an interchangeable backend using:

```text
io_uring
registered buffers
batched I/O
O_DIRECT experiments
```

All backends MUST pass identical recovery tests.

---

## 30. `mmap`

`mmap` MAY be used by diagnostics or read-only tools.

It SHALL NOT be the v1 mutable-page default.

---

# Part IV — MVCC

## 31. MVCC objective

MVCC provides:

- snapshot reads;
- local read-your-writes;
- stable visibility;
- non-blocking readers versus committed writers;
- version retention for active snapshots.

MVCC does not select C0–C5.

---

## 32. Local snapshot

```rust
pub struct LocalSnapshot {
    pub epoch: StorageEpoch,
    pub visible_seq: LocalCommitSeq,
}
```

Default transaction snapshot:

```text
visible_seq = durable_commit_seq
```

at begin.

---

## 33. Visibility rule

A version is locally visible if:

```text
record.epoch == snapshot.epoch
AND
record.seq <= snapshot.visible_seq
AND
record is the newest qualifying version for the key
```

A qualifying tombstone means `NotFound`.

Higher layers MAY impose causal/certified/serial visibility constraints.

---

## 34. Point read

```text
seek logical_key
scan versions newest -> oldest
skip seq > snapshot.visible_seq
return first visible non-tombstone
```

---

## 35. Range scan

Range scan MUST emit at most one visible value per logical key.

Algorithm:

1. seek start key;
2. select visible version;
3. skip remaining versions of same key;
4. advance;
5. continue over leaf siblings.

---

## 36. Transaction write set

Uncommitted local mutations reside in transaction memory.

Read resolution:

```text
transaction write set
        ↓
snapshot B+Tree
```

No other transaction's uncommitted write is visible.

---

## 37. Storage mutation

```rust
pub enum StorageMutation {
    Put {
        key: LogicalKey,
        value: Vec<u8>,
        semantic_meta: SemanticMeta,
    },
    Delete {
        key: LogicalKey,
        semantic_meta: SemanticMeta,
    },
}
```

---

## 38. No universal same-key abort

The storage kernel MUST NOT automatically abort all concurrent same-key writes.

Under C1, operations such as semantically commutative increments may be valid.

Conflict meaning belongs to the compiled plan.

The storage layer only provides the physical primitives.

---

## 39. Expected-version primitive

For compare-and-swap semantics:

```rust
pub enum ExpectedVersion {
    Any,
    Absent,
    Exact(VersionStamp),
}
```

Mismatch returns a typed conflict.

---

## 40. Read-set capture

C4 MAY request capture of:

```text
key -> VersionStamp
range -> range token
```

This MUST be opt-in by plan.

AstraDB SHALL NOT pay SSI-like tracking overhead on every transaction.

---

# Part V — Compiled batches

## 41. CompiledBatch

The semantic runtime passes a normalized business-operation batch. All identity types are distinct newtypes owned by SPEC-011; SPEC-012 owns request allocation and canonical result records. The records here are normative logical schemas, not Rust memory images or a frozen wire ABI.

```rust
pub struct CompiledBatch {
    pub batch_version: u32,
    pub txn_id: TxnId,
    pub request_key: RequestKey,
    pub request_hash: RequestHash,
    pub operation: OperationRef,
    pub operation_hash: OperationHash,
    pub contract_hash: ContractHash,
    pub schema_hash: SchemaHash,
    pub plan: PlanRef,
    pub idc_bindings: Vec<IdcBinding>,
    pub consistency_class: ConsistencyClass,
    pub origin: Option<OriginId>,
    pub semantic_evidence: Vec<ProtocolRecordRef>,
    pub captured_inputs: CanonicalBytes,
    pub mutations: Vec<StorageMutation>,
    pub protocol_mutations: Vec<ProtocolMutation>,
    pub terminal_outcome: Option<TerminalOutcome>,
    pub semantic_digest: SemanticDigest,
}
```

`PlanRef` binds `PlanId`, `PlanGeneration` and `PlanHash`. `IdcBinding` binds `IdcId`, `IdcGeneration` and `IdcAuthorityEpoch`; the vector is canonical, sorted and duplicate-free. `OriginId` is assigned at origin commit and preserved on replay; prepare may precede its allocation. `semantic_evidence` references recoverable typed causal, authority, reservation or serial records; a digest of unavailable evidence is insufficient. Captured inputs include the accepted reads/operands needed for deterministic result recovery; mutable current state cannot substitute for them.

`terminal_outcome: None` is permitted for prepare or participant-local installation before the whole invocation is final. A locally final operation requires a terminal record atomically with its effects. A composite participant instead persists its prepared digest, accepted result material and unique decision reference; SPEC-008 requires publication/completion evidence before RequestHome installs the terminal receipt. Recovery must produce byte-identical result material without reevaluation. Client success MUST NOT be inferred from a participant's local `COMMITTED` state.

`SemanticDigest` hashes the canonical normalized semantic payload, excluding its own digest, physical MVCC/LSN, transport envelopes and certificates that later reference this digest. The payload binds all identities, effects, captured inputs and accepted result material. Later evidence is append-only and references that immutable digest; it cannot create a circular digest or alter accepted effects. A repeated identity with different semantic payload is `RequestIdentityMismatch`/corruption, never an idempotent success.

---

## 42. ProtocolMutation

```rust
pub enum ProtocolMutation {
    PutRecord(ProtocolRecordWrite),
    SetTxnStatus(TxnStatusTransition),
}

pub struct ProtocolRecordWrite {
    pub key: ProtocolRecordKey,
    pub expected: ExpectedRecordRevision,
    pub next: VersionedProtocolRecord,
}
pub struct TxnStatusTransition {
    pub txn_id: TxnId,
    pub request_key: RequestKey,
    pub request_hash: RequestHash,
    pub expected_revision: ExpectedRecordRevision,
    pub next: TxnStatusRecord,
}
pub struct TxnStatusRecord {
    pub revision: RecordRevision,
    pub phase: TxnPhase,
    pub plan: PlanRef,
    pub idc_bindings: Vec<IdcBinding>,
    pub prepared_digest: Option<SemanticDigest>,
    pub decision_ref: Option<ProtocolRecordRef>,
    pub accepted_result: Option<CanonicalBytes>,
    pub terminal_outcome: Option<TerminalOutcome>,
}
pub enum TxnPhase { Bound, Admitted, Prepared, Installed, Aborted, Terminal }
pub enum TerminalOutcome {
    Committed(FinalReceiptV1),
    Rejected(FinalReceiptV1),
}
```

`ExpectedRecordRevision = Absent | Exact(RecordRevision)`: there is no unchecked overwrite. Every status key carries immutable request binding plus a monotonic record revision; recovery validates legal predecessor/successor transitions. `Terminal` requires a matching terminal outcome; other phases cannot contain one. Terminal outcomes cannot change. A retry returns the identical stored record or typed mismatch; it never rewinds the phase. `Aborted` here denotes a protocol execution decision, not automatically a final business rejection. SPEC-012 owns the client mapping; SPEC-007/008 own legal prepared/installed transitions and publication gates.

`ProtocolRecordKey = (record_kind, scope_key, record_id)`. `VersionedProtocolRecord = (record_kind, record_version, canonical_payload)` uses a closed versioned registry: escrow state/transfer records (SPEC-006), certification/reservations (SPEC-007), decision/publication records (SPEC-008), migration/fences (SPEC-009), catalog/grants (SPEC-011), and request bindings/receipts/frontiers (SPEC-005/012). Each decoder validates the owning schema and permitted transition before persistence. Unknown kinds fail closed; this is not an arbitrary user-writable blob. Protocol references bind kind/version/key and payload digest, with recoverable bytes retained.

Protocol state MUST join the same local atomic batch as user state whenever correctness depends on both. Internal transfer, fence, catalog and cursor transitions use `ProtocolOnlyBatch { internal_record_id: ProtocolRecordKey, authorizing_evidence: Vec<ProtocolRecordRef>, writes: Vec<ProtocolRecordWrite> }`. It permits no user mutations and never fabricates a client request or operation identity. Duplicate internal records compare immutable content just as business batches do. The common journal boundary atomically validates all CAS preconditions and persists all writes or none.

---

## 43. Local atomicity

For one storage node:

```text
ALL mutations in CompiledBatch become visible
OR
NONE become visible
```

This remains true after crash.

---

## 44. Plan and schema identity

Every business batch carries exact:

```text
RequestKey / RequestHash / TxnId
OperationRef / OperationHash / ContractHash
PlanRef (PlanGeneration / PlanHash)
schema_hash
IdcBinding[] (definition generation / authority epoch)
```

The runtime MUST reject retired/incompatible generations for new admissions before persistence using SPEC-011's grants and fences. Resolving an admitted prepare or replaying an authenticated committed record uses its retained original plan under SPEC-009; it is not new admission. Local catalog copies cannot authorize themselves.

Recovery retains these identities so it can understand under which plan the transaction committed.

---

# Part VI — Commit Journal

## 45. Journal role

The Commit Journal is the local durability authority.

It stores:

- committed batches;
- prepared batches;
- commit/abort decisions;
- semantic envelope;
- protocol mutations;
- checkpoint markers;
- storage-generation changes required for recovery.

It is not an eternal historical truth log.

---

## 46. Explicit difference from HeraclitusDB

HeraclitusDB's immutable event log is its canonical history.

AstraDB's journal is:

```text
durability + recovery + replication substrate
```

Old journal segments MAY be reclaimed after all safety horizons pass.

AstraDB does not promise `AS OF LSN` forever.

This distinction MUST remain architectural, not merely marketing.

---

## 47. Journal segments

Default:

```text
64 MiB
```

per segment.

Naming:

```text
journal-0000000000000001.astj
journal-0000000000000002.astj
...
```

This is a baseline default, not a performance claim.

---

## 48. Journal LSN

```rust
pub struct JournalLsn(pub u64);
```

Monotonic within one storage epoch.

It is physical journal order only.

---

## 49. Journal frame

Conceptual:

```text
FrameHeader {
    magic
    format_version
    record_kind
    flags
    total_len
    payload_len
    lsn
    txn_id
    crc32c
}

payload

FrameTrailer {
    total_len
}
```

A trailer assists torn-tail detection.

---

## 50. Record kinds

```rust
pub enum JournalRecordKind {
    CommitBatch,
    PrepareBatch,
    CommitPrepared,
    AbortPrepared,
    CheckpointBegin,
    CheckpointEnd,
    CatalogGeneration,
    StorageEpochChange,
}
```

---

## 51. Canonical encoding

Persistent encoding MUST be:

- deterministic;
- explicitly versioned;
- architecture independent;
- bounds checked;
- endian explicit;
- independent of Rust in-memory enum layout.

Persisting `repr(Rust)` memory images is forbidden.

---

## 52. Checksums

Each journal frame MUST have CRC32C.

Rules:

```text
invalid frame at physical tail
    => may be torn tail

invalid required frame in middle
    => corruption, fail closed
```

The scanner MUST NOT silently skip a corrupted committed record.

---

## 53. Durability modes

Production:

```text
SYNC
GROUP_SYNC
```

Testing only:

```text
UNSAFE_NO_FSYNC
```

Unsafe mode MUST be obnoxiously explicit and never default.

---

## 54. Commit point

A local storage commit may be acknowledged only after its decision, exact result material and required identity/protocol records have crossed the configured durable journal barrier. This ACK proves local installation/durability only. Client finality additionally requires the contract's replicated witnesses and any SPEC-008 publication/completion barriers.

No success before this point.

---

## 55. Group commit

MVP SHOULD implement:

```text
transaction threads
      ↓
commit queue
      ↓
local commit coordinator
      ├─ assign LocalCommitSeq
      ├─ encode frames
      ├─ append
      ├─ one fsync for batch
      ├─ advance durable barriers
      └─ wake transactions
```

One local commit coordinator is acceptable initially.

Its scalability must be measured before inventing a more complex one.

---

## 56. Durable barriers

```rust
AtomicU64 durable_commit_seq;
AtomicU64 durable_journal_lsn;
```

Readers MUST NOT observe a version above `durable_commit_seq`.

---

## 57. WAL-before-page

A dirty page with:

```text
page_lsn = X
```

MUST NOT be written to stable storage until:

```text
durable_journal_lsn >= X
```

This is non-negotiable.

---

## 58. Install-before-fsync optimization

The engine MAY eventually install versions into the in-memory tree before fsync if:

- readers are gated by `durable_commit_seq`;
- page flushing respects WAL-before-page;
- a failed fsync never advances visibility.

The first implementation MAY instead use the simpler:

```text
journal
fsync
install
ack
```

path.

Correctness precedes cleverness.

---

# Part VII — Prepared transactions

## 59. Transaction states

Local:

```text
ACTIVE -> COMMITTING -> COMMITTED
     \-> ABORTED
```

Prepared:

```text
ACTIVE
  ↓
PREPARING
  ↓
PREPARED
  ├─> COMMITTED
  └─> ABORTED
```

---

## 60. Prepared writes are not ordinary leaf versions

`PrepareBatch` is durable, but its data MUST remain invisible.

Prepared mutations are represented by:

```text
journal PrepareBatch
+
PreparedTxn metadata
```

They enter committed MVCC state only after a durable commit decision.

This is what allows redo-only recovery.

---

## 61. Prepare

```text
1. validate batch
2. validate plan/schema generation
3. append PrepareBatch
4. durable barrier
5. register PREPARED
6. reply PREPARED
```

---

## 62. Commit prepared

```text
1. validate PreparedToken
2. append CommitPrepared
3. durable barrier
4. assign/confirm local commit sequence
5. install all user mutations
6. install all protocol mutations
7. mark COMMITTED
8. release local staging resources only; retain protocol reservations/read gates until their owner authorizes publication/release
9. reply locally INSTALLED/COMMITTED; this is not a client final ACK
```

---

## 63. Abort prepared

```text
1. append AbortPrepared when durable abort is required
2. durable barrier
3. mark ABORTED
4. release resources only under the validated unique abort decision and protocol-owner rules
```

---

## 64. In-doubt recovery

After crash:

```text
PrepareBatch
without
CommitPrepared/AbortPrepared
```

becomes:

```text
IN_DOUBT
```

The local storage engine MUST NOT invent the outcome.

The distributed protocol resolves it.

---

## 65. Prepared identity

A prepare MUST bind:

```text
txn_id
request_key / request_hash
operation / operation_hash / contract_hash / schema_hash
plan (generation + hash)
idc_bindings (definition generation + authority epoch)
semantic_digest / accepted result material
original decision-authority reference and participant manifest
```

Mismatched commit decisions MUST be rejected.

---

# Part VIII — Commit/recovery invariants

## 66. Acknowledged commit invariant

After any supported crash:

```text
every acknowledged commit
MUST recover as committed
```

---

## 67. No phantom commit

```text
no durable commit decision
=>
must not recover as committed
```

---

## 68. Atomic local batch

After recovery:

```text
visible mutation count
=
all mutations
```

or zero.

No partial batch.

---

## 69. Idempotent commit

`TxnId` identifies the execution allocated by SPEC-012's durable RequestHome mapping. `RequestKey` is routed before that ID exists; concurrent gateways MUST NOT allocate independent executions for the same key. The binding is durable before any participant dispatch.

Duplicate commit or replicated-install requests MUST NOT apply mutations twice.

---

## 70. Txn status store

Logical system keys:

```text
TXN_STATUS/<txn_id>
REQUEST_BINDING/<tenant>/<namespace>/<stable_request_id>
```

Possible states:

```text
BOUND -> ADMITTED -> PREPARED -> INSTALLED -> TERMINAL
                     \-> ABORTED
```

Direct local execution may go from `ADMITTED` to `TERMINAL` atomically. `IN_DOUBT` is a recovery/knowledge condition on a prepared execution with unresolved decision; it never authorizes abort. Binding records at RequestHome and participant status records retain the same immutable identity. Result retention, anti-reexecution tombstones and namespace retirement follow SPEC-012; migration/GC pins follow SPEC-009/011. Expiring result bytes is not permission to remove the binding and execute again.

---

## 71. Unknown client outcome

A connection may fail after durable commit but before the client receives the response.

Resolve by `RequestKey` through SPEC-012; the client need not know `TxnId`. Public replies distinguish:

```text
Committed(FinalReceiptV1)
Rejected(FinalReceiptV1)
Unavailable / OutcomeUnknown
ResultExpired / IdentityExpired
```

Never advise blind duplicate execution.

---

# Part IX — B+Tree mutation and structural recovery

## 72. Insert path

```text
locate leaf
latch leaf
insert sorted physical version
split if needed
propagate separator
set page_lsn
mark dirty
release
```

---

## 73. Structural redo strategy

MVP SHOULD use deterministic logical redo:

```text
replay committed logical mutations
```

and allow the B+Tree to reproduce whatever physical splits are required.

This avoids coupling the journal to page choreography.

If this proves insufficient, physiological structural records MAY be added later.

---

## 74. Root publication

The active root page ID is stored in the database manifest.

Root changes MUST be crash-safe.

---

# Part X — Manifest and files

## 75. Database manifest

Two generations:

```text
MANIFEST.A
MANIFEST.B
```

Conceptual contents:

```text
magic
format_version
manifest_generation
storage_epoch
page_size
root_page_id
allocator metadata
checkpoint_lsn
checkpoint_commit_seq
journal segment
checksum
```

Startup selects the highest valid generation.

---

## 76. Manifest publication

```text
write inactive manifest
fsync
publish by generation
```

A torn new manifest MUST leave an older valid generation.

---

## 77. Directory layout

```text
db/
  LOCK
  MANIFEST.A
  MANIFEST.B
  data.astr
  journal/
    journal-0000000000000001.astj
    ...
  tmp/
```

---

## 78. File ownership

MVP allows one process to own one database directory for writes.

Two independent write processes opening the same directory MUST fail.

Distributed nodes use separate directories.

---

## 79. File lock

A local lock file protects operational ownership.

It is not distributed consensus.

---

# Part XI — Checkpoints

## 80. Checkpoint purpose

A checkpoint limits recovery work.

It means:

> persisted pages plus journal after `checkpoint_lsn` are sufficient to reconstruct committed local state.

---

## 81. Fuzzy checkpoint

```text
1. append CheckpointBegin(target_lsn)
2. flush eligible dirty pages up to target
3. continue ordinary commits
4. wait until required pages are stable
5. publish manifest with checkpoint_lsn
6. append CheckpointEnd
7. allow journal truncation subject to all horizons
```

---

## 82. Dirty-page eligibility

A background writer may persist a dirty page only if:

```text
page_lsn <= durable_journal_lsn
```

---

## 83. Recovery start

Recovery begins from:

```text
manifest.checkpoint_lsn
```

unless older journal history is required by prepared transactions or external consumers.

---

# Part XII — Recovery

## 84. Startup recovery algorithm

```text
1. acquire file ownership
2. validate manifests
3. choose newest valid manifest
4. load storage epoch/root
5. validate formats
6. locate checkpoint journal position
7. scan journal forward
8. truncate only valid torn tail
9. fail on required mid-log corruption
10. reconstruct txn outcomes
11. redo committed batches after checkpoint
12. rebuild unresolved prepares
13. restore local commit sequence
14. restore protocol metadata
15. verify B+Tree structural invariants
16. enter RECOVERED_LOCAL
17. reconcile distributed protocol state
18. resolve IN_DOUBT
19. advertise READY only when semantically safe
```

---

## 85. Recovery idempotence

Running recovery repeatedly without new commits MUST yield the same logical state.

---

## 86. Corruption behavior

Required committed history corruption:

```text
FAIL
```

Do not skip.

Do not infer.

Do not “repair” with guessed values.

Replicated repair belongs to later specs.

---

# Part XIII — MVCC GC

## 87. AstraDB is not unlimited time travel

Old MVCC versions SHALL be reclaimed when no longer needed.

This is another explicit difference from HeraclitusDB.

---

## 88. Safety horizons

At minimum:

```text
active_snapshot_horizon
replication_horizon
backup_horizon
prepared_txn_horizon
```

The effective GC horizon is the most conservative requirement.

---

## 89. Retention rule

For each logical key, GC MUST preserve:

- every version visible to any retained snapshot;
- versions required by replication/backup;
- the newest base version required below the horizon;
- anything referenced by unresolved prepared work.

---

## 90. Tombstones

A tombstone may be removed only when no older version can legally resurrect after its removal.

Replication MAY impose an additional anti-resurrection horizon.

---

## 91. Snapshot registration

Long-lived snapshots MUST register through a guard.

Metrics MUST expose which snapshot prevents GC.

---

# Part XIV — Indexes

## 92. Secondary indexes

Secondary indexes are additional ordered namespaces.

Example:

```text
INDEX/<index_id>/<index_key>/<primary_key>
```

Base-row mutation and local index mutation MUST be in the same atomic batch.

---

## 93. Unique indexes

Local B+Tree uniqueness does not imply global uniqueness.

`SPEC-001` chooses the distributed coordination/authority needed for a UNIQUE invariant.

Storage only provides atomic local primitives.

---

# Part XV — Support for C0–C5

## 94. C0 LOCAL

Storage provides:

```text
local snapshot
atomic batch
durable journal
```

No remote coordination is introduced by storage.

---

## 95. C1 COMMUTATIVE

Storage MUST support:

- stable operation identity;
- idempotent replicated install;
- atomic local application;
- semantic replication records.

It MUST NOT force semantic serialization merely because two physical writes touch the same key.

---

## 96. C2 CAUSAL

Storage MUST be able to atomically persist:

```text
business mutation
+
causal frontier metadata
```

The causal scheduler is outside the page engine.

---

## 97. C3 ESCROW

Rights consumption and business mutation MUST commit together.

Example:

```text
Product.stock -= 5
Escrow.local_rights -= 5
```

Any crash result where only one becomes visible is a release-blocking bug.

---

## 98. C4 CERTIFIED

Storage provides:

```text
stable snapshot
read-version capture
prepare
commit/abort decision
```

Certification logic is above storage.

---

## 99. C5 SERIAL

The IDC runtime supplies semantic order.

Storage persists ordered decisions.

The B+Tree does not implement Raft.

---

# Part XVI — Replication boundary

## 100. Replication uses semantic commits

Upper replication layers consume:

```text
CommittedBatch
```

not raw page images.

This decouples physical layout from C1/C2/C3 semantics.

---

## 101. Replica-local versions

When a remote operation is installed:

```text
new LocalCommitSeq
```

is assigned locally.

Original identity remains in:

```text
OriginId
TxnId
OperationId
causal/serial metadata
```

Physical versions therefore need not match across replicas.

---

## 102. Origin identity

```rust
pub struct OriginId {
    pub node_id: NodeId,
    pub origin_epoch: OriginEpoch,
    pub origin_seq: OriginSeq,
}
```

Used for deduplication and tracing.

---

## 103. Duplicate delivery

Already-applied origin/txn identity:

```text
do not mutate again
```

Return idempotent success or typed `AlreadyApplied`.

---

## 104. Journal consumers

Replication/runtime consumers use:

```rust
pub struct JournalCursor {
    pub epoch: StorageEpoch,
    pub lsn: JournalLsn,
}
```

Consumer progress contributes to journal-retention horizon.

---

# Part XVII — Journal retention

## 105. Deletion rule

A journal segment may be reclaimed only when older than all:

```text
checkpoint requirement
replication requirement
backup requirement
prepared-transaction requirement
```

The minimum retained LSN MUST be observable.

---

# Part XVIII — System metadata atomicity

## 106. Escrow state

Escrow rights are not cache.

They are durable protocol state.

They MUST participate in atomic batches.

---

## 107. IDC metadata

`IdcGeneration` and `IdcAuthorityEpoch` changes are independently durable and versioned in `IdcBinding`; SPEC-011 owns their allocation and scope. Equal integers cannot be substituted.

A node MUST reject stale-generation batches outside the accepted drain window.

---

## 108. Plan generation

Plan activation metadata MUST be persisted so crash recovery cannot combine:

```text
new plan
+
old incompatible protocol state
```

as though it were valid.

---

## 109. Old-plan drain

During migration, runtime may resolve previously admitted work under:

```text
G
G+1
```

only as authorized by SPEC-009's retained evidence. The baseline closes old admission and drains before target activation; it does not admit incompatible new requests concurrently. Mixed-generation admission requires a separately qualified compatibility certificate. A time limit alone cannot authorize it.

Every committed batch records exact generation/hash.

---

# Part XIX — Error and safety states

## 110. Error taxonomy

Initial categories:

```rust
pub enum StorageError {
    Io,
    DiskFull,
    Corruption,
    ChecksumMismatch,
    UnsupportedFormat,
    InvalidManifest,

    KeyTooLarge,
    ValueTooLarge,
    BatchTooLarge,

    Conflict,
    StaleEpoch,
    StalePlan,
    StaleSchema,

    TxnAlreadyCommitted,
    TxnAlreadyAborted,
    TxnInDoubt,

    ReadOnly,
    NotReady,
}
```

---

## 111. Node readiness

```text
OPENING
RECOVERING_LOCAL
WAITING_PROTOCOL_RECONCILIATION
READY
DEGRADED_STORAGE
READ_ONLY_SAFETY
FAILED
```

A locally recovered node with unresolved distributed authority MUST NOT advertise full readiness.

---

## 112. Disk full

Journal append failure:

```text
commit fails
```

Fsync failure:

```text
no success response
```

Page-flush failure with intact journal may enter:

```text
DEGRADED_STORAGE
```

and trigger backpressure.

---

## 113. Backpressure

Triggers MAY include:

```text
dirty page ratio
journal retained bytes
free disk
checkpoint lag
prepared txn count
buffer pressure
```

The node MUST throttle before durability/recovery safety is endangered.

---

## 114. Read-only safety

If durable writes become impossible but pages are readable, the node MAY enter `READ_ONLY_SAFETY`.

Upper semantic runtime decides which read modes remain legal.

---

# Part XX — Persistent format discipline

## 115. No Rust memory images on disk

`repr(Rust)` layout MUST NOT be persisted.

Dedicated encoders/decoders are mandatory.

---

## 116. Endianness

Persistent numeric endianness MUST be explicit.

Default storage format uses little-endian fixed-width integers except ordered key codecs that intentionally use order-preserving encodings.

---

## 117. Independent format versions

Persist separately:

```text
page_format_version
journal_format_version
manifest_format_version
key_codec_version
```

Unknown mandatory versions fail open attempts.

---

## 118. Forward compatibility

Unknown optional fields may be skipped only if explicitly marked skippable.

Unknown mandatory semantics MUST fail closed.

---

# Part XXI — Rust implementation boundaries

## 119. Initial workspace

Do not begin with thirty crates.

Recommended:

```text
crates/
  astra-core
  astra-storage
  astra-runtime
  astra-server
```

Inside `astra-storage`:

```text
page/
btree/
buffer/
mvcc/
journal/
txn/
checkpoint/
recovery/
gc/
verify/
io/
```

---

## 120. `astra-core`

Contains:

```text
TxnId
OperationId
PlanHash
SchemaHash
IdcId
ConsistencyClass
NodeId
OriginId
VersionStamp
JournalLsn
```

No file I/O.

---

## 121. `astra-storage`

Owns:

```text
page format
B+Tree
buffer pool
MVCC
journal
local atomic commit
prepared state
checkpoint
recovery
GC
verification
```

---

## 122. `astra-runtime`

Consumes compiled plans and maps operations into protocol paths.

C0–C5 runtime implementations belong to later SPECs.

---

## 123. DurableStorageKernel boundary

The semantic runtime depends on this kernel boundary, with the native Astra B+Tree as the selected implementation. A reference in-memory backend has an explicit simulated durability model; an experimental PostgreSQL/other backend is a research adapter. Every backend declares support for durable prepare, atomic metadata/result writes, recoverable semantic records, snapshots and retained references; unsupported obligations disable the corresponding plans. No adapter may silently emulate durable prepare with volatile memory or weaken final ACKs. Page/checkpoint tooling remains native-engine specific.

```rust
pub trait DurableStorageKernel {
    fn begin(&self, options: TxnOptions) -> Result<Transaction>;

    fn commit(&self, batch: CompiledBatch) -> Result<CommitResult>;
    fn commit_protocol(&self, batch: ProtocolOnlyBatch) -> Result<CommitResult>;

    fn prepare(&self, batch: CompiledBatch) -> Result<PreparedToken>;

    fn commit_prepared(
        &self,
        token: PreparedToken,
        decision: CommitDecision,
    ) -> Result<CommitResult>;

    fn abort_prepared(&self, token: PreparedToken, decision: AbortDecision) -> Result<()>;
    fn resolve(&self, request: &RequestKey) -> Result<LocalRequestEvidence>;

    fn checkpoint(&self) -> Result<CheckpointInfo>;
    fn verify(&self, mode: VerifyMode) -> Result<VerifyReport>;
}
```

---

## 124. Transaction interface

```rust
pub trait StorageTxn {
    fn snapshot(&self) -> LocalSnapshot;
    fn get(&mut self, key: &LogicalKey) -> Result<Option<Value>>;
    fn scan(&mut self, range: KeyRange) -> Result<ScanIter>;
    fn put(&mut self, key: LogicalKey, value: Value) -> Result<()>;
    fn delete(&mut self, key: LogicalKey) -> Result<()>;
    fn build_batch(self, ctx: PlanContext) -> Result<CompiledBatch>;
}
```

---

## 125. Commit result

```rust
pub struct CommitResult {
    pub identity: BusinessTxnOrInternalRecord,
    pub local_version: VersionStamp,
    pub journal_lsn: JournalLsn,
    pub local_durability: DurabilityState,
    pub stored_evidence: Vec<ProtocolRecordRef>,
}
```

---

## 126. Unsafe Rust policy

Default to safe Rust.

Persistent decoders and journal parsers SHOULD forbid unsafe code.

Any unsafe storage optimization requires:

- documented invariant;
- focused benchmark showing need;
- fuzz coverage;
- property tests;
- code review marker.

---

# Part XXII — Verification tooling

## 127. Storage verifier

Command:

```text
astra storage verify
```

Checks:

- page checksums;
- page IDs;
- slot ordering;
- B+Tree separator ordering;
- child reachability;
- sibling linkage;
- MVCC physical ordering;
- duplicate physical version identities;
- free-list sanity;
- manifest consistency.

---

## 128. Journal verifier

```text
astra journal verify
```

Checks:

- segment order;
- frame boundaries;
- CRC32C;
- monotonic LSN;
- transaction-decision consistency;
- checkpoint references.

---

## 129. Diagnostic tools

```text
astra storage info
astra storage dump-page <id>
astra journal inspect
astra txn status <txn_id>
astra checkpoint
```

Diagnostic read-only tools MUST NOT mutate storage.

---

# Part XXIII — Observability

## 130. Required metrics

```text
storage_get_total
storage_scan_total
storage_put_total

buffer_hits
buffer_misses
buffer_dirty_pages
buffer_evictions

journal_append_bytes
journal_fsync_total
journal_group_size
journal_retained_bytes

commit_total
commit_latency
prepare_total
prepared_in_doubt

checkpoint_total
checkpoint_duration
checkpoint_lag_lsn

mvcc_versions
mvcc_gc_versions
oldest_snapshot_seq
mvcc_versions_examined_per_read

page_split_total
page_checksum_failure_total

recovery_replayed_batches
recovery_duration
```

---

## 131. Tracing

A commit trace SHOULD contain:

```text
txn_id
operation_id
plan_hash
consistency_class
journal_lsn
local_commit_seq
affected_idcs
phase timings
```

User values MUST NOT be logged by default.

---

# Part XXIV — Fault injection

## 132. Mandatory crash hooks

Inject at:

```text
before journal append
mid journal write
before fsync
after fsync before publish
during page split
before page flush
mid page write
before manifest publish
after manifest write before fsync
during checkpoint
during prepare
after prepare before decision
after commit decision before install
```

---

## 133. Fault types

Where possible:

```text
process abort
kill -9 equivalent
short write
I/O error
fsync error
reopen
```

---

# Part XXV — Correctness properties

## 134. P1 — No lost acknowledged commit

```text
ACK => recovered
```

---

## 135. P2 — No phantom commit

```text
no durable decision => not committed after recovery
```

---

## 136. P3 — Atomic local batch

```text
all or none
```

---

## 137. P4 — Stable snapshot

Newer commits do not change an existing local snapshot.

---

## 138. P5 — Read-your-writes

Own transaction buffer shadows snapshot state.

---

## 139. P6 — Replay idempotence

Redo twice yields the same logical state.

---

## 140. P7 — B+Tree ordering

All physical entries remain globally ordered.

---

## 141. P8 — WAL-before-page

No stable page depends on non-durable journal history.

---

## 142. P9 — Prepared safety

Prepared without decision never becomes committed visibility.

---

## 143. P10 — Epoch fencing

Old storage/protocol epoch authority is never accepted as current authority.

---

# Part XXVI — Testing strategy

## 144. Reference model

Tests SHALL include a deliberately slow model:

```text
BTreeMap<LogicalKey, Vec<Version>>
```

Random workloads compare the real engine against the model.

---

## 145. Property tests

Generate:

```text
random inserts
random deletes
random snapshots
random crash points
random duplicate replication
random prepare/commit/abort
random GC horizons
random page splits
```

---

## 146. Fuzz targets

```text
page decoder
journal decoder
manifest decoder
key codec
recovery scanner
compiled batch decoder
```

Malformed bytes must return errors, not undefined behavior.

---

## 147. Formal-model targets

TLA+ or equivalent models SHOULD additionally cover the local-only targets below. Distributed features MUST pass the mandatory FM-1/2/3 gates in SPEC-010; this local recommendation cannot waive them:

```text
durable commit barrier
prepare/commit/abort states
manifest A/B publication
journal truncation horizon
atomic business + escrow-rights mutation
```

---

# Part XXVII — Benchmark plan

## 148. Storage microbenchmarks

```text
sequential insert
random insert
hot point read
cold point read
range scan
update-heavy MVCC
mixed read/write
long version chain
group commit
checkpoint under load
recovery after large journal tail
```

---

## 149. Semantic workloads

Also run:

```text
TPC-C-derived local subset
inventory workload
banking workload
causal order workflow
```

Storage performance cannot be evaluated only on synthetic KV operations when AstraDB exists to execute compiled business operations.

---

## 150. Baselines

Potential reproducible baselines:

```text
RocksDB
SQLite WAL
PostgreSQL local transactions
FoundationDB where semantics/configuration are comparable
```

Semantic differences must be disclosed.

---

## 151. Metrics

Measure at least:

```text
throughput
p50/p95/p99 latency
fsync latency
group size
page read/write count
write amplification
journal bytes/op
recovery time
checkpoint interference
version-chain steps/read
buffer hit ratio
```

---

## 152. No invented target numbers

Do not claim:

```text
1M TPS
10µs commits
zero-copy
lock-free everywhere
```

until measured.

Correctness targets may be absolute.

Performance targets come after baseline.

---

# Part XXVIII — Optimization order

## 153. Priority

```text
1. correctness
2. recovery determinism
3. bounded resource use
4. observability
5. latency
6. throughput
7. platform-specific acceleration
```

Not the traditional project-management masterpiece:

```text
1. io_uring
2. SIMD
3. GPU
4. discover durability later
```

---

## 154. Linux phase

After portable baseline, experiments MAY include:

```text
io_uring
registered buffers
fallocate
direct I/O
batched writes
```

All must pass the same crash suite.

---

## 155. No GPU

GPU is out of scope for the storage kernel.

---

## 156. No CXL dependency

AstraDB MAY experiment with CXL in later research.

The core database MUST remain correct on ordinary commodity hardware.

---

# Part XXIX — Security and corruption

## 157. Encryption deferred

At-rest and backup protection profiles belong to SPEC-013. A plaintext local research profile is explicitly limited to its declared environment and has no encrypted-at-rest claim.

Persistent formats SHOULD reserve versioned flags for future encryption.

---

## 158. Checksums are not authentication

CRC32C detects accidental corruption.

It does not prove hostile tamper resistance.

---

## 159. Parsers treat disk as untrusted input

Bounds checking is mandatory.

A corrupt page or journal record MUST NOT create memory-unsafe behavior.

---

# Part XXX — Milestones

## 160. S0 — Persistent formats

Implement:

```text
manifest
page header
journal frame
version stamp
```

Exit:

- deterministic encode/decode;
- corruption fixtures;
- unsupported version rejection.

---

## 161. S1 — Page manager

Implement:

```text
allocation
page I/O
checksums
manifest A/B
buffer pool
```

Exit:

repeated reopen/crash does not lose root/allocator state.

---

## 162. S2 — Single-thread B+Tree

Implement:

```text
get
insert physical version
tombstone
range scan
leaf split
root split
```

Exit:

random differential tests pass.

---

## 163. S3 — MVCC

Implement:

```text
LocalSnapshot
visibility
write set
read-your-writes
tombstones
snapshot registration
```

Exit:

random history equals reference model.

---

## 164. S4 — Commit Journal

Implement:

```text
segments
frames
CRC32C
append
fsync
CommitBatch
tail recovery
```

Exit:

crashes around journal boundaries preserve P1/P2.

---

## 165. S5 — Integrated commit

Implement:

```text
journal + state publication
durable barriers
WAL-before-page
group commit
```

Exit:

crash campaign preserves P1/P2/P3.

---

## 166. S6 — Checkpoint/recovery

Implement:

```text
fuzzy checkpoint
manifest checkpoint_lsn
redo replay
retention horizon
```

Exit:

crash at every checkpoint phase recovers reference digest.

---

## 167. S7 — Prepared transactions

Implement:

```text
PrepareBatch
CommitPrepared
AbortPrepared
IN_DOUBT
```

Exit:

no crash boundary invents a decision.

---

## 168. S8 — Protocol metadata

Implement atomic persistence of:

```text
IDC generation
plan generation
escrow state
causal metadata
txn status
```

Exit:

no crash separates business state from required protocol state.

---

## 169. S9 — Concurrency

Remove global tree lock.

Implement page-level concurrency.

Exit:

stress and differential tests pass.

---

## 170. S10 — MVCC GC

Implement safety horizons and reclamation.

Exit:

GC never alters any retained legal snapshot.

---

## 171. S11 — Tooling

Implement verifier and diagnostic commands.

---

## 172. S12 — Baseline benchmarks

Run and publish the baseline before optimization sub-specs.

---

# Part XXXI — Acceptance scenarios

## 173. Inventory / escrow atomicity

Given:

```text
Product/42.stock = 10
Escrow/42/local_rights = 10
```

and C3 operation:

```text
sell(42, 3)
```

local batch:

```text
Product/42.stock = 7
Escrow/42/local_rights = 7
```

Both commit together or neither does.

Crash after journal fsync but before page flush:

```text
recovery replays both
```

Crash before durable journal decision:

```text
neither is committed
```

---

## 174. Prepared transfer

Node X prepares:

```text
A -= 100
```

Node Y prepares:

```text
B += 100
```

If X crashes after durable prepare but before decision:

```text
A remains unchanged in committed visibility
txn = IN_DOUBT
```

Only the distributed protocol may resolve the decision.

---

## 175. Crash immediately after success

```text
CommitBatch durable
success returned
process killed
```

Restart MUST recover the transaction even if zero modified pages had been flushed.

It also recovers the original RequestKey-to-TxnId binding, exact result bytes, contract/plan identity and terminal decision. Two gateways racing the same key must recover one binding. Changed arguments, a substituted IDC definition/authority epoch, or changed result bytes are rejected. Crash after participant install but before whole-invocation publication must not create a final client success. Expiry followed by an ancient retry must preserve anti-reexecution evidence under SPEC-012.

---

## 176. Torn tail

Journal:

```text
valid A
valid B
partial C
```

Recovery:

```text
replay A/B
discard incomplete C
```

Mid-log corruption in required B:

```text
fail corruption
```

not silent truncation.

---

## 177. Old snapshot

Versions:

```text
K@10 = A
K@20 = B
K@30 = C
```

Snapshot `20` must keep reading `B` after commit 30.

GC cannot remove B while snapshot 20 is registered.

---

## 178. Duplicate replication

Same `OriginId` arrives twice.

Exactly one logical application occurs.

---

## 179. Stale plan

Active minimum plan generation = 12.

Batch references retired generation 10.

Result:

```text
StalePlan
```

with no persisted prefix mutation.

---

# Part XXXII — Research discipline

## 180. When to abandon the custom B+Tree

If implementation evidence shows the custom page engine consumes disproportionate effort and an embedded engine can expose all required control over:

```text
commit boundary
prepare/decision lifecycle
semantic commit record
protocol metadata atomicity
replication payload
recovery semantics
```

then the project MAY revisit the engine decision through a written ADR/SPEC amendment.

Owning page-split code is not a scientific contribution.

Control over the semantic transaction boundary is.

---

## 181. Definition of success

SPEC-002 succeeds when the storage engine becomes sufficiently boring and trustworthy that failures in later C0–C5 experiments can be attributed to the coordination protocol, not local persistence ambiguity.

---

## 182. Final storage invariant

```text
ACKNOWLEDGED COMMIT
    =>
DURABLE JOURNAL DECISION
    =>
ATOMICALLY RECOVERABLE STATE
```

For prepared work:

```text
NO DURABLE COMMIT DECISION
    =>
NO COMMITTED VISIBILITY
```

Everything below this line is optimization.

---

# Part XXXIII — Next SPECs

Recommended sequence:

```text
SPEC-003 — Invariant & Effect IR
           DSL, typed AST, normalization,
           dependency hypergraph, proof obligations

SPEC-004 — Coordination Compiler
           C0–C5 derivation, asymmetric dependencies,
           plan certificate, conservative fallback

SPEC-005 — C1/C2 Runtime
           commutative replication + causal scheduler

SPEC-006 — Escrow Runtime
           rights synthesis, allocation, transfer,
           rebalancing and epoch/failover safety

SPEC-007 — Certified Transactions
           read sets, range tokens,
           invariant-specific certification

SPEC-008 — Serial IDC Runtime
           ordered IDC execution, Raft/sequencer boundary,
           multi-IDC atomicity

SPEC-009 — Plan Evolution
           G→G+1 observable refinement,
           preserving already-issued commitments
           across plan/version/placement changes

SPEC-010 — Qualification
           deterministic simulation, Jepsen,
           crash/network fault campaigns and benchmarks

SPEC-011 — Catalog, Control Plane & Authority Registry
SPEC-012 — Request Identity, Client Protocol, Wire Encoding & Compatibility
SPEC-013 — Security, Authentication & Trust Model
SPEC-014 — Implementation Profile & Vertical Slice
```

`SPEC-009` is central to the narrower research contribution described in `PROPOSTA-DE-PESQUISA.md`: correctness must survive not only one fixed protocol, but transitions between valid execution plans.

---

# Implementation instructions for coding agents

A coding agent implementing SPEC-002 SHALL:

```text
DO:
- implement persistent formats first
- build the slow reference model
- add crash hooks before performance optimization
- keep semantic metadata versioned
- preserve IN_DOUBT explicitly
- test recovery as a first-class feature
- use deterministic codecs
- produce typed errors
- keep protocol state atomic with business state when required

DO NOT:
- hide RocksDB under astra-storage as the production kernel
- use SQLite as the final engine
- copy HeraclitusDB storage wholesale
- put Raft inside astra-storage
- add SQL before local recovery is proven
- add GPU/CXL/io_uring before the portable baseline
- silently weaken durability
- invent benchmark claims
```

RocksDB, SQLite and PostgreSQL are valid baselines and test references. They are not substitutes for proving AstraDB's own transaction boundary.

---

# References

1. FoundationDB — Architecture
   https://apple.github.io/foundationdb/architecture.html

2. FoundationDB — Storage configuration / Redwood and RocksDB engines
   https://apple.github.io/foundationdb/configuration.html

3. Viktor Leis et al. — **LeanStore: In-Memory Data Management Beyond Main Memory**
   https://db.in.tum.de/~leis/papers/leanstore.pdf

4. Umbra Database System
   https://umbra.db.in.tum.de/

5. Peter Bailis et al. — **Coordination Avoidance in Database Systems**
   https://www.vldb.org/pvldb/vol8/p185-bailis.pdf

6. Jonathan Arns et al. — **Event Horizon: Asymmetric Dependencies for Fast Geo-Distributed Operations**, CIDR 2026
   https://www.vldb.org/cidrdb/2026/event-horizon-asymmetric-dependencies-for-fast-geo-distributed-operations.html

---

# End of SPEC-002
