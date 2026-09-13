//! Node configuration (canonical JSON file).

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;

use carolina_catalog::BootstrapManifest;
use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::ids::{NodeId, RequestHomeId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerConfig {
    pub node: String,
    pub addr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeConfig {
    /// Human label; `NodeId::derive(node)` is the registered identity.
    pub node: String,
    pub listen: String,
    pub peers: Vec<PeerConfig>,
    pub data_dir: String,
    pub manifest: BootstrapManifest,
    /// `fixture:<name>` or a path to a `.cdl` module.
    pub module: String,
    pub seed: u64,
    /// Label of the replicated request home (its id is derived from it).
    pub home: String,
    pub tick_millis: u64,
}

impl NodeConfig {
    /// Validate the complete fixed three-voter, loopback-only configuration before startup
    /// opens a listener, starts peer threads or creates durable state.
    pub fn validate(&self) -> CoreResult<()> {
        let invalid = |message| CoreError::new(ErrorCode::InvalidManifest, message);
        if self.manifest.security_profile != crate::transport::SECURITY_PROFILE {
            return Err(invalid(
                "this node supports only the DEV_LOCAL security profile",
            ));
        }
        let voters: BTreeSet<_> = self.manifest.voters.iter().copied().collect();
        if self.manifest.voters.len() != 3 || voters.len() != 3 {
            return Err(invalid("bootstrap requires exactly three distinct voters"));
        }
        let me = self.node_id();
        if self.node.trim().is_empty() || !voters.contains(&me) {
            return Err(invalid(
                "this node must be a named voter of the pinned manifest",
            ));
        }
        if self.tick_millis == 0 {
            return Err(invalid("tick_millis must be positive"));
        }
        let listen = self.listen_addr()?;
        if self.peers.len() != 2 {
            return Err(invalid(
                "peers must contain exactly the other two bootstrap voters",
            ));
        }
        let mut peers = BTreeSet::new();
        let mut addresses = BTreeSet::from([listen]);
        for peer in &self.peers {
            let id = NodeId::derive(&peer.node);
            if peer.node.trim().is_empty() || id == me || !voters.contains(&id) {
                return Err(invalid(
                    "peer must be another named voter of the pinned manifest",
                ));
            }
            if !peers.insert(id) {
                return Err(invalid("duplicate peer identity"));
            }
            if !addresses.insert(config_addr(&peer.addr, "peer")?) {
                return Err(invalid("listen and peer addresses must be distinct"));
            }
        }
        // Two distinct known peers plus this voter exhaust the validated three-voter manifest.
        Ok(())
    }

    pub fn node_id(&self) -> NodeId {
        NodeId::derive(&self.node)
    }
    pub fn home_id(&self) -> RequestHomeId {
        RequestHomeId::derive(&self.home)
    }
    pub fn listen_addr(&self) -> CoreResult<SocketAddr> {
        config_addr(&self.listen, "listen")
    }
    pub fn data_dir(&self) -> PathBuf {
        PathBuf::from(&self.data_dir)
    }
    pub fn module_source(&self) -> CoreResult<String> {
        if let Some(name) = self.module.strip_prefix("fixture:") {
            return carolina_lang::fixtures::fixture_source(name)
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    CoreError::new(ErrorCode::MissingRecord, format!("fixture {name}"))
                });
        }
        std::fs::read_to_string(&self.module)
            .map_err(|e| CoreError::new(ErrorCode::Io, format!("module {}: {e}", self.module)))
    }
}

fn config_addr(text: &str, kind: &str) -> CoreResult<SocketAddr> {
    let addr: SocketAddr = text.parse().map_err(|_| {
        CoreError::new(
            ErrorCode::InvalidManifest,
            format!("invalid {kind} address {text}"),
        )
    })?;
    crate::transport::require_loopback(addr)?;
    if addr.port() == 0 {
        return Err(CoreError::new(
            ErrorCode::InvalidManifest,
            format!("{kind} address requires a fixed nonzero port"),
        ));
    }
    Ok(addr)
}

impl Canonical for PeerConfig {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("addr", &self.addr)
            .fstr("node", &self.node)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&["addr", "node"])?;
        Ok(PeerConfig {
            node: v.field("node")?.as_str()?.to_string(),
            addr: v.field("addr")?.as_str()?.to_string(),
        })
    }
}

impl Canonical for NodeConfig {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("data_dir", &self.data_dir)
            .fstr("home", &self.home)
            .fstr("listen", &self.listen)
            .fc("manifest", &self.manifest)
            .fstr("module", &self.module)
            .fstr("node", &self.node)
            .fvec("peers", &self.peers)
            .fu64("seed", self.seed)
            .fu64("tick_millis", self.tick_millis)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        v.expect_fields(&[
            "data_dir",
            "home",
            "listen",
            "manifest",
            "module",
            "node",
            "peers",
            "seed",
            "tick_millis",
        ])?;
        let config = NodeConfig {
            node: v.field("node")?.as_str()?.to_string(),
            listen: v.field("listen")?.as_str()?.to_string(),
            peers: v
                .field("peers")?
                .as_array()?
                .iter()
                .map(PeerConfig::from_canon)
                .collect::<CoreResult<Vec<_>>>()?,
            data_dir: v.field("data_dir")?.as_str()?.to_string(),
            manifest: BootstrapManifest::from_canon(v.field("manifest")?)?,
            module: v.field("module")?.as_str()?.to_string(),
            seed: v.field("seed")?.as_u64()?,
            home: v.field("home")?.as_str()?.to_string(),
            tick_millis: v.field("tick_millis")?.as_u64()?,
        };
        config.validate()?;
        Ok(config)
    }
}

impl NodeConfig {
    pub fn load(path: &std::path::Path) -> CoreResult<NodeConfig> {
        let bytes = std::fs::read(path).map_err(|e| {
            CoreError::new(ErrorCode::Io, format!("config {}: {e}", path.display()))
        })?;
        NodeConfig::decode(&bytes, &carolina_core::limits::Limits::v1())
    }
}
