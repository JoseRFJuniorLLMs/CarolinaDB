//! DEV_LOCAL startup must reject unsafe or inconsistent configuration before side effects.

use std::net::TcpListener;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use carolina_catalog::BootstrapManifest;
use carolina_core::canon::Canonical;
use carolina_core::error::ErrorCode;
use carolina_core::hash::Hash256;
use carolina_core::ids::{ClusterId, NodeId, PrincipalId, SecurityPolicyId};
use carolina_core::limits::Limits;
use carolina_node::config::PeerConfig;
use carolina_node::{run_node, Client, NodeConfig};
use carolina_wire::negotiation::EndpointRole;

fn config() -> NodeConfig {
    NodeConfig {
        node: "alpha".into(),
        listen: "127.0.0.1:31001".into(),
        peers: vec![
            PeerConfig {
                node: "beta".into(),
                addr: "127.0.0.1:31002".into(),
            },
            PeerConfig {
                node: "gamma".into(),
                addr: "127.0.0.1:31003".into(),
            },
        ],
        data_dir: "unused-config-validation".into(),
        manifest: BootstrapManifest {
            cluster_id: ClusterId::derive("config-test"),
            voters: ["alpha", "beta", "gamma"].map(NodeId::derive).to_vec(),
            trust_root_hash: Hash256([7; 32]),
            security_policy: SecurityPolicyId::derive("dev-local-policy"),
            bootstrap_admin: PrincipalId::derive("admin"),
            security_profile: "DEV_LOCAL".into(),
        },
        module: "fixture:inventory_reserve_release".into(),
        seed: 10,
        home: "config-home".into(),
        tick_millis: 40,
    }
}

fn rejected(cfg: &NodeConfig) {
    assert_eq!(cfg.validate().unwrap_err().code, ErrorCode::InvalidManifest);
}

#[test]
fn accepts_exact_three_voters_on_ipv4_and_ipv6_loopback() {
    let mut cfg = config();
    cfg.validate().unwrap();
    cfg.listen = "[::1]:31001".into();
    cfg.peers[0].addr = "[::1]:31002".into();
    cfg.peers.reverse();
    cfg.manifest.voters.reverse();
    cfg.validate().unwrap();
    // Canonical encoding sorts voters as a set; validation does not depend on input ordering.
    let decoded = NodeConfig::decode(&cfg.encode(), &Limits::v1()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded.node, cfg.node);
    assert_eq!(decoded.peers, cfg.peers);
}

#[test]
fn rejects_wrong_profiles_voters_and_zero_tick() {
    let mut cfg = config();
    cfg.manifest.security_profile = "ENCRYPTED_HOST_V1".into();
    rejected(&cfg);
    cfg = config();
    cfg.manifest.voters.pop();
    rejected(&cfg);
    cfg = config();
    cfg.manifest.voters[2] = cfg.manifest.voters[1];
    rejected(&cfg);
    cfg = config();
    cfg.node = "outsider".into();
    rejected(&cfg);
    cfg = config();
    cfg.tick_millis = 0;
    rejected(&cfg);
}

#[test]
fn rejects_missing_duplicate_unknown_and_self_peers() {
    let mut cfg = config();
    cfg.peers.pop();
    rejected(&cfg);
    cfg = config();
    cfg.peers.push(cfg.peers[0].clone());
    rejected(&cfg);
    cfg = config();
    cfg.peers[1].node = cfg.peers[0].node.clone();
    rejected(&cfg);
    cfg = config();
    cfg.peers[1].node = "outsider".into();
    rejected(&cfg);
    cfg = config();
    cfg.peers[0].node = cfg.node.clone();
    rejected(&cfg);
}

#[test]
fn rejects_non_loopback_invalid_zero_and_reused_addresses() {
    for addr in [
        "0.0.0.0:31001",
        "[::]:31001",
        "192.0.2.1:31001",
        "[2001:db8::1]:31001",
        "[::ffff:192.0.2.1]:31001",
        "localhost:31001",
        "127.0.0.1",
        "127.0.0.1:0",
    ] {
        let mut cfg = config();
        cfg.listen = addr.into();
        rejected(&cfg);
        cfg = config();
        cfg.peers[0].addr = addr.into();
        rejected(&cfg);
    }
    let mut cfg = config();
    cfg.peers[1].addr = cfg.peers[0].addr.clone();
    rejected(&cfg);
    cfg = config();
    cfg.peers[0].addr = cfg.listen.clone();
    rejected(&cfg);
    cfg.listen = "[::1]:31001".into();
    cfg.peers[0].addr = "[0:0:0:0:0:0:0:1]:31001".into();
    rejected(&cfg);
}

#[test]
fn decoding_rejects_invalid_configuration() {
    let mut cfg = config();
    cfg.peers[0].addr = "192.0.2.1:31002".into();
    assert_eq!(
        NodeConfig::decode(&cfg.encode(), &Limits::v1())
            .unwrap_err()
            .code,
        ErrorCode::InvalidManifest
    );
}

#[test]
fn invalid_startup_does_not_bind_or_create_data() {
    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let root = std::env::temp_dir().join(format!(
        "carolina-invalid-config-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut cfg = config();
    cfg.listen = occupied.local_addr().unwrap().to_string();
    cfg.data_dir = root.to_string_lossy().into_owned();
    cfg.peers[1].addr = "invalid-address".into();
    // Binding first would return Io (address already in use), rather than InvalidManifest.
    assert_eq!(run_node(cfg).unwrap_err().code, ErrorCode::InvalidManifest);
    assert!(!root.exists(), "invalid startup created durable state");
}

#[test]
fn client_rejects_remote_address_before_connecting() {
    let result = Client::connect(
        "192.0.2.1:31001".parse().unwrap(),
        ClusterId::derive("config-test"),
        EndpointRole::Admin,
        Duration::from_millis(1),
    );
    match result {
        Err(error) => assert_eq!(error.code, ErrorCode::InvalidManifest),
        Ok(_) => panic!("DEV_LOCAL dialed a remote address"),
    }
}
