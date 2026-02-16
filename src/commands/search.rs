use crate::cli::SearchArgs;
use crate::commands::context::AppContext;
use crate::domain::source::{SourcePreferences, SourceResult};
use crate::infra::sources::search_all_sources;

pub fn cmd_search(args: &SearchArgs, ctx: &AppContext) -> i32 {
    let prefs = SourcePreferences {
        bleeding_edge: args.bleeding_edge,
        nur: args.nur,
        ..Default::default()
    };

    let flake_lock = ctx.repo_root.join("flake.lock");
    let flake_lock_path = flake_lock.exists().then_some(flake_lock.as_path());

    ctx.printer.searching(&args.package);
    let results = search_all_sources(&args.package, &prefs, flake_lock_path);
    ctx.printer.searching_done();

    if results.is_empty() {
        ctx.printer
            .error(&format!("{}: not found in any source", args.package));
        return 1;
    }

    if args.json {
        return render_json(&results);
    }

    render_table(&results, ctx);
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

fn render_table(results: &[SourceResult], ctx: &AppContext) {
    println!();
    ctx.printer
        .action(&format!("Results for '{}'", results[0].name));
    println!();

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

        println!("  {:<12} {attr_display}{version}{desc}", r.source,);
    }
}
