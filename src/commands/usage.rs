use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use serde::Serialize;

use crate::cli::UsageArgs;
use crate::commands::context::QueryContext;
use crate::domain::drift::{ManifestHealth, format_issue};
use crate::domain::package::{InventoryIssue, PackageDeclaration, PackageInventory, PackageSource};
use crate::domain::usage::{
    EvidenceConfidence, UsageRecord, UsageVerdict, classify_usage, parse_since_seconds,
};
use crate::infra::config_scan::scan_packages;
use crate::infra::shell_history::{ShellHistoryEntry, parse_shell_history};
use crate::infra::usage_evidence::UsageEvidence;
use crate::output::printer::Printer;

const PACKAGE_COLUMN_WIDTH: usize = 25;
const ARTIFACT_EXAMPLE_LIMIT: usize = 8;
const HUMAN_ARTIFACT_LIMIT: usize = 12;

pub fn cmd_usage(args: &UsageArgs, ctx: &QueryContext<'_>) -> i32 {
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

    let inventory = match scan_packages(ctx.repo_root) {
        Ok(inventory) => inventory,
        Err(err) => {
            ctx.printer.error(&format!("package scan failed: {err}"));
            return 1;
        }
    };
    let declarations = match select_declarations(&inventory, source_filter, args, ctx.printer) {
        Ok(declarations) => declarations,
        Err(code) => return code,
    };
    let shell_history = if args.no_history {
        LoadedShellHistory::default()
    } else {
        load_shell_history(&history_paths(args), (!args.json).then_some(ctx.printer))
    };
    let aliases = ctx
        .manifest
        .map_or_else(HashMap::new, |manifest| manifest.aliases.clone());
    let now = current_epoch_secs();
    let evidence = UsageEvidence::collect(
        &shell_history.entries,
        shell_history.limitations,
        !args.no_history,
        aliases,
        now,
    );
    let cutoff = now.saturating_sub(i64::try_from(since_seconds).unwrap_or(i64::MAX));
    let mut records = declarations
        .into_iter()
        .map(|declaration| {
            let observation = evidence.observe(declaration, cutoff);
            classify_usage(
                declaration,
                observation.plan,
                observation.evidence,
                observation.coverage,
                cutoff,
            )
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| {
        left.source
            .as_str()
            .cmp(right.source.as_str())
            .then_with(|| left.name.cmp(&right.name))
    });

    if args.package.is_some() {
        let record = records
            .first()
            .expect("single-package filtering should retain exactly one record");
        if args.json {
            return render_json(
                std::slice::from_ref(record),
                0,
                &inventory.issues,
                args,
                since_seconds,
                ctx,
            );
        }
        render_explanation(record, &inventory.issues, ctx);
        return 0;
    }

    let hidden_protected = if args.include_protected {
        0
    } else {
        records
            .iter()
            .filter(|record| record.verdict == UsageVerdict::Protected)
            .count()
    };
    records.retain(|record| args.include_protected || record.verdict != UsageVerdict::Protected);
    if args.json {
        return render_json(
            &records,
            hidden_protected,
            &inventory.issues,
            args,
            since_seconds,
            ctx,
        );
    }

    render_human(&records, hidden_protected, &inventory.issues, args, ctx);
    0
}

fn select_declarations<'a>(
    inventory: &'a PackageInventory,
    source_filter: SourceFilter,
    args: &UsageArgs,
    printer: &Printer,
) -> Result<Vec<&'a PackageDeclaration>, i32> {
    let mut declarations = inventory
        .declarations
        .iter()
        .filter(|declaration| source_filter.matches(declaration.source))
        .collect::<Vec<_>>();
    let Some(package) = args.package.as_deref() else {
        return Ok(declarations);
    };

    declarations.retain(|declaration| declaration.name.eq_ignore_ascii_case(package));
    if declarations.is_empty() {
        printer.error(&format!(
            "{package} is not in the selected package inventory"
        ));
        return Err(1);
    }
    if declarations.len() > 1 {
        let sources = declarations
            .iter()
            .map(|declaration| declaration.source.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        printer.error(&format!(
            "{package} is declared in multiple sources ({sources}); select one with --source"
        ));
        return Err(2);
    }
    Ok(declarations)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceFilter {
    All,
    Source(PackageSource),
}

impl SourceFilter {
    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "nix" | "nxs" => Some(Self::Source(PackageSource::Nix)),
            "brew" | "brews" | "homebrew" => Some(Self::Source(PackageSource::Homebrew)),
            "cask" | "casks" => Some(Self::Source(PackageSource::Cask)),
            "mas" => Some(Self::Source(PackageSource::Mas)),
            "service" | "services" => Some(Self::Source(PackageSource::Service)),
            _ => None,
        }
    }

    fn matches(self, source: PackageSource) -> bool {
        match self {
            Self::All => true,
            Self::Source(selected) => selected == source,
        }
    }
}

