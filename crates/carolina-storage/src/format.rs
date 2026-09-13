//! Persistent formats (SPEC-002 §12–§19, §47–§52, §75, §115–§118).
//!
//! Every persistent structure has an explicit, versioned, little-endian, bounds-checked
//! encoding with a CRC32C. No Rust memory image is ever written. CRC32C detects accidental
//! corruption only; it is not tamper evidence.

use carolina_core::error::{CoreError, CoreResult, ErrorCode};

pub const PAGE_SIZE: usize = 8192;
pub const PAGE_FORMAT_VERSION: u16 = 1;
pub const JOURNAL_FORMAT_VERSION: u16 = 1;
pub const MANIFEST_FORMAT_VERSION: u16 = 1;
pub const KEY_CODEC_VERSION: u32 = carolina_core::keycodec::KEY_CODEC_VERSION;

pub const PAGE_MAGIC: [u8; 4] = *b"CDBP";
pub const JOURNAL_MAGIC: [u8; 4] = *b"CDBJ";
pub const MANIFEST_MAGIC: [u8; 4] = *b"CDBM";

/// Page header: 64 bytes at offset 0 of every page.
pub const PAGE_HEADER_LEN: usize = 64;
/// Journal frame header: 64 bytes; trailer: 4 bytes (`total_len`).
pub const FRAME_HEADER_LEN: usize = 64;
pub const FRAME_TRAILER_LEN: usize = 4;
/// Manifest record: fixed 160 bytes.
pub const MANIFEST_LEN: usize = 160;

pub const MAX_VALUE_LEN: usize = 1024 * 1024;
pub const MAX_KEY_LEN: usize = 2048;

pub fn crc(bytes: &[u8]) -> u32 {
    crc32c::crc32c(bytes)
}

fn corrupt(msg: impl Into<String>) -> CoreError {
    CoreError::new(ErrorCode::Corruption, msg)
}

// ---------------------------------------------------------------------------
// Page header
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageType {
    Internal = 1,
    Leaf = 2,
    FreeList = 3,
    Meta = 4,
    Overflow = 5,
}

impl PageType {
    pub fn from_u8(v: u8) -> CoreResult<PageType> {
        Ok(match v {
            1 => PageType::Internal,
            2 => PageType::Leaf,
            3 => PageType::FreeList,
            4 => PageType::Meta,
            5 => PageType::Overflow,
            _ => return Err(corrupt(format!("unknown page type {v}"))),
        })
    }
}

/// Layout (little-endian):
/// `magic[4] format_version[2] page_type[1] flags[1] page_id[8] page_generation[8] page_lsn[8]
///  item_count[2] free_start[2] free_end[2] left_sibling[8] right_sibling[8] reserved[6] crc[4]`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageHeader {
    pub page_type: PageType,
    pub flags: u8,
    pub page_id: u64,
    pub page_generation: u64,
    pub page_lsn: u64,
    pub item_count: u16,
    pub free_start: u16,
    pub free_end: u16,
    pub left_sibling: u64,
    pub right_sibling: u64,
}

pub const NO_PAGE: u64 = 0;

impl PageHeader {
    pub fn new(page_type: PageType, page_id: u64) -> PageHeader {
        PageHeader {
            page_type,
            flags: 0,
            page_id,
            page_generation: 1,
            page_lsn: 0,
            item_count: 0,
            free_start: PAGE_HEADER_LEN as u16,
            free_end: PAGE_SIZE as u16,
            left_sibling: NO_PAGE,
            right_sibling: NO_PAGE,
        }
    }

    /// Write the header into `page` and compute the CRC over the whole page (crc field zeroed).
    pub fn write(&self, page: &mut [u8]) {
        assert_eq!(page.len(), PAGE_SIZE);
        page[0..4].copy_from_slice(&PAGE_MAGIC);
        page[4..6].copy_from_slice(&PAGE_FORMAT_VERSION.to_le_bytes());
        page[6] = self.page_type as u8;
        page[7] = self.flags;
        page[8..16].copy_from_slice(&self.page_id.to_le_bytes());
        page[16..24].copy_from_slice(&self.page_generation.to_le_bytes());
        page[24..32].copy_from_slice(&self.page_lsn.to_le_bytes());
        page[32..34].copy_from_slice(&self.item_count.to_le_bytes());
        page[34..36].copy_from_slice(&self.free_start.to_le_bytes());
        page[36..38].copy_from_slice(&self.free_end.to_le_bytes());
        page[38..46].copy_from_slice(&self.left_sibling.to_le_bytes());
        page[46..54].copy_from_slice(&self.right_sibling.to_le_bytes());
        page[54..60].copy_from_slice(&[0u8; 6]);
        page[60..64].copy_from_slice(&[0u8; 4]);
        let c = crc(page);
        page[60..64].copy_from_slice(&c.to_le_bytes());
    }

