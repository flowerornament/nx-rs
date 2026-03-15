use super::*;
use std::cell::Cell;
use std::fs;

use tempfile::TempDir;

use crate::domain::source::PackageSource;

mod cache;
mod flakehub;
mod formatting;

fn info_args() -> InfoArgs {
    InfoArgs {
        package: Some("ripgrep".to_string()),
        json: true,
        bleeding_edge: false,
        verbose: false,
    }
}

fn source_result(
    name: &str,
    source: PackageSource,
    attr: Option<&str>,
    confidence: f64,
) -> SourceResult {
    SourceResult {
        name: name.to_string(),
        source,
        attr: attr.map(str::to_string),
        version: Some("1.2.3".to_string()),
        confidence,
        description: "desc".to_string(),
        requires_flake_mod: false,
        flake_url: None,
    }
}

fn write_flake_lock(root: &Path) {
    let lock = serde_json::json!({
        "nodes": {
            "root": {"inputs": {"nixpkgs": "nixpkgs"}},
            "nixpkgs": {"locked": {"rev": "abcdef1234567890"}}
        }
    });
    fs::write(
        root.join("flake.lock"),
        serde_json::to_string(&lock).unwrap(),
    )
    .expect("flake.lock should be written");
}

fn package_from_args(args: &InfoArgs) -> &str {
    args.package
        .as_deref()
        .expect("info args in tests should include package")
}
