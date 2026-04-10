use serde::Serialize;

use crate::app::dirs_home;
use crate::cli::DoctorArgs;
use crate::commands::context::AppContext;
use crate::domain::drift::ManifestHealth;
use crate::domain::routing::RoutingAudit;
use crate::infra::cache::MultiSourceCache;
use crate::infra::shell::command_path;
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

#[derive(Debug, Clone, Serialize)]
struct DoctorReport {
    repo_root: String,
    repo_root_source: String,
    manifest_status: String,
    manifest_detail: Option<String>,
    routing_ok: bool,
    routing_issue_count: usize,
    checks: Vec<DoctorCheck>,
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

    let checks = vec![
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
    .map(|(name, required)| ToolCheck {
        name: name.to_string(),
        required,
        path: command_path(name),
        available: command_path(name).is_some(),
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
        tools,
        cache_path: cache_path.display().to_string(),
        cache_available,
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
    println!();

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

    println!();
    Printer::heading("Checks");
    for check in &report.checks {
        let line = format!("{}: {}", check.name, check.detail);
        if check.ok {
            ctx.printer.success(&line);
        } else {
            ctx.printer.error(&line);
        }
    }

    println!();
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