fn render_human(
    records: &[UsageRecord],
    hidden_protected: usize,
    inventory_issues: &[InventoryIssue],
    args: &UsageArgs,
    ctx: &QueryContext<'_>,
) {
    let summary = UsageSummary::from_records(records);
    Printer::heading(&format!("Package Usage ({})", args.since));
    Printer::detail(&format!(
        "{} packages: {} recent, {} observed without recency, {} review {}",
        records.len(),
        summary.observed_recent,
        summary.observed_undated,
        summary.candidates,
        plural(summary.candidates, "candidate", "candidates")
    ));
    if hidden_protected > 0 {
        Printer::detail(&format!(
            "{hidden_protected} protected {} hidden; use --include-protected to show {}",
            plural(hidden_protected, "package", "packages"),
            plural(hidden_protected, "it", "them")
        ));
    }
    render_health(inventory_issues, ctx.manifest_health);

    println!();
    Printer::body(&format!("Review candidates ({})", summary.candidates));
    let candidates = candidate_records(records);
    if candidates.is_empty() {
        Printer::detail("No packages have a trustworthy dated observation outside this window.");
    } else {
        Printer::body(&format!(
            "{:<package_width$} {:<10} {:<14} Why",
            "Package",
            "Source",
            "Last observed",
            package_width = PACKAGE_COLUMN_WIDTH
        ));
        for record in candidates.iter().take(args.limit) {
            Printer::body(&format!(
                "{:<package_width$} {:<10} {:<14} {}",
                package_cell(&record.name),
                record.source.as_str(),
                last_observed(record),
                record.explanation(),
                package_width = PACKAGE_COLUMN_WIDTH
            ));
            if args.verbose {
                render_record_details(record);
            }
        }
        if candidates.len() > args.limit {
            Printer::detail(&format!(
                "... and {} more; use --limit {} to show all candidates",
                candidates.len() - args.limit,
                candidates.len()
            ));
        }
    }

    if args.include_protected {
        let protected = records
            .iter()
            .filter(|record| record.verdict == UsageVerdict::Protected)
            .collect::<Vec<_>>();
        if !protected.is_empty() {
            println!();
            Printer::body(&format!("Protected ({})", protected.len()));
            for record in protected {
                Printer::detail(&format!(
                    "{} ({}) - {}",
                    record.name,
                    record.source.as_str(),
                    record.explanation()
                ));
            }
        }
    }

    println!();
    Printer::body("Evidence coverage");
    Printer::detail(&format!(
        "{} unobserved (not candidates), {} insufficient, {} not auditable, {} inventory uncertain",
        summary.no_evidence,
        summary.insufficient_evidence,
        summary.not_auditable,
        summary.inventory_uncertain
    ));
    if !candidates.is_empty() {
        Printer::detail("Inspect one package with `nx usage <package>` before removal.");
    }
}

