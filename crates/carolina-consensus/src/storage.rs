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
}

pub trait RaftStorage {
    fn load(&self) -> CoreResult<Persistent>;
    fn save_term_vote(&mut self, term: u64, voted_for: Option<NodeId>) -> CoreResult<()>;
    fn append(&mut self, entries: &[Entry]) -> CoreResult<()>;
    /// Remove every entry with `index >= from`.
    fn truncate_from(&mut self, from: u64) -> CoreResult<()>;
}

/// In-memory storage for the simulator: survives a simulated crash because the simulator keeps
/// it outside the node; every write is "durable" at the moment it returns.
#[derive(Debug, Clone, Default)]
pub struct MemRaftStorage {
    pub state: Persistent,
    pub writes: u64,
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
}

/// File-backed storage: `<dir>/raft.state` (term + vote, written atomically via rename + fsync)
/// and `<dir>/raft.log` (CRC32C-framed entries, appended with fsync; truncation rewrites the file).
pub struct FileRaftStorage {
    dir: PathBuf,
    log: File,
    entries: Vec<Entry>,
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
                if e.index != entries.len() as u64 + 1 {
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
        Ok(FileRaftStorage {
            dir: dir.to_path_buf(),
            log,
            entries,
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
        })
    }
    fn save_term_vote(&mut self, term: u64, voted_for: Option<NodeId>) -> CoreResult<()> {
        let v = CanonValue::obj()
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
