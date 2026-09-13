# Security Policy

CarolinaDB is a research prototype. It has no production deployment profile, no transport
security implementation yet (SPEC-013 is specified, not implemented) and no security
qualification evidence. Do not expose a CarolinaDB data directory or process to untrusted input
outside an isolated test environment.

## Reporting

Report suspected vulnerabilities privately to the repository owner (see the repository metadata)
rather than in a public issue. Include the commit or working-tree digest from
`carolina qualify` output, the reproduction steps and, when possible, a failure bundle
(`carolina simulate --out <dir>` or a `qualification/<campaign>/<run>/` directory). Bundles contain
only synthetic data; never attach production data or credentials.

## What counts

- Any way to make the engine acknowledge an effect that is not durable, execute a bound request
  twice, or return a receipt that differs from the persisted one.
- Decoder panics or unbounded resource use on malformed canonical JSON, wire frames, journal
  segments, pages or manifests (SPEC-012 §8, SPEC-002 §127).
- Escapes from the allowlisted test root by the qualification runner (SPEC-010 QA-10).

## Scope notes

- Hash domains use the permanent protocol codename `astra.*`; that is not a product name.
- The fault model exercised by the crash campaigns is process kill / short write inside one
  process. OS page-cache loss on power failure is not modelled; do not treat a PASS as evidence
  for it.
- The node's only transport profile is `DEV_LOCAL`: plaintext `ASTR` frames, endpoint roles that
  are unauthenticated declarations, and listeners/peers/clients that refuse any non-loopback
  address. It is a test profile for trusted processes on one host, not a network security
  boundary.
