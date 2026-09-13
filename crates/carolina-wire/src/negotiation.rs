//! Hello/HelloAck negotiation with downgrade protection (SPEC-012 §10).
//!
//! HelloAck may only select versions/codecs/capabilities the peer offered, and limits become the
//! minimum of both sides. The transcript hash (`astra.negotiation.v1`) over the ordered
//! offered/selected records binds the exchange; SPEC-013 authenticates it inside the channel.

use carolina_core::canon::{decode_set, CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, domains, Hash256};
use carolina_core::ids::ClusterId;
use carolina_core::limits::Limits;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointRole {
    Client,
    Node,
    Admin,
}

impl EndpointRole {
    fn label(&self) -> &'static str {
        match self {
            EndpointRole::Client => "client",
            EndpointRole::Node => "node",
            EndpointRole::Admin => "admin",
        }
    }
    fn from_label(s: &str) -> CoreResult<Self> {
        Ok(match s {
            "client" => EndpointRole::Client,
            "node" => EndpointRole::Node,
            "admin" => EndpointRole::Admin,
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("role {k}"),
                ))
            }
        })
    }
}

/// Advertised limits (subset that negotiation takes minima over).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedLimits {
    pub max_frame_payload: u64,
    pub max_arguments_bytes: u64,
    pub max_result_bytes: u64,
    pub max_token_bytes: u64,
}

impl AdvertisedLimits {
    pub fn from_limits(l: &Limits) -> Self {
        AdvertisedLimits {
            max_frame_payload: l.max_frame_payload as u64,
            max_arguments_bytes: l.max_arguments_bytes as u64,
            max_result_bytes: l.max_result_bytes as u64,
            max_token_bytes: l.max_token_bytes as u64,
        }
    }
    pub fn min(&self, other: &AdvertisedLimits) -> AdvertisedLimits {
        AdvertisedLimits {
            max_frame_payload: self.max_frame_payload.min(other.max_frame_payload),
            max_arguments_bytes: self.max_arguments_bytes.min(other.max_arguments_bytes),
            max_result_bytes: self.max_result_bytes.min(other.max_result_bytes),
            max_token_bytes: self.max_token_bytes.min(other.max_token_bytes),
        }
    }
}

impl Canonical for AdvertisedLimits {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fu64("max_arguments_bytes", self.max_arguments_bytes)
            .fu64("max_frame_payload", self.max_frame_payload)
            .fu64("max_result_bytes", self.max_result_bytes)
            .fu64("max_token_bytes", self.max_token_bytes)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "max_arguments_bytes",
            "max_frame_payload",
            "max_result_bytes",
            "max_token_bytes",
        ])?;
        Ok(AdvertisedLimits {
            max_frame_payload: v.field("max_frame_payload")?.as_u64()?,
            max_arguments_bytes: v.field("max_arguments_bytes")?.as_u64()?,
            max_result_bytes: v.field("max_result_bytes")?.as_u64()?,
            max_token_bytes: v.field("max_token_bytes")?.as_u64()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloV1 {
    pub cluster_id: ClusterId,
    pub role: EndpointRole,
    pub nonce: Vec<u8>,
    pub wire_versions: Vec<String>,
    pub record_codecs: Vec<String>,
    pub ir_versions: Vec<String>,
    pub plan_versions: Vec<String>,
    pub protocol_capabilities: Vec<String>,
    pub limits: AdvertisedLimits,
    pub capability_manifest_ref: Hash256,
    pub minimum_security_profile: String,
}

impl Canonical for HelloV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("capability_manifest_ref", &self.capability_manifest_ref)
            .fc("cluster_id", &self.cluster_id)
            .fset("ir_versions", &self.ir_versions)
            .fc("limits", &self.limits)
            .fstr("minimum_security_profile", &self.minimum_security_profile)
            .fbytes("nonce", &self.nonce)
            .fset("plan_versions", &self.plan_versions)
            .fset("protocol_capabilities", &self.protocol_capabilities)
            .fset("record_codecs", &self.record_codecs)
            .fstr("role", self.role.label())
            .fset("wire_versions", &self.wire_versions)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "capability_manifest_ref",
            "cluster_id",
            "ir_versions",
            "limits",
            "minimum_security_profile",
            "nonce",
            "plan_versions",
            "protocol_capabilities",
            "record_codecs",
            "role",
            "wire_versions",
        ])?;
        Ok(HelloV1 {
            cluster_id: ClusterId::from_canon(v.field("cluster_id")?)?,
            role: EndpointRole::from_label(v.field("role")?.as_str()?)?,
            nonce: v.field("nonce")?.as_bytes()?,
            wire_versions: decode_set(v.field("wire_versions")?)?,
            record_codecs: decode_set(v.field("record_codecs")?)?,
            ir_versions: decode_set(v.field("ir_versions")?)?,
            plan_versions: decode_set(v.field("plan_versions")?)?,
            protocol_capabilities: decode_set(v.field("protocol_capabilities")?)?,
            limits: AdvertisedLimits::from_canon(v.field("limits")?)?,
            capability_manifest_ref: Hash256::from_canon(v.field("capability_manifest_ref")?)?,
            minimum_security_profile: v.field("minimum_security_profile")?.as_str()?.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloAckV1 {
    pub cluster_id: ClusterId,
    pub role: EndpointRole,
    pub nonce: Vec<u8>,
    pub peer_nonce: Vec<u8>,
    pub selected_wire_version: String,
    pub selected_record_codecs: Vec<String>,
    pub selected_ir_version: String,
    pub selected_plan_version: String,
    pub selected_capabilities: Vec<String>,
    pub limits: AdvertisedLimits,
    pub capability_manifest_ref: Hash256,
    pub security_profile: String,
}

impl Canonical for HelloAckV1 {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("capability_manifest_ref", &self.capability_manifest_ref)
            .fc("cluster_id", &self.cluster_id)
            .fc("limits", &self.limits)
            .fbytes("nonce", &self.nonce)
            .fbytes("peer_nonce", &self.peer_nonce)
            .fstr("role", self.role.label())
            .fstr("security_profile", &self.security_profile)
            .fset("selected_capabilities", &self.selected_capabilities)
            .fstr("selected_ir_version", &self.selected_ir_version)
            .fstr("selected_plan_version", &self.selected_plan_version)
            .fset("selected_record_codecs", &self.selected_record_codecs)
            .fstr("selected_wire_version", &self.selected_wire_version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "capability_manifest_ref",
            "cluster_id",
            "limits",
            "nonce",
            "peer_nonce",
            "role",
            "security_profile",
            "selected_capabilities",
            "selected_ir_version",
            "selected_plan_version",
            "selected_record_codecs",
            "selected_wire_version",
        ])?;
        Ok(HelloAckV1 {
            cluster_id: ClusterId::from_canon(v.field("cluster_id")?)?,
            role: EndpointRole::from_label(v.field("role")?.as_str()?)?,
            nonce: v.field("nonce")?.as_bytes()?,
            peer_nonce: v.field("peer_nonce")?.as_bytes()?,
            selected_wire_version: v.field("selected_wire_version")?.as_str()?.to_string(),
            selected_record_codecs: decode_set(v.field("selected_record_codecs")?)?,
            selected_ir_version: v.field("selected_ir_version")?.as_str()?.to_string(),
            selected_plan_version: v.field("selected_plan_version")?.as_str()?.to_string(),
            selected_capabilities: decode_set(v.field("selected_capabilities")?)?,
            limits: AdvertisedLimits::from_canon(v.field("limits")?)?,
            capability_manifest_ref: Hash256::from_canon(v.field("capability_manifest_ref")?)?,
            security_profile: v.field("security_profile")?.as_str()?.to_string(),
        })
    }
}

