//! Nominal identity, generation and epoch taxonomy (SPEC-011 §2) and composite
//! identities (SPEC-012 §2–3, SPEC-002 §102).
//!
//! Every scalar type is a distinct newtype. There are deliberately no conversions
//! between epoch types: `IdcGeneration(3)` and `IdcAuthorityEpoch(3)` are unrelated
//! values. Counters reject overflow instead of wrapping.

use std::fmt;

use crate::canon::{CanonValue, Canonical};
use crate::error::{CoreError, CoreResult, ErrorCode};
use crate::hash::{hex_decode, hex_encode, Hash256};

// ---------------------------------------------------------------------------
// 128-bit identities: canonical text is exactly 32 lowercase hex characters
// ---------------------------------------------------------------------------

macro_rules! id128 {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub [u8; 16]);

        impl $name {
            pub const NIL: $name = $name([0u8; 16]);
            pub fn from_u128(v: u128) -> Self { $name(v.to_be_bytes()) }
            pub fn as_u128(&self) -> u128 { u128::from_be_bytes(self.0) }
            pub fn to_hex(&self) -> String { hex_encode(&self.0) }
            pub fn from_hex(s: &str) -> CoreResult<Self> {
                let b = hex_decode(s)?;
                if b.len() != 16 {
                    return Err(CoreError::new(ErrorCode::NonCanonicalEncoding,
                        format!("{} must be 32 hex chars", stringify!($name))));
                }
                let mut a = [0u8; 16];
                a.copy_from_slice(&b);
                Ok($name(a))
            }
            /// Deterministic derivation for fixtures/tests: `SHA-256(label)[..16]`.
            pub fn derive(label: &str) -> Self {
                let h = crate::hash::domain_hash(concat!("astra.id.", stringify!($name)), label.as_bytes());
                let mut a = [0u8; 16];
                a.copy_from_slice(&h.0[..16]);
                $name(a)
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), &self.to_hex()[..8])
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.to_hex()) }
        }
        impl Canonical for $name {
            fn to_canon(&self) -> CanonValue { CanonValue::Str(self.to_hex()) }
            fn from_canon(v: &CanonValue) -> CoreResult<Self> { Self::from_hex(v.as_str()?) }
        }
    };
}

id128!(
    #[doc = "Trust and identity namespace of one cluster genesis."]
    ClusterId
);
id128!(
    #[doc = "Immutable tenant namespace."]
    TenantId
);
id128!(
    #[doc = "Tenant-owned request namespace with non-reuse lifecycle."]
    RequestNamespace
);
id128!(
    #[doc = "One logical invocation inside a namespace."]
    StableRequestId
);
id128!(
    #[doc = "One transport attempt; excluded from request identity."]
    AttemptId
);
id128!(
    #[doc = "Registered immutable node identity."]
    NodeId
);
id128!(
    #[doc = "Request identity/admission owner (SPEC-012 §3)."]
    RequestHomeId
);
id128!(
    #[doc = "Logical plan lineage identity."]
    PlanId
);
id128!(
    #[doc = "Stable semantic domain identity (IDC)."]
    IdcId
);
id128!(
    #[doc = "Logical rights holder."]
    HolderId
);
id128!(
    #[doc = "One durable migration procedure."]
    MigrationId
);
id128!(
    #[doc = "Logical replication group."]
    ReplicationGroupId
);
id128!(
    #[doc = "Registered runtime authority (consensus group / certifier / sequencer)."]
    AuthorityId
);
id128!(
    #[doc = "One authority grant."]
    GrantId
);
id128!(
    #[doc = "Immutable security policy version."]
    SecurityPolicyId
);
id128!(
    #[doc = "Admin command identity for catalog CAS."]
    AdminRequestId
);
id128!(
    #[doc = "Client/administrator principal."]
    PrincipalId
);
id128!(
    #[doc = "Rights transfer identity."]
    TransferId
);
id128!(
    #[doc = "Business reservation identity."]
    ReservationId
);
id128!(
    #[doc = "Local storage instance identity."]
    StorageId
);
id128!(
    #[doc = "Semantic scope identity used by catalog locks."]
    SemanticScopeId
);
id128!(
    #[doc = "Registered credential."]
    CredentialId
);
id128!(
    #[doc = "UUID value type used by the DSL."]
    Uuid
);

