use crate::cli::SearchArgs;
use crate::commands::context::JsonCommandContext;
use crate::domain::source::{SourcePreferences, SourceResult};
use crate::infra::sources::{UnavailableSource, search_all_sources};
use crate::output::printer::Printer;

pub fn cmd_search(args: &SearchArgs, ctx: &JsonCommandContext<'_>) -> i32 {
    let prefs = SourcePreferences {
        bleeding_edge: args.bleeding_edge,
        nur: args.nur,
        ..Default::default()
    };

    let flake_lock = ctx.repo_root.join("flake.lock");
    let flake_lock_path = flake_lock.exists().then_some(flake_lock.as_path());

    Printer::searching(&args.package);
    let outcome = search_all_sources(&args.package, &prefs, flake_lock_path);
    Printer::searching_done();

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

    if ctx.wants_json(args.json) {
        return render_json(&outcome.results);
    }

    render_table(&outcome.results);
    0
}

fn render_json(results: &[SourceResult]) -> i32 {
    let entries: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "source": r.source.as_str(),
                "attr": r.attr,
                "version": r.version,
                "confidence": r.confidence,
                "description": r.description,
            })
        })
        .collect();

    match serde_json::to_string_pretty(&entries) {
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
