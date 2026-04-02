use std::fs;
use std::path::Path;

use anyhow::{Context, anyhow, bail};
use regex::Regex;
use serde::Serialize;

use crate::domain::generations::{
    DiskUsageSnapshot, GenerationId, GenerationKind, GenerationRecord, PrunePlan,
};
use crate::infra::shell::{run_captured_command, run_indented_command};
use crate::output::printer::Printer;

const SYSTEM_PROFILES_DIR: &str = "/nix/var/nix/profiles";
const SYSTEM_CURRENT_LINK: &str = "/nix/var/nix/profiles/system";
const NIX_MOUNT_PATH: &str = "/nix";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenerationState {
    pub generations: Vec<GenerationRecord>,
    pub disk_usage: DiskUsageSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedCommand {
    pub description: &'static str,
    pub program: String,
    pub args: Vec<String>,
}

impl PlannedCommand {
    fn new(description: &'static str, program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            description,
            program: program.into(),
            args,
        }
    }
}

pub fn discover_generation_state() -> anyhow::Result<GenerationState> {
    Ok(GenerationState {
        generations: discover_generations()?,
        disk_usage: snapshot_nix_disk_usage()?,
    })
}

pub fn discover_generations() -> anyhow::Result<Vec<GenerationRecord>> {
    let mut generations = discover_darwin_generations()?;
    generations.extend(discover_home_manager_generations()?);
    Ok(generations)
}

pub fn discover_darwin_generations() -> anyhow::Result<Vec<GenerationRecord>> {
    let profiles_dir = Path::new(SYSTEM_PROFILES_DIR);
    if !profiles_dir.exists() {
        return Ok(Vec::new());
    }
    let current_link = fs::read_link(SYSTEM_CURRENT_LINK).ok().and_then(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    });

    discover_darwin_generations_in_dir(profiles_dir, current_link.as_deref())
}

