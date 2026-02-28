use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::cli::{InfoArgs, InstalledArgs, ListArgs, WhereArgs};
use crate::commands::context::AppContext;
use crate::commands::shared::{
    SnippetMode, missing_argument_error, relative_location, show_snippet,
};
use crate::domain::location::PackageLocation;
use crate::domain::source::{
    OVERLAY_PACKAGES, PackageSource, SourcePreferences, SourceResult, normalize_name,
};
use crate::infra::cache::MultiSourceCache;
use crate::infra::config_scan::{PackageBuckets, collect_nix_files, scan_packages};
use crate::infra::finder::{PackageMatch, find_package, find_package_fuzzy};
use crate::infra::query_info::{
    ConfigOptionInfo, FlakeHubInfo, darwin_service_info, hm_module_info, search_flakehub,
};
use crate::infra::shell::run_json_command_quiet;
use crate::infra::sources::search_all_sources_quiet;
use crate::output::json::to_string_compact;
use crate::output::printer::Printer;

const VALID_SOURCES_TEXT: &str =
    "  Valid sources: brew, brews, cask, casks, homebrew, mas, nix, nxs, service,\n  services";

pub fn cmd_where(args: &WhereArgs, ctx: &AppContext) -> i32 {
    let Some(package) = &args.package else {
        return missing_argument_error("where", "PACKAGE");
    };

    match find_package(package, &ctx.repo_root) {
        Ok(Some(location)) => {
            ctx.printer.success(&format!(
                "{package} at {}",
                relative_location(&location, &ctx.repo_root)
            ));
            if let Some(line_num) = location.line() {
                show_snippet(location.path(), line_num, 2, SnippetMode::Add, false);
            }
        }
        Ok(None) => {
            ctx.printer.error(&format!("{package} not found"));
            println!();
            Printer::detail(&format!("Try: nx info {package}"));
        }
        Err(err) => {
            ctx.printer.error(&format!("where lookup failed: {err}"));
            return 1;
        }
    }

    0
}

pub fn cmd_list(args: &ListArgs, ctx: &AppContext) -> i32 {
    let buckets = match scan_packages(&ctx.repo_root) {
        Ok(buckets) => buckets,
        Err(err) => {
            ctx.printer.error(&format!("package scan failed: {err}"));
            return 1;
        }
    };

    let source = if let Some(raw) = args.source.as_deref() {
        let Some(valid) = normalize_source_filter(raw) else {
            ctx.printer.error(&format!("Unknown source: {raw}"));
            println!("{VALID_SOURCES_TEXT}");
            return 1;
        };
        Some(valid)
    } else {
        None
    };

    if ctx.wants_json(args.json) {
        return render_list_json(source, &buckets, &ctx.printer);
    }

    if let Some(source_key) = source {
        let mut only = source_values(source_key, &buckets).to_vec();
        only.sort();
        for package in &only {
            println!("  {package}");
        }
        return 0;
    }

    print_plain_list(&buckets);
    0
}

pub fn cmd_info(args: &InfoArgs, ctx: &AppContext) -> i32 {
    let Some(package) = &args.package else {
        return missing_argument_error("info", "PACKAGE");
    };

    let location = match find_package(package, &ctx.repo_root) {
        Ok(location) => location,
        Err(err) => {
            ctx.printer.error(&format!("info lookup failed: {err}"));
            return 1;
        }
    };

    let mut cache = MultiSourceCache::load(&ctx.repo_root).ok();
    let info_sources = collect_info_sources(package, args, &ctx.repo_root, &mut cache);

    if ctx.wants_json(args.json) {
        let sources = info_sources
            .into_iter()
            .map(info_source_json_from_result)
            .collect();

        let output = InfoJsonOutput {
            name: package.clone(),
            installed: location.is_some(),
            location: location.map(|value| value.to_string()),
            sources,
            hm_module: hm_module_info(package, &ctx.repo_root),
            darwin_service: darwin_service_info(package, &ctx.repo_root),
            flakehub: collect_info_flakehub(package, args.bleeding_edge, search_flakehub),
        };
        match serde_json::to_string_pretty(&output) {
            Ok(text) => {
                println!("{text}");
                return 0;
            }
            Err(err) => {
                ctx.printer
                    .error(&format!("info json rendering failed: {err}"));
                return 1;
            }
        }
    }

    let (installed_source, active_overlay) = location.as_ref().map_or((None, None), |found| {
        detect_installed_source(found, package, &ctx.repo_root)
    });
    let status = info_status_text(
        location.is_some(),
        installed_source.as_deref(),
        active_overlay.as_deref(),
    );
    let flakehub = collect_info_flakehub(package, args.bleeding_edge, search_flakehub);

    println!();
    Printer::detail(&format!("{package} ({status})"));
    if let Some(location) = location.as_ref() {
        Printer::detail(&format!(
            "Location: {}",
            relative_location(location, &ctx.repo_root)
        ));
        if let Some(line_num) = location.line() {
            show_snippet(location.path(), line_num, 1, SnippetMode::Add, false);
        }
    }

    if location.is_none() && info_sources.is_empty() && flakehub.is_empty() {
        ctx.printer.error(&format!("{package} not found"));
        println!();
        Printer::detail(&format!("Try: nx {package}"));
        return 0;
    }

    render_info_sources_plain(&info_sources, installed_source.as_deref());
    render_config_option_plain(
        "Home-manager module",
        "Module",
        hm_module_info(package, &ctx.repo_root),
    );
    render_config_option_plain(
        "nix-darwin service",
        "Service",
        darwin_service_info(package, &ctx.repo_root),
    );
    render_flakehub_plain(&flakehub);
    render_install_hints_plain(package, &info_sources, location.is_some());

    0
}

