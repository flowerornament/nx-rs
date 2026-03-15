use std::path::PathBuf;

use crate::domain::config::ConfigFiles;
use crate::domain::drift::{DriftReport, ManifestHealth, format_issue};
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

    pub fn require_manifest_write_safe(&self, action: &str) -> Result<(), i32> {
        match &self.manifest_health {
            ManifestHealth::Invalid(err) => {
                self.printer.error(&format!(
                    "Cannot {action} while .nx/manifest.toml is unreadable"
                ));
                Printer::detail(&format!("Details: {err}"));
                Printer::detail("Run: nx init --refresh");
                Err(1)
            }
            ManifestHealth::Drifted { report, .. } => {
                self.printer
                    .error(&format!("Cannot {action} while manifest drift is detected"));
                render_drift_hint(report);
                Err(1)
            }
            ManifestHealth::Missing | ManifestHealth::InSync(_) => Ok(()),
        }
    }
}

fn render_drift_hint(report: &DriftReport) {
    for issue in report.issues.iter().take(3) {
        Printer::detail(&format!("- {}", format_issue(issue)));
    }
    if report.issues.len() > 3 {
        Printer::detail(&format!(
            "... and {} more issue(s)",
            report.issues.len() - 3
        ));
    }
    Printer::detail("Run: nx init --refresh");
}
