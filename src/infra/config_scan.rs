use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rnix::ast;
use rowan::ast::AstNode;

use crate::domain::package::{
    DeclarationKind, DeclarationSite, InventoryIssue, PackageDeclaration, PackageInventory,
    PackageSource,
};
use crate::domain::repo_scan::{NixFileScanPolicy, collect_managed_nix_files};

pub fn scan_packages(repo_root: &Path) -> anyhow::Result<PackageInventory> {
    scan_package_files(&collect_nix_files(repo_root), repo_root)
}

fn scan_package_files(files: &[PathBuf], repo_root: &Path) -> anyhow::Result<PackageInventory> {
    let mut inventory = PackageInventory::default();

    for nix_file in files {
        let content = fs::read_to_string(nix_file)
            .with_context(|| format!("reading {}", nix_file.display()))?;
        let relative = nix_file.strip_prefix(repo_root).unwrap_or(nix_file);
        scan_file(&content, relative, &mut inventory);
    }

    Ok(inventory)
}

fn scan_file(content: &str, relative: &Path, inventory: &mut PackageInventory) {
    let parse = rnix::Root::parse(content);
    for error in parse.errors() {
        inventory.issues.push(InventoryIssue {
            location: relative.display().to_string(),
            summary: format!("Nix parse error: {error}"),
        });
    }

    let root = parse.tree();
    let Some(expr) = root.expr() else {
        return;
    };

    let mut recognized_homebrew_manifest = false;
    if let Some(source) = bare_homebrew_source(relative)
        && let ast::Expr::List(list) = &expr
    {
        recognized_homebrew_manifest = true;
        let declaration = if source == PackageSource::Cask {
            DeclarationKind::Application
        } else {
            DeclarationKind::Package
        };
        collect_string_list(list, source, &declaration, content, relative, inventory);
    }

    for descendant in expr.syntax().descendants() {
        let Some(assignment) = ast::AttrpathValue::cast(descendant) else {
            continue;
        };
        let Some(segments) = static_attrpath(&assignment) else {
            continue;
        };
        let segments = segments.iter().map(String::as_str).collect::<Vec<_>>();

        match segments.as_slice() {
            ["home", "packages"] | ["environment", "systemPackages"] => {
                collect_nix_packages(&assignment, content, relative, inventory);
            }
            ["homebrew", "brews"] => {
                recognized_homebrew_manifest = true;
                collect_assigned_string_list(
                    &assignment,
                    PackageSource::Homebrew,
                    &DeclarationKind::Package,
                    content,
                    relative,
                    inventory,
                );
            }
            ["homebrew", "casks"] => {
                recognized_homebrew_manifest = true;
                collect_assigned_string_list(
                    &assignment,
                    PackageSource::Cask,
                    &DeclarationKind::Application,
                    content,
                    relative,
                    inventory,
                );
            }
            ["homebrew", "masApps"] | ["masApps"] => {
                collect_mas_apps(&assignment, content, relative, inventory);
            }
            ["launchd", "agents", name, ..] | ["launchd", "user", "agents", name, ..] => {
                push_declaration(
                    inventory,
                    (*name).to_string(),
                    PackageSource::Service,
                    DeclarationKind::Service,
                    location(content, relative, assignment.syntax()),
                );
            }
            _ => {}
        }
    }

    if bare_homebrew_source(relative).is_some() && !recognized_homebrew_manifest {
        inventory.issues.push(InventoryIssue {
            location: relative.display().to_string(),
            summary: "Homebrew manifest has no statically inspectable package list".to_string(),
        });
    }
}

fn collect_nix_packages(
    assignment: &ast::AttrpathValue,
    content: &str,
    relative: &Path,
    inventory: &mut PackageInventory,
) {
    let Some(value) = assignment.value() else {
        return;
    };
    let Some(list) = package_list(&value) else {
        inventory.issues.push(InventoryIssue {
            location: location(content, relative, assignment.syntax()),
            summary: "package assignment is not a statically inspectable list".to_string(),
        });
        return;
    };

    for item in list.items() {
        collect_nix_item(&item, content, relative, inventory);
    }
}

