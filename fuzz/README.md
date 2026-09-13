# CarolinaDB fuzz targets

These coverage-guided targets exercise the bounded parser and decoder boundaries required by
SPEC-002, SPEC-003, SPEC-010 and SPEC-012. Run them with nightly Rust and `cargo-fuzz`:

```sh
cargo fuzz run dsl_frontend -- -max_total_time=60
cargo fuzz run canonical_artifacts -- -max_total_time=60
cargo fuzz run wire_protocol -- -max_total_time=60
cargo fuzz run storage_formats -- -max_total_time=60
```

Crashes and timeouts are failures. Corpus and generated artifacts stay outside source control.
The short CI smoke run uses cargo-fuzz 0.13.2 and checks that every harness builds and executes;
release qualification uses longer retained campaigns and records the exact toolchain, corpus
digest and run budget.
