use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::cli::UnusedArgs;
use crate::commands::context::QueryContext;
use crate::commands::shared::relative_location;
use crate::domain::usage::{
    DeclaredPackage, UsageAuditOptions, UsageRecord, UsageSource, UsageStatus, audit_usage_records,
    parse_since_seconds,
};
use crate::infra::config_scan::{PackageBuckets, scan_packages};
use crate::infra::finder::find_package;
use crate::infra::shell_history::{ShellHistoryEntry, parse_timestamped_shell_history};
use crate::output::printer::Printer;

const PACKAGE_COLUMN_WIDTH: usize = 24;

pub fn cmd_unused(args: &UnusedArgs, ctx: &QueryContext<'_>) -> i32 {
    let Some(since_seconds) = parse_since_seconds(&args.since) else {
        ctx.printer
            .error("invalid --since value; use a duration like 30d, 12w, 6mo, or 1y");
        return 2;
    };
    let Some(source_filter) = SourceFilter::parse(&args.source) else {
        ctx.printer
            .error("invalid --source value; use all, nix, homebrew, cask, mas, or service");
        return 2;
    };

    let buckets = match scan_packages(ctx.repo_root) {
        Ok(buckets) => buckets,
        Err(err) => {
            ctx.printer.error(&format!("package scan failed: {err}"));
            return 1;
        }
    };

    let declared = declared_packages(&buckets, ctx.repo_root)
        .into_iter()
        .filter(|package| source_filter.matches(package.source))
        .collect::<Vec<_>>();
    let shell_history = if args.no_history {
        Vec::new()
    } else {
        load_shell_history(&history_paths(args), (!args.json).then_some(ctx.printer))
    };
    let now_epoch_secs = current_epoch_secs();
    let all_records = audit_usage_records(
        &declared,
        &shell_history,
        now_epoch_secs,
        UsageAuditOptions { since_seconds },
    );
    let hidden_protected = all_records
        .iter()
        .filter(|record| record.status == UsageStatus::Protected)
        .count();
    let records = all_records
        .into_iter()
        .filter(|record| args.include_protected || record.status != UsageStatus::Protected)
        .collect::<Vec<_>>();

    if args.json {
        return render_json(&records, args, since_seconds, ctx.printer);
    }

    render_human(&records, args, hidden_protected);
    0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceFilter {
    All,
    Source(UsageSource),
}

impl SourceFilter {
    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "nix" | "nxs" => Some(Self::Source(UsageSource::Nix)),
            "brew" | "brews" | "homebrew" => Some(Self::Source(UsageSource::Homebrew)),
            "cask" | "casks" => Some(Self::Source(UsageSource::Cask)),
            "mas" => Some(Self::Source(UsageSource::Mas)),
            "service" | "services" => Some(Self::Source(UsageSource::Service)),
            _ => None,
        }
    }

    fn matches(self, source: UsageSource) -> bool {
        match self {
            Self::All => true,
            Self::Source(selected) => source == selected,
        }
    }
}

fn declared_packages(buckets: &PackageBuckets, repo_root: &Path) -> Vec<DeclaredPackage> {
    let mut out = Vec::new();
    push_declared(&mut out, &buckets.nxs, UsageSource::Nix, repo_root);
    push_declared(&mut out, &buckets.brews, UsageSource::Homebrew, repo_root);
    push_declared(&mut out, &buckets.casks, UsageSource::Cask, repo_root);
    push_declared(&mut out, &buckets.mas, UsageSource::Mas, repo_root);
    push_declared(&mut out, &buckets.services, UsageSource::Service, repo_root);
    out
}

fn push_declared(
    out: &mut Vec<DeclaredPackage>,
    names: &[String],
    source: UsageSource,
    repo_root: &Path,
) {
    for name in names {
        let location = find_package(name, repo_root)
            .ok()
            .flatten()
            .map(|location| relative_location(&location, repo_root));
        out.push(DeclaredPackage {
            name: name.clone(),
            source,
            location,
        });
    }
}

fn history_paths(args: &UnusedArgs) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for path in &args.history {
        push_history_path(&mut out, &mut seen, path.clone());
    }
    if let Some(path) = env::var_os("HISTFILE").map(PathBuf::from) {
        push_history_path(&mut out, &mut seen, expand_home(path));
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        for rel in [".zsh_history", ".bash_history", ".config/fish/fish_history"] {
            push_history_path(&mut out, &mut seen, home.join(rel));
        }
    }
    out
}

fn push_history_path(out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, path: PathBuf) {
    if seen.insert(path.clone()) {
        out.push(path);
    }
}

fn expand_home(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return env::var_os("HOME").map_or(path, PathBuf::from);
    }
    if let Some(stripped) = raw.strip_prefix("~/")
        && let Some(home) = env::var_os("HOME").map(PathBuf::from)
    {
        return home.join(stripped);
    }
    path
}

fn load_shell_history(paths: &[PathBuf], printer: Option<&Printer>) -> Vec<ShellHistoryEntry> {
    let mut entries = Vec::new();
    for path in paths {
        match fs::read_to_string(path) {
            Ok(text) => entries.extend(parse_timestamped_shell_history(&text)),
            Err(err) if path.exists() => {
                let Some(printer) = printer else {
                    continue;
                };
                printer.warn(&format!(
                    "Could not read history file {}: {err}",
                    path.display()
                ));
            }
            Err(_) => {}
        }
    }
    entries
}