fn package_list(value: &ast::Expr) -> Option<ast::List> {
    match value {
        ast::Expr::List(list) => Some(list.clone()),
        ast::Expr::With(with) => match with.body()? {
            ast::Expr::List(list) => Some(list),
            _ => None,
        },
        _ => None,
    }
}

fn collect_nix_item(
    item: &ast::Expr,
    content: &str,
    relative: &Path,
    inventory: &mut PackageInventory,
) {
    let item = unparenthesized(item);
    let item_location = location(content, relative, item.syntax());

    if let Some(name) = direct_package_name(&item) {
        push_declaration(
            inventory,
            name,
            PackageSource::Nix,
            DeclarationKind::Package,
            item_location,
        );
        return;
    }

    if let Some(name) = input_package_name(&item) {
        push_declaration(
            inventory,
            name,
            PackageSource::Nix,
            DeclarationKind::ExternalInput,
            item_location,
        );
        return;
    }

    if let Some(name) = wrapped_package_name(&item) {
        push_declaration(
            inventory,
            name,
            PackageSource::Nix,
            DeclarationKind::Package,
            item_location,
        );
        return;
    }

    if let Some(name) = generated_command_name(&item) {
        push_declaration(
            inventory,
            name,
            PackageSource::Nix,
            DeclarationKind::GeneratedCommand,
            item_location,
        );
        return;
    }

    if let Some((runtime, members)) = runtime_members(&item) {
        push_declaration(
            inventory,
            runtime.clone(),
            PackageSource::Nix,
            DeclarationKind::RuntimeEnvironment,
            item_location.clone(),
        );
        for member in members {
            if let Some(name) = member.name {
                push_declaration(
                    inventory,
                    name,
                    PackageSource::Nix,
                    DeclarationKind::RuntimeMember {
                        runtime: runtime.clone(),
                    },
                    item_location.clone(),
                );
            } else {
                inventory.issues.push(InventoryIssue {
                    location: item_location.clone(),
                    summary: format!("opaque runtime member: {}", member.expression),
                });
            }
        }
        return;
    }

    inventory.issues.push(InventoryIssue {
        location: item_location,
        summary: format!(
            "opaque package expression: {}",
            compact_expression(&item.syntax().text().to_string())
        ),
    });
}

fn direct_package_name(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Ident(ident) => Some(ident.ident_token()?.text().to_string()),
        ast::Expr::Select(select) => select_name(select),
        _ => None,
    }
}

fn input_package_name(expr: &ast::Expr) -> Option<String> {
    let ast::Expr::Select(select) = expr else {
        return None;
    };
    let ast::Expr::Ident(base) = select.expr()? else {
        return None;
    };
    if base.ident_token()?.text() != "inputs" {
        return None;
    }

    let attrs = select.attrpath()?.attrs().collect::<Vec<_>>();
    let ast::Attr::Ident(input) = attrs.first()? else {
        return None;
    };
    attrs
        .iter()
        .skip(1)
        .any(|attr| {
            matches!(
                attr,
                ast::Attr::Ident(ident)
                    if ident.ident_token().is_some_and(|token| token.text() == "packages")
            )
        })
        .then(|| input.ident_token().map(|token| token.text().to_string()))
        .flatten()
}

fn wrapped_package_name(expr: &ast::Expr) -> Option<String> {
    let (function, arguments) = flatten_apply(expr);
    let ast::Expr::Select(select) = function else {
        return None;
    };
    let function_name = select_name(&select)?;
    if !matches!(
        function_name.as_str(),
        "lib.setPrio" | "lib.lowPrio" | "lib.hiPrio"
    ) {
        return None;
    }
    direct_package_name(&unparenthesized(arguments.last()?))
}

fn select_name(select: &ast::Select) -> Option<String> {
    let base = match select.expr()? {
        ast::Expr::Ident(ident) => ident.ident_token()?.text().to_string(),
        _ => return None,
    };
    let attrs = static_attrs(&select.attrpath()?)?;
    if attrs.is_empty() {
        return None;
    }
    if matches!(base.as_str(), "pkgs" | "self") {
        Some(attrs.join("."))
    } else {
        Some(
            std::iter::once(base)
                .chain(attrs)
                .collect::<Vec<_>>()
                .join("."),
        )
    }
}