// ---------------------------------------------------------------------------
// u64 generations / epochs / sequences (distinct newtypes, no cross conversion)
// ---------------------------------------------------------------------------

macro_rules! epoch64 {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub u64);

        impl $name {
            pub const ZERO: $name = $name(0);
            /// Successor value; overflow is an error, never a wrap.
            pub fn checked_next(&self) -> CoreResult<Self> {
                self.0.checked_add(1).map($name).ok_or_else(|| CoreError::new(
                    ErrorCode::NumericOverflow, concat!(stringify!($name), " exhausted")))
            }
            pub fn value(&self) -> u64 { self.0 }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
        }
        impl Canonical for $name {
            fn to_canon(&self) -> CanonValue { CanonValue::uint(self.0) }
            fn from_canon(v: &CanonValue) -> CoreResult<Self> { Ok($name(v.as_u64()?)) }
        }
    };
}

epoch64!(
    #[doc = "Committed catalog state revision."]
    CatalogGeneration
);
epoch64!(
    #[doc = "Immutable successor plan publication within a lineage."]
    PlanGeneration
);
epoch64!(
    #[doc = "Semantic IDC definition version."]
    IdcGeneration
);
epoch64!(
    #[doc = "IDC decision/admission authority incarnation."]
    IdcAuthorityEpoch
);
epoch64!(
    #[doc = "Physical placement version."]
    PlacementEpoch
);
epoch64!(
    #[doc = "Logical replication membership version."]
    MembershipGeneration
);
epoch64!(
    #[doc = "Resource definition incarnation."]
    ResourceGeneration
);
epoch64!(
    #[doc = "Conserved allocation-manifest lineage."]
    EscrowEpoch
);
epoch64!(
    #[doc = "Exclusive holder authority incarnation."]
    HolderAuthorityEpoch
);
epoch64!(
    #[doc = "One local durable storage history."]
    StorageEpoch
);
epoch64!(
    #[doc = "Semantic origin allocator incarnation."]
    OriginEpoch
);
epoch64!(
    #[doc = "Monotonic semantic-commit counter within an origin epoch."]
    OriginSeq
);
epoch64!(
    #[doc = "RequestHome ownership incarnation."]
    RequestHomeEpoch
);
epoch64!(
    #[doc = "Local MVCC commit sequence; never a distributed timestamp."]
    LocalCommitSeq
);
epoch64!(
    #[doc = "Ordered semantic position within one IdcBinding."]
    SerialPosition
);
epoch64!(
    #[doc = "Monotonic request allocation counter at one home/epoch."]
    RequestAllocationSeq
);
epoch64!(
    #[doc = "Protocol-record CAS revision."]
    RecordRevision
);
epoch64!(
    #[doc = "Physical journal position within one storage epoch."]
    JournalLsn
);
epoch64!(
    #[doc = "Dot sequence within a replication stream (SPEC-005)."]
    DotSeq
);

// ---------------------------------------------------------------------------
// Catalog-allocated small stable identities of language declarations (SPEC-003 §3.1)
// ---------------------------------------------------------------------------

macro_rules! stable_u64_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub u64);
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}#{}", stringify!($name), self.0)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
        }
        impl Canonical for $name {
            fn to_canon(&self) -> CanonValue { CanonValue::uint(self.0) }
            fn from_canon(v: &CanonValue) -> CoreResult<Self> { Ok($name(v.as_u64()?)) }
        }
    };
}

stable_u64_id!(
    #[doc = "Stable operation identity."]
    OperationId
);
stable_u64_id!(
    #[doc = "Stable record (table) identity."]
    RecordId
);
stable_u64_id!(
    #[doc = "Stable field identity within a record."]
    FieldId
);
stable_u64_id!(
    #[doc = "Stable invariant identity."]
    InvariantId
);
stable_u64_id!(
    #[doc = "Stable index identity."]
    IndexId
);
stable_u64_id!(
    #[doc = "Stable enum type identity."]
    EnumId
);
stable_u64_id!(
    #[doc = "Stable resource declaration identity."]
    ResourceDeclId
);
stable_u64_id!(
    #[doc = "Protocol template identity in the compiler library."]
    TemplateId
);

// ---------------------------------------------------------------------------
// Hash newtypes (all 32 bytes, 64 hex chars)
// ---------------------------------------------------------------------------

