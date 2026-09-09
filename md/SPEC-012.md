# SPEC-012 — Request Identity, Client Protocol, Wire Encoding & Compatibility

**Status:** Draft 0.1 — normative design; codecs, full golden corpus and interoperability are not implemented or qualified  
**Date:** 2026-09-09  
**Depends on:** [SPEC-002](SPEC-002.md), [SPEC-003](SPEC-003.md), [SPEC-005](SPEC-005.md)–[SPEC-011](SPEC-011.md)  
**Trust owner:** [SPEC-013](SPEC-013.md)  
**Qualification and implementation:** [SPEC-010](SPEC-010.md), [SPEC-014](SPEC-014.md)  
**Normative terms:** MUST, MUST NOT, SHOULD and MAY express requirements, not implemented capabilities.

## 1. Ownership and scope

This document owns the client request lifecycle, stable identity allocation, exact receipt schema, public retry/resolution outcomes, runtime record encoding, transport framing, snapshot interchange and compatibility negotiation. These are observable contract requirements. An SDK cannot silently replace an unknown request with another invocation or weaken a requested session guarantee.

SPEC-003 remains authoritative for canonical compiler IR, normalization and its hash domains. SPEC-002 owns pages, ordered keys and the local journal; this document does not replace their physical codecs. SPEC-005–009 own protocol state-machine semantics. SPEC-011 owns typed identities, routing and authority allocation. SPEC-013 authenticates bytes and callers. A hash, a successful decode or a transport ACK is not authority or proof of a committed transaction.

v1 uses operation RPCs and explicit reads/status lookup. Arbitrary SQL execution, external side effects, cross-cluster request relocation and an implicit multi-group C2 session are unsupported.

## 2. Request identities and immutable content

```text
RequestKey {
  tenant_id: TenantId,
  request_namespace: RequestNamespace,
  stable_request_id: StableRequestId
}
OperationRef { operation_id: OperationId, version: u32 }
RequestContentV1 {
  request_key: RequestKey,
  operation: OperationRef,
  operation_hash: OperationHash,
  schema_hash: SchemaHash,
  contract_hash: ContractHash,
  arguments: CanonicalTypedValue,
  read_contract: ReadContractV1,
  initial_session: Option<ObservationTokenV1>
}
InvokeV1 {
  content: RequestContentV1,
  request_hash: RequestHash,
  attempt_id: AttemptId,
  deadline_budget_ms: u64,
  route_hint: Option<RouteHint>,
  accepted_result_codecs: SortedSet<CodecRef>
}
```

`RequestNamespace` is an immutable tenant-owned catalog identity with access policy and a non-reuse lifecycle. It is not an arbitrary unauthenticated string chosen to bypass retention. Each `StableRequestId` identifies one logical invocation in that namespace. Clients persist it and the original content before sending the first attempt. Retries keep both unchanged. `TenantId`, `RequestNamespace`, `StableRequestId` and `AttemptId` use distinct 128-bit values; their canonical textual form is exactly 32 lowercase hexadecimal characters. Namespace and tenant names are display labels only.

`RequestHash = SHA-256(UTF8("astra.request.v1") || 0x00 || canonical(RequestContentV1))`. It includes the complete originally requested observation contract and initial token payload, excluding the token's external authentication wrapper. No field is defaulted away. Hashes are 32 bytes encoded as 64 lowercase hexadecimal characters. A client-supplied digest is recomputed by the server; it cannot replace schema/type/authorization checks.

Attempt IDs, deadlines, trace IDs, selected execution plan, routing hints, TLS credentials and codec transport negotiation are excluded. They do not change the logical operation. A different operation version, arguments, contract, tenant, namespace or original dependency token is different semantic content. The same RequestKey with any such change returns `RequestIdentityMismatch`; no new execution is created. A retry needing newly accumulated session context resolves the old invocation first, then uses that context on a separate subsequent invocation/read.

