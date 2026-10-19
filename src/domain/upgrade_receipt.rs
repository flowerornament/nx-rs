use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum UpgradeStep {
    #[serde(rename = "not run")]
    NotRun,
    #[serde(rename = "skipped")]
    Skipped,
    #[serde(rename = "lock changed")]
    LockChanged,
    #[serde(rename = "unchanged")]
    Unchanged,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "committed")]
    Committed,
    #[serde(rename = "no owned changes")]
    NoOwnedChanges,
    #[serde(rename = "not needed")]
    NotNeeded,
    #[serde(rename = "already current")]
    AlreadyCurrent,
    #[serde(rename = "completed by configured runner")]
    ConfiguredRunner,
    #[serde(rename = "not observed")]
    NotObserved,
}

impl std::fmt::Display for UpgradeStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotRun => "not run",
            Self::Skipped => "skipped",
            Self::LockChanged => "lock changed",
            Self::Unchanged => "unchanged",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Committed => "committed",
            Self::NoOwnedChanges => "no owned changes",
            Self::NotNeeded => "not needed",
            Self::AlreadyCurrent => "already current",
            Self::ConfiguredRunner => "completed by configured runner",
            Self::NotObserved => "not observed",
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ObservationStatus {
    Observed,
    Unknown,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageState {
    pub version: String,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

pub type Inventory = BTreeMap<String, Vec<PackageState>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageChange {
    pub name: String,
    pub before: Vec<PackageState>,
    pub after: Vec<PackageState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservedChanges {
    pub scope: String,
    pub status: ObservationStatus,
    pub changes: Vec<PackageChange>,
    pub detail: Option<String>,
}

impl ObservedChanges {
    pub fn between(
        scope: &str,
        before: &Result<Inventory, String>,
        after: &Result<Inventory, String>,
    ) -> Self {
        let (Ok(before), Ok(after)) = (before, after) else {
            return Self {
                scope: scope.into(),
                status: ObservationStatus::Unknown,
                changes: vec![],
                detail: before
                    .as_ref()
                    .err()
                    .or_else(|| after.as_ref().err())
                    .cloned(),
            };
        };
        let names: BTreeSet<_> = before.keys().chain(after.keys()).collect();
        let changes = names
            .into_iter()
            .filter_map(|name| {
                let old = before.get(name).cloned().unwrap_or_default();
                let new = after.get(name).cloned().unwrap_or_default();
                let same = old.len() == new.len()
                    && old.iter().zip(&new).all(|(old, new)| {
                        old.version == new.version && old.identity == new.identity
                    });
                (!same).then(|| PackageChange {
                    name: name.clone(),
                    before: old,
                    after: new,
                })
            })
            .collect();
        Self {
            scope: scope.into(),
            status: ObservationStatus::Observed,
            changes,
            detail: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpgradeReceipt {
    pub scope: String,
    pub inputs: UpgradeStep,
    pub changed_inputs: Vec<String>,
    pub activation: UpgradeStep,
    pub homebrew: UpgradeStep,
    pub rebuild: UpgradeStep,
    pub commit: UpgradeStep,
    pub system_before: Option<String>,
    pub system_after: Option<String>,
    pub packages: Vec<ObservedChanges>,
    pub shadowed_commands: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(version: &str) -> Vec<PackageState> {
        vec![PackageState {
            version: version.into(),
            identity: version.into(),
            commands: vec![],
        }]
    }

    #[test]
    fn distinguishes_changes_from_unknown_observations() {
        let before = Ok(BTreeMap::from([
            ("removed".into(), package("1")),
            ("updated".into(), package("1")),
        ]));
        let after = Ok(BTreeMap::from([
            ("added".into(), package("2")),
            ("updated".into(), package("2")),
        ]));
        let changes = ObservedChanges::between("test", &before, &after);
        assert_eq!(changes.changes.len(), 3);
        assert!(changes.changes[0].before.is_empty());
        assert!(changes.changes[1].after.is_empty());
        let unknown = ObservedChanges::between("test", &before, &Err("query failed".into()));
        assert_eq!(unknown.status, ObservationStatus::Unknown);
        assert!(unknown.changes.is_empty());
    }
}
