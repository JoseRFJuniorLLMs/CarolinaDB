# SPEC-013 — Security, Authentication & Trust Model

**Status:** Draft 0.1; implementation and security qualification remain open.
**Implementation status (2026-09-14):** Partially implemented. The non-cryptographic authorization state machine is implemented and qualified by `QI-AUTHZ`: immutable `PermissionGrant` records enforce exact principal/tenant/request-namespace/operation access, administrative catalog mutations require the manifest bootstrap administrator, resolve rechecks current permission, revocation by tombstone stops new disclosure, and the admitting principal is replicated in the ordered `Admit` entry. The node still speaks only the plaintext, loopback-only `DEV_LOCAL` profile, whose endpoint role maps to a fixture principal without authentication. TLS 1.3/mTLS, credential/X.509 records, portable signatures, durable audit, encrypted backup, key rotation and cryptographic revocation remain absent; therefore `QI-SECURITY` and the aggregate QI gate remain `NOT_RUN`. SEC-02, SEC-09 and SEC-14 have executable evidence; the other 13 SEC rows remain open. Table: [docs/AUDIT.md](../docs/AUDIT.md).
**Date:** 2026-09-09
**Depends on:** [SPEC-011](SPEC-011.md) for identities, catalog policy and fencing; [SPEC-012](SPEC-012.md) for client identity, canonical payloads and compatibility.
**Applies to:** the storage, replication, escrow, certification, serial and migration boundaries of SPEC-002 and SPEC-005–009.

## 1. Threat model and limits

The baseline protects authenticated client/node communication, tenant authorization boundaries, protocol evidence against unauthenticated forgery, and confidential stored/backup bytes under the selected security profile. An attacker may send arbitrary malformed inputs, retry/replay captured messages, impersonate unregistered endpoints, control network delivery, possess another tenant's credentials, or obtain an encrypted offline disk/backup without its keys.

Runtime consensus and rights-preservation claims assume trusted, non-Byzantine admitted members. A malicious or compromised consensus member is not a crash-fault member. Signatures, mTLS, checksums and content hashes do not convert Raft, escrow or SPEC-008 certificates into Byzantine consensus. Compromise of a host holding plaintext, a trusted authority implementation, the cluster CA, an authorized administrator or backup decryption keys exceeds the corresponding confidentiality/integrity boundary. It triggers containment and evidence recovery, not an assertion that the remaining protocol automatically tolerates that adversary.

Protection against traffic analysis, all denial of service, malicious cloud/hypervisor administrators, side channels, SQL-compatible row-level policy, externally caused payment duplication, and cryptographic proof of invariant correctness are outside v1. Resource limits and authorization are still required. No deployed security guarantee may be claimed solely from this draft.

## 2. Principals, trust roots and security records

The immutable `ClusterId` from the bootstrap manifest is the trust namespace. A certificate valid in cluster A is not valid in cluster B. Tenant labels, DNS names, network addresses and copied database files cannot change that identity.

```text
Principal = Client(PrincipalId) | Node(NodeId) | Administrator(PrincipalId)
CredentialRecord {
  credential_id; principal; cluster_id
  public_key_fingerprint; certificate_issuer_and_serial
  allowed_purposes[]; state: STAGED | ACTIVE | RETIRED | REVOKED
  security_policy_id; activation_catalog_generation
  historical_verification_pins[]
}
SecurityPolicy {
  policy_id; immutable_policy_hash
  trusted_ca_refs[]; allowed_credential_profiles[]
  principal_grants[]; node_protocol_roles[]
  token_signer_keys[]; minimum_protocol_and_security_profiles
  admission_mode: ONLINE_BARRIER | PINNED_OFFLINE
  validity_clock_policy; limits; audit_policy
}
PermissionGrant {
  principal; tenant_scope
  action; exact_resource_scope; allowed_operation_versions
  request_namespace_scope; delegation: DISALLOWED
}
```

The catalog retains immutable policy versions, active-policy pointers and pending closure status. Cryptographic private keys MUST NOT be stored in catalog artifacts or replicated business metadata. Keys are referenced by `KeyId` in an OS-protected secret store or external key service; generated/imported using a maintained cryptographic library or provider. Separate keys serve TLS identity, portable-token signing and backup decryption.

Trusted CA enrollment alone does not grant a node a protocol role or a client access to a tenant. The certificate's identity and registered public key must match an admitted credential record. Principals cannot delegate authority merely by embedding another identity in a request.

## 3. TLS and mTLS profile

