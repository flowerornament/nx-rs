use super::*;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tempfile::TempDir;

use crate::commands::context::GlobalFlags;
use crate::domain::config::ConfigFiles;
use crate::domain::source::PackageSource;
use crate::infra::ai_engine::RouteDecision;
use crate::output::printer::Printer;
use crate::output::style::OutputStyle;

mod edit;
mod flake_input;
mod post_install;
mod resolution;
mod selection;
mod service;
mod source;

fn source_result(name: &str, source: PackageSource, attr: Option<&str>) -> SourceResult {
    SourceResult {
        name: name.to_string(),
        source,
        attr: attr.map(str::to_string),
        version: None,
        confidence: 1.0,
        description: String::new(),
        requires_flake_mod: false,
        flake_url: None,
    }
}

fn write_nix(root: &Path, rel_path: &str, content: &str) {
    let full = root.join(rel_path);
    fs::create_dir_all(full.parent().expect("nix file should have parent dirs"))
        .expect("nix parent dirs should be created");
    fs::write(full, content).expect("nix content should be written");
}

fn test_context(root: &Path) -> AppContext {
    AppContext::new(
        root.to_path_buf(),
        Printer::new(OutputStyle::from_flags(true, false, false)),
        ConfigFiles::discover(root),
        GlobalFlags::default(),
    )
}

fn test_plan(root: &Path, token: &str) -> InstallPlan {
    InstallPlan {
        source_result: SourceResult::new(token, PackageSource::Nxs),
        package_token: token.to_string(),
        target_file: root.join("packages/nix/cli.nix"),
        insertion_mode: InsertionMode::NixManifest,
        language_info: None,
        routing_warning: None,
    }
}

fn install_args_template() -> InstallArgs {
    InstallArgs {
        packages: vec!["ripgrep".to_string()],
        ..InstallArgs::default()
    }
}

struct StubEngine {
    engine_name: &'static str,
    supports_flake: bool,
    run_edit_calls: Arc<AtomicUsize>,
    run_edit_outcome: CommandOutcome,
}

impl AiEngine for StubEngine {
    fn route_package(
        &self,
        _package: &str,
        _description: &str,
        _context: &str,
        _candidates: &[String],
        fallback: &str,
        _cwd: &Path,
    ) -> RouteDecision {
        RouteDecision {
            target_file: fallback.to_string(),
            warning: None,
        }
    }

    fn run_edit(&self, _prompt: &str, _cwd: &Path) -> CommandOutcome {
        self.run_edit_calls.fetch_add(1, Ordering::SeqCst);
        self.run_edit_outcome.clone()
    }

    fn supports_flake_input(&self) -> bool {
        self.supports_flake
    }

    fn name(&self) -> &'static str {
        self.engine_name
    }
}
