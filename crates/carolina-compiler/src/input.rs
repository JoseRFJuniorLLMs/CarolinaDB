//! Compiler inputs (SPEC-004 §2): topology snapshot, protocol library, analysis rules, policy and budget.
//!
//! Every version, rule, capability and topology identity is an explicit input. A placement
//! snapshot is an assumption to validate at admission, not perpetual authority.

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, Hash256};
use carolina_core::ids::*;
use carolina_lang::ir::ModuleIR;

/// Kind of runtime authority a domain provides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityKind {
    /// One fenced exclusive local writer (C0 eligibility requires this plus admission fencing).
    ExclusiveLocal,
    /// Durable ordered authority (v1: one process or a Raft group) for C5.
    OrderedSerial { voters: u32 },
    /// Escrow holder authority (C3); not available before MVP-6.
    EscrowHolder,
    /// Replicated certifier (C4); not available before MVP-8.
    Certifier { voters: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityDomain {
    pub id: AuthorityId,
    pub name: String,
    pub kind: AuthorityKind,
    /// Records whose complete scope this authority covers (empty = all records of the module).
    pub covered_records: Vec<RecordId>,
    /// Nodes admitted to execute for this authority.
    pub admitted_nodes: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureDomainPolicy {
    pub name: String,
    pub required_durable_copies: u32,
    pub failure_domains: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub scope: SemanticScopeId,
    pub placement_epoch: PlacementEpoch,
    pub nodes: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCapability {
    pub node_id: NodeId,
    pub families: Vec<ConsistencyClass>,
    pub wire_versions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologySnapshot {
    pub placements: Vec<Placement>,
    pub authority_domains: Vec<AuthorityDomain>,
    pub durability_policies: Vec<FailureDomainPolicy>,
    pub node_capabilities: Vec<NodeCapability>,
}

impl TopologySnapshot {
    /// The MVP-1/MVP-2 local profile: one process, one exclusive+ordered local authority, `LocalStable` only.
    pub fn local_single_node() -> TopologySnapshot {
        let node = NodeId::derive("local-node");
        TopologySnapshot {
            placements: vec![Placement {
                scope: SemanticScopeId::derive("local-scope"),
                placement_epoch: PlacementEpoch(1),
                nodes: vec![node],
            }],
            authority_domains: vec![
                AuthorityDomain {
                    id: AuthorityId::derive("local-serial"),
                    name: "local-serial".into(),
                    kind: AuthorityKind::OrderedSerial { voters: 1 },
                    covered_records: vec![],
                    admitted_nodes: vec![node],
                },
                AuthorityDomain {
                    id: AuthorityId::derive("local-exclusive"),
                    name: "local-exclusive".into(),
                    kind: AuthorityKind::ExclusiveLocal,
                    covered_records: vec![],
                    admitted_nodes: vec![node],
                },
            ],
            durability_policies: vec![FailureDomainPolicy {
                name: "local".into(),
                required_durable_copies: 1,
                failure_domains: 1,
            }],
            node_capabilities: vec![NodeCapability {
                node_id: node,
                families: vec![ConsistencyClass::C0Local, ConsistencyClass::C5Serial],
                wire_versions: vec!["1.0".into()],
            }],
        }
    }

    /// The MVP-3 profile: fixed three-voter ordered authority, no exclusive local writer.
    pub fn three_node_serial() -> TopologySnapshot {
        let nodes: Vec<NodeId> = (1..=3)
            .map(|i| NodeId::derive(&format!("node-{i}")))
            .collect();
        TopologySnapshot {
            placements: vec![Placement {
                scope: SemanticScopeId::derive("cluster-scope"),
                placement_epoch: PlacementEpoch(1),
                nodes: nodes.clone(),
            }],
            authority_domains: vec![AuthorityDomain {
                id: AuthorityId::derive("raft-serial"),
                name: "raft-serial".into(),
                kind: AuthorityKind::OrderedSerial { voters: 3 },
                covered_records: vec![],
                admitted_nodes: nodes.clone(),
            }],
            durability_policies: vec![
                FailureDomainPolicy {
                    name: "local".into(),
                    required_durable_copies: 1,
                    failure_domains: 1,
                },
                FailureDomainPolicy {
                    name: "majority".into(),
                    required_durable_copies: 2,
                    failure_domains: 3,
                },
            ],
            node_capabilities: nodes
                .iter()
                .map(|n| NodeCapability {
                    node_id: *n,
                    families: vec![ConsistencyClass::C5Serial],
                    wire_versions: vec!["1.0".into()],
                })
                .collect(),
        }
    }

    pub fn topology_hash(&self) -> Hash256 {
        domain_hash("astra.topology.v1", &self.to_canon().encode())
    }

    pub fn policy(&self, name: &str) -> Option<&FailureDomainPolicy> {
        self.durability_policies.iter().find(|p| p.name == name)
    }
}

impl Canonical for AuthorityKind {
    fn to_canon(&self) -> CanonValue {
        match self {
            AuthorityKind::ExclusiveLocal => {
                CanonValue::obj().fstr("kind", "exclusive_local").build()
            }
            AuthorityKind::OrderedSerial { voters } => CanonValue::obj()
                .fstr("kind", "ordered_serial")
                .fu32("voters", *voters)
                .build(),
            AuthorityKind::EscrowHolder => CanonValue::obj().fstr("kind", "escrow_holder").build(),
            AuthorityKind::Certifier { voters } => CanonValue::obj()
                .fstr("kind", "certifier")
                .fu32("voters", *voters)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "exclusive_local" => AuthorityKind::ExclusiveLocal,
            "ordered_serial" => AuthorityKind::OrderedSerial {
                voters: v.field("voters")?.as_u32()?,
            },
            "escrow_holder" => AuthorityKind::EscrowHolder,
            "certifier" => AuthorityKind::Certifier {
                voters: v.field("voters")?.as_u32()?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("unknown authority kind {k}"),
                ))
            }
        })
    }
}

impl Canonical for AuthorityDomain {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fset("admitted_nodes", &self.admitted_nodes)
            .fset("covered_records", &self.covered_records)
            .fc("id", &self.id)
            .fc("kind", &self.kind)
            .fstr("name", &self.name)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(AuthorityDomain {
            id: AuthorityId::from_canon(v.field("id")?)?,
            name: v.field("name")?.as_str()?.to_string(),
            kind: AuthorityKind::from_canon(v.field("kind")?)?,
            covered_records: carolina_core::canon::decode_set(v.field("covered_records")?)?,
            admitted_nodes: carolina_core::canon::decode_set(v.field("admitted_nodes")?)?,
        })
    }
}

