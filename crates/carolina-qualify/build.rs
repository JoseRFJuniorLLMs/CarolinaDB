//! Records the compiler that built the qualification runner (SPEC-010 §2 `toolchain_id`).
use std::process::Command;

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = Command::new(&rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "rustc (version unavailable at build time)".into());
    println!("cargo:rustc-env=CAROLINA_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-env-changed=RUSTC");
}
