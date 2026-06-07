use std::path::{Path, PathBuf};

use crate::domain::config::ConfigFiles;
use crate::domain::drift::ManifestHealth;
use crate::domain::manifest::Manifest;
use crate::domain::manifest_scan::ScannedRepo;
use crate::output::printer::Printer;

pub struct RepoContext<'a> {
    pub repo_root: &'a Path,
    pub printer: &'a Printer,
}

pub struct HostContext<'a> {
    pub printer: &'a Printer,
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
    pub manifest: Option<&'a Manifest>,
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
}

impl AppContext {
    pub fn new(
        repo_root: PathBuf,
        printer: Printer,
        config_files: ConfigFiles,
        manifest_health: ManifestHealth,
        scanned_repo: ScannedRepo,
    ) -> Self {
        Self {
            repo_root,
            printer,
            config_files,
            manifest_health,
            scanned_repo,
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
            manifest: self.manifest_health.routing_manifest(),
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

impl HostContext<'_> {
    pub const fn new(printer: &Printer) -> HostContext<'_> {
        HostContext { printer }
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