    /// Parse and verify a page (magic, version, checksum, bounds).
    pub fn read(page: &[u8], expected_id: u64) -> CoreResult<PageHeader> {
        if page.len() != PAGE_SIZE {
            return Err(corrupt("page length"));
        }
        if page[0..4] != PAGE_MAGIC {
            return Err(corrupt(format!("page {expected_id}: bad magic")));
        }
        let ver = u16::from_le_bytes([page[4], page[5]]);
        if ver != PAGE_FORMAT_VERSION {
            return Err(CoreError::new(
                ErrorCode::UnsupportedFormat,
                format!("page format {ver}"),
            ));
        }
        let stored = u32::from_le_bytes(page[60..64].try_into().unwrap());
        let mut tmp = [0u8; PAGE_SIZE];
        tmp.copy_from_slice(page);
        tmp[60..64].copy_from_slice(&[0u8; 4]);
        if crc(&tmp) != stored {
            return Err(CoreError::new(
                ErrorCode::ChecksumMismatch,
                format!("page {expected_id}: checksum mismatch"),
            ));
        }
        let h = PageHeader {
            page_type: PageType::from_u8(page[6])?,
            flags: page[7],
            page_id: u64::from_le_bytes(page[8..16].try_into().unwrap()),
            page_generation: u64::from_le_bytes(page[16..24].try_into().unwrap()),
            page_lsn: u64::from_le_bytes(page[24..32].try_into().unwrap()),
            item_count: u16::from_le_bytes([page[32], page[33]]),
            free_start: u16::from_le_bytes([page[34], page[35]]),
            free_end: u16::from_le_bytes([page[36], page[37]]),
            left_sibling: u64::from_le_bytes(page[38..46].try_into().unwrap()),
            right_sibling: u64::from_le_bytes(page[46..54].try_into().unwrap()),
        };
        if h.page_id != expected_id {
            return Err(corrupt(format!(
                "page id mismatch: expected {expected_id}, found {}",
                h.page_id
            )));
        }
        if (h.free_start as usize) < PAGE_HEADER_LEN
            || h.free_end as usize > PAGE_SIZE
            || h.free_start > h.free_end
        {
            return Err(corrupt(format!("page {expected_id}: invalid free bounds")));
        }
        Ok(h)
    }
}

// ---------------------------------------------------------------------------
// Journal frame
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum JournalRecordKind {
    CommitBatch = 1,
    PrepareBatch = 2,
    CommitPrepared = 3,
    AbortPrepared = 4,
    CheckpointBegin = 5,
    CheckpointEnd = 6,
    CatalogGeneration = 7,
    StorageEpochChange = 8,
}

impl JournalRecordKind {
    pub fn from_u8(v: u8) -> CoreResult<Self> {
        Ok(match v {
            1 => JournalRecordKind::CommitBatch,
            2 => JournalRecordKind::PrepareBatch,
            3 => JournalRecordKind::CommitPrepared,
            4 => JournalRecordKind::AbortPrepared,
            5 => JournalRecordKind::CheckpointBegin,
            6 => JournalRecordKind::CheckpointEnd,
            7 => JournalRecordKind::CatalogGeneration,
            8 => JournalRecordKind::StorageEpochChange,
            _ => return Err(corrupt(format!("unknown journal record kind {v}"))),
        })
    }
}

/// Frame header layout (64 bytes, LE):
/// `magic[4] format_version[2] record_kind[1] flags[1] total_len[4] payload_len[4] lsn[8] txn_id[32] crc[4] reserved[4]`
/// followed by `payload_len` bytes and a 4-byte trailer `total_len`.
/// CRC covers header (crc field zeroed) + payload + trailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFrame {
    pub kind: JournalRecordKind,
    pub flags: u8,
    pub lsn: u64,
    pub txn_id: [u8; 32],
    pub payload: Vec<u8>,
}

