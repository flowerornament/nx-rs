//! Read-only observations; failure never changes the upgrade's exit status.
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cli::{UpgradeArgs, UpgradeScope};
use crate::domain::upgrade_receipt::{
    Inventory, ObservationStatus, ObservedChanges, PackageState, UpgradeReceipt, UpgradeStep,
};
use crate::infra::bounded_command;
use crate::infra::timing::TimingPhase;
use crate::output::printer::Printer;

pub(super) struct Snapshot {
    system: Option<String>,
    brew: Result<Inventory, String>,
    nix: Result<Inventory, String>,
}

impl Snapshot {
    pub(super) fn observe(args: &UpgradeArgs) -> Self {
        // One budget covers the complete observation, including all pipe drains.
        let deadline = Instant::now() + Duration::from_secs(5);
        let profile = fs::canonicalize(super::rebuild::system_profile_path()).ok();
        let (brew, nix) = std::thread::scope(|scope| {
            let brew = scope.spawn(crate::infra::run_trace::with_parent(|| {
                if args.should_run_brew_phase() || !args.skip_rebuild() {
                    query("brew", &["info", "--json=v2", "--installed"], deadline)
                        .and_then(|text| brew_inventory(&text))
                } else {
                    Err("skipped Homebrew and rebuild".into())
                }
            }));
            let nix = if args.skip_rebuild() {
                Err("rebuild skipped".into())
            } else {
                profile
                    .as_ref()
                    .ok_or_else(|| "system profile unavailable".into())
                    .and_then(|path| nix_inventory(path, deadline))
            };
            (
                brew.join()
                    .unwrap_or_else(|_| Err("Homebrew observation worker failed".into())),
                nix,
            )
        });
        Self {
            system: profile.map(|path| path.display().to_string()),
            brew,
            nix,
        }
    }
}

fn query(program: &str, args: &[&str], deadline: Instant) -> Result<String, String> {
    let output =
        bounded_command::run(program, args, deadline).map_err(|error| error.to_string())?;
    if output.code != 0 {
        return Err(format!("{program} inventory exited {}", output.code));
    }
    Ok(output.stdout)
}

pub(super) fn start(args: &UpgradeArgs) -> UpgradeReceipt {
    UpgradeReceipt {
        scope: match args.scope() {
            UpgradeScope::Homebrew => "homebrew",
            UpgradeScope::Inputs => "inputs",
            UpgradeScope::Full => "full",
        }
        .into(),
        changed_inputs: vec![],
        activation: if args.skip_rebuild() {
            UpgradeStep::Skipped
        } else {
            UpgradeStep::NotRun
        },
        inputs: if args.brew_only() {
            UpgradeStep::Skipped
        } else {
            UpgradeStep::NotRun
        },
        homebrew: if args.should_run_brew_phase() {
            UpgradeStep::NotRun
        } else {
            UpgradeStep::Skipped
        },
        rebuild: if args.skip_rebuild() {
            UpgradeStep::Skipped
        } else {
            UpgradeStep::NotRun
        },
        commit: if args.skip_commit() {
            UpgradeStep::Skipped
        } else {
            UpgradeStep::NotRun
        },
        system_before: None,
        system_after: None,
        packages: vec![
            ObservedChanges::between(
                "Homebrew",
                &Err("package changes not reached".into()),
                &Err("package changes not reached".into()),
            ),
            ObservedChanges::between(
                "Nix system-profile references",
                &Err("package changes not reached".into()),
                &Err("package changes not reached".into()),
            ),
        ],
        shadowed_commands: vec![],
    }
}

pub(super) fn finish(receipt: &mut UpgradeReceipt, before: &Snapshot, after: &Snapshot) {
    receipt.system_before.clone_from(&before.system);
    receipt.system_after.clone_from(&after.system);
    receipt.packages = vec![
        ObservedChanges::between("Homebrew", &before.brew, &after.brew),
        ObservedChanges::between("Nix system-profile references", &before.nix, &after.nix),
    ];
    if receipt.rebuild == UpgradeStep::Skipped {
        receipt.packages[1].status = ObservationStatus::Skipped;
        if receipt.homebrew == UpgradeStep::Skipped {
            receipt.packages[0].status = ObservationStatus::Skipped;
        }
    }
    receipt.shadowed_commands = shadowed_commands(&receipt.packages[0]);
}