macro_rules! hash_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub Hash256);
        impl $name {
            pub fn to_hex(&self) -> String { self.0.to_hex() }
            pub fn from_hex(s: &str) -> CoreResult<Self> { Ok($name(Hash256::from_hex(s)?)) }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), &self.0.to_hex()[..12])
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.to_hex()) }
        }
        impl Canonical for $name {
            fn to_canon(&self) -> CanonValue { CanonValue::Str(self.to_hex()) }
            fn from_canon(v: &CanonValue) -> CoreResult<Self> { Self::from_hex(v.as_str()?) }
        }
    };
}

hash_newtype!(
    #[doc = "`astra.schema.v1` digest."]
    SchemaHash
);
hash_newtype!(
    #[doc = "`astra.operation.v1` digest."]
    OperationHash
);
hash_newtype!(
    #[doc = "`astra.contract.v1` digest."]
    ContractHash
);
hash_newtype!(
    #[doc = "`astra.invariant.v1` digest."]
    InvariantHash
);
hash_newtype!(
    #[doc = "`astra.module.v1` digest."]
    ModuleHash
);
hash_newtype!(
    #[doc = "`astra.plan.v1` digest."]
    PlanHash
);
hash_newtype!(
    #[doc = "`astra.certificate.v1` digest."]
    CertificateHash
);
hash_newtype!(
    #[doc = "`astra.request.v1` digest (SPEC-012 §2)."]
    RequestHash
);
hash_newtype!(
    #[doc = "`astra.batch.v1` digest of a CompiledBatch semantic payload (SPEC-002 §41)."]
    SemanticDigest
);
hash_newtype!(
    #[doc = "`astra.result.v1` digest."]
    ResultDigest
);
hash_newtype!(
    #[doc = "`astra.receipt.v1` digest."]
    ReceiptDigest
);

// ---------------------------------------------------------------------------
// Composite identities
// ---------------------------------------------------------------------------

/// `RequestKey = (TenantId, RequestNamespace, StableRequestId)` (SPEC-012 §2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RequestKey {
    pub tenant_id: TenantId,
    pub request_namespace: RequestNamespace,
    pub stable_request_id: StableRequestId,
}

impl Canonical for RequestKey {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("request_namespace", &self.request_namespace)
            .fc("stable_request_id", &self.stable_request_id)
            .fc("tenant_id", &self.tenant_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["request_namespace", "stable_request_id", "tenant_id"])?;
        Ok(RequestKey {
            tenant_id: TenantId::from_canon(v.field("tenant_id")?)?,
            request_namespace: RequestNamespace::from_canon(v.field("request_namespace")?)?,
            stable_request_id: StableRequestId::from_canon(v.field("stable_request_id")?)?,
        })
    }
}

/// `TxnId` is the exact 256-bit concatenation `RequestHomeId (128) || RequestHomeEpoch (u64 BE) || RequestAllocationSeq (u64 BE)` (SPEC-012 §3).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct TxnId(pub [u8; 32]);

impl TxnId {
    pub fn new(home: RequestHomeId, epoch: RequestHomeEpoch, seq: RequestAllocationSeq) -> TxnId {
        let mut b = [0u8; 32];
        b[..16].copy_from_slice(&home.0);
        b[16..24].copy_from_slice(&epoch.0.to_be_bytes());
        b[24..32].copy_from_slice(&seq.0.to_be_bytes());
        TxnId(b)
    }
    pub fn home(&self) -> RequestHomeId {
        let mut a = [0u8; 16];
        a.copy_from_slice(&self.0[..16]);
        RequestHomeId(a)
    }
    pub fn epoch(&self) -> RequestHomeEpoch {
        let mut a = [0u8; 8];
        a.copy_from_slice(&self.0[16..24]);
        RequestHomeEpoch(u64::from_be_bytes(a))
    }
    pub fn seq(&self) -> RequestAllocationSeq {
        let mut a = [0u8; 8];
        a.copy_from_slice(&self.0[24..32]);
        RequestAllocationSeq(u64::from_be_bytes(a))
    }
    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }
    pub fn from_hex(s: &str) -> CoreResult<TxnId> {
        let b = hex_decode(s)?;
        if b.len() != 32 {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "TxnId must be 64 hex chars",
            ));
        }
        let mut a = [0u8; 32];
        a.copy_from_slice(&b);
        Ok(TxnId(a))
    }
}