The plan is bound at admission and retained for that execution. It is deliberately not an input to client request identity: rerouting or a pre-admission plan publication must not manufacture another logical request. Once admitted, retries use the recorded plan, even during migration.

## 3. RequestHome selection and transaction allocation

The order is normative:

```text
authenticate + authorize namespace/operation
    -> route(RequestKey) using SPEC-011's request-route registry
    -> RequestHome durable BindIfAbsent
    -> bind execution plan and dispatch
```

Routing never requires a TxnId. A route entry identifies a home group, `RequestHomeEpoch` and route coverage. Gateways may cache hints; the serving home validates its current durable grant/fence. A redirect carries a verifiable route reference, not permission to allocate locally.

```text
RequestBindingV1 {
  request_key: RequestKey,
  request_hash: RequestHash,
  txn_id: TxnId,
  allocation_home: RequestHomeId,
  allocation_epoch: RequestHomeEpoch,
  allocation_seq: RequestAllocationSeq,
  record_revision: RecordRevision,
  state: BOUND | ADMITTED | TERMINAL | RESULT_EXPIRED,
  admitted_plan: Option<PlanRef>,
  decision_authority: Option<ProtocolRecordRef>,
  terminal_receipt: Option<FinalReceiptV1>,
  tombstone: Option<ResultTombstoneV1>
}
```

The active RequestHome performs one linearizable durable CAS that inserts an absent RequestKey, allocates its sequence and stores the complete content digest. `TxnId` is the exact 256-bit concatenation of `RequestHomeId` (128 bits), allocation `RequestHomeEpoch` (u64) and `RequestAllocationSeq` (u64); integers in this identity use unsigned big-endian bytes. Canonical TxnId text is 64 lowercase hexadecimal characters. Each home/epoch sequence starts at 1, is never reused and stops on u64 exhaustion. SPEC-011 allocates unique home identities/epochs; replacement or restore cannot reuse an allocation epoch without its intact counter and binding history. Lost allocation evidence requires a successor epoch, not a counter reset.

An existing same-hash binding returns its original TxnId; a different hash fails without effects. The allocated tuple is immutable even if current routing moves to another home. A crash before durable CAS admits no work; a lost CAS reply is resolved by RequestKey. No participant dispatch is legal until the binding satisfies the home authority durability policy, even when the business result contract is only `LocalStable`.

Admission atomically stores the selected plan, original decision authority and recoverable execution inputs at the home before dispatch. Concurrent dispatches use the same immutable identity and idempotent protocol transitions. Allocation is not business commitment and consumes no business rights. A BOUND request can wait across plan publication; an ADMITTED request must resolve its original execution rather than move to a different protocol.

Moving a home requires SPEC-011 fencing and SPEC-009 closure/drain or an explicitly qualified handoff preserving every binding, unresolved request, counter and tombstone. v1 uses closure/drain. An unreachable old home with possible admitted requests blocks conflicting replacement; timeout does not establish absence. Permanent evidence loss returns unavailability/unknown rather than reallocation.

## 4. Public outcomes and retry rules

```text
ClientReplyV1 = Committed(FinalReceiptV1)
              | Rejected(FinalReceiptV1)
              | Unavailable(RefusalV1)
              | OutcomeUnknown(ResolutionHintV1)
              | RequestIdentityMismatch
              | ResultExpired(ResultTombstoneV1)
              | IdentityExpired(NamespaceRetirementRef)
              | ProtocolError(code)
```