fn source_prefs_from_info_args(_args: &InfoArgs) -> SourcePreferences {
    SourcePreferences::default()
}

fn collect_info_sources(
    package: &str,
    args: &InfoArgs,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
) -> Vec<SourceResult> {
    collect_info_sources_with(package, args, repo_root, cache, search_all_sources_quiet)
}

fn collect_info_sources_with<F>(
    package: &str,
    args: &InfoArgs,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
    mut search: F,
) -> Vec<SourceResult>
where
    F: FnMut(&str, &SourcePreferences, Option<&Path>) -> Vec<SourceResult>,
{
    if let Some(cache_ref) = cache.as_ref() {
        let cached = cache_ref.get_all(package);
        if !cached.is_empty() {
            return cached;
        }
    }

    let prefs = source_prefs_from_info_args(args);
    let flake_lock = repo_root.join("flake.lock");
    let flake_lock_path = flake_lock.exists().then_some(flake_lock.as_path());
    let results = search(package, &prefs, flake_lock_path);

    if !results.is_empty()
        && let Some(cache_ref) = cache.as_mut()
    {
        let _ = cache_ref.set_many(&results);
    }

    results
}

fn collect_info_flakehub<F>(package: &str, include: bool, mut search: F) -> Vec<FlakeHubInfo>
where
    F: FnMut(&str) -> Vec<FlakeHubInfo>,
{
    if !include {
        return Vec::new();
    }
    search(package).into_iter().take(3).collect()
}

fn info_status_text(
    installed: bool,
    installed_source: Option<&str>,
    active_overlay: Option<&str>,
) -> String {
    if !installed {
        return "not installed".to_string();
    }
    if let Some(overlay) = active_overlay {
        return format!("installed via {overlay}");
    }
    installed_source.map_or_else(
        || "installed".to_string(),
        |source| format!("installed ({source})"),
    )
}

fn detect_installed_source(
    location: &PackageLocation,
    package: &str,
    repo_root: &Path,
) -> (Option<String>, Option<String>) {
    let rel = relative_location(location, repo_root);
    let rel_path = rel.rsplit_once(':').map_or(rel.as_str(), |(path, _)| path);

    if rel_path == "system/darwin.nix"
        && let Ok(buckets) = scan_packages(repo_root)
    {
        let package_norm = normalize_name(package);
        let matches = |value: &String| {
            value.eq_ignore_ascii_case(package) || value.eq_ignore_ascii_case(&package_norm)
        };
        if buckets.casks.iter().any(matches) {
            return (Some(PackageSource::Cask.as_str().to_string()), None);
        }
        if buckets.brews.iter().any(matches) {
            return (Some(PackageSource::Homebrew.as_str().to_string()), None);
        }
        if buckets.mas.iter().any(matches) {
            return (Some(PackageSource::Mas.as_str().to_string()), None);
        }
    }

    if let Some(overlay) = check_overlay_active(package, repo_root) {
        return (
            Some(PackageSource::FlakeInput.as_str().to_string()),
            Some(overlay),
        );
    }

    (Some(PackageSource::Nxs.as_str().to_string()), None)
}

