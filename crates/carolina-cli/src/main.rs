//! `carolina` command-line interface.
//!
//! Compilation and explanation are deterministic artifact operations; they never activate a
//! plan (SPEC-004 §13). Storage/journal/qualification commands are added with their crates.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use carolina_compiler::{
    check_artifacts, compile, explain, library, rules, CompileInput, ConsistencyCertificate,
    OperationPlan,
};
use carolina_core::canon::Canonical;
use carolina_core::limits::Limits;
use carolina_lang::lower::lower_module;
use carolina_lang::parser::parse_module;

fn usage() -> ExitCode {
    eprintln!(
        "carolina — CarolinaDB tools\n\n\
         USAGE:\n  carolina compile <module.cdl> [--out <dir>]     compile to canonical plans + certificate\n  \
         carolina explain <module.cdl> [operation]       EXPLAIN candidates, evidence and assumptions\n  \
         carolina plan <module.cdl> [operation]          print the canonical OperationPlan artifact(s)\n  \
         carolina graph invariants <module.cdl>          print IDC templates, affected records/invariants and interaction edges\n  \
         carolina check <dir>                            verify plans/certificate written by `compile`\n  \
         carolina ir <module.cdl>                         print canonical IR bytes and hashes\n  \
         carolina fixtures                                list embedded SPEC-003 fixtures\n  \
         carolina verify <data-dir>                       recover a local store and verify its structure\n  \
         carolina workload <data-dir> [available]         run the SPEC-014 §4 reserve/release workload locally\n  \
         carolina qualify [--out <dir>] [--quick] [--seeds a,b] [--ops n] [--crash-nth n] [--test-root <dir>] [--durability sync|unsafe] [--skip ID]...\n  \
         carolina simulate --seed <n> [--fault Point#nth] [--ops n] [--out <dir>]   one deterministic local schedule + history checker\n  \
         carolina replay --bundle <dir>                   re-run and re-check a bundle's schedule\n  \
         carolina minimize --bundle <dir>                 shrink a failing bundle's schedule\n  \
         carolina report --campaign <dir>                 print a campaign/run verdict\n  \
         carolina node status --addr <host:port>          read a running node's status and counters\n"
    );
    ExitCode::from(2)
}

