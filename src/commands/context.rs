use std::path::{Path, PathBuf};

use crate::domain::config::ConfigFiles;
use crate::domain::drift::ManifestHealth;
use crate::domain::manifest_scan::ScannedRepo;
use crate::output::printer::Printer;

#[derive(Debug, Clone, Copy, Default)]
pub struct GlobalFlags {
    pub json: bool,
}

pub struct RepoContext<'a> {
    pub repo_root: &'a Path,
    pub printer: &'a Printer,
}

pub struct JsonCommandContext<'a> {
    pub repo_root: &'a Path,
    pub printer: &'a Printer,
    flags: GlobalFlags,
}

pub struct InitContext<'a> {
    pub repo_root: &'a Path,
    pub printer: &'a Printer,
    pub scanned_repo: &'a ScannedRepo,
}

pub struct QueryContext<'a> {
    pub repo_root: &'a Path,
    pub printer: &'a Printer,
    pub manifest_health: &'a ManifestHealth,
    flags: GlobalFlags,
}

pub struct SystemContext<'a> {
    pub repo_root: &'a Path,
    pub printer: &'a Printer,
    pub config_files: &'a ConfigFiles,
    pub manifest_health: &'a ManifestHealth,
}

pub struct AppContext {
    pub repo_root: PathBuf,
    pub printer: Printer,
    pub config_files: ConfigFiles,
    pub manifest_health: ManifestHealth,
    pub scanned_repo: ScannedRepo,
    pub flags: GlobalFlags,
}

impl AppContext {
    pub fn new(
        repo_root: PathBuf,
        printer: Printer,
        config_files: ConfigFiles,
        manifest_health: ManifestHealth,
        scanned_repo: ScannedRepo,
        flags: GlobalFlags,
    ) -> Self {
        Self {
            repo_root,
            printer,
            config_files,
            manifest_health,
            scanned_repo,
            flags,
        }
    }

    pub fn require_manifest_system_safe(&self, action: &str) -> Result<(), i32> {
        self.system_context().require_manifest_system_safe(action)
    }

    pub fn repo_context(&self) -> RepoContext<'_> {
        RepoContext {
            repo_root: &self.repo_root,
            printer: &self.printer,
        }
    }

    pub fn json_context(&self) -> JsonCommandContext<'_> {
        JsonCommandContext {
            repo_root: &self.repo_root,
            printer: &self.printer,
            flags: self.flags,
        }
    }

    pub fn init_context(&self) -> InitContext<'_> {
        InitContext {
            repo_root: &self.repo_root,
            printer: &self.printer,
            scanned_repo: &self.scanned_repo,
        }
    }

    pub fn query_context(&self) -> QueryContext<'_> {
        QueryContext {
            repo_root: &self.repo_root,
            printer: &self.printer,
            manifest_health: &self.manifest_health,
            flags: self.flags,
        }
    }

    pub fn system_context(&self) -> SystemContext<'_> {
        SystemContext {
            repo_root: &self.repo_root,
            printer: &self.printer,
            config_files: &self.config_files,
            manifest_health: &self.manifest_health,
        }
    }
}

impl JsonCommandContext<'_> {
    pub const fn wants_json(&self, local_json_flag: bool) -> bool {
        local_json_flag || self.flags.json
    }
}

impl QueryContext<'_> {
    pub const fn wants_json(&self, local_json_flag: bool) -> bool {
        local_json_flag || self.flags.json
    }
}

impl SystemContext<'_> {
    pub fn require_manifest_system_safe(&self, action: &str) -> Result<(), i32> {
        if self.manifest_health.blocks_system_commands() {
            if let Some(err) = self.manifest_health.invalid_error() {
                self.printer.error(&format!(
                    "Cannot {action} while .nx/manifest.toml is unreadable"
                ));
                Printer::detail(&format!("Details: {err}"));
            }
            Printer::detail(
                "Run: nx init --refresh so custom platform settings can be recovered safely",
            );
            return Err(1);
        }
        Ok(())
    }
}