impl JournalFrame {
    pub fn total_len(&self) -> usize {
        FRAME_HEADER_LEN + self.payload.len() + FRAME_TRAILER_LEN
    }

    pub fn encode(&self) -> Vec<u8> {
        let total = self.total_len();
        let mut out = vec![0u8; total];
        out[0..4].copy_from_slice(&JOURNAL_MAGIC);
        out[4..6].copy_from_slice(&JOURNAL_FORMAT_VERSION.to_le_bytes());
        out[6] = self.kind as u8;
        out[7] = self.flags;
        out[8..12].copy_from_slice(&(total as u32).to_le_bytes());
        out[12..16].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out[16..24].copy_from_slice(&self.lsn.to_le_bytes());
        out[24..56].copy_from_slice(&self.txn_id);
        out[FRAME_HEADER_LEN..FRAME_HEADER_LEN + self.payload.len()].copy_from_slice(&self.payload);
        out[total - 4..].copy_from_slice(&(total as u32).to_le_bytes());
        let c = crc(&out);
        out[56..60].copy_from_slice(&c.to_le_bytes());
        out
    }

    /// Decode one frame at the start of `bytes`. Returns `Ok(None)` when the bytes are an incomplete
    /// (possibly torn) tail; `Err(Corruption)` when a complete frame fails validation.
    pub fn decode(bytes: &[u8], max_payload: usize) -> CoreResult<Option<(JournalFrame, usize)>> {
        if bytes.len() < FRAME_HEADER_LEN {
            return Ok(None);
        }
        if bytes[0..4] != JOURNAL_MAGIC {
            // an all-zero header is unwritten space (tail); anything else is corruption
            if bytes[..FRAME_HEADER_LEN].iter().all(|b| *b == 0) {
                return Ok(None);
            }
            return Err(corrupt("journal frame: bad magic"));
        }
        let ver = u16::from_le_bytes([bytes[4], bytes[5]]);
        if ver != JOURNAL_FORMAT_VERSION {
            return Err(CoreError::new(
                ErrorCode::UnsupportedFormat,
                format!("journal format {ver}"),
            ));
        }
        let total = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let payload_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        if payload_len > max_payload || total != FRAME_HEADER_LEN + payload_len + FRAME_TRAILER_LEN
        {
            return Err(corrupt("journal frame: inconsistent lengths"));
        }
        if bytes.len() < total {
            return Ok(None);
        }
        let frame_bytes = &bytes[..total];
        let stored = u32::from_le_bytes(frame_bytes[56..60].try_into().unwrap());
        let mut tmp = frame_bytes.to_vec();
        tmp[56..60].copy_from_slice(&[0u8; 4]);
        if crc(&tmp) != stored {
            return Err(CoreError::new(
                ErrorCode::ChecksumMismatch,
                "journal frame checksum mismatch",
            ));
        }
        let trailer = u32::from_le_bytes(frame_bytes[total - 4..].try_into().unwrap()) as usize;
        if trailer != total {
            return Err(corrupt("journal frame: trailer mismatch"));
        }
        let kind = JournalRecordKind::from_u8(frame_bytes[6])?;
        let mut txn_id = [0u8; 32];
        txn_id.copy_from_slice(&frame_bytes[24..56]);
        Ok(Some((
            JournalFrame {
                kind,
                flags: frame_bytes[7],
                lsn: u64::from_le_bytes(frame_bytes[16..24].try_into().unwrap()),
                txn_id,
                payload: frame_bytes[FRAME_HEADER_LEN..FRAME_HEADER_LEN + payload_len].to_vec(),
            },
            total,
        )))
    }
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// Layout (160 bytes, LE):
/// `magic[4] format_version[2] page_format[2] journal_format[2] reserved[2] key_codec[4]
///  manifest_generation[8] storage_epoch[8] page_size[4] reserved[4] root_page_id[8] next_page_id[8]
///  free_list_head[8] checkpoint_lsn[8] checkpoint_commit_seq[8] journal_segment[8]
///  storage_id[16] cluster_id[16] reserved[32] crc[4]`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manifest {
    pub manifest_generation: u64,
    pub storage_epoch: u64,
    pub root_page_id: u64,
    pub next_page_id: u64,
    pub free_list_head: u64,
    pub checkpoint_lsn: u64,
    pub checkpoint_commit_seq: u64,
    pub journal_segment: u64,
    pub storage_id: [u8; 16],
    pub cluster_id: [u8; 16],
}

