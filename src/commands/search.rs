use crate::cli::SearchArgs;
use crate::commands::context::QueryContext;
use crate::domain::source::{SourcePreferences, SourceResult};
use crate::infra::cache::MultiSourceCache;
use crate::infra::package_query::query_package;
use crate::infra::sources::UnavailableSource;
use crate::output::printer::Printer;

pub fn cmd_search(args: &SearchArgs, ctx: &QueryContext<'_>) -> i32 {
    let prefs = SourcePreferences {
        bleeding_edge: args.bleeding_edge(),
        nur: args.nur(),
        force_source: args.source().map(str::to_owned),
        ..Default::default()
    };

    let mut cache = MultiSourceCache::load(ctx.repo_root).ok();

    Printer::searching(&args.package);
    let report = query_package(&args.package, &prefs, ctx.repo_root, &mut cache);
    Printer::searching_done();
    let outcome = &report.outcome;

    if outcome.results.is_empty() {
        if outcome.unavailable_sources.is_empty() {
            ctx.printer
                .error(&format!("{}: not found in any source", args.package));
        } else {
            ctx.printer.error(&format!(
                "{}: not found in any available source",
                args.package
            ));
            render_unavailable_sources(ctx.printer, &outcome.unavailable_sources);
        }
        return 1;
    }

    if args.json() {
        return render_json(&args.package, &report);
    }

    render_table(&outcome.results);
    if args.verbose() {
        println!();
        Printer::detail(&format!(
            "Query diagnostics: cache={}, elapsed={}ms, unavailable_backends={}",
            if report.cache_hit { "hit" } else { "miss" },
            report.elapsed.as_millis(),
            outcome.unavailable_sources.len()
        ));
    }
    0
}

fn render_json(package: &str, report: &crate::infra::package_query::PackageQueryReport) -> i32 {
    let entries: Vec<serde_json::Value> = report
        .outcome
        .results
        .iter()
        .map(source_result_json)
        .collect();

    match serde_json::to_string_pretty(&serde_json::json!({
        "package": package,
        "cache_hit": report.cache_hit,
        "elapsed_ms": report.elapsed.as_millis(),
        "unavailable_sources": report.outcome.unavailable_sources,
        "results": entries,
    })) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(err) => {
            eprintln!("json rendering failed: {err}");
            1
        }
    }
}

fn source_result_json(result: &SourceResult) -> serde_json::Value {
    serde_json::json!({
        "name": result.name,
        "source": result.source.as_str(),
        "attr": result.attr,
        "version": result.version,
        "confidence": result.confidence,
        "description": result.description,
    })
}

fn render_table(results: &[SourceResult]) {
    Printer::heading(&format!(
        "Results for '{}' ({} sources)",
        results[0].name,
        results.len()
    ));

    for r in results {
        let attr_display = r.attr.as_deref().unwrap_or(&r.name);
        let version = r
            .version
            .as_deref()
            .map(|v| format!(" ({v})"))
            .unwrap_or_default();
        let desc = if r.description.is_empty() {
            String::new()
        } else {
            format!(" - {}", r.description)
        };

        Printer::body(&format!("{:<12} {attr_display}{version}{desc}", r.source));
    }
}

fn render_unavailable_sources(_printer: &Printer, unavailable_sources: &[UnavailableSource]) {
    for source in unavailable_sources {
        Printer::detail(&format!("- {}: {}", source.source, source.reason));
    }
}
