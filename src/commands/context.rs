use std::path::PathBuf;

use crate::domain::config::ConfigFiles;
use crate::output::printer::Printer;

#[derive(Debug, Clone, Copy, Default)]
pub struct GlobalFlags {
    pub json: bool,
}

pub struct AppContext {
    pub repo_root: PathBuf,
    pub printer: Printer,
    pub config_files: ConfigFiles,
    pub flags: GlobalFlags,
}

impl AppContext {
    pub const fn new(
        repo_root: PathBuf,
        printer: Printer,
        config_files: ConfigFiles,
        flags: GlobalFlags,
    ) -> Self {
        Self {
            repo_root,
            printer,
            config_files,
            flags,
        }
    }

    pub const fn wants_json(&self, local_json_flag: bool) -> bool {
        local_json_flag || self.flags.json
    }
}
