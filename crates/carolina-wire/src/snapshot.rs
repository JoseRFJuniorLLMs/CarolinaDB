//! Snapshot interchange and the codec manifest (SPEC-012 §11–§12).

use carolina_core::canon::{decode_set, CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, domains, Hash256};
use carolina_core::ids::*;
use carolina_core::limits::Limits;

use crate::registry::{is_registered, CanonicalRecord, REGISTERED_KINDS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotKind {
    LocalStorage,
    SemanticGroup,
    Catalog,
}

impl SnapshotKind {
    fn label(&self) -> &'static str {
        match self {
            SnapshotKind::LocalStorage => "LocalStorage",
            SnapshotKind::SemanticGroup => "SemanticGroup",
            SnapshotKind::Catalog => "Catalog",
        }
    }
    fn from_label(s: &str) -> CoreResult<Self> {
        Ok(match s {
            "LocalStorage" => SnapshotKind::LocalStorage,
            "SemanticGroup" => SnapshotKind::SemanticGroup,
            "Catalog" => SnapshotKind::Catalog,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("snapshot kind {k}"),
                ))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDescriptor {
    pub index: u32,
    pub record_count: u32,
    pub byte_length: u32,
    pub chunk_hash: Hash256,
}

impl Canonical for ChunkDescriptor {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fu32("byte_length", self.byte_length)
            .fc("chunk_hash", &self.chunk_hash)
            .fu32("index", self.index)
            .fu32("record_count", self.record_count)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["byte_length", "chunk_hash", "index", "record_count"])?;
        Ok(ChunkDescriptor {
            index: v.field("index")?.as_u32()?,
            record_count: v.field("record_count")?.as_u32()?,
            byte_length: v.field("byte_length")?.as_u32()?,
            chunk_hash: Hash256::from_canon(v.field("chunk_hash")?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotManifestV1 {
    pub snapshot_id: [u8; 16],
    pub snapshot_version: u32,
    pub cluster_id: ClusterId,
    pub tenant_scope: Vec<TenantId>,
    pub kind: SnapshotKind,
    pub source_identity: StorageId,
    pub source_storage_epoch: StorageEpoch,
    pub catalog_generation: CatalogGeneration,
    pub plan_refs: Vec<PlanRef>,
    pub idc_bindings: Vec<IdcBinding>,
    pub membership_generation: MembershipGeneration,
    /// Opaque canonical cut description (SPEC-005 contexts with holes, or local commit seq).
    pub semantic_cut_with_holes: CanonValue,
    pub required_codec_manifest: Hash256,
    pub required_artifact_refs: Vec<Hash256>,
    pub request_namespace_retirements: Vec<RequestNamespace>,
    pub retained_result_horizons: CanonValue,
    pub unresolved_protocol_refs: Vec<ProtocolRecordRef>,
    pub authority_fences: Vec<ProtocolRecordRef>,
    pub chunks: Vec<ChunkDescriptor>,
}

impl SnapshotManifestV1 {
    /// Manifest hash excludes its own digest and any external security envelope (there is none inside).
    pub fn manifest_hash(&self) -> Hash256 {
        domain_hash(domains::SNAPSHOT_MANIFEST_V1, &self.encode())
    }
    /// Validate a complete chunk set against this manifest.
    ///
    /// `limits` is required because a chunk arrives from outside this process: its declared size
    /// bound (SPEC-012 §11) has to be enforced on ingest, and every record it carries has to be a
    /// registered kind/version. Decoding a chunk does not check either — `CanonicalRecord::from_canon`
    /// accepts any kind string, and only `decode_checked` consults the registry — so an importer
    /// that trusted decoding alone would install records this build cannot interpret.
    pub fn validate_chunks(&self, chunks: &[SnapshotChunkV1], limits: &Limits) -> CoreResult<()> {
        if chunks.len() != self.chunks.len() {
            return Err(CoreError::new(
                ErrorCode::Corruption,
                format!(
                    "expected {} chunks, got {}",
                    self.chunks.len(),
                    chunks.len()
                ),
            ));
        }
        for (i, (d, c)) in self.chunks.iter().zip(chunks).enumerate() {
            if d.index != i as u32 || c.index != i as u32 {
                return Err(CoreError::new(ErrorCode::Corruption, "chunk out of order"));
            }
            if c.snapshot_id != self.snapshot_id {
                return Err(CoreError::new(
                    ErrorCode::Corruption,
                    "chunk belongs to another snapshot",
                ));
            }
            if c.records.len() as u32 != d.record_count {
                return Err(CoreError::new(
                    ErrorCode::Corruption,
                    "chunk record count mismatch",
                ));
            }
            let bytes = c.encode();
            if bytes.len() as u32 != d.byte_length || c.chunk_hash() != d.chunk_hash {
                return Err(CoreError::new(
                    ErrorCode::Corruption,
                    "chunk length/hash mismatch",
                ));
            }
            c.check_size(limits)?;
            for r in &c.records {
                if !is_registered(&r.record_kind, r.record_version) {
                    return Err(CoreError::new(
                        ErrorCode::UnsupportedCodec,
                        format!(
                            "chunk {i} carries unregistered record kind {}/{}",
                            r.record_kind, r.record_version
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Canonical for SnapshotManifestV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fset("authority_fences", &self.authority_fences)
            .fc("catalog_generation", &self.catalog_generation)
            .fvec("chunks", &self.chunks)
            .fc("cluster_id", &self.cluster_id)
            .fset("idc_bindings", &self.idc_bindings)
            .fstr("kind", self.kind.label())
            .fc("membership_generation", &self.membership_generation)
            .fset("plan_refs", &self.plan_refs)
            .fset(
                "request_namespace_retirements",
                &self.request_namespace_retirements,
            )
            .fset("required_artifact_refs", &self.required_artifact_refs)
            .fc("required_codec_manifest", &self.required_codec_manifest)
            .f(
                "retained_result_horizons",
                self.retained_result_horizons.clone(),
            )
            .f(
                "semantic_cut_with_holes",
                self.semantic_cut_with_holes.clone(),
            )
            .fbytes("snapshot_id", &self.snapshot_id)
            .fu32("snapshot_version", self.snapshot_version)
            .fc("source_identity", &self.source_identity)
            .fc("source_storage_epoch", &self.source_storage_epoch)
            .fset("tenant_scope", &self.tenant_scope)
            .fset("unresolved_protocol_refs", &self.unresolved_protocol_refs)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "authority_fences",
            "catalog_generation",
            "chunks",
            "cluster_id",
            "idc_bindings",
            "kind",
            "membership_generation",
            "plan_refs",
            "request_namespace_retirements",
            "required_artifact_refs",
            "required_codec_manifest",
            "retained_result_horizons",
            "semantic_cut_with_holes",
            "snapshot_id",
            "snapshot_version",
            "source_identity",
            "source_storage_epoch",
            "tenant_scope",
            "unresolved_protocol_refs",
        ])?;
        let sid = v.field("snapshot_id")?.as_bytes()?;
        if sid.len() != 16 {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "snapshot_id must be 16 bytes",
            ));
        }
        let mut snapshot_id = [0u8; 16];
        snapshot_id.copy_from_slice(&sid);
        Ok(SnapshotManifestV1 {
            snapshot_id,
            snapshot_version: v.field("snapshot_version")?.as_u32()?,
            cluster_id: ClusterId::from_canon(v.field("cluster_id")?)?,
            tenant_scope: decode_set(v.field("tenant_scope")?)?,
            kind: SnapshotKind::from_label(v.field("kind")?.as_str()?)?,
            source_identity: StorageId::from_canon(v.field("source_identity")?)?,
            source_storage_epoch: StorageEpoch::from_canon(v.field("source_storage_epoch")?)?,
            catalog_generation: CatalogGeneration::from_canon(v.field("catalog_generation")?)?,
            plan_refs: decode_set(v.field("plan_refs")?)?,
            idc_bindings: decode_set(v.field("idc_bindings")?)?,
            membership_generation: MembershipGeneration::from_canon(
                v.field("membership_generation")?,
            )?,
            semantic_cut_with_holes: v.field("semantic_cut_with_holes")?.clone(),
            required_codec_manifest: Hash256::from_canon(v.field("required_codec_manifest")?)?,
            required_artifact_refs: decode_set(v.field("required_artifact_refs")?)?,
            request_namespace_retirements: decode_set(v.field("request_namespace_retirements")?)?,
            retained_result_horizons: v.field("retained_result_horizons")?.clone(),
            unresolved_protocol_refs: decode_set(v.field("unresolved_protocol_refs")?)?,
            authority_fences: decode_set(v.field("authority_fences")?)?,
            chunks: Vec::from_canon(v.field("chunks")?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotChunkV1 {
    pub snapshot_id: [u8; 16],
    pub index: u32,
    /// Records sorted by canonical (namespace,key); duplicates rejected by the owning schema.
    pub records: Vec<CanonicalRecord>,
}

impl SnapshotChunkV1 {
    pub fn chunk_hash(&self) -> Hash256 {
        domain_hash(domains::SNAPSHOT_CHUNK_V1, &self.encode())
    }
    pub fn descriptor(&self) -> ChunkDescriptor {
        ChunkDescriptor {
            index: self.index,
            record_count: self.records.len() as u32,
            byte_length: self.encode().len() as u32,
            chunk_hash: self.chunk_hash(),
        }
    }
    pub fn check_size(&self, limits: &Limits) -> CoreResult<()> {
        if self.encode().len() > limits.max_snapshot_chunk_bytes {
            return Err(CoreError::new(
                ErrorCode::ResourceLimit,
                "snapshot chunk exceeds limit",
            ));
        }
        Ok(())
    }
}

impl Canonical for SnapshotChunkV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fu32("index", self.index)
            .fvec("records", &self.records)
            .fbytes("snapshot_id", &self.snapshot_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["index", "records", "snapshot_id"])?;
        let sid = v.field("snapshot_id")?.as_bytes()?;
        if sid.len() != 16 {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "snapshot_id must be 16 bytes",
            ));
        }
        let mut snapshot_id = [0u8; 16];
        snapshot_id.copy_from_slice(&sid);
        Ok(SnapshotChunkV1 {
            snapshot_id,
            index: v.field("index")?.as_u32()?,
            records: Vec::from_canon(v.field("records")?)?,
        })
    }
}

/// Content-addressed codec manifest (§12): every registered kind with owner, version, hash domain
/// and the golden fixture digest when frozen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecManifestEntry {
    pub record_kind: String,
    pub record_version: u32,
    pub owner: String,
    pub hash_domain: String,
    pub golden_fixture_hash: Option<Hash256>,
}

impl Canonical for CodecManifestEntry {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fopt("golden_fixture_hash", &self.golden_fixture_hash)
            .fstr("hash_domain", &self.hash_domain)
            .fstr("owner", &self.owner)
            .fstr("record_kind", &self.record_kind)
            .fu32("record_version", self.record_version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "golden_fixture_hash",
            "hash_domain",
            "owner",
            "record_kind",
            "record_version",
        ])?;
        Ok(CodecManifestEntry {
            record_kind: v.field("record_kind")?.as_str()?.to_string(),
            record_version: v.field("record_version")?.as_u32()?,
            owner: v.field("owner")?.as_str()?.to_string(),
            hash_domain: v.field("hash_domain")?.as_str()?.to_string(),
            golden_fixture_hash: Option::from_canon(v.field("golden_fixture_hash")?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecManifest {
    pub manifest_version: u32,
    pub wire_version: String,
    pub entries: Vec<CodecManifestEntry>,
    pub limits_version: u32,
}

impl CodecManifest {
    pub fn v1() -> CodecManifest {
        let entries = REGISTERED_KINDS
            .iter()
            .map(|(k, v, owner)| CodecManifestEntry {
                record_kind: k.to_string(),
                record_version: *v,
                owner: owner.to_string(),
                hash_domain: match *k {
                    "request_content" => domains::REQUEST_V1,
                    "final_receipt" => domains::RECEIPT_V1,
                    "snapshot_manifest" => domains::SNAPSHOT_MANIFEST_V1,
                    "snapshot_chunk" => domains::SNAPSHOT_CHUNK_V1,
                    "hello" | "hello_ack" => domains::NEGOTIATION_V1,
                    _ => domains::PROTOCOL_RECORD_V1,
                }
                .to_string(),
                golden_fixture_hash: None,
            })
            .collect();
        CodecManifest {
            manifest_version: 1,
            wire_version: "1.0".into(),
            entries,
            limits_version: Limits::v1().limits_version,
        }
    }
    pub fn manifest_hash(&self) -> Hash256 {
        domain_hash("astra.codec-manifest.v1", &self.encode())
    }
}

impl Canonical for CodecManifest {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("entries", &self.entries)
            .fstr("kind", "codec-manifest.v1")
            .fu32("limits_version", self.limits_version)
            .fu32("manifest_version", self.manifest_version)
            .fstr("wire_version", &self.wire_version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "entries",
            "kind",
            "limits_version",
            "manifest_version",
            "wire_version",
        ])?;
        Ok(CodecManifest {
            manifest_version: v.field("manifest_version")?.as_u32()?,
            wire_version: v.field("wire_version")?.as_str()?.to_string(),
            entries: Vec::from_canon(v.field("entries")?)?,
            limits_version: v.field("limits_version")?.as_u32()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_validates_chunks() {
        let sid = [7u8; 16];
        let chunk = SnapshotChunkV1 {
            snapshot_id: sid,
            index: 0,
            records: vec![],
        };
        let m = SnapshotManifestV1 {
            snapshot_id: sid,
            snapshot_version: 1,
            cluster_id: ClusterId::derive("c"),
            tenant_scope: vec![],
            kind: SnapshotKind::LocalStorage,
            source_identity: StorageId::derive("s"),
            source_storage_epoch: StorageEpoch(1),
            catalog_generation: CatalogGeneration(1),
            plan_refs: vec![],
            idc_bindings: vec![],
            membership_generation: MembershipGeneration(1),
            semantic_cut_with_holes: CanonValue::Null,
            required_codec_manifest: CodecManifest::v1().manifest_hash(),
            required_artifact_refs: vec![],
            request_namespace_retirements: vec![],
            retained_result_horizons: CanonValue::Null,
            unresolved_protocol_refs: vec![],
            authority_fences: vec![],
            chunks: vec![chunk.descriptor()],
        };
        let lim = Limits::v1();
        assert!(m
            .validate_chunks(std::slice::from_ref(&chunk), &lim)
            .is_ok());
        assert!(m.validate_chunks(&[], &lim).is_err());
        let other = SnapshotChunkV1 {
            snapshot_id: [8u8; 16],
            ..chunk.clone()
        };
        assert!(m.validate_chunks(&[other], &lim).is_err());
        let dup = SnapshotChunkV1 {
            index: 0,
            ..chunk.clone()
        };
        assert!(m.validate_chunks(&[chunk.clone(), dup], &lim).is_err());
        let back = SnapshotManifestV1::decode(&m.encode(), &Limits::v1()).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.manifest_hash(), m.manifest_hash());
        let cm = CodecManifest::v1();
        assert_eq!(
            CodecManifest::decode(&cm.encode(), &Limits::v1()).unwrap(),
            cm
        );
    }

    /// SPEC-012 §11: a chunk arrives from outside this process, so validation has to enforce the
    /// declared size bound and refuse record kinds this build does not know. Decoding checks
    /// neither — `CanonicalRecord::from_canon` accepts any kind string.
    #[test]
    fn a_chunk_is_refused_when_it_is_oversized_or_carries_an_unregistered_kind() {
        let sid = [3u8; 16];
        let manifest_for = |c: &SnapshotChunkV1| SnapshotManifestV1 {
            snapshot_id: sid,
            snapshot_version: 1,
            cluster_id: ClusterId::derive("c"),
            tenant_scope: vec![],
            kind: SnapshotKind::LocalStorage,
            source_identity: StorageId::derive("s"),
            source_storage_epoch: StorageEpoch(1),
            catalog_generation: CatalogGeneration(1),
            plan_refs: vec![],
            idc_bindings: vec![],
            membership_generation: MembershipGeneration(1),
            semantic_cut_with_holes: CanonValue::Null,
            required_codec_manifest: CodecManifest::v1().manifest_hash(),
            required_artifact_refs: vec![],
            request_namespace_retirements: vec![],
            retained_result_horizons: CanonValue::Null,
            unresolved_protocol_refs: vec![],
            authority_fences: vec![],
            chunks: vec![c.descriptor()],
        };

        // a record whose kind is not in the registry
        let unknown = SnapshotChunkV1 {
            snapshot_id: sid,
            index: 0,
            records: vec![CanonicalRecord {
                record_kind: "storage_row".into(),
                record_version: 1,
                body: CanonValue::Null,
            }],
        };
        let m = manifest_for(&unknown);
        assert_eq!(
            m.validate_chunks(std::slice::from_ref(&unknown), &Limits::v1())
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedCodec
        );

        // a registered kind at an unregistered version is refused the same way
        let bad_version = SnapshotChunkV1 {
            records: vec![CanonicalRecord {
                record_kind: "compiled_batch".into(),
                record_version: 2,
                body: CanonValue::Null,
            }],
            ..unknown.clone()
        };
        let m = manifest_for(&bad_version);
        assert_eq!(
            m.validate_chunks(std::slice::from_ref(&bad_version), &Limits::v1())
                .unwrap_err()
                .code,
            ErrorCode::UnsupportedCodec
        );

        // and the size bound is enforced: the same chunk passes under v1 and fails under the
        // tiny profile, whose limit is far smaller
        let ok = SnapshotChunkV1 {
            records: vec![CanonicalRecord {
                record_kind: "compiled_batch".into(),
                record_version: 1,
                body: CanonValue::str("x".repeat(4096)),
            }],
            ..unknown.clone()
        };
        let m = manifest_for(&ok);
        assert!(m
            .validate_chunks(std::slice::from_ref(&ok), &Limits::v1())
            .is_ok());
        assert_eq!(
            m.validate_chunks(std::slice::from_ref(&ok), &Limits::tiny())
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
    }
}