fn generated_command_name(expr: &ast::Expr) -> Option<String> {
    let (function, arguments) = flatten_apply(expr);
    let function_name = function_name(&function)?;
    if !matches!(
        function_name.as_str(),
        "writeShellScriptBin" | "writeShellApplication"
    ) {
        return None;
    }
    literal_string(arguments.first()?)
}

struct RuntimeMember {
    name: Option<String>,
    expression: String,
}

fn runtime_members(expr: &ast::Expr) -> Option<(String, Vec<RuntimeMember>)> {
    let (function, arguments) = flatten_apply(expr);
    let ast::Expr::Select(select) = function else {
        return None;
    };
    let attrs = static_attrs(&select.attrpath()?)?;
    if attrs.last().map(String::as_str) != Some("withPackages") {
        return None;
    }

    let runtime = match select.expr()? {
        ast::Expr::Ident(ident) => {
            let base = ident.ident_token()?.text().to_string();
            let runtime_attrs = &attrs[..attrs.len() - 1];
            if matches!(base.as_str(), "pkgs" | "self") {
                runtime_attrs.join(".")
            } else {
                std::iter::once(base)
                    .chain(runtime_attrs.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(".")
            }
        }
        _ => return None,
    };

    let list = arguments
        .first()?
        .syntax()
        .descendants()
        .find_map(ast::List::cast)?;
    let members = list
        .items()
        .map(|item| RuntimeMember {
            name: direct_package_name(&unparenthesized(&item)),
            expression: compact_expression(&item.syntax().text().to_string()),
        })
        .collect();
    Some((runtime, members))
}

fn flatten_apply(expr: &ast::Expr) -> (ast::Expr, Vec<ast::Expr>) {
    let mut function = expr.clone();
    let mut arguments = Vec::new();
    while let ast::Expr::Apply(apply) = &function {
        if let Some(argument) = apply.argument() {
            arguments.push(argument);
        }
        let Some(lambda) = apply.lambda() else {
            break;
        };
        function = lambda;
    }
    arguments.reverse();
    (function, arguments)
}

fn function_name(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Ident(ident) => Some(ident.ident_token()?.text().to_string()),
        ast::Expr::Select(select) => static_attrs(&select.attrpath()?)?.pop(),
        _ => None,
    }
}

fn unparenthesized(expr: &ast::Expr) -> ast::Expr {
    let mut current = expr.clone();
    while let ast::Expr::Paren(paren) = &current {
        let Some(inner) = paren.expr() else {
            break;
        };
        current = inner;
    }
    current
}

fn collect_assigned_string_list(
    assignment: &ast::AttrpathValue,
    source: PackageSource,
    declaration: &DeclarationKind,
    content: &str,
    relative: &Path,
    inventory: &mut PackageInventory,
) {
    if let Some(ast::Expr::List(list)) = assignment.value() {
        collect_string_list(&list, source, declaration, content, relative, inventory);
    } else {
        inventory.issues.push(InventoryIssue {
            location: location(content, relative, assignment.syntax()),
            summary: "expected a literal package list".to_string(),
        });
    }
}

fn collect_string_list(
    list: &ast::List,
    source: PackageSource,
    declaration: &DeclarationKind,
    content: &str,
    relative: &Path,
    inventory: &mut PackageInventory,
) {
    for item in list.items() {
        let Some(name) = literal_string(&item) else {
            inventory.issues.push(InventoryIssue {
                location: location(content, relative, item.syntax()),
                summary: "non-literal package name".to_string(),
            });
            continue;
        };
        push_declaration(
            inventory,
            name,
            source,
            declaration.clone(),
            location(content, relative, item.syntax()),
        );
    }
}

