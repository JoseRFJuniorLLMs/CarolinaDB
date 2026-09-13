//! The native durable storage kernel: `Store` (SPEC-002 §41–§44, §53–§71, §75–§91, §123–§125).
//!
//! Commit path (SPEC-002 §58, simple form): validate → journal append → durable barrier →
//! install into MVCC state → ACK. Readers never observe a version above `durable_commit_seq`.
//! Prepared batches are durable in the journal but invisible until a durable decision.
//! Checkpoints persist dirty pages copy-on-write and publish the root through MANIFEST.A/B.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::Hash256;
use carolina_core::ids::*;
use carolina_core::keycodec::{KeyWriter, Namespace};
use carolina_core::limits::Limits;
use carolina_wire::records::RequestBindingV1;

use crate::batch::*;
use crate::btree::{BTree, TreeCtx, ENTRY_FLAG_TOMBSTONE};
use crate::buffer::BufferPool;
use crate::format::{JournalRecordKind, Manifest, MANIFEST_LEN};
use crate::io::{fault, write_file_synced, FaultPoint, Faults, FilePageIo, NoFaults, PageIo};
use crate::journal::{DurabilityMode, Journal, DEFAULT_SEGMENT_BYTES};

// ---------------------------------------------------------------------------
// Public kernel types (SPEC-002 §32, §39, §123–§125)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalSnapshot {
    pub epoch: StorageEpoch,
    pub visible_seq: LocalCommitSeq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionStamp {
    pub epoch: StorageEpoch,
    pub seq: LocalCommitSeq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityState {
    LocalDurable,
    Unsafe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitResult {
    pub txn_id: Option<TxnId>,
    pub local_version: VersionStamp,
    pub journal_lsn: JournalLsn,
    pub local_durability: DurabilityState,
    pub stored_evidence: Vec<ProtocolRecordRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedToken {
    pub txn_id: TxnId,
    pub lsn: JournalLsn,
    pub digest: SemanticDigest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDecision {
    pub decision_ref: ProtocolRecordRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbortDecision {
    pub decision_ref: ProtocolRecordRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRequestEvidence {
    pub binding: Option<RequestBindingV1>,
    pub status: Option<TxnStatusRecord>,
    pub in_doubt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Opening,
    RecoveringLocal,
    WaitingProtocolReconciliation,
    Ready,
    DegradedStorage,
    ReadOnlySafety,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    Quick,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerifyReport {
    pub pages: u64,
    pub leaves: u64,
    pub entries: u64,
    pub journal_frames: u64,
    pub prepared_in_doubt: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointInfo {
    pub checkpoint_lsn: JournalLsn,
    pub checkpoint_commit_seq: LocalCommitSeq,
    pub root_page_id: u64,
    pub manifest_generation: u64,
    pub pages_written: u64,
}

/// Kernel capabilities a backend declares (SPEC-002 §123). Unsupported obligations disable plans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelCapabilities {
    pub durable_prepare: bool,
    pub atomic_metadata_and_result: bool,
    pub recoverable_semantic_records: bool,
    pub registered_snapshots: bool,
    pub retained_references: bool,
    /// Whether local durability claims survive process crash on stable media.
    pub local_crash_durability: bool,
}

/// The durable boundary consumed by the semantic runtime (SPEC-002 §123).
pub trait DurableStorageKernel {
    fn capabilities(&self) -> KernelCapabilities;
    fn readiness(&self) -> Readiness;
    fn snapshot(&mut self) -> LocalSnapshot;
    fn release_snapshot(&mut self, snap: LocalSnapshot);
    fn get(&mut self, key: &LogicalKey, snap: LocalSnapshot) -> CoreResult<Option<Vec<u8>>>;
    fn scan(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        snap: LocalSnapshot,
        limit: usize,
    ) -> CoreResult<Vec<(LogicalKey, Vec<u8>)>>;
    fn commit(&mut self, batch: CompiledBatch) -> CoreResult<CommitResult>;
    fn commit_protocol(&mut self, batch: ProtocolOnlyBatch) -> CoreResult<CommitResult>;
    fn prepare(&mut self, batch: CompiledBatch) -> CoreResult<PreparedToken>;
    fn commit_prepared(
        &mut self,
        token: PreparedToken,
        decision: CommitDecision,
    ) -> CoreResult<CommitResult>;
    fn abort_prepared(&mut self, token: PreparedToken, decision: AbortDecision) -> CoreResult<()>;
    fn in_doubt(&self) -> Vec<PreparedToken>;
    fn resolve(&mut self, request: &RequestKey) -> CoreResult<LocalRequestEvidence>;
    fn read_protocol_record(
        &mut self,
        key: &ProtocolRecordKey,
    ) -> CoreResult<Option<(RecordRevision, VersionedProtocolRecord)>>;
    fn txn_status(&mut self, txn: TxnId) -> CoreResult<Option<TxnStatusRecord>>;
    fn checkpoint(&mut self) -> CoreResult<CheckpointInfo>;
    fn verify(&mut self, mode: VerifyMode) -> CoreResult<VerifyReport>;
}

// ---------------------------------------------------------------------------
// Physical keys of protocol state
// ---------------------------------------------------------------------------

pub fn txn_status_key(txn: TxnId) -> LogicalKey {
    LogicalKey(
        KeyWriter::new()
            .namespace(Namespace::TxnStatus)
            .str("txn_status")
            .bytes(&txn.0)
            .finish(),
    )
}

pub fn request_binding_record_key(key: &RequestKey) -> ProtocolRecordKey {
    let mut scope = Vec::with_capacity(32);
    scope.extend_from_slice(&key.tenant_id.0);
    scope.extend_from_slice(&key.request_namespace.0);
    ProtocolRecordKey {
        record_kind: "request_binding".into(),
        scope_key: scope,
        record_id: key.stable_request_id.0.to_vec(),
    }
}

fn encode_record_value(rev: RecordRevision, rec: &VersionedProtocolRecord) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&rev.0.to_le_bytes());
    v.extend_from_slice(&rec.encode());
    v
}

fn decode_record_value(bytes: &[u8]) -> CoreResult<(RecordRevision, VersionedProtocolRecord)> {
    if bytes.len() < 8 {
        return Err(CoreError::new(
            ErrorCode::Corruption,
            "protocol record value too short",
        ));
    }
    let rev = RecordRevision(u64::from_le_bytes(bytes[..8].try_into().unwrap()));
    let rec = VersionedProtocolRecord::decode(&bytes[8..], &Limits::v1())?;
    Ok((rev, rec))
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct StoreOptions {
    pub durability: DurabilityMode,
    pub segment_bytes: u64,
    pub pool_frames: usize,
    pub faults: Faults,
    pub storage_id: [u8; 16],
    pub cluster_id: [u8; 16],
    /// Checkpoint automatically when the pool holds this many dirty pages.
    pub auto_checkpoint_dirty_pages: usize,
    /// Reclaim MVCC versions and journal segments at checkpoint (SPEC-002 §87–§91, §105).
    /// Registered snapshots and prepared transactions always pin what they need; disable this to
    /// keep the full history for differential debugging.
    pub reclaim_at_checkpoint: bool,
}

impl Default for StoreOptions {
    fn default() -> Self {
        StoreOptions {
            durability: DurabilityMode::Sync,
            segment_bytes: DEFAULT_SEGMENT_BYTES,
            pool_frames: 4096,
            faults: std::sync::Arc::new(NoFaults),
            storage_id: [0u8; 16],
            cluster_id: [0u8; 16],
            auto_checkpoint_dirty_pages: 2048,
            reclaim_at_checkpoint: true,
        }
    }
}

#[derive(Debug, Clone)]
struct PreparedTxn {
    batch: CompiledBatch,
    lsn: u64,
}

#[derive(Debug, Default, Clone)]
pub struct Metrics {
    pub commit_total: u64,
    pub prepare_total: u64,
    pub checkpoint_total: u64,
    pub recovery_replayed_batches: u64,
    pub journal_fsync_total: u64,
    pub page_split_total: u64,
    /// MVCC versions reclaimed by checkpoints (SPEC-002 §87–§91, §130).
    pub mvcc_versions_reclaimed_total: u64,
    /// Journal segments and bytes reclaimed after a checkpoint (§105, §130).
    pub journal_segments_reclaimed_total: u64,
    pub journal_bytes_reclaimed_total: u64,
    /// Bytes of journal still retained after the last checkpoint.
    pub journal_retained_bytes: u64,
    /// Oldest sequence any registered snapshot may still read (the GC horizon).
    pub oldest_snapshot_seq: u64,
}

pub struct Store {
    dir: PathBuf,
    opts: StoreOptions,
    _lock: File,
    io: FilePageIo,
    pool: BufferPool,
    tree: BTree,
    journal: Journal,
    manifest: Manifest,
    storage_epoch: u64,
    next_seq: u64,
    durable_commit_seq: u64,
    prepared: BTreeMap<TxnId, PreparedTxn>,
    snapshots: BTreeMap<u64, u32>,
    readiness: Readiness,
    recovery_required: Arc<AtomicBool>,
    pub min_plan_generation: PlanGeneration,
    pub active_schema: Option<SchemaHash>,
    pub metrics: Metrics,
}

/// A failed durable step can leave a journal frame or a partially installed batch behind.
/// Keep the handle fenced until reopen; neither a later success nor set_readiness can unfence it.
struct DurableWriteGuard {
    recovery_required: Arc<AtomicBool>,
    completed: bool,
}

impl DurableWriteGuard {
    fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for DurableWriteGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.recovery_required.store(true, Ordering::Relaxed);
        }
    }
}

fn manifest_paths(dir: &Path) -> (PathBuf, PathBuf) {
    (dir.join("MANIFEST.A"), dir.join("MANIFEST.B"))
}

fn read_manifest(path: &Path) -> Option<Manifest> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < MANIFEST_LEN {
        return None;
    }
    Manifest::decode(&bytes).ok()
}

impl Store {
    /// Create a new database directory (fails if a manifest already exists).
    pub fn create(dir: &Path, opts: StoreOptions) -> CoreResult<Store> {
        std::fs::create_dir_all(dir)?;
        std::fs::create_dir_all(dir.join("journal"))?;
        let (a, b) = manifest_paths(dir);
        if a.exists() || b.exists() {
            return Err(CoreError::new(
                ErrorCode::InvalidManifest,
                "database already initialized",
            ));
        }
        let lock = Self::take_lock(dir)?;
        let faults = opts.faults.clone();
        let io = FilePageIo::open(
            &dir.join("data.astr"),
            crate::format::PAGE_SIZE,
            faults.clone(),
        )?;
        let mut pool = BufferPool::new(opts.pool_frames);
        // page 0 is reserved (SPEC-002 §13)
        let mut ctx = TreeCtx {
            pool: &mut pool,
            io: &io,
            durable_lsn: 0,
            current_lsn: 0,
            faults: &faults,
        };
        let mut tree = BTree::create(&mut ctx, 1)?;
        // persist the empty root so the manifest references a real page
        let root = tree.persist(&mut ctx)?;
        io.sync_data()?;
        let manifest = Manifest {
            manifest_generation: 1,
            storage_epoch: 1,
            root_page_id: root,
            next_page_id: tree.next_page_id,
            free_list_head: 0,
            checkpoint_lsn: 0,
            checkpoint_commit_seq: 0,
            journal_segment: 1,
            storage_id: opts.storage_id,
            cluster_id: opts.cluster_id,
        };
        write_file_synced(&a, &manifest.encode(), &faults)?;
        crate::io::sync_dir(dir)?;
        let (journal, _) = Journal::open(
            &dir.join("journal"),
            1,
            faults.clone(),
            opts.durability,
            opts.segment_bytes,
        )?;
        Ok(Store {
            dir: dir.to_path_buf(),
            opts,
            _lock: lock,
            io,
            pool,
            tree,
            journal,
            manifest,
            storage_epoch: 1,
            next_seq: 1,
            durable_commit_seq: 0,
            prepared: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            readiness: Readiness::Ready,
            recovery_required: Arc::new(AtomicBool::new(false)),
            min_plan_generation: PlanGeneration(0),
            active_schema: None,
            metrics: Metrics::default(),
        })
    }

    /// Open an existing database and run local recovery (SPEC-002 §84).
    pub fn open(dir: &Path, opts: StoreOptions) -> CoreResult<Store> {
        let lock = Self::take_lock(dir)?;
        let faults = opts.faults.clone();
        let (a, b) = manifest_paths(dir);
        let manifest = match (read_manifest(&a), read_manifest(&b)) {
            (Some(x), Some(y)) => {
                if x.manifest_generation >= y.manifest_generation {
                    x
                } else {
                    y
                }
            }
            (Some(x), None) | (None, Some(x)) => x,
            (None, None) => {
                return Err(CoreError::new(
                    ErrorCode::InvalidManifest,
                    "no valid manifest",
                ))
            }
        };
        let io = FilePageIo::open(
            &dir.join("data.astr"),
            crate::format::PAGE_SIZE,
            faults.clone(),
        )?;
        let mut pool = BufferPool::new(opts.pool_frames);
        let mut tree = BTree::open(manifest.root_page_id, manifest.next_page_id);
        let (journal, frames) = Journal::open(
            &dir.join("journal"),
            manifest.journal_segment,
            faults.clone(),
            opts.durability,
            opts.segment_bytes,
        )?;
        let mut store = Store {
            dir: dir.to_path_buf(),
            opts,
            _lock: lock,
            io,
            pool: BufferPool::new(8),
            tree: BTree::open(0, 0),
            journal,
            manifest,
            storage_epoch: manifest.storage_epoch,
            next_seq: manifest.checkpoint_commit_seq + 1,
            durable_commit_seq: manifest.checkpoint_commit_seq,
            prepared: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            readiness: Readiness::RecoveringLocal,
            recovery_required: Arc::new(AtomicBool::new(false)),
            min_plan_generation: PlanGeneration(0),
            active_schema: None,
            metrics: Metrics::default(),
        };
        std::mem::swap(&mut store.pool, &mut pool);
        std::mem::swap(&mut store.tree, &mut tree);
        // redo: replay frames after the checkpoint (commits are idempotent by (key, seq))
        let mut max_seq = manifest.checkpoint_commit_seq;
        for sf in frames {
            let f = sf.frame;
            if f.lsn <= manifest.checkpoint_lsn && f.kind != JournalRecordKind::PrepareBatch {
                continue;
            }
            let payload = CanonValue::decode(&f.payload, &Limits::v1())?;
            match f.kind {
                JournalRecordKind::CommitBatch => {
                    let seq = payload.field("seq")?.as_u64()?;
                    let kind = payload.field("kind")?.as_str()?.to_string();
                    if kind == "commit_batch" {
                        let batch = CompiledBatch::from_canon(payload.field("batch")?)?;
                        store.install_batch(&batch, seq, f.lsn, true, None)?;
                    } else {
                        let batch = ProtocolOnlyBatch::from_canon(payload.field("batch")?)?;
                        store.install_protocol_batch(&batch, seq, f.lsn, true)?;
                    }
                    max_seq = max_seq.max(seq);
                    store.durable_commit_seq = max_seq;
                    store.metrics.recovery_replayed_batches += 1;
                }
                JournalRecordKind::PrepareBatch => {
                    let batch = CompiledBatch::from_canon(payload.field("batch")?)?;
                    if f.lsn > manifest.checkpoint_lsn
                        || !store.tree_has_status_beyond_prepared(&batch)?
                    {
                        store
                            .prepared
                            .insert(batch.txn_id, PreparedTxn { batch, lsn: f.lsn });
                    }
                }
                JournalRecordKind::CommitPrepared => {
                    let txn = TxnId::from_canon(payload.field("txn_id")?)?;
                    let seq = payload.field("seq")?.as_u64()?;
                    let decision_ref =
                        ProtocolRecordRef::from_canon(payload.field("decision_ref")?)?;
                    if let Some(p) = store.prepared.remove(&txn) {
                        let rec = Self::decision_status(&p.batch, &decision_ref, false);
                        store.install_batch(&p.batch, seq, f.lsn, true, Some(&rec))?;
                        store.metrics.recovery_replayed_batches += 1;
                    }
                    max_seq = max_seq.max(seq);
                    store.durable_commit_seq = max_seq;
                }
                JournalRecordKind::AbortPrepared => {
                    let txn = TxnId::from_canon(payload.field("txn_id")?)?;
                    let seq = payload.field("seq")?.as_u64()?;
                    let decision_ref =
                        ProtocolRecordRef::from_canon(payload.field("decision_ref")?)?;
                    if let Some(p) = store.prepared.remove(&txn) {
                        let rec = Self::decision_status(&p.batch, &decision_ref, true);
                        store.install_status_record(txn, &rec, seq, f.lsn)?;
                    }
                    max_seq = max_seq.max(seq);
                    store.durable_commit_seq = max_seq;
                }
                JournalRecordKind::StorageEpochChange => {
                    let epoch = payload.field("epoch")?.as_u64()?;
                    if epoch < store.storage_epoch {
                        return Err(CoreError::new(
                            ErrorCode::Corruption,
                            "storage epoch change frame goes backwards",
                        ));
                    }
                    store.storage_epoch = epoch;
                }
                JournalRecordKind::CheckpointBegin
                | JournalRecordKind::CheckpointEnd
                | JournalRecordKind::CatalogGeneration => {}
            }
        }
        store.next_seq = max_seq + 1;
        store.durable_commit_seq = max_seq;
        store.readiness = if store.prepared.is_empty() {
            Readiness::Ready
        } else {
            Readiness::WaitingProtocolReconciliation
        };
        // structural verification of the recovered tree (SPEC-002 §84 step 15)
        store.verify(VerifyMode::Quick)?;
        Ok(store)
    }

    fn tree_has_status_beyond_prepared(&mut self, batch: &CompiledBatch) -> CoreResult<bool> {
        Ok(
            matches!(self.txn_status(batch.txn_id)?, Some(s) if matches!(s.phase, TxnPhase::Installed | TxnPhase::Terminal | TxnPhase::Aborted)),
        )
    }

    /// One write process per database directory (SPEC-002 §78–§79): an exclusive advisory lock on
    /// `LOCK` (`flock`/OFD on Unix, a mandatory byte-range lock on Windows) held for the store's
    /// lifetime; a second independent writer fails with `Conflict` on every supported platform.
    fn take_lock(dir: &Path) -> CoreResult<File> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("LOCK");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| CoreError::new(ErrorCode::Conflict, format!("cannot open lock: {e}")))?;
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => Err(CoreError::new(
                ErrorCode::Conflict,
                "database directory is already open for writing",
            )),
            Err(std::fs::TryLockError::Error(e)) => Err(CoreError::new(
                ErrorCode::Conflict,
                format!("cannot take lock: {e}"),
            )),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn storage_epoch(&self) -> StorageEpoch {
        StorageEpoch(self.storage_epoch)
    }

    /// Advance the storage epoch (SPEC-002 §85 `StorageEpochChange`): journaled, then published by
    /// a checkpoint so the manifest carries it. Registered snapshots of the old epoch are refused
    /// afterwards (`StaleEpoch`); an old epoch never serves as the current one (P10, local scope).
    pub fn advance_storage_epoch(&mut self) -> CoreResult<StorageEpoch> {
        let durable = self.begin_durable_write()?;
        if !self.snapshots.is_empty() {
            self.snapshots.clear();
        }
        let next = self.storage_epoch + 1;
        let payload = CanonValue::obj()
            .fu64("epoch", next)
            .fstr("kind", "storage_epoch_change")
            .build()
            .encode();
        self.journal
            .append(JournalRecordKind::StorageEpochChange, [0u8; 32], payload)?;
        self.journal.sync()?;
        self.storage_epoch = next;
        self.checkpoint()?;
        durable.complete();
        Ok(StorageEpoch(next))
    }
    pub fn durable_commit_seq(&self) -> LocalCommitSeq {
        LocalCommitSeq(self.durable_commit_seq)
    }
    pub fn durable_journal_lsn(&self) -> JournalLsn {
        JournalLsn(self.journal.durable_lsn())
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn dirty_pages(&self) -> usize {
        self.pool.dirty_count()
    }
    pub fn set_readiness(&mut self, r: Readiness) {
        self.readiness = r;
    }

    fn require_recovered(&self) -> CoreResult<()> {
        if self.recovery_required.load(Ordering::Relaxed) {
            return Err(CoreError::new(
                ErrorCode::NotReady,
                "durable operation failed; close and reopen the store to recover before further access",
            ));
        }
        Ok(())
    }

    fn begin_durable_write(&self) -> CoreResult<DurableWriteGuard> {
        self.require_recovered()?;
        Ok(DurableWriteGuard {
            recovery_required: Arc::clone(&self.recovery_required),
            completed: false,
        })
    }

    /// Newest committed version (any seq ≤ durable_commit_seq) as raw entry.
    fn current_entry(&mut self, key: &[u8]) -> CoreResult<Option<crate::btree::LeafEntry>> {
        let visible = self.durable_commit_seq;
        self.entry_at(key, visible)
    }

    fn entry_at(
        &mut self,
        key: &[u8],
        visible: u64,
    ) -> CoreResult<Option<crate::btree::LeafEntry>> {
        let tree = &self.tree;
        let mut ctx = TreeCtx {
            pool: &mut self.pool,
            io: &self.io,
            durable_lsn: self.journal.durable_lsn(),
            current_lsn: 0,
            faults: &self.opts.faults,
        };
        tree.get_version(&mut ctx, key, visible)
    }

    fn current_value(&mut self, key: &[u8]) -> CoreResult<Option<Vec<u8>>> {
        match self.current_entry(key)? {
            Some(e) if !e.is_tombstone() => {
                let tree = &self.tree;
                let mut ctx = TreeCtx {
                    pool: &mut self.pool,
                    io: &self.io,
                    durable_lsn: self.journal.durable_lsn(),
                    current_lsn: 0,
                    faults: &self.opts.faults,
                };
                Ok(Some(tree.read_value(&mut ctx, &e)?))
            }
            _ => Ok(None),
        }
    }

    // ---- validation --------------------------------------------------------

    fn validate_admission(&self, batch: &CompiledBatch) -> CoreResult<()> {
        self.require_recovered()?;
        batch.verify()?;
        // §21: key/value caps are checked before anything is journaled, so a refused batch leaves
        // no journal record that recovery could fail to replay.
        for m in &batch.mutations {
            let (key, value_len) = match m {
                StorageMutation::Put { key, value, .. } => (key, value.len()),
                StorageMutation::Delete { key, .. } => (key, 0),
            };
            if key.0.len() > crate::format::MAX_KEY_LEN {
                return Err(CoreError::new(
                    ErrorCode::KeyTooLarge,
                    format!("key of {} bytes exceeds MAX_KEY_LEN", key.0.len()),
                ));
            }
            if value_len > crate::format::MAX_VALUE_LEN {
                return Err(CoreError::new(
                    ErrorCode::ValueTooLarge,
                    format!("value of {value_len} bytes exceeds MAX_VALUE_LEN"),
                ));
            }
        }
        if self.readiness != Readiness::Ready {
            return Err(CoreError::new(
                ErrorCode::NotReady,
                format!("store readiness is {:?}", self.readiness),
            ));
        }
        if batch.plan.generation < self.min_plan_generation {
            return Err(CoreError::new(
                ErrorCode::StalePlan,
                format!(
                    "plan generation {} below active minimum {}",
                    batch.plan.generation, self.min_plan_generation
                ),
            ));
        }
        if let Some(s) = self.active_schema {
            if batch.schema_hash != s {
                return Err(CoreError::new(
                    ErrorCode::StaleSchema,
                    "batch schema hash does not match the active schema",
                ));
            }
        }
        Ok(())
    }

    /// §69: a transaction that already has a durable decision (INSTALLED/TERMINAL/ABORTED) is never
    /// installed a second time; a duplicate commit request is a typed refusal, not a new version.
    fn refuse_decided_txn(&mut self, txn: TxnId) -> CoreResult<()> {
        match self.txn_status(txn)?.map(|s| s.phase) {
            Some(TxnPhase::Aborted) => Err(CoreError::new(
                ErrorCode::TxnAlreadyAborted,
                "duplicate commit of an aborted transaction refused (SPEC-002 §69)",
            )),
            Some(TxnPhase::Installed) | Some(TxnPhase::Terminal) => Err(CoreError::new(
                ErrorCode::TxnAlreadyCommitted,
                "duplicate commit of a decided transaction refused (SPEC-002 §69)",
            )),
            _ => Ok(()),
        }
    }

    /// Validate CAS preconditions of all protocol mutations against the current committed state.
    fn validate_protocol_preconditions(
        &mut self,
        mutations: &[ProtocolMutation],
    ) -> CoreResult<()> {
        let mut seen: BTreeMap<LogicalKey, ()> = BTreeMap::new();
        for pm in mutations {
            match pm {
                ProtocolMutation::PutRecord(w) => {
                    let lk = w.key.logical_key();
                    if seen.insert(lk.clone(), ()).is_some() {
                        return Err(CoreError::new(
                            ErrorCode::Conflict,
                            "duplicate protocol record key in one batch",
                        ));
                    }
                    let current = self
                        .current_value(&lk.0)?
                        .map(|v| decode_record_value(&v))
                        .transpose()?;
                    match (&w.expected, &current) {
                        (ExpectedRecordRevision::Absent, None) => {}
                        (ExpectedRecordRevision::Absent, Some((rev, existing))) => {
                            if existing == &w.next {
                                // idempotent identical write is a no-op success
                                continue;
                            }
                            return Err(CoreError::new(
                                ErrorCode::Conflict,
                                format!(
                                    "protocol record exists at revision {}; expected absent",
                                    rev
                                ),
                            ));
                        }
                        (ExpectedRecordRevision::Exact(r), Some((rev, _))) if r == rev => {}
                        (ExpectedRecordRevision::Exact(r), Some((rev, _))) => {
                            return Err(CoreError::new(
                                ErrorCode::Conflict,
                                format!("protocol record revision {} != expected {}", rev, r),
                            ));
                        }
                        (ExpectedRecordRevision::Exact(_), None) => {
                            return Err(CoreError::new(
                                ErrorCode::Conflict,
                                "protocol record absent; expected exact revision",
                            ))
                        }
                    }
                }
                ProtocolMutation::SetTxnStatus(t) => {
                    let prev = self.txn_status(t.txn_id)?;
                    match (&t.expected_revision, &prev) {
                        (ExpectedRecordRevision::Absent, None) => {}
                        (ExpectedRecordRevision::Absent, Some(p)) if *p == t.next => continue,
                        (ExpectedRecordRevision::Absent, Some(_)) => {
                            return Err(CoreError::new(
                                ErrorCode::Conflict,
                                "txn status exists; expected absent",
                            ))
                        }
                        (ExpectedRecordRevision::Exact(r), Some(p)) if *r == p.revision => {}
                        (ExpectedRecordRevision::Exact(r), Some(p)) => {
                            return Err(CoreError::new(
                                ErrorCode::Conflict,
                                format!("txn status revision {} != expected {}", p.revision, r),
                            ))
                        }
                        (ExpectedRecordRevision::Exact(_), None) => {
                            return Err(CoreError::new(
                                ErrorCode::Conflict,
                                "txn status absent; expected exact revision",
                            ))
                        }
                    }
                    TxnStatusRecord::validate_transition(prev.as_ref(), &t.next)?;
                }
            }
        }
        Ok(())
    }

    // ---- install ----------------------------------------------------------

    fn install_batch(
        &mut self,
        batch: &CompiledBatch,
        seq: u64,
        lsn: u64,
        replay: bool,
        status_override: Option<&TxnStatusRecord>,
    ) -> CoreResult<()> {
        let txn = batch.txn_id.0;
        for m in &batch.mutations {
            let (key, flags, meta, value): (&LogicalKey, u8, &[u8], &[u8]) = match m {
                StorageMutation::Put {
                    key,
                    value,
                    semantic_meta,
                } => (
                    key,
                    semantic_meta.flags & !ENTRY_FLAG_TOMBSTONE,
                    &semantic_meta.bytes,
                    value,
                ),
                StorageMutation::Delete { key, semantic_meta } => (
                    key,
                    semantic_meta.flags | ENTRY_FLAG_TOMBSTONE,
                    &semantic_meta.bytes,
                    &[],
                ),
            };
            let tree = &mut self.tree;
            let mut ctx = TreeCtx {
                pool: &mut self.pool,
                io: &self.io,
                durable_lsn: self.journal.durable_lsn(),
                current_lsn: lsn,
                faults: &self.opts.faults,
            };
            tree.insert_version(&mut ctx, &key.0, seq, txn, flags, meta, value)?;
        }
        let mut wrote_override = false;
        for pm in &batch.protocol_mutations {
            match (pm, status_override) {
                (ProtocolMutation::SetTxnStatus(t), Some(rec)) if t.txn_id == batch.txn_id => {
                    self.install_status_record(batch.txn_id, rec, seq, lsn)?;
                    wrote_override = true;
                }
                _ => self.install_protocol_mutation(pm, seq, lsn, txn, replay)?,
            }
        }
        if let (Some(rec), false) = (status_override, wrote_override) {
            self.install_status_record(batch.txn_id, rec, seq, lsn)?;
        }
        self.metrics.page_split_total = self.tree.splits;
        Ok(())
    }

    fn install_status_record(
        &mut self,
        txn: TxnId,
        rec: &TxnStatusRecord,
        seq: u64,
        lsn: u64,
    ) -> CoreResult<()> {
        let lk = txn_status_key(txn);
        let value = rec.encode();
        let tree = &mut self.tree;
        let mut ctx = TreeCtx {
            pool: &mut self.pool,
            io: &self.io,
            durable_lsn: self.journal.durable_lsn(),
            current_lsn: lsn,
            faults: &self.opts.faults,
        };
        tree.insert_version(&mut ctx, &lk.0, seq, txn.0, FLAG_SYSTEM, &[], &value)?;
        Ok(())
    }

    fn install_protocol_mutation(
        &mut self,
        pm: &ProtocolMutation,
        seq: u64,
        lsn: u64,
        txn: [u8; 32],
        _replay: bool,
    ) -> CoreResult<()> {
        match pm {
            ProtocolMutation::PutRecord(w) => {
                let lk = w.key.logical_key();
                let rev = match &w.expected {
                    ExpectedRecordRevision::Absent => match self
                        .current_value(&lk.0)?
                        .map(|v| decode_record_value(&v))
                        .transpose()?
                    {
                        // An identical create-if-absent retry preserves the existing revision.
                        Some((cur, existing)) if existing == w.next => cur,
                        _ => RecordRevision(1),
                    },
                    // The accepted CAS fixes this revision even when the payload is unchanged.
                    // Replay must not infer it from a newer checkpoint-visible record.
                    ExpectedRecordRevision::Exact(r) => RecordRevision(r.0 + 1),
                };
                let value = encode_record_value(rev, &w.next);
                let tree = &mut self.tree;
                let mut ctx = TreeCtx {
                    pool: &mut self.pool,
                    io: &self.io,
                    durable_lsn: self.journal.durable_lsn(),
                    current_lsn: lsn,
                    faults: &self.opts.faults,
                };
                tree.insert_version(&mut ctx, &lk.0, seq, txn, FLAG_SYSTEM, &[], &value)?;
            }
            ProtocolMutation::SetTxnStatus(t) => {
                let lk = txn_status_key(t.txn_id);
                let value = t.next.encode();
                let tree = &mut self.tree;
                let mut ctx = TreeCtx {
                    pool: &mut self.pool,
                    io: &self.io,
                    durable_lsn: self.journal.durable_lsn(),
                    current_lsn: lsn,
                    faults: &self.opts.faults,
                };
                tree.insert_version(&mut ctx, &lk.0, seq, t.txn_id.0, FLAG_SYSTEM, &[], &value)?;
            }
        }
        Ok(())
    }

    /// The first durable status record of a prepared transaction is its decision (SPEC-002 §85):
    /// the PREPARED phase lives only in the journal, so the decision record replaces the batch's
    /// PREPARED transition under the same revision. Pure; identical on replay.
    pub fn decision_status(
        batch: &CompiledBatch,
        decision_ref: &ProtocolRecordRef,
        aborted: bool,
    ) -> TxnStatusRecord {
        let txn = batch.txn_id;
        let template = batch.protocol_mutations.iter().find_map(|pm| match pm {
            ProtocolMutation::SetTxnStatus(t) if t.txn_id == txn => Some(t.next.clone()),
            _ => None,
        });
        let mut rec = template.unwrap_or_else(|| TxnStatusRecord {
            revision: RecordRevision(1),
            phase: TxnPhase::Prepared,
            plan: batch.plan,
            idc_bindings: batch.idc_bindings.clone(),
            prepared_digest: Some(batch.semantic_digest),
            decision_ref: None,
            accepted_result: None,
            terminal_outcome: None,
        });
        rec.decision_ref = Some(decision_ref.clone());
        if aborted {
            rec.phase = TxnPhase::Aborted;
            rec.terminal_outcome = None;
        } else {
            match &batch.terminal_outcome {
                Some(t) => {
                    rec.phase = TxnPhase::Terminal;
                    rec.terminal_outcome = Some(t.clone());
                }
                None => {
                    rec.phase = TxnPhase::Installed;
                    rec.terminal_outcome = None;
                }
            }
        }
        rec
    }

    fn install_protocol_batch(
        &mut self,
        batch: &ProtocolOnlyBatch,
        seq: u64,
        lsn: u64,
        replay: bool,
    ) -> CoreResult<()> {
        let txn = [0u8; 32];
        for w in &batch.writes {
            self.install_protocol_mutation(
                &ProtocolMutation::PutRecord(w.clone()),
                seq,
                lsn,
                txn,
                replay,
            )?;
        }
        Ok(())
    }

    /// Checkpoint when dirty pages approach the pool capacity (SPEC-002 §113 backpressure). Dirty pages
    /// are never evicted, so the dirty working set between checkpoints must fit in the pool.
    fn maybe_auto_checkpoint(&mut self) -> CoreResult<()> {
        let threshold = self
            .opts
            .auto_checkpoint_dirty_pages
            .min(self.opts.pool_frames / 2)
            .max(1);
        if self.pool.dirty_count() >= threshold {
            self.checkpoint()?;
        }
        Ok(())
    }

    fn commit_result(
        &self,
        txn: Option<TxnId>,
        seq: u64,
        lsn: u64,
        evidence: Vec<ProtocolRecordRef>,
    ) -> CommitResult {
        CommitResult {
            txn_id: txn,
            local_version: VersionStamp {
                epoch: StorageEpoch(self.storage_epoch),
                seq: LocalCommitSeq(seq),
            },
            journal_lsn: JournalLsn(lsn),
            local_durability: if self.opts.durability == DurabilityMode::UnsafeNoFsync {
                DurabilityState::Unsafe
            } else {
                DurabilityState::LocalDurable
            },
            stored_evidence: evidence,
        }
    }

    fn evidence_refs(mutations: &[ProtocolMutation]) -> Vec<ProtocolRecordRef> {
        let mut v: Vec<ProtocolRecordRef> = mutations
            .iter()
            .filter_map(|m| match m {
                ProtocolMutation::PutRecord(w) => Some(w.next.reference(&w.key)),
                ProtocolMutation::SetTxnStatus(_) => None,
            })
            .collect();
        v.sort();
        v
    }

    // ---- manifest ---------------------------------------------------------

    fn publish_manifest(&mut self, m: Manifest) -> CoreResult<()> {
        let (a, b) = manifest_paths(&self.dir);
        // write the slot that does NOT hold the current highest generation
        let cur_a = read_manifest(&a)
            .map(|x| x.manifest_generation)
            .unwrap_or(0);
        let cur_b = read_manifest(&b)
            .map(|x| x.manifest_generation)
            .unwrap_or(0);
        let target = if cur_a > cur_b { b } else { a };
        write_file_synced(&target, &m.encode(), &self.opts.faults)?;
        self.manifest = m;
        Ok(())
    }
}

impl DurableStorageKernel for Store {
    fn capabilities(&self) -> KernelCapabilities {
        KernelCapabilities {
            durable_prepare: true,
            atomic_metadata_and_result: true,
            recoverable_semantic_records: true,
            registered_snapshots: true,
            retained_references: true,
            local_crash_durability: self.opts.durability != DurabilityMode::UnsafeNoFsync,
        }
    }

    fn readiness(&self) -> Readiness {
        if self.recovery_required.load(Ordering::Relaxed) {
            Readiness::Failed
        } else {
            self.readiness
        }
    }

    fn snapshot(&mut self) -> LocalSnapshot {
        let seq = self.durable_commit_seq;
        *self.snapshots.entry(seq).or_insert(0) += 1;
        LocalSnapshot {
            epoch: StorageEpoch(self.storage_epoch),
            visible_seq: LocalCommitSeq(seq),
        }
    }

    fn release_snapshot(&mut self, snap: LocalSnapshot) {
        if let Some(c) = self.snapshots.get_mut(&snap.visible_seq.0) {
            *c -= 1;
            if *c == 0 {
                self.snapshots.remove(&snap.visible_seq.0);
            }
        }
    }

    fn get(&mut self, key: &LogicalKey, snap: LocalSnapshot) -> CoreResult<Option<Vec<u8>>> {
        self.require_recovered()?;
        if snap.epoch.0 != self.storage_epoch {
            return Err(CoreError::new(
                ErrorCode::StaleEpoch,
                "snapshot from another storage epoch",
            ));
        }
        let visible = snap.visible_seq.0.min(self.durable_commit_seq);
        let tree = &self.tree;
        let mut ctx = TreeCtx {
            pool: &mut self.pool,
            io: &self.io,
            durable_lsn: self.journal.durable_lsn(),
            current_lsn: 0,
            faults: &self.opts.faults,
        };
        match tree.get_version(&mut ctx, &key.0, visible)? {
            Some(e) if !e.is_tombstone() => Ok(Some(tree.read_value(&mut ctx, &e)?)),
            _ => Ok(None),
        }
    }

    fn scan(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        snap: LocalSnapshot,
        limit: usize,
    ) -> CoreResult<Vec<(LogicalKey, Vec<u8>)>> {
        self.require_recovered()?;
        if snap.epoch.0 != self.storage_epoch {
            return Err(CoreError::new(
                ErrorCode::StaleEpoch,
                "snapshot from another storage epoch",
            ));
        }
        let visible = snap.visible_seq.0.min(self.durable_commit_seq);
        let tree = &self.tree;
        let mut ctx = TreeCtx {
            pool: &mut self.pool,
            io: &self.io,
            durable_lsn: self.journal.durable_lsn(),
            current_lsn: 0,
            faults: &self.opts.faults,
        };
        let entries = tree.scan_visible(
            &mut ctx,
            start,
            end,
            visible,
            limit.saturating_mul(2).max(limit),
        )?;
        let mut out = Vec::new();
        for e in entries {
            if e.is_tombstone() {
                continue;
            }
            let v = tree.read_value(&mut ctx, &e)?;
            out.push((LogicalKey(e.key), v));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    fn commit(&mut self, batch: CompiledBatch) -> CoreResult<CommitResult> {
        self.validate_admission(&batch)?;
        self.refuse_decided_txn(batch.txn_id)?;
        self.validate_protocol_preconditions(&batch.protocol_mutations)?;
        self.maybe_auto_checkpoint()?;
        if self.prepared.contains_key(&batch.txn_id) {
            return Err(CoreError::new(
                ErrorCode::TxnInDoubt,
                "transaction has a prepared batch; use commit_prepared",
            ));
        }
        let durable = self.begin_durable_write()?;
        let seq = self.next_seq;
        let payload = CanonValue::obj()
            .fc("batch", &batch)
            .fstr("kind", "commit_batch")
            .fu64("seq", seq)
            .build()
            .encode();
        let lsn = self
            .journal
            .append(JournalRecordKind::CommitBatch, batch.txn_id.0, payload)?;
        self.journal.sync()?;
        self.metrics.journal_fsync_total = self.journal.fsync_count;
        fault(&self.opts.faults, FaultPoint::AfterFsyncBeforePublish)?;
        self.install_batch(&batch, seq, lsn, false, None)?;
        self.durable_commit_seq = seq;
        self.next_seq = seq + 1;
        self.metrics.commit_total += 1;
        let evidence = Self::evidence_refs(&batch.protocol_mutations);
        let r = self.commit_result(Some(batch.txn_id), seq, lsn, evidence);
        self.maybe_auto_checkpoint()?;
        durable.complete();
        Ok(r)
    }

    fn commit_protocol(&mut self, batch: ProtocolOnlyBatch) -> CoreResult<CommitResult> {
        self.require_recovered()?;
        if self.readiness != Readiness::Ready
            && self.readiness != Readiness::WaitingProtocolReconciliation
        {
            return Err(CoreError::new(ErrorCode::NotReady, "store not ready"));
        }
        for w in &batch.writes {
            w.next.validate()?;
        }
        let pms: Vec<ProtocolMutation> = batch
            .writes
            .iter()
            .map(|w| ProtocolMutation::PutRecord(w.clone()))
            .collect();
        self.validate_protocol_preconditions(&pms)?;
        self.maybe_auto_checkpoint()?;
        let durable = self.begin_durable_write()?;
        let seq = self.next_seq;
        let payload = CanonValue::obj()
            .fc("batch", &batch)
            .fstr("kind", "commit_protocol")
            .fu64("seq", seq)
            .build()
            .encode();
        let lsn = self
            .journal
            .append(JournalRecordKind::CommitBatch, [0u8; 32], payload)?;
        self.journal.sync()?;
        fault(&self.opts.faults, FaultPoint::AfterFsyncBeforePublish)?;
        self.install_protocol_batch(&batch, seq, lsn, false)?;
        self.durable_commit_seq = seq;
        self.next_seq = seq + 1;
        self.metrics.commit_total += 1;
        let evidence: Vec<ProtocolRecordRef> = batch
            .writes
            .iter()
            .map(|w| w.next.reference(&w.key))
            .collect();
        let r = self.commit_result(None, seq, lsn, evidence);
        self.maybe_auto_checkpoint()?;
        durable.complete();
        Ok(r)
    }

    fn prepare(&mut self, batch: CompiledBatch) -> CoreResult<PreparedToken> {
        self.validate_admission(&batch)?;
        if batch.terminal_outcome.is_some() {
            return Err(CoreError::new(
                ErrorCode::InvalidEvidence,
                "terminal_outcome must be absent in a prepare",
            ));
        }
        self.validate_protocol_preconditions(&batch.protocol_mutations)?;
        if let Some(p) = self.prepared.get(&batch.txn_id) {
            if p.batch.semantic_digest == batch.semantic_digest {
                return Ok(PreparedToken {
                    txn_id: batch.txn_id,
                    lsn: JournalLsn(p.lsn),
                    digest: batch.semantic_digest,
                });
            }
            return Err(CoreError::new(
                ErrorCode::IdentityConflict,
                "a different batch is already prepared for this transaction",
            ));
        }
        let durable = self.begin_durable_write()?;
        fault(&self.opts.faults, FaultPoint::DuringPrepare)?;
        let payload = CanonValue::obj()
            .fc("batch", &batch)
            .fstr("kind", "prepare_batch")
            .build()
            .encode();
        let lsn = self
            .journal
            .append(JournalRecordKind::PrepareBatch, batch.txn_id.0, payload)?;
        self.journal.sync()?;
        let digest = batch.semantic_digest;
        let txn = batch.txn_id;
        self.prepared.insert(txn, PreparedTxn { batch, lsn });
        self.metrics.prepare_total += 1;
        fault(&self.opts.faults, FaultPoint::AfterPrepareBeforeDecision)?;
        durable.complete();
        Ok(PreparedToken {
            txn_id: txn,
            lsn: JournalLsn(lsn),
            digest,
        })
    }

    fn commit_prepared(
        &mut self,
        token: PreparedToken,
        decision: CommitDecision,
    ) -> CoreResult<CommitResult> {
        self.require_recovered()?;
        let p = match self.prepared.get(&token.txn_id) {
            Some(p) => p.clone(),
            None => {
                // duplicate decision: already installed?
                return match self.txn_status(token.txn_id)? {
                    Some(s) if matches!(s.phase, TxnPhase::Installed | TxnPhase::Terminal) => {
                        Err(CoreError::new(
                            ErrorCode::TxnAlreadyCommitted,
                            "prepared transaction already committed",
                        ))
                    }
                    Some(s) if s.phase == TxnPhase::Aborted => Err(CoreError::new(
                        ErrorCode::TxnAlreadyAborted,
                        "prepared transaction already aborted",
                    )),
                    _ => Err(CoreError::new(
                        ErrorCode::TxnInDoubt,
                        "no prepared batch for this token",
                    )),
                };
            }
        };
        if p.batch.semantic_digest != token.digest {
            return Err(CoreError::new(
                ErrorCode::IdentityMismatch,
                "prepared token digest mismatch",
            ));
        }
        self.maybe_auto_checkpoint()?;
        let durable = self.begin_durable_write()?;
        let seq = self.next_seq;
        let payload = CanonValue::obj()
            .fc("decision_ref", &decision.decision_ref)
            .fstr("kind", "commit_prepared")
            .fu64("seq", seq)
            .fc("txn_id", &token.txn_id)
            .build()
            .encode();
        let lsn =
            self.journal
                .append(JournalRecordKind::CommitPrepared, token.txn_id.0, payload)?;
        self.journal.sync()?;
        fault(&self.opts.faults, FaultPoint::AfterDecisionBeforeInstall)?;
        let rec = Self::decision_status(&p.batch, &decision.decision_ref, false);
        self.install_batch(&p.batch, seq, lsn, false, Some(&rec))?;
        self.prepared.remove(&token.txn_id);
        self.durable_commit_seq = seq;
        self.next_seq = seq + 1;
        self.metrics.commit_total += 1;
        let mut evidence = Self::evidence_refs(&p.batch.protocol_mutations);
        evidence.push(decision.decision_ref);
        evidence.sort();
        let r = self.commit_result(Some(token.txn_id), seq, lsn, evidence);
        if self.prepared.is_empty() && self.readiness == Readiness::WaitingProtocolReconciliation {
            self.readiness = Readiness::Ready;
        }
        self.maybe_auto_checkpoint()?;
        durable.complete();
        Ok(r)
    }

    fn abort_prepared(&mut self, token: PreparedToken, decision: AbortDecision) -> CoreResult<()> {
        self.require_recovered()?;
        let p = match self.prepared.get(&token.txn_id) {
            Some(p) => p.clone(),
            None => {
                return match self.txn_status(token.txn_id)? {
                    Some(s) if matches!(s.phase, TxnPhase::Installed | TxnPhase::Terminal) => {
                        Err(CoreError::new(
                            ErrorCode::TxnAlreadyCommitted,
                            "prepared transaction already committed",
                        ))
                    }
                    Some(s) if s.phase == TxnPhase::Aborted => Err(CoreError::new(
                        ErrorCode::TxnAlreadyAborted,
                        "prepared transaction already aborted",
                    )),
                    _ => Err(CoreError::new(
                        ErrorCode::TxnInDoubt,
                        "no prepared batch for this token",
                    )),
                };
            }
        };
        if p.batch.semantic_digest != token.digest {
            return Err(CoreError::new(
                ErrorCode::IdentityMismatch,
                "prepared token digest mismatch",
            ));
        }
        self.maybe_auto_checkpoint()?;
        let durable = self.begin_durable_write()?;
        let seq = self.next_seq;
        let payload = CanonValue::obj()
            .fc("decision_ref", &decision.decision_ref)
            .fstr("kind", "abort_prepared")
            .fu64("seq", seq)
            .fc("txn_id", &token.txn_id)
            .build()
            .encode();
        let lsn = self
            .journal
            .append(JournalRecordKind::AbortPrepared, token.txn_id.0, payload)?;
        self.journal.sync()?;
        fault(&self.opts.faults, FaultPoint::AfterDecisionBeforeInstall)?;
        let rec = Self::decision_status(&p.batch, &decision.decision_ref, true);
        self.install_status_record(token.txn_id, &rec, seq, lsn)?;
        self.prepared.remove(&token.txn_id);
        self.durable_commit_seq = seq;
        self.next_seq = seq + 1;
        if self.prepared.is_empty() && self.readiness == Readiness::WaitingProtocolReconciliation {
            self.readiness = Readiness::Ready;
        }
        durable.complete();
        Ok(())
    }

    fn in_doubt(&self) -> Vec<PreparedToken> {
        self.prepared
            .iter()
            .map(|(t, p)| PreparedToken {
                txn_id: *t,
                lsn: JournalLsn(p.lsn),
                digest: p.batch.semantic_digest,
            })
            .collect()
    }

    fn resolve(&mut self, request: &RequestKey) -> CoreResult<LocalRequestEvidence> {
        let key = request_binding_record_key(request);
        let binding = match self.read_protocol_record(&key)? {
            Some((_, rec)) => Some(RequestBindingV1::decode(
                &rec.canonical_payload,
                &Limits::v1(),
            )?),
            None => None,
        };
        let (status, in_doubt) = match &binding {
            Some(b) => (
                self.txn_status(b.txn_id)?,
                self.prepared.contains_key(&b.txn_id),
            ),
            None => (None, false),
        };
        Ok(LocalRequestEvidence {
            binding,
            status,
            in_doubt,
        })
    }

    fn read_protocol_record(
        &mut self,
        key: &ProtocolRecordKey,
    ) -> CoreResult<Option<(RecordRevision, VersionedProtocolRecord)>> {
        self.require_recovered()?;
        let lk = key.logical_key();
        match self.current_value(&lk.0)? {
            Some(v) => Ok(Some(decode_record_value(&v)?)),
            None => Ok(None),
        }
    }

    fn txn_status(&mut self, txn: TxnId) -> CoreResult<Option<TxnStatusRecord>> {
        self.require_recovered()?;
        let lk = txn_status_key(txn);
        match self.current_value(&lk.0)? {
            Some(v) => Ok(Some(TxnStatusRecord::decode(&v, &Limits::v1())?)),
            None => Ok(None),
        }
    }

    /// Fuzzy checkpoint (SPEC-002 §81): CheckpointBegin → persist dirty pages (CoW) → publish manifest → CheckpointEnd.
    fn checkpoint(&mut self) -> CoreResult<CheckpointInfo> {
        let durable = self.begin_durable_write()?;
        let target_lsn = self.journal.durable_lsn();
        let target_seq = self.durable_commit_seq;
        let begin_payload = CanonValue::obj()
            .fstr("kind", "checkpoint_begin")
            .fu64("target_lsn", target_lsn)
            .fu64("target_seq", target_seq)
            .build()
            .encode();
        let begin_lsn =
            self.journal
                .append(JournalRecordKind::CheckpointBegin, [0u8; 32], begin_payload)?;
        self.journal.sync()?;
        fault(&self.opts.faults, FaultPoint::DuringCheckpoint)?;
        let dirty_before = self.pool.dirty_count() as u64;
        // GC horizon (SPEC-002 §87–§90): the oldest sequence a registered snapshot may still read,
        // never above the current read point. With no registered snapshot the horizon is the
        // durable read point itself, so only superseded versions are reclaimable.
        let oldest_snapshot = self
            .snapshots
            .keys()
            .copied()
            .min()
            .unwrap_or(self.durable_commit_seq)
            .min(self.durable_commit_seq);
        self.metrics.oldest_snapshot_seq = oldest_snapshot;
        let gc_horizon = if self.opts.reclaim_at_checkpoint {
            Some(oldest_snapshot)
        } else {
            None
        };
        let (root, reclaimed) = {
            let tree = &mut self.tree;
            let mut ctx = TreeCtx {
                pool: &mut self.pool,
                io: &self.io,
                durable_lsn: self.journal.durable_lsn(),
                current_lsn: begin_lsn,
                faults: &self.opts.faults,
            };
            tree.persist_with_gc(&mut ctx, gc_horizon)?
        };
        self.metrics.mvcc_versions_reclaimed_total += reclaimed;
        self.io.sync_data()?;
        self.pool.shrink_to_capacity();
        // prepared transactions pin the journal: recovery must see their PrepareBatch frames
        let oldest_prepared = self.prepared.values().map(|p| p.lsn).min();
        let checkpoint_lsn = match oldest_prepared {
            Some(l) => target_lsn.min(l.saturating_sub(1)),
            None => target_lsn,
        };
        let m = Manifest {
            manifest_generation: self.manifest.manifest_generation + 1,
            storage_epoch: self.storage_epoch,
            root_page_id: root,
            next_page_id: self.tree.next_page_id,
            free_list_head: 0,
            checkpoint_lsn,
            checkpoint_commit_seq: target_seq,
            journal_segment: self
                .journal
                .current_segment()
                .min(self.manifest.journal_segment.max(1)),
            storage_id: self.opts.storage_id,
            cluster_id: self.opts.cluster_id,
        };
        // Retention (SPEC-002 §105): keep the segment that holds `checkpoint_lsn` and everything
        // after it. `checkpoint_lsn` is already clamped below the oldest prepared frame, so
        // prepared transactions pin the journal. The manifest is published *before* the old files
        // are deleted, so a crash in between only leaves files recovery already skips.
        // Safety belt: only advance the retention point when the persisted image is complete,
        // i.e. the pool holds no dirty page the checkpoint failed to write.
        let image_complete = self.pool.dirty_count() == 0;
        let first_needed = if self.opts.reclaim_at_checkpoint && image_complete {
            self.journal.first_needed_segment(checkpoint_lsn)?
        } else {
            self.manifest.journal_segment.max(1)
        };
        let m = Manifest {
            journal_segment: first_needed,
            ..m
        };
        self.publish_manifest(m)?;
        if self.opts.reclaim_at_checkpoint && image_complete {
            let (files, bytes) = self.journal.reclaim_segments_below(first_needed)?;
            self.metrics.journal_segments_reclaimed_total += files;
            self.metrics.journal_bytes_reclaimed_total += bytes;
        }
        self.metrics.journal_retained_bytes = self.journal.retained_bytes()?;
        let end_payload = CanonValue::obj()
            .fu64("checkpoint_lsn", checkpoint_lsn)
            .fstr("kind", "checkpoint_end")
            .build()
            .encode();
        self.journal
            .append(JournalRecordKind::CheckpointEnd, [0u8; 32], end_payload)?;
        self.journal.sync()?;
        self.metrics.checkpoint_total += 1;
        durable.complete();
        Ok(CheckpointInfo {
            checkpoint_lsn: JournalLsn(checkpoint_lsn),
            checkpoint_commit_seq: LocalCommitSeq(target_seq),
            root_page_id: root,
            manifest_generation: self.manifest.manifest_generation,
            pages_written: dirty_before,
        })
    }

    fn verify(&mut self, mode: VerifyMode) -> CoreResult<VerifyReport> {
        self.require_recovered()?;
        let tree = &self.tree;
        let mut ctx = TreeCtx {
            pool: &mut self.pool,
            io: &self.io,
            durable_lsn: self.journal.durable_lsn(),
            current_lsn: 0,
            faults: &self.opts.faults,
        };
        let stats = tree.verify(&mut ctx)?;
        let mut report = VerifyReport {
            pages: stats.pages,
            leaves: stats.leaves,
            entries: stats.entries,
            journal_frames: 0,
            prepared_in_doubt: self.prepared.len() as u64,
        };
        if mode == VerifyMode::Full {
            let (_, frames) = Journal::open(
                &self.dir.join("journal"),
                1,
                std::sync::Arc::new(NoFaults),
                DurabilityMode::Sync,
                self.opts.segment_bytes,
            )?;
            report.journal_frames = frames.len() as u64;
        }
        Ok(report)
    }
}

/// Convenience for tests: digest of all visible user-visible key/values at the current durable seq.
pub fn state_digest(store: &mut Store) -> CoreResult<Hash256> {
    let snap = store.snapshot();
    let rows = store.scan(&[], None, snap, usize::MAX)?;
    store.release_snapshot(snap);
    let mut bytes = Vec::new();
    for (k, v) in rows {
        if !k.namespace()?.is_user_writable() {
            continue;
        }
        bytes.extend_from_slice(&(k.0.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&k.0);
        bytes.extend_from_slice(&(v.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&v);
    }
    Ok(carolina_core::hash::sha256(&bytes))
}
