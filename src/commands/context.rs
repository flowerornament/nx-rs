use std::path::PathBuf;

use crate::output::printer::Printer;

pub struct AppContext {
    pub repo_root: PathBuf,
    pub printer: Printer,
}

impl AppContext {
    pub fn new(repo_root: PathBuf, printer: Printer) -> Self {
        Self { repo_root, printer }
    }
}
