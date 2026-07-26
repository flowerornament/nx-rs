use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde_json::Value;
use tree_sitter::{Node, Parser};

use crate::domain::package::{PackageDeclaration, PackageSource};
use crate::domain::usage::{
    ArtifactAttribution, AuditPlan, AuditablePlan, EvidenceConfidence, EvidenceCoverage,
    EvidenceItem, EvidenceKind, EvidenceLimitation, EvidenceProvider, UsageArtifact,
    aliases_for_package, protection_reason,
};
use crate::infra::shell::run_captured_command;
use crate::infra::shell_history::ShellHistoryEntry;

pub struct UsageEvidence {
    artifacts: ArtifactCatalog,
    history: HistoryIndex,
    processes: ProcessIndex,
    package_aliases: HashMap<String, String>,
}

pub struct PackageObservation {
    pub plan: AuditPlan,
    pub evidence: Vec<EvidenceItem>,
    pub coverage: EvidenceCoverage,
}

impl UsageEvidence {
    #[must_use]
    pub fn collect(
        shell_history: &[ShellHistoryEntry],
        history_limitations: Vec<String>,
        history_enabled: bool,
        package_aliases: HashMap<String, String>,
        observed_at: i64,
    ) -> Self {
        Self {
            artifacts: ArtifactCatalog::discover(),
            history: HistoryIndex::new(shell_history, history_limitations, history_enabled),
            processes: ProcessIndex::collect(observed_at),
            package_aliases,
        }
    }

    #[must_use]
    pub fn observe(
        &self,
        package: &PackageDeclaration,
        cutoff_epoch_secs: i64,
    ) -> PackageObservation {
        let mut coverage = self.artifacts.coverage_for(package.source);
        let Some(plan) = self.plan_for(package) else {
            return PackageObservation {
                plan: AuditPlan::InventoryUncertain {
                    reason: "no installed artifact ownership matched this declaration".to_string(),
                },
                evidence: Vec::new(),
                coverage,
            };
        };

        let AuditPlan::Auditable(auditable) = &plan else {
            return PackageObservation {
                plan,
                evidence: Vec::new(),
                coverage,
            };
        };

        let mut evidence = Vec::new();
        for artifact in auditable.artifacts() {
            match artifact {
                UsageArtifact::Command { name, attribution } => {
                    self.history.observe(
                        name,
                        attribution.confidence(),
                        cutoff_epoch_secs,
                        &mut evidence,
                        &mut coverage,
                    );
                }
                UsageArtifact::Application { path, attribution } => {
                    observe_application(
                        path,
                        attribution.confidence(),
                        &mut evidence,
                        &mut coverage,
                    );
                    self.processes.observe_application(
                        path,
                        attribution.confidence(),
                        &mut evidence,
                        &mut coverage,
                    );
                }
            }
        }

        PackageObservation {
            plan,
            evidence,
            coverage,
        }
    }

    fn plan_for(&self, package: &PackageDeclaration) -> Option<AuditPlan> {
        if let Some(reason) = protection_reason(package) {
            return Some(AuditPlan::Protected {
                reason: reason.to_string(),
            });
        }

        if let Some(runtime) = package.runtime_member() {
            return Some(AuditPlan::NotAuditable {
                reason: format!(
                    "runtime member of {runtime}; shell invocation cannot establish library use"
                ),
            });
        }

        let aliases = aliases_for_package(&package.name, &self.package_aliases);
        let mut artifacts = match package.source {
            PackageSource::Nix => self.artifacts.nix_artifacts(&package.name, &aliases),
            PackageSource::Homebrew => self
                .artifacts
                .homebrew_formula_artifacts(&package.name, &aliases),
            PackageSource::Cask => self.artifacts.cask_artifacts(&package.name, &aliases),
            PackageSource::Mas => ArtifactCatalog::mas_artifacts(&package.name),
            PackageSource::Service => Vec::new(),
        };
        if artifacts.is_empty()
            && matches!(package.source, PackageSource::Nix | PackageSource::Homebrew)
        {
            artifacts.extend(aliases.into_iter().map(|name| UsageArtifact::Command {
                name,
                attribution: ArtifactAttribution::ExpectedName,
            }));
        }

        AuditablePlan::new(artifacts)
            .map(AuditPlan::Auditable)
            .or_else(|| self.non_auditable_plan(package))
    }