impl Canonical for FailureDomainPolicy {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fu32("failure_domains", self.failure_domains)
            .fstr("name", &self.name)
            .fu32("required_durable_copies", self.required_durable_copies)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(FailureDomainPolicy {
            name: v.field("name")?.as_str()?.to_string(),
            required_durable_copies: v.field("required_durable_copies")?.as_u32()?,
            failure_domains: v.field("failure_domains")?.as_u32()?,
        })
    }
}

impl Canonical for Placement {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fset("nodes", &self.nodes)
            .fc("placement_epoch", &self.placement_epoch)
            .fc("scope", &self.scope)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(Placement {
            scope: SemanticScopeId::from_canon(v.field("scope")?)?,
            placement_epoch: PlacementEpoch::from_canon(v.field("placement_epoch")?)?,
            nodes: carolina_core::canon::decode_set(v.field("nodes")?)?,
        })
    }
}

impl Canonical for NodeCapability {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fset("families", &self.families)
            .fc("node_id", &self.node_id)
            .fset("wire_versions", &self.wire_versions)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(NodeCapability {
            node_id: NodeId::from_canon(v.field("node_id")?)?,
            families: carolina_core::canon::decode_set(v.field("families")?)?,
            wire_versions: carolina_core::canon::decode_set(v.field("wire_versions")?)?,
        })
    }
}

impl Canonical for TopologySnapshot {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fvec("authority_domains", &self.authority_domains)
            .fvec("durability_policies", &self.durability_policies)
            .fstr("kind", "topology.v1")
            .fvec("node_capabilities", &self.node_capabilities)
            .fvec("placements", &self.placements)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(TopologySnapshot {
            placements: Vec::from_canon(v.field("placements")?)?,
            authority_domains: Vec::from_canon(v.field("authority_domains")?)?,
            durability_policies: Vec::from_canon(v.field("durability_policies")?)?,
            node_capabilities: Vec::from_canon(v.field("node_capabilities")?)?,
        })
    }
}

/// Compile policy (SPEC-004 §3): preferences among safe candidates; never overrides a failed obligation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilePolicy {
    /// Prefer fewer remote participants / rounds (lexicographic cost). Always true in v1.
    pub minimize_cost: bool,
    /// Administrator-required serial semantics for these operations (by name).
    pub force_serial: Vec<String>,
}

impl Default for CompilePolicy {
    fn default() -> Self {
        CompilePolicy {
            minimize_cost: true,
            force_serial: vec![],
        }
    }
}

impl Canonical for CompilePolicy {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fset("force_serial", &self.force_serial)
            .fbool("minimize_cost", self.minimize_cost)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(CompilePolicy {
            minimize_cost: v.field("minimize_cost")?.as_bool()?,
            force_serial: carolina_core::canon::decode_set(v.field("force_serial")?)?,
        })
    }
}

/// Deterministic analysis budget: abstract work steps for closure/counterexample exploration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeterministicBudget {
    pub max_steps: u64,
    /// Maximum rows per record in generated small states.
    pub max_rows_per_record: usize,
    /// Maximum distinct values per parameter in generated small arguments.
    pub max_values_per_param: usize,
}

impl Default for DeterministicBudget {
    fn default() -> Self {
        DeterministicBudget {
            max_steps: 200_000,
            max_rows_per_record: 2,
            max_values_per_param: 3,
        }
    }
}

impl Canonical for DeterministicBudget {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fu64("max_rows_per_record", self.max_rows_per_record as u64)
            .fu64("max_steps", self.max_steps)
            .fu64("max_values_per_param", self.max_values_per_param as u64)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(DeterministicBudget {
            max_steps: v.field("max_steps")?.as_u64()?,
            max_rows_per_record: v.field("max_rows_per_record")?.as_u64()? as usize,
            max_values_per_param: v.field("max_values_per_param")?.as_u64()? as usize,
        })
    }
}

pub struct CompileInput {
    pub module_ir: ModuleIR,
    pub topology: TopologySnapshot,
    pub active_generation: CatalogGeneration,
    pub protocol_library: crate::library::ProtocolLibraryManifest,
    pub analysis_rules: crate::rules::AnalysisRuleManifest,
    pub policy: CompilePolicy,
    pub analysis_budget: DeterministicBudget,
    pub prior_plans: Vec<PlanRef>,
}

impl CompileInput {
    /// Standard local MVP-1 input for a module.
    pub fn local(module_ir: ModuleIR) -> CompileInput {
        CompileInput {
            module_ir,
            topology: TopologySnapshot::local_single_node(),
            active_generation: CatalogGeneration(1),
            protocol_library: crate::library::ProtocolLibraryManifest::v1(),
            analysis_rules: crate::rules::AnalysisRuleManifest::v1(),
            policy: CompilePolicy::default(),
            analysis_budget: DeterministicBudget::default(),
            prior_plans: vec![],
        }
    }
}
