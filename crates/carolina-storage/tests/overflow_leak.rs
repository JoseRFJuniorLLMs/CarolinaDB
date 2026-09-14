//! SPEC-002 §80–§84, §105: a rejected insert must not leave pages behind.
//!
//! `insert_version` writes the overflow chain of a large value *before* it descends and discovers
//! that `(key, seq)` is already present. The chain it wrote is then unreachable from the root, so
//! no copy-on-write checkpoint ever writes those pages (the walk descends from the root) and the
//! buffer pool never evicts them (dirty frames are never evicted). `dirty_count()` therefore never
//! returns to zero again, which is exactly the condition the checkpoint uses to decide that the
//! persisted image is complete — so journal retention silently stops advancing for the life of the
//! store.

use std::sync::Arc;

use carolina_storage::btree::{BTree, TreeCtx};
use carolina_storage::buffer::BufferPool;
use carolina_storage::format::PAGE_SIZE;
use carolina_storage::io::{FaultInjector, FilePageIo, NoFaults};

fn temp_dir(label: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("carolina-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_duplicate_insert_of_a_large_value_leaves_no_page_behind() {
    let dir = temp_dir("overflow-leak");
    let io = FilePageIo::open(&dir.join("data"), PAGE_SIZE, Arc::new(NoFaults)).unwrap();
    let mut pool = BufferPool::new(64);
    let faults: Arc<dyn FaultInjector> = Arc::new(NoFaults);

    let mut ctx = TreeCtx {
        pool: &mut pool,
        io: &io,
        durable_lsn: 0,
        current_lsn: 1,
        faults: &faults,
    };
    let mut tree = BTree::create(&mut ctx, 1).unwrap();

    // a value that does not fit inline, so the insert allocates an overflow chain
    let value = vec![0xABu8; 8 * 1024];
    assert!(tree
        .insert_version(&mut ctx, b"k", 1, [1; 32], 0, &[], &value)
        .unwrap());
    let after_first = tree.next_page_id;

    // the same (key, seq) again: the tree must refuse it, and must not have allocated anything
    assert!(
        !tree
            .insert_version(&mut ctx, b"k", 1, [1; 32], 0, &[], &value)
            .unwrap(),
        "a duplicate (key, seq) must be refused"
    );
    assert_eq!(
        tree.next_page_id, after_first,
        "the refused insert allocated pages for an overflow chain it then abandoned"
    );

    // and the image must still be completable: every dirty page has to be reachable from the root,
    // or the checkpoint cannot write it and journal retention stops advancing forever
    tree.persist(&mut ctx).unwrap();
    assert_eq!(
        ctx.pool.dirty_count(),
        0,
        "the checkpoint left dirty pages behind: they are unreachable from the root, so no \
         checkpoint can ever write them and no eviction can ever reclaim them"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
