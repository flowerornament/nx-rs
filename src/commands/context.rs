use std::path::PathBuf;

use crate::domain::config::ConfigFiles;
use crate::output::printer::Printer;

pub struct AppContext {
    pub repo_root: PathBuf,
    pub printer: Printer,
    pub config_files: ConfigFiles,
}

impl AppContext {
    pub fn new(repo_root: PathBuf, printer: Printer, config_files: ConfigFiles) -> Self {
        Self {
            repo_root,
            printer,
            config_files,
        }
    }
}
