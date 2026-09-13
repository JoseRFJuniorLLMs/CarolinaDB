//! Crash campaign at every durable boundary (SPEC-002 §132–§143, SPEC-010 §7).
//! The campaign bodies live in `carolina_storage::campaign` and are shared with `carolina-qualify`.

use carolina_storage::campaign::{
    crash_matrix, io_error_never_yields_success, stale_epoch_is_refused,
};

#[test]
fn crash_at_every_fault_point_preserves_p1_p2_p3_p6_p9() {
    let stats = crash_matrix(4).unwrap();
    eprintln!(
        "crash matrix: {} crashing cases out of {} scheduled",
        stats.exercised, stats.cases
    );
}

#[test]
fn io_error_never_yields_success_test() {
    io_error_never_yields_success().unwrap();
}

#[test]
fn old_storage_epoch_is_refused() {
    stale_epoch_is_refused().unwrap();
}