pub fn discover_home_manager_generations() -> anyhow::Result<Vec<GenerationRecord>> {
    let output = run_captured_command("home-manager", &["generations"], None)
        .context("discovering home-manager generations")?;

    if output.code != 0 {
        let detail = first_nonempty_output(&output);
        bail!(
            "home-manager generations failed{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }

    parse_home_manager_generations(&output.stdout)
}

pub fn snapshot_nix_disk_usage() -> anyhow::Result<DiskUsageSnapshot> {
    let output = run_captured_command("df", &["-h", NIX_MOUNT_PATH], None)
        .context("capturing /nix disk usage")?;

    if output.code != 0 {
        let detail = first_nonempty_output(&output);
        bail!(
            "df -h /nix failed{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }

    parse_disk_usage(&output.stdout)
}

#[must_use]
pub fn planned_commands(plan: &PrunePlan) -> Vec<PlannedCommand> {
    let mut commands = Vec::new();

    if !plan.execution.home_manager_remove.is_empty() {
        commands.push(PlannedCommand::new(
            "remove old home-manager generations",
            "home-manager",
            generation_id_args("remove-generations", &plan.execution.home_manager_remove),
        ));
    }

    if !plan.execution.darwin_remove.is_empty() {
        let mut args = vec![
            "nix-env".to_string(),
            "--delete-generations".to_string(),
            "--profile".to_string(),
            "/nix/var/nix/profiles/system".to_string(),
        ];
        args.extend(
            plan.execution
                .darwin_remove
                .iter()
                .copied()
                .map(render_generation_id),
        );
        commands.push(PlannedCommand::new(
            "remove old nix-darwin generations",
            "sudo",
            args,
        ));
    }

    if plan.execution.run_gc {
        commands.push(PlannedCommand::new(
            "collect garbage",
            "sudo",
            vec!["nix-collect-garbage".to_string(), "-d".to_string()],
        ));
    }

    commands
}

#[allow(dead_code)]
pub fn execute_planned_commands(
    commands: &[PlannedCommand],
    printer: &Printer,
) -> anyhow::Result<()> {
    for command in commands {
        let args = command.args.iter().map(String::as_str).collect::<Vec<_>>();
        let code = run_indented_command(&command.program, &args, None, printer, "  ")?;
        if code != 0 {
            return Err(anyhow!(
                "{} failed with exit code {code}",
                command.description
            ));
        }
    }
    Ok(())
}

fn discover_darwin_generations_in_dir(
    profiles_dir: &Path,
    current_link_name: Option<&str>,
) -> anyhow::Result<Vec<GenerationRecord>> {
    let mut generations = fs::read_dir(profiles_dir)
        .with_context(|| format!("reading {}", profiles_dir.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().to_string();
            parse_system_generation_name(&entry.file_name().to_string_lossy()).map(|id| {
                GenerationRecord {
                    kind: GenerationKind::DarwinSystem,
                    id,
                    created_at: None,
                    current: current_link_name.is_some_and(|name| name == file_name),
                    label: entry
                        .path()
                        .read_link()
                        .ok()
                        .map(|target| target.to_string_lossy().to_string()),
                }
            })
        })
        .collect::<Vec<_>>();
    generations.sort_by_key(|generation| std::cmp::Reverse(generation.id));
    Ok(generations)
}

fn parse_system_generation_name(name: &str) -> Option<GenerationId> {
    let regex = Regex::new(r"^system-(\d+)-link$").expect("valid regex");
    regex
        .captures(name)
        .and_then(|captures| captures.get(1))
        .and_then(|digits| digits.as_str().parse::<u64>().ok())
        .map(GenerationId::new)
}

fn parse_home_manager_generations(output: &str) -> anyhow::Result<Vec<GenerationRecord>> {
    let regex = Regex::new(r"\bid\s+(\d+)\s+->\s+(\S+)(?:\s+\((current)\))?").expect("valid regex");
    let mut generations = output
        .lines()
        .filter_map(|line| {
            regex.captures(line).and_then(|captures| {
                let id = captures.get(1)?.as_str().parse::<u64>().ok()?;
                let label = captures.get(2)?.as_str().to_string();
                let current = captures.get(3).is_some() || line.contains("(current)");
                Some(GenerationRecord {
                    kind: GenerationKind::HomeManager,
                    id: GenerationId::new(id),
                    created_at: None,
                    current,
                    label: Some(label),
                })
            })
        })
        .collect::<Vec<_>>();

    if output.lines().any(|line| line.contains("id")) && generations.is_empty() {
        bail!("failed to parse home-manager generations output");
    }

    generations.sort_by_key(|generation| std::cmp::Reverse(generation.id));
    Ok(generations)
}

fn parse_disk_usage(output: &str) -> anyhow::Result<DiskUsageSnapshot> {
    let line = output
        .lines()
        .find(|line| line.trim_start().starts_with('/'))
        .ok_or_else(|| anyhow!("df output missing filesystem row"))?;
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 6 {
        bail!("df output row was too short: {line}");
    }

    let mounted_on = parts.last().expect("length already checked").to_string();

    Ok(DiskUsageSnapshot {
        filesystem: parts[0].to_string(),
        size: parts[1].to_string(),
        used: parts[2].to_string(),
        available: parts[3].to_string(),
        capacity: parts[4].to_string(),
        mounted_on,
    })
}

fn generation_id_args(subcommand: &str, ids: &[GenerationId]) -> Vec<String> {
    let mut args = vec![subcommand.to_string()];
    args.extend(ids.iter().copied().map(render_generation_id));
    args
}

fn render_generation_id(id: GenerationId) -> String {
    id.get().to_string()
}

fn first_nonempty_output(output: &crate::infra::shell::CapturedCommand) -> &str {
    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        output.stdout.trim()
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::generations::{RetentionPolicy, plan_prune};
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[test]
    fn discover_darwin_generations_marks_current_from_system_link() {
        let temp = TempDir::new().expect("temp dir");
        let profiles = temp.path().join("profiles");
        fs::create_dir_all(&profiles).expect("profiles dir");

        symlink("/nix/store/system-40", profiles.join("system-40-link")).expect("system link");
        symlink("/nix/store/system-41", profiles.join("system-41-link")).expect("system link");

        let generations = discover_darwin_generations_in_dir(&profiles, Some("system-41-link"))
            .expect("discover darwin");

        assert_eq!(generations.len(), 2);
        assert_eq!(generations[0].id.get(), 41);
        assert!(generations[0].current);
        assert_eq!(generations[1].id.get(), 40);
        assert!(!generations[1].current);
    }

    #[test]
    fn parse_home_manager_generations_extracts_ids_and_current() {
        let generations = parse_home_manager_generations(
            "2026-04-02 13:00 : id 14 -> /nix/store/aaa-home-manager-generation\n\
             2026-04-02 14:00 : id 15 -> /nix/store/bbb-home-manager-generation (current)\n",
        )
        .expect("parse hm");

        assert_eq!(generations.len(), 2);
        assert_eq!(generations[0].id.get(), 15);
        assert!(generations[0].current);
        assert_eq!(generations[1].id.get(), 14);
        assert!(!generations[1].current);
    }

    #[test]
    fn parse_disk_usage_extracts_core_fields() {
        let snapshot = parse_disk_usage(
            "Filesystem      Size    Used   Avail Capacity Mounted on\n\
             /dev/disk3s7   926Gi    81Gi    25Gi    77%   /nix\n",
        )
        .expect("parse disk usage");

        assert_eq!(snapshot.filesystem, "/dev/disk3s7");
        assert_eq!(snapshot.size, "926Gi");
        assert_eq!(snapshot.used, "81Gi");
        assert_eq!(snapshot.available, "25Gi");
        assert_eq!(snapshot.capacity, "77%");
        assert_eq!(snapshot.mounted_on, "/nix");
    }

    #[test]
    fn planned_commands_include_prunes_then_gc() {
        let plan = plan_prune(
            &[
                GenerationRecord {
                    kind: GenerationKind::DarwinSystem,
                    id: GenerationId::new(12),
                    created_at: None,
                    current: false,
                    label: None,
                },
                GenerationRecord {
                    kind: GenerationKind::DarwinSystem,
                    id: GenerationId::new(11),
                    created_at: None,
                    current: false,
                    label: None,
                },
                GenerationRecord {
                    kind: GenerationKind::HomeManager,
                    id: GenerationId::new(4),
                    created_at: None,
                    current: false,
                    label: None,
                },
                GenerationRecord {
                    kind: GenerationKind::HomeManager,
                    id: GenerationId::new(3),
                    created_at: None,
                    current: false,
                    label: None,
                },
            ],
            RetentionPolicy::all(1, true),
        );

        let commands = planned_commands(&plan);

        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0].program, "home-manager");
        assert_eq!(
            commands[0].args,
            vec!["remove-generations".to_string(), "3".to_string()]
        );
        assert_eq!(commands[1].program, "sudo");
        assert_eq!(
            commands[1].args,
            vec![
                "nix-env".to_string(),
                "--delete-generations".to_string(),
                "--profile".to_string(),
                "/nix/var/nix/profiles/system".to_string(),
                "11".to_string(),
            ]
        );
        assert_eq!(
            commands[2].args,
            vec!["nix-collect-garbage".to_string(), "-d".to_string()]
        );
    }

    #[test]
    fn planned_commands_skip_empty_prune_steps() {
        let plan = plan_prune(&[], RetentionPolicy::all(10, false));

        let commands = planned_commands(&plan);

        assert!(commands.is_empty());
    }
}
