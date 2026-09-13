//! Three isolated processes and data directories (SPEC-014 §2): single-IDC C5 over a fixed
//! three-voter authority. The campaign body lives in `carolina_node::campaign` and is shared with
//! the qualification runner.

use std::path::PathBuf;

use carolina_node::campaign::three_process_c5;

#[test]
fn three_processes_single_idc_c5_survives_leader_kill() {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_carolina-node"));
    let root = std::env::temp_dir().join(format!("carolina-c5-test-{}", std::process::id()));
    let summary = three_process_c5(&binary, &root).unwrap();
    for c in &summary.checks {
        eprintln!("[c5] {c}");
    }
    assert!(summary.checks.len() >= 6);
}