| Reply | Meaning | Permitted client action |
| --- | --- | --- |
| `Committed` | Complete invocation is final at its contract boundary | Retain exact receipt; repeating the same key only resolves it |
| `Rejected` | Durable final business rejection established by the declared predicate/observation | Retain exact rejection; it is not reevaluated on retry |
| `Unavailable` | This attempt was refused without authorizing new effects; does not settle an already admitted execution | Retry/resolve the same RequestKey and content |
| `OutcomeUnknown` | The system cannot yet report whether the original invocation became final | Resolve the same RequestKey; never auto-generate a replacement |
| `RequestIdentityMismatch` | Existing immutable identity disagrees with supplied content | Surface an application identity error |
| `ResultExpired` | Terminal execution is known and protected against reexecution; exact result bytes were evicted | Report expiration and retained evidence; do not reconstruct a fresh value |
| `IdentityExpired` | Namespace was durably retired; new invocation under it is forbidden | Never retry that logical invocation under a new namespace automatically |
| `ProtocolError` | Malformed/unsupported/unauthorized request or guarantee | Surface the specific error; it is not a final business rejection |

Admission errors (`AuthorityUnavailable`, `MigrationInProgress`, `NotReady`, `SessionScopeMismatch`, `UnsupportedSessionScope`) map to `Unavailable` or `ProtocolError` according to the owning contract. They cannot masquerade as `OutOfStock` or another final predicate result. `Unknown`, `UnknownOutcome` and `UNKNOWN` in internal design notation map to the single public `OutcomeUnknown`; internal `ABORTED` is not automatically public `Rejected`.

Deadline expiry after possible dispatch returns `OutcomeUnknown` unless the original decision is already known. A timeout or disconnect never cancels durable prepare or proves abort. Cancellation would be a separately defined operation and is outside v1.

SDKs may use bounded backoff and refreshed routing, but preserve RequestKey, content, original dependency token and decision authority. They must expose unknown/expired outcomes as distinct types and must not treat all non-success replies as safe-to-repeat-new-operation failures.

## 5. ResolveRequest and retention

```text
ResolveRequestV1 { request_key: RequestKey, expected_request_hash: RequestHash }
ResolveReplyV1 = Terminal(ClientReplyV1)
               | Pending(txn_id, phase, original_plan, resolver_ref)
               | AbsentAtBarrier(catalog_ref, home_ref, record_revision)
               | Unavailable(reason)
```

Resolution reauthenticates and authorizes access to that tenant/namespace/result. It routes by RequestKey and consults the home and retained original decision authority; stale follower absence is insufficient. A known final result is returned unchanged. A pending transaction remains pending until evidence resolves it. The client need not know TxnId to recover a lost first reply.

`AbsentAtBarrier` is a linearizable absence observation at a specific live home boundary, not a permanent nonexecution certificate. A concurrent Invoke may bind immediately afterwards; retry still uses the same key. A tombstone or retired namespace must never return Absent. A home unavailable during resolution yields no absence inference.

Result eviction is a durable transition from TERMINAL to RESULT_EXPIRED retaining `(RequestKey, RequestHash, TxnId, outcome, receipt_digest, decision_ref)` as `ResultTombstoneV1`. Exact receipt bytes remain pinned while any active contract/backup/migration/resolution obligation requires them. Tombstones stay until the entire namespace is durably closed to every future admission and its retirement record survives all supported restore/GC paths. Wall-clock expiration alone never permits reuse of a key or namespace. Namespace retirement blocks new admission; retained historical terminal receipts may still be resolved while authorized. Otherwise return `IdentityExpired`, without hiding retained known outcome evidence.

The retained namespace retirement registry is authoritative for both live serving and backup restore. Ancient clients and stale nodes cannot recreate an evicted request. Storage pressure causes refusal/backpressure, not loss of anti-reexecution state.

## 6. Reads and session scope

```text
ReadContractV1 {
  visibility: LocalSnapshot | Causal | Certified | Serial,
  scope: CanonicalScope,
  session_guarantees: SortedSet<ReadYourWrites | MonotonicReads | CausalDependencies>,
  session_scope: None | ReplicationGroup(ReplicationGroupId) | CompositeScope(CanonicalScope)
}
ObservationTokenV1 = Group(SessionTokenV1)
                   | Certified(CertifiedReadEvidenceV1)
                   | Serial(SerialReadEvidenceV1)
                   | Composite(CompositeReadEvidenceV1)
```

