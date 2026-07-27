use serde::Serialize;

use crate::app::dirs_home;
use crate::cli::DoctorArgs;
use crate::commands::context::AppContext;
use crate::domain::drift::ManifestHealth;
use crate::domain::routing::RoutingAudit;
use crate::infra::cache::MultiSourceCache;
use crate::infra::generations::snapshot_nix_disk_usage;
use crate::infra::nix_runtime::{
    DeterminateFreshness, InstalledNix, NixDistribution, detect_installed_nix,
    determinate_version_status,
};
use crate::infra::shell::{command_path, first_nonempty_output, run_captured_command};
use crate::output::printer::Printer;

#[derive(Debug, Clone, Serialize)]
struct DoctorCheck {
    name: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct ToolCheck {
    name: String,
    required: bool,
    available: bool,
    path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SubstrateStatus {
    Healthy,
    Informational,
    Warning,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
struct SubstrateCheck {
    name: String,
    status: SubstrateStatus,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct DoctorReport {
    repo_root: String,
    repo_root_source: String,
    manifest_status: String,
    manifest_detail: Option<String>,
    routing_ok: bool,
    routing_issue_count: usize,
    checks: Vec<DoctorCheck>,
    substrate: Vec<SubstrateCheck>,
    tools: Vec<ToolCheck>,
    cache_path: String,
    cache_available: bool,
}

pub fn cmd_doctor(args: &DoctorArgs, ctx: &AppContext) -> i32 {
    let report = build_report(ctx);

    if args.json {
        return render_json(&report);
    }

    render_plain(&report, args.verbose, ctx);
    i32::from(!doctor_ok(&report))
}

fn build_report(ctx: &AppContext) -> DoctorReport {
    let routing = RoutingAudit::scan(&ctx.repo_root);
    let cache_path = dirs_home()
        .join(".cache")
        .join("nx")
        .join("packages_v4.json");
    let cache_available = MultiSourceCache::load(&ctx.repo_root).is_ok();

    let mut checks = vec![
        DoctorCheck {
            name: "flake.lock".to_string(),
            ok: ctx.repo_root.join("flake.lock").is_file(),
            detail: if ctx.repo_root.join("flake.lock").is_file() {
                "present".to_string()
            } else {
                "missing".to_string()
            },
        },
        DoctorCheck {
            name: "routing metadata".to_string(),
            ok: routing.is_clean(),
            detail: if routing.is_clean() {
                "passed".to_string()
            } else {
                format!("{} issue(s)", routing.issues().len())
            },
        },
        DoctorCheck {
            name: "manifest".to_string(),
            ok: !matches!(ctx.manifest_health, ManifestHealth::Invalid { .. }),
            detail: manifest_status_text(&ctx.manifest_health).to_string(),
        },
        DoctorCheck {
            name: "package cache".to_string(),
            ok: cache_available,
            detail: cache_path.display().to_string(),
        },
    ];
    let installed_nix = detect_installed_nix().ok().flatten();
    checks.extend(local_nix_checks(installed_nix.as_ref()));
    let substrate = substrate_checks(installed_nix.as_ref());

    let tools = [
        ("git", true),
        ("nix", true),
        ("just", false),
        ("sops", false),
        ("brew", false),
        ("home-manager", false),
        ("darwin-rebuild", false),
    ]
    .into_iter()
    .map(|(name, required)| {
        let path = command_path(name);
        ToolCheck {
            name: name.to_string(),
            required,
            available: path.is_some(),
            path,
        }
    })
    .collect();

    DoctorReport {
        repo_root: ctx.repo_root.display().to_string(),
        repo_root_source: if std::env::var_os("NX_REPO_ROOT").is_some() {
            "NX_REPO_ROOT".to_string()
        } else {
            "auto-discovery".to_string()
        },
        manifest_status: manifest_status_text(&ctx.manifest_health).to_string(),
        manifest_detail: manifest_detail(&ctx.manifest_health),
        routing_ok: routing.is_clean(),
        routing_issue_count: routing.issues().len(),
        checks,
        substrate,
        tools,
        cache_path: cache_path.display().to_string(),
        cache_available,
    }
}

fn local_nix_checks(installed: Option<&InstalledNix>) -> Vec<DoctorCheck> {
    let store = run_captured_command(
        "nix",
        &[
            "store",
            "info",
            "--store",
            "daemon",
            "--json",
            "--no-pretty",
        ],
        None,
    );
    let config = run_captured_command("nix", &["config", "check"], None);

    vec![
        DoctorCheck {
            name: "Nix runtime".to_string(),
            ok: installed.is_some(),
            detail: installed.map_or_else(
                || "unavailable".to_string(),
                |nix| format!("{} {}", nix.distribution, nix.version),
            ),
        },
        daemon_check(store),
        command_check("Nix configuration", config),
    ]
}

fn daemon_check(output: anyhow::Result<crate::infra::shell::CapturedCommand>) -> DoctorCheck {
    let (ok, detail) = match output {
        Ok(output) if output.code == 0 => match parse_daemon_info(&output.stdout) {
            Some(version) => (true, format!("version {version}")),
            None => (false, "invalid daemon response".to_string()),
        },
        Ok(output) => (false, format!("failed: {}", first_nonempty_output(&output))),
        Err(err) => (false, format!("unavailable: {err:#}")),
    };
    DoctorCheck {
        name: "Nix daemon".to_string(),
        ok,
        detail,
    }
}

fn parse_daemon_info(output: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(output).ok()?;
    (value.get("url")?.as_str()? == "daemon")
        .then(|| value.get("version")?.as_str().map(str::to_string))
        .flatten()
}

fn command_check(
    name: &str,
    output: anyhow::Result<crate::infra::shell::CapturedCommand>,
) -> DoctorCheck {
    match output {
        Ok(output) => DoctorCheck {
            name: name.to_string(),
            ok: output.code == 0,
            detail: if output.code == 0 {
                "passed".to_string()
            } else {
                format!("failed: {}", first_nonempty_output(&output))
            },
        },
        Err(err) => DoctorCheck {
            name: name.to_string(),
            ok: false,
            detail: format!("unavailable: {err:#}"),
        },
    }
}

fn substrate_checks(installed: Option<&InstalledNix>) -> Vec<SubstrateCheck> {
    let is_determinate =
        installed.is_some_and(|nix| nix.distribution == NixDistribution::Determinate);
    let determinate = is_determinate.then(determinate_check);

    vec![
        determinate
            .unwrap_or_else(|| SubstrateCheck::unavailable("Determinate Nix", "not installed")),
        lazy_trees_check(),
        if is_determinate {
            determinate_gc_check()
        } else {
            SubstrateCheck::unavailable("Determinate GC", "not installed")
        },
        nix_disk_check(),
    ]
}

fn determinate_check() -> SubstrateCheck {
    match determinate_version_status() {
        Ok(Some(status)) => match status.freshness {
            DeterminateFreshness::Current => SubstrateCheck::healthy(
                "Determinate Nix",
                format!(
                    "daemon {} / client {} / current",
                    status.daemon, status.client
                ),
            ),
            DeterminateFreshness::UpdateAvailable(latest) => SubstrateCheck::warning(
                "Determinate Nix",
                format!(
                    "daemon {} / client {} / latest {}; run `sudo determinate-nixd upgrade`",
                    status.daemon, status.client, latest
                ),
            ),
            DeterminateFreshness::DaemonClientMismatch => SubstrateCheck::warning(
                "Determinate Nix",
                format!(
                    "daemon {} / client {} mismatch",
                    status.daemon, status.client
                ),
            ),
            DeterminateFreshness::Unknown => SubstrateCheck::unavailable(
                "Determinate Nix",
                format!(
                    "daemon {} / client {}; freshness unknown",
                    status.daemon, status.client
                ),
            ),
        },
        Ok(None) => SubstrateCheck::unavailable("Determinate Nix", "version output unavailable"),
        Err(err) => SubstrateCheck::unavailable("Determinate Nix", format!("{err:#}")),
    }
}

fn lazy_trees_check() -> SubstrateCheck {
    match run_captured_command("nix", &["config", "show", "lazy-trees"], None) {
        Ok(output) if output.code == 0 && output.stdout.trim() == "true" => {
            classify_lazy_trees(true)
        }
        Ok(output) if output.code == 0 && output.stdout.trim() == "false" => {
            classify_lazy_trees(false)
        }
        Ok(output) => {
            SubstrateCheck::unavailable("lazy-trees", first_nonempty_output(&output).to_string())
        }
        Err(err) => SubstrateCheck::unavailable("lazy-trees", format!("{err:#}")),
    }
}

fn classify_lazy_trees(enabled: bool) -> SubstrateCheck {
    if enabled {
        SubstrateCheck::healthy("lazy-trees", "enabled")
    } else {
        SubstrateCheck::informational("lazy-trees", "disabled")
    }
}

fn determinate_gc_check() -> SubstrateCheck {
    let path = std::path::Path::new("/etc/determinate/config.json");
    let strategy = match std::fs::read_to_string(path) {
        Ok(text) => match determinate_gc_strategy(&text) {
            Ok(strategy) => strategy,
            Err(err) => {
                return SubstrateCheck::unavailable("Determinate GC", err);
            }
        },
        Err(err) => {
            return SubstrateCheck::unavailable(
                "Determinate GC",
                format!("{}: {err}", path.display()),
            );
        }
    };

    classify_gc_strategy(&strategy)
}

fn classify_gc_strategy(strategy: &str) -> SubstrateCheck {
    if strategy == "automatic" {
        SubstrateCheck::healthy("Determinate GC", "automatic")
    } else {
        SubstrateCheck::informational("Determinate GC", strategy)
    }
}

fn determinate_gc_strategy(text: &str) -> Result<String, String> {
    let config = serde_json::from_str::<serde_json::Value>(text)
        .map_err(|err| format!("invalid config: {err}"))?;
    Ok(config
        .pointer("/garbageCollector/strategy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("automatic")
        .to_string())
}

fn nix_disk_check() -> SubstrateCheck {
    match snapshot_nix_disk_usage() {
        Ok(disk) => classify_nix_disk(&disk),
        Err(err) => SubstrateCheck::unavailable("/nix disk", format!("{err:#}")),
    }
}

fn classify_nix_disk(disk: &crate::domain::generations::DiskUsageSnapshot) -> SubstrateCheck {
    const TARGET_FREE_BYTES: u64 = 30 * 1024 * 1024 * 1024;

    let detail = format!(
        "{} available on {} ({} used)",
        disk.available, disk.mounted_on, disk.capacity
    );
    let urgent = disk
        .capacity
        .trim_end_matches('%')
        .parse::<u8>()
        .is_ok_and(|used| used >= 95);
    if urgent {
        SubstrateCheck::warning("/nix disk", format!("{detail}; at least 95% used"))
    } else if disk.available_bytes >= TARGET_FREE_BYTES {
        SubstrateCheck::healthy("/nix disk", detail)
    } else {
        SubstrateCheck::informational("/nix disk", detail)
    }
}

impl SubstrateCheck {
    fn healthy(name: &str, detail: impl Into<String>) -> Self {
        Self::new(name, SubstrateStatus::Healthy, detail)
    }

    fn informational(name: &str, detail: impl Into<String>) -> Self {
        Self::new(name, SubstrateStatus::Informational, detail)
    }

    fn warning(name: &str, detail: impl Into<String>) -> Self {
        Self::new(name, SubstrateStatus::Warning, detail)
    }

    fn unavailable(name: &str, detail: impl Into<String>) -> Self {
        Self::new(name, SubstrateStatus::Unavailable, detail)
    }

    fn new(name: &str, status: SubstrateStatus, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            status,
            detail: detail.into(),
        }
    }
}

fn manifest_status_text(health: &ManifestHealth) -> &'static str {
    match health {
        ManifestHealth::Missing => "missing",
        ManifestHealth::Invalid { .. } => "invalid",
        ManifestHealth::InSync { .. } => "in_sync",
        ManifestHealth::Drifted { .. } => "drifted",
    }
}

fn manifest_detail(health: &ManifestHealth) -> Option<String> {
    match health {
        ManifestHealth::Invalid { error } => Some(error.clone()),
        ManifestHealth::Drifted { report, .. } => Some(format!("{} issue(s)", report.issues.len())),
        _ => None,
    }
}

fn doctor_ok(report: &DoctorReport) -> bool {
    report.checks.iter().all(|check| check.ok)
        && report
            .tools
            .iter()
            .filter(|tool| tool.required)
            .all(|tool| tool.available)
}

fn render_json(report: &DoctorReport) -> i32 {
    match serde_json::to_string_pretty(report) {
        Ok(text) => {
            println!("{text}");
            i32::from(!doctor_ok(report))
        }
        Err(err) => {
            eprintln!("doctor json rendering failed: {err}");
            1
        }
    }
}

fn render_plain(report: &DoctorReport, verbose: bool, ctx: &AppContext) {
    ctx.printer.action("Diagnosing nx environment");

    Printer::heading("Doctor Report");
    Printer::body(&format!("Repo root: {}", report.repo_root));
    Printer::detail(&format!("Repo root source: {}", report.repo_root_source));
    Printer::detail(&format!("Manifest: {}", report.manifest_status));
    if let Some(detail) = &report.manifest_detail {
        Printer::detail(&format!("Manifest detail: {detail}"));
    }
    Printer::detail(&format!(
        "Routing metadata: {}",
        if report.routing_ok {
            "passed".to_string()
        } else {
            format!("{} issue(s)", report.routing_issue_count)
        }
    ));
    Printer::detail(&format!(
        "Package cache: {}",
        if report.cache_available {
            &report.cache_path
        } else {
            "unavailable"
        }
    ));

    Printer::heading("Checks");
    for check in &report.checks {
        let line = format!("{}: {}", check.name, check.detail);
        if check.ok {
            ctx.printer.success(&line);
        } else {
            ctx.printer.error(&line);
        }
    }

    Printer::heading("Nix Substrate");
    for check in &report.substrate {
        let line = format!("{}: {}", check.name, check.detail);
        match check.status {
            SubstrateStatus::Healthy => ctx.printer.success(&line),
            SubstrateStatus::Informational => Printer::detail(&line),
            SubstrateStatus::Warning => ctx.printer.warn(&line),
            SubstrateStatus::Unavailable => Printer::detail(&format!("unavailable: {line}")),
        }
    }

    Printer::heading("Tools");
    for tool in &report.tools {
        let line = if verbose {
            let path = tool.path.as_deref().unwrap_or("not found");
            format!(
                "{} ({}) - {}",
                tool.name,
                if tool.required {
                    "required"
                } else {
                    "optional"
                },
                path
            )
        } else {
            format!(
                "{} ({})",
                tool.name,
                if tool.required {
                    "required"
                } else {
                    "optional"
                }
            )
        };

        if tool.available {
            ctx.printer.success(&line);
        } else if tool.required {
            ctx.printer.error(&line);
        } else {
            ctx.printer.warn(&line);
        }
    }

    if !report.routing_ok {
        println!();
        Printer::detail("Run `nx lint` for the full routing issue list.");
    }
    if matches!(
        ctx.manifest_health,
        ManifestHealth::Missing | ManifestHealth::Invalid { .. }
    ) {
        Printer::detail("Run `nx init --refresh` to recover manifest-based routing.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::generations::DiskUsageSnapshot;

    #[test]
    fn determinate_gc_defaults_only_for_valid_config() {
        assert_eq!(determinate_gc_strategy("{}").as_deref(), Ok("automatic"));
        assert_eq!(
            determinate_gc_strategy(r#"{"garbageCollector":{"strategy":"disabled"}}"#).as_deref(),
            Ok("disabled")
        );
        assert!(determinate_gc_strategy("{").is_err());
    }

    #[test]
    fn daemon_info_requires_structured_daemon_response() {
        assert_eq!(
            parse_daemon_info(r#"{"url":"daemon","version":"2.34.8","trusted":true}"#),
            Some("2.34.8".to_string())
        );
        assert_eq!(
            parse_daemon_info(r#"{"url":"local","version":"2.34.8"}"#),
            None
        );
        assert_eq!(parse_daemon_info(""), None);
    }

    #[test]
    fn disk_predicates_use_exact_available_bytes() {
        let disk = |available_bytes: u64, capacity: &str| DiskUsageSnapshot {
            filesystem: "/dev/test".to_string(),
            size: "100Gi".to_string(),
            used: "75Gi".to_string(),
            available: "25Gi".to_string(),
            capacity: capacity.to_string(),
            mounted_on: "/nix".to_string(),
            available_bytes,
        };
        assert_eq!(
            classify_nix_disk(&disk(30 * 1024 * 1024 * 1024, "70%")).status,
            SubstrateStatus::Healthy
        );
        assert_eq!(
            classify_nix_disk(&disk(25 * 1024 * 1024 * 1024, "75%")).status,
            SubstrateStatus::Informational
        );
        assert_eq!(
            classify_nix_disk(&disk(40 * 1024 * 1024 * 1024, "94%")).status,
            SubstrateStatus::Healthy
        );
        assert_eq!(
            classify_nix_disk(&disk(40 * 1024 * 1024 * 1024, "95%")).status,
            SubstrateStatus::Warning
        );
        let urgent = classify_nix_disk(&disk(512 * 1024, "99%"));
        assert_eq!(urgent.status, SubstrateStatus::Warning);
        assert!(urgent.detail.contains("at least 95% used"));
    }

    #[test]
    fn optional_substrate_settings_are_informational() {
        assert_eq!(
            classify_lazy_trees(false).status,
            SubstrateStatus::Informational
        );
        assert_eq!(
            classify_gc_strategy("disabled").status,
            SubstrateStatus::Informational
        );
        assert_eq!(classify_lazy_trees(true).status, SubstrateStatus::Healthy);
        assert_eq!(
            classify_gc_strategy("automatic").status,
            SubstrateStatus::Healthy
        );
    }

    #[test]
    fn every_substrate_status_is_exit_neutral_and_serialized() {
        for status in [
            SubstrateStatus::Healthy,
            SubstrateStatus::Informational,
            SubstrateStatus::Warning,
            SubstrateStatus::Unavailable,
        ] {
            let report = DoctorReport {
                repo_root: String::new(),
                repo_root_source: String::new(),
                manifest_status: String::new(),
                manifest_detail: None,
                routing_ok: true,
                routing_issue_count: 0,
                checks: vec![DoctorCheck {
                    name: String::new(),
                    ok: true,
                    detail: String::new(),
                }],
                substrate: vec![SubstrateCheck::new("test", status, "detail")],
                tools: Vec::new(),
                cache_path: String::new(),
                cache_available: true,
            };
            assert!(doctor_ok(&report));
            assert!(serde_json::to_string(&report).unwrap().contains(&format!(
                r#""status":"{}""#,
                serde_json::to_value(status).unwrap().as_str().unwrap()
            )));
        }
    }
}
