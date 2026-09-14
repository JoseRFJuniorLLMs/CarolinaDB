//! Log commands and administrative messages of the node.

use std::collections::BTreeMap;

use carolina_catalog::{BootstrapManifest, CatalogCommand, CatalogCommit};
use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::Hash256;
use carolina_core::ids::*;
use carolina_lang::types::Value;
use carolina_wire::records::InvokeV1;

/// Rows of an administrative bulk load: (primary key, named fields).
pub type SeedRows = Vec<(Value, Vec<(String, Value)>)>;

/// What the consensus log carries (all voters apply these deterministically).
// the admitted invoke dominates the size; boxing it buys nothing on the apply path
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeCommand {
    Genesis(BootstrapManifest),
    Catalog(CatalogCommand),
    /// Ordered admission of one client request (SPEC-008 §8 steps 3–4): the execution slot.
    Admit {
        invoke: InvokeV1,
        principal: PrincipalId,
        admitted_by: NodeId,
        catalog_generation: CatalogGeneration,
    },
    /// The unique decision for an admitted request: the exact receipt digest the admitting
    /// node computed (SPEC-008 §10.3). Followers verify their own digest against it.
    Decision {
        request_key: RequestKey,
        txn_id: TxnId,
        outcome: String,
        receipt_digest: Hash256,
        decided_by: NodeId,
    },
    /// Administrative bulk load through the ordered log (synthetic fixtures only).
    Seed {
        label: String,
        record: String,
        rows: SeedRows,
    },
}

fn rows_canon(rows: &[(Value, Vec<(String, Value)>)]) -> CanonValue {
    CanonValue::Array(
        rows.iter()
            .map(|(k, fields)| {
                let fs: Vec<CanonValue> = fields
                    .iter()
                    .map(|(n, v)| CanonValue::obj().fstr("name", n).fc("value", v).build())
                    .collect();
                CanonValue::obj()
                    .f("fields", CanonValue::Array(fs))
                    .fc("key", k)
                    .build()
            })
            .collect(),
    )
}

fn rows_from(v: &CanonValue) -> CoreResult<SeedRows> {
    let mut out = Vec::new();
    for r in v.as_array()? {
        let key = Value::from_canon(r.field("key")?)?;
        let mut fields = Vec::new();
        for f in r.field("fields")?.as_array()? {
            fields.push((
                f.field("name")?.as_str()?.to_string(),
                Value::from_canon(f.field("value")?)?,
            ));
        }
        out.push((key, fields));
    }
    Ok(out)
}

impl Canonical for NodeCommand {
    fn to_canon(&self) -> CanonValue {
        match self {
            NodeCommand::Genesis(m) => CanonValue::obj()
                .fstr("kind", "Genesis")
                .fc("manifest", m)
                .build(),
            NodeCommand::Catalog(c) => CanonValue::obj()
                .fc("command", c)
                .fstr("kind", "Catalog")
                .build(),
            NodeCommand::Admit {
                invoke,
                principal,
                admitted_by,
                catalog_generation,
            } => CanonValue::obj()
                .fc("admitted_by", admitted_by)
                .fc("catalog_generation", catalog_generation)
                .fc("invoke", invoke)
                .fstr("kind", "Admit")
                .fc("principal", principal)
                .build(),
            NodeCommand::Decision {
                request_key,
                txn_id,
                outcome,
                receipt_digest,
                decided_by,
            } => CanonValue::obj()
                .fc("decided_by", decided_by)
                .fstr("kind", "Decision")
                .fstr("outcome", outcome)
                .fc("receipt_digest", receipt_digest)
                .fc("request_key", request_key)
                .fc("txn_id", txn_id)
                .build(),
            NodeCommand::Seed {
                label,
                record,
                rows,
            } => CanonValue::obj()
                .fstr("kind", "Seed")
                .fstr("label", label)
                .fstr("record", record)
                .f("rows", rows_canon(rows))
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "Genesis" => NodeCommand::Genesis(BootstrapManifest::from_canon(v.field("manifest")?)?),
            "Catalog" => NodeCommand::Catalog(CatalogCommand::from_canon(v.field("command")?)?),
            "Admit" => NodeCommand::Admit {
                invoke: InvokeV1::from_canon(v.field("invoke")?)?,
                principal: PrincipalId::from_canon(v.field("principal")?)?,
                admitted_by: NodeId::from_canon(v.field("admitted_by")?)?,
                catalog_generation: CatalogGeneration::from_canon(v.field("catalog_generation")?)?,
            },
            "Decision" => NodeCommand::Decision {
                request_key: RequestKey::from_canon(v.field("request_key")?)?,
                txn_id: TxnId::from_canon(v.field("txn_id")?)?,
                outcome: v.field("outcome")?.as_str()?.to_string(),
                receipt_digest: Hash256::from_canon(v.field("receipt_digest")?)?,
                decided_by: NodeId::from_canon(v.field("decided_by")?)?,
            },
            "Seed" => NodeCommand::Seed {
                label: v.field("label")?.as_str()?.to_string(),
                record: v.field("record")?.as_str()?.to_string(),
                rows: rows_from(v.field("rows")?)?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("node command {k}"),
                ))
            }
        })
    }
}

/// Administrative requests (`MessageKind::Admin`), accepted on `Admin`-role connections only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminRequest {
    Status,
    Catalog(CatalogCommand),
    Seed {
        label: String,
        record: String,
        rows: SeedRows,
    },
}

