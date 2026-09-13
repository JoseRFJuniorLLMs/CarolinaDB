#![no_main]

use carolina_core::limits::Limits;
use carolina_storage::format::{JournalFrame, Manifest, PageHeader, PAGE_SIZE};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::tiny();
    let _ = JournalFrame::decode(data, limits.max_payload_bytes);
    let _ = Manifest::decode(data);
    if data.len() == PAGE_SIZE {
        let expected_id = u64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);
        let _ = PageHeader::read(data, expected_id);
    }
});