    fn non_auditable_plan(&self, package: &PackageDeclaration) -> Option<AuditPlan> {
        match package.source {
            PackageSource::Cask if self.artifacts.cask_known(&package.name) => {
                Some(AuditPlan::NotAuditable {
                    reason: "installed cask exposes no command or application artifact".to_string(),
                })
            }
            PackageSource::Mas => Some(AuditPlan::InventoryUncertain {
                reason: "installed application bundle could not be located".to_string(),
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ArtifactSource {
    Nix,
    Homebrew,
    Cask,
}

#[derive(Debug, Clone)]
struct OwnedCommand {
    name: String,
    source: ArtifactSource,
    owner: String,
}

#[derive(Debug, Clone, Default)]
struct CaskArtifacts {
    commands: Vec<String>,
    applications: Vec<String>,
}

#[derive(Default)]
struct ArtifactCatalog {
    commands: Vec<OwnedCommand>,
    casks: HashMap<String, CaskArtifacts>,
    command_discovery_complete: bool,
    homebrew_metadata_complete: bool,
    limitations: Vec<EvidenceLimitation>,
}

impl ArtifactCatalog {
    fn discover() -> Self {
        let mut catalog = Self::default();
        catalog.scan_command_roots();
        catalog.load_homebrew_metadata();
        catalog.commands.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.owner.cmp(&right.owner))
        });
        catalog
    }

    fn scan_command_roots(&mut self) {
        let mut seen = HashSet::new();
        let mut complete = true;
        let mut scanned_root = false;
        for root in command_roots() {
            let entries = match fs::read_dir(&root) {
                Ok(entries) => {
                    scanned_root = true;
                    entries
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    complete = false;
                    self.limitations.push(EvidenceLimitation {
                        provider: EvidenceProvider::ArtifactDiscovery,
                        message: format!("could not scan {}: {error}", root.display()),
                    });
                    continue;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        complete = false;
                        self.limitations.push(EvidenceLimitation {
                            provider: EvidenceProvider::ArtifactDiscovery,
                            message: format!("could not read an installed command entry: {error}"),
                        });
                        continue;
                    }
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                let target = match entry.path().canonicalize() {
                    Ok(target) => target,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        complete = false;
                        self.limitations.push(EvidenceLimitation {
                            provider: EvidenceProvider::ArtifactDiscovery,
                            message: format!(
                                "could not resolve installed command {}: {error}",
                                entry.path().display()
                            ),
                        });
                        continue;
                    }
                };
                let Some((source, owner)) = artifact_owner(&target) else {
                    continue;
                };
                if seen.insert((name.clone(), source, owner.clone())) {
                    self.commands.push(OwnedCommand {
                        name,
                        source,
                        owner,
                    });
                }
            }
        }
        self.command_discovery_complete = scanned_root && complete;
    }

    fn load_homebrew_metadata(&mut self) {
        let output = match run_captured_command("brew", &["info", "--json=v2", "--installed"], None)
        {
            Ok(output) if output.code == 0 => output,
            Ok(output) => {
                self.limitations.push(EvidenceLimitation {
                    provider: EvidenceProvider::HomebrewMetadata,
                    message: format!("`brew info` exited with status {}", output.code),
                });
                return;
            }
            Err(error) => {
                self.limitations.push(EvidenceLimitation {
                    provider: EvidenceProvider::HomebrewMetadata,
                    message: format!("could not run `brew info`: {error}"),
                });
                return;
            }
        };
        let value: Value = match serde_json::from_str(&output.stdout) {
            Ok(value) => value,
            Err(error) => {
                self.limitations.push(EvidenceLimitation {
                    provider: EvidenceProvider::HomebrewMetadata,
                    message: format!("could not parse `brew info` output: {error}"),
                });
                return;
            }
        };
        self.homebrew_metadata_complete = true;
        for cask in value
            .get("casks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(token) = cask.get("token").and_then(Value::as_str) else {
                continue;
            };
            let mut artifacts = CaskArtifacts::default();
            for artifact in cask
                .get("artifacts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                collect_cask_artifact(artifact, &mut artifacts);
            }
            self.casks.insert(token.to_string(), artifacts);
        }
    }

