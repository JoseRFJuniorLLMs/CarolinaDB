//! Portable positional file I/O and fault injection points (SPEC-002 §28–§29, §132–§133).
//!
//! The default path uses positional reads/writes on a plain file. Every I/O call passes through
//! the [`FaultInjector`] so crash campaigns can stop the process at any durable boundary, produce
//! short writes or return I/O errors. Tests treat a `Crash` error as a process kill: the store is
//! dropped and reopened from the bytes on disk.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::{Arc, Mutex};

use carolina_core::error::{CoreError, CoreResult, ErrorCode};

/// Crash/fault points (SPEC-002 §132).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FaultPoint {
    BeforeJournalAppend,
    MidJournalWrite,
    BeforeFsync,
    AfterFsyncBeforePublish,
    DuringPageSplit,
    BeforePageFlush,
    MidPageWrite,
    BeforeManifestPublish,
    AfterManifestWriteBeforeFsync,
    DuringCheckpoint,
    DuringPrepare,
    AfterPrepareBeforeDecision,
    AfterDecisionBeforeInstall,
}

pub const ALL_FAULT_POINTS: [FaultPoint; 13] = [
    FaultPoint::BeforeJournalAppend,
    FaultPoint::MidJournalWrite,
    FaultPoint::BeforeFsync,
    FaultPoint::AfterFsyncBeforePublish,
    FaultPoint::DuringPageSplit,
    FaultPoint::BeforePageFlush,
    FaultPoint::MidPageWrite,
    FaultPoint::BeforeManifestPublish,
    FaultPoint::AfterManifestWriteBeforeFsync,
    FaultPoint::DuringCheckpoint,
    FaultPoint::DuringPrepare,
    FaultPoint::AfterPrepareBeforeDecision,
    FaultPoint::AfterDecisionBeforeInstall,
];

/// What the injector wants to happen at a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultAction {
    None,
    /// Simulated process kill: the operation stops here; bytes already written stay on disk.
    Crash,
    /// Return an I/O error (the caller must not report success).
    IoError,
}

pub trait FaultInjector: Send + Sync {
    fn at(&self, point: FaultPoint) -> FaultAction;
}

#[derive(Debug, Default)]
pub struct NoFaults;

impl FaultInjector for NoFaults {
    fn at(&self, _point: FaultPoint) -> FaultAction {
        FaultAction::None
    }
}

/// Crash at the n-th occurrence of one point.
#[derive(Debug)]
pub struct CrashAtNth {
    pub point: FaultPoint,
    pub nth: u32,
    pub action: FaultAction,
    seen: Mutex<u32>,
}

impl CrashAtNth {
    pub fn new(point: FaultPoint, nth: u32) -> Self {
        CrashAtNth {
            point,
            nth,
            action: FaultAction::Crash,
            seen: Mutex::new(0),
        }
    }
    pub fn io_error(point: FaultPoint, nth: u32) -> Self {
        CrashAtNth {
            point,
            nth,
            action: FaultAction::IoError,
            seen: Mutex::new(0),
        }
    }
    pub fn occurrences(&self) -> u32 {
        *self.seen.lock().unwrap()
    }
}

impl FaultInjector for CrashAtNth {
    fn at(&self, point: FaultPoint) -> FaultAction {
        if point != self.point {
            return FaultAction::None;
        }
        let mut s = self.seen.lock().unwrap();
        *s += 1;
        if *s == self.nth {
            self.action
        } else {
            FaultAction::None
        }
    }
}

pub type Faults = Arc<dyn FaultInjector>;

pub fn crash_error(point: FaultPoint) -> CoreError {
    CoreError::new(ErrorCode::Io, format!("simulated crash at {point:?}"))
}

/// Consult the injector; returns `Err` on Crash/IoError.
pub fn fault(faults: &Faults, point: FaultPoint) -> CoreResult<()> {
    match faults.at(point) {
        FaultAction::None => Ok(()),
        FaultAction::Crash => Err(crash_error(point)),
        FaultAction::IoError => Err(CoreError::new(
            ErrorCode::Io,
            format!("injected I/O error at {point:?}"),
        )),
    }
}

/// Page I/O over a data file (SPEC-002 §28).
pub trait PageIo: Send {
    fn read_page(&self, id: u64, dst: &mut [u8]) -> CoreResult<()>;
    fn write_page(&self, id: u64, src: &[u8]) -> CoreResult<()>;
    fn sync_data(&self) -> CoreResult<()>;
    fn page_count(&self) -> CoreResult<u64>;
}

pub struct FilePageIo {
    file: File,
    page_size: usize,
    faults: Faults,
}

