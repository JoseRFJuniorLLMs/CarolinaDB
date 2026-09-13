//! Commit Journal (SPEC-002 §45–§58, §104–§105, §176).
//!
//! Segments `journal/journal-%016x.astj` of a configurable size. Frames carry a monotonic LSN.
//! Recovery scans forward; an invalid frame at the physical tail of the last segment is a torn
//! tail and is truncated; an invalid frame anywhere else is corruption and fails closed.

use std::path::{Path, PathBuf};

use carolina_core::error::{CoreError, CoreResult, ErrorCode};

use crate::format::{JournalFrame, JournalRecordKind, FRAME_HEADER_LEN};
use crate::io::{Faults, SeqFile};

pub const DEFAULT_SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_JOURNAL_PAYLOAD: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityMode {
    Sync,
    GroupSync,
    /// Testing only. Never default. Excluded from durability qualification (SPEC-002 §53).
    UnsafeNoFsync,
}

pub struct Journal {
    dir: PathBuf,
    faults: Faults,
    segment_bytes: u64,
    current_segment: u64,
    file: SeqFile,
    next_lsn: u64,
    durable_lsn: u64,
    appended_lsn: u64,
    pub mode: DurabilityMode,
    pub fsync_count: u64,
    pub appended_bytes: u64,
}

pub fn segment_path(dir: &Path, seg: u64) -> PathBuf {
    dir.join(format!("journal-{seg:016x}.astj"))
}

/// A scanned frame with its physical position.
#[derive(Debug, Clone)]
pub struct ScannedFrame {
    pub segment: u64,
    pub offset: u64,
    pub frame: JournalFrame,
}

impl Journal {
    /// Open (or create) the journal. `start_segment` is the segment named by the manifest; scanning
    /// from it yields every frame after the checkpoint. Returns the journal positioned for appends
    /// plus all frames found (callers filter by LSN). Torn tails are truncated.
    pub fn open(
        dir: &Path,
        start_segment: u64,
        faults: Faults,
        mode: DurabilityMode,
        segment_bytes: u64,
    ) -> CoreResult<(Journal, Vec<ScannedFrame>)> {
        std::fs::create_dir_all(dir)?;
        let mut segments: Vec<u64> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().to_string();
            if let Some(hex) = name
                .strip_prefix("journal-")
                .and_then(|s| s.strip_suffix(".astj"))
            {
                if let Ok(n) = u64::from_str_radix(hex, 16) {
                    segments.push(n);
                }
            }
        }
        segments.sort_unstable();
        let mut frames = Vec::new();
        let mut max_lsn = 0u64;
        let last_seg = segments.last().copied();
        for &seg in segments.iter().filter(|s| **s >= start_segment) {
            let mut f = SeqFile::open(&segment_path(dir, seg), faults.clone())?;
            let bytes = f.read_all()?;
            let mut off = 0usize;
            loop {
                if off >= bytes.len() {
                    break;
                }
                match JournalFrame::decode(&bytes[off..], MAX_JOURNAL_PAYLOAD) {
                    Ok(Some((frame, n))) => {
                        if frame.lsn <= max_lsn && max_lsn != 0 {
                            return Err(CoreError::new(
                                ErrorCode::Corruption,
                                format!("non-monotonic LSN {} after {}", frame.lsn, max_lsn),
                            ));
                        }
                        max_lsn = frame.lsn;
                        frames.push(ScannedFrame {
                            segment: seg,
                            offset: off as u64,
                            frame,
                        });
                        off += n;
                    }
                    Ok(None) => {
                        // incomplete frame: legal only as the physical tail of the last segment
                        if Some(seg) == last_seg {
                            f.truncate(off as u64)?;
                            break;
                        }
                        return Err(CoreError::new(
                            ErrorCode::Corruption,
                            format!("incomplete frame in non-final segment {seg}"),
                        ));
                    }
                    Err(e) => {
                        // A checksum failure in the final frame of the last segment is a torn write; elsewhere corruption.
                        if Some(seg) == last_seg && is_tail_frame(&bytes[off..]) {
                            f.truncate(off as u64)?;
                            break;
                        }
                        return Err(CoreError::new(
                            ErrorCode::Corruption,
                            format!("journal corruption in segment {seg} at offset {off}: {e}"),
                        ));
                    }
                }
            }
        }
        let current_segment = last_seg.unwrap_or(1).max(start_segment.max(1));
        let file = SeqFile::open(&segment_path(dir, current_segment), faults.clone())?;
        let j = Journal {
            dir: dir.to_path_buf(),
            faults,
            segment_bytes,
            current_segment,
            file,
            next_lsn: max_lsn + 1,
            durable_lsn: max_lsn,
            appended_lsn: max_lsn,
            mode,
            fsync_count: 0,
            appended_bytes: 0,
        };
        Ok((j, frames))
    }

    pub fn durable_lsn(&self) -> u64 {
        self.durable_lsn
    }
    pub fn next_lsn(&self) -> u64 {
        self.next_lsn
    }
    pub fn current_segment(&self) -> u64 {
        self.current_segment
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Append one frame (not yet durable). Returns its LSN.
    pub fn append(
        &mut self,
        kind: JournalRecordKind,
        txn_id: [u8; 32],
        payload: Vec<u8>,
    ) -> CoreResult<u64> {
        if payload.len() > MAX_JOURNAL_PAYLOAD {
            return Err(CoreError::new(
                ErrorCode::BatchTooLarge,
                "journal payload exceeds limit",
            ));
        }
        if self.file.len() >= self.segment_bytes {
            self.roll_segment()?;
        }
        let lsn = self.next_lsn;
        let frame = JournalFrame {
            kind,
            flags: 0,
            lsn,
            txn_id,
            payload,
        };
        let bytes = frame.encode();
        self.file.append(&bytes)?;
        self.appended_bytes += bytes.len() as u64;
        self.next_lsn += 1;
        self.appended_lsn = lsn;
        Ok(lsn)
    }

    fn roll_segment(&mut self) -> CoreResult<()> {
        self.file.sync()?;
        self.current_segment += 1;
        self.file = SeqFile::open(
            &segment_path(&self.dir, self.current_segment),
            self.faults.clone(),
        )?;
        crate::io::sync_dir(&self.dir)?;
        Ok(())
    }

    /// Cross the durable barrier for everything appended so far.
    pub fn sync(&mut self) -> CoreResult<()> {
        match self.mode {
            DurabilityMode::Sync | DurabilityMode::GroupSync => {
                self.file.sync()?;
                self.fsync_count += 1;
            }
            DurabilityMode::UnsafeNoFsync => {}
        }
        self.durable_lsn = self.appended_lsn;
        Ok(())
    }
}

/// True when the bytes at `b` look like a partially written frame at the end of a file: the
/// declared total length exceeds what is on disk, or the trailer is missing/zero.
fn is_tail_frame(b: &[u8]) -> bool {
    if b.len() < FRAME_HEADER_LEN {
        return true;
    }
    let total = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    if total == 0 || total > b.len() {
        return true;
    }
    let trailer = u32::from_le_bytes(b[total - 4..total].try_into().unwrap()) as usize;
    trailer != total || b[total..].iter().all(|x| *x == 0)
}