    fn coverage_for(&self, source: PackageSource) -> EvidenceCoverage {
        let mut coverage = EvidenceCoverage::default();
        if self.command_discovery_complete {
            coverage.add_provider(EvidenceProvider::ArtifactDiscovery);
        }
        if source == PackageSource::Cask && self.homebrew_metadata_complete {
            coverage.add_provider(EvidenceProvider::HomebrewMetadata);
        }
        for limitation in self.limitations.iter().filter(|limitation| {
            limitation.provider == EvidenceProvider::ArtifactDiscovery
                || (source == PackageSource::Cask
                    && limitation.provider == EvidenceProvider::HomebrewMetadata)
        }) {
            coverage.add_limitation(limitation.provider, &limitation.message);
        }
        coverage
    }

    fn nix_artifacts(&self, package: &str, aliases: &[String]) -> Vec<UsageArtifact> {
        self.command_artifacts(package, aliases, ArtifactSource::Nix)
    }

    fn homebrew_formula_artifacts(&self, package: &str, aliases: &[String]) -> Vec<UsageArtifact> {
        self.command_artifacts(package, aliases, ArtifactSource::Homebrew)
    }

    fn cask_artifacts(&self, package: &str, aliases: &[String]) -> Vec<UsageArtifact> {
        let bare = bare_package_name(package);
        let mut artifacts = self.command_artifacts(package, aliases, ArtifactSource::Cask);
        if let Some(cask) = self.casks.get(bare) {
            for command in &cask.commands {
                push_artifact(
                    &mut artifacts,
                    UsageArtifact::Command {
                        name: command.clone(),
                        attribution: ArtifactAttribution::PackageMetadata,
                    },
                );
            }
            for path in &cask.applications {
                push_artifact(
                    &mut artifacts,
                    UsageArtifact::Application {
                        path: path.clone(),
                        attribution: ArtifactAttribution::PackageMetadata,
                    },
                );
            }
        }
        artifacts
    }

    fn mas_artifacts(package: &str) -> Vec<UsageArtifact> {
        application_roots()
            .into_iter()
            .map(|root| root.join(format!("{package}.app")))
            .find(|path| path.exists())
            .map_or_else(Vec::new, |path| {
                vec![UsageArtifact::Application {
                    path: path.display().to_string(),
                    attribution: ArtifactAttribution::ExpectedName,
                }]
            })
    }

    fn command_artifacts(
        &self,
        package: &str,
        aliases: &[String],
        source: ArtifactSource,
    ) -> Vec<UsageArtifact> {
        let mut artifacts = Vec::new();
        for command in self.commands.iter().filter(|command| {
            command.source == source
                && (owner_matches(package, &command.owner, source)
                    || aliases
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(&command.name)))
        }) {
            let attribution = if owner_matches(package, &command.owner, source) {
                if source == ArtifactSource::Nix {
                    ArtifactAttribution::StoreNameHeuristic
                } else {
                    ArtifactAttribution::InstalledOwner
                }
            } else {
                ArtifactAttribution::ExpectedName
            };
            push_artifact(
                &mut artifacts,
                UsageArtifact::Command {
                    name: command.name.clone(),
                    attribution,
                },
            );
        }
        artifacts.sort();
        artifacts
    }

    fn cask_known(&self, package: &str) -> bool {
        self.casks.contains_key(bare_package_name(package))
    }
}

fn artifact_owner(path: &Path) -> Option<(ArtifactSource, String)> {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();

    if components
        .get(1)
        .is_some_and(|component| component == "nix")
        && components
            .get(2)
            .is_some_and(|component| component == "store")
    {
        let store_name = components.get(3)?;
        return Some((
            ArtifactSource::Nix,
            store_name
                .split_once('-')
                .map_or(store_name.as_ref(), |(_, name)| name)
                .to_string(),
        ));
    }
    for (marker, source) in [
        ("Cellar", ArtifactSource::Homebrew),
        ("Caskroom", ArtifactSource::Cask),
    ] {
        if let Some(index) = components.iter().position(|component| component == marker) {
            return Some((source, components.get(index + 1)?.to_string()));
        }
    }
    None
}