fn collect_mas_apps(
    assignment: &ast::AttrpathValue,
    content: &str,
    relative: &Path,
    inventory: &mut PackageInventory,
) {
    let Some(ast::Expr::AttrSet(set)) = assignment.value() else {
        inventory.issues.push(InventoryIssue {
            location: location(content, relative, assignment.syntax()),
            summary: "expected a literal MAS application set".to_string(),
        });
        return;
    };
    for descendant in set.syntax().children() {
        let Some(entry) = ast::AttrpathValue::cast(descendant) else {
            continue;
        };
        let Some(name) = static_attrpath(&entry).and_then(|segments| segments.into_iter().next())
        else {
            inventory.issues.push(InventoryIssue {
                location: location(content, relative, entry.syntax()),
                summary: "non-literal MAS application name".to_string(),
            });
            continue;
        };
        push_declaration(
            inventory,
            name,
            PackageSource::Mas,
            DeclarationKind::Application,
            location(content, relative, entry.syntax()),
        );
    }
}

fn literal_string(expr: &ast::Expr) -> Option<String> {
    let ast::Expr::Str(string) = expr else {
        return None;
    };
    let parts = string.normalized_parts();
    if parts.len() != 1 {
        return None;
    }
    match parts.into_iter().next()? {
        ast::InterpolPart::Literal(value) => Some(value),
        ast::InterpolPart::Interpolation(_) => None,
    }
}

fn static_attrpath(assignment: &ast::AttrpathValue) -> Option<Vec<String>> {
    static_attrs(&assignment.attrpath()?)
}

fn static_attrs(attrpath: &ast::Attrpath) -> Option<Vec<String>> {
    attrpath
        .attrs()
        .map(|attr| match attr {
            ast::Attr::Ident(ident) => Some(ident.ident_token()?.text().to_string()),
            ast::Attr::Str(string) => {
                let parts = string.normalized_parts();
                if parts.len() != 1 {
                    return None;
                }
                match parts.into_iter().next()? {
                    ast::InterpolPart::Literal(value) => Some(value),
                    ast::InterpolPart::Interpolation(_) => None,
                }
            }
            ast::Attr::Dynamic(_) => None,
        })
        .collect()
}

fn bare_homebrew_source(path: &Path) -> Option<PackageSource> {
    let parent = path.parent()?.file_name()?.to_str()?;
    let file = path.file_name()?.to_str()?;
    match (parent, file) {
        ("homebrew", "brews.nix") => Some(PackageSource::Homebrew),
        ("homebrew", "casks.nix") => Some(PackageSource::Cask),
        _ => None,
    }
}

fn push_declaration(
    inventory: &mut PackageInventory,
    name: String,
    source: PackageSource,
    declaration: DeclarationKind,
    location: String,
) {
    if !name.is_empty() {
        inventory.push(PackageDeclaration {
            name,
            source,
            sites: vec![DeclarationSite {
                location,
                kind: declaration,
            }],
        });
    }
}

