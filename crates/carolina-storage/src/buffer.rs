//! Explicit buffer pool with CLOCK replacement (SPEC-002 §23–§27, §57, §82).
//!
//! Dirty pages are never evicted: the tree persists them copy-on-write at checkpoint (see
//! `btree::BTree::persist`), which also enforces WAL-before-page because a checkpoint runs only
//! after the journal barrier. [`BufferPool::flush_page`] additionally refuses to write a page whose
//! LSN is not durable. The pool is single-owner in the correctness-first kernel; the runtime
//! serializes access to the kernel.

use std::collections::HashMap;

use carolina_core::error::{CoreError, CoreResult, ErrorCode};

use crate::format::{PageHeader, PageType, PAGE_SIZE};
use crate::io::PageIo;

pub struct Frame {
    pub page_id: u64,
    pub dirty: bool,
    pub page_lsn: u64,
    pub pin: u32,
    pub referenced: bool,
    pub bytes: Box<[u8; PAGE_SIZE]>,
}

pub struct BufferPool {
    frames: Vec<Frame>,
    map: HashMap<u64, usize>,
    clock: usize,
    capacity: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub flushes: u64,
    pub grown: u64,
}

impl BufferPool {
    pub fn new(capacity: usize) -> BufferPool {
        BufferPool {
            frames: Vec::with_capacity(capacity),
            map: HashMap::new(),
            clock: 0,
            capacity: capacity.max(8),
            hits: 0,
            misses: 0,
            evictions: 0,
            flushes: 0,
            grown: 0,
        }
    }

    pub fn dirty_count(&self) -> usize {
        self.frames.iter().filter(|f| f.dirty).count()
    }

    pub fn contains(&self, page_id: u64) -> bool {
        self.map.contains_key(&page_id)
    }

    /// Fetch a page into the pool (reading from `io` if absent) and return its frame index.
    pub fn fetch(&mut self, io: &dyn PageIo, page_id: u64, durable_lsn: u64) -> CoreResult<usize> {
        if let Some(&i) = self.map.get(&page_id) {
            self.hits += 1;
            self.frames[i].referenced = true;
            return Ok(i);
        }
        self.misses += 1;
        let mut bytes = Box::new([0u8; PAGE_SIZE]);
        io.read_page(page_id, &mut bytes[..])?;
        PageHeader::read(&bytes[..], page_id)?;
        let i = self.place(io, page_id, bytes, durable_lsn)?;
        Ok(i)
    }

    /// Insert a brand-new page (already formatted by the caller) as dirty.
    pub fn insert_new(
        &mut self,
        io: &dyn PageIo,
        page_id: u64,
        page_type: PageType,
        page_lsn: u64,
        durable_lsn: u64,
    ) -> CoreResult<usize> {
        let mut bytes = Box::new([0u8; PAGE_SIZE]);
        let mut h = PageHeader::new(page_type, page_id);
        h.page_lsn = page_lsn;
        h.write(&mut bytes[..]);
        let i = self.place(io, page_id, bytes, durable_lsn)?;
        self.frames[i].dirty = true;
        self.frames[i].page_lsn = page_lsn;
        Ok(i)
    }

    fn place(
        &mut self,
        io: &dyn PageIo,
        page_id: u64,
        bytes: Box<[u8; PAGE_SIZE]>,
        durable_lsn: u64,
    ) -> CoreResult<usize> {
        let _ = (io, durable_lsn);
        let lsn = PageHeader::read(&bytes[..], page_id)
            .map(|h| h.page_lsn)
            .unwrap_or(0);
        if self.frames.len() < self.capacity {
            let i = self.frames.len();
            self.frames.push(Frame {
                page_id,
                dirty: false,
                page_lsn: lsn,
                pin: 0,
                referenced: true,
                bytes,
            });
            self.map.insert(page_id, i);
            return Ok(i);
        }
        match self.evict_one() {
            Some(i) => {
                self.map.remove(&self.frames[i].page_id);
                self.frames[i] = Frame {
                    page_id,
                    dirty: false,
                    page_lsn: lsn,
                    pin: 0,
                    referenced: true,
                    bytes,
                };
                self.map.insert(page_id, i);
                Ok(i)
            }
            None => {
                // Only dirty/pinned frames remain: grow past the target capacity. Dirty pages are never
                // written except by a checkpoint, and the kernel checkpoints on dirty pressure, so the
                // overshoot is bounded by one batch plus the checkpoint threshold (SPEC-002 §113).
                if self.frames.len() >= self.capacity.saturating_mul(16) {
                    return Err(CoreError::new(
                        ErrorCode::ResourceLimit,
                        "buffer pool overshoot limit reached; checkpoint policy is misconfigured",
                    ));
                }
                let i = self.frames.len();
                self.frames.push(Frame {
                    page_id,
                    dirty: false,
                    page_lsn: lsn,
                    pin: 0,
                    referenced: true,
                    bytes,
                });
                self.map.insert(page_id, i);
                self.grown += 1;
                Ok(i)
            }
        }
    }

