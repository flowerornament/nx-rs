use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::cli::{
    Cli, CommandKind, InfoArgs, InstallArgs, InstalledArgs, ListArgs, PassthroughArgs, RemoveArgs,
    WhereArgs,
};
use crate::nix_scan::{PackageBuckets, find_package, find_package_fuzzy, scan_packages};
use crate::output::printer::Printer;
use crate::output::style::OutputStyle;

const VALID_SOURCES_TEXT: &str =
    "  Valid sources: brew, brews, cask, casks, homebrew, mas, nix, nxs, service,\n  services";
const DARWIN_REBUILD: &str = "/run/current-system/sw/bin/darwin-rebuild";

pub fn execute(cli: Cli) -> i32 {
    let style = OutputStyle::from_flags(cli.plain, cli.unicode, cli.minimal);
    let printer = Printer::new(style);

    let repo_root = match find_repo_root() {
        Ok(path) => path,
        Err(message) => {
            printer.error(&message);
            return 1;
        }
    };

    match cli.command {
        CommandKind::Install(args) => cmd_install(&args, &repo_root, &printer),
        CommandKind::Remove(args) => cmd_remove(&args, &repo_root, &printer),
        CommandKind::Where(args) => cmd_where(&args, &repo_root),
        CommandKind::List(args) => cmd_list(&args, &repo_root),
        CommandKind::Info(args) => cmd_info(&args, &repo_root),
        CommandKind::Status => cmd_status(&repo_root),
        CommandKind::Installed(args) => cmd_installed(&args, &repo_root),
        CommandKind::Undo => 0,
        CommandKind::Update(args) => cmd_update(&args, &repo_root, &printer),
        CommandKind::Test => cmd_test(&repo_root, &printer),
        CommandKind::Rebuild(args) => cmd_rebuild(&args, &repo_root, &printer),
        CommandKind::Upgrade(_args) => 0,
    }
}

fn find_repo_root() -> Result<PathBuf, String> {
    if let Some(env_root) = env::var_os("B2NIX_REPO_ROOT") {
        let env_path = PathBuf::from(env_root);
        return Ok(fs::canonicalize(&env_path).unwrap_or(env_path));
    }

    let git_output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|err| format!("git root detection failed: {err}"))?;

    if git_output.status.success() {
        let root = String::from_utf8_lossy(&git_output.stdout)
            .trim()
            .to_string();
        if !root.is_empty() {
            let candidate = PathBuf::from(&root);
            if candidate.join("flake.nix").exists() {
                return Ok(candidate);
            }
        }
    }

    let fallback = dirs_home().join(".nix-config");
    if fallback.exists() {
        return Ok(fallback);
    }

    Err("Could not find nix-config repository".to_string())
}

fn dirs_home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

struct CapturedCommand {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_captured_command(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
) -> Result<CapturedCommand, String> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let output = command
        .output()
        .map_err(|err| format!("command execution failed ({program}): {err}"))?;