fn location(content: &str, relative: &Path, node: &rnix::SyntaxNode) -> String {
    let offset = u32::from(node.text_range().start()) as usize;
    let line = content[..offset.min(content.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    format!("{}:{line}", relative.display())
}

fn compact_expression(expression: &str) -> String {
    const LIMIT: usize = 80;
    let compact = expression.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= LIMIT {
        compact
    } else {
        compact
            .chars()
            .take(LIMIT - 3)
            .chain("...".chars())
            .collect()
    }
}

pub fn collect_nix_files(repo_root: &Path) -> Vec<PathBuf> {
    collect_managed_nix_files(repo_root, NixFileScanPolicy::for_package_scan())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::scan_packages;
    use crate::domain::package::{DeclarationKind, PackageSource};

    fn write(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture path should have a parent"))
            .expect("fixture parent should be created");
        fs::write(path, content).expect("fixture should be written");
    }

    use std::path::Path;

    #[test]
    fn scans_static_packages_without_shell_body_artifacts() {
        let temp = TempDir::new().expect("temp dir should be created");
        write(
            temp.path(),
            "home/packages.nix",
            r#"{ pkgs, ... }: {
  home.packages = with pkgs; [
    jq
    pkgs.mosh
    (writeShellScriptBin "demo" ''
      set -euo pipefail
      workdir="$HOME/demo"
      if test -d "$workdir"; then echo ok; fi
    '')
  ];
}"#,
        );

        let inventory = scan_packages(temp.path()).expect("scan should succeed");
        let names = inventory
            .declarations
            .iter()
            .map(|declaration| declaration.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["jq", "mosh", "demo"]);
        assert!(inventory.issues.is_empty());
    }

    #[test]
    fn classifies_runtime_members_without_treating_them_as_commands() {
        let temp = TempDir::new().expect("temp dir should be created");
        write(
            temp.path(),
            "home/languages.nix",
            r"{ pkgs, ... }: {
  home.packages = [
    (pkgs.python3.withPackages (ps: with ps; [ pyyaml rich ]))
  ];
}",
        );

        let inventory = scan_packages(temp.path()).expect("scan should succeed");
        assert_eq!(inventory.declarations.len(), 3);
        assert_eq!(
            inventory.declarations[0].sites[0].kind,
            DeclarationKind::RuntimeEnvironment
        );
        assert!(matches!(
            inventory.declarations[1].sites[0].kind,
            DeclarationKind::RuntimeMember { ref runtime } if runtime == "python3"
        ));
    }

    #[test]
    fn records_opaque_package_expressions_as_inventory_issues() {
        let temp = TempDir::new().expect("temp dir should be created");
        write(
            temp.path(),
            "home/packages.nix",
            "{ ... }: { home.packages = [ (if true then foo else bar) ]; }",
        );

        let inventory = scan_packages(temp.path()).expect("scan should succeed");
        assert!(inventory.declarations.is_empty());
        assert_eq!(inventory.issues.len(), 1);
        assert!(
            inventory.issues[0]
                .summary
                .starts_with("opaque package expression:")
        );
    }

    #[test]
    fn records_non_list_package_assignments_as_inventory_issues() {
        let temp = TempDir::new().expect("temp dir should be created");
        write(
            temp.path(),
            "home/packages.nix",
            "{ ... }: { home.packages = someComputedPackages; }",
        );

        let inventory = scan_packages(temp.path()).expect("scan should succeed");
        assert!(inventory.declarations.is_empty());
        assert_eq!(
            inventory.issues[0].summary,
            "package assignment is not a statically inspectable list"
        );
    }

    #[test]
    fn records_opaque_runtime_members_as_inventory_issues() {
        let temp = TempDir::new().expect("temp dir should be created");
        write(
            temp.path(),
            "home/packages.nix",
            r"{ pkgs, ... }: {
  home.packages = [
    (pkgs.python3.withPackages (ps: [ (if true then ps.rich else ps.pyyaml) ]))
  ];
}",
        );

        let inventory = scan_packages(temp.path()).expect("scan should succeed");
        assert!(inventory.declarations.iter().any(|declaration| {
            declaration.sites[0].kind == DeclarationKind::RuntimeEnvironment
        }));
        assert!(
            inventory
                .issues
                .iter()
                .any(|issue| issue.summary.starts_with("opaque runtime member:"))
        );
    }

    #[test]
    fn scans_bare_homebrew_manifests_and_services() {
        let temp = TempDir::new().expect("temp dir should be created");
        write(
            temp.path(),
            "packages/homebrew/brews.nix",
            "[ \"jq\" \"graphviz\" ]",
        );
        write(
            temp.path(),
            "packages/homebrew/casks.nix",
            "[ \"ghostty\" ]",
        );
        write(
            temp.path(),
            "home/services.nix",
            "{ ... }: { launchd.agents.sync.config.Program = \"sync\"; }",
        );

        let inventory = scan_packages(temp.path()).expect("scan should succeed");
        assert!(inventory.declarations.iter().any(|declaration| {
            declaration.name == "jq" && declaration.source == PackageSource::Homebrew
        }));
        assert!(inventory.declarations.iter().any(|declaration| {
            declaration.name == "ghostty" && declaration.source == PackageSource::Cask
        }));
        assert!(inventory.declarations.iter().any(|declaration| {
            declaration.name == "sync" && declaration.source == PackageSource::Service
        }));
    }
}