pub(super) fn print(receipt: &UpgradeReceipt, code: i32) {
    Printer::heading(if code == 0 {
        "Upgrade result"
    } else {
        "Upgrade result (incomplete)"
    });
    Printer::detail(&format!(
        "Inputs: {}; Homebrew: {}; rebuild: {}; activation: {}; commit: {}",
        receipt.inputs, receipt.homebrew, receipt.rebuild, receipt.activation, receipt.commit
    ));
    if !receipt.changed_inputs.is_empty() {
        Printer::detail(&format!(
            "Changed inputs: {}",
            receipt.changed_inputs.join(", ")
        ));
    }
    match (&receipt.system_before, &receipt.system_after) {
        (Some(old), Some(new)) if old != new => {
            Printer::detail(&format!("System profile changed: {old} → {new}"));
        }
        (Some(_), Some(_)) => Printer::detail("System profile unchanged"),
        _ => Printer::detail("System profile change unknown"),
    }
    for observed in &receipt.packages {
        if observed.status == ObservationStatus::Unknown {
            Printer::detail(&format!(
                "{}: unknown ({})",
                observed.scope,
                observed
                    .detail
                    .as_deref()
                    .unwrap_or("observation unavailable")
            ));
        } else if observed.status == ObservationStatus::Skipped {
            Printer::detail(&format!("{}: skipped", observed.scope));
        } else if observed.changes.is_empty() {
            Printer::detail(&format!("{}: no observed package changes", observed.scope));
        } else {
            Printer::detail(&format!(
                "{}: {} changed",
                observed.scope,
                observed.changes.len()
            ));
            for change in &observed.changes {
                let old = versions(&change.before);
                let new = versions(&change.after);
                Printer::sub_detail(&if change.before.is_empty() {
                    format!(
                        "{}: {} {new}",
                        change.name,
                        if observed.scope == "Homebrew" {
                            "installed"
                        } else {
                            "added reference"
                        }
                    )
                } else if change.after.is_empty() {
                    format!(
                        "{}: {} {old}",
                        change.name,
                        if observed.scope == "Homebrew" {
                            "removed"
                        } else {
                            "removed reference"
                        }
                    )
                } else if old == new {
                    format!("{}: {new} (output changed)", change.name)
                } else {
                    format!("{}: {old} → {new}", change.name)
                });
            }
        }
    }
    for shadowed in &receipt.shadowed_commands {
        Printer::detail(shadowed);
    }
}

fn versions(packages: &[PackageState]) -> String {
    let mut versions: Vec<_> = packages
        .iter()
        .map(|package| package.version.as_str())
        .collect();
    versions.sort_unstable();
    versions.dedup();
    versions.join(", ")
}

fn brew_inventory(text: &str) -> Result<Inventory, String> {
    let value: Value = serde_json::from_str(text).map_err(|_| "invalid Homebrew inventory JSON")?;
    let mut packages = BTreeMap::new();
    for (kind, key, name_key) in [("formula", "formulae", "name"), ("cask", "casks", "token")] {
        let rows = value
            .get(key)
            .and_then(Value::as_array)
            .ok_or("incomplete Homebrew inventory")?;
        for row in rows {
            let name = row
                .get(name_key)
                .and_then(Value::as_str)
                .ok_or("missing Homebrew package name")?;
            let installed = row
                .get("installed")
                .ok_or("missing Homebrew installed state")?;
            let mut versions = if kind == "cask" {
                match installed {
                    Value::Null => vec![],
                    Value::String(version) => vec![version.clone()],
                    Value::Array(items) => items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                    _ => return Err("invalid installed cask versions".into()),
                }
            } else {
                installed
                    .as_array()
                    .ok_or("invalid installed formula versions")?
                    .iter()
                    .map(|entry| {
                        let version = entry
                            .get("version")
                            .and_then(Value::as_str)
                            .ok_or("missing installed formula version")?;
                        Ok(version.to_owned())
                    })
                    .collect::<Result<Vec<_>, &str>>()?
            };
            versions.sort();
            versions.dedup();
            if !versions.is_empty() {
                packages.insert(
                    format!("{kind}:{name}"),
                    versions
                        .into_iter()
                        .map(|version| PackageState {
                            identity: version.clone(),
                            version,
                            commands: cask_commands(row),
                        })
                        .collect(),
                );
            }
        }
    }
    Ok(packages)
}