fn owner_matches(package: &str, owner: &str, source: ArtifactSource) -> bool {
    let package = bare_package_name(package).to_ascii_lowercase();
    let owner = owner.to_ascii_lowercase();
    if owner == package {
        return true;
    }
    if source != ArtifactSource::Nix {
        return false;
    }

    owner
        .strip_prefix(&package)
        .and_then(|suffix| suffix.strip_prefix('-'))
        .is_some_and(|suffix| {
            suffix.starts_with(|character: char| character.is_ascii_digit())
                || suffix.starts_with("unstable-")
                || suffix.starts_with("git-")
        })
}

fn bare_package_name(package: &str) -> &str {
    package
        .rsplit('/')
        .next()
        .unwrap_or(package)
        .rsplit('.')
        .next()
        .unwrap_or(package)
}

fn collect_cask_artifact(value: &Value, out: &mut CaskArtifacts) {
    let Some(object) = value.as_object() else {
        return;
    };
    if let Some(binary) = object.get("binary").and_then(Value::as_array) {
        let command = object
            .get("target")
            .and_then(Value::as_str)
            .or_else(|| binary.first().and_then(Value::as_str))
            .and_then(|path| Path::new(path).file_name())
            .map(|name| name.to_string_lossy().into_owned());
        if let Some(command) = command {
            push_unique(&mut out.commands, command);
        }
    }
    if let Some(app) = object
        .get("app")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
    {
        let target = object.get("target").and_then(Value::as_str).map_or_else(
            || {
                application_roots()
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| PathBuf::from("/Applications"))
                    .join(app)
                    .display()
                    .to_string()
            },
            str::to_string,
        );
        push_unique(&mut out.applications, target);
    }
}

struct HistoryIndex {
    by_command: HashMap<String, CommandObservation>,
    load_limitations: Vec<String>,
    state: HistoryState,
}