fn load(path: &Path) -> Result<carolina_lang::ir::ModuleIR, String> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let ast = parse_module(&src, &Limits::v1()).map_err(|e| format!("parse error: {e}"))?;
    let (ir, _) = lower_module(&ast, None).map_err(|e| format!("lowering error: {e}"))?;
    Ok(ir)
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let option_spec: Option<(&[&str], &[&str])> = match args.first().map(String::as_str) {
        Some("qualify") => Some((
            &[
                "--out",
                "--seeds",
                "--ops",
                "--crash-nth",
                "--test-root",
                "--durability",
                "--skip",
            ],
            &["--quick", "--keep-bundles"],
        )),
        Some("simulate") => Some((&["--seed", "--fault", "--ops", "--out", "--test-root"], &[])),
        Some("replay" | "minimize") => Some((&["--bundle", "--test-root"], &[])),
        Some("report") => Some((&["--campaign"], &[])),
        Some("node") => Some((&["--addr", "--cluster", "--timeout-ms"], &[])),
        _ => None,
    };
    if let Some((values, switches)) = option_spec {
        // `node` takes a subcommand word before its options; every other command's options
        // start right after the command itself.
        let first_flag = if args.first().map(String::as_str) == Some("node") {
            2
        } else {
            1
        };
        if let Err(e) = validate_flags(args.get(first_flag..).unwrap_or(&[]), values, switches) {
            eprintln!("invalid configuration: {e}");
            return Ok(ExitCode::from(2));
        }
    }
    match args.first().map(|s| s.as_str()) {
        Some("compile") => {
            let path = args.get(1).ok_or("missing module path")?;
            let out_dir = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|i| args.get(i + 1))
                .map(PathBuf::from);
            let ir = load(Path::new(path))?;
            let out = compile(&CompileInput::local(ir.clone()))
                .map_err(|e| format!("compile failed: {e}"))?;
            for p in &out.plans {
                println!(
                    "plan {:<24} {:<14} {}",
                    p.operation_name,
                    p.profile.family.label(),
                    p.plan_hash()
                );
            }
            println!("certificate {}", out.certificate.certificate_hash());
            for d in &out.diagnostics {
                println!("note: {d}");
            }
            if let Some(dir) = out_dir {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                for p in &out.plans {
                    std::fs::write(
                        dir.join(format!("plan-{}.json", p.operation_name)),
                        p.encode(),
                    )
                    .map_err(|e| e.to_string())?;
                }
                std::fs::write(dir.join("certificate.json"), out.certificate.encode())
                    .map_err(|e| e.to_string())?;
                std::fs::write(dir.join("module.ir.json"), ir.encode())
                    .map_err(|e| e.to_string())?;
                println!("artifacts written to {}", dir.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("explain") => {
            let path = args.get(1).ok_or("missing module path")?;
            let ir = load(Path::new(path))?;
            let out = compile(&CompileInput::local(ir.clone()))
                .map_err(|e| format!("compile failed: {e}"))?;
            match args.get(2) {
                Some(op) => match explain::explain_operation(&ir, &out, op) {
                    Some(t) => print!("{t}"),
                    None => return Err(format!("unknown operation {op}")),
                },
                None => print!("{}", explain::explain_all(&ir, &out)),
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("plan") => {
            let path = args.get(1).ok_or("missing module path")?;
            let ir = load(Path::new(path))?;
            let out = compile(&CompileInput::local(ir.clone()))
                .map_err(|e| format!("compile failed: {e}"))?;
            let filter = args.get(2);
            let mut shown = 0usize;
            for p in &out.plans {
                if filter.is_some_and(|f| f != &p.operation_name) {
                    continue;
                }
                println!(
                    "plan {} {} {}",
                    p.operation_name,
                    p.profile.family.label(),
                    p.plan_hash()
                );
                println!("{}", String::from_utf8_lossy(&p.encode()));
                shown += 1;
            }
            if shown == 0 {
                return Err(format!(
                    "unknown operation {}",
                    filter.map(String::as_str).unwrap_or("")
                ));
            }
            println!("certificate {}", out.certificate.certificate_hash());
            println!("note: a plan is an artifact; activation requires catalog registration and qualification evidence (SPEC-004 §13)");
            Ok(ExitCode::SUCCESS)
        }
        Some("graph") => {
            if args.get(1).map(String::as_str) != Some("invariants") {
                return Err("usage: carolina graph invariants <module.cdl>".into());
            }
            let path = args.get(2).ok_or("missing module path")?;
            let ir = load(Path::new(path))?;
            let out = compile(&CompileInput::local(ir.clone()))
                .map_err(|e| format!("compile failed: {e}"))?;
            print!("{}", render_invariant_graph(&ir, &out));
            Ok(ExitCode::SUCCESS)
        }
        Some("check") => {
            let dir = PathBuf::from(args.get(1).ok_or("missing artifact dir")?);
            let cert_bytes =
                std::fs::read(dir.join("certificate.json")).map_err(|e| e.to_string())?;
            let cert = ConsistencyCertificate::decode(&cert_bytes, &Limits::v1())
                .map_err(|e| format!("certificate: {e}"))?;
            let mut plans = Vec::new();
            for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("plan-") && name.ends_with(".json") {
                    let bytes = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
                    plans.push(
                        OperationPlan::decode(&bytes, &Limits::v1())
                            .map_err(|e| format!("{name}: {e}"))?,
                    );
                }
            }
            let report = check_artifacts(
                &plans,
                &cert,
                &rules::AnalysisRuleManifest::v1(),
                &library::ProtocolLibraryManifest::v1(),
            )
            .map_err(|e| format!("CHECK FAILED: {e}"))?;
            println!("structurally valid plans: {}", report.structurally_valid);
            println!("proof obligations checked: {}", report.obligations_checked);
            println!("certificate: {}", report.certificate_hash);
            println!("note: structural validity and discharged obligations do not imply runtime qualification (SPEC-004 §13)");
            Ok(ExitCode::SUCCESS)
        }
        Some("ir") => {
            let path = args.get(1).ok_or("missing module path")?;
            let ir = load(Path::new(path))?;
            let h = ir.hashes();
            println!("{}", String::from_utf8_lossy(&ir.encode()));
            println!("schema_hash={}", h.schema_hash);
            println!("module_hash={}", h.module_hash);
            Ok(ExitCode::SUCCESS)
        }
        Some("node") => {
            if args.get(1).map(String::as_str) != Some("status") {
                return Err(
                    "usage: carolina node status --addr <host:port> [--cluster <name>]".into(),
                );
            }
            let flag = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1))
                    .cloned()
            };
            let addr: std::net::SocketAddr = flag("--addr")
                .ok_or("missing --addr")?
                .parse()
                .map_err(|e| format!("invalid --addr: {e}"))?;
            let cluster = carolina_core::ids::ClusterId::derive(
                &flag("--cluster").unwrap_or("carolina".into()),
            );
            let timeout = std::time::Duration::from_millis(
                flag("--timeout-ms")
                    .map(|t| t.parse::<u64>())
                    .transpose()
                    .map_err(|e| format!("invalid --timeout-ms: {e}"))?
                    .unwrap_or(3000),
            );
            let mut client = carolina_node::Client::connect(
                addr,
                cluster,
                carolina_wire::negotiation::EndpointRole::Admin,
                timeout,
            )
            .map_err(|e| format!("connect: {e}"))?;
            let s = client.status().map_err(|e| format!("status: {e}"))?;
            println!(
                "node {} {} term={} leader={} readiness={}",
                carolina_core::hash::hex_encode(&s.node.0[..4]),
                s.role,
                s.term,
                s.leader
                    .map(|l| carolina_core::hash::hex_encode(&l.0[..4]))
                    .unwrap_or_else(|| "-".into()),
                s.readiness
            );
            println!(
                "commit_index={} applied_index={} catalog_generation={} durable_commit_seq={}",
                s.commit_index, s.applied_index, s.catalog_generation.0, s.durable_commit_seq
            );
            println!("state_digest={}", s.state_digest);
            if let Some(f) = &s.failure {
                println!("failure: {f}");
            }
            if s.metrics.is_empty() {
                println!("note: this node reports no counters");
            } else {
                println!("counters:");
                for (k, v) in &s.metrics {
                    println!("  {k:<40} {v}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("fixtures") => {
            for n in carolina_lang::fixtures::FIXTURE_NAMES {
                println!("{n}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("verify") => {
            let dir = PathBuf::from(args.get(1).ok_or("missing data dir")?);
            let mut store =
                carolina_storage::Store::open(&dir, carolina_storage::StoreOptions::default())
                    .map_err(|e| format!("open failed: {e}"))?;
            use carolina_storage::DurableStorageKernel;
            let report = store
                .verify(carolina_storage::kernel::VerifyMode::Full)
                .map_err(|e| format!("VERIFY FAILED: {e}"))?;
            println!("readiness: {:?}", store.readiness());
            println!("durable_commit_seq: {}", store.durable_commit_seq().0);
            println!(
                "pages: {}  leaves: {}  entries: {}",
                report.pages, report.leaves, report.entries
            );
            println!(
                "journal_frames: {}  prepared_in_doubt: {}",
                report.journal_frames, report.prepared_in_doubt
            );
            Ok(ExitCode::SUCCESS)
        }
        Some("qualify") => qualify(&args[1..]),
        Some("simulate") => simulate(&args[1..]),
        Some("replay") => replay(&args[1..], false),
        Some("minimize") => replay(&args[1..], true),
        Some("report") => {
            let dir =
                PathBuf::from(flag(&args[1..], "--campaign").ok_or("missing --campaign <dir>")?);
            let b = carolina_qualify::bundle::load_bundle(&dir)
                .map_err(|e| format!("cannot load bundle: {e}"))?;
            print!("{}", b.verdict.summary());
            println!(
                "manifest: source {} tree {} toolchain {}",
                b.manifest.source_revision, b.manifest.working_tree_digest, b.manifest.toolchain_id
            );
            Ok(ExitCode::from(b.verdict.exit_code()))
        }
        Some("workload") => {
            let dir = PathBuf::from(args.get(1).ok_or("missing data dir")?);
            let available: i64 = args
                .get(2)
                .map(|s| s.parse().map_err(|_| "available must be an integer"))
                .transpose()?
                .unwrap_or(5);
            workload(&dir, available)
        }
        _ => Ok(usage()),
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn validate_flags(args: &[String], values: &[&str], switches: &[&str]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut i = 0;
    while i < args.len() {
        let name = args[i].as_str();
        if !seen.insert(name) && name != "--skip" {
            return Err(format!("duplicate option {name}"));
        }
        if values.contains(&name) {
            let value = args
                .get(i + 1)
                .ok_or_else(|| format!("{name} requires a value"))?;
            if value.starts_with("--") || value.trim().is_empty() {
                return Err(format!("{name} requires a value"));
            }
            i += 2;
        } else if switches.contains(&name) {
            i += 1;
        } else {
            return Err(format!("unknown option {name}"));
        }
    }
    Ok(())
}

/// Where the repository data the campaigns read (`fixtures/`, `models/`) lives.
///
/// Discovered at run time, never at compile time. Baking `CARGO_MANIFEST_DIR` into the binary put
/// the absolute path of the build machine inside every release artifact: it made the build
/// unreproducible (two checkouts in different directories produced different bytes) and it pointed
/// at a directory that exists on no other machine. `CAROLINA_REPO_ROOT` overrides; otherwise the
/// nearest ancestor of the working directory that actually holds `fixtures/` wins.
fn repo_root() -> PathBuf {
    if let Some(explicit) = std::env::var_os("CAROLINA_REPO_ROOT") {
        return PathBuf::from(explicit);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut here: &Path = cwd.as_path();
    loop {
        if here.join("fixtures").is_dir() {
            return here.to_path_buf();
        }
        match here.parent() {
            Some(up) => here = up,
            None => return cwd,
        }
    }
}

/// The repository root, or a configuration error naming exactly what is missing.
///
/// The campaigns read `fixtures/` and `models/`, which a binary release does not ship. Without this
/// check the campaign runs anyway and reports `overall FAIL` with "golden bytes missing", which
/// reads as "the database is broken" when it means "this command needs the repository".
fn require_repo_root() -> Result<PathBuf, String> {
    let root = repo_root();
    if root.join("fixtures").is_dir() {
        return Ok(root);
    }
    Err(format!(
        "this command reads the repository's `fixtures/`, which a binary release does not ship; none was found under {} or any of its ancestors. Run it from a checkout of the matching commit, or point CAROLINA_REPO_ROOT at one.",
        root.display()
    ))
}

fn default_test_root() -> PathBuf {
    std::env::temp_dir().join("carolina-qualify-scratch")
}

/// `carolina qualify`: SPEC-010 campaign over the local profile; exit 0 pass, 1 fail, 2 invalid
/// configuration, 3 inconclusive/not run.
fn qualify(args: &[String]) -> Result<ExitCode, String> {
    use carolina_qualify::runner::{run_campaign, CampaignConfig, ConfigError};
    let parsed = (|| -> Result<CampaignConfig, String> {
        let root = require_repo_root()?;
        let test_root = flag(args, "--test-root")
            .map(PathBuf::from)
            .unwrap_or_else(default_test_root);
        let mut cfg = if has(args, "--quick") {
            CampaignConfig::quick(&root, &test_root)
        } else {
            CampaignConfig::standard(&root, &test_root)
        };
        cfg.out_dir = flag(args, "--out").map(PathBuf::from);
        if let Some(s) = flag(args, "--seeds") {
            cfg.seeds = s.split(',').map(|x| {
            x.trim().parse().map_err(|_| format!("invalid seed `{x}`; --seeds requires comma-separated unsigned integers"))
        }).collect::<Result<Vec<_>, _>>()?;
        }
        if let Some(n) = flag(args, "--ops") {
            cfg.ops_per_schedule = n.parse().map_err(|_| "--ops must be an integer")?;
        }
        if let Some(n) = flag(args, "--crash-nth") {
            cfg.crash_nth_max = n.parse().map_err(|_| "--crash-nth must be an integer")?;
        }
        if let Some(d) = flag(args, "--durability") {
            cfg.durability_mode = d.into();
        }
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--skip" {
                if let Some(id) = args.get(i + 1) {
                    cfg.skip.push(id.clone());
                }
            }
            i += 1;
        }
        cfg.keep_passing_bundles = has(args, "--keep-bundles");
        Ok(cfg)
    })();
    let cfg = match parsed {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("invalid configuration: {e}");
            return Ok(ExitCode::from(2));
        }
    };
    match run_campaign(&cfg) {
        Ok(report) => {
            print!("{}", report.verdict.summary());
            for d in &report.bundle_dirs {
                println!("bundle: {}", d.display());
            }
            Ok(ExitCode::from(report.verdict.exit_code()))
        }
        Err(e @ ConfigError::InvalidBudget(_))
        | Err(e @ ConfigError::UnsafeDurability(_))
        | Err(e @ ConfigError::TestRootOutsideAllowlist(_)) => {
            eprintln!("{e}");
            Ok(ExitCode::from(2))
        }
        Err(e) => Err(e.to_string()),
    }
}

fn parse_fault(s: &str) -> Result<(carolina_storage::io::FaultPoint, u32), String> {
    let (p, n) = s.split_once('#').ok_or("--fault expects Point#nth")?;
    let point = carolina_qualify::local::fault_point_from_label(p)
        .ok_or_else(|| format!("unknown fault point {p}"))?;
    let nth = n.parse().map_err(|_| "nth must be an integer")?;
    if nth == 0 {
        return Err("fault occurrence must be greater than zero".into());
    }
    Ok((point, nth))
}

fn print_checks(results: &[carolina_qualify::verdict::CheckResult]) {
    for c in results {
        println!("  {:<8} {:<14} {}", c.id, c.status.label(), c.detail);
    }
}

/// `carolina simulate`: one deterministic schedule, its history and the checker's verdict.
fn simulate(args: &[String]) -> Result<ExitCode, String> {
    use carolina_qualify::local::generate_schedule;
    use carolina_qualify::runner::{check_schedule, validate_test_root};
    let seed: u64 = flag(args, "--seed")
        .ok_or("missing --seed")?
        .parse()
        .map_err(|_| "--seed must be an integer")?;
    let ops: usize = flag(args, "--ops")
        .map(|n| n.parse().map_err(|_| "--ops must be an integer"))
        .transpose()?
        .unwrap_or(40);
    if ops == 0 {
        return Err("--ops must be greater than zero".into());
    }
    let fault = flag(args, "--fault").map(parse_fault).transpose()?;
    let test_root = validate_test_root(
        &repo_root(),
        &flag(args, "--test-root")
            .map(PathBuf::from)
            .unwrap_or_else(default_test_root),
    )
    .map_err(|e| e.to_string())?;
    let sched = generate_schedule(seed, ops, fault);
    let (history, results, verify_error) = check_schedule(&test_root, &sched, 200_000)?;
    println!(
        "schedule: seed {seed}, {} ops, fault {:?}",
        sched.ops.len(),
        sched.fault
    );
    println!(
        "history: {} events, trace {}",
        history.len(),
        history.trace_digest()
    );
    if let Some(v) = verify_error {
        println!("storage verifier: {v}");
    }
    print_checks(&results);
    if let Some(out) = flag(args, "--out") {
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        std::fs::write(out.join("history.jsonl"), history.to_jsonl()).map_err(|e| e.to_string())?;
        std::fs::write(out.join("schedule.json"), sched.encode()).map_err(|e| e.to_string())?;
        println!("written: {}", out.display());
    }
    let failed = results
        .iter()
        .any(|c| c.status == carolina_qualify::verdict::Status::Fail);
    let inconclusive = results.iter().any(|c| {
        matches!(
            c.status,
            carolina_qualify::verdict::Status::Inconclusive
                | carolina_qualify::verdict::Status::NotRun
        )
    });
    Ok(ExitCode::from(if failed {
        1
    } else if inconclusive {
        3
    } else {
        0
    }))
}

/// `carolina replay --bundle` re-runs and re-checks; `carolina minimize --bundle` shrinks.
fn replay(args: &[String], do_minimize: bool) -> Result<ExitCode, String> {
    use carolina_qualify::bundle::load_bundle;
    use carolina_qualify::minimize::minimize;
    use carolina_qualify::runner::{check_schedule, validate_test_root};
    use carolina_qualify::verdict::Status;
    let dir = PathBuf::from(flag(args, "--bundle").ok_or("missing --bundle <dir>")?);
    let b = load_bundle(&dir).map_err(|e| format!("cannot load bundle: {e}"))?;
    let sched = b
        .schedule
        .ok_or("bundle has no schedule.json (campaign-level bundle?)")?;
    let test_root = validate_test_root(
        &repo_root(),
        &flag(args, "--test-root")
            .map(PathBuf::from)
            .unwrap_or_else(default_test_root),
    )
    .map_err(|e| e.to_string())?;
    let budget = b.manifest.run_budget.checker_budget as usize;
    let (history, results, verify_error) = check_schedule(&test_root, &sched, budget)?;
    let same_trace = history.trace_digest() == b.verdict.trace_digest;
    println!(
        "replayed seed {} fault {:?}: {} events, trace {} (matches bundle: {:?})",
        sched.seed,
        sched.fault,
        history.len(),
        history.trace_digest(),
        same_trace
    );
    if let Some(v) = &verify_error {
        println!("storage verifier: {v}");
    }
    print_checks(&results);
    let fails =
        |r: &[carolina_qualify::verdict::CheckResult]| r.iter().any(|c| c.status == Status::Fail);
    let incomplete = results
        .iter()
        .any(|c| matches!(c.status, Status::Inconclusive | Status::NotRun));
    if !do_minimize {
        return Ok(ExitCode::from(
            if fails(&results) || verify_error.is_some() || !same_trace {
                1
            } else if incomplete {
                3
            } else {
                0
            },
        ));
    }
    if !fails(&results) && verify_error.is_none() {
        if incomplete {
            println!(
                "cannot establish a failure to minimize: checker is inconclusive or did not run"
            );
            return Ok(ExitCode::from(3));
        }
        if !same_trace {
            println!("replay differs from retained history; no checker failure to minimize");
            return Ok(ExitCode::FAILURE);
        }
        println!("nothing to minimize: the schedule does not fail");
        return Ok(ExitCode::SUCCESS);
    }
    let (m, runs) = minimize(
        &sched,
        |c| match check_schedule(&test_root, c, budget) {
            Ok((_, r, v)) => fails(&r) || v.is_some(),
            Err(_) => false,
        },
        b.manifest.run_budget.minimizer_runs as usize,
    );
    println!(
        "minimized: {} -> {} ops in {runs} runs",
        sched.ops.len(),
        m.ops.len()
    );
    for op in &m.ops {
        println!("  {}", String::from_utf8_lossy(&op.encode()));
    }
    std::fs::write(dir.join("schedule.min.json"), m.encode()).map_err(|e| e.to_string())?;
    println!("written: {}", dir.join("schedule.min.json").display());
    Ok(ExitCode::from(1))
}

/// SPEC-014 §4: reserve/release with receipts, duplicate delivery, contention on the last unit,
/// a lost reply resolved from durable state after reopening the directory.
fn workload(dir: &Path, available: i64) -> Result<ExitCode, String> {
    use carolina_core::canon::Canonical;
    use carolina_lang::types::Value;
    use carolina_runtime::engine::{make_invoke, EngineOptions, LocalEngine};
    use carolina_runtime::LocalCatalog;
    use carolina_wire::records::{ClientReplyV1, ResolveReplyV1, ResolveRequestV1};
    let src = carolina_lang::fixtures::fixture_source("inventory_reserve_release").unwrap();
    let catalog =
        std::sync::Arc::new(LocalCatalog::from_source(src).map_err(|e| format!("catalog: {e}"))?);
    let item = [0x11u8; 16];
    let fresh = !dir.join("MANIFEST.A").exists();
    let opts = EngineOptions::local(carolina_storage::StoreOptions::default());
    let mut e = if fresh {
        let mut e = LocalEngine::create(dir, catalog, opts).map_err(|e| format!("create: {e}"))?;
        e.load_rows(
            "seed",
            "Item",
            &[(
                Value::Uuid(item),
                vec![
                    ("id", Value::Uuid(item)),
                    ("available", Value::I64(available)),
                    ("reserved", Value::I64(0)),
                    ("total", Value::I64(available)),
                ],
            )],
        )
        .map_err(|e| format!("seed: {e}"))?;
        println!("created {} with Item available={available}", dir.display());
        e
    } else {
        let e = LocalEngine::open(dir, catalog, opts).map_err(|e| format!("open: {e}"))?;
        println!("opened {} (home epoch {})", dir.display(), e.home_epoch().0);
        e
    };
    let show = |label: &str, r: &ClientReplyV1| match r {
        ClientReplyV1::Committed(rc) | ClientReplyV1::Rejected(rc) => println!(
            "{label:<28} {:<9} txn={} receipt={} result={}",
            rc.outcome.label(),
            carolina_core::hash::hex_encode(&rc.txn_id.0),
            rc.receipt_digest(),
            String::from_utf8_lossy(&rc.exact_result_bytes)
        ),
        other => println!("{label:<28} {}", other.kind()),
    };
    let mk = |e: &LocalEngine, id: &str, op: &str, q: i64, rid: u8| {
        make_invoke(
            &e.catalog,
            "tenant",
            id,
            op,
            vec![Value::Uuid(item), Value::I64(q), Value::Uuid([rid; 16])],
        )
        .unwrap()
    };
    let r1 = mk(&e, "reserve-1", "reserve", 2, 1);
    let a = e.invoke(&r1);
    show("reserve 2 (req reserve-1)", &a);
    let b = e.invoke(&r1);
    show("same request again", &b);
    if let (ClientReplyV1::Committed(x), ClientReplyV1::Committed(y)) = (&a, &b) {
        println!(
            "{:<28} {}",
            "identical receipt bytes",
            x.encode() == y.encode()
        );
    }
    let rel = mk(&e, "release-1", "release", 2, 1);
    show("release 2 (req release-1)", &e.invoke(&rel));
    let dup = mk(&e, "release-2", "release", 2, 1);
    show("duplicate release", &e.invoke(&dup));
    let big = mk(&e, "reserve-all", "reserve", available + 1, 2);
    show("reserve more than available", &e.invoke(&big));
    let row = e
        .read_row("Item", &Value::Uuid(item))
        .map_err(|e| e.to_string())?
        .unwrap();
    println!("{:<28} {:?}", "Item row", row);
    // lost reply: resolve from durable state
    match e.resolve(&ResolveRequestV1 {
        request_key: r1.content.request_key,
        expected_request_hash: r1.request_hash,
    }) {
        ResolveReplyV1::Terminal(r) => show("resolve(reserve-1)", &r),
        other => println!("{:<28} {other:?}", "resolve(reserve-1)"),
    }
    let info = e.checkpoint().map_err(|e| e.to_string())?;
    println!(
        "checkpoint lsn={} seq={} pages_written={}",
        info.checkpoint_lsn.0, info.checkpoint_commit_seq.0, info.pages_written
    );
    println!("note: local slice only (SPEC-014 MVP-2); no replication or multi-IDC claim");
    Ok(ExitCode::SUCCESS)
}

/// `carolina graph invariants`: the conservative closure the compiler derived (SPEC-004 §4/§6) —
/// IDC templates with their records and invariants, every operation's affected records and
/// templates, the directed interaction edges and the implicit invariants the closure added.
fn render_invariant_graph(
    ir: &carolina_lang::ir::ModuleIR,
    out: &carolina_compiler::CompileOutput,
) -> String {
    use std::fmt::Write as _;
    let rec = |id: carolina_core::ids::RecordId| {
        ir.record(id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|_| format!("{id:?}"))
    };
    let inv = |id: carolina_core::ids::InvariantId| {
        ir.invariant(id)
            .map(|i| i.name.clone())
            .unwrap_or_else(|_| format!("{id:?}"))
    };
    let op = |r: carolina_core::ids::OperationRef| {
        ir.operations
            .iter()
            .find(|o| o.identity == r)
            .map(|o| format!("{}@{}", o.name, r.version))
            .unwrap_or_else(|| format!("{r:?}"))
    };
    let join = |v: Vec<String>| {
        if v.is_empty() {
            "-".to_string()
        } else {
            v.join(", ")
        }
    };
    let mut s = String::new();
    let _ = writeln!(s, "IDC templates ({}):", out.closure.templates.len());
    for t in &out.closure.templates {
        let _ = writeln!(
            s,
            "  {} [{}] records: {} invariants: {}",
            t.name,
            if t.per_key { "per-key" } else { "global" },
            join(t.records.iter().map(|r| rec(*r)).collect()),
            join(t.invariants.iter().map(|i| inv(*i)).collect())
        );
    }
    let _ = writeln!(s, "Operations ({}):", out.closure.op_records.len());
    for (o, records) in &out.closure.op_records {
        let templates = out
            .closure
            .op_templates
            .get(o)
            .map(|ts| {
                ts.iter()
                    .filter_map(|i| out.closure.templates.get(*i))
                    .map(|t| t.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let invariants = out
            .closure
            .op_invariants
            .get(o)
            .map(|is| is.iter().map(|i| inv(*i)).collect())
            .unwrap_or_default();
        let _ = writeln!(
            s,
            "  {} records: {} invariants: {} templates: {}",
            op(*o),
            join(records.iter().map(|r| rec(*r)).collect()),
            join(invariants),
            join(templates)
        );
    }
    let _ = writeln!(s, "Interaction edges ({}):", out.interaction_graph.len());
    for e in &out.interaction_graph {
        let _ = writeln!(
            s,
            "  {} -> {} {} [{}] invariants: {}",
            op(e.predecessor),
            op(e.successor),
            e.kind.label(),
            e.key_relation,
            join(e.invariant_refs.iter().map(|i| inv(*i)).collect())
        );
    }
    if !out.closure.implicit_invariants.is_empty() {
        let _ = writeln!(
            s,
            "Implicit invariants ({}):",
            out.closure.implicit_invariants.len()
        );
        for i in &out.closure.implicit_invariants {
            let _ = writeln!(s, "  {i}");
        }
    }
    s.push_str("note: the closure is conservative (SPEC-004 §4); it is an analysis artifact, not an activation\n");
    s
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("carolina-cli-unit-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.cdl"));
        std::fs::write(
            &path,
            carolina_lang::fixtures::fixture_source(name).unwrap(),
        )
        .unwrap();
        path
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// `carolina plan` / `carolina graph invariants` are deterministic artifact operations
    /// (SPEC-004 §13): they succeed on every fixture, refuse unknown operations and never write.
    #[test]
    fn plan_and_graph_commands_are_artifact_operations() {
        let path = fixture_file("account_transfer");
        let p = path.to_string_lossy().to_string();
        assert!(run(&args(&["plan", &p])).is_ok());
        assert!(run(&args(&["plan", &p, "transfer"])).is_ok());
        assert!(run(&args(&["plan", &p, "no_such_operation"]))
            .unwrap_err()
            .contains("unknown operation"));
        assert!(run(&args(&["graph", "invariants", &p])).is_ok());
        assert!(run(&args(&["graph", "records", &p])).is_err());
        let ir = load(&path).unwrap();
        let out = compile(&CompileInput::local(ir.clone())).unwrap();
        let text = render_invariant_graph(&ir, &out);
        assert!(text.contains("IDC templates"));
        assert!(text.contains("conservation"));
        assert!(text.contains("transfer@1"));
        assert!(text.contains("Interaction edges"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// `compile --out` followed by `check` round-trips the artifacts; a tampered plan file fails.
    #[test]
    fn compile_out_then_check_roundtrips_and_detects_tampering() {
        let path = fixture_file("inventory_sell");
        let p = path.to_string_lossy().to_string();
        let out_dir = path.parent().unwrap().join("artifacts");
        let o = out_dir.to_string_lossy().to_string();
        assert!(run(&args(&["compile", &p, "--out", &o])).is_ok());
        assert!(run(&args(&["check", &o])).is_ok());
        let plan_file = out_dir.join("plan-sell.json");
        let mut bytes = std::fs::read(&plan_file).unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        let tampered = text.replacen(
            "\"remote_participants\":\"0\"",
            "\"remote_participants\":\"1\"",
            1,
        );
        assert_ne!(
            tampered, text,
            "tamper anchor must exist in the plan artifact"
        );
        bytes = tampered.into_bytes();
        std::fs::write(&plan_file, bytes).unwrap();
        assert!(run(&args(&["check", &o]))
            .unwrap_err()
            .contains("CHECK FAILED"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