impl fmt::Debug for TxnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TxnId({}/{}/{})",
            &self.home().to_hex()[..8],
            self.epoch(),
            self.seq()
        )
    }
}
impl fmt::Display for TxnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}
impl Canonical for TxnId {
    fn to_canon(&self) -> CanonValue {
        CanonValue::Str(self.to_hex())
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        TxnId::from_hex(v.as_str()?)
    }
}

/// `OperationRef { operation_id, version }` (SPEC-012 §2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct OperationRef {
    pub operation_id: OperationId,
    pub version: u32,
}

impl Canonical for OperationRef {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("operation_id", &self.operation_id)
            .fu32("version", self.version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["operation_id", "version"])?;
        Ok(OperationRef {
            operation_id: OperationId::from_canon(v.field("operation_id")?)?,
            version: v.field("version")?.as_u32()?,
        })
    }
}

/// `IdcBinding { idc_id, idc_generation, authority_epoch }` (SPEC-011 §2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct IdcBinding {
    pub idc_id: IdcId,
    pub idc_generation: IdcGeneration,
    pub authority_epoch: IdcAuthorityEpoch,
}

impl Canonical for IdcBinding {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("authority_epoch", &self.authority_epoch)
            .fc("idc_generation", &self.idc_generation)
            .fc("idc_id", &self.idc_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["authority_epoch", "idc_generation", "idc_id"])?;
        Ok(IdcBinding {
            idc_id: IdcId::from_canon(v.field("idc_id")?)?,
            idc_generation: IdcGeneration::from_canon(v.field("idc_generation")?)?,
            authority_epoch: IdcAuthorityEpoch::from_canon(v.field("authority_epoch")?)?,
        })
    }
}

/// `PlanRef { plan_id, generation, hash }` (SPEC-011 §2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct PlanRef {
    pub plan_id: PlanId,
    pub generation: PlanGeneration,
    pub hash: PlanHash,
}

impl Canonical for PlanRef {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("generation", &self.generation)
            .fc("hash", &self.hash)
            .fc("plan_id", &self.plan_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["generation", "hash", "plan_id"])?;
        Ok(PlanRef {
            plan_id: PlanId::from_canon(v.field("plan_id")?)?,
            generation: PlanGeneration::from_canon(v.field("generation")?)?,
            hash: PlanHash::from_canon(v.field("hash")?)?,
        })
    }
}

/// `OriginId { node_id, origin_epoch, origin_seq }` (SPEC-002 §102).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct OriginId {
    pub node_id: NodeId,
    pub origin_epoch: OriginEpoch,
    pub origin_seq: OriginSeq,
}

impl Canonical for OriginId {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("node_id", &self.node_id)
            .fc("origin_epoch", &self.origin_epoch)
            .fc("origin_seq", &self.origin_seq)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["node_id", "origin_epoch", "origin_seq"])?;
        Ok(OriginId {
            node_id: NodeId::from_canon(v.field("node_id")?)?,
            origin_epoch: OriginEpoch::from_canon(v.field("origin_epoch")?)?,
            origin_seq: OriginSeq::from_canon(v.field("origin_seq")?)?,
        })
    }
}

/// `ResourceRef { invariant_id, resource_key, resource_generation }` (SPEC-006 §4).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct ResourceRef {
    pub invariant_id: InvariantId,
    /// Canonical ordered key bytes of the resource instance (SPEC-002 §11 codec).
    pub resource_key: Vec<u8>,
    pub resource_generation: ResourceGeneration,
}

impl Canonical for ResourceRef {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("invariant_id", &self.invariant_id)
            .fc("resource_generation", &self.resource_generation)
            .fbytes("resource_key", &self.resource_key)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["invariant_id", "resource_generation", "resource_key"])?;
        Ok(ResourceRef {
            invariant_id: InvariantId::from_canon(v.field("invariant_id")?)?,
            resource_key: v.field("resource_key")?.as_bytes()?,
            resource_generation: ResourceGeneration::from_canon(v.field("resource_generation")?)?,
        })
    }
}

/// `HolderRef { holder_id, authority_epoch }` (SPEC-006 §4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct HolderRef {
    pub holder_id: HolderId,
    pub authority_epoch: HolderAuthorityEpoch,
}