fn check_overlay_active(name: &str, repo_root: &Path) -> Option<String> {
    let normalized = normalize_name(name);
    let (overlay_name, _, _) = OVERLAY_PACKAGES.get(normalized.as_str())?;

    let flake_lock = repo_root.join("flake.lock");
    let lock_content = fs::read_to_string(flake_lock).ok()?;
    let lock_json: Value = serde_json::from_str(&lock_content).ok()?;
    let nodes = lock_json.get("nodes").and_then(Value::as_object)?;
    if !nodes.contains_key(*overlay_name) {
        return None;
    }

    let overlay_token = format!("inputs.{overlay_name}");
    for nix_file in collect_nix_files(repo_root) {
        let Ok(content) = fs::read_to_string(nix_file) else {
            continue;
        };
        if content.contains("overlays") && content.contains(&overlay_token) {
            return Some((*overlay_name).to_string());
        }
    }

    None
}

fn render_info_sources_plain(sources: &[SourceResult], installed_source: Option<&str>) {
    for source in sources {
        let label = info_source_label(source);
        let is_current = installed_source.is_some_and(|current| current == source.source.as_str());
        let metadata = info_source_json_from_result(source.clone());

        println!();
        if is_current {
            println!("  {label} (current)");
        } else {
            println!("  {label}");
        }

        render_info_metadata_plain(&metadata);
        render_info_warnings_plain(&metadata);
        render_info_dependencies_plain(&metadata);
        render_info_artifacts_plain(&metadata);
        render_info_caveats_plain(&metadata);
    }
}

fn info_source_label(source: &SourceResult) -> String {
    let label_name = source.attr.as_deref().unwrap_or(&source.name);
    match source.source {
        PackageSource::Nxs => format!("nxs (pkgs.{label_name})"),
        PackageSource::Homebrew => "Homebrew formula".to_string(),
        PackageSource::Cask => "Homebrew cask".to_string(),
        PackageSource::Nur => format!("NUR ({label_name})"),
        PackageSource::FlakeInput => format!("Flake overlay ({label_name})"),
        PackageSource::Mas => "Mac App Store".to_string(),
    }
}

fn render_info_metadata_plain(source: &InfoSourceJson) {
    if let Some(value) = source.version.as_deref() {
        println!("  {:<13} {value}", "Version:");
    }
    if let Some(value) = source.description.as_deref() {
        println!("  {:<13} {value}", "Description:");
    }
    if let Some(value) = source.homepage.as_deref() {
        println!("  {:<13} {value}", "Homepage:");
    }
    if let Some(value) = source.license.as_deref() {
        println!("  {:<13} {value}", "License:");
    }
    if source.head_available {
        println!("  {:<13} Available (brew install --HEAD)", "HEAD build:");
    }
}

fn render_info_warnings_plain(source: &InfoSourceJson) {
    if source.broken {
        println!("! This package is marked as BROKEN");
    }
    if source.insecure {
        println!("! This package is marked as INSECURE");
    }
}

fn render_info_dependencies_plain(source: &InfoSourceJson) {
    if let Some(dependencies) = source.dependencies.as_ref()
        && !dependencies.is_empty()
    {
        let shown = dependencies
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = if dependencies.len() > 10 {
            format!(" (+{} more)", dependencies.len() - 10)
        } else {
            String::new()
        };
        println!("  {:<13} {shown}{more}", "Dependencies:");
    }

    if let Some(build_dependencies) = source.build_dependencies.as_ref()
        && !build_dependencies.is_empty()
    {
        let shown = build_dependencies
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = if build_dependencies.len() > 5 {
            format!(" (+{} more)", build_dependencies.len() - 5)
        } else {
            String::new()
        };
        println!("  {:<13} {shown}{more}", "Build deps:");
    }
}

fn render_info_artifacts_plain(source: &InfoSourceJson) {
    if let Some(artifacts) = source.artifacts.as_ref()
        && !artifacts.is_empty()
    {
        let shown = artifacts
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        println!("  {:<13} {shown}", "Installs:");
    }
}

fn render_info_caveats_plain(source: &InfoSourceJson) {
    let Some(caveats) = source.caveats.as_deref() else {
        return;
    };
    let trimmed = caveats.trim();
    if trimmed.is_empty() {
        return;
    }

    println!();
    println!("  Caveats:");
    for line in trimmed.split('\n').take(5) {
        println!("    {line}");
    }
    if caveats.matches('\n').count() > 5 {
        println!("    ...");
    }
}