The v1 network profile requires TLS 1.3 for client, peer and administrative connections, with mutual certificate authentication. Plaintext fallback, opportunistic TLS, ignored certificate failures and TLS early application data (0-RTT) are disabled. Use a maintained TLS implementation, its validated TLS 1.3 cipher suites and release-pinned configuration. The TLS guidance emphasizes secure configuration and the application replay risks of early data. [RFC 9325](https://www.rfc-editor.org/rfc/rfc9325)

Peer identities have exactly one registered URI SAN of the form `astra://cluster/<ClusterId>/node/<NodeId>`; client/admin credentials use `.../principal/<PrincipalId>` and an explicitly registered purpose. IDs use the canonical encoding fixed by SPEC-012. DNS-based service endpoints additionally validate the configured service identity through the TLS library. A shared friendly common name, source IP or HTTP identity header is insufficient.

The verifier checks the configured trust path, signature, validity interval, key usage, extended key usage for client/server roles, expected identity and the catalog credential binding. Certificate parsing/path validation uses the library's X.509 implementation; no custom ASN.1 or path builder is introduced. The certificate profile is based on the standard X.509 certificate and CRL model. [RFC 5280](https://www.rfc-editor.org/rfc/rfc5280)

The deployment's validity-clock policy sets time source, tolerated skew and uncertain-clock behavior. A node unable to establish acceptable certificate validity fails new authentication closed. This clock assumption concerns credential availability; certificate expiry never returns escrow rights, aborts prepared work, or proves that an offline writer stopped.

Network roles are distinct listeners or explicitly authenticated application roles. A client connection cannot submit consensus, grant, snapshot-install or migration commands. TLS termination at an untrusted proxy is unsupported. A trusted gateway must authenticate upstream nodes and carry a verifiable original-principal binding; v1 may require end-to-end client mTLS instead of supporting delegation.

TLS resumption rechecks the credential and active/pinned policy at application admission. Session tickets cannot prolong a revoked identity's application permissions. Close affected live channels when local revocation becomes effective; transport connection lifetime is not authorization lifetime.

## 4. Client and tenant authorization

Every invocation, status query, resolve request, read and administrative command authenticates a principal. The requested `TenantId` is checked against the credential's permission grants before catalog/data lookup or identity creation. The tenant field is never trusted merely because it is signed by the client.

Business permissions identify exact `OperationId`, allowed versions, request namespace and scope. Read/query permissions identify their declared semantic scope and read contract. Client-selected arguments, operation name, requested durability and session tokens cannot expand a grant. Arbitrary storage/protocol mutation APIs are internal-only. Cross-tenant transactions and session frontiers are unsupported in v1 unless a separately specified operation explicitly owns every tenant boundary.

`RequestNamespace` has a registered owner principal or tenant application role. Only that owner or an explicitly granted resolver may bind/resolve its request keys. `ResolveRequest` requires current authorization to that namespace/outcome; knowledge of a `TxnId`, request hash or receipt digest is not an access token. A changed principal cannot reveal another principal's result by guessing an ID. Authorization errors need not reveal whether the hidden identity exists.

Canonical `request_hash` binds the request semantics specified by SPEC-012, including tenant and namespace. Credentials and mutable routing are not added to that semantic hash merely because they rotate. The durable request binding separately records the admitting principal and authorization-policy evidence. Credential rotation for the same principal preserves retries; explicit namespace access policy governs principal replacement.

All internal keys, secondary indices, deduplication rows, cache entries, snapshots, result stores and diagnostics retain tenant scope. A tenant filter on the gateway alone is insufficient. At execution, the runtime rechecks resolved resource ownership under the same protected admission boundary used for business/protocol effects.

## 5. Administrative actions and policy activation

Administration has separate named actions: `RegisterArtifact`, `PublishPlan`, `StartMigration`, `ResumeMigration`, `EnrollNode`, `ManageCredentials`, `ManagePermissions`, `ManageBackup`, `RestorePassive`, `InspectProtocolState`, and `ManageRetention`. An application operation permission grants none of these. `ResumeMigration` only resumes its recorded state machine; it does not grant forced activation or heuristic commit/abort.

Mutating administrative requests have stable `AdminRequestId`, immutable command hash, authenticated actor and an expected policy/catalog revision. SPEC-011's CAS rechecks authorization with the mutation and durable audit event. Repeated IDs preserve the original result; changed terms fail. Read-only inspection is still tenant/scope-authorized and redacted.

A policy change is `PROPOSED -> INSTALLING/CLOSING -> EFFECTIVE -> RETIRED`. A less restrictive grant requires normal catalog authorization and installation. A restrictive change that affects permitted offline admission follows SPEC-009/011 closure over every affected issuer; it is reported `PENDING_CLOSURE` until all relevant durable barriers exist. An old authority cannot issue a fresh delegated grant while closing.

`ONLINE_BARRIER` admission checks a current catalog policy barrier. `PINNED_OFFLINE` admission uses its installed immutable policy and grant; restrictive updates cannot be reported globally effective while an allowed offline issuer remains unclosed. Operators may cause connected nodes to deny a credential sooner, but this partial containment is not proof of semantic revocation elsewhere. The plan and EXPLAIN report this availability/revocation tradeoff.

The race with a restrictive update is serialized: a request admitted before the local durable policy fence carries old-policy evidence and is included in drain; one arriving after the fence is denied. Denying a subsequent retry does not undo its original committed outcome. Protocol recovery of already admitted work remains possible through authorized node roles even if the initiating client loses access.

## 6. Authority evidence and historical records

`AuthorityEvidence` identifies `ClusterId`, the exact tagged `AuthorityBinding` or SPEC-008 participant descriptor, grant/plan hashes, committed record reference, canonical record digest and evidence purpose. The recipient verifies membership and installed fence rules under SPEC-011 and checks the committed record through an authenticated authoritative lookup or the trusted consensus adapter's committed-state interface.

A certificate blob or digest arriving from an arbitrary data replica is insufficient. A valid mTLS peer is authenticated as that node, not as every authority. For example, a passive escrow replica cannot issue a donor commit; a C1 relay cannot issue a C5 publication certificate; a catalog voter cannot fabricate a holder's durable close acknowledgement.

Authority evidence is checked at each protocol boundary that creates usable rights, votes yes, records a decision, publishes data, activates a migration or enables admission. The authenticated identity must match the permitted role and exact resource/IDC/home bindings. Terms, request/transaction identity, membership, participant set and decision state must match the persisted record. C4/C5 participants bind their full `ParticipantDescriptor` hash, not an untyped epoch.

Retired authority may still authenticate historical committed records if those records match retained authoritative history. Historical verification is a separate operation from new admission: it can install a previously decided effect or resolve an existing request idempotently, never mint a new grant. If a signing credential is compromised, its signature alone cannot establish that a presented record predates revocation; require a committed catalog/authority reference already anchored in retained history. Missing evidence returns unavailable/integrity failure.

A suspected malicious authority moves affected scopes to containment and reconciliation. Revoking network credentials can stop accepted future traffic at informed peers, but does not prove that lost or maliciously hidden rights/decisions are absent. Automated replacement using their presumed absence is forbidden.

## 7. Session tokens and portable integrity envelopes

SPEC-012 owns canonical session, receipt and protocol-record payload bytes. This specification owns an external integrity wrapper; signatures and mutable key identifiers are not recursively included in the payload's own semantic digest. Persisted `FinalReceipt` payload bytes remain immutable across retries and key rotation. A transport may attach a fresh verification wrapper around those same bytes without changing the result or request identity.

The v1 portable wrapper uses the standard `COSE_Sign1` structure with an attached byte-string payload, Ed25519 signature and a pinned per-purpose signer key. The protected header contains algorithm, key ID and content type; unprotected headers are empty. Receivers allow only the configured algorithm/key/profile and reject duplicate headers, unsupported critical semantics and ambiguous encodings. Implement COSE using a maintained library, including its defined signature structure. [RFC 9052](https://www.rfc-editor.org/rfc/rfc9052), [RFC 9053](https://www.rfc-editor.org/rfc/rfc9053), [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032)

```text
SignedPayloadV1 {                   // canonical SPEC-012 bytes inside COSE
  envelope_version
  cluster_id; tenant_id
  purpose: SESSION | RECEIPT | BACKUP_MANIFEST
  issuer_authority_binding
  subject_scope: PrincipalOrNamespaceScope
  payload_format_id
  payload_bytes                     // exact canonical semantic payload
}
```

The wrapper's content type is the fixed profile string `application/astra.signed-payload.v1`; COSE external AAD is the empty byte string. Cluster, tenant, purpose, issuer and subject are in the signed payload, so the same bytes cannot be substituted across clusters, tenants or token purposes. Algorithm/key mapping, canonical wrapper encoding, maximum length and positive/negative vectors are frozen with SPEC-012's compatibility corpus before interoperability is claimed. A signer key has exactly its cataloged allowed purposes; a session-signing key is not a node/grant credential.

Tokens are observation evidence, not bearer authorization to spend rights or inspect results. Verification authenticates the request separately and checks token subject, tenant, scope, exact schema/contract/frontier and issuer against retained policy. C2 v1 tokens bind one replication group and membership generation. Combining unrelated group tokens does not create a supported global causal session.

An expired/unverifiable token produces a typed error or an authenticated refresh against retained evidence. The client/server MUST NOT silently drop it and claim the originally requested read guarantee. Rotation retains verification keys for every still-supported token or supplies a verified translation/refresh route. A token signature establishes origin/integrity under the trust model; it does not prove the issuer executed the contract correctly.

## 8. Replay, downgrade and parser boundaries

TLS authenticates transport; stable application identity supplies replay semantics. Retrying identical `RequestKey`/`request_hash` resolves the same mapping and outcome. A changed hash fails before effects. Internal phase replay carries the original transaction/transfer/migration identity and immutable terms; unexpected transitions cannot create a second decision or grant. Retired identity tombstones and namespace floors survive GC.

Every connection negotiates the SPEC-012 protocol version, required features and minimum security profile inside the authenticated channel. The selected capabilities are checked against the installed plan and catalog node manifest. Unsupported mandatory semantics abort the exchange. A connection failure cannot trigger automatic plaintext, older semantic codec, weaker read contract or unauthenticated retry. Security-profile downgrades require a separately authorized catalog transition and cannot bypass pinned plan requirements.

Bounds apply before allocation/decompression and before signature-heavy work where safe: frame length, nesting, collection counts, token/context size, outstanding requests, per-principal/tenant budgets and handshake concurrency. Cryptographic verification precedes trusting payload-derived authority. Parsers reject duplicate semantic fields and noncanonical identity encodings; no language-specific default inserts missing required authority fields.

## 9. Credentials, rotation and revocation

Enrollment registers a fresh key fingerprint and verified principal identity under a catalog-authorized credential command. For normal rotation, stage the replacement key, distribute its public verification material, verify required targets installed it, activate new issuance/use, then retire the old key after retained token/certificate and history obligations are satisfied. Key IDs are never reassigned to different keys.

CA rotation stages new trust alongside old trust under an authenticated old-root/catalog transition, then reissues credentials and closes/removes old trust according to the policy's offline scope. A node missing the new roots remains unavailable until re-enrolled through an authorized recovery path; it never disables certificate validation to reconnect. Bootstrap cannot be silently reopened.

Emergency revocation persists credential status, affected scopes, policy revision and audit evidence. Connected peers apply denial and close channels; outstanding prepared/decided work is still resolved from its original durable authorities. Global revocation completion follows the closure model in §5. Expiry, CRLs, catalog denial and key deletion are containment mechanisms, not rights reclamation algorithms.

Private-key retirement and public verification retention are independent. Historical public keys/certificate chains may remain pinned after issuance stops; private keys can be removed once no required signer/decryptor function remains. Backup decryption keys are retained until every dependent backup is reencrypted or expires under an approved retention policy. Lost keys cause explicit unrecoverability, not an unencrypted fallback.

## 10. Secrets, at-rest and backup profiles

The release manifest selects one explicit profile; health/diagnostics report it without revealing key material. `DEV_LOCAL` is permitted only for isolated local development and synthetic fixtures, with nonproduction identities. It is not a confidential deployment claim and cannot be negotiated as a fallback by a production client.

`ENCRYPTED_HOST_V1` is the initial confidential-storage profile: all data, journal, catalog, snapshot, staging, temporary spill, page/swap and crash-dump locations reside on OS-managed encrypted volumes; the release pins and qualifies the platform/provider configuration. File permissions restrict access to the service identity and explicitly authorized backup operators. TLS/signing/decryption keys use the OS secret store or external key provider with separate access control. No custom page cipher is part of v1. This protects unavailable/locked storage under the provider's assumptions, not a compromised running host or tenant isolation from the database administrator.

`ENCRYPTED_BACKUP_V1` encrypts the complete bounded backup stream with the age v1 file format and configured X25519 recipients using a maintained implementation. It uses the format's generated per-file keys and authentication; no homegrown encryption framing is permitted. The implementation/spec revision and recipient fingerprints are recorded in the backup policy. [age file-format specification](https://age-encryption.org/v1)

The encrypted plaintext contains the canonical SPEC-012 backup/snapshot manifest, its `BACKUP_MANIFEST` integrity wrapper, all required snapshot/log/artifact data and an exact file inventory. Encryption alone does not authenticate which trusted authority produced a backup. Restore verifies the decrypted manifest signer, cluster/tenant scope, catalog/authority cut, file lengths/digests, completeness and supported formats before atomic passive installation. Archive extraction rejects absolute/parent paths, links escaping staging and oversized entries.

Backup creation requires `ManageBackup` over every included tenant; restore requires `RestorePassive` plus key-provider access. Backups include rights, fences, decisions, deduplication and retired identities as required by SPEC-011, not only user rows. Decrypted temporary files remain on the protected staging volume and are removed after use. A restored backup never gains live authority merely because its signature/decryption succeeds.

Secrets, private keys, plaintext credentials, full request arguments/results and decrypted backup contents MUST NOT appear in ordinary logs, exception messages, command-line diagnostics or telemetry. Diagnostic export is scoped and authorized; default redaction retains IDs/digests and safe state labels. Secret provider failure causes the affected function to fail closed.

## 11. Audit and incident evidence

Durable audit records contain event ID, authenticated principal/credential ID, cluster/tenant scope, action, target identity/digest, request/admin-command ID, applicable policy/catalog revision, authorization result and authority/outcome reference. Timestamps are diagnostic only; catalog/authority references establish ordered evidence. Sensitive values are omitted or represented by authorized opaque references.

Privileged catalog mutations, credential changes, plan activation, authority closure/replacement, backup/restore and retention changes persist their audit record with the deciding state-machine transaction. A privileged mutation is not acknowledged successful if required audit persistence failed. Business admission stores the policy/principal reference with its request evidence; asynchronous audit export cannot fabricate or undo its outcome.

Failed authentication and malformed-request events are rate-limited with aggregate counts so an unauthenticated flood cannot exhaust durable storage. Audit export uses authenticated channels, explicit retention and restricted readers. An append-only local log is not tamper-proof against an administrator/root compromise; stronger forensic claims require a separately configured external immutable audit destination and its trust assumptions.

## 12. Acceptance and qualification

| ID | Attack/failure schedule | Required result |
|---|---|---|
| SEC-01 | Valid certificate for another cluster or unregistered NodeId | Reject before protocol admission |
| SEC-02 | Client with tenant A permissions requests tenant B result/namespace | Deny without disclosing the hidden result or creating an identity |
| SEC-03 | Passive replica presents an authentic-looking donor commit digest | Require admitted donor role and authoritative durable record; no rights credit |
| SEC-04 | Replay one request/transfer/admin phase with changed immutable terms | Identity conflict before effects; original outcome unchanged |
| SEC-05 | Alter session frontier, subject, tenant or token purpose | Signature/scope validation fails; no token stripping fallback |
| SEC-06 | Remove required feature or request older security profile on retry | Negotiation fails; contract/security guarantee is not weakened |
| SEC-07 | Rotate signer while valid old receipts/sessions remain | Exact receipt bytes remain; retained verification or verified refresh works |
| SEC-08 | Revoke disconnected rights holder; replace its grant | Report pending closure/containment; no duplicate spend authority |
| SEC-09 | Client loses permissions after transaction commits | Original decision remains; result disclosure follows current authorization |
| SEC-10 | Credential expiry or uncertain authentication clock during prepare | New authentication fails; no timeout-based abort or rights reclaim |
| SEC-11 | Truncate/change/reorder encrypted backup content or extract escaping path | Restore rejects before live installation; no partial authority activation |
| SEC-12 | Valid signed old backup or grant replayed after retirement | Passive recovery/retirement checks prevent authority resurrection |
| SEC-13 | Huge/malformed token, duplicate key or signature algorithm confusion | Bounded parser failure; no allocation explosion or guessed semantics |
| SEC-14 | Required audit write fails during privileged catalog mutation | No successful unaudited privileged mutation |
| SEC-15 | TLS connection resumes after local credential revocation | Application authorization recheck denies affected operations |
| SEC-16 | Suspected compromised consensus member supplies conflicting history | Scope contained and evidence investigated; no crash-fault/BFT equivalence claim |

Release gates require independent negative authorization tests, codec/parser fuzzing, TLS/certificate failure tests, token golden/negative vectors, rotation/revocation fault campaigns, and encrypted backup restore drills. The qualification report records the exact crypto libraries/configurations, trust assumptions and unsupported profiles. Distributed correctness additionally requires SPEC-010's formal/simulation/process-fault gates; security testing does not replace those protocol obligations.