impl Canonical for HolderRef {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("authority_epoch", &self.authority_epoch)
            .fc("holder_id", &self.holder_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["authority_epoch", "holder_id"])?;
        Ok(HolderRef {
            holder_id: HolderId::from_canon(v.field("holder_id")?)?,
            authority_epoch: HolderAuthorityEpoch::from_canon(v.field("authority_epoch")?)?,
        })
    }
}

/// Tagged authority identity (SPEC-011 §2). No generic integer epoch exists.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum AuthorityBinding {
    Idc {
        authority_id: AuthorityId,
        idc: IdcBinding,
    },
    Holder {
        resource: ResourceRef,
        escrow_epoch: EscrowEpoch,
        holder_id: HolderId,
        epoch: HolderAuthorityEpoch,
    },
    RequestHome {
        home_id: RequestHomeId,
        epoch: RequestHomeEpoch,
    },
}

impl Canonical for AuthorityBinding {
    fn to_canon(&self) -> CanonValue {
        match self {
            AuthorityBinding::Idc { authority_id, idc } => CanonValue::obj()
                .fc("authority_id", authority_id)
                .fc("idc", idc)
                .fstr("kind", "idc")
                .build(),
            AuthorityBinding::Holder {
                resource,
                escrow_epoch,
                holder_id,
                epoch,
            } => CanonValue::obj()
                .fc("epoch", epoch)
                .fc("escrow_epoch", escrow_epoch)
                .fc("holder_id", holder_id)
                .fstr("kind", "holder")
                .fc("resource", resource)
                .build(),
            AuthorityBinding::RequestHome { home_id, epoch } => CanonValue::obj()
                .fc("epoch", epoch)
                .fc("home_id", home_id)
                .fstr("kind", "request_home")
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        match v.field("kind")?.as_str()? {
            "idc" => {
                v.expect_fields(&["authority_id", "idc", "kind"])?;
                Ok(AuthorityBinding::Idc {
                    authority_id: AuthorityId::from_canon(v.field("authority_id")?)?,
                    idc: IdcBinding::from_canon(v.field("idc")?)?,
                })
            }
            "holder" => {
                v.expect_fields(&["epoch", "escrow_epoch", "holder_id", "kind", "resource"])?;
                Ok(AuthorityBinding::Holder {
                    resource: ResourceRef::from_canon(v.field("resource")?)?,
                    escrow_epoch: EscrowEpoch::from_canon(v.field("escrow_epoch")?)?,
                    holder_id: HolderId::from_canon(v.field("holder_id")?)?,
                    epoch: HolderAuthorityEpoch::from_canon(v.field("epoch")?)?,
                })
            }
            "request_home" => {
                v.expect_fields(&["epoch", "home_id", "kind"])?;
                Ok(AuthorityBinding::RequestHome {
                    home_id: RequestHomeId::from_canon(v.field("home_id")?)?,
                    epoch: RequestHomeEpoch::from_canon(v.field("epoch")?)?,
                })
            }
            other => Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                format!("unknown authority kind `{other}`"),
            )),
        }
    }
}

/// `ProtocolRecordRef` binds record kind, schema version, stable record key and canonical payload hash (SPEC-012 §7).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct ProtocolRecordRef {
    pub record_kind: String,
    pub record_version: u32,
    pub record_key: Vec<u8>,
    pub payload_hash: Hash256,
}

impl Canonical for ProtocolRecordRef {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .f("payload_hash", CanonValue::Str(self.payload_hash.to_hex()))
            .fbytes("record_key", &self.record_key)
            .fstr("record_kind", &self.record_kind)
            .fu32("record_version", self.record_version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "payload_hash",
            "record_key",
            "record_kind",
            "record_version",
        ])?;
        Ok(ProtocolRecordRef {
            record_kind: v.field("record_kind")?.as_str()?.to_string(),
            record_version: v.field("record_version")?.as_u32()?,
            record_key: v.field("record_key")?.as_bytes()?,
            payload_hash: Hash256::from_hex(v.field("payload_hash")?.as_str()?)?,
        })
    }
}

impl Canonical for Hash256 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::Str(self.to_hex())
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Hash256::from_hex(v.as_str()?)
    }
}

/// Protocol family label (SPEC-004 §3). A dispatch/metrics summary; never a proof.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum ConsistencyClass {
    C0Local,
    C1Commutative,
    C2Causal,
    C3Escrow,
    C4Certified,
    C5Serial,
}

