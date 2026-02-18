use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::cli::{InfoArgs, InstalledArgs, ListArgs, WhereArgs};
use crate::commands::context::AppContext;
use crate::commands::shared::{SnippetMode, relative_location, show_snippet};
use crate::domain::source::{SourcePreferences, SourceResult};
use crate::infra::cache::MultiSourceCache;
use crate::infra::config_scan::{PackageBuckets, scan_packages};
use crate::infra::finder::{PackageMatch, find_package, find_package_fuzzy};
use crate::infra::sources::search_all_sources;
use crate::output::json::to_string_compact;
use crate::output::printer::Printer;

const VALID_SOURCES_TEXT: &str =
    "  Valid sources: brew, brews, cask, casks, homebrew, mas, nix, nxs, service,\n  services";

pub fn cmd_where(args: &WhereArgs, ctx: &AppContext) -> i32 {
    let Some(package) = &args.package else {
        ctx.printer.error("No package specified");
        return 1;
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
            ctx.printer.detail(&format!("Try: nx info {package}"));
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

    if args.json {
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
        ctx.printer.error("No package specified");
        ctx.printer.detail("Usage: nx info <package>");
        return 1;
    };

    let location = match find_package(package, &ctx.repo_root) {
        Ok(location) => location,
        Err(err) => {
            ctx.printer.error(&format!("info lookup failed: {err}"));
            return 1;
        }
    };

    if args.json {
        let mut cache = MultiSourceCache::load(&ctx.repo_root).ok();
        let sources = collect_info_sources(
            package,
            args,
            &ctx.repo_root,
            &mut cache,
            location.is_some(),
        )
        .into_iter()
        .map(InfoSourceJson::from)
        .collect();

        let output = InfoJsonOutput {
            name: package.clone(),
            installed: location.is_some(),
            location: location.map(|value| value.to_string()),
            sources,
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
                ctx.printer
                    .error(&format!("info json rendering failed: {err}"));
                return 1;
            }
        }
    }

    let status = if location.is_some() {
        "installed"
    } else {
        "not installed"
    };
    println!();
    ctx.printer.detail(&format!("{package} ({status})"));
    if let Some(location) = location {
        ctx.printer.detail(&format!(
            "Location: {}",
            relative_location(&location, &ctx.repo_root)
        ));
        if let Some(line_num) = location.line() {
            show_snippet(location.path(), line_num, 1, SnippetMode::Add, false);
        }
    } else {
        ctx.printer.error(&format!("{package} not found"));
        println!();
        ctx.printer.detail(&format!("Try: nx {package}"));
    }
    0
}

fn source_prefs_from_info_args(args: &InfoArgs) -> SourcePreferences {
    SourcePreferences {
        bleeding_edge: args.bleeding_edge,
        nur: args.bleeding_edge,
        ..Default::default()
    }
}

fn collect_info_sources(
    package: &str,
    args: &InfoArgs,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
    search_on_miss: bool,
) -> Vec<SourceResult> {
    collect_info_sources_with(
        package,
        args,
        repo_root,
        cache,
        search_on_miss,
        search_all_sources,
    )
}

fn collect_info_sources_with<F>(
    package: &str,
    args: &InfoArgs,
    repo_root: &Path,
    cache: &mut Option<MultiSourceCache>,
    search_on_miss: bool,
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
    if !search_on_miss {
        return Vec::new();
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
        ctx.printer.error("No package specified");
        return 1;
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

    if args.json {
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
    let output = if let Some(source_key) = source {
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
    } else {
        let json = ListJsonOutput::from(buckets);
        serde_json::to_string_pretty(&json)
    };
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
            let entry = match &result.matched {
                Some(found) => InstalledEntry {
                    match_name: Some(found.name.clone()),
                    location: Some(found.location.to_string()),
                },
                None => InstalledEntry {
                    match_name: None,
                    location: None,
                },
            };
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
    ctx.printer.detail(&format!(
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
            ctx.printer.detail(&format!("  {rel}"));
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
    hm_module: Option<Value>,
    darwin_service: Option<Value>,
    flakehub: Vec<Value>,
}

#[derive(Serialize)]
struct InfoSourceJson {
    source: String,
    name: String,
    version: Option<String>,
    confidence: f64,
    description: String,
}

impl From<SourceResult> for InfoSourceJson {
    fn from(value: SourceResult) -> Self {
        Self {
            source: value.source.to_string(),
            name: value.name,
            version: value.version,
            confidence: value.confidence,
            description: value.description,
        }
    }
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
            .set(&source_result(
                "ripgrep",
                PackageSource::Nxs,
                Some("ripgrep"),
                0.95,
            ))
            .expect("cache set should succeed");

        let args = info_args();
        let searches = Cell::new(0usize);

        let results = collect_info_sources_with(
            package_from_args(&args),
            &args,
            root,
            &mut cache,
            true,
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
            true,
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
    fn collect_info_sources_skips_search_when_not_installed() {
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
            false,
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

        assert!(results.is_empty());
        assert_eq!(searches.get(), 0);
    }

    #[test]
    fn info_source_json_serializes_required_metadata() {
        let source = source_result("ripgrep", PackageSource::Nxs, Some("ripgrep"), 0.87);
        let entry = InfoSourceJson::from(source);
        let value = serde_json::to_value(entry).expect("source json should serialize");

        assert_eq!(value.get("source").and_then(Value::as_str), Some("nxs"));
        assert_eq!(value.get("name").and_then(Value::as_str), Some("ripgrep"));
        assert_eq!(value.get("version").and_then(Value::as_str), Some("1.2.3"));
        assert_eq!(
            value.get("description").and_then(Value::as_str),
            Some("desc")
        );
        assert_eq!(value.get("confidence").and_then(Value::as_f64), Some(0.87));
    }

    fn package_from_args(args: &InfoArgs) -> &str {
        args.package
            .as_deref()
            .expect("info args in tests should include package")
    }
}