fn cask_commands(row: &Value) -> Vec<String> {
    row.get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|artifact| artifact.get("binary").is_some())
        .filter_map(|artifact| artifact.get("target").and_then(Value::as_str))
        .filter(|target| Path::new(target).is_absolute())
        .map(str::to_owned)
        .collect()
}

fn references(path: &Path, deadline: Instant) -> Result<Vec<String>, String> {
    // nix-store interprets a subpath under /nix/store as its enclosing store
    // object. Resolve environment symlinks first to query the environment itself.
    let target = fs::canonicalize(path).map_err(|error| error.to_string())?;
    let path = target.to_str().ok_or("non-UTF8 environment path")?;
    let output = query("nix-store", &["--query", "--references", path], deadline)?;
    let paths: Vec<_> = output.lines().map(str::to_owned).collect();
    if paths.iter().any(|path| !path.starts_with("/nix/store/")) {
        return Err("invalid Nix system-profile references".into());
    }
    Ok(paths)
}

fn nix_inventory(profile: &Path, deadline: Instant) -> Result<Inventory, String> {
    let mut paths = references(&profile.join("sw"), deadline)?;
    for user in user_environments(&profile.join("etc/profiles/per-user"))? {
        for environment in references(&user, deadline)? {
            if environment.ends_with("-home-manager-path") {
                paths.extend(references(Path::new(&environment), deadline)?);
            } else {
                paths.push(environment);
            }
        }
    }
    nix_packages(paths)
}

fn user_environments(directory: &Path) -> Result<Vec<PathBuf>, String> {
    match fs::read_dir(directory) {
        Ok(users) => users
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|error| error.to_string())
            })
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(error) => Err(error.to_string()),
    }
}

fn nix_packages(mut paths: Vec<String>) -> Result<Inventory, String> {
    paths.sort();
    paths.dedup();
    let mut packages: Inventory = BTreeMap::new();
    for path in paths {
        let basename = path
            .strip_prefix("/nix/store/")
            .ok_or("invalid store path")?;
        let (_, label) = basename.split_once('-').ok_or("invalid store name")?;
        let split = label
            .match_indices('-')
            .find(|(index, _)| {
                label
                    .as_bytes()
                    .get(index + 1)
                    .is_some_and(u8::is_ascii_digit)
            })
            .map(|(index, _)| index);
        let (name, version) = split.map_or((label, "unversioned"), |index| {
            (&label[..index], &label[index + 1..])
        });
        packages.entry(name.into()).or_default().push(PackageState {
            version: version.into(),
            identity: path,
            commands: vec![],
        });
    }
    Ok(packages)
}

pub(super) fn activation_outcome(phases: &[TimingPhase], rebuild: UpgradeStep) -> UpgradeStep {
    fn find<'a>(phases: &'a [TimingPhase], name: &str) -> Option<&'a TimingPhase> {
        phases
            .iter()
            .find(|phase| phase.name == name)
            .or_else(|| phases.iter().find_map(|phase| find(&phase.children, name)))
    }
    if find(phases, "already-current").is_some() {
        return UpgradeStep::AlreadyCurrent;
    }
    if let Some(phase) = find(phases, "activate").or_else(|| find(phases, "legacy-switch")) {
        return if phase.status == "ok" {
            UpgradeStep::Completed
        } else {
            UpgradeStep::Failed
        };
    }
    if rebuild == UpgradeStep::Skipped {
        UpgradeStep::Skipped
    } else if rebuild == UpgradeStep::Completed {
        UpgradeStep::ConfiguredRunner
    } else {
        UpgradeStep::NotObserved
    }
}