/// Local capabilities used to answer a Hello.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCapabilities {
    pub cluster_id: ClusterId,
    pub role: EndpointRole,
    pub wire_versions: Vec<String>,
    pub record_codecs: Vec<String>,
    pub ir_versions: Vec<String>,
    pub plan_versions: Vec<String>,
    pub protocol_capabilities: Vec<String>,
    pub limits: AdvertisedLimits,
    pub capability_manifest_ref: Hash256,
    pub security_profile: String,
    pub minimum_security_profile: String,
}

impl LocalCapabilities {
    pub fn v1(
        cluster_id: ClusterId,
        role: EndpointRole,
        limits: &Limits,
        security_profile: &str,
    ) -> Self {
        let mut codecs: Vec<String> = crate::registry::REGISTERED_KINDS
            .iter()
            .map(|(k, v, _)| format!("{k}:{v}"))
            .collect();
        codecs.sort();
        LocalCapabilities {
            cluster_id,
            role,
            wire_versions: vec!["1.0".into()],
            record_codecs: codecs,
            ir_versions: vec!["1".into()],
            plan_versions: vec!["1".into()],
            protocol_capabilities: vec![
                "C0_LOCAL_V1:1".into(),
                "C5_SERIAL_V1:1".into(),
                "resolve:1".into(),
            ],
            limits: AdvertisedLimits::from_limits(limits),
            capability_manifest_ref: Hash256::ZERO,
            security_profile: security_profile.into(),
            minimum_security_profile: security_profile.into(),
        }
    }

    pub fn hello(&self, nonce: Vec<u8>) -> HelloV1 {
        HelloV1 {
            cluster_id: self.cluster_id,
            role: self.role,
            nonce,
            wire_versions: self.wire_versions.clone(),
            record_codecs: self.record_codecs.clone(),
            ir_versions: self.ir_versions.clone(),
            plan_versions: self.plan_versions.clone(),
            protocol_capabilities: self.protocol_capabilities.clone(),
            limits: self.limits.clone(),
            capability_manifest_ref: self.capability_manifest_ref,
            minimum_security_profile: self.minimum_security_profile.clone(),
        }
    }