enum HistoryState {
    Disabled,
    Enabled {
        earliest_trustworthy_timestamp: Option<i64>,
        latest_trustworthy_timestamp: Option<i64>,
        conditions: HashSet<HistoryCondition>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum HistoryCondition {
    ImportedTimestampCohort,
    ParseFailures,
    Untimestamped,
}

#[derive(Default)]
struct CommandObservation {
    count: usize,
    latest_timestamp: Option<i64>,
    has_undated: bool,
}

impl CommandObservation {
    fn push(&mut self, timestamp: Option<i64>) {
        self.count += 1;
        if let Some(timestamp) = timestamp {
            self.latest_timestamp = Some(
                self.latest_timestamp
                    .map_or(timestamp, |latest| latest.max(timestamp)),
            );
        } else {
            self.has_undated = true;
        }
    }
}

impl HistoryIndex {
    fn new(entries: &[ShellHistoryEntry], load_limitations: Vec<String>, enabled: bool) -> Self {
        if !enabled {
            return Self {
                by_command: HashMap::new(),
                load_limitations,
                state: HistoryState::Disabled,
            };
        }

        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .expect("tree-sitter Bash language should load");
        let mut seen: HashSet<(&str, Option<i64>, Option<u64>)> = HashSet::new();
        let entries = entries
            .iter()
            .filter(|entry| {
                seen.insert((
                    entry.command.as_str(),
                    entry.started_at_epoch_secs,
                    entry.duration_secs,
                ))
            })
            .collect::<Vec<_>>();
        let timestamp_counts = entries
            .iter()
            .filter_map(|entry| entry.started_at_epoch_secs)
            .fold(HashMap::new(), |mut counts, timestamp| {
                *counts.entry(timestamp).or_insert(0) += 1;
                counts
            });
        let imported_timestamp = timestamp_counts
            .iter()
            .find_map(|(&timestamp, &count)| (count > entries.len() / 2).then_some(timestamp));
        let earliest_trustworthy_timestamp = timestamp_counts
            .iter()
            .filter_map(|(&timestamp, _)| {
                (Some(timestamp) != imported_timestamp).then_some(timestamp)
            })
            .min();
        let latest_trustworthy_timestamp = timestamp_counts
            .iter()
            .filter_map(|(&timestamp, _)| {
                (Some(timestamp) != imported_timestamp).then_some(timestamp)
            })
            .max();
        let has_untimestamped = entries
            .iter()
            .any(|entry| entry.started_at_epoch_secs.is_none());
        let mut by_command: HashMap<String, CommandObservation> = HashMap::new();
        let mut has_parse_failures = false;

        for entry in entries {
            let (commands, complete) = command_words(&mut parser, &entry.command);
            has_parse_failures |= !complete;
            let timestamp = entry
                .started_at_epoch_secs
                .filter(|timestamp| Some(*timestamp) != imported_timestamp);
            for command in commands {
                by_command.entry(command).or_default().push(timestamp);
            }
        }

        let mut conditions = HashSet::new();
        if imported_timestamp.is_some() {
            conditions.insert(HistoryCondition::ImportedTimestampCohort);
        }
        if has_parse_failures {
            conditions.insert(HistoryCondition::ParseFailures);
        }
        if has_untimestamped {
            conditions.insert(HistoryCondition::Untimestamped);
        }
        Self {
            by_command,
            load_limitations,
            state: HistoryState::Enabled {
                earliest_trustworthy_timestamp,
                latest_trustworthy_timestamp,
                conditions,
            },
        }
    }

    fn observe(
        &self,
        command: &str,
        attribution_confidence: EvidenceConfidence,
        cutoff_epoch_secs: i64,
        evidence: &mut Vec<EvidenceItem>,
        coverage: &mut EvidenceCoverage,
    ) {
        let HistoryState::Enabled {
            earliest_trustworthy_timestamp,
            latest_trustworthy_timestamp,
            conditions,
        } = &self.state
        else {
            coverage.add_limitation(
                EvidenceProvider::TimestampedShellHistory,
                "shell history scanning was disabled",
            );
            return;
        };
        for limitation in &self.load_limitations {
            coverage.add_limitation(EvidenceProvider::TimestampedShellHistory, limitation);
        }

        if self.load_limitations.is_empty()
            && earliest_trustworthy_timestamp.is_some_and(|earliest| earliest <= cutoff_epoch_secs)
            && latest_trustworthy_timestamp.is_some_and(|latest| latest >= cutoff_epoch_secs)
        {
            coverage.add_provider(EvidenceProvider::TimestampedShellHistory);
        } else if earliest_trustworthy_timestamp.is_some() {
            coverage.add_limitation(
                EvidenceProvider::TimestampedShellHistory,
                "timestamped shell history does not span the requested window",
            );
        } else {
            coverage.add_limitation(
                EvidenceProvider::TimestampedShellHistory,
                "no trustworthy timestamped shell history was available",
            );
        }
        if conditions.contains(&HistoryCondition::ImportedTimestampCohort) {
            coverage.add_limitation(
                EvidenceProvider::TimestampedShellHistory,
                "a dominant shell-history timestamp cohort was treated as imported and undated",
            );
        }
        if conditions.contains(&HistoryCondition::ParseFailures) {
            coverage.add_limitation(
                EvidenceProvider::TimestampedShellHistory,
                "some shell-history entries were incomplete and were ignored",
            );
        }
        if conditions.contains(&HistoryCondition::Untimestamped) {
            coverage.add_provider(EvidenceProvider::UntimestampedShellHistory);
        }

        if let Some(observation) = self.by_command.get(command) {
            if let Some(timestamp) = observation.latest_timestamp {
                evidence.push(EvidenceItem {
                    kind: EvidenceKind::ShellHistory,
                    summary: history_summary(command, observation.count),
                    timestamp: Some(timestamp),
                    confidence: attribution_confidence,
                });
            }
            if observation.has_undated {
                evidence.push(EvidenceItem {
                    kind: EvidenceKind::ShellHistory,
                    summary: history_summary(command, observation.count),
                    timestamp: None,
                    confidence: EvidenceConfidence::Medium,
                });
            }
        }
    }
}

fn history_summary(command: &str, count: usize) -> String {
    if count == 1 {
        format!("command `{command}` appeared once in shell history")
    } else {
        format!("command `{command}` appeared {count} times in shell history")
    }
}

fn command_words(parser: &mut Parser, command: &str) -> (Vec<String>, bool) {
    let Some(tree) = parser.parse(command, None) else {
        return (Vec::new(), false);
    };
    let root = tree.root_node();
    if root.has_error() {
        return (Vec::new(), false);
    }
    let mut commands = Vec::new();
    collect_command_nodes(root, command.as_bytes(), &mut commands);
    (commands, true)
}

fn collect_command_nodes(node: Node<'_>, source: &[u8], out: &mut Vec<String>) {
    if node.kind() == "command"
        && let Some(command) = command_word(node, source)
    {
        push_unique(out, command);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_command_nodes(child, source, out);
    }
}

fn command_word(node: Node<'_>, source: &[u8]) -> Option<String> {
    let name = static_shell_word(node.child_by_field_name("name")?.utf8_text(source).ok()?)?;
    let mut cursor = node.walk();
    let arguments = node
        .children_by_field_name("argument", &mut cursor)
        .filter_map(|argument| static_shell_word(argument.utf8_text(source).ok()?))
        .collect::<Vec<_>>();
    resolve_command(&name, &arguments)
}

fn static_shell_word(word: &str) -> Option<String> {
    let word = word.trim();
    let word = if word.len() >= 2
        && ((word.starts_with('"') && word.ends_with('"'))
            || (word.starts_with('\'') && word.ends_with('\'')))
    {
        &word[1..word.len() - 1]
    } else {
        word
    };
    (!word.is_empty() && !word.contains(['$', '`', '\n']) && !word.chars().any(char::is_whitespace))
        .then(|| word.to_string())
}

fn resolve_command(name: &str, arguments: &[String]) -> Option<String> {
    let bare = name.rsplit('/').next().unwrap_or(name);
    if !matches!(
        bare,
        "command" | "env" | "noglob" | "sudo" | "time" | "xargs"
    ) {
        return is_static_command(bare).then(|| bare.to_string());
    }
    if bare == "command"
        && arguments
            .iter()
            .any(|argument| matches!(argument.as_str(), "-v" | "-V"))
    {
        return None;
    }

    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        if argument == "--" {
            index += 1;
            break;
        }
        if !argument.starts_with('-') {
            break;
        }
        if wrapper_option_takes_value(bare, argument)
            && !argument.contains('=')
            && !(argument.starts_with('-') && !argument.starts_with("--") && argument.len() > 2)
        {
            index += 1;
        }
        index += 1;
    }
    let command = arguments.get(index)?;
    let bare = command.rsplit('/').next().unwrap_or(command);
    is_static_command(bare).then(|| bare.to_string())
}

fn wrapper_option_takes_value(wrapper: &str, option: &str) -> bool {
    match wrapper {
        "env" => matches!(
            option,
            "-u" | "--unset" | "-C" | "--chdir" | "-S" | "--split-string"
        ),
        "sudo" => matches!(
            option,
            "-A" | "-a"
                | "-C"
                | "-c"
                | "-D"
                | "-g"
                | "-h"
                | "-p"
                | "-R"
                | "-r"
                | "-T"
                | "-t"
                | "-U"
                | "-u"
                | "--askpass"
                | "--auth-type"
                | "--close-from"
                | "--chdir"
                | "--group"
                | "--host"
                | "--prompt"
                | "--chroot"
                | "--role"
                | "--command-timeout"
                | "--type"
                | "--other-user"
                | "--user"
        ),
        "xargs" => matches!(
            option,
            "-a" | "--arg-file" | "-E" | "-I" | "-L" | "-n" | "-P" | "-s"
        ),
        _ => false,
    }
}

fn is_static_command(command: &str) -> bool {
    !command.is_empty()
        && command
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_+.-".contains(character))
}

struct ProcessIndex {
    commands: Vec<String>,
    observed_at: i64,
    available: bool,
}

impl ProcessIndex {
    fn collect(observed_at: i64) -> Self {
        let output = run_captured_command("ps", &["-axo", "command="], None);
        match output {
            Ok(output) if output.code == 0 => Self {
                commands: output.stdout.lines().map(str::to_string).collect(),
                observed_at,
                available: true,
            },
            _ => Self {
                commands: Vec::new(),
                observed_at,
                available: false,
            },
        }
    }