impl Manifest {
    pub fn encode(&self) -> [u8; MANIFEST_LEN] {
        let mut m = [0u8; MANIFEST_LEN];
        m[0..4].copy_from_slice(&MANIFEST_MAGIC);
        m[4..6].copy_from_slice(&MANIFEST_FORMAT_VERSION.to_le_bytes());
        m[6..8].copy_from_slice(&PAGE_FORMAT_VERSION.to_le_bytes());
        m[8..10].copy_from_slice(&JOURNAL_FORMAT_VERSION.to_le_bytes());
        m[12..16].copy_from_slice(&KEY_CODEC_VERSION.to_le_bytes());
        m[16..24].copy_from_slice(&self.manifest_generation.to_le_bytes());
        m[24..32].copy_from_slice(&self.storage_epoch.to_le_bytes());
        m[32..36].copy_from_slice(&(PAGE_SIZE as u32).to_le_bytes());
        m[40..48].copy_from_slice(&self.root_page_id.to_le_bytes());
        m[48..56].copy_from_slice(&self.next_page_id.to_le_bytes());
        m[56..64].copy_from_slice(&self.free_list_head.to_le_bytes());
        m[64..72].copy_from_slice(&self.checkpoint_lsn.to_le_bytes());
        m[72..80].copy_from_slice(&self.checkpoint_commit_seq.to_le_bytes());
        m[80..88].copy_from_slice(&self.journal_segment.to_le_bytes());
        m[88..104].copy_from_slice(&self.storage_id);
        m[104..120].copy_from_slice(&self.cluster_id);
        let c = crc(&m[..MANIFEST_LEN - 4]);
        m[MANIFEST_LEN - 4..].copy_from_slice(&c.to_le_bytes());
        m
    }