fn current_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn render_human(records: &[UsageRecord], args: &UnusedArgs, hidden_protected: usize) {
    let candidates = candidate_records(records);
    let rendered = candidates.iter().take(args.limit).collect::<Vec<_>>();

    Printer::heading(&format!("Package Usage Audit ({})", args.since));
    Printer::body(&format!("Review candidates ({})", candidates.len()));
    if rendered.is_empty() {
        Printer::detail("No review candidates found with the selected filters.");
    } else {
        Printer::body(&format!(
            "{:<24} {:<10} {:<18} Why",
            "Package", "Source", "Last evidence"
        ));
        for record in &rendered {
            Printer::body(&format!(
                "{:<24} {:<10} {:<18} {}",
                package_cell(&record.name),
                record.source.as_str(),
                last_evidence(record),
                reason(record)
            ));
            if args.verbose {
                for item in &record.evidence {
                    Printer::detail(&format!(
                        "- {} ({:?}, {:?})",
                        item.summary, item.kind, item.confidence
                    ));
                }
                if let Some(location) = record.location.as_deref() {
                    Printer::detail(&format!("- declared at {location}"));
                }
            }
        }
        if candidates.len() > args.limit {
            Printer::detail(&format!(
                "... and {} more candidate(s); use --limit {} to show more",
                candidates.len() - args.limit,
                candidates.len()
            ));
        }
    }

    if hidden_protected > 0 {
        println!();
        Printer::detail(&format!(
            "Protected hidden: {hidden_protected}; use --include-protected to show them"
        ));
    }
    if !rendered.is_empty() {
        println!();
        Printer::body("Try:");
        for record in rendered.iter().take(2) {
            for suggestion in &record.suggestions {
                Printer::sub_detail(suggestion);
            }
        }
    }
}

fn render_json(
    records: &[UsageRecord],
    args: &UnusedArgs,
    since_seconds: u64,
    printer: &Printer,
) -> i32 {
    let output = UnusedJsonOutput {
        since: args.since.clone(),
        since_seconds,
        records,
    };
    match serde_json::to_string_pretty(&output) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(err) => {
            printer.error(&format!("unused json rendering failed: {err}"));
            1
        }
    }
}

#[derive(Serialize)]
struct UnusedJsonOutput<'a> {
    since: String,
    since_seconds: u64,
    records: &'a [UsageRecord],
}

fn candidate_records(records: &[UsageRecord]) -> Vec<&UsageRecord> {
    let mut out = records
        .iter()
        .filter(|record| matches!(record.status, UsageStatus::Old | UsageStatus::Unknown))
        .collect::<Vec<_>>();
    out.sort_by_key(|record| {
        (
            record.status != UsageStatus::Unknown,
            record.last_seen.unwrap_or(i64::MIN),
            record.name.clone(),
        )
    });
    out
}

fn last_evidence(record: &UsageRecord) -> String {
    record
        .last_seen
        .map_or_else(|| "none".to_string(), |timestamp| timestamp.to_string())
}

fn package_cell(name: &str) -> String {
    let chars = name.chars().collect::<Vec<_>>();
    if chars.len() <= PACKAGE_COLUMN_WIDTH {
        return name.to_string();
    }

    chars
        .into_iter()
        .take(PACKAGE_COLUMN_WIDTH.saturating_sub(3))
        .chain("...".chars())
        .collect()
}

fn reason(record: &UsageRecord) -> &'static str {
    match record.status {
        UsageStatus::Unknown => "no command evidence found",
        UsageStatus::Old => "last command evidence is outside the window",
        UsageStatus::Recent => "recent command evidence found",
        UsageStatus::Protected => "protected by policy",
    }
}

#[cfg(test)]
mod tests {
    use super::{SourceFilter, candidate_records, package_cell};
    use crate::domain::usage::{UsageRecord, UsageSource, UsageStatus};

    #[test]
    fn source_filter_accepts_aliases() {
        assert!(SourceFilter::parse("nix").is_some_and(|filter| filter.matches(UsageSource::Nix)));
        assert!(
            SourceFilter::parse("brew").is_some_and(|filter| filter.matches(UsageSource::Homebrew))
        );
        assert!(SourceFilter::parse("wat").is_none());
    }

    #[test]
    fn candidate_records_prioritize_unknown_before_old() {
        let old = record("old", UsageStatus::Old, Some(10));
        let unknown = record("unknown", UsageStatus::Unknown, None);
        let recent = record("recent", UsageStatus::Recent, Some(20));

        let records = [old, unknown, recent];
        let names = candidate_records(&records)
            .into_iter()
            .map(|record| record.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["unknown", "old"]);
    }

    #[test]
    fn package_cell_truncates_long_names_to_table_width() {
        assert_eq!(
            package_cell("beam27Packages.elixir_1_20"),
            "beam27Packages.elixir..."
        );
    }

    fn record(name: &str, status: UsageStatus, last_seen: Option<i64>) -> UsageRecord {
        UsageRecord {
            name: name.to_string(),
            source: UsageSource::Nix,
            location: None,
            status,
            last_seen,
            confidence: crate::domain::usage::EvidenceConfidence::None,
            evidence: Vec::new(),
            suggestions: Vec::new(),
        }
    }
}