fn render_explanation(
    record: &UsageRecord,
    inventory_issues: &[InventoryIssue],
    ctx: &QueryContext<'_>,
) {
    Printer::heading(&format!("Package Usage: {}", record.name));
    Printer::body(&format!("Verdict: {}", verdict_label(record.verdict)));
    Printer::detail(record.explanation());
    Printer::detail(&format!("Source: {}", record.source.as_str()));
    for site in &record.declarations {
        Printer::detail(&format!(
            "Declared: {} at {}",
            declaration_kind_label(&site.kind),
            site.location
        ));
    }

    if !record.artifacts.is_empty() {
        println!();
        Printer::body("Artifacts");
        render_artifacts(&record.artifacts);
    }

    println!();
    Printer::body("Evidence");
    if record.evidence.is_empty() {
        Printer::detail("No observations.");
    } else {
        for evidence in &record.evidence {
            Printer::detail(&format!("- {}", evidence.summary));
        }
    }

    println!();
    Printer::body("Coverage");
    if record.coverage.providers.is_empty() {
        Printer::detail("No applicable evidence provider completed.");
    } else {
        for provider in &record.coverage.providers {
            Printer::detail(&format!("- {}", provider_label(*provider)));
        }
    }
    for limitation in &record.coverage.limitations {
        Printer::detail(&format!(
            "- {} limitation: {}",
            provider_label(limitation.provider),
            limitation.message
        ));
    }

    render_health(inventory_issues, ctx.manifest_health);
    println!();
    Printer::body("Try:");
    for suggestion in record.suggestions() {
        Printer::detail(&suggestion);
    }
}

fn render_record_details(record: &UsageRecord) {
    render_artifacts(&record.artifacts);
    for evidence in &record.evidence {
        Printer::detail(&format!("- {}", evidence.summary));
    }
    for limitation in &record.coverage.limitations {
        Printer::detail(&format!(
            "- {} limitation: {}",
            provider_label(limitation.provider),
            limitation.message
        ));
    }
    for site in &record.declarations {
        Printer::detail(&format!("- declared at {}", site.location));
    }
}

fn render_health(inventory_issues: &[InventoryIssue], health: &ManifestHealth) {
    match health {
        ManifestHealth::Missing => {
            Printer::detail("Manifest health: missing; run `nx init`.");
        }
        ManifestHealth::Invalid { error } => {
            Printer::detail(&format!("Manifest health: invalid ({error})."));
        }
        ManifestHealth::Drifted { report, .. } => {
            Printer::detail(&format!(
                "Manifest health: drifted ({} issue(s)); run `nx init --refresh`.",
                report.issues.len()
            ));
        }
        ManifestHealth::InSync { .. } => {}
    }
    if !inventory_issues.is_empty() {
        Printer::detail(&format!(
            "Inventory uncertainty: {} expression(s) could not be resolved statically.",
            inventory_issues.len()
        ));
    }
}

fn render_json(
    records: &[UsageRecord],
    hidden_protected: usize,
    inventory_issues: &[InventoryIssue],
    args: &UsageArgs,
    since_seconds: u64,
    ctx: &QueryContext<'_>,
) -> i32 {
    let output = UsageJsonOutput {
        since: &args.since,
        since_seconds,
        manifest_health: manifest_health_json(ctx.manifest_health),
        inventory_issues,
        hidden_protected,
        summary: UsageSummary::from_records(records),
        records: records
            .iter()
            .map(|record| {
                UsageJsonRecord::new(
                    record,
                    if args.package.is_some() {
                        usize::MAX
                    } else {
                        ARTIFACT_EXAMPLE_LIMIT
                    },
                )
            })
            .collect(),
    };
    match serde_json::to_string_pretty(&output) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(err) => {
            ctx.printer
                .error(&format!("usage json rendering failed: {err}"));
            1
        }
    }
}

#[derive(Serialize)]
struct UsageJsonOutput<'a> {
    since: &'a str,
    since_seconds: u64,
    manifest_health: serde_json::Value,
    inventory_issues: &'a [InventoryIssue],
    hidden_protected: usize,
    summary: UsageSummary,
    records: Vec<UsageJsonRecord<'a>>,
}

#[derive(Serialize)]
struct UsageJsonRecord<'a> {
    #[serde(flatten)]
    record: &'a UsageRecord,
    explanation: &'a str,
    suggestions: Vec<String>,
    artifacts: ArtifactSummary,
}

