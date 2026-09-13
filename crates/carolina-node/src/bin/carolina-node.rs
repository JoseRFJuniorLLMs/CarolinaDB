//! `carolina-node --config <file>`: run one CarolinaDB node (SPEC-014 MVP-3 profile).

use std::path::PathBuf;
use std::process::ExitCode;

use carolina_node::{run_node, NodeConfig};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = match args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
    {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: carolina-node --config <node.json>");
            return ExitCode::from(2);
        }
    };
    let cfg = match NodeConfig::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return ExitCode::from(2);
        }
    };
    match run_node(cfg) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("node failed: {e}");
            ExitCode::FAILURE
        }
    }
}
