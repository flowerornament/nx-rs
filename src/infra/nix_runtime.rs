use std::fmt;

use crate::infra::shell::{CapturedCommand, run_captured_command};

pub use semver::Version as ReleaseVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NixDistribution {
    Determinate,
    Lix,
    Upstream,
    Unknown,
}

impl fmt::Display for NixDistribution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Determinate => "Determinate Nix",
            Self::Lix => "Lix",
            Self::Upstream => "Nix",
            Self::Unknown => "unknown Nix distribution",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledNix {
    pub distribution: NixDistribution,
    pub version: ReleaseVersion,
}

impl InstalledNix {
    #[must_use]
    pub fn parse(output: &str) -> Option<Self> {
        let line = output.lines().find(|line| !line.trim().is_empty())?.trim();
        if let Some(rest) = line.strip_prefix("nix (Determinate Nix ") {
            let (determinate, _) = rest.split_once(") ")?;
            return Some(Self {
                distribution: NixDistribution::Determinate,
                version: parse_version(determinate)?,
            });
        }

        let version = line.split_whitespace().rev().find_map(parse_version)?;
        let lowercase = line.to_ascii_lowercase();
        Some(Self {
            distribution: if lowercase.contains("lix") {
                NixDistribution::Lix
            } else if line.starts_with("nix ") {
                NixDistribution::Upstream
            } else {
                NixDistribution::Unknown
            },
            version,
        })
    }
}

pub fn detect_installed_nix() -> anyhow::Result<Option<InstalledNix>> {
    let output = run_captured_command("nix", &["--version"], None)?;
    Ok((output.code == 0)
        .then(|| InstalledNix::parse(&output.stdout))
        .flatten())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeterminateFreshness {
    Current,
    UpdateAvailable(ReleaseVersion),
    DaemonClientMismatch,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeterminateVersionStatus {
    pub daemon: ReleaseVersion,
    pub client: ReleaseVersion,
    pub freshness: DeterminateFreshness,
}

impl DeterminateVersionStatus {
    #[must_use]
    pub fn parse(output: &str) -> Option<Self> {
        let daemon = labeled_version(output, "Determinate Nixd daemon version:")?;
        let client = labeled_version(output, "Determinate Nixd client version:")?;
        let latest = labeled_version(output, "Latest version:");
        let freshness = if daemon != client {
            DeterminateFreshness::DaemonClientMismatch
        } else if output.contains("You are running the latest version of Determinate Nix.") {
            DeterminateFreshness::Current
        } else if let Some(latest) = latest.filter(|latest| *latest > daemon) {
            DeterminateFreshness::UpdateAvailable(latest)
        } else {
            DeterminateFreshness::Unknown
        };

        Some(Self {
            daemon,
            client,
            freshness,
        })
    }
}

fn labeled_version(output: &str, label: &str) -> Option<ReleaseVersion> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .and_then(parse_version)
}

fn parse_version(text: &str) -> Option<ReleaseVersion> {
    ReleaseVersion::parse(text.trim().trim_start_matches('v')).ok()
}

pub fn determinate_version_status() -> anyhow::Result<Option<DeterminateVersionStatus>> {
    let output = run_captured_command("determinate-nixd", &["version"], None)?;
    Ok(successful_output(&output).and_then(DeterminateVersionStatus::parse))
}

fn successful_output(output: &CapturedCommand) -> Option<&str> {
    (output.code == 0)
        .then_some(output.stdout.as_str())
        .filter(|stdout| !stdout.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_determinate_nix_version_spaces() {
        assert_eq!(
            InstalledNix::parse("nix (Determinate Nix 3.21.8) 2.34.8"),
            Some(InstalledNix {
                distribution: NixDistribution::Determinate,
                version: ReleaseVersion::new(3, 21, 8),
            })
        );
    }

    #[test]
    fn parses_upstream_and_lix_without_crossing_version_spaces() {
        assert_eq!(
            InstalledNix::parse("nix 2.28.3").map(|nix| (nix.distribution, nix.version)),
            Some((NixDistribution::Upstream, ReleaseVersion::new(2, 28, 3)))
        );
        assert_eq!(
            InstalledNix::parse("nix (Lix, like Nix) 2.93.0-pre20260701")
                .map(|nix| (nix.distribution, nix.version)),
            Some((
                NixDistribution::Lix,
                ReleaseVersion::parse("2.93.0-pre20260701").expect("valid fixture version"),
            ))
        );
    }

    #[test]
    fn rejects_malformed_nix_version() {
        assert_eq!(InstalledNix::parse("nix unknown"), None);
        assert_eq!(InstalledNix::parse(""), None);
    }

    #[test]
    fn parses_current_determinate_status_with_features() {
        let output = "\
Determinate Nixd daemon version: 3.21.8
Determinate Nixd client version: 3.21.8

You are running the latest version of Determinate Nix.

The following features are enabled:

 * \u{1b}[1mlazy-trees\u{1b}[0m
";
        assert_eq!(
            DeterminateVersionStatus::parse(output),
            Some(DeterminateVersionStatus {
                daemon: ReleaseVersion::new(3, 21, 8),
                client: ReleaseVersion::new(3, 21, 8),
                freshness: DeterminateFreshness::Current,
            })
        );
    }

    #[test]
    fn parses_stale_determinate_status() {
        let output = "\
Determinate Nixd daemon version: 3.15.1
Determinate Nixd client version: 3.15.1

Latest version: 3.21.8

A new version of Determinate Nix is available.
";
        assert_eq!(
            DeterminateVersionStatus::parse(output),
            Some(DeterminateVersionStatus {
                daemon: ReleaseVersion::new(3, 15, 1),
                client: ReleaseVersion::new(3, 15, 1),
                freshness: DeterminateFreshness::UpdateAvailable(ReleaseVersion::new(3, 21, 8)),
            })
        );
    }

    #[test]
    fn unrecognized_freshness_is_unknown() {
        let output = "\
Determinate Nixd daemon version: 3.21.8
Determinate Nixd client version: 3.21.8
";
        assert_eq!(
            DeterminateVersionStatus::parse(output).map(|status| status.freshness),
            Some(DeterminateFreshness::Unknown)
        );
    }

    #[test]
    fn daemon_client_mismatch_takes_priority_over_freshness_text() {
        let output = "Determinate Nixd daemon version: 3.21.8\n\
                      Determinate Nixd client version: 3.20.0\n\
                      You are running the latest version of Determinate Nix.\n";
        assert_eq!(
            DeterminateVersionStatus::parse(output).map(|status| status.freshness),
            Some(DeterminateFreshness::DaemonClientMismatch)
        );
    }
}