    /// Answer a Hello: intersect offers, take minima of limits, refuse on incompatibility.
    pub fn answer(&self, hello: &HelloV1, nonce: Vec<u8>) -> CoreResult<HelloAckV1> {
        if hello.cluster_id != self.cluster_id {
            return Err(CoreError::new(
                ErrorCode::IncompatiblePeer,
                "peer belongs to another cluster",
            ));
        }
        let pick_common =
            |mine: &[String], theirs: &[String], what: &str| -> CoreResult<Vec<String>> {
                let common: Vec<String> = mine
                    .iter()
                    .filter(|m| theirs.contains(m))
                    .cloned()
                    .collect();
                if common.is_empty() {
                    return Err(CoreError::new(
                        ErrorCode::UnsupportedCodec,
                        format!("no common {what}"),
                    ));
                }
                Ok(common)
            };
        let wire = pick_common(&self.wire_versions, &hello.wire_versions, "wire version")?;
        let ir = pick_common(&self.ir_versions, &hello.ir_versions, "IR version")?;
        let plan = pick_common(&self.plan_versions, &hello.plan_versions, "plan version")?;
        let codecs = pick_common(&self.record_codecs, &hello.record_codecs, "record codec")?;
        let caps: Vec<String> = self
            .protocol_capabilities
            .iter()
            .filter(|c| hello.protocol_capabilities.contains(c))
            .cloned()
            .collect();
        if hello.minimum_security_profile != self.security_profile
            && !security_profile_at_least(&self.security_profile, &hello.minimum_security_profile)
        {
            return Err(CoreError::new(
                ErrorCode::IncompatiblePeer,
                "peer requires a stronger security profile",
            ));
        }
        Ok(HelloAckV1 {
            cluster_id: self.cluster_id,
            role: self.role,
            nonce,
            peer_nonce: hello.nonce.clone(),
            selected_wire_version: wire.iter().max().cloned().unwrap(),
            selected_record_codecs: codecs,
            selected_ir_version: ir.iter().max().cloned().unwrap(),
            selected_plan_version: plan.iter().max().cloned().unwrap(),
            selected_capabilities: caps,
            limits: self.limits.min(&hello.limits),
            capability_manifest_ref: self.capability_manifest_ref,
            security_profile: self.security_profile.clone(),
        })
    }

    /// Verify a HelloAck against the Hello we sent: nothing selected that we did not offer; limits ≤ ours.
    pub fn verify_ack(&self, sent: &HelloV1, ack: &HelloAckV1) -> CoreResult<()> {
        if ack.cluster_id != self.cluster_id || ack.peer_nonce != sent.nonce {
            return Err(CoreError::new(
                ErrorCode::IncompatiblePeer,
                "HelloAck does not answer our Hello",
            ));
        }
        if !sent.wire_versions.contains(&ack.selected_wire_version) {
            return Err(CoreError::new(
                ErrorCode::UnsupportedCodec,
                "peer selected an unoffered wire version",
            ));
        }
        if !sent.ir_versions.contains(&ack.selected_ir_version)
            || !sent.plan_versions.contains(&ack.selected_plan_version)
        {
            return Err(CoreError::new(
                ErrorCode::UnsupportedCodec,
                "peer selected an unoffered IR/plan version",
            ));
        }
        if ack
            .selected_record_codecs
            .iter()
            .any(|c| !sent.record_codecs.contains(c))
        {
            return Err(CoreError::new(
                ErrorCode::UnsupportedCodec,
                "peer selected an unoffered codec",
            ));
        }
        if ack
            .selected_capabilities
            .iter()
            .any(|c| !sent.protocol_capabilities.contains(c))
        {
            return Err(CoreError::new(
                ErrorCode::UnsupportedCodec,
                "peer selected an unoffered capability",
            ));
        }
        let l = &ack.limits;
        let s = &sent.limits;
        if l.max_frame_payload > s.max_frame_payload
            || l.max_arguments_bytes > s.max_arguments_bytes
            || l.max_result_bytes > s.max_result_bytes
            || l.max_token_bytes > s.max_token_bytes
        {
            return Err(CoreError::new(
                ErrorCode::ProtocolError,
                "peer exceeded our advertised limits",
            ));
        }
        if !security_profile_at_least(&ack.security_profile, &sent.minimum_security_profile) {
            return Err(CoreError::new(
                ErrorCode::IncompatiblePeer,
                "peer downgraded the security profile",
            ));
        }
        Ok(())
    }
}

/// Ordered security profiles (SPEC-013 §10). `DEV_LOCAL` can never satisfy a production minimum.
pub fn security_profile_at_least(actual: &str, minimum: &str) -> bool {
    let rank = |p: &str| match p {
        "DEV_LOCAL" => 0,
        "ENCRYPTED_HOST_V1" => 1,
        _ => -1,
    };
    rank(actual) >= 0 && rank(minimum) >= 0 && rank(actual) >= rank(minimum)
}

/// Transcript hash over the ordered (hello, ack) records.
pub fn transcript_hash(hello: &HelloV1, ack: &HelloAckV1) -> Hash256 {
    let t = CanonValue::obj()
        .fc("ack", ack)
        .fc("hello", hello)
        .fstr("kind", "negotiation.v1")
        .build();
    domain_hash(domains::NEGOTIATION_V1, &t.encode())
}