    pub fn decode(m: &[u8]) -> CoreResult<Manifest> {
        if m.len() < MANIFEST_LEN {
            return Err(CoreError::new(
                ErrorCode::InvalidManifest,
                "manifest too short",
            ));
        }
        let m = &m[..MANIFEST_LEN];
        if m[0..4] != MANIFEST_MAGIC {
            return Err(CoreError::new(
                ErrorCode::InvalidManifest,
                "bad manifest magic",
            ));
        }
        let stored = u32::from_le_bytes(m[MANIFEST_LEN - 4..].try_into().unwrap());
        if crc(&m[..MANIFEST_LEN - 4]) != stored {
            return Err(CoreError::new(
                ErrorCode::InvalidManifest,
                "manifest checksum mismatch",
            ));
        }
        let mf = u16::from_le_bytes([m[4], m[5]]);
        let pf = u16::from_le_bytes([m[6], m[7]]);
        let jf = u16::from_le_bytes([m[8], m[9]]);
        let kc = u32::from_le_bytes(m[12..16].try_into().unwrap());
        if mf != MANIFEST_FORMAT_VERSION
            || pf != PAGE_FORMAT_VERSION
            || jf != JOURNAL_FORMAT_VERSION
            || kc != KEY_CODEC_VERSION
        {
            return Err(CoreError::new(
                ErrorCode::UnsupportedFormat,
                format!("format versions manifest={mf} page={pf} journal={jf} key_codec={kc}"),
            ));
        }
        let page_size = u32::from_le_bytes(m[32..36].try_into().unwrap()) as usize;
        if page_size != PAGE_SIZE {
            return Err(CoreError::new(
                ErrorCode::UnsupportedFormat,
                format!("page size {page_size} != {PAGE_SIZE}"),
            ));
        }
        let mut storage_id = [0u8; 16];
        storage_id.copy_from_slice(&m[88..104]);
        let mut cluster_id = [0u8; 16];
        cluster_id.copy_from_slice(&m[104..120]);
        Ok(Manifest {
            manifest_generation: u64::from_le_bytes(m[16..24].try_into().unwrap()),
            storage_epoch: u64::from_le_bytes(m[24..32].try_into().unwrap()),
            root_page_id: u64::from_le_bytes(m[40..48].try_into().unwrap()),
            next_page_id: u64::from_le_bytes(m[48..56].try_into().unwrap()),
            free_list_head: u64::from_le_bytes(m[56..64].try_into().unwrap()),
            checkpoint_lsn: u64::from_le_bytes(m[64..72].try_into().unwrap()),
            checkpoint_commit_seq: u64::from_le_bytes(m[72..80].try_into().unwrap()),
            journal_segment: u64::from_le_bytes(m[80..88].try_into().unwrap()),
            storage_id,
            cluster_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_header_roundtrip_and_checksum() {
        let mut page = vec![0u8; PAGE_SIZE];
        let mut h = PageHeader::new(PageType::Leaf, 7);
        h.page_lsn = 99;
        h.right_sibling = 8;
        h.write(&mut page);
        let back = PageHeader::read(&page, 7).unwrap();
        assert_eq!(back, h);
        page[100] ^= 1;
        assert_eq!(
            PageHeader::read(&page, 7).unwrap_err().code,
            ErrorCode::ChecksumMismatch
        );
        page[100] ^= 1;
        assert!(PageHeader::read(&page, 8).is_err());
        page[4] = 9;
        assert!(PageHeader::read(&page, 7).is_err());
    }

    #[test]
    fn journal_frame_roundtrip_tail_and_corruption() {
        let f = JournalFrame {
            kind: JournalRecordKind::CommitBatch,
            flags: 0,
            lsn: 5,
            txn_id: [1u8; 32],
            payload: b"hello".to_vec(),
        };
        let bytes = f.encode();
        let (back, n) = JournalFrame::decode(&bytes, 1 << 20).unwrap().unwrap();
        assert_eq!(back, f);
        assert_eq!(n, bytes.len());
        // torn tail: incomplete bytes are not corruption
        assert!(JournalFrame::decode(&bytes[..bytes.len() - 1], 1 << 20)
            .unwrap()
            .is_none());
        // zero-filled tail
        assert!(JournalFrame::decode(&[0u8; 128], 1 << 20)
            .unwrap()
            .is_none());
        // mid corruption is a typed failure
        let mut bad = bytes.clone();
        bad[FRAME_HEADER_LEN] ^= 0xff;
        assert_eq!(
            JournalFrame::decode(&bad, 1 << 20).unwrap_err().code,
            ErrorCode::ChecksumMismatch
        );
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert_eq!(
            JournalFrame::decode(&bad, 1 << 20).unwrap_err().code,
            ErrorCode::Corruption
        );
        // oversized declared payload is rejected before allocation
        let mut bad = bytes.clone();
        bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(JournalFrame::decode(&bad, 1 << 20).is_err());
    }

    #[test]
    fn manifest_roundtrip_and_version_rejection() {
        let m = Manifest {
            manifest_generation: 3,
            storage_epoch: 1,
            root_page_id: 1,
            next_page_id: 9,
            free_list_head: 0,
            checkpoint_lsn: 100,
            checkpoint_commit_seq: 40,
            journal_segment: 1,
            storage_id: [2u8; 16],
            cluster_id: [3u8; 16],
        };
        let bytes = m.encode();
        assert_eq!(Manifest::decode(&bytes).unwrap(), m);
        let mut bad = bytes;
        bad[20] ^= 1;
        assert_eq!(
            Manifest::decode(&bad).unwrap_err().code,
            ErrorCode::InvalidManifest
        );
        let mut bad = m.encode();
        bad[6] = 2;
        let c = crc(&bad[..MANIFEST_LEN - 4]);
        bad[MANIFEST_LEN - 4..].copy_from_slice(&c.to_le_bytes());
        assert_eq!(
            Manifest::decode(&bad).unwrap_err().code,
            ErrorCode::UnsupportedFormat
        );
        // §12: a manifest recorded with another page size fails open (valid CRC, wrong size)
        let mut other = m.encode();
        other[32..36].copy_from_slice(&((PAGE_SIZE as u32) * 2).to_le_bytes());
        let c = crc(&other[..MANIFEST_LEN - 4]);
        other[MANIFEST_LEN - 4..].copy_from_slice(&c.to_le_bytes());
        let err = Manifest::decode(&other).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnsupportedFormat);
        assert!(err.message.contains("page size"));
    }
}