`SessionTokenV1` and `CausalContextV1` payload fields and dotted-hole semantics belong to SPEC-005. The client-visible wrapper binds cluster, tenant, namespace, contract, token kind/version and the exact canonical payload; SPEC-013 supplies its integrity envelope. A plain foreign LSN is not an observation token. Trusted server lookup of an opaque retained token reference may replace portable bytes only under a separately versioned supported codec; v1 portable tokens do not silently change into references.

C2 v1 carries one group context. A group token used in another group returns `SessionScopeMismatch` before admission/read publication. Clients may keep independent group sessions, but cannot merge them into one promised causal session. An invocation with a dependency from G1 and effects/observations in G2 requires an explicit qualified composite plan. This document does not enable that extension merely by listing `Composite` as a token discriminant; unavailable token families produce `UnsupportedSessionScope`.

Within a group, frontiers join with hole preservation under SPEC-005, never by taking an unjustified sequence maximum. Across plan/IDC migration, SPEC-009 must translate dependencies through retained mappings; missing mappings yield `SessionFrontierUnavailable`. No implicit token reset, omitted predecessor or local-snapshot downgrade is allowed.

Certified/serial/composite evidence obeys SPEC-007/008's read barriers and publication gates. Composite C5 atomic read evidence is not a multi-group C2 frontier and grants no unsupported causal session promise.

## 7. Exact final receipts and evidence

```text
FinalReceiptV1 {
  receipt_version: u32,
  cluster_id: ClusterId,
  request_key: RequestKey,
  request_hash: RequestHash,
  txn_id: TxnId,
  operation: OperationRef,
  operation_hash: OperationHash,
  schema_hash: SchemaHash,
  contract_hash: ContractHash,
  plan: PlanRef,
  idc_bindings: SortedSet<IdcBinding>,
  origin_ids: SortedSet<OriginId>,
  outcome: COMMITTED | REJECTED,
  result_codec: CodecRef,
  result_type_hash: Hash,
  exact_result_bytes: Bytes,
  result_digest: Hash,
  commitments: CanonicalTypedValue,
  observation_token: Option<ObservationTokenV1>,
  durability_policy: PolicyRef,
  durability_evidence: SortedSet<ProtocolRecordRef>,
  decision_ref: ProtocolRecordRef,
  completion_ref: Option<ProtocolRecordRef>
}
```

`FinalReceipt` in other SPECs is the logical alias of `FinalReceiptV1`, not another schema. `result_digest = SHA-256(UTF8("astra.result.v1") || 0x00 || canonical({result_codec,result_type_hash,exact_result_bytes}))`. `ReceiptDigest` hashes the whole receipt under `astra.receipt.v1`; its own digest/signature is not embedded in that input. Business result bytes include typed rejection reason when outcome is REJECTED. Commitments explicitly encode what was confirmed, using the operation's versioned contract. A receipt for allocation cannot imply an exact current global balance.

Accepted result material is fixed at the unique decision boundary and stored atomically or deterministically recoverably with it under SPEC-002. Later publication/completion evidence refers to that immutable material. RequestHome persists the complete receipt once all required evidence exists, before returning a final reply. Composite C4/C5 requires SPEC-008 completion; an individual participant commit cannot create a final receipt. Subsequent resolution returns byte-identical canonical receipt payloads, including rejection, origin identities and observation evidence. Crypto key rotation may rewrap the same payload; it never changes the historical receipt bytes.

ProtocolRecordRef binds record kind, schema version, stable record key and canonical payload hash. `DecisionCertificate`, `PublicationCertificate`, `PublishSeen` and `CompletionCertificate` retain their distinct SPEC-008 meanings. Referenced decision, authority and durability records must be recoverable and validated; unknown or unauthenticated evidence does not authorize a receipt. Hash/receipt construction is acyclic: accepted payload first, decision references its digest, publication/completion references the decision, final receipt references completed evidence, and the external security envelope covers receipt bytes last.

## 8. Canonical runtime encoding and domains

