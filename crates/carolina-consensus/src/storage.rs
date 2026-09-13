//! Persistent Raft state: current term, vote and the log (SPEC-008 §5: proposal acceptance by one
//! process and a local flush are not `propose` success; durable per-voter state is what a quorum
//! acknowledges).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::NodeId;

use crate::Entry;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Persistent {
    pub term: u64,
    pub voted_for: Option<NodeId>,
    pub entries: Vec<Entry>,
    /// Last index (and its term) covered by the applied-state snapshot: the log starts at
    /// `snapshot_index + 1` (SPEC-008 §5, SPEC-011 §9). Zero means nothing has been compacted.
    pub snapshot_index: u64,
    pub snapshot_term: u64,
}

pub trait RaftStorage {
    fn load(&self) -> CoreResult<Persistent>;
    fn save_term_vote(&mut self, term: u64, voted_for: Option<NodeId>) -> CoreResult<()>;
    fn append(&mut self, entries: &[Entry]) -> CoreResult<()>;
    /// Remove every entry with `index >= from`.
    fn truncate_from(&mut self, from: u64) -> CoreResult<()>;
    /// Drop every entry at or below `index` and record it as the new log base. The caller
    /// guarantees that every voter has durably applied that index, so no peer can still need the
    /// entries. The base is written before the entries disappear: a crash in between leaves a log
    /// longer than the base claims (tolerated by `load`), never one that is shorter.
    fn compact_to(&mut self, index: u64, term: u64) -> CoreResult<()>;
}

/// In-memory storage for the simulator: survives a simulated crash because the simulator keeps
/// it outside the node; every write is "durable" at the moment it returns.
#[derive(Debug, Clone, Default)]
pub struct MemRaftStorage {
    pub state: Persistent,
    pub writes: u64,
    pub compactions: u64,
}

impl RaftStorage for MemRaftStorage {
    fn load(&self) -> CoreResult<Persistent> {
        Ok(self.state.clone())
    }
    fn save_term_vote(&mut self, term: u64, voted_for: Option<NodeId>) -> CoreResult<()> {
        self.state.term = term;
        self.state.voted_for = voted_for;
        self.writes += 1;
        Ok(())
    }
    fn append(&mut self, entries: &[Entry]) -> CoreResult<()> {
        self.state.entries.extend_from_slice(entries);
        self.writes += 1;
        Ok(())
    }
    fn truncate_from(&mut self, from: u64) -> CoreResult<()> {
        self.state.entries.retain(|e| e.index < from);
        self.writes += 1;
        Ok(())
    }
    fn compact_to(&mut self, index: u64, term: u64) -> CoreResult<()> {
        self.state.snapshot_index = index;
        self.state.snapshot_term = term;
        self.state.entries.retain(|e| e.index > index);
        self.writes += 1;
        self.compactions += 1;
        Ok(())
    }
}

/// File-backed storage: `<dir>/raft.state` (term + vote, written atomically via rename + fsync)
/// and `<dir>/raft.log` (CRC32C-framed entries, appended with fsync; truncation rewrites the file).
pub struct FileRaftStorage {
    dir: PathBuf,
    log: File,
    entries: Vec<Entry>,
    snapshot_index: u64,
    snapshot_term: u64,
}

/// `(snapshot_index, snapshot_term)` recorded in `raft.state`, or `(0, 0)` when the file is absent
/// or predates compaction.
fn read_base(path: &Path) -> CoreResult<(u64, u64)> {
    if !path.exists() {
        return Ok((0, 0));
    }
    let bytes = std::fs::read(path)?;
    let v = CanonValue::decode(&bytes, &carolina_core::limits::Limits::v1())?;
    match (v.field("snapshot_index"), v.field("snapshot_term")) {
        (Ok(i), Ok(t)) => Ok((i.as_u64()?, t.as_u64()?)),
        _ => Ok((0, 0)),
    }
}

const FRAME_MAGIC: &[u8; 4] = b"CRFT";