fn render_config_option_plain(title: &str, kind: &str, info: Option<ConfigOptionInfo>) {
    let Some(info) = info else {
        return;
    };
    let tag = if info.enabled { " (enabled)" } else { "" };

    println!();
    println!("  {title}{tag}");
    println!("  {kind}: {}", info.path);
    println!("  Example: {}", info.example);
}

fn render_flakehub_plain(results: &[FlakeHubInfo]) {
    if results.is_empty() {
        return;
    }

    println!();
    println!("  FlakeHub flakes ({})", results.len());
    for result in results.iter().take(3) {
        println!("  {}", result.name);
        if !result.description.is_empty() {
            let summary = result.description.chars().take(60).collect::<String>();
            println!("    {summary}...");
        }
        if let Some(version) = result.version.as_deref() {
            println!("    Latest: {version}");
        }
    }

    println!();
    let first = &results[0];
    let flake_short = first.name.split('/').nth(1).unwrap_or(&first.name);
    println!("  To use a FlakeHub flake, add to flake.nix inputs:");
    println!("    {flake_short} = {{");
    println!(
        "      url = \"https://flakehub.com/f/{name}/*.tar.gz\";",
        name = first.name
    );
    println!("    }};");
    println!();
    println!("  Then use: inputs.{flake_short}.packages.${{system}}.default");
}

fn render_install_hints_plain(name: &str, infos: &[SourceResult], installed: bool) {
    if installed || infos.len() <= 1 {
        return;
    }

    println!("  Available from multiple sources. Install with:");
    for info in infos {
        let hint = install_hint_for_source(name, info.source);
        println!("  {hint}");
    }
}

fn install_hint_for_source(name: &str, source: PackageSource) -> String {
    match source {
        PackageSource::Nxs => format!("nx {name}"),
        PackageSource::Nur => format!("nx --nur {name}"),
        PackageSource::Homebrew => format!("nx --source homebrew {name}"),
        PackageSource::Cask => format!("nx --cask {name}"),
        PackageSource::Mas => format!("nx --mas {name}"),
        PackageSource::FlakeInput => format!("nx --bleeding-edge {name}"),
    }
}

