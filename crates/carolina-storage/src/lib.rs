//! CarolinaDB storage kernel (SPEC-002).
//!
//! * [`format`] — persistent formats (pages, journal frames, manifest)
//! * [`io`] — positional file I/O and fault injection points
//! * [`buffer`] — explicit buffer pool (CLOCK)
//! * [`btree`] — page-oriented B+Tree with physical MVCC order, copy-on-write persistence
//! * [`journal`] — segmented redo-first Commit Journal
//! * [`batch`] — `CompiledBatch`, `ProtocolMutation`, `TxnStatusRecord`
//! * [`kernel`] — the native `Store` implementing [`kernel::DurableStorageKernel`]
//! * [`memkernel`] — reference in-memory kernel with simulated durability
//! * [`campaign`] — callable P1–P10 qualification campaigns shared by tests and `carolina-qualify`

pub mod batch;
pub mod btree;
pub mod buffer;
pub mod campaign;
pub mod format;
pub mod io;
pub mod journal;
pub mod kernel;
pub mod memkernel;
pub mod testutil;

pub use batch::*;
pub use kernel::{DurableStorageKernel, LocalSnapshot, Store, StoreOptions};
