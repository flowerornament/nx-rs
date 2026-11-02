use std::collections::BTreeSet;
use std::sync::LazyLock;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::upgrade::{BrewOutdatedPackage, compare_summary_from_output};
use crate::domain::upgrade::{InputChange, github_owner_repo};
use crate::infra::bounded_command;
use crate::infra::text::truncate_with_ellipsis;

#[derive(Clone)]
pub(super) struct Target {
    pub key: String,
    pub label: String,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub old: String,
    pub new: String,
    pub input: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Evidence {
    pub url: String,
    pub description: String,
    pub titles: Vec<String>,
    #[serde(default)]
    pub total_commits: Option<usize>,
}

impl Target {
    pub fn input(change: &InputChange) -> Self {
        let repository = format!("{}/{}", change.owner, change.repo);
        Self {
            key: format!("input:{repository}:{}...{}", change.old_rev, change.new_rev),
            label: change.name.clone(),
            homepage: None,
            repository: Some(repository),
            old: change.old_rev.clone(),
            new: change.new_rev.clone(),
            input: true,
        }
    }

    pub fn brew(package: &BrewOutdatedPackage) -> Self {
        Self {
            key: format!(
                "brew:{}:{}...{}:{}",
                package.name,
                package.installed_version,
                package.current_version,
                package.homepage.as_deref().unwrap_or_default()
            ),
            label: package.name.clone(),
            homepage: package.homepage.clone(),
            repository: package
                .homepage
                .as_deref()
                .and_then(github_owner_repo)
                .map(|(owner, repo)| format!("{owner}/{repo}")),
            old: upstream_version(&package.installed_version).to_owned(),
            new: upstream_version(&package.current_version).to_owned(),
            input: false,
        }
    }
}

fn upstream_version(version: &str) -> &str {
    version
        .split([',', '_'])
        .next()
        .unwrap_or(version)
        .trim_start_matches('v')
}

pub(super) fn evidence(
    target: &Target,
    deadline: Instant,
    cancel: &AtomicBool,
    known_repo: Option<&str>,
) -> Option<(String, Evidence)> {
    if target.old == target.new {
        return None;
    }
    let repository = target
        .repository
        .clone()
        .or_else(|| known_repo.map(str::to_owned))
        .or_else(|| discover_repository(target, deadline, cancel))?;
    if !target.input {
        let endpoint = format!("repos/{repository}/releases?per_page=20");
        if let Some(releases) = github(&endpoint, deadline, cancel)
            && let Some(evidence) = published_release(&releases, &repository, &target.new)
        {
            return Some((repository, evidence));
        }
    }
    let pairs = if target.input {
        vec![(target.old.clone(), target.new.clone())]
    } else {
        vec![
            (format!("v{}", target.old), format!("v{}", target.new)),
            (target.old.clone(), target.new.clone()),
            (format!("V{}", target.old), format!("V{}", target.new)),
        ]
    };
    for (old, new) in pairs {
        let endpoint = format!("repos/{repository}/compare/{old}...{new}");
        if let Ok(output) =
            bounded_command::run_cancellable("gh", &["api", &endpoint], deadline, Some(cancel))
            && let Ok(summary) = compare_summary_from_output(&output)
        {
            let titles = summary
                .commit_subjects
                .into_iter()
                .map(|title| truncate_with_ellipsis(&title, 220))
                .collect::<Vec<_>>();
            return Some((
                repository.clone(),
                Evidence {
                    url: format!("https://github.com/{repository}/compare/{old}...{new}"),
                    description: format!(
                        "{} sampled commit titles out of {} total commits, not full diffs:\n{}",
                        titles.len(),
                        summary.total_commits,
                        titles.join("\n")
                    ),
                    titles,
                    total_commits: Some(summary.total_commits),
                },
            ));
        }
    }
    None
}

fn published_release(releases: &Value, repository: &str, version: &str) -> Option<Evidence> {
    let release = releases.as_array()?.iter().find(|release| {
        release["draft"].as_bool() != Some(true)
            && release["tag_name"]
                .as_str()
                .is_some_and(|tag| tag.trim_start_matches(['v', 'V']) == version)
    })?;
    let body = release["body"].as_str()?.trim();
    let url = release["html_url"].as_str()?;
    if body.is_empty()
        || !url.to_ascii_lowercase().starts_with(&format!(
            "https://github.com/{}/releases/",
            repository.to_ascii_lowercase()
        ))
    {
        return None;
    }
    Some(Evidence {
        url: url.to_owned(),
        description: format!(
            "Latest candidate release notes, not the complete upgrade range:\n{}",
            truncate_with_ellipsis(body, 6000)
        ),
        titles: vec![],
        total_commits: None,
    })
}

fn discover_repository(target: &Target, deadline: Instant, cancel: &AtomicBool) -> Option<String> {
    let homepage = target.homepage.as_deref()?;
    if !homepage.starts_with("https://") {
        return None;
    }
    let output = bounded_command::run_cancellable(
        "curl",
        &[
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--compressed",
            "--location",
            "--max-time",
            "2",
            "--connect-timeout",
            "1",
            "--max-filesize",
            "3145728",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--",
            homepage,
        ],
        deadline,
        Some(cancel),
    )
    .ok()?;
    (output.code == 0)
        .then(|| repository_from_page(&output.stdout, &target.label))
        .flatten()
}

fn github(endpoint: &str, deadline: Instant, cancel: &AtomicBool) -> Option<Value> {
    let output =
        bounded_command::run_cancellable("gh", &["api", endpoint], deadline, Some(cancel)).ok()?;
    (output.code == 0)
        .then(|| serde_json::from_str(&output.stdout).ok())
        .flatten()
}

pub(super) fn repository_from_page(page: &str, package: &str) -> Option<String> {
    static LINKS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"https://github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)")
            .expect("static GitHub link pattern")
    });
    let name = package.rsplit('/').next()?.split('@').next()?;
    let repositories = LINKS
        .captures_iter(page)
        .filter(|link| link[2].trim_end_matches(".git").eq_ignore_ascii_case(name))
        .map(|link| format!("{}/{}", &link[1], link[2].trim_end_matches(".git")))
        .collect::<BTreeSet<_>>();
    (repositories.len() == 1)
        .then(|| repositories.into_iter().next())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_release_matches_exact_upstream_version_with_either_tag_case() {
        let releases = serde_json::json!([
            {"tag_name":"tip","body":"unsupported current tip","html_url":"https://github.com/tw93/Mole/releases/tag/tip"},
            {"tag_name":"V1.65.0","body":"Supports new behavior.","html_url":"https://github.com/tw93/Mole/releases/tag/V1.65.0"}
        ]);
        assert!(published_release(&releases, "tw93/Mole", "1.65.0").is_some());
        assert!(published_release(&releases, "tw93/Mole", "1.65.1").is_none());
        assert!(published_release(&releases, "wrong/repository", "1.65.0").is_none());
    }

    #[test]
    fn homepage_discovery_requires_a_unique_matching_repository() {
        assert_eq!(
            repository_from_page(
                r#"<a href="https://github.com/ghostty-org/ghostty">source</a>"#,
                "ghostty"
            )
            .as_deref(),
            Some("ghostty-org/ghostty")
        );
        assert_eq!(
            repository_from_page("https://github.com/tw93/Mole", "mole").as_deref(),
            Some("tw93/Mole")
        );
        assert!(
            repository_from_page(
                "https://github.com/one/tool https://github.com/two/tool",
                "tool"
            )
            .is_none()
        );
        assert!(repository_from_page("https://github.com/unrelated/project", "mole").is_none());
        assert_eq!(upstream_version("1.4.0,123_1"), "1.4.0");
    }
}