    Ok(CapturedCommand {
        code: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn run_indented_command(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    printer: &Printer,
    indent: &str,
) -> Result<i32, String> {
    let output = run_captured_command(program, args, cwd)?;
    let mut merged = output.stdout;
    merged.push_str(&output.stderr);

    for raw_line in merged.replace("\r\n", "\n").lines() {
        let trimmed = raw_line.trim_end();
        if trimmed.is_empty() {
            println!();
            continue;
        }
        printer.stream_line(trimmed, indent, 80);
    }

    Ok(output.code)
}

fn cmd_install(args: &InstallArgs, repo_root: &Path, printer: &Printer) -> i32 {
    if args.packages.is_empty() {
        printer.error("No packages specified");
        return 1;
    }

    if args.dry_run {
        printer.dry_run_banner();
    }

    printer.action(&format!("Installing {}", args.packages[0]));

    for package in &args.packages {
        match find_package(package, repo_root) {
            Ok(Some(location)) => {
                println!();
                printer.success(&format!(
                    "{package} already installed ({})",
                    relative_location(&location, repo_root)
                ));
            }
            Ok(None) => {
                printer.error(&format!("{package} not found"));
                return 1;
            }
            Err(err) => {
                printer.error(&format!("install lookup failed: {err}"));
                return 1;
            }
        }
    }

    0
}

fn cmd_remove(args: &RemoveArgs, repo_root: &Path, printer: &Printer) -> i32 {
    if args.packages.is_empty() {
        printer.error("No packages specified");
        return 1;
    }

    if !args.dry_run {
        printer.error("remove is not implemented yet");
        return 1;
    }

    for package in &args.packages {
        match find_package(package, repo_root) {
            Ok(Some(location)) => {
                printer.dry_run_banner();
                printer.action(&format!("Removing {package}"));
                printer.detail(&format!(
                    "Location: {}",
                    relative_location(&location, repo_root)
                ));
                let (file_path, line_num) = location_path_and_line(&location);
                if let Some(line_num) = line_num {
                    show_snippet(file_path, line_num, 1, SnippetMode::Remove, true);
                }
                println!("\n- Would remove {package}");
            }
            Ok(None) => {
                printer.error(&format!("{package} not found"));
            }
            Err(err) => {
                printer.error(&format!("remove lookup failed: {err}"));
                return 1;
            }
        }
    }

    0
}

fn cmd_where(args: &WhereArgs, repo_root: &Path) -> i32 {
    let Some(package) = &args.package else {
        eprintln!("x No package specified");
        return 1;
    };

    match find_package(package, repo_root) {
        Ok(Some(location)) => {
            println!("+ {package} at {}", relative_location(&location, repo_root));
            let (file_path, line_num) = location_path_and_line(&location);
            if let Some(line_num) = line_num {
                show_snippet(file_path, line_num, 2, SnippetMode::Add, false);
            }
        }
        Ok(None) => {
            eprintln!("x {package} not found");
            println!("\n  Try: nx info {package}");
        }
        Err(err) => {
            eprintln!("x where lookup failed: {err}");
            return 1;
        }
    }

    0
}

fn cmd_list(args: &ListArgs, repo_root: &Path) -> i32 {
    let buckets = match scan_packages(repo_root) {
        Ok(buckets) => buckets,
        Err(err) => {
            eprintln!("x package scan failed: {err}");
            return 1;
        }
    };

    let filter = args.source.as_deref().map(str::to_string);
    let source = match filter.as_deref() {
        Some(raw) => match normalize_source_filter(raw) {
            Some(valid) => Some(valid),
            None => {
                eprintln!("x Unknown source: {raw}");
                println!("{VALID_SOURCES_TEXT}");
                return 1;
            }
        },
        None => None,
    };

    if args.json {
        let output = if let Some(source_key) = source {
            let mut map = Map::new();
            map.insert(
                source_key.to_string(),
                Value::Array(
                    source_values(source_key, &buckets)
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
            serde_json::to_string_pretty(&map)
        } else {
            let json = ListJsonOutput::from(&buckets);
            serde_json::to_string_pretty(&json)
        };

        match output {
            Ok(text) => {
                println!("{text}");
                return 0;
            }
            Err(err) => {
                eprintln!("x list json rendering failed: {err}");
                return 1;
            }
        }
    }

    if args.plain {
        if let Some(source_key) = source {
            let mut only = source_values(source_key, &buckets).to_vec();
            only.sort();
            for package in &only {
                println!("  {package}");
            }
            return 0;
        }

        print_plain_list(&buckets);
        return 0;
    }

    print_plain_list(&buckets);
    0
}

fn cmd_info(args: &InfoArgs, repo_root: &Path) -> i32 {
    let Some(package) = &args.package else {
        eprintln!("x No package specified");
        println!("  Usage: nx info <package>");
        return 1;
    };

    let location = match find_package(package, repo_root) {
        Ok(location) => location,
        Err(err) => {
            eprintln!("x info lookup failed: {err}");
            return 1;
        }
    };

    if args.json {
        let output = InfoJsonOutput {
            name: package.clone(),
            installed: location.is_some(),
            location,
            sources: Vec::new(),
            hm_module: None,
            darwin_service: None,
            flakehub: Vec::new(),
        };
        match serde_json::to_string_pretty(&output) {
            Ok(text) => {
                println!("{text}");
                return 0;
            }
            Err(err) => {
                eprintln!("x info json rendering failed: {err}");
                return 1;
            }
        }
    }

    let status = if location.is_some() {
        "installed"
    } else {
        "not installed"
    };
    println!("\n  {package} ({status})");
    if let Some(location) = location {
        println!("  Location: {}", relative_location(&location, repo_root));
        let (file_path, line_num) = location_path_and_line(&location);
        if let Some(line_num) = line_num {
            show_snippet(file_path, line_num, 1, SnippetMode::Add, false);
        }
    } else {
        eprintln!("x {package} not found");
        println!("\n  Try: nx {package}");
    }
    0
}

fn cmd_status(repo_root: &Path) -> i32 {
    let buckets = match scan_packages(repo_root) {
        Ok(buckets) => buckets,
        Err(err) => {
            eprintln!("x package scan failed: {err}");
            return 1;
        }
    };

    let total = [
        buckets.nxs.len(),
        buckets.brews.len(),
        buckets.casks.len(),
        buckets.mas.len(),
        buckets.services.len(),
    ]
    .into_iter()
    .sum::<usize>();

    println!("\n  Package Status ({total} packages installed)");
    println!("\n  Source       Count  Examples");

    for (label, packages) in [
        ("nxs", &buckets.nxs),
        ("homebrew", &buckets.brews),
        ("casks", &buckets.casks),
        ("Mac App Store", &buckets.mas),
        ("services", &buckets.services),
    ] {
        if packages.is_empty() {
            continue;
        }
        let examples = render_examples(packages);
        println!("  {label:<12} {:>5}  {examples}", packages.len());
    }

    0
}

fn cmd_installed(args: &InstalledArgs, repo_root: &Path) -> i32 {
    if args.packages.is_empty() {
        eprintln!("x No package specified");
        return 1;
    }

    let mut all_installed = true;
    let mut rendered = Vec::new();
    for query in &args.packages {
        match find_package_fuzzy(query, repo_root) {
            Ok(Some(found)) => {
                rendered.push(format!(
                    "{}: {{\"match\": {}, \"location\": {}}}",
                    json_string(query),
                    json_string(&found.name),
                    json_string(&found.location),
                ));
            }
            Ok(None) => {
                all_installed = false;
                rendered.push(format!(
                    "{}: {{\"match\": null, \"location\": null}}",
                    json_string(query)
                ));
            }
            Err(err) => {
                eprintln!("x installed lookup failed: {err}");
                return 1;
            }
        }
    }

    if args.json {
        println!("{{{}}}", rendered.join(", "));
        return if all_installed { 0 } else { 1 };
    }

    if all_installed { 0 } else { 1 }
}

fn cmd_update(args: &PassthroughArgs, repo_root: &Path, printer: &Printer) -> i32 {
    printer.action("Updating flake inputs");

    let mut command_args = vec!["flake".to_string(), "update".to_string()];
    command_args.extend(args.passthrough.iter().cloned());
    let return_code =
        match run_indented_command("nix", &command_args, Some(repo_root), printer, "  ") {
            Ok(code) => code,
            Err(err) => {
                printer.error(&err);
                return 1;
            }
        };

    if return_code == 0 {
        println!();
        printer.success("Flake inputs updated");
        printer.detail("Run 'nx rebuild' to rebuild, or 'nx upgrade' for full upgrade");
        return 0;
    }

    printer.error("Flake update failed");
    1
}

fn cmd_test(repo_root: &Path, printer: &Printer) -> i32 {
    let steps: [(&str, Vec<String>, Option<PathBuf>); 3] = [
        (
            "ruff",
            vec!["check".to_string(), ".".to_string()],
            Some(repo_root.join("scripts/nx")),
        ),
        (
            "mypy",
            vec![".".to_string()],
            Some(repo_root.join("scripts/nx")),
        ),
        (
            "tests",
            vec![
                "-m".to_string(),
                "unittest".to_string(),
                "discover".to_string(),
                "-s".to_string(),
                "scripts/nx/tests".to_string(),
            ],
            Some(repo_root.to_path_buf()),
        ),
    ];

    for (label, args, cwd) in steps {
        printer.action(&format!("Running {label}"));
        println!();
        let program = if label == "tests" { "python3" } else { label };
        let return_code = match run_indented_command(program, &args, cwd.as_deref(), printer, "  ")
        {
            Ok(code) => code,
            Err(err) => {
                printer.error(&format!("{label} failed"));
                printer.error(&err);
                return 1;
            }
        };

        if return_code != 0 {
            printer.error(&format!("{label} failed"));
            return 1;
        }

        println!();
        printer.success(&format!("{label} passed"));
    }

    0
}

fn cmd_rebuild(args: &PassthroughArgs, repo_root: &Path, printer: &Printer) -> i32 {
    printer.action("Checking tracked nix files");
    let preflight_args = vec![
        "-C".to_string(),
        repo_root.display().to_string(),
        "ls-files".to_string(),
        "--others".to_string(),
        "--exclude-standard".to_string(),
        "--".to_string(),
        "home".to_string(),
        "packages".to_string(),
        "system".to_string(),
        "hosts".to_string(),
    ];
    let output = match run_captured_command("git", &preflight_args, None) {
        Ok(output) => output,
        Err(_) => {
            printer.error("Git preflight failed");
            return 1;
        }
    };

    if output.code != 0 {
        printer.error("Git preflight failed");
        let stderr = output.stderr.trim().to_string();
        if !stderr.is_empty() {
            printer.detail(&stderr);
        } else {
            let stdout = output.stdout.trim().to_string();
            if !stdout.is_empty() {
                printer.detail(&stdout);
            }
        }
        return 1;
    }

    let mut untracked: Vec<String> = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| line.ends_with(".nix"))
        .map(ToOwned::to_owned)
        .collect();
    untracked.sort();

    if untracked.is_empty() {
        printer.success("Git preflight passed");
    } else {
        printer.error("Untracked .nix files would be ignored by flake evaluation");
        println!("\n  Track these files before rebuild:");
        for rel_path in &untracked {
            println!("  - {rel_path}");
        }
        println!("\n  Run: git -C \"{}\" add <files>", repo_root.display());
        return 1;
    }

    printer.action("Checking flake");
    let flake_args = vec![
        "flake".to_string(),
        "check".to_string(),
        repo_root.display().to_string(),
    ];
    let flake_output = match run_captured_command("nix", &flake_args, None) {
        Ok(output) => output,
        Err(err) => {
            printer.error("Flake check failed");
            println!("{err}");
            return 1;
        }
    };
    if flake_output.code != 0 {
        printer.error("Flake check failed");
        let err_text = if flake_output.stderr.trim().is_empty() {
            flake_output.stdout.trim()
        } else {
            flake_output.stderr.trim()
        };
        if !err_text.is_empty() {
            println!("{err_text}");
        }
        return 1;
    }
    printer.success("Flake check passed");

    printer.action("Rebuilding system");
    println!();
    let mut rebuild_args = vec![
        DARWIN_REBUILD.to_string(),
        "switch".to_string(),
        "--flake".to_string(),
        repo_root.display().to_string(),
    ];
    rebuild_args.extend(args.passthrough.iter().cloned());

    let return_code = match run_indented_command("sudo", &rebuild_args, None, printer, "  ") {
        Ok(code) => code,
        Err(err) => {
            printer.error("Rebuild failed");
            printer.error(&err);
            return 1;
        }
    };
    if return_code == 0 {
        println!();
        printer.success("System rebuilt");
        return 0;
    }

    printer.error("Rebuild failed");
    1
}

fn relative_location(location: &str, repo_root: &Path) -> String {
    let (path_part, suffix) = split_location(location);
    let raw_root = repo_root.display().to_string();
    let canonical_root = fs::canonicalize(repo_root)
        .ok()
        .map(|path| path.display().to_string());

    let mut rel = path_part.to_string();
    if let Some(root) = canonical_root {
        let prefix = format!("{root}/");
        rel = rel.strip_prefix(&prefix).unwrap_or(&rel).to_string();
    }
    let raw_prefix = format!("{raw_root}/");
    rel = rel.strip_prefix(&raw_prefix).unwrap_or(&rel).to_string();

    format!("{rel}{suffix}")
}

fn location_path_and_line(location: &str) -> (&str, Option<usize>) {
    match location.rsplit_once(':') {
        Some((path, line)) if line.chars().all(|ch| ch.is_ascii_digit()) => {
            (path, line.parse::<usize>().ok())
        }
        _ => (location, None),
    }
}

fn split_location(location: &str) -> (&str, &str) {
    match location.rsplit_once(':') {
        Some((path, line)) if line.chars().all(|ch| ch.is_ascii_digit()) => {
            (path, &location[path.len()..])
        }
        _ => (location, ""),
    }
}

#[derive(Clone, Copy)]
enum SnippetMode {
    Add,
    Remove,
}

fn show_snippet(
    file_path: &str,
    line_num: usize,
    context: usize,
    mode: SnippetMode,
    preview: bool,
) {
    if line_num == 0 {
        return;
    }

    let Ok(content) = fs::read_to_string(file_path) else {
        return;
    };

    let lines: Vec<&str> = content.split('\n').collect();
    let start = line_num.saturating_sub(context + 1);
    let end = usize::min(lines.len(), line_num + context);
    if start >= end {
        return;
    }

    let path = Path::new(file_path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(file_path);
    let header_suffix = if preview { " (preview)" } else { "" };

    println!();
    println!("  ┌── {file_name}{header_suffix} ───");
    for (offset, line) in lines[start..end].iter().enumerate() {
        let number = start + offset + 1;
        match mode {
            SnippetMode::Add => {
                let marker = if number == line_num { '+' } else { ' ' };
                println!("  │ {marker} {number:4} │ {line}");
            }
            SnippetMode::Remove => {
                if number == line_num {
                    println!("  │ - {number:4} │ {line}");
                } else {
                    println!("  │   {number:4} │ {line}");
                }
            }
        }
    }
    println!("  └{}", "─".repeat(40));
}

fn normalize_source_filter(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "nix" | "nxs" => Some("nxs"),
        "brew" | "brews" | "homebrew" => Some("brews"),
        "cask" | "casks" => Some("casks"),
        "mas" => Some("mas"),
        "service" | "services" => Some("services"),
        _ => None,
    }
}

fn source_values<'a>(source: &str, buckets: &'a PackageBuckets) -> &'a [String] {
    match source {
        "nxs" => &buckets.nxs,
        "brews" => &buckets.brews,
        "casks" => &buckets.casks,
        "mas" => &buckets.mas,
        "services" => &buckets.services,
        _ => &[],
    }
}

fn render_examples(packages: &[String]) -> String {
    let mut sorted = packages.to_vec();
    sorted.sort();

    let mut examples = sorted.into_iter().take(4).collect::<Vec<_>>().join(", ");
    if packages.len() > 4 {
        if !examples.is_empty() {
            examples.push_str(", ");
        }
        examples.push_str("...");
    }
    examples
}

fn print_plain_list(buckets: &PackageBuckets) {
    for source in [
        &buckets.nxs,
        &buckets.brews,
        &buckets.casks,
        &buckets.mas,
        &buckets.services,
    ] {
        let mut packages = source.clone();
        packages.sort();
        for package in &packages {
            println!("  {package}");
        }
    }
}

fn json_string(input: &str) -> String {
    serde_json::to_string(input).unwrap_or_else(|_| "\"\"".to_string())
}

#[derive(Serialize)]
struct ListJsonOutput<'a> {
    nxs: &'a [String],
    brews: &'a [String],
    casks: &'a [String],
    mas: &'a [String],
    services: &'a [String],
}

#[derive(Serialize)]
struct InfoJsonOutput {
    name: String,
    installed: bool,
    location: Option<String>,
    sources: Vec<Value>,
    hm_module: Option<Value>,
    darwin_service: Option<Value>,
    flakehub: Vec<Value>,
}

impl<'a> From<&'a PackageBuckets> for ListJsonOutput<'a> {
    fn from(value: &'a PackageBuckets) -> Self {
        Self {
            nxs: &value.nxs,
            brews: &value.brews,
            casks: &value.casks,
            mas: &value.mas,
            services: &value.services,
        }
    }
}
