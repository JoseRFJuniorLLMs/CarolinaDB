//! Page-oriented B+Tree with physical MVCC ordering (SPEC-002 §6, §17–§22, §33–§35, §72–§74).
//!
//! Leaf entries are ordered by `(logical_key ASC, local_commit_seq DESC)`. Internal pages hold
//! `(separator_key, child_page_id)`; entry 0 has an empty separator (leftmost child).
//!
//! Persistence is **copy-on-write at checkpoint**: a dirty page is never rewritten in place.
//! At checkpoint every dirty page receives a fresh page id, parents are updated bottom-up and the
//! new root is published through the A/B manifest. A torn page write can therefore only damage a
//! page that no valid manifest references. Sibling pointers are not used for navigation (they
//! would go stale under CoW); scans re-seek by key.
//!
//! Values larger than [`MAX_INLINE_VALUE`] live in overflow page chains.

use carolina_core::error::{CoreError, CoreResult, ErrorCode};

use crate::buffer::BufferPool;
use crate::format::{PageHeader, PageType, MAX_KEY_LEN, MAX_VALUE_LEN, PAGE_HEADER_LEN, PAGE_SIZE};
use crate::io::{fault, FaultPoint, Faults, PageIo};

pub const MAX_INLINE_VALUE: usize = 1024;
const OVERFLOW_DATA_PER_PAGE: usize = PAGE_SIZE - PAGE_HEADER_LEN - 8; // next pointer u64 at end

pub const ENTRY_FLAG_TOMBSTONE: u8 = 0x01;
pub const ENTRY_FLAG_OVERFLOW: u8 = 0x80;

/// Decoded leaf entry (owned).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafEntry {
    pub key: Vec<u8>,
    pub seq: u64,
    pub txn_id: [u8; 32],
    pub flags: u8,
    pub meta: Vec<u8>,
    /// Inline value bytes, or `[first_overflow_page u64 LE][total_len u32 LE]` when `ENTRY_FLAG_OVERFLOW`.
    pub value: Vec<u8>,
}

impl LeafEntry {
    pub fn is_tombstone(&self) -> bool {
        self.flags & ENTRY_FLAG_TOMBSTONE != 0
    }
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            2 + 8 + 32 + 1 + 2 + 4 + self.key.len() + self.meta.len() + self.value.len(),
        );
        out.extend_from_slice(&(self.key.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.txn_id);
        out.push(self.flags);
        out.extend_from_slice(&(self.meta.len() as u16).to_le_bytes());
        out.extend_from_slice(&(self.value.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.key);
        out.extend_from_slice(&self.meta);
        out.extend_from_slice(&self.value);
        out
    }
    fn decode(b: &[u8]) -> CoreResult<LeafEntry> {
        if b.len() < 49 {
            return Err(corrupt("leaf entry too short"));
        }
        let key_len = u16::from_le_bytes([b[0], b[1]]) as usize;
        let seq = u64::from_le_bytes(b[2..10].try_into().unwrap());
        let mut txn_id = [0u8; 32];
        txn_id.copy_from_slice(&b[10..42]);
        let flags = b[42];
        let meta_len = u16::from_le_bytes([b[43], b[44]]) as usize;
        let value_len = u32::from_le_bytes(b[45..49].try_into().unwrap()) as usize;
        let end = 49 + key_len + meta_len + value_len;
        if end > b.len() || key_len > MAX_KEY_LEN {
            return Err(corrupt("leaf entry bounds"));
        }
        Ok(LeafEntry {
            key: b[49..49 + key_len].to_vec(),
            seq,
            txn_id,
            flags,
            meta: b[49 + key_len..49 + key_len + meta_len].to_vec(),
            value: b[49 + key_len + meta_len..end].to_vec(),
        })
    }
}

/// Internal separator: the full physical position `(key, seq)`; entries at or after it (in leaf
/// order) live in `child`. Entry 0 is `(empty key, u64::MAX)` = before everything.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InternalEntry {
    key: Vec<u8>,
    seq: u64,
    child: u64,
}

impl InternalEntry {
    fn leftmost(child: u64) -> InternalEntry {
        InternalEntry {
            key: vec![],
            seq: u64::MAX,
            child,
        }
    }
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(18 + self.key.len());
        out.extend_from_slice(&(self.key.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.child.to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.key);
        out
    }
    fn decode(b: &[u8]) -> CoreResult<InternalEntry> {
        if b.len() < 18 {
            return Err(corrupt("internal entry too short"));
        }
        let key_len = u16::from_le_bytes([b[0], b[1]]) as usize;
        if 18 + key_len > b.len() {
            return Err(corrupt("internal entry bounds"));
        }
        Ok(InternalEntry {
            child: u64::from_le_bytes(b[2..10].try_into().unwrap()),
            seq: u64::from_le_bytes(b[10..18].try_into().unwrap()),
            key: b[18..18 + key_len].to_vec(),
        })
    }
}