impl FilePageIo {
    pub fn open(path: &Path, page_size: usize, faults: Faults) -> CoreResult<FilePageIo> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        Ok(FilePageIo {
            file,
            page_size,
            faults,
        })
    }
}

#[cfg(windows)]
fn write_at(file: &File, offset: u64, buf: &[u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut written = 0;
    while written < buf.len() {
        let n = file.seek_write(&buf[written..], offset + written as u64)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "short write",
            ));
        }
        written += n;
    }
    Ok(())
}

#[cfg(windows)]
fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    let mut read = 0;
    while read < buf.len() {
        let n = file.seek_read(&mut buf[read..], offset + read as u64)?;
        if n == 0 {
            break;
        }
        read += n;
    }
    Ok(read)
}

#[cfg(unix)]
fn write_at(file: &File, offset: u64, buf: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

#[cfg(unix)]
fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    let mut read = 0;
    while read < buf.len() {
        let n = file.read_at(&mut buf[read..], offset + read as u64)?;
        if n == 0 {
            break;
        }
        read += n;
    }
    Ok(read)
}

impl PageIo for FilePageIo {
    fn read_page(&self, id: u64, dst: &mut [u8]) -> CoreResult<()> {
        let n = read_at(&self.file, id * self.page_size as u64, dst)?;
        if n != dst.len() {
            return Err(CoreError::new(
                ErrorCode::Corruption,
                format!("page {id}: short read ({n} bytes)"),
            ));
        }
        Ok(())
    }
    fn write_page(&self, id: u64, src: &[u8]) -> CoreResult<()> {
        fault(&self.faults, FaultPoint::BeforePageFlush)?;
        let off = id * self.page_size as u64;
        if self.faults.at(FaultPoint::MidPageWrite) == FaultAction::Crash {
            // torn page: only the first half reaches the file
            write_at(&self.file, off, &src[..src.len() / 2])?;
            return Err(crash_error(FaultPoint::MidPageWrite));
        }
        write_at(&self.file, off, src)?;
        Ok(())
    }
    fn sync_data(&self) -> CoreResult<()> {
        fault(&self.faults, FaultPoint::BeforeFsync)?;
        self.file.sync_data()?;
        Ok(())
    }
    fn page_count(&self) -> CoreResult<u64> {
        Ok(self.file.metadata()?.len() / self.page_size as u64)
    }
}

/// Append-only sequential file with positional reads (journal segments).
pub struct SeqFile {
    file: File,
    len: u64,
    faults: Faults,
}

impl SeqFile {
    pub fn open(path: &Path, faults: Faults) -> CoreResult<SeqFile> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let len = file.metadata()?.len();
        Ok(SeqFile { file, len, faults })
    }
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Append bytes; a MidJournalWrite crash leaves a torn tail on disk.
    pub fn append(&mut self, bytes: &[u8]) -> CoreResult<u64> {
        fault(&self.faults, FaultPoint::BeforeJournalAppend)?;
        let off = self.len;
        if self.faults.at(FaultPoint::MidJournalWrite) == FaultAction::Crash {
            let half = bytes.len() / 2;
            write_at(&self.file, off, &bytes[..half])?;
            self.len += half as u64;
            return Err(crash_error(FaultPoint::MidJournalWrite));
        }
        write_at(&self.file, off, bytes)?;
        self.len += bytes.len() as u64;
        Ok(off)
    }
    pub fn sync(&self) -> CoreResult<()> {
        fault(&self.faults, FaultPoint::BeforeFsync)?;
        self.file.sync_data()?;
        Ok(())
    }
    pub fn read_all(&self) -> CoreResult<Vec<u8>> {
        let mut buf = vec![0u8; self.len as usize];
        let n = read_at(&self.file, 0, &mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }
    /// Truncate a torn tail after recovery decided it is discardable.
    pub fn truncate(&mut self, len: u64) -> CoreResult<()> {
        self.file.set_len(len)?;
        self.file.sync_all()?;
        self.len = len;
        Ok(())
    }
}

/// Write bytes to a whole file atomically with respect to the caller's manifest protocol:
/// write, then fsync (fault points before/after).
pub fn write_file_synced(path: &Path, bytes: &[u8], faults: &Faults) -> CoreResult<()> {
    fault(faults, FaultPoint::BeforeManifestPublish)?;
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    std::io::Write::write_all(&mut f, bytes)?;
    fault(faults, FaultPoint::AfterManifestWriteBeforeFsync)?;
    f.sync_all()?;
    Ok(())
}

/// Sync a directory (no-op on Windows; on Unix required for file creation durability).
pub fn sync_dir(path: &Path) -> CoreResult<()> {
    #[cfg(unix)]
    {
        let d = File::open(path)?;
        d.sync_all()?;
    }
    let _ = path;
    Ok(())
}
