//! Real-process fault campaign for the single-IDC C5 slice (SPEC-010 §9, SPEC-014 §2/§3 MVP-3).
//!
//! Three `carolina-node` processes with isolated data directories on the loopback network
//! (an explicitly allowlisted scratch root). Real process kills; nothing is mocked. Shared by the
//! crate's integration test and the qualification runner (`carolina-qualify`).
//!
//! Checks (SPEC-008 §20 subset):
//! * C5-001 — results match the sequential contract (a reserve commits once with the exact result);
//! * C5-010 — an exact duplicate returns byte-identical bytes; changed payload under one key is refused;
//! * C5-009 — a follower refuses mutations; a minority survivor cannot admit;
//! * C5-021 — leader killed after admissions: a new leader continues the same home epoch and
//!   allocation counter, and `ResolveRequest` by `RequestKey` returns the original receipt;
//! * replicated determinism — a restarted voter replays the log and reports the same state digest
//!   as the survivors, with no fail-closed divergence.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use carolina_catalog::BootstrapManifest;
use carolina_core::canon::Canonical;
use carolina_core::hash::Hash256;
use carolina_core::ids::*;
use carolina_lang::fixtures::fixture_source;
use carolina_lang::types::Value;
use carolina_runtime::engine::make_invoke;
use carolina_runtime::LocalCatalog;
use carolina_wire::negotiation::EndpointRole;
use carolina_wire::records::*;

use crate::client::Client;
use crate::config::{NodeConfig, PeerConfig};
use crate::protocol::AdminReply;

const ITEM: [u8; 16] = [0x33; 16];

pub struct Proc {
    pub name: String,
    cfg_path: PathBuf,
    addr: String,
    child: Option<Child>,
    binary: PathBuf,
}