impl<'a> UsageJsonRecord<'a> {
    fn new(record: &'a UsageRecord, artifact_limit: usize) -> Self {
        Self {
            record,
            explanation: record.explanation(),
            suggestions: record.suggestions(),
            artifacts: ArtifactSummary::new(&record.artifacts, artifact_limit),
        }
    }
}

#[derive(Serialize)]
struct ArtifactSummary {
    commands: ArtifactGroup,
    applications: ArtifactGroup,
}

impl ArtifactSummary {
    fn new(artifacts: &[crate::domain::usage::UsageArtifact], limit: usize) -> Self {
        let mut commands = Vec::new();
        let mut applications = Vec::new();
        let mut command_count = 0;
        let mut application_count = 0;
        for artifact in artifacts {
            match artifact {
                crate::domain::usage::UsageArtifact::Command { name, attribution } => {
                    command_count += 1;
                    if commands.len() < limit {
                        commands.push(ArtifactExample {
                            value: name.clone(),
                            attribution: *attribution,
                        });
                    }
                }
                crate::domain::usage::UsageArtifact::Application { path, attribution } => {
                    application_count += 1;
                    if applications.len() < limit {
                        applications.push(ArtifactExample {
                            value: path.clone(),
                            attribution: *attribution,
                        });
                    }
                }
            }
        }
        Self {
            commands: ArtifactGroup::new(command_count, commands),
            applications: ArtifactGroup::new(application_count, applications),
        }
    }
}

#[derive(Serialize)]
struct ArtifactGroup {
    count: usize,
    truncated: bool,
    examples: Vec<ArtifactExample>,
}

#[derive(Serialize)]
struct ArtifactExample {
    value: String,
    attribution: crate::domain::usage::ArtifactAttribution,
}

impl ArtifactGroup {
    fn new(count: usize, examples: Vec<ArtifactExample>) -> Self {
        Self {
            count,
            truncated: count > examples.len(),
            examples,
        }
    }
}

#[derive(Debug, Default, Serialize)]
struct UsageSummary {
    observed_recent: usize,
    observed_undated: usize,
    observed_stale: usize,
    no_evidence: usize,
    insufficient_evidence: usize,
    not_auditable: usize,
    inventory_uncertain: usize,
    protected: usize,
    candidates: usize,
}

impl UsageSummary {
    fn from_records(records: &[UsageRecord]) -> Self {
        let mut counts = BTreeMap::new();
        for record in records {
            *counts.entry(verdict_label(record.verdict)).or_insert(0) += 1;
        }
        Self {
            observed_recent: count(&counts, UsageVerdict::ObservedRecent),
            observed_undated: count(&counts, UsageVerdict::ObservedUndated),
            observed_stale: count(&counts, UsageVerdict::ObservedStale),
            no_evidence: count(&counts, UsageVerdict::NoEvidence),
            insufficient_evidence: count(&counts, UsageVerdict::InsufficientEvidence),
            not_auditable: count(&counts, UsageVerdict::NotAuditable),
            inventory_uncertain: count(&counts, UsageVerdict::InventoryUncertain),
            protected: count(&counts, UsageVerdict::Protected),
            candidates: records
                .iter()
                .filter(|record| record.is_candidate())
                .count(),
        }
    }
}

fn count(counts: &BTreeMap<&'static str, usize>, verdict: UsageVerdict) -> usize {
    counts.get(verdict_label(verdict)).copied().unwrap_or(0)
}

fn candidate_records(records: &[UsageRecord]) -> Vec<&UsageRecord> {
    let mut candidates = records
        .iter()
        .filter(|record| record.is_candidate())
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        (left.last_seen.unwrap_or(i64::MIN), left.name.as_str())
            .cmp(&(right.last_seen.unwrap_or(i64::MIN), right.name.as_str()))
    });
    candidates
}

