# Build and verify CarolinaDB

Run these commands from the repository root. The workspace declares
`rust-version = "1.89"` in `Cargo.toml`, which is the real minimum: the one-writer
database lock uses `std::fs::File::try_lock`, stable since 1.89. `rust-toolchain.toml`
pins local builds to `1.89.0`; CI also tests the current stable compiler on Linux and
Windows. Toolchain upgrades therefore require an explicit source change. Install the normal native linker
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
python tools/run_tlc.py
cargo audit --file Cargo.lock --no-fetch
cargo build -p carolina-node --locked
cargo run -p carolina-cli -- qualify --quick --out target/qualification
```

`cargo audit --no-fetch` uses the locally available RustSec advisory database. CI installs the
pinned cargo-audit 0.22.2 with nightly Rust and refreshes the advisory database on every push and
pull request. The audit toolchain is separate from the project's Rust 1.89 MSRV build.

`tools/run_tlc.py` requires Java 17. It downloads TLA+ tools v1.8.0 only when absent,
verifies the pinned SHA-256 digest, and checks FM-1, FM-2 and FM-3 with their bounded configs.

## Coverage-guided fuzzing

Four `cargo-fuzz` targets cover the DSL frontend, canonical compiler artifacts, wire/snapshot
records and storage formats. Run them with nightly Rust as documented in
[`fuzz/README.md`](../fuzz/README.md). The CI smoke job executes 256 inputs per target; release
qualification uses longer retained campaigns.

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

## Running a cluster by hand

The qualification campaign builds and tears down its own three-process cluster. To keep one
running, write three node configurations and start the binary once per voter. A configuration is
canonical JSON produced by `NodeConfig::encode`; the practical way to obtain one is to construct
`carolina_node::config::NodeConfig` in a small program of your own and write `cfg.encode()` — the
manifest fields are derived identities, not free text, so hand-writing the file is impractical.

The manifest must name exactly three distinct voters, every address must be numeric loopback with a
fixed nonzero port, and `security_profile` must be `DEV_LOCAL`.

```sh
carolina-node --config alpha.json &
carolina-node --config beta.json &
carolina-node --config gamma.json &
carolina node status --addr 127.0.0.1:7301 --cluster <label>
```

`--cluster` takes the **label** the configuration's `cluster_id` was derived from, not the derived
identity; the default is `carolina`. A mismatch closes the connection during negotiation, which is
reported as `connection closed during negotiation`, not as a wrong-cluster error.

The three voters bootstrap themselves through the log (genesis, home grant, request route). Once a
leader reports `Ready`, an admin endpoint can seed rows and a client endpoint can invoke. Only the
hardcoded tenant `tenant-c5` and principal `dev-local-client` are granted: see the tenancy row in
[STATUS.md](STATUS.md).

## Linux and WSL

Linux is a supported build target and CI tests it. Under WSL, build inside the Linux filesystem
(`~/carolinadb`), not under `/mnt/...` — the Windows drive is reached over a translation layer that
makes both compilation and the storage kernel's fsync-heavy tests far slower, and the timing of the
crash matrix is not something to run over it. Copy the tree in and build there:

```sh
tar --exclude=./target --exclude=./.git -cf - . | tar -xf - -C ~/carolinadb
cd ~/carolinadb && cargo test --workspace --locked
```

## Scope

The network profile is DEV_LOCAL: numeric loopback endpoints, three distinct pinned
voters, and exactly the other two voters in each node's peer list. This is plaintext
local development. It has no mTLS or production security qualification. See
[STATUS.md](STATUS.md) and [AUDIT.md](AUDIT.md) for incomplete stages and evidence limits.
