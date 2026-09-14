# Changelog

## 0.1.0 — 2026-09-14

The first vertical slice of [SPEC-014](md/SPEC-014.md): the semantic core, the conservative
compiler, the local durable slice, and a single-IDC C5 slice running as three real processes.

**What this release is.** A research prototype. It compiles a declarative operation contract into a
plan, executes it against a durable storage kernel with MVCC and a redo-first journal, replicates
the ordered execution across three voters, and returns a receipt whose bytes are identical on every
retry of the same request identity.

**What this release is not.** It has no transport security. The node speaks only the plaintext,
loopback-only `DEV_LOCAL` profile and an endpoint declares its own role, so there is no
authentication of any kind. Exactly one tenant and one principal are granted, both fixed in code.
Three processes on one machine demonstrate neither machine nor region fault tolerance. The
qualification gate `QI-SECURITY` is `NOT_RUN`, so the SPEC-014 §3 exit criteria of MVP-3 are **not**
met even though `Q3-C5` passes. MVP-4 through MVP-8 are not started.

### Verification

- Gates `Q0`, `Q1`, `Q2`, `FM` and `Q3-C5` PASS; `QI` is `NOT_RUN` on `QI-SECURITY`; `Q3`, `Q4`,
  `Q5`, `Q6`, `Q7` are `NOT_RUN` for capabilities that do not exist.
- `QI-AUTHZ` PASS: the non-cryptographic authorization state machine (SEC-02, SEC-09, SEC-14).
- FM-1 is **proved unbounded** in Lean 4 (`lean/Carolina/Escrow.lean`), where TLC only checks
  `Total = 3, Quantities = <<1, 2>>`. Axioms audited; no `sorry`; mathlib is not a dependency.
- The release build is **reproducible**: `tools/build_release.py --verify` builds the committed tree
  twice from differently named directories and compares the artifacts.
- `sbom.json` (CycloneDX 1.5) is generated from `Cargo.lock` and checked in CI. The dependency
  closure is 13 third-party crates, every licence determined, all MIT/Apache-2.0.

### Defects found and fixed while qualifying this slice

- **A cold restart of the whole cluster bricked it permanently.** A leader elected out of a cold
  start bootstrapped against a catalog it had not replayed, proposed a second genesis, and every
  voter failed closed on the durable entry. Found by restarting a live cluster; the campaign only
  ever restarted one voter, which a live majority carries.
- **A refused insert silently disabled journal retention.** The overflow chain of a large value was
  written before the duplicate `(key, seq)` was discovered, leaving pages unreachable from the root
  that no checkpoint writes and no eviction reclaims. Reachable through ordinary recovery.
- **`carolina node status` could never run**: option validation treated its subcommand as an unknown
  option.
- **Snapshot chunk ingest** enforced neither the declared size bound nor the record registry.
- **The checkpoint image walk** skipped dirty pages whose parent had been evicted.
- **`GROUP BY <key> <= <bound>`** could not be parsed at all.
- The released `carolina` binary embedded the build machine's absolute path.
- Running the campaign from an extracted release reported `overall FAIL` with "golden bytes
  missing" instead of saying that a binary release does not ship the repository's `fixtures/`.

### Not yet done

Signed releases and an architecture freeze. `ARCHITECTURE-FREEZE-0.1.md` is blocked on two owner
decisions: the `CodecManifest` is incomplete (15 registered kinds have no implemented codec), and
the security profile is unresolved because SPEC-013 needs a TLS dependency that has not been chosen.
Signing needs a key. See [md/FALTA.md](md/FALTA.md) for the full list.