Runtime payloads use SPEC-003's strict canonical JSON scalar rules: UTF-8, sorted unique ASCII field names, explicit required fields, integers as canonical strings, byte arrays as lowercase hex, no floats, no normalization, no redundant zeros and no skippable semantic fields. Ordered vectors preserve order; sets sort by canonical bytes and reject duplicates. Absent optional values are explicit `null`. Decoders reencode and compare byte-for-byte before accepting canonical input.

Every record has exactly `{"body":<typed body>,"record_kind":<registered name>,"record_version":"1"}`. The body schema is selected by exact kind/version, not by heuristics on content. The registry includes requests/replies/bindings/receipts in this document and the canonical records of SPEC-005–009/011. Owning schemas must enumerate every field, scalar width, enum value and required reference in the frozen codec manifest. `...`, implementation-dependent maps, Rust memory layout and unregistered variants are not valid codec definitions. SPEC-003 artifacts keep their existing raw canonical representation inside explicit artifact records.

New domains are `astra.request.v1`, `astra.result.v1`, `astra.receipt.v1`, `astra.protocol-record.v1`, `astra.snapshot-chunk.v1`, `astra.snapshot-manifest.v1` and `astra.negotiation.v1`. Each uses `SHA-256(UTF8(domain) || 0x00 || canonical_payload)`. Request/result/receipt inputs are their named bodies above; protocol-record inputs are the complete wrapped record. Snapshot chunks hash their exact canonical chunk payload. Manifest hashes exclude their own digest and external security envelope. Negotiation hashes the ordered offered/selected transcript records. Existing domains from SPEC-003/004 remain unchanged.

Local journal CRCs and physical page encoding remain SPEC-002-owned. Semantic journal payloads reference these canonical runtime records; they may not substitute local pointers, LSNs or build-dependent binary enums for semantic identity.

## 9. Binary wire envelope and limits

The baseline application transport is an ordered byte stream protected by SPEC-013. Every frame starts with this 32-byte header; all header integers are unsigned little-endian. Exactly `payload_length` canonical payload bytes follow, with no padding or trailer. TLS is external to this envelope.

| Offset | Bytes | Field / v1 rule |
| ---: | ---: | --- |
| 0 | 4 | Magic ASCII `ASTR` = `41 53 54 52` |
| 4 | 2 | Wire major = 1 |
| 6 | 2 | Wire minor = 0 |
| 8 | 2 | Message kind: 1=Hello, 2=HelloAck, 3=Invoke, 4=Reply, 5=ResolveRequest, 6=ResolveReply, 7=Read, 8=ReadReply, 9=ProtocolRecord, 10=SnapshotManifest, 11=SnapshotChunk |
| 10 | 2 | Flags = 0; all other bits rejected |
| 12 | 4 | Payload byte length |
| 16 | 8 | Stream ID: 0 for negotiation; nonzero client/node-chosen request stream otherwise |
| 24 | 8 | Per-direction connection frame sequence, starts at 0, increments by 1 without wrap |

Read and ReadReply records carry ReadContractV1, token and typed query/result with a frozen query schema; unsupported query languages fail before execution. ProtocolRecord is peer/admin-only according to its registered kind. A frame-kind/body mismatch is rejected.

v1 hard limits: payload 16 MiB; request arguments and exact business result each 1 MiB; portable token 64 KiB; canonical nesting depth 64; one collection at most 65,536 elements; whole snapshot chunk at most 4 MiB. Chunking cannot bypass a semantic result/token limit. The receiver applies limits before allocation and incrementally enforces depth/count bounds. Applications can configure smaller advertised limits; negotiation takes minima. Larger requirements fail with `ResourceLimit` before effects, or return a retained outcome via an already supported representation; there is no result truncation.