    /// CLOCK: find a clean, unpinned victim. Dirty pages are never evicted: under copy-on-write
    /// persistence a dirty page reaches disk only through a checkpoint that also rewrites its
    /// ancestors. Returns `None` when every frame is dirty or pinned.
    fn evict_one(&mut self) -> Option<usize> {
        let n = self.frames.len();
        for _ in 0..(2 * n + 1) {
            let i = self.clock;
            self.clock = (self.clock + 1) % n;
            let f = &mut self.frames[i];
            if f.pin > 0 || f.dirty {
                continue;
            }
            if f.referenced {
                f.referenced = false;
                continue;
            }
            self.evictions += 1;
            return Some(i);
        }
        None
    }

    /// Shrink back toward the target capacity by dropping clean frames (called after a checkpoint).
    pub fn shrink_to_capacity(&mut self) {
        if self.frames.len() <= self.capacity {
            return;
        }
        let mut keep: Vec<Frame> = Vec::with_capacity(self.capacity);
        let mut dropped = 0;
        for f in self.frames.drain(..) {
            if (f.dirty || f.pin > 0 || keep.len() < self.capacity)
                && !(f.pin == 0 && !f.dirty && keep.len() >= self.capacity)
            {
                keep.push(f);
            } else {
                dropped += 1;
            }
        }
        self.map.clear();
        for (i, f) in keep.iter().enumerate() {
            self.map.insert(f.page_id, i);
        }
        self.frames = keep;
        self.clock = 0;
        let _ = dropped;
    }

    pub fn frame(&self, i: usize) -> &Frame {
        &self.frames[i]
    }
    pub fn frame_mut(&mut self, i: usize) -> &mut Frame {
        &mut self.frames[i]
    }
    pub fn pin(&mut self, i: usize) {
        self.frames[i].pin += 1;
    }
    pub fn unpin(&mut self, i: usize) {
        self.frames[i].pin = self.frames[i].pin.saturating_sub(1);
    }
    pub fn mark_dirty(&mut self, i: usize, page_lsn: u64) {
        let f = &mut self.frames[i];
        f.dirty = true;
        f.page_lsn = f.page_lsn.max(page_lsn);
    }

    fn flush_frame(&mut self, io: &dyn PageIo, i: usize) -> CoreResult<()> {
        let f = &mut self.frames[i];
        // rewrite header lsn + checksum before flushing
        let mut h = PageHeader::read(&f.bytes[..], f.page_id)
            .unwrap_or_else(|_| PageHeader::new(PageType::Leaf, f.page_id));
        h.page_lsn = f.page_lsn;
        h.write(&mut f.bytes[..]);
        io.write_page(f.page_id, &f.bytes[..])?;
        f.dirty = false;
        self.flushes += 1;
        Ok(())
    }

    /// Flush every dirty page whose LSN is durable (`page_lsn <= durable_lsn`). Returns the number flushed.
    pub fn flush_dirty_up_to(&mut self, io: &dyn PageIo, durable_lsn: u64) -> CoreResult<usize> {
        let mut n = 0;
        for i in 0..self.frames.len() {
            if self.frames[i].dirty && self.frames[i].page_lsn <= durable_lsn {
                self.flush_frame(io, i)?;
                n += 1;
            }
        }
        Ok(n)
    }

    /// Explicit flush of one page (refuses if its LSN is not durable).
    pub fn flush_page(
        &mut self,
        io: &dyn PageIo,
        page_id: u64,
        durable_lsn: u64,
    ) -> CoreResult<()> {
        if let Some(&i) = self.map.get(&page_id) {
            if self.frames[i].dirty {
                if self.frames[i].page_lsn > durable_lsn {
                    return Err(CoreError::new(
                        ErrorCode::Internal,
                        "WAL-before-page violation refused",
                    ));
                }
                self.flush_frame(io, i)?;
            }
        }
        Ok(())
    }

    /// Update the recorded page_lsn of every dirty frame that still has the unwritten sentinel.
    pub fn stamp_dirty_lsn(&mut self, lsn: u64) {
        for f in &mut self.frames {
            if f.dirty && f.page_lsn == u64::MAX {
                f.page_lsn = lsn;
            }
        }
    }

    /// Re-key a frame after copy-on-write persistence: the page now lives at `new_id`, clean, with the given bytes.
    pub fn rekey(&mut self, old_id: u64, new_id: u64, bytes: [u8; PAGE_SIZE]) {
        if let Some(i) = self.map.remove(&old_id) {
            let f = &mut self.frames[i];
            f.page_id = new_id;
            f.dirty = false;
            *f.bytes = bytes;
            self.map.insert(new_id, i);
        }
    }

    /// Drop all cached frames (used after a simulated crash to forget volatile state).
    pub fn clear(&mut self) {
        self.frames.clear();
        self.map.clear();
        self.clock = 0;
    }
}