    fn observe_application(
        &self,
        path: &str,
        attribution_confidence: EvidenceConfidence,
        evidence: &mut Vec<EvidenceItem>,
        coverage: &mut EvidenceCoverage,
    ) {
        if !self.available {
            coverage.add_limitation(
                EvidenceProvider::ProcessSnapshot,
                "process snapshot was unavailable",
            );
            return;
        }
        coverage.add_provider(EvidenceProvider::ProcessSnapshot);
        let app_name = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if self.commands.iter().any(|command| {
            command.contains(path)
                || app_name
                    .as_deref()
                    .is_some_and(|name| command.contains(name))
        }) {
            evidence.push(EvidenceItem {
                kind: EvidenceKind::Process,
                summary: format!("application is present in the current process snapshot: {path}"),
                timestamp: Some(self.observed_at),
                confidence: attribution_confidence,
            });
        }
    }
}

fn observe_application(
    path: &str,
    attribution_confidence: EvidenceConfidence,
    evidence: &mut Vec<EvidenceItem>,
    coverage: &mut EvidenceCoverage,
) {
    let output = run_captured_command(
        "mdls",
        &["-raw", "-name", "kMDItemLastUsedDate", path],
        None,
    );
    let Ok(output) = output else {
        coverage.add_limitation(
            EvidenceProvider::Spotlight,
            "Spotlight metadata command was unavailable",
        );
        return;
    };
    if output.code != 0 {
        coverage.add_limitation(
            EvidenceProvider::Spotlight,
            format!("Spotlight metadata unavailable for {path}"),
        );
        return;
    }

    coverage.add_provider(EvidenceProvider::Spotlight);
    match parse_spotlight_use(output.stdout.trim()) {
        SpotlightUse::NotObserved => {}
        SpotlightUse::LastUsed(timestamp) => evidence.push(EvidenceItem {
            kind: EvidenceKind::Spotlight,
            summary: format!("application last used according to Spotlight: {path}"),
            timestamp: Some(timestamp),
            confidence: attribution_confidence,
        }),
        SpotlightUse::Invalid => {
            coverage.add_limitation(
                EvidenceProvider::Spotlight,
                format!("unrecognized Spotlight timestamp for {path}"),
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpotlightUse {
    NotObserved,
    LastUsed(i64),
    Invalid,
}

fn parse_spotlight_use(raw: &str) -> SpotlightUse {
    if raw.is_empty() || raw == "(null)" {
        return SpotlightUse::NotObserved;
    }
    DateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S %z")
        .map_or(SpotlightUse::Invalid, |timestamp| {
            SpotlightUse::LastUsed(timestamp.timestamp())
        })
}

fn push_artifact(artifacts: &mut Vec<UsageArtifact>, artifact: UsageArtifact) {
    if !artifacts.contains(&artifact) {
        artifacts.push(artifact);
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn command_roots() -> Vec<PathBuf> {
    if let Some(roots) = env::var_os("NX_USAGE_COMMAND_ROOTS") {
        return env::split_paths(&roots).collect();
    }

    let mut roots = vec![
        PathBuf::from("/run/current-system/sw/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(user) = env::var_os("USER") {
        roots.push(Path::new("/etc/profiles/per-user").join(user).join("bin"));
    }
    if let Some(home) = env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".nix-profile/bin"));
    }
    roots
}

fn application_roots() -> Vec<PathBuf> {
    if let Some(roots) = env::var_os("NX_USAGE_APPLICATION_ROOTS") {
        return env::split_paths(&roots).collect();
    }

    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactSource, HistoryIndex, ProcessIndex, SpotlightUse, command_words, owner_matches,
        parse_spotlight_use,
    };
    use crate::domain::usage::{
        EvidenceConfidence, EvidenceCoverage, EvidenceKind, EvidenceProvider,
    };
    use crate::infra::shell_history::ShellHistoryEntry;
    use tree_sitter::Parser;

    #[test]
    fn extracts_each_pipeline_command_and_transparent_wrappers() {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .expect("Bash language should load");
        let (commands, complete) =
            command_words(&mut parser, "curl example.test | sudo -u root jq -r .name");

        assert!(complete);
        assert_eq!(commands, vec!["curl", "jq"]);
    }

    #[test]
    fn ignores_commands_from_incomplete_shell_input() {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .expect("Bash language should load");
        let (commands, complete) = command_words(&mut parser, "jq 'unterminated");

        assert!(!complete);
        assert!(commands.is_empty());
    }

    #[test]
    fn history_deduplicates_overlapping_files() {
        let entries = vec![
            ShellHistoryEntry {
                command: "jq .".to_string(),
                started_at_epoch_secs: Some(100),
                duration_secs: None,
            },
            ShellHistoryEntry {
                command: "jq .".to_string(),
                started_at_epoch_secs: Some(100),
                duration_secs: None,
            },
        ];
        let index = HistoryIndex::new(&entries, Vec::new(), true);
        let mut evidence = Vec::new();
        index.observe(
            "jq",
            EvidenceConfidence::Strong,
            100,
            &mut evidence,
            &mut EvidenceCoverage::default(),
        );

        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].kind, EvidenceKind::ShellHistory);
    }

    #[test]
    fn dominant_history_timestamp_cohort_is_observed_without_recency() {
        let entries = vec![
            history_entry("jq .", 100),
            history_entry("rg needle", 100),
            history_entry("fd src", 200),
        ];
        let index = HistoryIndex::new(&entries, Vec::new(), true);
        let mut evidence = Vec::new();
        let mut coverage = EvidenceCoverage::default();
        index.observe(
            "jq",
            EvidenceConfidence::Strong,
            50,
            &mut evidence,
            &mut coverage,
        );

        assert_eq!(evidence[0].timestamp, None);
        assert_eq!(evidence[0].confidence, EvidenceConfidence::Medium);
        assert!(
            coverage
                .limitations
                .iter()
                .any(|limitation| limitation.message.contains("dominant"))
        );
    }

    #[test]
    fn nix_owner_matching_requires_a_version_like_suffix() {
        assert!(owner_matches("go", "go-1.24.5", ArtifactSource::Nix));
        assert!(owner_matches(
            "ripgrep",
            "ripgrep-unstable-2026-01-01",
            ArtifactSource::Nix
        ));
        assert!(!owner_matches("go", "go-task-3.44.1", ArtifactSource::Nix));
        assert!(!owner_matches(
            "tree",
            "tree-sitter-0.25.8",
            ArtifactSource::Nix
        ));
        assert!(!owner_matches("go", "go-task", ArtifactSource::Homebrew));
    }

    #[test]
    fn spotlight_values_distinguish_absence_dates_and_invalid_metadata() {
        assert_eq!(parse_spotlight_use("(null)"), SpotlightUse::NotObserved);
        assert_eq!(parse_spotlight_use(""), SpotlightUse::NotObserved);
        assert_eq!(
            parse_spotlight_use("2026-07-26 10:30:00 -0700"),
            SpotlightUse::LastUsed(1_785_087_000)
        );
        assert_eq!(parse_spotlight_use("yesterday"), SpotlightUse::Invalid);
    }

    #[test]
    fn process_snapshot_is_positive_evidence_not_an_absence_claim() {
        let index = ProcessIndex {
            commands: vec![
                "/Applications/Resilio Sync.app/Contents/MacOS/Resilio Sync".to_string(),
            ],
            observed_at: 123,
            available: true,
        };
        let mut evidence = Vec::new();
        let mut coverage = EvidenceCoverage::default();
        index.observe_application(
            "/Applications/Resilio Sync.app",
            EvidenceConfidence::Strong,
            &mut evidence,
            &mut coverage,
        );

        assert_eq!(evidence[0].kind, EvidenceKind::Process);
        assert_eq!(evidence[0].timestamp, Some(123));
        assert!(
            coverage
                .providers
                .contains(&EvidenceProvider::ProcessSnapshot)
        );

        evidence.clear();
        index.observe_application(
            "/Applications/Other.app",
            EvidenceConfidence::Strong,
            &mut evidence,
            &mut coverage,
        );
        assert!(evidence.is_empty());
    }

    fn history_entry(command: &str, timestamp: i64) -> ShellHistoryEntry {
        ShellHistoryEntry {
            command: command.to_string(),
            started_at_epoch_secs: Some(timestamp),
            duration_secs: None,
        }
    }
}