Unknown magic/major/minor/kind/flag, sequence gap/reuse, truncated or oversized frame fails the connection before dispatch of that frame. Previously dispatched requests may still be unresolved; transport failure does not roll them back. v1 has no compression or semantic fragmentation. Large snapshots use independent bounded chunks. Stream IDs and sequence numbers are transport correlation/replay checks, never dedupe identities; reconnect retries use RequestKey or protocol record ID.

## 10. Negotiation and downgrade protection

Hello/HelloAck are the only records accepted before negotiation. Each contains cluster/endpoint role, nonce, offered or selected wire versions, record codecs, IR/plan versions, protocol capabilities, limits, and authenticated capability-manifest reference. The selected transcript binds the offer, selection, peer identities and cluster under SPEC-013. HelloAck cannot choose an unoffered version/capability or exceed either side's limits.

v1 supports only wire 1.0 and explicitly registered record versions. Future versions may coexist only through an explicit compatibility matrix with golden corpus and rolling-upgrade evidence. Unsupported semantics return `UnsupportedCodec`/`IncompatiblePeer`; the client must not silently reconnect using an older codec after authenticated incompatibility. A changed transport encoding never changes contract hashes, required authority/durability, scopes or finality.

Unknown fields in v1 semantic records are rejected, including fields labelled optional by an unrecognized sender. Optional extension skipping requires a future registered version proving the extension has no semantic/hash effect on the accepted contract. Upgrades do not gain optionality merely because a generic serialization library ignores unknown fields.

Capabilities used in an active plan must be supported by all required serving/decision/recovery participants under SPEC-011. Writer upgrades retain old readers/decoders while pinned artifacts remain. Rollback is forbidden once newer retained state cannot be read by the proposed old binary. Fail closed rather than remove fields or rehash old receipts to fit an older codec.

## 11. Snapshots, restore and compatibility

```text
SnapshotManifestV1 {
  snapshot_id, snapshot_version, cluster_id, tenant_scope,
  kind: LocalStorage | SemanticGroup | Catalog,
  source_identity, source_storage_epoch,
  catalog_generation, plan_refs, idc_bindings,
  membership_generation, semantic_cut_with_holes,
  required_codec_manifest, required_artifact_refs,
  request_namespace_retirements, retained_result_horizons,
  unresolved_protocol_refs, authority_fences,
  chunks: ordered Vec<{index, record_count, byte_length, chunk_hash}>
}
SnapshotChunkV1 { snapshot_id, index, records: ordered Vec<CanonicalRecord> }
```

The manifest and chunks are separately canonical records. Each chunk belongs to exactly one manifest position; missing, reordered, duplicated, corrupted or extra chunks fail validation. Records use registered namespaces and typed keys, sorted by canonical `(namespace,key)` with duplicates rejected unless the owning schema explicitly models physical MVCC versions. Manifest pins and exact codec/type definitions are frozen before snapshot compatibility is advertised. A complete snapshot includes request mappings/tombstones, result/evidence records, unresolved prepares, transfer authority, causal holes, fences and retained plan/schema artifacts, not merely user rows.

LocalStorage is an opaque SPEC-002 checkpoint/journal bundle with pinned physical versions/codecs. SemanticGroup is a recoverable semantic cut that assigns fresh target-local MVCC positions; it does not import a remote LSN as global time. Catalog uses SPEC-011's consensus snapshot/commit index and typed records. Snapshot digest identity does not establish that the captured cut was causally complete or authorized; owning protocols certify those facts.

Install into closed staging state; validate all hashes, schema and namespace ownership, cut closure, retained commitments, unresolved evidence and storage barriers before marking ready. Restore/replacement gets the required fresh storage/origin/authority epochs under SPEC-011; a copied image never grants authority by itself. Admission remains closed until the catalog authorizes the reconciled state. An older backup cannot discard namespace retirements or re-enable an expired request/holder. Missing necessary history or keys yields explicit recovery failure.

At-rest and backup encryption/authentication wraps the exact snapshot bytes under SPEC-013; plaintext test snapshots cannot be advertised as encrypted backups. Restore compatibility is tested independently from wire compatibility.