impl ConsistencyClass {
    pub fn label(&self) -> &'static str {
        match self {
            ConsistencyClass::C0Local => "C0_LOCAL",
            ConsistencyClass::C1Commutative => "C1_COMMUTATIVE",
            ConsistencyClass::C2Causal => "C2_CAUSAL",
            ConsistencyClass::C3Escrow => "C3_ESCROW",
            ConsistencyClass::C4Certified => "C4_CERTIFIED",
            ConsistencyClass::C5Serial => "C5_SERIAL",
        }
    }
    pub fn from_label(s: &str) -> CoreResult<Self> {
        Ok(match s {
            "C0_LOCAL" => ConsistencyClass::C0Local,
            "C1_COMMUTATIVE" => ConsistencyClass::C1Commutative,
            "C2_CAUSAL" => ConsistencyClass::C2Causal,
            "C3_ESCROW" => ConsistencyClass::C3Escrow,
            "C4_CERTIFIED" => ConsistencyClass::C4Certified,
            "C5_SERIAL" => ConsistencyClass::C5Serial,
            _ => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("unknown consistency class `{s}`"),
                ))
            }
        })
    }
}

impl Canonical for ConsistencyClass {
    fn to_canon(&self) -> CanonValue {
        CanonValue::str(self.label())
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        ConsistencyClass::from_label(v.as_str()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::Limits;

    #[test]
    fn txn_id_layout_is_big_endian_concatenation() {
        let home = RequestHomeId::from_u128(0x0102030405060708090a0b0c0d0e0f10);
        let t = TxnId::new(home, RequestHomeEpoch(2), RequestAllocationSeq(7));
        assert_eq!(
            t.to_hex(),
            "0102030405060708090a0b0c0d0e0f1000000000000000020000000000000007"
        );
        assert_eq!(t.home(), home);
        assert_eq!(t.epoch(), RequestHomeEpoch(2));
        assert_eq!(t.seq(), RequestAllocationSeq(7));
        assert_eq!(TxnId::from_hex(&t.to_hex()).unwrap(), t);
    }

    #[test]
    fn epochs_are_not_interchangeable_and_do_not_wrap() {
        // This does not compile by design: `let _: IdcAuthorityEpoch = IdcGeneration(1);`
        assert_eq!(
            IdcGeneration(u64::MAX).checked_next().unwrap_err().code,
            ErrorCode::NumericOverflow
        );
        assert_eq!(IdcGeneration(4).checked_next().unwrap(), IdcGeneration(5));
    }

    #[test]
    fn composite_canonical_roundtrip() {
        let b = IdcBinding {
            idc_id: IdcId::derive("inv"),
            idc_generation: IdcGeneration(3),
            authority_epoch: IdcAuthorityEpoch(9),
        };
        let bytes = b.encode();
        assert!(String::from_utf8(bytes.clone())
            .unwrap()
            .starts_with(r#"{"authority_epoch":"9","idc_generation":"3","idc_id":""#));
        assert_eq!(IdcBinding::decode(&bytes, &Limits::v1()).unwrap(), b);

        let a = AuthorityBinding::Holder {
            resource: ResourceRef {
                invariant_id: InvariantId(1),
                resource_key: vec![1, 2],
                resource_generation: ResourceGeneration(1),
            },
            escrow_epoch: EscrowEpoch(1),
            holder_id: HolderId::derive("h"),
            epoch: HolderAuthorityEpoch(2),
        };
        assert_eq!(
            AuthorityBinding::decode(&a.encode(), &Limits::v1()).unwrap(),
            a
        );

        let k = RequestKey {
            tenant_id: TenantId::derive("t"),
            request_namespace: RequestNamespace::derive("n"),
            stable_request_id: StableRequestId::derive("r"),
        };
        assert_eq!(RequestKey::decode(&k.encode(), &Limits::v1()).unwrap(), k);
        // typed-binding substitution: an object missing a field is rejected
        let mut o = k.to_canon().as_object().unwrap().clone();
        o.remove("tenant_id");
        assert!(RequestKey::from_canon(&CanonValue::Object(o)).is_err());
    }

    #[test]
    fn id128_hex_strict() {
        assert!(TenantId::from_hex("00").is_err());
        let t = TenantId::derive("x");
        assert_eq!(TenantId::from_hex(&t.to_hex()).unwrap(), t);
        assert!(TenantId::from_hex(&t.to_hex().to_uppercase()).is_err());
    }
}
