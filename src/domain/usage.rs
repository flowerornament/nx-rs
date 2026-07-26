use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::domain::package::{PackageDeclaration, PackageSource};

const DEFAULT_USAGE_ALIAS_CATALOG: &[(&str, &str)] = &[
    ("agda", "agdaWithoutMailutils"),
    ("awk", "gawk"),
    ("aws", "awscli2"),
    ("aws", "awscli"),
    ("bd", "steveyegge/beads/bd"),
    ("bq", "google-cloud-sdk"),
    ("btm", "bottom"),
    ("cabal", "cabal-install"),
    ("claude", "claude-code"),
    ("cmp", "diffutils"),
    ("codex", "codex-cli"),
    ("compare", "imagemagick"),
    ("composite", "imagemagick"),
    ("convert", "imagemagick"),
    ("createdb", "postgresql"),
    ("createuser", "postgresql"),
    ("diff", "diffutils"),
    ("diff3", "diffutils"),
    ("difft", "difftastic"),
    ("display", "imagemagick"),
    ("docker", "docker-client"),
    ("dropdb", "postgresql"),
    ("dropuser", "postgresql"),
    ("egrep", "gnugrep"),
    ("elixir", "beam.packages.erlang_28.elixir_1_20"),
    ("emcc", "emscripten"),
    ("ffplay", "ffmpeg"),
    ("ffprobe", "ffmpeg"),
    ("fgrep", "gnugrep"),
    ("find", "findutils"),
    ("gcloud", "google-cloud-sdk"),
    ("grep", "gnugrep"),
    ("gpg", "gnupg"),
    ("gpg-agent", "gnupg"),
    ("gpg2", "gnupg"),
    ("gs", "ghostscript"),
    ("gsutil", "google-cloud-sdk"),
    ("helm", "kubernetes-helm"),
    ("http", "httpie"),
    ("https", "httpie"),
    ("identify", "imagemagick"),
    ("import", "imagemagick"),
    ("magick", "imagemagick"),
    ("make", "gnumake"),
    ("mogrify", "imagemagick"),
    ("montage", "imagemagick"),
    ("node", "nodejs"),
    ("npm", "nodejs"),
    ("npx", "nodejs"),
    ("nvim", "neovim"),
    ("pdfinfo", "poppler-utils"),
    ("pdfseparate", "poppler-utils"),
    ("pdftocairo", "poppler-utils"),
    ("pdftoppm", "poppler-utils"),
    ("pdftotext", "poppler-utils"),
    ("pdfunite", "poppler-utils"),
    ("pg_dump", "postgresql"),
    ("pg_isready", "postgresql"),
    ("pg_restore", "postgresql"),
    ("ps2pdf", "ghostscript"),
    ("psql", "postgresql"),
    ("python", "python3"),
    ("python3", "python3"),
    ("rga", "ripgrep-all"),
    ("rg", "ripgrep"),
    ("scp", "openssh"),
    ("sdiff", "diffutils"),
    ("sed", "gnused"),
    ("sftp", "openssh"),
    ("sqlite3", "sqlite"),
    ("ssh", "openssh"),
    ("ssh-add", "openssh"),
    ("ssh-agent", "openssh"),
    ("ssh-keygen", "openssh"),
    ("tar", "gnutar"),
    ("tldr", "tealdeer"),
    ("tofu", "opentofu"),
    ("vercel", "vercel-cli"),
    ("wg", "wireguard-tools"),
    ("wg-quick", "wireguard-tools"),
    ("xdg-mime", "xdg-utils"),
    ("xdg-open", "xdg-utils"),
    ("xdg-settings", "xdg-utils"),
    ("xargs", "findutils"),
    ("yq", "yq-go"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceConfidence {
    None,
    Medium,
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceKind {
    ShellHistory,
    Spotlight,
    Process,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceItem {
    pub kind: EvidenceKind,
    pub summary: String,
    pub timestamp: Option<i64>,
    pub confidence: EvidenceConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UsageArtifact {
    Command {
        name: String,
        attribution: ArtifactAttribution,
    },
    Application {
        path: String,
        attribution: ArtifactAttribution,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactAttribution {
    InstalledOwner,
    StoreNameHeuristic,
    PackageMetadata,
    ExpectedName,
}

impl ArtifactAttribution {
    #[must_use]
    pub const fn confidence(self) -> EvidenceConfidence {
        match self {
            Self::InstalledOwner | Self::PackageMetadata => EvidenceConfidence::Strong,
            Self::StoreNameHeuristic | Self::ExpectedName => EvidenceConfidence::Medium,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditablePlan {
    artifacts: Vec<UsageArtifact>,
}

impl AuditablePlan {
    #[must_use]
    pub fn new(artifacts: Vec<UsageArtifact>) -> Option<Self> {
        (!artifacts.is_empty()).then_some(Self { artifacts })
    }

    #[must_use]
    pub fn artifacts(&self) -> &[UsageArtifact] {
        &self.artifacts
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditPlan {
    Auditable(AuditablePlan),
    NotAuditable { reason: String },
    InventoryUncertain { reason: String },
    Protected { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceProvider {
    ArtifactDiscovery,
    HomebrewMetadata,
    TimestampedShellHistory,
    UntimestampedShellHistory,
    Spotlight,
    ProcessSnapshot,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct EvidenceCoverage {
    pub providers: Vec<EvidenceProvider>,
    pub limitations: Vec<EvidenceLimitation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceLimitation {
    pub provider: EvidenceProvider,
    pub message: String,
}

impl EvidenceCoverage {
    pub fn add_provider(&mut self, provider: EvidenceProvider) {
        if !self.providers.contains(&provider) {
            self.providers.push(provider);
        }
    }

    pub fn add_limitation(&mut self, provider: EvidenceProvider, message: impl Into<String>) {
        let limitation = EvidenceLimitation {
            provider,
            message: message.into(),
        };
        if !self.limitations.contains(&limitation) {
            self.limitations.push(limitation);
        }
    }

    #[must_use]
    fn covers(&self, artifacts: &[UsageArtifact]) -> bool {
        artifacts.iter().all(|artifact| {
            let (usage_provider, attribution) = match artifact {
                UsageArtifact::Command { attribution, .. } => {
                    (EvidenceProvider::TimestampedShellHistory, attribution)
                }
                UsageArtifact::Application { attribution, .. } => {
                    (EvidenceProvider::Spotlight, attribution)
                }
            };
            let attribution_covered = match attribution {
                ArtifactAttribution::InstalledOwner | ArtifactAttribution::StoreNameHeuristic => {
                    self.providers
                        .contains(&EvidenceProvider::ArtifactDiscovery)
                }
                ArtifactAttribution::PackageMetadata => {
                    self.providers.contains(&EvidenceProvider::HomebrewMetadata)
                }
                ArtifactAttribution::ExpectedName => true,
            };
            self.providers.contains(&usage_provider) && attribution_covered
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageVerdict {
    ObservedRecent,
    ObservedUndated,
    ObservedStale,
    NoEvidence,
    InsufficientEvidence,
    NotAuditable,
    InventoryUncertain,
    Protected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReviewCandidate {
    pub observed_at: i64,
    pub evidence: EvidenceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageRecord {
    pub name: String,
    pub source: PackageSource,
    pub declarations: Vec<crate::domain::package::DeclarationSite>,
    pub verdict: UsageVerdict,
    pub last_seen: Option<i64>,
    pub confidence: EvidenceConfidence,
    pub candidate: Option<ReviewCandidate>,
    #[serde(skip_serializing)]
    pub artifacts: Vec<UsageArtifact>,
    pub coverage: EvidenceCoverage,
    pub evidence: Vec<EvidenceItem>,
    #[serde(skip_serializing)]
    pub(crate) detail: Option<String>,
}

impl UsageRecord {
    #[must_use]
    pub const fn is_candidate(&self) -> bool {
        self.candidate.is_some()
    }

    #[must_use]
    pub fn explanation(&self) -> &str {
        self.detail
            .as_deref()
            .unwrap_or_else(|| verdict_explanation(self.verdict))
    }

    #[must_use]
    pub fn suggestions(&self) -> Vec<String> {
        let mut suggestions = vec![format!("nx where {}", self.name)];
        if self.is_candidate() {
            suggestions.push(format!("nx remove --dry-run {}", self.name));
        }
        suggestions
    }
}

pub fn classify_usage(
    declaration: &PackageDeclaration,
    plan: AuditPlan,
    mut evidence: Vec<EvidenceItem>,
    coverage: EvidenceCoverage,
    cutoff_epoch_secs: i64,
) -> UsageRecord {
    deduplicate_evidence(&mut evidence);
    let last_seen = evidence.iter().filter_map(|item| item.timestamp).max();
    let (verdict, artifacts, detail) = match plan {
        AuditPlan::Protected { reason } => (UsageVerdict::Protected, Vec::new(), Some(reason)),
        AuditPlan::NotAuditable { reason } => {
            (UsageVerdict::NotAuditable, Vec::new(), Some(reason))
        }
        AuditPlan::InventoryUncertain { reason } => {
            (UsageVerdict::InventoryUncertain, Vec::new(), Some(reason))
        }
        AuditPlan::Auditable(plan) => {
            let verdict = if last_seen.is_some_and(|timestamp| timestamp >= cutoff_epoch_secs) {
                UsageVerdict::ObservedRecent
            } else if !evidence.is_empty() {
                if evidence.iter().any(|item| item.timestamp.is_none()) {
                    UsageVerdict::ObservedUndated
                } else {
                    UsageVerdict::ObservedStale
                }
            } else if coverage.covers(plan.artifacts()) {
                UsageVerdict::NoEvidence
            } else {
                UsageVerdict::InsufficientEvidence
            };
            (verdict, plan.artifacts, None)
        }
    };
    let latest_evidence = last_seen.and_then(|last_seen| {
        evidence
            .iter()
            .filter(|item| item.timestamp == Some(last_seen))
            .max_by_key(|item| item.confidence)
    });
    let confidence = latest_evidence.map_or_else(
        || {
            evidence
                .iter()
                .filter(|item| item.timestamp.is_none())
                .map(|item| item.confidence)
                .max()
                .unwrap_or(EvidenceConfidence::None)
        },
        |item| item.confidence,
    );
    let candidate = (verdict == UsageVerdict::ObservedStale && coverage.covers(&artifacts))
        .then_some(latest_evidence)
        .flatten()
        .filter(|item| item.confidence == EvidenceConfidence::Strong)
        .and_then(|item| {
            item.timestamp.map(|observed_at| ReviewCandidate {
                observed_at,
                evidence: item.kind,
            })
        });

    UsageRecord {
        name: declaration.name.clone(),
        source: declaration.source,
        declarations: declaration.sites.clone(),
        verdict,
        last_seen,
        confidence,
        candidate,
        artifacts,
        coverage,
        evidence,
        detail,
    }
}

fn verdict_explanation(verdict: UsageVerdict) -> &'static str {
    match verdict {
        UsageVerdict::ObservedRecent => "recent use was observed",
        UsageVerdict::ObservedUndated => "use was observed without reliable recency",
        UsageVerdict::ObservedStale => "the latest observation is outside the selected window",
        UsageVerdict::NoEvidence => {
            "no use was observed; absence from local evidence is not proof of inactivity"
        }
        UsageVerdict::InsufficientEvidence => {
            "available evidence cannot support an inactivity judgment"
        }
        UsageVerdict::NotAuditable => "this package has no supported usage artifact",
        UsageVerdict::InventoryUncertain => "package artifact ownership could not be established",
        UsageVerdict::Protected => "package is protected by policy",
    }
}

fn deduplicate_evidence(evidence: &mut Vec<EvidenceItem>) {
    let mut seen = HashSet::new();
    evidence.retain(|item| {
        seen.insert((
            item.kind,
            item.summary.clone(),
            item.timestamp,
            item.confidence,
        ))
    });
    evidence.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.summary.cmp(&right.summary))
    });
}

#[must_use]
pub fn protection_reason(package: &PackageDeclaration) -> Option<&'static str> {
    if package.source == PackageSource::Service {
        return Some("active service declarations are protected from usage pruning");
    }

    [
        "bash",
        "darwin-rebuild",
        "fish",
        "git",
        "home-manager",
        "neovim",
        "nix",
        "nx",
        "vim",
        "zsh",
    ]
    .contains(&package.name.as_str())
    .then_some("core shell, editor, package manager, or nx workflow tool")
}

pub fn parse_since_seconds(value: &str) -> Option<u64> {
    let (amount, unit) = split_duration(value)?;
    let multiplier = match unit {
        "d" => 86_400,
        "w" => 7 * 86_400,
        "mo" => 30 * 86_400,
        "y" => 365 * 86_400,
        _ => return None,
    };
    amount.checked_mul(multiplier)
}

pub fn default_usage_aliases_for_packages<'a>(
    packages: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, String> {
    let package_match_names = packages
        .into_iter()
        .flat_map(package_match_names)
        .map(str::to_ascii_lowercase)
        .collect::<HashSet<_>>();

    DEFAULT_USAGE_ALIAS_CATALOG
        .iter()
        .filter(|(_, package)| package_match_names.contains(*package))
        .map(|(alias, package)| ((*alias).to_string(), (*package).to_string()))
        .collect()
}

#[must_use]
pub fn aliases_for_package(
    package: &str,
    package_aliases: &HashMap<String, String>,
) -> Vec<String> {
    let mut aliases = package_match_names(package)
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();

    if let Some(cli_name) = aliases
        .last()
        .and_then(|name| name.strip_suffix("-cli"))
        .map(str::to_string)
    {
        push_unique(&mut aliases, cli_name);
    }

    for (alias, target) in DEFAULT_USAGE_ALIAS_CATALOG
        .iter()
        .map(|(alias, target)| (*alias, *target))
        .chain(
            package_aliases
                .iter()
                .map(|(alias, target)| (alias.as_str(), target.as_str())),
        )
    {
        if package_match_names(package)
            .iter()
            .any(|name| name.eq_ignore_ascii_case(target))
        {
            push_unique(&mut aliases, alias.to_string());
        }
    }

    aliases
}

fn package_match_names(package: &str) -> Vec<&str> {
    let slash_bare = package.rsplit('/').next().unwrap_or(package);
    let bare = slash_bare.rsplit('.').next().unwrap_or(slash_bare);
    let mut names = Vec::new();
    for name in [package, slash_bare, bare] {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn split_duration(value: &str) -> Option<(u64, &str)> {
    let digit_len = value
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_digit())
        .map(|(index, ch)| index + ch.len_utf8())
        .last()?;
    let amount = value[..digit_len].parse().ok()?;
    let unit = &value[digit_len..];
    (!unit.is_empty()).then_some((amount, unit))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        ArtifactAttribution, AuditPlan, AuditablePlan, EvidenceConfidence, EvidenceCoverage,
        EvidenceItem, EvidenceKind, EvidenceProvider, UsageArtifact, UsageVerdict,
        aliases_for_package, classify_usage, parse_since_seconds,
    };
    use crate::domain::package::{
        DeclarationKind, DeclarationSite, PackageDeclaration, PackageSource,
    };

    #[test]
    fn parses_supported_usage_windows() {
        assert_eq!(parse_since_seconds("30d"), Some(30 * 86_400));
        assert_eq!(parse_since_seconds("12w"), Some(12 * 7 * 86_400));
        assert_eq!(parse_since_seconds("6mo"), Some(6 * 30 * 86_400));
        assert_eq!(parse_since_seconds("1y"), Some(365 * 86_400));
        assert_eq!(parse_since_seconds("2h"), None);
    }

    #[test]
    fn no_evidence_is_visible_but_never_actionable() {
        let mut coverage = EvidenceCoverage::default();
        coverage.add_provider(EvidenceProvider::ArtifactDiscovery);
        coverage.add_provider(EvidenceProvider::TimestampedShellHistory);
        let plan = AuditPlan::Auditable(
            AuditablePlan::new(vec![UsageArtifact::Command {
                name: "jq".to_string(),
                attribution: ArtifactAttribution::InstalledOwner,
            }])
            .expect("artifact should be auditable"),
        );

        let record = classify_usage(&package("jq"), plan, Vec::new(), coverage, 100);
        assert_eq!(record.verdict, UsageVerdict::NoEvidence);
        assert!(!record.is_candidate());
    }

    #[test]
    fn missing_required_provider_is_insufficient_not_unused() {
        let plan = AuditPlan::Auditable(
            AuditablePlan::new(vec![UsageArtifact::Application {
                path: "/Applications/Ghostty.app".to_string(),
                attribution: ArtifactAttribution::PackageMetadata,
            }])
            .expect("artifact should be auditable"),
        );

        let record = classify_usage(
            &package("ghostty"),
            plan,
            Vec::new(),
            EvidenceCoverage::default(),
            100,
        );
        assert_eq!(record.verdict, UsageVerdict::InsufficientEvidence);
    }

    #[test]
    fn undated_observation_never_becomes_a_candidate() {
        let plan = AuditPlan::Auditable(
            AuditablePlan::new(vec![UsageArtifact::Command {
                name: "nixd".to_string(),
                attribution: ArtifactAttribution::InstalledOwner,
            }])
            .expect("artifact should be auditable"),
        );
        let evidence = vec![EvidenceItem {
            kind: EvidenceKind::ShellHistory,
            summary: "undated shell observation".to_string(),
            timestamp: None,
            confidence: EvidenceConfidence::Medium,
        }];

        let record = classify_usage(
            &package("nixd"),
            plan,
            evidence,
            EvidenceCoverage::default(),
            100,
        );
        assert_eq!(record.verdict, UsageVerdict::ObservedUndated);
        assert!(!record.is_candidate());
    }

    #[test]
    fn undated_evidence_overrides_an_old_timestamp() {
        let mut coverage = EvidenceCoverage::default();
        coverage.add_provider(EvidenceProvider::TimestampedShellHistory);
        let plan = AuditPlan::Auditable(
            AuditablePlan::new(vec![UsageArtifact::Command {
                name: "nixd".to_string(),
                attribution: ArtifactAttribution::InstalledOwner,
            }])
            .expect("artifact should be auditable"),
        );
        let evidence = vec![
            EvidenceItem {
                kind: EvidenceKind::ShellHistory,
                summary: "old shell observation".to_string(),
                timestamp: Some(50),
                confidence: EvidenceConfidence::Strong,
            },
            EvidenceItem {
                kind: EvidenceKind::ShellHistory,
                summary: "undated shell observation".to_string(),
                timestamp: None,
                confidence: EvidenceConfidence::Medium,
            },
        ];

        let record = classify_usage(&package("nixd"), plan, evidence, coverage, 100);

        assert_eq!(record.verdict, UsageVerdict::ObservedUndated);
        assert!(!record.is_candidate());
        assert_eq!(record.suggestions(), vec!["nx where nixd"]);
    }

    #[test]
    fn expected_command_names_cannot_create_review_candidates() {
        let mut coverage = EvidenceCoverage::default();
        coverage.add_provider(EvidenceProvider::TimestampedShellHistory);
        let plan = AuditPlan::Auditable(
            AuditablePlan::new(vec![UsageArtifact::Command {
                name: "tool".to_string(),
                attribution: ArtifactAttribution::ExpectedName,
            }])
            .expect("artifact should be auditable"),
        );
        let evidence = vec![EvidenceItem {
            kind: EvidenceKind::ShellHistory,
            summary: "expected command appeared".to_string(),
            timestamp: Some(50),
            confidence: EvidenceConfidence::Medium,
        }];

        let record = classify_usage(&package("tool"), plan, evidence, coverage, 100);

        assert_eq!(record.verdict, UsageVerdict::ObservedStale);
        assert!(!record.is_candidate());
        assert_eq!(record.suggestions(), vec!["nx where tool"]);
    }

    #[test]
    fn newer_medium_evidence_cannot_borrow_strength_from_older_evidence() {
        let mut coverage = EvidenceCoverage::default();
        coverage.add_provider(EvidenceProvider::ArtifactDiscovery);
        coverage.add_provider(EvidenceProvider::TimestampedShellHistory);
        let plan = AuditPlan::Auditable(
            AuditablePlan::new(vec![UsageArtifact::Command {
                name: "tool".to_string(),
                attribution: ArtifactAttribution::InstalledOwner,
            }])
            .expect("artifact should be auditable"),
        );
        let evidence = vec![
            EvidenceItem {
                kind: EvidenceKind::ShellHistory,
                summary: "strong old observation".to_string(),
                timestamp: Some(25),
                confidence: EvidenceConfidence::Strong,
            },
            EvidenceItem {
                kind: EvidenceKind::ShellHistory,
                summary: "medium newer observation".to_string(),
                timestamp: Some(50),
                confidence: EvidenceConfidence::Medium,
            },
        ];

        let record = classify_usage(&package("tool"), plan, evidence, coverage, 100);

        assert_eq!(record.verdict, UsageVerdict::ObservedStale);
        assert_eq!(record.last_seen, Some(50));
        assert!(!record.is_candidate());
    }

    #[test]
    fn candidate_requires_usage_and_artifact_coverage() {
        let plan = AuditPlan::Auditable(
            AuditablePlan::new(vec![UsageArtifact::Command {
                name: "tool".to_string(),
                attribution: ArtifactAttribution::InstalledOwner,
            }])
            .expect("artifact should be auditable"),
        );
        let evidence = vec![EvidenceItem {
            kind: EvidenceKind::ShellHistory,
            summary: "strong stale observation".to_string(),
            timestamp: Some(50),
            confidence: EvidenceConfidence::Strong,
        }];
        let mut incomplete = EvidenceCoverage::default();
        incomplete.add_provider(EvidenceProvider::TimestampedShellHistory);

        let record = classify_usage(
            &package("tool"),
            plan.clone(),
            evidence.clone(),
            incomplete,
            100,
        );
        assert!(!record.is_candidate());

        let mut complete = EvidenceCoverage::default();
        complete.add_provider(EvidenceProvider::ArtifactDiscovery);
        complete.add_provider(EvidenceProvider::TimestampedShellHistory);
        let record = classify_usage(&package("tool"), plan, evidence, complete, 100);
        assert!(record.is_candidate());
    }

    #[test]
    fn aliases_combine_catalog_and_manifest_hints() {
        let aliases = HashMap::from([("node24".to_string(), "nodejs".to_string())]);
        let names = aliases_for_package("nodejs", &aliases);
        assert!(names.contains(&"node".to_string()));
        assert!(names.contains(&"node24".to_string()));
    }

    fn package(name: &str) -> PackageDeclaration {
        PackageDeclaration {
            name: name.to_string(),
            source: PackageSource::Nix,
            sites: vec![DeclarationSite {
                location: "packages.nix:1".to_string(),
                kind: DeclarationKind::Package,
            }],
        }
    }
}