## 12. Codec manifest and golden vectors

Every supported release emits a content-addressed `CodecManifest` listing kind/version, owning SPEC, every body field/type/width/enum, canonical order, allowed sizes, hash input domain, encoder/decoder build and fixture hashes. It includes full golden request, receipt, each protocol/certificate kind, token-with-holes and all snapshot kinds. Missing schema entries make that capability unpublishable. A generic JSON round trip is insufficient.

The wire header-only vector for a Hello with an empty canonical object payload is:

```text
41 53 54 52 01 00 00 00 01 00 00 00 02 00 00 00
00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
7b 7d
```

This fixes offsets, endianness and length only. `{}` deliberately fails Hello's required semantic fields and MUST NOT negotiate a connection. A decoder that accepts it as a valid Hello fails the negative fixture. The complete golden semantic corpus is an implementation deliverable, not claimed by this small envelope vector.

Required negative vectors include duplicate keys, decimal-string leading zeros, wrong fixed hash/ID width, invalid UTF-8, unknown enums, missing versus null fields, altered mandatory fields, excessive depth/count/length, foreign tenants, mismatched wire kinds, signature substitution, digest input cycles and a snapshot missing one prepared decision. Golden byte/digest files must be generated and verified with independent implementations on supported architectures before any cross-version/persistent compatibility claim.

| Compatibility pair | v1 policy / required evidence |
| --- | --- |
| Same wire and record version | Exact golden bytes and semantic conformance required |
| Unknown major/minor/record version | Refuse; no heuristic decoder |
| New binary with retained old data | Explicit old decoder coverage and recovery corpus |
| Old binary after new data written | Refuse unless manifest proves full retained-state readability |
| Plan migration with codec change | SPEC-009 drain/transform plus both codec and observation evidence |
| Backup from earlier catalog generation | Recovery reconciles retained fences/retirements before authority |

## 13. Acceptance scenarios

| ID | Scenario | Required result |
| --- | --- | --- |
| CP-01 | Two gateways race identical RequestKey before any TxnId exists | One durable mapping, one TxnId, one effect |
| CP-02 | Same key with changed args/version/contract/initial token | RequestIdentityMismatch before new effects |
| CP-03 | Crash before/after BindIfAbsent; allocation reply lost | Same key resolves one binding or absence at barrier; no dispatch before durability |
| CP-04 | Home moves; delayed old gateway and ancient retry arrive | Fenced old home; original TxnId/results/tombstones preserved |
| CP-05 | Participant installs; publication incomplete; client times out | OutcomeUnknown/pending; no premature final receipt |
| CP-06 | Commit or final rejection reply lost, followed by migration and retry | Byte-identical retained canonical receipt |
| CP-07 | Result evicted, then namespace retired and restored from older backup | No reexecution; ResultExpired/IdentityExpired or retained exact result |
| CP-08 | G1 token submitted to G2; token loses a causal hole | Scope mismatch or explicit qualified composition; malformed frontier rejected |
| CP-09 | Frames split arbitrarily by transport; corrupt/oversized/unknown frame | Bounded parsing, exact frames, no invalid dispatch |
| CP-10 | Peer changes negotiation transcript or drops required guarantee | Authentication/compatibility failure; no downgrade |
| CP-11 | Snapshot chunk/evidence missing, duplicated or reordered | Closed staging/recovery failure; no authority resurrection |
| CP-12 | Encode/decode all enabled schemas on supported architectures | Identical golden bytes/digests; malformed fixtures rejected |
| CP-13 | Resolve reports absence while concurrent identical Invoke binds | Retry same key still converges to one execution |
| CP-14 | Credential rotation or different authorized resolver accesses receipt | Exact payload preserved; current access policy enforced |

SPEC-010 records independent decoder, deterministic simulation and real-process evidence for applicable cases. No distributed capability is ready merely because this protocol has been written. SPEC-014 orders the local identity/codec slice before distributed execution.
