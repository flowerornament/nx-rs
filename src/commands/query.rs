use serde::Serialize;
use serde_json::{Map, Value};

use crate::cli::{InfoArgs, InstalledArgs, ListArgs, WhereArgs};
use crate::commands::context::AppContext;
use crate::commands::shared::{
    SnippetMode, location_path_and_line, relative_location, show_snippet,
};
use crate::infra::config_scan::{PackageBuckets, scan_packages};
use crate::infra::finder::{PackageMatch, find_package, find_package_fuzzy};
use crate::output::json::to_string_compact;

const VALID_SOURCES_TEXT: &str =
    "  Valid sources: brew, brews, cask, casks, homebrew, mas, nix, nxs, service,\n  services";

pub fn cmd_where(args: &WhereArgs, ctx: &AppContext) -> i32 {
    let Some(package) = &args.package else {
        eprintln!("x No package specified");
        return 1;
    };

    match find_package(package, &ctx.repo_root) {
        Ok(Some(location)) => {
            println!(
                "+ {package} at {}",
                relative_location(&location, &ctx.repo_root)
            );
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

pub fn cmd_list(args: &ListArgs, ctx: &AppContext) -> i32 {
    let buckets = match scan_packages(&ctx.repo_root) {
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

pub fn cmd_info(args: &InfoArgs, ctx: &AppContext) -> i32 {
    let Some(package) = &args.package else {
        eprintln!("x No package specified");
        println!("  Usage: nx info <package>");
        return 1;
    };

    let location = match find_package(package, &ctx.repo_root) {
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
        println!(
            "  Location: {}",
            relative_location(&location, &ctx.repo_root)
        );
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

pub fn cmd_status(ctx: &AppContext) -> i32 {
    let buckets = match scan_packages(&ctx.repo_root) {
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

pub fn cmd_installed(args: &InstalledArgs, ctx: &AppContext) -> i32 {
    if args.packages.is_empty() {
        eprintln!("x No package specified");
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
                eprintln!("x installed lookup failed: {err}");
                return 1;
            }
        }
    }

    if args.json {
        return render_installed_json(&results);
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

fn render_installed_json(results: &[InstalledResult]) -> i32 {
    let mut map = Map::new();
    let mut all_installed = true;
    for result in results {
        let entry = match &result.matched {
            Some(found) => InstalledEntry {
                match_name: Some(found.name.clone()),
                location: Some(found.location.clone()),
            },
            None => {
                all_installed = false;
                InstalledEntry {
                    match_name: None,
                    location: None,
                }
            }
        };
        map.insert(
            result.query.clone(),
            serde_json::to_value(entry).unwrap_or_default(),
        );
    }
    match to_string_compact(&map) {
        Ok(text) => println!("{text}"),
        Err(err) => {
            eprintln!("x installed json rendering failed: {err}");
            return 1;
        }
    }
    if all_installed { 0 } else { 1 }
}

fn render_single_installed(result: &InstalledResult, ctx: &AppContext, show_location: bool) -> i32 {
    let Some(found) = &result.matched else {
        return 1;
    };
    if show_location {
        let rel = relative_location(&found.location, &ctx.repo_root);
        if found.name != result.query {
            ctx.printer
                .success(&format!("{} → {} ({rel})", result.query, found.name));
        } else {
            ctx.printer.success(&format!("{} ({rel})", found.name));
        }
    }
    0
}

fn render_multi_installed(results: &[InstalledResult], ctx: &AppContext) -> i32 {
    let installed_count = results.iter().filter(|r| r.matched.is_some()).count();
    println!();
    ctx.printer.detail(&format!(
        "Package Check ({installed_count}/{} installed)",
        results.len()
    ));

    let mut all_installed = true;
    for result in results {
        if let Some(found) = &result.matched {
            let rel = relative_location(&found.location, &ctx.repo_root);
            if found.name != result.query {
                ctx.printer
                    .success(&format!("{} → {}", result.query, found.name));
            } else {
                ctx.printer.success(&result.query);
            }
            ctx.printer.detail(&format!("  {rel}"));
        } else {
            ctx.printer
                .warn(&format!("{} is not installed", result.query));
            all_installed = false;
        }
    }
    if all_installed { 0 } else { 1 }
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