fn frame(e: &Entry) -> Vec<u8> {
    let body = e.encode();
    let mut out = Vec::with_capacity(body.len() + 12);
    out.extend_from_slice(FRAME_MAGIC);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&crc32c::crc32c(&body).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

impl FileRaftStorage {
    pub fn open(dir: &Path) -> CoreResult<FileRaftStorage> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("raft.log");
        let mut entries = Vec::new();
        if path.exists() {
            let mut bytes = Vec::new();
            File::open(&path)?.read_to_end(&mut bytes)?;
            let mut pos = 0usize;
            let limits = carolina_core::limits::Limits::v1();
            let base = read_base(&dir.join("raft.state"))?.0;
            while pos + 12 <= bytes.len() {
                if &bytes[pos..pos + 4] != FRAME_MAGIC {
                    return Err(CoreError::new(
                        ErrorCode::Corruption,
                        "raft log frame magic",
                    ));
                }
                let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
                let crc = u32::from_le_bytes(bytes[pos + 8..pos + 12].try_into().unwrap());
                let start = pos + 12;
                if start + len > bytes.len() {
                    break; // torn tail: the frame was never acknowledged as durable
                }
                let body = &bytes[start..start + len];
                if crc32c::crc32c(body) != crc {
                    return Err(CoreError::new(
                        ErrorCode::ChecksumMismatch,
                        "raft log frame crc",
                    ));
                }
                let e = Entry::decode(body, &limits)?;
                // Entries at or below the base survive a crash between writing the base and
                // rewriting the file. They are redundant, never contradictory, so they are skipped
                // instead of failing closed.
                if e.index <= base {
                    pos = start + len;
                    continue;
                }
                let expected = entries
                    .last()
                    .map(|l: &Entry| l.index + 1)
                    .unwrap_or(base + 1);
                if e.index != expected {
                    return Err(CoreError::new(ErrorCode::Corruption, "raft log index gap"));
                }
                entries.push(e);
                pos = start + len;
            }
            if pos < bytes.len() {
                // drop the torn tail durably
                let keep = bytes[..pos].to_vec();
                std::fs::write(&path, keep)?;
            }
        }
        let log = OpenOptions::new().create(true).append(true).open(&path)?;
        let (snapshot_index, snapshot_term) = read_base(&dir.join("raft.state"))?;
        Ok(FileRaftStorage {
            dir: dir.to_path_buf(),
            log,
            entries,
            snapshot_index,
            snapshot_term,
        })
    }

    fn read_state(&self) -> CoreResult<(u64, Option<NodeId>)> {
        let path = self.dir.join("raft.state");
        if !path.exists() {
            return Ok((0, None));
        }
        let bytes = std::fs::read(&path)?;
        let v = CanonValue::decode(&bytes, &carolina_core::limits::Limits::v1())?;
        let term = v.field("term")?.as_u64()?;
        let voted_for = match v.field("voted_for")? {
            CanonValue::Null => None,
            x => Some(NodeId::from_canon(x)?),
        };
        Ok((term, voted_for))
    }

    fn rewrite_log(&mut self) -> CoreResult<()> {
        let path = self.dir.join("raft.log");
        let tmp = self.dir.join("raft.log.tmp");
        {
            let mut f = File::create(&tmp)?;
            for e in &self.entries {
                f.write_all(&frame(e))?;
            }
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &path)?;
        self.log = OpenOptions::new().append(true).open(&path)?;
        self.log.sync_all()?;
        Ok(())
    }
}

impl RaftStorage for FileRaftStorage {
    fn load(&self) -> CoreResult<Persistent> {
        let (term, voted_for) = self.read_state()?;
        Ok(Persistent {
            term,
            voted_for,
            entries: self.entries.clone(),
            snapshot_index: self.snapshot_index,
            snapshot_term: self.snapshot_term,
        })
    }
    fn save_term_vote(&mut self, term: u64, voted_for: Option<NodeId>) -> CoreResult<()> {
        let v = CanonValue::obj()
            .fu64("snapshot_index", self.snapshot_index)
            .fu64("snapshot_term", self.snapshot_term)
            .fu64("term", term)
            .fopt("voted_for", &voted_for)
            .build()
            .encode();
        let tmp = self.dir.join("raft.state.tmp");
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&v)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, self.dir.join("raft.state"))?;
        Ok(())
    }
    fn append(&mut self, entries: &[Entry]) -> CoreResult<()> {
        let mut buf = Vec::new();
        for e in entries {
            buf.extend_from_slice(&frame(e));
        }
        self.log.write_all(&buf)?;
        self.log.sync_data()?;
        self.entries.extend_from_slice(entries);
        Ok(())
    }
    fn truncate_from(&mut self, from: u64) -> CoreResult<()> {
        self.entries.retain(|e| e.index < from);
        self.rewrite_log()
    }
    fn compact_to(&mut self, index: u64, term: u64) -> CoreResult<()> {
        if index <= self.snapshot_index {
            return Ok(());
        }
        let (cur_term, voted_for) = self.read_state()?;
        self.snapshot_index = index;
        self.snapshot_term = term;
        // base first, entries second: a crash in between only leaves redundant entries behind
        let v = CanonValue::obj()
            .fu64("snapshot_index", index)
            .fu64("snapshot_term", term)
            .fu64("term", cur_term)
            .fopt("voted_for", &voted_for)
            .build()
            .encode();
        let tmp = self.dir.join("raft.state.tmp");
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&v)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, self.dir.join("raft.state"))?;
        self.entries.retain(|e| e.index > index);
        self.rewrite_log()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_storage_roundtrip_and_torn_tail() {
        let dir = std::env::temp_dir().join(format!("carolina-raft-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut s = FileRaftStorage::open(&dir).unwrap();
            s.save_term_vote(3, Some(NodeId::derive("n1"))).unwrap();
            s.append(&[
                Entry {
                    term: 1,
                    index: 1,
                    data: b"a".to_vec(),
                },
                Entry {
                    term: 3,
                    index: 2,
                    data: b"b".to_vec(),
                },
            ])
            .unwrap();
            s.truncate_from(2).unwrap();
            s.append(&[Entry {
                term: 3,
                index: 2,
                data: b"c".to_vec(),
            }])
            .unwrap();
        }
        // torn tail: append half a frame
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(dir.join("raft.log"))
                .unwrap();
            f.write_all(b"CRFT\x10\x00\x00\x00abcd").unwrap();
        }
        let s = FileRaftStorage::open(&dir).unwrap();
        let p = s.load().unwrap();
        assert_eq!(p.term, 3);
        assert_eq!(p.voted_for, Some(NodeId::derive("n1")));
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.entries[1].data, b"c");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