impl Canonical for AdminRequest {
    fn to_canon(&self) -> CanonValue {
        match self {
            AdminRequest::Status => CanonValue::obj().fstr("kind", "Status").build(),
            AdminRequest::Catalog(c) => CanonValue::obj()
                .fc("command", c)
                .fstr("kind", "Catalog")
                .build(),
            AdminRequest::Seed {
                label,
                record,
                rows,
            } => CanonValue::obj()
                .fstr("kind", "Seed")
                .fstr("label", label)
                .fstr("record", record)
                .f("rows", rows_canon(rows))
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "Status" => AdminRequest::Status,
            "Catalog" => AdminRequest::Catalog(CatalogCommand::from_canon(v.field("command")?)?),
            "Seed" => AdminRequest::Seed {
                label: v.field("label")?.as_str()?.to_string(),
                record: v.field("record")?.as_str()?.to_string(),
                rows: rows_from(v.field("rows")?)?,
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("admin request {k}"),
                ))
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeStatus {
    pub node: NodeId,
    pub readiness: String,
    pub role: String,
    pub term: u64,
    pub leader: Option<NodeId>,
    pub commit_index: u64,
    pub applied_index: u64,
    pub catalog_generation: CatalogGeneration,
    pub genesis: bool,
    pub grant_active: bool,
    pub state_digest: Hash256,
    pub durable_commit_seq: u64,
    pub failure: Option<String>,
    /// Operational counters, `area.name -> value` (SPEC-001 §63, SPEC-008 §19). Names are stable;
    /// a missing name means the build does not produce it, never that the value is zero.
    pub metrics: BTreeMap<String, u64>,
}

impl Canonical for NodeStatus {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fu64("applied_index", self.applied_index)
            .fc("catalog_generation", &self.catalog_generation)
            .fu64("commit_index", self.commit_index)
            .fu64("durable_commit_seq", self.durable_commit_seq)
            .f(
                "failure",
                match &self.failure {
                    Some(f) => CanonValue::str(f.as_str()),
                    None => CanonValue::Null,
                },
            )
            .fbool("genesis", self.genesis)
            .fbool("grant_active", self.grant_active)
            .fopt("leader", &self.leader)
            .f(
                "metrics",
                CanonValue::Object(
                    self.metrics
                        .iter()
                        .map(|(k, v)| (k.clone(), CanonValue::uint(*v)))
                        .collect(),
                ),
            )
            .fc("node", &self.node)
            .fstr("readiness", &self.readiness)
            .fstr("role", &self.role)
            .fc("state_digest", &self.state_digest)
            .fu64("term", self.term)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        let mut metrics = BTreeMap::new();
        if let CanonValue::Object(o) = v.field("metrics")? {
            for (k, val) in o {
                metrics.insert(k.clone(), val.as_u64()?);
            }
        }
        Ok(NodeStatus {
            metrics,
            node: NodeId::from_canon(v.field("node")?)?,
            readiness: v.field("readiness")?.as_str()?.to_string(),
            role: v.field("role")?.as_str()?.to_string(),
            term: v.field("term")?.as_u64()?,
            leader: match v.field("leader")? {
                CanonValue::Null => None,
                x => Some(NodeId::from_canon(x)?),
            },
            commit_index: v.field("commit_index")?.as_u64()?,
            applied_index: v.field("applied_index")?.as_u64()?,
            catalog_generation: CatalogGeneration::from_canon(v.field("catalog_generation")?)?,
            genesis: v.field("genesis")?.as_bool()?,
            grant_active: v.field("grant_active")?.as_bool()?,
            state_digest: Hash256::from_canon(v.field("state_digest")?)?,
            durable_commit_seq: v.field("durable_commit_seq")?.as_u64()?,
            failure: match v.field("failure")? {
                CanonValue::Null => None,
                x => Some(x.as_str()?.to_string()),
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminReply {
    Status(NodeStatus),
    Committed(CatalogCommit),
    Seeded {
        log_index: u64,
    },
    Refused {
        code: String,
        detail: String,
        leader: Option<NodeId>,
    },
}

impl Canonical for AdminReply {
    fn to_canon(&self) -> CanonValue {
        match self {
            AdminReply::Status(s) => CanonValue::obj()
                .fstr("kind", "Status")
                .fc("status", s)
                .build(),
            AdminReply::Committed(c) => CanonValue::obj()
                .fc("commit", c)
                .fstr("kind", "Committed")
                .build(),
            AdminReply::Seeded { log_index } => CanonValue::obj()
                .fstr("kind", "Seeded")
                .fu64("log_index", *log_index)
                .build(),
            AdminReply::Refused {
                code,
                detail,
                leader,
            } => CanonValue::obj()
                .fstr("code", code)
                .fstr("detail", detail)
                .fstr("kind", "Refused")
                .fopt("leader", leader)
                .build(),
        }
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(match v.field("kind")?.as_str()? {
            "Status" => AdminReply::Status(NodeStatus::from_canon(v.field("status")?)?),
            "Committed" => AdminReply::Committed(CatalogCommit::from_canon(v.field("commit")?)?),
            "Seeded" => AdminReply::Seeded {
                log_index: v.field("log_index")?.as_u64()?,
            },
            "Refused" => AdminReply::Refused {
                code: v.field("code")?.as_str()?.to_string(),
                detail: v.field("detail")?.as_str()?.to_string(),
                leader: match v.field("leader")? {
                    CanonValue::Null => None,
                    x => Some(NodeId::from_canon(x)?),
                },
            },
            k => {
                return Err(CoreError::new(
                    ErrorCode::NonCanonicalEncoding,
                    format!("admin reply {k}"),
                ))
            }
        })
    }
}