const fn verdict_label(verdict: UsageVerdict) -> &'static str {
    match verdict {
        UsageVerdict::ObservedRecent => "observed-recent",
        UsageVerdict::ObservedUndated => "observed-undated",
        UsageVerdict::ObservedStale => "observed-stale",
        UsageVerdict::NoEvidence => "no-evidence",
        UsageVerdict::InsufficientEvidence => "insufficient-evidence",
        UsageVerdict::NotAuditable => "not-auditable",
        UsageVerdict::InventoryUncertain => "inventory-uncertain",
        UsageVerdict::Protected => "protected",
    }
}

fn last_observed(record: &UsageRecord) -> String {
    record
        .last_seen
        .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0))
        .map_or_else(
            || {
                if record.confidence > EvidenceConfidence::None {
                    "undated".to_string()
                } else {
                    "none".to_string()
                }
            },
            |timestamp| timestamp.format("%Y-%m-%d").to_string(),
        )
}

fn artifact_label(artifact: &crate::domain::usage::UsageArtifact) -> String {
    match artifact {
        crate::domain::usage::UsageArtifact::Command { name, attribution } => {
            format!("command `{name}` ({})", attribution_label(*attribution))
        }
        crate::domain::usage::UsageArtifact::Application { path, attribution } => {
            format!("application `{path}` ({})", attribution_label(*attribution))
        }
    }
}

const fn attribution_label(attribution: crate::domain::usage::ArtifactAttribution) -> &'static str {
    match attribution {
        crate::domain::usage::ArtifactAttribution::InstalledOwner => "installed owner",
        crate::domain::usage::ArtifactAttribution::StoreNameHeuristic => "Nix store-name inference",
        crate::domain::usage::ArtifactAttribution::PackageMetadata => "package metadata",
        crate::domain::usage::ArtifactAttribution::ExpectedName => "expected name",
    }
}

const fn declaration_kind_label(kind: &crate::domain::package::DeclarationKind) -> &'static str {
    match kind {
        crate::domain::package::DeclarationKind::Package => "package",
        crate::domain::package::DeclarationKind::ExternalInput => "external input",
        crate::domain::package::DeclarationKind::GeneratedCommand => "generated command",
        crate::domain::package::DeclarationKind::RuntimeEnvironment => "runtime environment",
        crate::domain::package::DeclarationKind::RuntimeMember { .. } => "runtime member",
        crate::domain::package::DeclarationKind::Application => "application",
        crate::domain::package::DeclarationKind::Service => "service",
    }
}

fn render_artifacts(artifacts: &[crate::domain::usage::UsageArtifact]) {
    for artifact in artifacts.iter().take(HUMAN_ARTIFACT_LIMIT) {
        Printer::detail(&format!("- {}", artifact_label(artifact)));
    }
    if artifacts.len() > HUMAN_ARTIFACT_LIMIT {
        Printer::detail(&format!(
            "- ... and {} more artifacts",
            artifacts.len() - HUMAN_ARTIFACT_LIMIT
        ));
    }
}

const fn provider_label(provider: crate::domain::usage::EvidenceProvider) -> &'static str {
    match provider {
        crate::domain::usage::EvidenceProvider::ArtifactDiscovery => "artifact discovery",
        crate::domain::usage::EvidenceProvider::HomebrewMetadata => "Homebrew metadata",
        crate::domain::usage::EvidenceProvider::TimestampedShellHistory => {
            "timestamped shell history"
        }
        crate::domain::usage::EvidenceProvider::UntimestampedShellHistory => {
            "untimestamped shell history"
        }
        crate::domain::usage::EvidenceProvider::Spotlight => "Spotlight application metadata",
        crate::domain::usage::EvidenceProvider::ProcessSnapshot => "current process snapshot",
    }
}

const fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
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

fn manifest_health_json(health: &ManifestHealth) -> serde_json::Value {
    match health {
        ManifestHealth::Missing => serde_json::json!({"status": "missing"}),
        ManifestHealth::Invalid { error } => {
            serde_json::json!({"status": "invalid", "error": error})
        }
        ManifestHealth::InSync { .. } => serde_json::json!({"status": "in_sync"}),
        ManifestHealth::Drifted { report, .. } => {
            let issues = report.issues.iter().map(format_issue).collect::<Vec<_>>();
            serde_json::json!({"status": "drifted", "issues": issues})
        }
    }
}