pub fn cmd_status(ctx: &AppContext) -> i32 {
    let buckets = match scan_packages(&ctx.repo_root) {
        Ok(buckets) => buckets,
        Err(err) => {
            ctx.printer.error(&format!("package scan failed: {err}"));
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

pub fn cmd_installed(args: &InstalledArgs, ctx: &AppContext) -> i32 {
    if args.packages.is_empty() {
        return missing_argument_error("installed", "PACKAGES...");
    }

    let mut results = Vec::new();
    for query in &args.packages {
        match find_package_fuzzy(query, &ctx.repo_root) {
            Ok(matched) => results.push(InstalledResult {
                query: query.clone(),
                matched,
            }),
            Err(err) => {
                ctx.printer
                    .error(&format!("installed lookup failed: {err}"));
                return 1;
            }
        }
    }

    if ctx.wants_json(args.json) {
        return render_installed_json(&results, &ctx.printer);
    }

    if results.len() == 1 {
        render_single_installed(&results[0], ctx, args.show_location)
    } else {
        render_multi_installed(&results, ctx)
    }
}

struct InstalledResult {
    query: String,
    matched: Option<PackageMatch>,
}

fn render_list_json(source: Option<&str>, buckets: &PackageBuckets, printer: &Printer) -> i32 {
    let output = source.map_or_else(
        || {
            let json = ListJsonOutput::from(buckets);
            serde_json::to_string_pretty(&json)
        },
        |source_key| {
            let mut map = Map::new();
            map.insert(
                source_key.to_string(),
                Value::Array(
                    source_values(source_key, buckets)
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
            serde_json::to_string_pretty(&map)
        },
    );
    match output {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(err) => {
            printer.error(&format!("list json rendering failed: {err}"));
            1
        }
    }
}

fn render_installed_json(results: &[InstalledResult], printer: &Printer) -> i32 {
    let all_installed = results.iter().all(|r| r.matched.is_some());
    let map: Map<String, Value> = results
        .iter()
        .map(|result| {
            let entry = result.matched.as_ref().map_or(
                InstalledEntry {
                    match_name: None,
                    location: None,
                },
                |found| InstalledEntry {
                    match_name: Some(found.name.clone()),
                    location: Some(found.location.to_string()),
                },
            );
            (
                result.query.clone(),
                serde_json::to_value(entry).unwrap_or_default(),
            )
        })
        .collect();
    match to_string_compact(&map) {
        Ok(text) => println!("{text}"),
        Err(err) => {
            printer.error(&format!("installed json rendering failed: {err}"));
            return 1;
        }
    }
    i32::from(!all_installed)
}

fn render_single_installed(result: &InstalledResult, ctx: &AppContext, show_location: bool) -> i32 {
    let Some(found) = &result.matched else {
        return 1;
    };
    if show_location {
        let rel = relative_location(&found.location, &ctx.repo_root);
        if found.name == result.query {
            ctx.printer.success(&format!("{} ({rel})", found.name));
        } else {
            ctx.printer
                .success(&format!("{} → {} ({rel})", result.query, found.name));
        }
    }
    0
}

fn render_multi_installed(results: &[InstalledResult], ctx: &AppContext) -> i32 {
    let all_installed = results.iter().all(|r| r.matched.is_some());
    let installed_count = results.iter().filter(|r| r.matched.is_some()).count();
    println!();
    Printer::detail(&format!(
        "Package Check ({installed_count}/{} installed)",
        results.len()
    ));

    for result in results {
        if let Some(found) = &result.matched {
            let rel = relative_location(&found.location, &ctx.repo_root);
            if found.name == result.query {
                ctx.printer.success(&result.query);
            } else {
                ctx.printer
                    .success(&format!("{} → {}", result.query, found.name));
            }
            Printer::detail(&format!("  {rel}"));
        } else {
            ctx.printer
                .warn(&format!("{} is not installed", result.query));
        }
    }
    i32::from(!all_installed)
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

#[derive(Serialize)]
struct InstalledEntry {
    #[serde(rename = "match")]
    match_name: Option<String>,
    location: Option<String>,
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
    sources: Vec<InfoSourceJson>,
    hm_module: Option<ConfigOptionInfo>,
    darwin_service: Option<ConfigOptionInfo>,
    flakehub: Vec<FlakeHubInfo>,
}

#[derive(Serialize)]
struct InfoSourceJson {
    source: String,
    version: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    dependencies: Option<Vec<String>>,
    build_dependencies: Option<Vec<String>>,
    caveats: Option<String>,
    artifacts: Option<Vec<String>>,
    broken: bool,
    insecure: bool,
    head_available: bool,
}

fn info_source_json_from_result(value: SourceResult) -> InfoSourceJson {
    let seed = InfoSourceSeed::new(value);
    match seed.source {
        PackageSource::Nxs | PackageSource::Nur | PackageSource::FlakeInput => {
            info_source_json_nix(seed)
        }
        PackageSource::Homebrew => info_source_json_homebrew(seed),
        PackageSource::Cask => info_source_json_cask(seed),
        PackageSource::Mas => info_source_json_mas(seed),
    }
}

struct InfoSourceSeed {
    source: PackageSource,
    source_name: String,
    lookup_name: String,
    version: Option<String>,
    fallback_description: Option<String>,
}

impl InfoSourceSeed {
    fn new(value: SourceResult) -> Self {
        let lookup_name = value.attr.clone().unwrap_or_else(|| value.name.clone());
        Self {
            source: value.source,
            source_name: value.source.to_string(),
            lookup_name,
            version: value.version,
            fallback_description: (!value.description.is_empty()).then_some(value.description),
        }
    }
}

fn info_source_json_nix(seed: InfoSourceSeed) -> InfoSourceJson {
    let metadata = nix_info_metadata(&seed.lookup_name);
    InfoSourceJson {
        source: seed.source_name,
        version: seed
            .version
            .or_else(|| metadata.as_ref().and_then(|meta| meta.version.clone())),
        description: metadata
            .as_ref()
            .and_then(|meta| meta.description.clone())
            .or(seed.fallback_description),
        homepage: metadata.as_ref().and_then(|meta| meta.homepage.clone()),
        license: metadata.as_ref().and_then(|meta| meta.license.clone()),
        dependencies: None,
        build_dependencies: None,
        caveats: None,
        artifacts: None,
        broken: metadata.as_ref().is_some_and(|meta| meta.broken),
        insecure: metadata.as_ref().is_some_and(|meta| meta.insecure),
        head_available: false,
    }
}

fn info_source_json_homebrew(seed: InfoSourceSeed) -> InfoSourceJson {
    let metadata = brew_formula_metadata(&seed.lookup_name);
    let (
        metadata_version,
        metadata_description,
        metadata_homepage,
        metadata_license,
        dependencies,
        build_dependencies,
        caveats,
        head_available,
    ) = metadata.map_or((None, None, None, None, None, None, None, false), |meta| {
        (
            meta.version,
            meta.description,
            meta.homepage,
            meta.license,
            meta.dependencies,
            meta.build_dependencies,
            meta.caveats,
            meta.head_available,
        )
    });
    InfoSourceJson {
        source: seed.source_name,
        version: seed.version.or(metadata_version),
        description: metadata_description.or(seed.fallback_description),
        homepage: metadata_homepage,
        license: metadata_license,
        dependencies,
        build_dependencies,
        caveats,
        artifacts: None,
        broken: false,
        insecure: false,
        head_available,
    }
}

fn info_source_json_cask(seed: InfoSourceSeed) -> InfoSourceJson {
    let metadata = brew_cask_metadata(&seed.lookup_name);
    let (metadata_version, metadata_description, metadata_homepage, artifacts) =
        metadata.map_or((None, None, None, None), |meta| {
            (
                meta.version,
                meta.description,
                meta.homepage,
                meta.artifacts,
            )
        });
    InfoSourceJson {
        source: seed.source_name,
        version: seed.version.or(metadata_version),
        description: metadata_description.or(seed.fallback_description),
        homepage: metadata_homepage,
        license: None,
        dependencies: None,
        build_dependencies: None,
        caveats: None,
        artifacts,
        broken: false,
        insecure: false,
        head_available: false,
    }
}

fn info_source_json_mas(seed: InfoSourceSeed) -> InfoSourceJson {
    InfoSourceJson {
        source: seed.source_name,
        version: seed.version,
        description: seed.fallback_description,
        homepage: None,
        license: None,
        dependencies: None,
        build_dependencies: None,
        caveats: None,
        artifacts: None,
        broken: false,
        insecure: false,
        head_available: false,
    }
}

#[derive(Default)]
struct NixInfoMetadata {
    version: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    broken: bool,
    insecure: bool,
}

#[derive(Default)]
struct BrewFormulaMetadata {
    version: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    dependencies: Option<Vec<String>>,
    build_dependencies: Option<Vec<String>>,
    caveats: Option<String>,
    head_available: bool,
}

#[derive(Default)]
struct BrewCaskMetadata {
    version: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    artifacts: Option<Vec<String>>,
}

fn nix_info_metadata(attr: &str) -> Option<NixInfoMetadata> {
    let mut meta = NixInfoMetadata {
        version: eval_nix_attr(attr, "version").and_then(|value| json_to_string(&value)),
        ..NixInfoMetadata::default()
    };

    let meta_json = eval_nix_attr(attr, "meta")?;
    meta.description = json_field_string(&meta_json, "description");
    meta.homepage = json_field_string(&meta_json, "homepage");
    meta.license = json_field_license(&meta_json);
    meta.broken = json_field_bool(&meta_json, "broken");
    meta.insecure = json_field_bool(&meta_json, "insecure");
    Some(meta)
}

fn brew_formula_metadata(name: &str) -> Option<BrewFormulaMetadata> {
    let entry = brew_info_entry(name, false)?;
    let versions = entry.get("versions");
    Some(BrewFormulaMetadata {
        version: versions
            .and_then(|value| value.get("stable"))
            .and_then(json_to_string),
        description: json_field_string(&entry, "desc"),
        homepage: json_field_string(&entry, "homepage"),
        license: json_field_string(&entry, "license"),
        dependencies: json_field_string_list(&entry, "dependencies"),
        build_dependencies: json_field_string_list(&entry, "build_dependencies"),
        caveats: json_field_string(&entry, "caveats"),
        head_available: versions
            .and_then(|value| value.get("head"))
            .is_some_and(|value| !value.is_null()),
    })
}

fn brew_cask_metadata(name: &str) -> Option<BrewCaskMetadata> {
    let entry = brew_info_entry(name, true)?;
    Some(BrewCaskMetadata {
        version: json_field_string(&entry, "version"),
        description: json_field_string(&entry, "desc"),
        homepage: json_field_string(&entry, "homepage"),
        artifacts: parse_cask_artifacts(entry.get("artifacts")),
    })
}

fn eval_nix_attr(attr: &str, suffix: &str) -> Option<Value> {
    for target in ["nxs", "nixpkgs", "github:nixos/nixpkgs/nixos-unstable"] {
        let query = format!("{target}#{attr}.{suffix}");
        if let Some(value) = run_json_command_quiet("nix", &["eval", "--json", &query]) {
            return Some(value);
        }
    }
    None
}

fn brew_info_entry(name: &str, is_cask: bool) -> Option<Value> {
    let mut args = vec!["info", "--json=v2"];
    if is_cask {
        args.push("--cask");
    }
    args.push(name);
    let key = if is_cask { "casks" } else { "formulae" };
    let data = run_json_command_quiet("brew", &args)?;
    data.get(key)
        .and_then(Value::as_array)
        .and_then(|entries| entries.first().cloned())
}

fn json_field_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(json_to_string)
}

fn json_field_bool(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn json_field_string_list(value: &Value, key: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for item in value.get(key).and_then(Value::as_array)? {
        let Some(text) = item.as_str() else {
            continue;
        };
        out.push(text.to_string());
    }
    Some(out)
}

fn json_field_license(meta: &Value) -> Option<String> {
    let license = meta.get("license")?;
    match license {
        Value::String(text) => Some(text.clone()),
        Value::Object(map) => map
            .get("spdxId")
            .and_then(json_to_string)
            .or_else(|| map.get("fullName").and_then(json_to_string)),
        Value::Array(items) => items.first().and_then(|first| match first {
            Value::Object(map) => map
                .get("spdxId")
                .and_then(json_to_string)
                .or_else(|| map.get("fullName").and_then(json_to_string)),
            Value::String(text) => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    }
}

fn json_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

fn parse_cask_artifacts(raw: Option<&Value>) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for artifact in raw.and_then(Value::as_array)? {
        let Some(map) = artifact.as_object() else {
            continue;
        };
        for key in ["app", "binary", "pkg"] {
            let Some(value) = map.get(key) else {
                continue;
            };
            match value {
                Value::String(item) => out.push(item.clone()),
                Value::Array(items) => {
                    for item in items {
                        if let Some(text) = item.as_str() {
                            out.push(text.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use tempfile::TempDir;

    use crate::domain::source::PackageSource;

    fn info_args() -> InfoArgs {
        InfoArgs {
            package: Some("ripgrep".to_string()),
            json: true,
            bleeding_edge: false,
            verbose: false,
        }
    }

    fn source_result(
        name: &str,
        source: PackageSource,
        attr: Option<&str>,
        confidence: f64,
    ) -> SourceResult {
        SourceResult {
            name: name.to_string(),
            source,
            attr: attr.map(str::to_string),
            version: Some("1.2.3".to_string()),
            confidence,
            description: "desc".to_string(),
            requires_flake_mod: false,
            flake_url: None,
        }
    }

    fn write_flake_lock(root: &Path) {
        let lock = serde_json::json!({
            "nodes": {
                "root": {"inputs": {"nixpkgs": "nixpkgs"}},
                "nixpkgs": {"locked": {"rev": "abcdef1234567890"}}
            }
        });
        fs::write(
            root.join("flake.lock"),
            serde_json::to_string(&lock).unwrap(),
        )
        .expect("flake.lock should be written");
    }

    #[test]
    fn collect_info_sources_uses_cache_before_search() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_flake_lock(root);
        let cache_dir = root.join(".cache/nx");
        fs::create_dir_all(&cache_dir).expect("cache dir should be created");

        let mut cache = Some(
            MultiSourceCache::load_with_cache_dir(root, &cache_dir).expect("cache should load"),
        );
        cache
            .as_mut()
            .expect("cache should exist")
            .set_many(&[source_result(
                "ripgrep",
                PackageSource::Nxs,
                Some("ripgrep"),
                0.95,
            )])
            .expect("cache set should succeed");

        let args = info_args();
        let searches = Cell::new(0usize);

        let results = collect_info_sources_with(
            package_from_args(&args),
            &args,
            root,
            &mut cache,
            |_, _, _| {
                searches.set(searches.get() + 1);
                Vec::new()
            },
        );

        assert_eq!(searches.get(), 0);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, PackageSource::Nxs);
    }

    #[test]
    fn collect_info_sources_falls_back_to_search_and_updates_cache() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_flake_lock(root);
        let cache_dir = root.join(".cache/nx");
        fs::create_dir_all(&cache_dir).expect("cache dir should be created");

        let mut cache = Some(
            MultiSourceCache::load_with_cache_dir(root, &cache_dir).expect("cache should load"),
        );

        let args = info_args();
        let search_calls = Cell::new(0usize);

        let searched_result = source_result("ripgrep", PackageSource::Nxs, Some("ripgrep"), 0.9);
        let results = collect_info_sources_with(
            package_from_args(&args),
            &args,
            root,
            &mut cache,
            |_, _, _| {
                search_calls.set(search_calls.get() + 1);
                vec![searched_result.clone()]
            },
        );

        assert_eq!(search_calls.get(), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].attr.as_deref(), Some("ripgrep"));

        let cached = cache
            .as_ref()
            .expect("cache should exist")
            .get_all("ripgrep");
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].attr.as_deref(), Some("ripgrep"));
    }

    #[test]
    fn collect_info_sources_searches_on_cache_miss() {
        let tmp = TempDir::new().expect("temp dir should be created");
        let root = tmp.path();
        write_flake_lock(root);
        let cache_dir = root.join(".cache/nx");
        fs::create_dir_all(&cache_dir).expect("cache dir should be created");

        let mut cache = Some(
            MultiSourceCache::load_with_cache_dir(root, &cache_dir).expect("cache should load"),
        );
        let args = info_args();
        let searches = Cell::new(0usize);

        let results = collect_info_sources_with(
            package_from_args(&args),
            &args,
            root,
            &mut cache,
            |_, _, _| {
                searches.set(searches.get() + 1);
                vec![source_result(
                    "ripgrep",
                    PackageSource::Nxs,
                    Some("ripgrep"),
                    1.0,
                )]
            },
        );

        assert_eq!(results.len(), 1);
        assert_eq!(searches.get(), 1);
    }

    #[test]
    fn info_source_json_serializes_required_metadata() {
        let source = source_result("mas-app", PackageSource::Mas, Some("mas-app"), 0.87);
        let entry = info_source_json_from_result(source);
        let value = serde_json::to_value(entry).expect("source json should serialize");

        assert_eq!(value.get("source").and_then(Value::as_str), Some("mas"));
        assert_eq!(value.get("version").and_then(Value::as_str), Some("1.2.3"));
        assert_eq!(
            value.get("description").and_then(Value::as_str),
            Some("desc")
        );
        assert!(value.get("homepage").is_some_and(Value::is_null));
        assert!(value.get("license").is_some_and(Value::is_null));
        assert!(value.get("dependencies").is_some_and(Value::is_null));
        assert!(value.get("build_dependencies").is_some_and(Value::is_null));
        assert!(value.get("caveats").is_some_and(Value::is_null));
        assert!(value.get("artifacts").is_some_and(Value::is_null));
        assert_eq!(value.get("broken").and_then(Value::as_bool), Some(false));
        assert_eq!(value.get("insecure").and_then(Value::as_bool), Some(false));
        assert_eq!(
            value.get("head_available").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn info_source_label_uses_nix_attr_display() {
        let source = source_result("ripgrep", PackageSource::Nxs, Some("ripgrep"), 1.0);
        assert_eq!(info_source_label(&source), "nxs (pkgs.ripgrep)");
    }

    #[test]
    fn info_status_text_matches_python_shape() {
        assert_eq!(info_status_text(false, None, None), "not installed");
        assert_eq!(info_status_text(true, Some("nxs"), None), "installed (nxs)");
        assert_eq!(
            info_status_text(true, Some("flake-input"), Some("fenix")),
            "installed via fenix"
        );
    }

    #[test]
    fn collect_info_flakehub_skips_lookup_when_disabled() {
        let searches = Cell::new(0usize);
        let results = collect_info_flakehub("ripgrep", false, |_| {
            searches.set(searches.get() + 1);
            vec![FlakeHubInfo {
                name: "Org/ripgrep".to_string(),
                description: "desc".to_string(),
                version: Some("1.0.0".to_string()),
            }]
        });
        assert!(results.is_empty());
        assert_eq!(searches.get(), 0);
    }

    #[test]
    fn collect_info_flakehub_limits_results_to_three() {
        let results = collect_info_flakehub("ripgrep", true, |_| {
            vec![
                FlakeHubInfo {
                    name: "Org/a".to_string(),
                    description: String::new(),
                    version: None,
                },
                FlakeHubInfo {
                    name: "Org/b".to_string(),
                    description: String::new(),
                    version: None,
                },
                FlakeHubInfo {
                    name: "Org/c".to_string(),
                    description: String::new(),
                    version: None,
                },
                FlakeHubInfo {
                    name: "Org/d".to_string(),
                    description: String::new(),
                    version: None,
                },
            ]
        });
        assert_eq!(results.len(), 3);
        assert_eq!(results[2].name, "Org/c");
    }

    fn package_from_args(args: &InfoArgs) -> &str {
        args.package
            .as_deref()
            .expect("info args in tests should include package")
    }
}
