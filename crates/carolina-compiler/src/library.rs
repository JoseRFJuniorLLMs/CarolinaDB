//! Finite, versioned protocol-template library (SPEC-004 §1, §3).
//!
//! A template is selectable only when it is *qualified* for the target milestone. Unqualified
//! templates are still analyzed so that EXPLAIN can report candidate eligibility, but they
//! yield `MissingRuntimeCapability` and can never be selected (SPEC-004 §9).

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, Hash256};
use carolina_core::ids::{ConsistencyClass, TemplateId};

/// Lexicographic cost descriptor (SPEC-004 §3): estimates, never measured latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CostDescriptor {
    pub remote_participants: u32,
    pub coordination_rounds: u32,
    pub background_transfer: u32,
    pub template_id: TemplateId,
}

impl Canonical for CostDescriptor {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fu32("background_transfer", self.background_transfer)
            .fu32("coordination_rounds", self.coordination_rounds)
            .fu32("remote_participants", self.remote_participants)
            .fc("template_id", &self.template_id)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(CostDescriptor {
            remote_participants: v.field("remote_participants")?.as_u32()?,
            coordination_rounds: v.field("coordination_rounds")?.as_u32()?,
            background_transfer: v.field("background_transfer")?.as_u32()?,
            template_id: TemplateId::from_canon(v.field("template_id")?)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolTemplate {
    pub id: TemplateId,
    pub name: String,
    pub version: u32,
    pub family: ConsistencyClass,
    /// Runtime qualification evidence exists for this template in the current implementation profile.
    pub qualified: bool,
    /// Milestone at which the template becomes selectable (SPEC-014).
    pub milestone: String,
    pub cost: CostDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolLibraryManifest {
    pub library_version: u32,
    pub templates: Vec<ProtocolTemplate>,
}

pub const T_C0_LOCAL: TemplateId = TemplateId(1);
pub const T_C1_COMMUTATIVE: TemplateId = TemplateId(2);
pub const T_C2_CAUSAL: TemplateId = TemplateId(3);
pub const T_C3_ESCROW: TemplateId = TemplateId(4);
pub const T_C4_CERTIFIED: TemplateId = TemplateId(5);
pub const T_C5_SERIAL: TemplateId = TemplateId(6);

impl ProtocolLibraryManifest {
    /// The v1 library. Only `C5_SERIAL_V1` and `C0_LOCAL_V1` are qualified in the MVP-1/MVP-2 local
    /// profile; C1/C2/C3/C4 templates exist for analysis and EXPLAIN but are not selectable.
    pub fn v1() -> ProtocolLibraryManifest {
        let t = |id: TemplateId,
                 name: &str,
                 family: ConsistencyClass,
                 qualified: bool,
                 milestone: &str,
                 rp: u32,
                 rounds: u32,
                 bg: u32| ProtocolTemplate {
            id,
            name: name.into(),
            version: 1,
            family,
            qualified,
            milestone: milestone.into(),
            cost: CostDescriptor {
                remote_participants: rp,
                coordination_rounds: rounds,
                background_transfer: bg,
                template_id: id,
            },
        };
        ProtocolLibraryManifest {
            library_version: 1,
            templates: vec![
                t(
                    T_C0_LOCAL,
                    "C0_LOCAL_V1",
                    ConsistencyClass::C0Local,
                    true,
                    "MVP-2",
                    0,
                    0,
                    0,
                ),
                t(
                    T_C1_COMMUTATIVE,
                    "C1_COMMUTATIVE_V1",
                    ConsistencyClass::C1Commutative,
                    false,
                    "MVP-5",
                    0,
                    0,
                    1,
                ),
                t(
                    T_C2_CAUSAL,
                    "C2_CAUSAL_V1",
                    ConsistencyClass::C2Causal,
                    false,
                    "MVP-5",
                    0,
                    0,
                    1,
                ),
                t(
                    T_C3_ESCROW,
                    "C3_ESCROW_V1",
                    ConsistencyClass::C3Escrow,
                    false,
                    "MVP-6",
                    0,
                    0,
                    2,
                ),
                t(
                    T_C4_CERTIFIED,
                    "C4_CERTIFIED_V1",
                    ConsistencyClass::C4Certified,
                    false,
                    "MVP-8",
                    1,
                    2,
                    0,
                ),
                t(
                    T_C5_SERIAL,
                    "C5_SERIAL_V1",
                    ConsistencyClass::C5Serial,
                    true,
                    "MVP-1",
                    1,
                    1,
                    0,
                ),
            ],
        }
    }

    pub fn template(&self, id: TemplateId) -> Option<&ProtocolTemplate> {
        self.templates.iter().find(|t| t.id == id)
    }

    pub fn hash(&self) -> Hash256 {
        domain_hash("astra.protocol-library.v1", &self.to_canon().encode())
    }
}

impl Canonical for ProtocolTemplate {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fc("cost", &self.cost)
            .fc("family", &self.family)
            .fc("id", &self.id)
            .fstr("milestone", &self.milestone)
            .fstr("name", &self.name)
            .fbool("qualified", self.qualified)
            .fu32("version", self.version)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(ProtocolTemplate {
            id: TemplateId::from_canon(v.field("id")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            version: v.field("version")?.as_u32()?,
            family: ConsistencyClass::from_canon(v.field("family")?)?,
            qualified: v.field("qualified")?.as_bool()?,
            milestone: v.field("milestone")?.as_str()?.to_string(),
            cost: CostDescriptor::from_canon(v.field("cost")?)?,
        })
    }
}

impl Canonical for ProtocolLibraryManifest {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("kind", "protocol-library.v1")
            .fu32("library_version", self.library_version)
            .fvec("templates", &self.templates)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        if v.field("kind")?.as_str()? != "protocol-library.v1" {
            return Err(CoreError::new(
                ErrorCode::NonCanonicalEncoding,
                "expected protocol-library.v1",
            ));
        }
        Ok(ProtocolLibraryManifest {
            library_version: v.field("library_version")?.as_u32()?,
            templates: Vec::from_canon(v.field("templates")?)?,
        })
    }
}