fn history_paths(args: &UsageArgs) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for path in &args.history {
        push_history_path(&mut paths, &mut seen, expand_home(path.clone()));
    }
    if let Some(path) = env::var_os("HISTFILE").map(PathBuf::from) {
        push_history_path(&mut paths, &mut seen, expand_home(path));
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        for relative in [
            ".zsh_history",
            ".local/state/zsh/history",
            ".bash_history",
            ".config/fish/fish_history",
        ] {
            push_history_path(&mut paths, &mut seen, home.join(relative));
        }
    }
    paths
}

fn push_history_path(paths: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, path: PathBuf) {
    if seen.insert(path.clone()) {
        paths.push(path);
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

#[derive(Default)]
struct LoadedShellHistory {
    entries: Vec<ShellHistoryEntry>,
    limitations: Vec<String>,
}

fn load_shell_history(paths: &[PathBuf], printer: Option<&Printer>) -> LoadedShellHistory {
    let mut history = LoadedShellHistory::default();
    for path in paths {
        match fs::read(path) {
            Ok(bytes) => history
                .entries
                .extend(parse_shell_history(&String::from_utf8_lossy(&bytes))),
            Err(error) if error.kind() != ErrorKind::NotFound => {
                history.limitations.push(format!(
                    "could not read history file {}: {error}",
                    path.display()
                ));
                if let Some(printer) = printer {
                    printer.warn(&format!(
                        "Could not read history file {}: {error}",
                        path.display()
                    ));
                }
            }
            Err(_) => {}
        }
    }
    history
}

fn current_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::{SourceFilter, UsageSummary, candidate_records};
    use crate::domain::package::PackageSource;
    use crate::domain::usage::{
        ArtifactAttribution, EvidenceConfidence, EvidenceCoverage, EvidenceProvider,
        ReviewCandidate, UsageArtifact, UsageVerdict,
    };

    #[test]
    fn source_filter_accepts_user_aliases() {
        assert!(
            SourceFilter::parse("brew")
                .is_some_and(|filter| filter.matches(PackageSource::Homebrew))
        );
        assert!(SourceFilter::parse("wat").is_none());
    }

    #[test]
    fn summary_and_candidates_use_only_actionable_verdicts() {
        let records = [
            record("recent", UsageVerdict::ObservedRecent),
            record("uncertain", UsageVerdict::InventoryUncertain),
            record("stale", UsageVerdict::ObservedStale),
            record("none", UsageVerdict::NoEvidence),
        ];
        let summary = UsageSummary::from_records(&records);
        let candidates = candidate_records(&records);

        assert_eq!(summary.candidates, 1);
        assert_eq!(
            candidates
                .iter()
                .map(|record| record.name.as_str())
                .collect::<Vec<_>>(),
            vec!["stale"]
        );
    }

    fn record(name: &str, verdict: UsageVerdict) -> crate::domain::usage::UsageRecord {
        let mut coverage = EvidenceCoverage::default();
        coverage.add_provider(EvidenceProvider::TimestampedShellHistory);
        crate::domain::usage::UsageRecord {
            name: name.to_string(),
            source: PackageSource::Nix,
            declarations: vec![crate::domain::package::DeclarationSite {
                location: "packages.nix:1".to_string(),
                kind: crate::domain::package::DeclarationKind::Package,
            }],
            verdict,
            last_seen: None,
            confidence: if verdict == UsageVerdict::ObservedStale {
                EvidenceConfidence::Strong
            } else {
                EvidenceConfidence::None
            },
            candidate: (verdict == UsageVerdict::ObservedStale).then_some(ReviewCandidate {
                observed_at: 0,
                evidence: crate::domain::usage::EvidenceKind::ShellHistory,
            }),
            artifacts: vec![UsageArtifact::Command {
                name: name.to_string(),
                attribution: ArtifactAttribution::InstalledOwner,
            }],
            coverage,
            evidence: Vec::new(),
            detail: None,
        }
    }
}