fn corrupt(msg: &str) -> CoreError {
    CoreError::new(ErrorCode::Corruption, msg)
}

/// Physical order of leaf entries: key ASC, seq DESC.
fn leaf_cmp(a_key: &[u8], a_seq: u64, b_key: &[u8], b_seq: u64) -> std::cmp::Ordering {
    a_key.cmp(b_key).then(b_seq.cmp(&a_seq))
}

// ---------------------------------------------------------------------------
// Slotted page helpers (records grow down from free_end; slot array grows up from free_start)
// ---------------------------------------------------------------------------

fn slot_count(page: &[u8]) -> usize {
    u16::from_le_bytes([page[32], page[33]]) as usize
}
fn free_start(page: &[u8]) -> usize {
    u16::from_le_bytes([page[34], page[35]]) as usize
}
fn free_end(page: &[u8]) -> usize {
    u16::from_le_bytes([page[36], page[37]]) as usize
}
fn slot(page: &[u8], i: usize) -> (usize, usize) {
    let off = PAGE_HEADER_LEN + i * 4;
    let start = u16::from_le_bytes([page[off], page[off + 1]]) as usize;
    let len = u16::from_le_bytes([page[off + 2], page[off + 3]]) as usize;
    (start, len)
}
fn record(page: &[u8], i: usize) -> CoreResult<&[u8]> {
    let (start, len) = slot(page, i);
    if start < PAGE_HEADER_LEN || start + len > PAGE_SIZE {
        return Err(corrupt("slot out of bounds"));
    }
    Ok(&page[start..start + len])
}

/// Rewrite a page from a list of encoded records (compacting). Returns false if they do not fit.
fn write_records(page: &mut [u8], header: &mut PageHeader, records: &[Vec<u8>]) -> bool {
    let total: usize = records.iter().map(|r| r.len()).sum::<usize>() + records.len() * 4;
    if PAGE_HEADER_LEN + total > PAGE_SIZE {
        return false;
    }
    let mut end = PAGE_SIZE;
    for (i, r) in records.iter().enumerate() {
        end -= r.len();
        page[end..end + r.len()].copy_from_slice(r);
        let off = PAGE_HEADER_LEN + i * 4;
        page[off..off + 2].copy_from_slice(&(end as u16).to_le_bytes());
        page[off + 2..off + 4].copy_from_slice(&(r.len() as u16).to_le_bytes());
    }
    header.item_count = records.len() as u16;
    header.free_start = (PAGE_HEADER_LEN + records.len() * 4) as u16;
    header.free_end = end as u16;
    header.write(page);
    true
}

fn read_records(page: &[u8]) -> CoreResult<Vec<Vec<u8>>> {
    let n = slot_count(page);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(record(page, i)?.to_vec());
    }
    Ok(out)
}

fn page_type(page: &[u8]) -> CoreResult<PageType> {
    PageType::from_u8(page[6])
}

fn free_space(page: &[u8]) -> usize {
    free_end(page).saturating_sub(free_start(page))
}

// ---------------------------------------------------------------------------
// Tree
// ---------------------------------------------------------------------------

/// The tree operates on a pool + io; the root id and allocator live here.
pub struct BTree {
    pub root: u64,
    pub next_page_id: u64,
    pub splits: u64,
}

pub struct TreeCtx<'a> {
    pub pool: &'a mut BufferPool,
    pub io: &'a dyn PageIo,
    pub durable_lsn: u64,
    pub current_lsn: u64,
    pub faults: &'a Faults,
}

impl BTree {
    /// Create an empty tree: root is a fresh empty leaf.
    pub fn create(ctx: &mut TreeCtx, next_page_id: u64) -> CoreResult<BTree> {
        let lsn = ctx.current_lsn;
        let mut t = BTree {
            root: 0,
            next_page_id,
            splits: 0,
        };
        let root = t.alloc();
        ctx.pool
            .insert_new(ctx.io, root, PageType::Leaf, lsn, ctx.durable_lsn)?;
        t.root = root;
        Ok(t)
    }

