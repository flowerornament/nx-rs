use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::config::{
    DARWIN_ROUTING_KEYWORDS, HOMEBREW_BREWS_ROUTING_KEYWORDS, HOMEBREW_CASKS_ROUTING_KEYWORDS,
    HOMEBREW_TAPS_ROUTING_KEYWORDS, LANGUAGES_ROUTING_KEYWORDS, PACKAGES_ROUTING_KEYWORDS,
    SERVICES_ROUTING_KEYWORDS,
};
use super::repo_scan::{NixFileScanPolicy, collect_managed_nix_files};

const ROUTING_RULES: &[RoutingRule] = &[
    RoutingRule::new("packages", PACKAGES_ROUTING_KEYWORDS),
    RoutingRule::new("languages", LANGUAGES_ROUTING_KEYWORDS),
    RoutingRule::new("services", SERVICES_ROUTING_KEYWORDS),
    RoutingRule::new("darwin", DARWIN_ROUTING_KEYWORDS),
    RoutingRule::new("homebrew brews", HOMEBREW_BREWS_ROUTING_KEYWORDS),
    RoutingRule::new("homebrew casks", HOMEBREW_CASKS_ROUTING_KEYWORDS),
    RoutingRule::new("homebrew taps", HOMEBREW_TAPS_ROUTING_KEYWORDS),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingAudit {
    issues: Vec<RoutingIssue>,
}

impl RoutingAudit {
    pub fn scan(repo_root: &Path) -> Self {
        let files = collect_managed_nix_files(repo_root, NixFileScanPolicy::for_config_routing());
        let mut issues = Vec::new();
        let mut purposes: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        let mut keyword_matches: BTreeMap<String, KeywordMatch> = BTreeMap::new();

        for path in files {
            match read_tag(&path) {
                TagState::Missing => {
                    issues.push(RoutingIssue::MissingComment { path: path.clone() });
                }
                TagState::Empty => issues.push(RoutingIssue::EmptyComment { path: path.clone() }),
                TagState::Purpose(purpose) => {
                    let purpose_lower = purpose.to_lowercase();
                    for rule in ROUTING_RULES {
                        for keyword in rule.keywords {
                            if purpose_lower.contains(keyword) {
                                keyword_matches
                                    .entry((*keyword).to_string())
                                    .or_insert_with(|| KeywordMatch::new(rule.target))
                                    .paths
                                    .push(path.clone());
                            }
                        }
                    }

                    purposes
                        .entry(purpose_lower)
                        .or_default()
                        .push(path.clone());
                }
            }
        }

        for paths in purposes.into_values() {
            if paths.len() > 1 {
                issues.push(RoutingIssue::DuplicatePurpose { paths });
            }
        }

        for (keyword, overlap) in keyword_matches {
            let mut paths = overlap.paths;
            sort_and_dedup_paths(&mut paths);
            if paths.len() > 1 {
                issues.push(RoutingIssue::KeywordOverlap {
                    keyword,
                    target: overlap.target.to_string(),
                    paths,
                });
            }
        }

        issues.sort();
        Self { issues }
    }

    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn issues(&self) -> &[RoutingIssue] {
        &self.issues
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoutingIssue {
    MissingComment {
        path: PathBuf,
    },
    EmptyComment {
        path: PathBuf,
    },
    DuplicatePurpose {
        paths: Vec<PathBuf>,
    },
    KeywordOverlap {
        keyword: String,
        target: String,
        paths: Vec<PathBuf>,
    },
}

impl RoutingIssue {
    pub fn summary(&self, repo_root: &Path) -> String {
        match self {
            Self::MissingComment { path } => format!(
                "{} is missing a first-line `# nx:` routing comment",
                display_path(path, repo_root)
            ),
            Self::EmptyComment { path } => format!(
                "{} has an empty first-line `# nx:` routing comment",
                display_path(path, repo_root)
            ),
            Self::DuplicatePurpose { paths } => format!(
                "duplicate routing purpose tag used by {}",
                display_paths(paths, repo_root)
            ),
            Self::KeywordOverlap {
                keyword,
                target,
                paths,
            } => format!(
                "routing keyword `{keyword}` for {target} matches multiple files: {}",
                display_paths(paths, repo_root)
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RoutingRule {
    target: &'static str,
    keywords: &'static [&'static str],
}

impl RoutingRule {
    const fn new(target: &'static str, keywords: &'static [&'static str]) -> Self {
        Self { target, keywords }
    }
}

#[derive(Debug, Clone)]
struct KeywordMatch {
    target: &'static str,
    paths: Vec<PathBuf>,
}

impl KeywordMatch {
    fn new(target: &'static str) -> Self {
        Self {
            target,
            paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TagState {
    Missing,
    Empty,
    Purpose(String),
}

fn read_tag(path: &Path) -> TagState {
    let Ok(file) = File::open(path) else {
        return TagState::Missing;
    };
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).is_err() {
        return TagState::Missing;
    }

    let trimmed = first_line.trim();
    let Some(rest) = trimmed.strip_prefix("# nx:") else {
        return TagState::Missing;
    };
    let purpose = rest.trim();
    if purpose.is_empty() {
        TagState::Empty
    } else {
        TagState::Purpose(purpose.to_string())
    }
}

fn sort_and_dedup_paths(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}

fn display_path(path: &Path, repo_root: &Path) -> String {
    path.strip_prefix(repo_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn display_paths(paths: &[PathBuf], repo_root: &Path) -> String {
    paths
        .iter()
        .map(|path| display_path(path, repo_root))
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for RoutingIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingComment { path } => {
                write!(f, "missing routing comment: {}", path.display())
            }
            Self::EmptyComment { path } => write!(f, "empty routing comment: {}", path.display()),
            Self::DuplicatePurpose { paths } => {
                write!(f, "duplicate routing purpose: ")?;
                for (index, path) in paths.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", path.display())?;
                }
                Ok(())
            }
            Self::KeywordOverlap {
                keyword,
                target,
                paths,
            } => {
                write!(f, "keyword overlap `{keyword}` for {target}: ")?;
                for (index, path) in paths.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", path.display())?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_nix(root: &Path, rel_path: &str, contents: &str) {
        let full = root.join(rel_path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, contents).unwrap();
    }

    #[test]
    fn scan_passes_clean_routing_layout() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(
            root,
            "packages/nix/cli.nix",
            "# nx: cli tools and utilities\n[]\n",
        );
        write_nix(
            root,
            "packages/nix/languages.nix",
            "# nx: language runtimes and toolchains\n[]\n",
        );
        write_nix(
            root,
            "home/services.nix",
            "# nx: services and daemons\n{}\n",
        );
        write_nix(
            root,
            "packages/homebrew/brews.nix",
            "# nx: Homebrew formula manifest\n[]\n",
        );

        let audit = RoutingAudit::scan(root);
        assert!(audit.is_clean(), "{audit:?}");
    }

    #[test]
    fn scan_reports_missing_and_empty_comments() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(root, "home/shell.nix", "{ ... }: {}\n");
        write_nix(root, "system/empty.nix", "# nx:\n{}\n");

        let audit = RoutingAudit::scan(root);
        assert_eq!(audit.issues.len(), 2);
        assert!(matches!(
            audit.issues[0],
            RoutingIssue::MissingComment { .. }
        ));
        assert!(matches!(audit.issues[1], RoutingIssue::EmptyComment { .. }));
    }

    #[test]
    fn scan_reports_duplicate_purpose_and_keyword_overlap() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        write_nix(
            root,
            "packages/homebrew/brews.nix",
            "# nx: Homebrew formula manifest\n[]\n",
        );
        write_nix(
            root,
            "packages/homebrew/more-brews.nix",
            "# nx: Homebrew formula manifest extras\n[]\n",
        );
        write_nix(root, "home/apps.nix", "# nx: GUI apps for macOS\n[]\n");
        write_nix(root, "home/duplicate.nix", "# nx: gui apps for macOS\n[]\n");

        let audit = RoutingAudit::scan(root);
        assert!(audit.issues.iter().any(|issue| matches!(
            issue,
            RoutingIssue::KeywordOverlap { keyword, .. } if keyword == "formula manifest"
        )));
        assert!(
            audit
                .issues
                .iter()
                .any(|issue| matches!(issue, RoutingIssue::DuplicatePurpose { .. }))
        );
        assert!(audit.issues.iter().any(|issue| matches!(
            issue,
            RoutingIssue::KeywordOverlap { keyword, .. } if keyword == "gui apps"
        )));
    }
}