impl Proc {
    pub fn start(&mut self) -> Result<(), String> {
        let child = Command::new(&self.binary)
            .arg("--config")
            .arg(&self.cfg_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", self.binary.display()))?;
        self.child = Some(child);
        Ok(())
    }
    pub fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
    pub fn is_up(&self) -> bool {
        self.child.is_some()
    }
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr.parse().unwrap()
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        self.kill();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Locate the node binary: `CARGO_BIN_EXE_carolina-node` (crate tests), an explicit override
/// (`CAROLINA_NODE_BIN`), or `carolina-node[.exe]` next to the running executable.
pub fn find_node_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CAROLINA_NODE_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(p) = option_env!("CARGO_BIN_EXE_carolina-node") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for name in ["carolina-node.exe", "carolina-node"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
        if let Some(parent) = dir.parent() {
            let p = parent.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

pub fn cluster(
    binary: &Path,
    root: &Path,
    label: &str,
) -> Result<(Vec<Proc>, BootstrapManifest), String> {
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let names = ["alpha", "beta", "gamma"];
    let ports: Vec<u16> = names.iter().map(|_| free_port()).collect();
    let manifest = BootstrapManifest {
        cluster_id: ClusterId::derive(&format!("cluster-{label}")),
        voters: names.iter().map(|n| NodeId::derive(n)).collect(),
        trust_root_hash: Hash256([7; 32]),
        security_policy: SecurityPolicyId::derive("dev-local-policy"),
        bootstrap_admin: PrincipalId::derive("bootstrap-admin"),
        security_profile: "DEV_LOCAL".into(),
    };
    let peers: Vec<PeerConfig> = names
        .iter()
        .zip(&ports)
        .map(|(n, p)| PeerConfig {
            node: n.to_string(),
            addr: format!("127.0.0.1:{p}"),
        })
        .collect();
    let mut procs = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let cfg = NodeConfig {
            node: n.to_string(),
            listen: format!("127.0.0.1:{}", ports[i]),
            peers: peers
                .iter()
                .filter(|peer| peer.node != *n)
                .cloned()
                .collect(),
            data_dir: root.join(n).to_string_lossy().to_string(),
            manifest: manifest.clone(),
            module: "fixture:inventory_reserve_release".into(),
            seed: 1000 + i as u64,
            home: format!("home-{label}"),
            tick_millis: 40,
        };
        let path = root.join(format!("{n}.json"));
        std::fs::write(&path, cfg.encode()).map_err(|e| e.to_string())?;
        procs.push(Proc {
            name: n.to_string(),
            cfg_path: path,
            addr: format!("127.0.0.1:{}", ports[i]),
            child: None,
            binary: binary.to_path_buf(),
        });
    }
    Ok((procs, manifest))
}

pub fn connect(p: &Proc, cluster: ClusterId, role: EndpointRole) -> Option<Client> {
    Client::connect(p.addr(), cluster, role, Duration::from_secs(5)).ok()
}

/// Wait until some live node reports Ready with a live leader; returns the leader's index.
pub fn wait_leader(procs: &[Proc], cluster: ClusterId, timeout: Duration) -> Option<usize> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        for p in procs.iter().filter(|p| p.is_up()) {
            if let Some(mut c) = connect(p, cluster, EndpointRole::Admin) {
                if let Ok(s) = c.status() {
                    if s.readiness == "Ready" {
                        if let Some(l) = s.leader {
                            if let Some(i) = procs.iter().position(|q| NodeId::derive(&q.name) == l)
                            {
                                if procs[i].is_up() {
                                    return Some(i);
                                }
                            }
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn reserve(cat: &LocalCatalog, req: &str, q: i64, rid: u8) -> InvokeV1 {
    make_invoke(
        cat,
        "tenant-c5",
        req,
        "reserve",
        vec![Value::Uuid(ITEM), Value::I64(q), Value::Uuid([rid; 16])],
    )
    .unwrap()
}

fn committed(r: &ClientReplyV1) -> Result<&FinalReceiptV1, String> {
    match r {
        ClientReplyV1::Committed(r) => Ok(r),
        other => Err(format!("expected Committed, got {}", other.kind())),
    }
}

fn invoke_via_leader(
    procs: &[Proc],
    cluster: ClusterId,
    inv: &InvokeV1,
    timeout: Duration,
) -> Result<ClientReplyV1, String> {
    let start = Instant::now();
    let mut last = String::new();
    while start.elapsed() < timeout {
        if let Some(l) = wait_leader(procs, cluster, Duration::from_secs(10)) {
            if let Some(mut c) = connect(&procs[l], cluster, EndpointRole::Client) {
                match c.invoke(inv) {
                    Ok(ClientReplyV1::Unavailable(r)) => {
                        last = format!("Unavailable {}: {}", r.code, r.detail);
                        std::thread::sleep(Duration::from_millis(150));
                        continue;
                    }
                    Ok(r) => return Ok(r),
                    Err(e) => {
                        last = e.to_string();
                        continue;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("no reply within {timeout:?}: last {last}"))
}

fn resolve_via_leader(
    procs: &[Proc],
    cluster: ClusterId,
    inv: &InvokeV1,
    timeout: Duration,
) -> Result<ResolveReplyV1, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(l) = wait_leader(procs, cluster, Duration::from_secs(10)) {
            if let Some(mut c) = connect(&procs[l], cluster, EndpointRole::Client) {
                match c.resolve(&ResolveRequestV1 {
                    request_key: inv.content.request_key,
                    expected_request_hash: inv.request_hash,
                }) {
                    Ok(ResolveReplyV1::Unavailable(_)) | Err(_) => {
                        std::thread::sleep(Duration::from_millis(150));
                        continue;
                    }
                    Ok(r) => return Ok(r),
                }
            }
        }
    }
    Err(format!("no resolve reply within {timeout:?}"))
}

#[derive(Debug, Default, Clone)]
pub struct C5Summary {
    pub checks: Vec<String>,
    pub metrics: BTreeMap<String, u64>,
}

macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            return Err(format!($($arg)*));
        }
    };
}

/// The campaign. `root` must be an allowlisted scratch directory (the caller validates it).
pub fn three_process_c5(binary: &Path, root: &Path) -> Result<C5Summary, String> {
    let (mut procs, manifest) = cluster(binary, root, "c5")?;
    let cluster_id = manifest.cluster_id;
    for p in procs.iter_mut() {
        p.start()?;
    }
    let mut summary = C5Summary::default();
    let leader = wait_leader(&procs, cluster_id, Duration::from_secs(60))
        .ok_or("cluster did not bootstrap (genesis, grant, route) within 60s")?;
    summary
        .checks
        .push("bootstrap: genesis + staged/activated home grant + route committed".into());
    let cat = LocalCatalog::from_source(fixture_source("inventory_reserve_release").unwrap())
        .map_err(|e| e.to_string())?;
    {
        let mut c =
            connect(&procs[leader], cluster_id, EndpointRole::Admin).ok_or("connect admin")?;
        let rows = vec![(
            Value::Uuid(ITEM),
            vec![
                ("id".to_string(), Value::Uuid(ITEM)),
                ("available".to_string(), Value::I64(10)),
                ("reserved".to_string(), Value::I64(0)),
                ("total".to_string(), Value::I64(10)),
            ],
        )];
        match c.seed("seed-1", "Item", rows).map_err(|e| e.to_string())? {
            AdminReply::Seeded { .. } => {}
            other => return Err(format!("seed: {other:?}")),
        }
    }
    let inv1 = reserve(&cat, "r1", 3, 1);
    let r1 = committed(&invoke_via_leader(
        &procs,
        cluster_id,
        &inv1,
        Duration::from_secs(30),
    )?)?
    .clone();
    let again = committed(&invoke_via_leader(
        &procs,
        cluster_id,
        &inv1,
        Duration::from_secs(30),
    )?)?
    .clone();
    ensure!(
        again.encode() == r1.encode(),
        "C5-010: duplicate returned different bytes"
    );
    summary
        .checks
        .push("C5-001/C5-010: reserve committed once; exact duplicate byte-identical".into());
    let mismatch = reserve(&cat, "r1", 4, 2);
    ensure!(
        matches!(
            invoke_via_leader(&procs, cluster_id, &mismatch, Duration::from_secs(30))?,
            ClientReplyV1::RequestIdentityMismatch
        ),
        "C5-010: changed payload under one key was not refused"
    );
    summary
        .checks
        .push("C5-010: changed payload under a bound key refused".into());
    let follower = (0..3).find(|i| *i != leader).unwrap();
    {
        let mut c = connect(&procs[follower], cluster_id, EndpointRole::Client)
            .ok_or("connect follower")?;
        let r = c
            .invoke(&reserve(&cat, "r-follower", 1, 9))
            .map_err(|e| e.to_string())?;
        ensure!(
            matches!(r, ClientReplyV1::Unavailable(_)),
            "C5-009: follower admitted a mutation: {}",
            r.kind()
        );
    }
    summary
        .checks
        .push("C5-009: follower refuses mutations".into());
    // kill the leader
    procs[leader].kill();
    summary.metrics.insert("leader_kills".into(), 1);
    let inv2 = reserve(&cat, "r2", 2, 2);
    let r2 = committed(&invoke_via_leader(
        &procs,
        cluster_id,
        &inv2,
        Duration::from_secs(60),
    )?)?
    .clone();
    ensure!(
        r2.txn_id.epoch() == r1.txn_id.epoch(),
        "C5-021: home epoch changed across failover"
    );
    ensure!(
        r2.txn_id.seq().0 == r1.txn_id.seq().0 + 1,
        "C5-021: allocation counter did not continue ({} vs {})",
        r2.txn_id.seq().0,
        r1.txn_id.seq().0
    );
    match resolve_via_leader(&procs, cluster_id, &inv1, Duration::from_secs(30))? {
        ResolveReplyV1::Terminal(b) => ensure!(
            committed(&b)?.encode() == r1.encode(),
            "C5-021: resolve after failover returned different bytes"
        ),
        other => return Err(format!("C5-021: resolve after failover: {other:?}")),
    }
    summary.checks.push(
        "C5-021: new leader continues the home; resolve by RequestKey returns the original receipt"
            .into(),
    );
    // restart the killed voter and wait for convergence
    procs[leader].start()?;
    let target = {
        let l = wait_leader(&procs, cluster_id, Duration::from_secs(60))
            .ok_or("no leader after restart")?;
        let mut c = connect(&procs[l], cluster_id, EndpointRole::Admin).ok_or("connect")?;
        c.status().map_err(|e| e.to_string())?
    };
    let start = Instant::now();
    let mut converged = false;
    while start.elapsed() < Duration::from_secs(60) {
        if let Some(mut c) = connect(&procs[leader], cluster_id, EndpointRole::Admin) {
            if let Ok(s) = c.status() {
                if s.failure.is_none()
                    && s.state_digest == target.state_digest
                    && s.applied_index >= target.applied_index
                {
                    converged = true;
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    ensure!(
        converged,
        "restarted voter did not converge to the replicated state digest"
    );
    for p in &procs {
        let mut c = connect(p, cluster_id, EndpointRole::Admin).ok_or("connect")?;
        let s = c.status().map_err(|e| e.to_string())?;
        ensure!(
            s.failure.is_none(),
            "{}: fail-closed: {:?}",
            p.name,
            s.failure
        );
        ensure!(
            s.state_digest == target.state_digest,
            "{}: state digest differs from the leader",
            p.name
        );
    }
    summary.checks.push(
        "determinism: restarted voter replayed the log; all three voters report one state digest"
            .into(),
    );
    summary
        .metrics
        .insert("applied_index".into(), target.applied_index);
    // minority cannot serve
    let l = wait_leader(&procs, cluster_id, Duration::from_secs(30)).ok_or("no leader")?;
    let others: Vec<usize> = (0..3).filter(|i| *i != l).collect();
    for i in &others {
        procs[*i].kill();
    }
    std::thread::sleep(Duration::from_millis(2500));
    let mut c = connect(&procs[l], cluster_id, EndpointRole::Client).ok_or("connect survivor")?;
    let reply = c.invoke(&reserve(&cat, "r-minority", 1, 5));
    ensure!(
        matches!(reply, Ok(ClientReplyV1::Unavailable(_)) | Err(_)),
        "C5-009: a minority survivor admitted a mutation: {reply:?}"
    );
    summary
        .checks
        .push("C5-009: a minority survivor cannot admit".into());
    drop(procs);
    let _ = std::fs::remove_dir_all(root);
    Ok(summary)
}