    pub fn open(root: u64, next_page_id: u64) -> BTree {
        BTree {
            root,
            next_page_id,
            splits: 0,
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_page_id;
        self.next_page_id += 1;
        id
    }

    fn fetch<'p>(ctx: &'p mut TreeCtx, id: u64) -> CoreResult<(usize, &'p mut BufferPool)> {
        let i = ctx.pool.fetch(ctx.io, id, ctx.durable_lsn)?;
        Ok((i, ctx.pool))
    }

    /// Find the leaf for `(key, seq)` following child pointers; returns the path of (page id, entry index).
    fn descend(&self, ctx: &mut TreeCtx, key: &[u8], seq: u64) -> CoreResult<Vec<(u64, usize)>> {
        let mut path = Vec::new();
        let mut pid = self.root;
        for _ in 0..64 {
            let (fi, pool) = Self::fetch(ctx, pid)?;
            let page = &pool.frame(fi).bytes[..];
            match page_type(page)? {
                PageType::Leaf => {
                    path.push((pid, 0));
                    return Ok(path);
                }
                PageType::Internal => {
                    let n = slot_count(page);
                    if n == 0 {
                        return Err(corrupt("empty internal page"));
                    }
                    // last entry i with sep(i) <= (key,seq) where separator compares by key then seq desc
                    let mut idx = 0;
                    for i in 1..n {
                        let e = InternalEntry::decode(record(page, i)?)?;
                        // entries at/after the separator position (key, seq) in leaf order go right
                        if leaf_cmp(&e.key, e.seq, key, seq) != std::cmp::Ordering::Greater {
                            idx = i;
                        } else {
                            break;
                        }
                    }
                    let e = InternalEntry::decode(record(page, idx)?)?;
                    path.push((pid, idx));
                    pid = e.child;
                }
                other => {
                    return Err(corrupt(&format!(
                        "unexpected page type {other:?} in tree path"
                    )))
                }
            }
        }
        Err(corrupt("tree too deep"))
    }

