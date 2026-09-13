# Build and verify CarolinaDB

Run these commands from the repository root. The workspace targets Rust 1.85 or
newer and uses the installed `stable` toolchain. Install the normal native linker
for Rust on your platform (MSVC Build Tools on Windows).

## Build

```sh
cargo build --workspace --locked
cargo build --workspace --release --locked
```

The binaries are `carolina` and `carolina-node` in the Cargo target directory.
On Windows they have the `.exe` suffix. If a global Cargo target directory is
unwritable, use a directory inside this checkout:

```powershell
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'target'
cargo build --workspace --locked
```

`--offline` may be added when every locked dependency is already cached.

## Local workload and compiler

Use a fresh scratch directory for the demonstration; its rows and request identities
are retained on subsequent runs.

```sh
cargo run -p carolina-cli -- compile fixtures/dsl/inventory_reserve_release.cdl --out target/plans-demo
cargo run -p carolina-cli -- check target/plans-demo
cargo run -p carolina-cli -- explain fixtures/dsl/inventory_reserve_release.cdl
cargo run -p carolina-cli -- workload target/data-demo
cargo run -p carolina-cli -- verify target/data-demo
```

## Validation

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python tools/test_spec_lint.py
python tools/spec_lint.py
cargo build -p carolina-node --locked
cargo run -p carolina-cli -- qualify --quick --out target/qualification
```

Omit `--quick` for the standard campaign budgets. `--keep-bundles` retains individual
passing schedule histories as well as the campaign report. Inspect the actual
directory printed as `bundle:` with:

```sh
carolina report --campaign <campaign-bundle-directory>
carolina replay --bundle <individual-schedule-bundle-directory>
```

Qualification exit codes are 0 for the claimed gates passing, 1 for failure, 2 for
invalid configuration and 3 for missing or inconclusive claimed evidence. Gates for
disabled capabilities remain NOT_RUN even when the enabled slice passes.

The C5 process campaign finds `carolina-node` next to the runner or through the
`CAROLINA_NODE_BIN` environment variable. Build both binaries from the same source
tree and profile. The campaign launches three loopback processes in isolated scratch
directories, kills/restarts those processes and verifies retained request results.

## Scope

The network profile is DEV_LOCAL: numeric loopback endpoints, three distinct pinned
voters, and exactly the other two voters in each node's peer list. This is plaintext
local development. It has no mTLS or production security qualification. See
[STATUS.md](STATUS.md) and [AUDIT.md](AUDIT.md) for incomplete stages and evidence limits.
