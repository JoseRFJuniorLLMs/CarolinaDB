//! `QualificationManifest` (SPEC-010 §2): the immutable input of a campaign.
//!
//! The manifest records the exact source state (tree digest over every tracked source file, VCS
//! revision when available, and an explicit flag when a dirty working tree cannot be verified),
//! the toolchain, the enabled and disabled features with justification, the workloads, seeds and
//! budgets. It contains synthetic data only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use carolina_core::canon::{CanonValue, Canonical};
use carolina_core::error::{CoreError, CoreResult, ErrorCode};
use carolina_core::hash::{domain_hash, sha256, Hash256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunBudget {
    pub ops_per_schedule: u64,
    pub crash_nth_max: u64,
    pub storage_crash_nth_max: u64,
    pub checker_budget: u64,
    pub minimizer_runs: u64,
    pub btree_ops: u64,
}

impl Canonical for RunBudget {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fu64("btree_ops", self.btree_ops)
            .fu64("checker_budget", self.checker_budget)
            .fu64("crash_nth_max", self.crash_nth_max)
            .fu64("minimizer_runs", self.minimizer_runs)
            .fu64("ops_per_schedule", self.ops_per_schedule)
            .fu64("storage_crash_nth_max", self.storage_crash_nth_max)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(RunBudget {
            ops_per_schedule: v.field("ops_per_schedule")?.as_u64()?,
            crash_nth_max: v.field("crash_nth_max")?.as_u64()?,
            storage_crash_nth_max: v.field("storage_crash_nth_max")?.as_u64()?,
            checker_budget: v.field("checker_budget")?.as_u64()?,
            minimizer_runs: v.field("minimizer_runs")?.as_u64()?,
            btree_ops: v.field("btree_ops")?.as_u64()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualificationManifest {
    pub manifest_version: u32,
    pub source_revision: String,
    pub working_tree_digest: Hash256,
    pub working_tree_files: u64,
    /// True when the runner could not prove the tree clean (no VCS status available): the tree
    /// digest, not the revision, identifies the source.
    pub dirty_tree_unverified: bool,
    pub build_profile: String,
    pub toolchain_id: String,
    pub dependency_lock_digest: Hash256,
    pub storage_format_versions: Vec<String>,
    pub compiler_rule_versions: Vec<String>,
    pub protocol_versions: Vec<String>,
    pub supported_contract_fragment: Vec<String>,
    pub enabled_features: Vec<String>,
    pub disabled_features: Vec<(String, String)>,
    pub operation_definitions: Vec<(String, Hash256)>,
    pub schema_hashes: Vec<Hash256>,
    pub plan_hashes: Vec<Hash256>,
    pub client_durability_profiles: Vec<String>,
    pub authority_durability_policies: Vec<String>,
    pub transfer_decision_durability_policies: Vec<String>,
    pub failure_assumptions: Vec<String>,
    pub formal_model_refs: Vec<String>,
    pub formal_bounds: Vec<String>,
    pub refinement_mapping_refs: Vec<String>,
    pub catalog_capabilities: Vec<String>,
    pub identity_codec_manifest: Hash256,
    pub security_profile: String,
    pub topology: String,
    pub authority_configuration: String,
    pub placement: String,
    pub resource_limits: Vec<(String, u64)>,
    pub timeout_and_retry_policy: String,
    pub seed_set: Vec<u64>,
    pub schedules: Vec<String>,
    pub checker_versions: Vec<String>,
    pub run_budget: RunBudget,
    pub workload_definitions: Vec<String>,
    pub load_sweep: Vec<String>,
    pub baseline_configs: Vec<String>,
    pub durability_mode: String,
    pub test_root: String,
}

fn strs(v: &[String]) -> CanonValue {
    CanonValue::Array(v.iter().map(CanonValue::str).collect())
}
fn hashes(v: &[Hash256]) -> CanonValue {
    CanonValue::Array(v.iter().map(|h| h.to_canon()).collect())
}
fn u64s(v: &[u64]) -> CanonValue {
    CanonValue::Array(v.iter().map(|x| CanonValue::uint(*x)).collect())
}
fn str_pairs(v: &[(String, String)]) -> CanonValue {
    CanonValue::Array(
        v.iter()
            .map(|(a, b)| CanonValue::obj().fstr("name", a).fstr("value", b).build())
            .collect(),
    )
}
fn hash_pairs(v: &[(String, Hash256)]) -> CanonValue {
    CanonValue::Array(
        v.iter()
            .map(|(a, b)| CanonValue::obj().fc("hash", b).fstr("name", a).build())
            .collect(),
    )
}
fn u64_pairs(v: &[(String, u64)]) -> CanonValue {
    CanonValue::Array(
        v.iter()
            .map(|(a, b)| CanonValue::obj().fstr("name", a).fu64("value", *b).build())
            .collect(),
    )
}
fn rd_strs(v: &CanonValue, f: &str) -> CoreResult<Vec<String>> {
    v.field(f)?
        .as_array()?
        .iter()
        .map(|x| x.as_str().map(|s| s.to_string()))
        .collect()
}
fn rd_hashes(v: &CanonValue, f: &str) -> CoreResult<Vec<Hash256>> {
    v.field(f)?
        .as_array()?
        .iter()
        .map(Hash256::from_canon)
        .collect()
}
fn rd_u64s(v: &CanonValue, f: &str) -> CoreResult<Vec<u64>> {
    v.field(f)?.as_array()?.iter().map(|x| x.as_u64()).collect()
}
fn rd_str_pairs(v: &CanonValue, f: &str) -> CoreResult<Vec<(String, String)>> {
    v.field(f)?
        .as_array()?
        .iter()
        .map(|x| {
            Ok((
                x.field("name")?.as_str()?.to_string(),
                x.field("value")?.as_str()?.to_string(),
            ))
        })
        .collect()
}
fn rd_hash_pairs(v: &CanonValue, f: &str) -> CoreResult<Vec<(String, Hash256)>> {
    v.field(f)?
        .as_array()?
        .iter()
        .map(|x| {
            Ok((
                x.field("name")?.as_str()?.to_string(),
                Hash256::from_canon(x.field("hash")?)?,
            ))
        })
        .collect()
}
fn rd_u64_pairs(v: &CanonValue, f: &str) -> CoreResult<Vec<(String, u64)>> {
    v.field(f)?
        .as_array()?
        .iter()
        .map(|x| {
            Ok((
                x.field("name")?.as_str()?.to_string(),
                x.field("value")?.as_u64()?,
            ))
        })
        .collect()
}
fn rd_str(v: &CanonValue, f: &str) -> CoreResult<String> {
    Ok(v.field(f)?.as_str()?.to_string())
}

impl Canonical for QualificationManifest {
    fn to_canon(&self) -> CanonValue {
        CanonValue::obj()
            .fstr("authority_configuration", &self.authority_configuration)
            .f(
                "authority_durability_policies",
                strs(&self.authority_durability_policies),
            )
            .f("baseline_configs", strs(&self.baseline_configs))
            .fstr("build_profile", &self.build_profile)
            .f("catalog_capabilities", strs(&self.catalog_capabilities))
            .f("checker_versions", strs(&self.checker_versions))
            .f(
                "client_durability_profiles",
                strs(&self.client_durability_profiles),
            )
            .f("compiler_rule_versions", strs(&self.compiler_rule_versions))
            .fc("dependency_lock_digest", &self.dependency_lock_digest)
            .fbool("dirty_tree_unverified", self.dirty_tree_unverified)
            .f("disabled_features", str_pairs(&self.disabled_features))
            .fstr("durability_mode", &self.durability_mode)
            .f("enabled_features", strs(&self.enabled_features))
            .f("failure_assumptions", strs(&self.failure_assumptions))
            .f("formal_bounds", strs(&self.formal_bounds))
            .f("formal_model_refs", strs(&self.formal_model_refs))
            .fc("identity_codec_manifest", &self.identity_codec_manifest)
            .f("load_sweep", strs(&self.load_sweep))
            .fu32("manifest_version", self.manifest_version)
            .f(
                "operation_definitions",
                hash_pairs(&self.operation_definitions),
            )
            .fstr("placement", &self.placement)
            .f("plan_hashes", hashes(&self.plan_hashes))
            .f("protocol_versions", strs(&self.protocol_versions))
            .f(
                "refinement_mapping_refs",
                strs(&self.refinement_mapping_refs),
            )
            .f("resource_limits", u64_pairs(&self.resource_limits))
            .fc("run_budget", &self.run_budget)
            .f("schedules", strs(&self.schedules))
            .f("schema_hashes", hashes(&self.schema_hashes))
            .fstr("security_profile", &self.security_profile)
            .f("seed_set", u64s(&self.seed_set))
            .fstr("source_revision", &self.source_revision)
            .f(
                "storage_format_versions",
                strs(&self.storage_format_versions),
            )
            .f(
                "supported_contract_fragment",
                strs(&self.supported_contract_fragment),
            )
            .fstr("test_root", &self.test_root)
            .fstr("timeout_and_retry_policy", &self.timeout_and_retry_policy)
            .fstr("toolchain_id", &self.toolchain_id)
            .fstr("topology", &self.topology)
            .f(
                "transfer_decision_durability_policies",
                strs(&self.transfer_decision_durability_policies),
            )
            .f("workload_definitions", strs(&self.workload_definitions))
            .fc("working_tree_digest", &self.working_tree_digest)
            .fu64("working_tree_files", self.working_tree_files)
            .build()
    }
    fn from_canon(v: &CanonValue) -> CoreResult<Self> {
        Ok(QualificationManifest {
            manifest_version: v.field("manifest_version")?.as_u64()? as u32,
            source_revision: rd_str(v, "source_revision")?,
            working_tree_digest: Hash256::from_canon(v.field("working_tree_digest")?)?,
            working_tree_files: v.field("working_tree_files")?.as_u64()?,
            dirty_tree_unverified: v.field("dirty_tree_unverified")?.as_bool()?,
            build_profile: rd_str(v, "build_profile")?,
            toolchain_id: rd_str(v, "toolchain_id")?,
            dependency_lock_digest: Hash256::from_canon(v.field("dependency_lock_digest")?)?,
            storage_format_versions: rd_strs(v, "storage_format_versions")?,
            compiler_rule_versions: rd_strs(v, "compiler_rule_versions")?,
            protocol_versions: rd_strs(v, "protocol_versions")?,
            supported_contract_fragment: rd_strs(v, "supported_contract_fragment")?,
            enabled_features: rd_strs(v, "enabled_features")?,
            disabled_features: rd_str_pairs(v, "disabled_features")?,
            operation_definitions: rd_hash_pairs(v, "operation_definitions")?,
            schema_hashes: rd_hashes(v, "schema_hashes")?,
            plan_hashes: rd_hashes(v, "plan_hashes")?,
            client_durability_profiles: rd_strs(v, "client_durability_profiles")?,
            authority_durability_policies: rd_strs(v, "authority_durability_policies")?,
            transfer_decision_durability_policies: rd_strs(
                v,
                "transfer_decision_durability_policies",
            )?,
            failure_assumptions: rd_strs(v, "failure_assumptions")?,
            formal_model_refs: rd_strs(v, "formal_model_refs")?,
            formal_bounds: rd_strs(v, "formal_bounds")?,
            refinement_mapping_refs: rd_strs(v, "refinement_mapping_refs")?,
            catalog_capabilities: rd_strs(v, "catalog_capabilities")?,
            identity_codec_manifest: Hash256::from_canon(v.field("identity_codec_manifest")?)?,
            security_profile: rd_str(v, "security_profile")?,
            topology: rd_str(v, "topology")?,
            authority_configuration: rd_str(v, "authority_configuration")?,
            placement: rd_str(v, "placement")?,
            resource_limits: rd_u64_pairs(v, "resource_limits")?,
            timeout_and_retry_policy: rd_str(v, "timeout_and_retry_policy")?,
            seed_set: rd_u64s(v, "seed_set")?,
            schedules: rd_strs(v, "schedules")?,
            checker_versions: rd_strs(v, "checker_versions")?,
            run_budget: RunBudget::from_canon(v.field("run_budget")?)?,
            workload_definitions: rd_strs(v, "workload_definitions")?,
            load_sweep: rd_strs(v, "load_sweep")?,
            baseline_configs: rd_strs(v, "baseline_configs")?,
            durability_mode: rd_str(v, "durability_mode")?,
            test_root: rd_str(v, "test_root")?,
        })
    }
}

impl QualificationManifest {
    pub const VERSION: u32 = 1;

    pub fn manifest_hash(&self) -> Hash256 {
        domain_hash("astra.qualification-manifest.v1", &self.encode())
    }
}

/// Deterministic digest of the source tree: sorted relative paths of every file under the
/// tracked roots, each hashed as `len(path) || path || len(bytes) || bytes`.
pub fn working_tree_digest(root: &Path) -> std::io::Result<(Hash256, u64)> {
    let mut files: Vec<PathBuf> = Vec::new();
    for top in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "README.md",
        "crates",
        "fixtures",
        "md",
        "tools",
        "docs",
        "models",
    ] {
        let p = root.join(top);
        if p.is_file() {
            files.push(p);
        } else if p.is_dir() {
            walk(&p, &mut files)?;
        }
    }
    files.sort();
    let mut payload = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(f)?;
        payload.extend_from_slice(&(rel.len() as u64).to_le_bytes());
        payload.extend_from_slice(rel.as_bytes());
        payload.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        payload.extend_from_slice(&bytes);
    }
    Ok((sha256(&payload), files.len() as u64))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort();
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.is_file() {
            out.push(p);
        }
    }
    Ok(())
}

/// VCS revision when a git checkout is present; `no-vcs` otherwise. Dirty state is not verified
/// here (no git invocation), so callers record `dirty_tree_unverified = true` unconditionally.
pub fn source_revision(root: &Path) -> String {
    let head = match std::fs::read_to_string(root.join(".git").join("HEAD")) {
        Ok(h) => h.trim().to_string(),
        Err(_) => return "no-vcs".into(),
    };
    if let Some(r) = head.strip_prefix("ref: ") {
        if let Ok(h) = std::fs::read_to_string(root.join(".git").join(r)) {
            return format!("git:{}", h.trim());
        }
        if let Ok(packed) = std::fs::read_to_string(root.join(".git").join("packed-refs")) {
            for line in packed.lines() {
                if let Some((hash, name)) = line.split_once(' ') {
                    if name.trim() == r {
                        return format!("git:{}", hash.trim());
                    }
                }
            }
        }
        return format!("git-ref:{r}");
    }
    format!("git:{head}")
}

pub fn toolchain_id(root: &Path) -> String {
    let channel = std::fs::read_to_string(root.join("rust-toolchain.toml"))
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("channel"))
                .map(|l| l.to_string())
        })
        .unwrap_or_else(|| "channel = unknown".into());
    format!(
        "{}; {}; {}-{}; carolina-qualify {}",
        env!("CAROLINA_RUSTC_VERSION"),
        channel.trim(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        env!("CARGO_PKG_VERSION")
    )
}

pub fn dependency_lock_digest(root: &Path) -> Hash256 {
    match std::fs::read(root.join("Cargo.lock")) {
        Ok(b) => sha256(&b),
        Err(_) => Hash256::ZERO,
    }
}

/// Keyed view for reports.
pub fn as_report_lines(m: &QualificationManifest) -> BTreeMap<&'static str, String> {
    let mut out = BTreeMap::new();
    out.insert("source_revision", m.source_revision.clone());
    out.insert("working_tree_digest", m.working_tree_digest.to_string());
    out.insert("toolchain_id", m.toolchain_id.clone());
    out.insert("enabled_features", m.enabled_features.join(", "));
    out.insert("durability_mode", m.durability_mode.clone());
    out
}

pub fn invalid(msg: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidManifest, msg.to_string())
}
