use super::*;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tempfile::TempDir;

use crate::domain::config::ConfigFiles;
use crate::domain::drift::ManifestHealth;
use crate::domain::manifest_scan::scan_repo;
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

const CLI_NIX_PATH: &str = "packages/nix/cli.nix";
const SERVICES_NIX_PATH: &str = "home/services.nix";
const DEFAULT_CLI_NIX: &str =
    "{ pkgs, ... }:\n{\n  home.packages = with pkgs; [\n    bat\n  ];\n}\n";
const SERVICES_NIX: &str = "# nx: services\n{}\n";

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

fn temp_root() -> TempDir {
    TempDir::new().expect("temp dir should be created")
}

fn setup_install_root(cli_content: &str) -> TempDir {
    let tmp = temp_root();
    write_nix(tmp.path(), CLI_NIX_PATH, cli_content);
    tmp
}

fn setup_services_root() -> TempDir {
    let tmp = temp_root();
    write_nix(tmp.path(), SERVICES_NIX_PATH, SERVICES_NIX);
    tmp
}

fn test_context(root: &Path) -> AppContext {
    AppContext::new(
        root.to_path_buf(),
        Printer::new(OutputStyle::from_flags(true, false, false)),
        ConfigFiles::discover(root),
        ManifestHealth::Missing,
        scan_repo(root),
    )
}

fn test_plan(root: &Path, token: &str) -> InstallPlan {
    InstallPlan {
        source_result: SourceResult::new(token, PackageSource::Nxs),
        target_file: root.join("packages/nix/cli.nix"),
        edit: EditSpec::nix_packages(token),
        routing_warning: None,
    }
}

fn test_routing_context() -> InstallRoutingContext {
    InstallRoutingContext {
        base: "routing".to_string(),
        enriched: None,
        candidates: Vec::new(),
    }
}

fn flake_input_plan(root: &Path, token: &str, flake_url: Option<&str>) -> InstallPlan {
    let mut plan = test_plan(root, token);
    plan.source_result.requires_flake_mod = true;
    plan.source_result.flake_url = flake_url.map(str::to_string);
    plan
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

fn stub_engine(
    engine_name: &'static str,
    supports_flake: bool,
    success: bool,
    output: &str,
) -> (StubEngine, Arc<AtomicUsize>) {
    let run_edit_calls = Arc::new(AtomicUsize::new(0));
    (
        StubEngine {
            engine_name,
            supports_flake,
            run_edit_calls: Arc::clone(&run_edit_calls),
            run_edit_outcome: CommandOutcome {
                success,
                output: output.to_string(),
            },
        },
        run_edit_calls,
    )
}