    /// Read the newest version of `key` visible at `visible_seq`. Returns the entry (tombstones included).
    pub fn get_version(
        &self,
        ctx: &mut TreeCtx,
        key: &[u8],
        visible_seq: u64,
    ) -> CoreResult<Option<LeafEntry>> {
        let mut path = self.descend(ctx, key, u64::MAX)?;
        // entries for key are contiguous in leaf order, but may continue in following leaves after splits
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 1_000_000 {
                return Err(corrupt("scan loop"));
            }
            let (leaf, _) = *path.last().unwrap();
            let (fi, pool) = Self::fetch(ctx, leaf)?;
            let page = &pool.frame(fi).bytes[..];
            let n = slot_count(page);
            let mut last_key_in_page: Option<Vec<u8>> = None;
            for i in 0..n {
                let e = LeafEntry::decode(record(page, i)?)?;
                last_key_in_page = Some(e.key.clone());
                match e.key.as_slice().cmp(key) {
                    std::cmp::Ordering::Less => continue,
                    std::cmp::Ordering::Greater => return Ok(None),
                    std::cmp::Ordering::Equal => {
                        if e.seq <= visible_seq {
                            return Ok(Some(e));
                        }
                    }
                }
            }
            // the key may continue on the right neighbor only if nothing greater was seen in this leaf
            if let Some(k) = &last_key_in_page {
                if k.as_slice() > key {
                    return Ok(None);
                }
            }
            match self.right_neighbor_path(ctx, &path)? {
                Some(next) => path = next,
                None => return Ok(None),
            }
        }
    }

    /// The path to the leaf immediately to the right of the leaf at the end of `path`, found by
    /// walking up to the first ancestor with a following entry and descending leftmost from it.
    fn right_neighbor_path(
        &self,
        ctx: &mut TreeCtx,
        path: &[(u64, usize)],
    ) -> CoreResult<Option<Vec<(u64, usize)>>> {
        for depth in (0..path.len().saturating_sub(1)).rev() {
            let (ppid, idx) = path[depth];
            let (fi, pool) = Self::fetch(ctx, ppid)?;
            let page = &pool.frame(fi).bytes[..];
            let n = slot_count(page);
            if idx + 1 < n {
                let e = InternalEntry::decode(record(page, idx + 1)?)?;
                let mut new_path: Vec<(u64, usize)> = path[..depth].to_vec();
                new_path.push((ppid, idx + 1));
                let mut pid = e.child;
                for _ in 0..64 {
                    let (fi, pool) = Self::fetch(ctx, pid)?;
                    let page = &pool.frame(fi).bytes[..];
                    match page_type(page)? {
                        PageType::Leaf => {
                            new_path.push((pid, 0));
                            return Ok(Some(new_path));
                        }
                        PageType::Internal => {
                            let e0 = InternalEntry::decode(record(page, 0)?)?;
                            new_path.push((pid, 0));
                            pid = e0.child;
                        }
                        _ => return Err(corrupt("bad page in leftmost descent")),
                    }
                }
                return Err(corrupt("tree too deep"));
            }
        }
        Ok(None)
    }

    /// Insert a physical version. Returns `false` if the identical `(key, seq)` already exists (idempotent redo).
    #[allow(clippy::too_many_arguments)]
    pub fn insert_version(
        &mut self,
        ctx: &mut TreeCtx,
        key: &[u8],
        seq: u64,
        txn_id: [u8; 32],
        flags: u8,
        meta: &[u8],
        value: &[u8],
    ) -> CoreResult<bool> {
        let lsn = ctx.current_lsn;
        if key.len() > MAX_KEY_LEN {
            return Err(CoreError::new(
                ErrorCode::KeyTooLarge,
                "key exceeds MAX_KEY_LEN",
            ));
        }
        if value.len() > MAX_VALUE_LEN {
            return Err(CoreError::new(
                ErrorCode::ValueTooLarge,
                "value exceeds MAX_VALUE_LEN",
            ));
        }
        let (flags, stored_value) = if value.len() > MAX_INLINE_VALUE {
            let first = self.write_overflow(ctx, value)?;
            let mut v = Vec::with_capacity(12);
            v.extend_from_slice(&first.to_le_bytes());
            v.extend_from_slice(&(value.len() as u32).to_le_bytes());
            (flags | ENTRY_FLAG_OVERFLOW, v)
        } else {
            (flags, value.to_vec())
        };
        let entry = LeafEntry {
            key: key.to_vec(),
            seq,
            txn_id,
            flags,
            meta: meta.to_vec(),
            value: stored_value,
        };
        let enc = entry.encode();
        let path = self.descend(ctx, key, seq)?;
        let (leaf, _) = *path.last().unwrap();
        // locate insert position and check duplicates
        let (fi, pool) = Self::fetch(ctx, leaf)?;
        let page = &pool.frame(fi).bytes[..];
        let n = slot_count(page);
        let mut pos = n;
        for i in 0..n {
            let e = LeafEntry::decode(record(page, i)?)?;
            match leaf_cmp(&e.key, e.seq, key, seq) {
                std::cmp::Ordering::Equal => return Ok(false),
                std::cmp::Ordering::Greater => {
                    pos = i;
                    break;
                }
                std::cmp::Ordering::Less => {}
            }
        }
        if free_space(page) >= enc.len() + 4 {
            // fast path: append record at free_end, shift slot array
            let frame = pool.frame_mut(fi);
            let pg = &mut frame.bytes[..];
            let mut header = PageHeader::read(pg, leaf)?;
            let end = free_end(pg) - enc.len();
            pg[end..end + enc.len()].copy_from_slice(&enc);
            // shift slots [pos..n) right by one
            let slots_start = PAGE_HEADER_LEN + pos * 4;
            let slots_end = PAGE_HEADER_LEN + n * 4;
            pg.copy_within(slots_start..slots_end, slots_start + 4);
            pg[slots_start..slots_start + 2].copy_from_slice(&(end as u16).to_le_bytes());
            pg[slots_start + 2..slots_start + 4].copy_from_slice(&(enc.len() as u16).to_le_bytes());
            header.item_count = (n + 1) as u16;
            header.free_start = (PAGE_HEADER_LEN + (n + 1) * 4) as u16;
            header.free_end = end as u16;
            header.page_lsn = lsn;
            header.write(pg);
            pool.mark_dirty(fi, lsn);
            return Ok(true);
        }
        // slow path: rebuild with compaction, split if needed
        let mut records = read_records(page)?;
        records.insert(pos, enc);
        self.rewrite_or_split(ctx, &path, records)?;
        Ok(true)
    }

    fn rewrite_or_split(
        &mut self,
        ctx: &mut TreeCtx,
        path: &[(u64, usize)],
        records: Vec<Vec<u8>>,
    ) -> CoreResult<()> {
        let lsn = ctx.current_lsn;
        let (leaf, _) = *path.last().unwrap();
        let (fi, pool) = Self::fetch(ctx, leaf)?;
        {
            let frame = pool.frame_mut(fi);
            let pg = &mut frame.bytes[..];
            let mut header = PageHeader::read(pg, leaf)?;
            header.page_lsn = lsn;
            if write_records(pg, &mut header, &records) {
                pool.mark_dirty(fi, lsn);
                return Ok(());
            }
        }
        // split: left keeps first half, right gets the rest (by byte size balance)
        fault(ctx.faults, FaultPoint::DuringPageSplit)?;
        self.splits += 1;
        let total: usize = records.iter().map(|r| r.len() + 4).sum();
        let mut acc = 0;
        let mut split = records.len() / 2;
        for (i, r) in records.iter().enumerate() {
            acc += r.len() + 4;
            if acc >= total / 2 {
                split = (i + 1).min(records.len() - 1).max(1);
                break;
            }
        }
        let (left, right) = records.split_at(split);
        let right_id = self.alloc();
        // separator = physical position of the first right entry
        let sep_entry = LeafEntry::decode(&right[0])?;
        let sep = (sep_entry.key.clone(), sep_entry.seq);
        {
            let (fi, pool) = Self::fetch(ctx, leaf)?;
            let frame = pool.frame_mut(fi);
            let pg = &mut frame.bytes[..];
            let mut header = PageHeader::read(pg, leaf)?;
            header.page_lsn = lsn;
            if !write_records(pg, &mut header, left) {
                return Err(corrupt("left half does not fit after split"));
            }
            pool.mark_dirty(fi, lsn);
        }
        {
            let ri = ctx
                .pool
                .insert_new(ctx.io, right_id, PageType::Leaf, lsn, ctx.durable_lsn)?;
            let frame = ctx.pool.frame_mut(ri);
            let pg = &mut frame.bytes[..];
            let mut header = PageHeader::read(pg, right_id)?;
            header.page_lsn = lsn;
            if !write_records(pg, &mut header, right) {
                return Err(CoreError::new(
                    ErrorCode::ValueTooLarge,
                    "entry does not fit in an empty page",
                ));
            }
            ctx.pool.mark_dirty(ri, lsn);
        }
        self.insert_separator(ctx, &path[..path.len() - 1], sep, right_id)
    }

    /// Insert `(sep -> right_child)` into the parent at the end of `parent_path`; split upward as needed.
    fn insert_separator(
        &mut self,
        ctx: &mut TreeCtx,
        parent_path: &[(u64, usize)],
        sep: (Vec<u8>, u64),
        right_child: u64,
    ) -> CoreResult<()> {
        let lsn = ctx.current_lsn;
        if parent_path.is_empty() {
            // root split: new root with [ (empty -> old_root), (sep -> right) ]
            let old_root = self.root;
            let new_root = self.alloc();
            let ri =
                ctx.pool
                    .insert_new(ctx.io, new_root, PageType::Internal, lsn, ctx.durable_lsn)?;
            let frame = ctx.pool.frame_mut(ri);
            let pg = &mut frame.bytes[..];
            let mut header = PageHeader::read(pg, new_root)?;
            header.page_lsn = lsn;
            let recs = vec![
                InternalEntry::leftmost(old_root).encode(),
                InternalEntry {
                    key: sep.0,
                    seq: sep.1,
                    child: right_child,
                }
                .encode(),
            ];
            write_records(pg, &mut header, &recs);
            ctx.pool.mark_dirty(ri, lsn);
            self.root = new_root;
            return Ok(());
        }
        let (ppid, idx) = *parent_path.last().unwrap();
        let (fi, pool) = Self::fetch(ctx, ppid)?;
        let page = &pool.frame(fi).bytes[..];
        let mut records = read_records(page)?;
        records.insert(
            idx + 1,
            InternalEntry {
                key: sep.0,
                seq: sep.1,
                child: right_child,
            }
            .encode(),
        );
        {
            let frame = pool.frame_mut(fi);
            let pg = &mut frame.bytes[..];
            let mut header = PageHeader::read(pg, ppid)?;
            header.page_lsn = lsn;
            if write_records(pg, &mut header, &records) {
                pool.mark_dirty(fi, lsn);
                return Ok(());
            }
        }
        // split internal page
        fault(ctx.faults, FaultPoint::DuringPageSplit)?;
        self.splits += 1;
        let split = records.len() / 2;
        let left: Vec<Vec<u8>> = records[..split].to_vec();
        let mid = InternalEntry::decode(&records[split])?;
        // right page's first entry becomes the leftmost (empty key) pointing to mid.child
        let mut right: Vec<Vec<u8>> = vec![InternalEntry::leftmost(mid.child).encode()];
        right.extend(records[split + 1..].iter().cloned());
        let right_id = self.alloc();
        {
            let (fi, pool) = Self::fetch(ctx, ppid)?;
            let frame = pool.frame_mut(fi);
            let pg = &mut frame.bytes[..];
            let mut header = PageHeader::read(pg, ppid)?;
            header.page_lsn = lsn;
            if !write_records(pg, &mut header, &left) {
                return Err(corrupt("internal left half does not fit"));
            }
            pool.mark_dirty(fi, lsn);
        }
        {
            let ri =
                ctx.pool
                    .insert_new(ctx.io, right_id, PageType::Internal, lsn, ctx.durable_lsn)?;
            let frame = ctx.pool.frame_mut(ri);
            let pg = &mut frame.bytes[..];
            let mut header = PageHeader::read(pg, right_id)?;
            header.page_lsn = lsn;
            if !write_records(pg, &mut header, &right) {
                return Err(corrupt("internal right half does not fit"));
            }
            ctx.pool.mark_dirty(ri, lsn);
        }
        self.insert_separator(
            ctx,
            &parent_path[..parent_path.len() - 1],
            (mid.key, mid.seq),
            right_id,
        )
    }

    fn write_overflow(&mut self, ctx: &mut TreeCtx, value: &[u8]) -> CoreResult<u64> {
        let lsn = ctx.current_lsn;
        let mut chunks: Vec<&[u8]> = value.chunks(OVERFLOW_DATA_PER_PAGE).collect();
        chunks.reverse();
        let mut next: u64 = 0;
        for chunk in chunks {
            let id = self.alloc();
            let fi = ctx
                .pool
                .insert_new(ctx.io, id, PageType::Overflow, lsn, ctx.durable_lsn)?;
            let frame = ctx.pool.frame_mut(fi);
            let pg = &mut frame.bytes[..];
            let mut header = PageHeader::read(pg, id)?;
            pg[PAGE_HEADER_LEN..PAGE_HEADER_LEN + chunk.len()].copy_from_slice(chunk);
            pg[PAGE_SIZE - 8..].copy_from_slice(&next.to_le_bytes());
            header.item_count = chunk.len() as u16;
            header.free_start = (PAGE_HEADER_LEN + chunk.len()) as u16;
            header.free_end = (PAGE_SIZE - 8) as u16;
            header.page_lsn = lsn;
            header.write(pg);
            ctx.pool.mark_dirty(fi, lsn);
            next = id;
        }
        Ok(next)
    }

    /// Resolve an entry's value (following overflow chains).
    pub fn read_value(&self, ctx: &mut TreeCtx, e: &LeafEntry) -> CoreResult<Vec<u8>> {
        if e.flags & ENTRY_FLAG_OVERFLOW == 0 {
            return Ok(e.value.clone());
        }
        if e.value.len() != 12 {
            return Err(corrupt("overflow pointer length"));
        }
        let mut pid = u64::from_le_bytes(e.value[0..8].try_into().unwrap());
        let total = u32::from_le_bytes(e.value[8..12].try_into().unwrap()) as usize;
        let mut out = Vec::with_capacity(total);
        let mut guard = 0;
        while pid != 0 {
            guard += 1;
            if guard > 1_000_000 {
                return Err(corrupt("overflow chain loop"));
            }
            let (fi, pool) = Self::fetch(ctx, pid)?;
            let page = &pool.frame(fi).bytes[..];
            if page_type(page)? != PageType::Overflow {
                return Err(corrupt("overflow chain points to non-overflow page"));
            }
            let len = slot_count(page);
            out.extend_from_slice(&page[PAGE_HEADER_LEN..PAGE_HEADER_LEN + len]);
            pid = u64::from_le_bytes(page[PAGE_SIZE - 8..].try_into().unwrap());
        }
        if out.len() != total {
            return Err(corrupt("overflow length mismatch"));
        }
        Ok(out)
    }

    /// Visible scan: at most one entry per key (newest visible), tombstones filtered by the caller.
    /// `start` inclusive, `end` exclusive (None = unbounded).
    pub fn scan_visible(
        &self,
        ctx: &mut TreeCtx,
        start: &[u8],
        end: Option<&[u8]>,
        visible_seq: u64,
        limit: usize,
    ) -> CoreResult<Vec<LeafEntry>> {
        let mut out = Vec::new();
        let mut cursor_key: Vec<u8> = start.to_vec();
        let mut cursor_seq = u64::MAX;
        let mut current_key: Option<Vec<u8>> = None;
        let mut guard = 0u64;
        let mut path = self.descend(ctx, &cursor_key, cursor_seq)?;
        'outer: loop {
            guard += 1;
            if guard > 10_000_000 {
                return Err(corrupt("scan loop"));
            }
            let (leaf, _) = *path.last().unwrap();
            let (fi, pool) = Self::fetch(ctx, leaf)?;
            let page = &pool.frame(fi).bytes[..];
            let n = slot_count(page);
            let mut advanced = false;
            for i in 0..n {
                let e = LeafEntry::decode(record(page, i)?)?;
                if leaf_cmp(&e.key, e.seq, &cursor_key, cursor_seq) == std::cmp::Ordering::Less {
                    continue;
                }
                if let Some(end) = end {
                    if e.key.as_slice() >= end {
                        break 'outer;
                    }
                }
                advanced = true;
                // next cursor position is strictly after this entry
                cursor_key = e.key.clone();
                cursor_seq = e.seq.saturating_sub(1);
                if e.seq == 0 {
                    // seq 0 never exists; move cursor to next key
                    cursor_key.push(0);
                    cursor_seq = u64::MAX;
                }
                if current_key.as_deref() == Some(e.key.as_slice()) {
                    continue; // already emitted a visible version for this key
                }
                if e.seq <= visible_seq {
                    current_key = Some(e.key.clone());
                    out.push(e);
                    if out.len() >= limit {
                        break 'outer;
                    }
                }
            }
            if !advanced {
                // nothing at/after the cursor in this leaf: move to the right neighbor
                match self.right_neighbor_path(ctx, &path)? {
                    Some(next) => path = next,
                    None => break,
                }
            } else {
                // continue in the leaf holding the next position (may be this leaf or its neighbor)
                path = self.descend(ctx, &cursor_key, cursor_seq)?;
            }
        }
        Ok(out)
    }

    /// Copy-on-write persistence of every dirty page reachable from the root. Returns the new root id.
    pub fn persist(&mut self, ctx: &mut TreeCtx) -> CoreResult<u64> {
        let new_root = self.persist_page(ctx, self.root)?;
        self.root = new_root;
        Ok(new_root)
    }

    fn persist_page(&mut self, ctx: &mut TreeCtx, pid: u64) -> CoreResult<u64> {
        let lsn = ctx.current_lsn;
        if !ctx.pool.contains(pid) {
            return Ok(pid); // clean, on disk
        }
        let (ptype, mut dirty, records) = {
            let (fi, pool) = Self::fetch(ctx, pid)?;
            let page = &pool.frame(fi).bytes[..];
            (page_type(page)?, pool.frame(fi).dirty, read_records(page)?)
        };
        if ptype == PageType::Internal {
            let mut changed = false;
            let mut new_records = Vec::with_capacity(records.len());
            for r in &records {
                let e = InternalEntry::decode(r)?;
                let new_child = self.persist_page(ctx, e.child)?;
                if new_child != e.child {
                    changed = true;
                    new_records.push(
                        InternalEntry {
                            key: e.key,
                            seq: e.seq,
                            child: new_child,
                        }
                        .encode(),
                    );
                } else {
                    new_records.push(r.clone());
                }
            }
            if changed {
                let (fi, pool) = Self::fetch(ctx, pid)?;
                let frame = pool.frame_mut(fi);
                let pg = &mut frame.bytes[..];
                let mut header = PageHeader::read(pg, pid)?;
                header.page_lsn = lsn;
                write_records(pg, &mut header, &new_records);
                pool.mark_dirty(fi, lsn);
                dirty = true;
            }
        }
        if ptype == PageType::Leaf {
            // overflow chains are immutable once written: flush dirty chain pages in place
            for r in &records {
                let e = LeafEntry::decode(r)?;
                if e.flags & ENTRY_FLAG_OVERFLOW != 0 && e.value.len() == 12 {
                    let mut opid = u64::from_le_bytes(e.value[0..8].try_into().unwrap());
                    // walk the whole chain: an evicted (already flushed) page may precede a dirty one
                    let mut guard = 0u32;
                    while opid != 0 {
                        guard += 1;
                        if guard > 1_000_000 {
                            return Err(corrupt("overflow chain loop"));
                        }
                        let next = if ctx.pool.contains(opid) {
                            let (ofi, opool) = Self::fetch(ctx, opid)?;
                            let f = opool.frame(ofi);
                            let next =
                                u64::from_le_bytes(f.bytes[PAGE_SIZE - 8..].try_into().unwrap());
                            if f.dirty {
                                let bytes = *f.bytes;
                                ctx.io.write_page(opid, &bytes)?;
                                ctx.pool.rekey(opid, opid, bytes);
                            }
                            next
                        } else {
                            // not resident => clean and on disk; read only the chain pointer
                            let mut buf = vec![0u8; PAGE_SIZE];
                            ctx.io.read_page(opid, &mut buf)?;
                            PageHeader::read(&buf, opid)?;
                            u64::from_le_bytes(buf[PAGE_SIZE - 8..].try_into().unwrap())
                        };
                        opid = next;
                    }
                }
            }
        }
        if !dirty {
            return Ok(pid);
        }
        // write under a fresh id
        let new_id = self.alloc();
        let mut bytes = {
            let (fi, pool) = Self::fetch(ctx, pid)?;
            *pool.frame(fi).bytes
        };
        let mut header = PageHeader::read(&bytes, pid)?;
        header.page_id = new_id;
        header.page_lsn = lsn;
        header.write(&mut bytes);
        ctx.io.write_page(new_id, &bytes)?;
        // re-key the frame under the new id, clean
        ctx.pool.rekey(pid, new_id, bytes);
        Ok(new_id)
    }

    /// Structural verification (SPEC-002 §127): checksums, types, ordering, separators, reachability.
    pub fn verify(&self, ctx: &mut TreeCtx) -> CoreResult<VerifyStats> {
        let mut stats = VerifyStats::default();
        let mut last_key: Option<(Vec<u8>, u64)> = None;
        self.verify_page(ctx, self.root, None, None, &mut stats, &mut last_key, 0)?;
        Ok(stats)
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_page(
        &self,
        ctx: &mut TreeCtx,
        pid: u64,
        low: Option<(&[u8], u64)>,
        high: Option<(&[u8], u64)>,
        stats: &mut VerifyStats,
        last: &mut Option<(Vec<u8>, u64)>,
        depth: usize,
    ) -> CoreResult<()> {
        if depth > 64 {
            return Err(corrupt("tree too deep"));
        }
        let (fi, pool) = Self::fetch(ctx, pid)?;
        let page = pool.frame(fi).bytes.to_vec();
        PageHeader::read(&page, pid)?;
        stats.pages += 1;
        match page_type(&page)? {
            PageType::Leaf => {
                stats.leaves += 1;
                let n = slot_count(&page);
                for i in 0..n {
                    let e = LeafEntry::decode(record(&page, i)?)?;
                    stats.entries += 1;
                    if let Some((lk, ls)) = low {
                        if leaf_cmp(&e.key, e.seq, lk, ls) == std::cmp::Ordering::Less {
                            return Err(corrupt("leaf entry below separator"));
                        }
                    }
                    if let Some((hk, hs)) = high {
                        if leaf_cmp(&e.key, e.seq, hk, hs) != std::cmp::Ordering::Less {
                            return Err(corrupt("leaf entry at/above upper separator"));
                        }
                    }
                    if let Some((lk, ls)) = last {
                        if leaf_cmp(lk, *ls, &e.key, e.seq) != std::cmp::Ordering::Less {
                            return Err(corrupt(
                                "leaf entries out of physical order or duplicated",
                            ));
                        }
                    }
                    *last = Some((e.key.clone(), e.seq));
                    if e.flags & ENTRY_FLAG_OVERFLOW != 0 {
                        self.read_value(ctx, &e)?;
                    }
                }
            }
            PageType::Internal => {
                stats.internals += 1;
                let n = slot_count(&page);
                if n == 0 {
                    return Err(corrupt("empty internal page"));
                }
                let entries: Vec<InternalEntry> = (0..n)
                    .map(|i| InternalEntry::decode(record(&page, i)?))
                    .collect::<CoreResult<_>>()?;
                if !entries[0].key.is_empty() || entries[0].seq != u64::MAX {
                    return Err(corrupt(
                        "first internal entry must be the leftmost separator",
                    ));
                }
                for w in entries.windows(2) {
                    if w[0].key.is_empty() && w[0].seq == u64::MAX {
                        continue;
                    }
                    if leaf_cmp(&w[0].key, w[0].seq, &w[1].key, w[1].seq)
                        != std::cmp::Ordering::Less
                    {
                        return Err(corrupt("separators not increasing"));
                    }
                }
                for (i, e) in entries.iter().enumerate() {
                    let lo = if i == 0 {
                        low
                    } else {
                        Some((e.key.as_slice(), e.seq))
                    };
                    let hi = if i + 1 < n {
                        Some((entries[i + 1].key.as_slice(), entries[i + 1].seq))
                    } else {
                        high
                    };
                    self.verify_page(ctx, e.child, lo, hi, stats, last, depth + 1)?;
                }
            }
            other => return Err(corrupt(&format!("unexpected page type {other:?} in tree"))),
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VerifyStats {
    pub pages: u64,
    pub leaves: u64,
    pub internals: u64,
    pub entries: u64,
}