fn selected_command(binary: &str, search: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(search)
        .map(|dir| dir.join(binary))
        .find(|path| {
            path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

fn shadowed_commands(observed: &ObservedChanges) -> Vec<String> {
    let search = std::env::var_os("PATH").unwrap_or_default();
    shadowed_in(
        observed,
        &[Path::new("/opt/homebrew"), Path::new("/usr/local")],
        &search,
    )
}

fn shadowed_in(
    observed: &ObservedChanges,
    prefixes: &[&Path],
    search: &std::ffi::OsStr,
) -> Vec<String> {
    let mut shadowed = vec![];
    for change in &observed.changes {
        if change.after.is_empty() {
            continue;
        }
        let mut commands: Vec<_> = change
            .after
            .iter()
            .flat_map(|package| package.commands.iter().map(PathBuf::from))
            .collect();
        if let Some(name) = change.name.strip_prefix("formula:") {
            for prefix in prefixes {
                let bins = prefix.join("opt").join(name).join("bin");
                if let Ok(entries) = fs::read_dir(bins) {
                    commands.extend(entries.flatten().map(|entry| entry.path()));
                }
            }
        }
        for command in commands {
            let Some(binary) = command.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(selected) = selected_command(binary, search) else {
                continue;
            };
            if let (Ok(expected), Ok(actual)) =
                (fs::canonicalize(&command), fs::canonicalize(&selected))
                && expected != actual
            {
                shadowed.push(format!(
                    "{binary}: PATH selects {} instead of Homebrew's upgraded command",
                    selected.display()
                ));
            }
        }
    }
    shadowed.sort();
    shadowed.dedup();
    shadowed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_users_is_empty_but_unreadable_directory_is_not_a_successful_inventory() {
        let temp = tempfile::TempDir::new().unwrap();
        assert!(
            user_environments(&temp.path().join("missing"))
                .unwrap()
                .is_empty()
        );
        let path = temp.path().join("not-a-directory");
        fs::write(&path, "").unwrap();
        assert!(user_environments(&path).is_err());
    }

    #[test]
    fn reference_inventory_keeps_output_identity_for_same_version_rebuilds() {
        let before = nix_packages(vec!["/nix/store/old-rg-1.0".into()]);
        let after = nix_packages(vec![
            "/nix/store/new-rg-1.0".into(),
            "/nix/store/doc-rg-1.0-man".into(),
        ]);
        let observed = ObservedChanges::between("Nix", &before, &after);
        assert_eq!(observed.changes.len(), 1);
        assert_eq!(observed.changes[0].after.len(), 2);
    }

    #[test]
    fn formula_and_cask_path_shadowing_uses_selected_executable() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::TempDir::new().unwrap();
        let prefix = temp.path().join("brew");
        let bins = prefix.join("opt/tool/bin");
        let local = temp.path().join("local");
        fs::create_dir_all(&bins).unwrap();
        fs::create_dir_all(&local).unwrap();
        for path in [bins.join("tool"), local.join("tool")] {
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let after = Ok(BTreeMap::from([(
            "formula:tool".into(),
            vec![PackageState {
                version: "2".into(),
                identity: "2".into(),
                commands: vec![],
            }],
        )]));
        let observed = ObservedChanges::between("Homebrew", &Ok(Inventory::new()), &after);
        assert_eq!(
            shadowed_in(&observed, &[&prefix], local.as_os_str()).len(),
            1
        );
        fs::remove_file(local.join("tool")).unwrap();
        symlink(bins.join("tool"), local.join("tool")).unwrap();
        assert!(shadowed_in(&observed, &[&prefix], local.as_os_str()).is_empty());
        let cask = brew_inventory(&serde_json::json!({"formulae":[],"casks":[{"token":"tool","installed":"2","artifacts":[{"binary":["tool"],"target":bins.join("tool")}]}]}).to_string()).unwrap();
        let observed = ObservedChanges::between("Homebrew", &Ok(Inventory::new()), &Ok(cask));
        assert!(shadowed_in(&observed, &[], local.as_os_str()).is_empty());
    }

    #[test]
    fn installed_inventory_preserves_formula_and_multiple_cask_versions() {
        let inventory = brew_inventory(r#"{"formulae":[{"name":"rg","installed":[{"version":"1_2"}]}],"casks":[{"token":"app","installed":"2,3"}]}"#).unwrap();
        assert_eq!(inventory["formula:rg"][0].version, "1_2");
        assert_eq!(inventory["cask:app"][0].version, "2,3");
        assert!(brew_inventory(r#"{"formulae":[]}"#).is_err());
    }
}
