use std::path::PathBuf;

use crate::domain::config::ConfigFiles;
use crate::domain::drift::ManifestHealth;
use crate::output::printer::Printer;

#[derive(Debug, Clone, Copy, Default)]
pub struct GlobalFlags {
    pub json: bool,
}

pub struct AppContext {
    pub repo_root: PathBuf,
    pub printer: Printer,
    pub config_files: ConfigFiles,
    pub manifest_health: ManifestHealth,
    pub flags: GlobalFlags,
}

impl AppContext {
    pub fn new(
        repo_root: PathBuf,
        printer: Printer,
        config_files: ConfigFiles,
        manifest_health: ManifestHealth,
        flags: GlobalFlags,
    ) -> Self {
        Self {
            repo_root,
            printer,
            config_files,
            manifest_health,
            flags,
        }
    }

    pub const fn wants_json(&self, local_json_flag: bool) -> bool {
        local_json_flag || self.flags.json
    }

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
