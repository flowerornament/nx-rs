"""
commands.py - CLI command implementations for nx.

Each function handles a specific subcommand (install, remove, where, etc.).
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import TYPE_CHECKING, Any

from claude_ops import remove_line_directly, remove_via_claude
from config import ConfigFiles
from finder import find_all_packages, find_package, find_package_fuzzy
from installed import detect_installed_source
from search import (
    filter_installable,
    install_packages,
    install_packages_turbo,
    run_post_install_actions,
    search_packages,
)
from shared import (
    format_info_source_label,
    format_source_display,
    install_hint_for_source,
    normalize_source_filter,
    relative_path,
    run_command,
    run_streaming_command,
    split_location,
    valid_source_filters,
)
from sources import (
    FlakeHubResult,
    PackageInfo,
    SourcePreferences,
    get_darwin_service_info,
    get_hm_module_info,
    get_package_info,
    search_flakehub,
)
from upgrade import (
    diff_locks,
    enrich_package_info,
    fetch_all_brew_changelogs,
    fetch_all_changes,
    generate_commit_message,
    get_outdated,
    load_flake_lock,
    short_rev,
    stream_nix_update,
    summarize_brew_change,
    summarize_change,
)

if TYPE_CHECKING:
    from cache import MultiSourceCache
    from nx_printer import NxPrinter as Printer


def cmd_install(
    args: Any,
    printer: Printer,
    repo_root: Path,
    config_files: ConfigFiles,
    cache: MultiSourceCache,
    source_prefs: SourcePreferences,
) -> int:
    """Install packages."""
    if not args.packages:
        printer.error("No packages specified")
        return 1

    pkg_count = len(args.packages)
    pkg_list = ", ".join(args.packages[:3])
    if pkg_count > 3:
        pkg_list += f", ... ({pkg_count} total)"

    # Show dry-run banner if applicable
    if args.dry_run:
        printer.dry_run_banner()

    printer.action(f"Installing {pkg_list}")

    # Search for packages
    results, alternatives = search_packages(
        args.packages, args, printer, repo_root, config_files, cache, source_prefs
    )

    # Filter and confirm (with source selection if alternatives exist)
    to_install = filter_installable(results, printer, args, alternatives)

    if not to_install:
        return 0

    engine = getattr(args, "engine", "codex")

    if engine == "codex":
        success_count = install_packages_turbo(
            to_install, config_files, repo_root, printer, args, alternatives
        )
    else:
        success_count = install_packages(
            to_install, config_files, repo_root, printer, args, alternatives
        )

    # Post-install actions
    run_post_install_actions(success_count, repo_root, args, printer)

    return 0 if success_count == len(to_install) else 1


def cmd_remove(
    args: Any,
    printer: Printer,
    repo_root: Path,
    config_files: ConfigFiles,
) -> int:
    """Remove packages."""
    if not args.packages:
        printer.error("No packages specified")
        return 1

    # Show dry-run banner if applicable
    if args.dry_run:
        printer.dry_run_banner()

    for name in args.packages:
        location = find_package(name, config_files)
        if not location:
            printer.not_found(name, [f"Check installed: nx list | grep -i {name}"])
            continue

        _announce_removal(name, location, printer, repo_root)
        file_path, line_num = split_location(location)

        if args.dry_run:
            _handle_remove_dry_run(name, file_path, line_num, printer)
            continue

        if not _confirm_remove(name, line_num, file_path, printer, args.yes):
            continue

        _perform_remove(
            name,
            file_path,
            line_num,
            repo_root,
            printer,
            getattr(args, "model", None),
        )

    return 0


def _announce_removal(name: str, location: str, printer: Printer, repo_root: Path) -> None:
    short_loc = relative_path(location, repo_root)
    printer.action(f"Removing {name}")
    printer.location(short_loc)


def _handle_remove_dry_run(
    name: str,
    file_path: str,
    line_num: int | None,
    printer: Printer,
) -> None:
    if line_num:
        printer.show_removal_preview(file_path, line_num, context=1)
    printer.result_remove(f"Would remove {name}")


def _confirm_remove(
    name: str,
    line_num: int | None,
    file_path: str,
    printer: Printer,
    auto_yes: bool,
) -> bool:
    if line_num:
        printer.show_snippet(file_path, line_num, context=1, mode="remove")

    if auto_yes:
        return True

    print()
    if not printer.confirm(f"Remove {name}?", default=False):
        printer.line("Cancelled.")
        return False
    return True


def _perform_remove(
    name: str,
    file_path: str,
    line_num: int | None,
    repo_root: Path,
    printer: Printer,
    model: str | None,
) -> None:
    if line_num:
        printer.activity("editing", Path(file_path).name)
        ok, msg = remove_line_directly(file_path, line_num)
        if ok:
            printer.complete(f"{name} removed from {Path(file_path).name}")
        else:
            printer.error(f"Failed to remove {name}: {msg}")
        return

    _, before_diff = run_command(["git", "diff"], cwd=repo_root)
    printer.activity("analyzing", f"Removing {name}")
    ok, msg = remove_via_claude(name, file_path, repo_root, printer=printer, dry_run=False, model=model)
    if ok:
        _, after_diff = run_command(["git", "diff"], cwd=repo_root)
        if after_diff != before_diff:
            printer.complete(f"{name} removed from {Path(file_path).name}")
        else:
            printer.warn(f"No changes made for {name}")
    else:
        printer.error(f"Failed to remove {name}: {msg}")


def cmd_where(args: Any, printer: Printer, config_files: ConfigFiles) -> int:
    """Find where a package is configured."""
    if not args.packages:
        printer.error("No package specified")
        return 1

    repo_root = config_files.repo_root
    name = args.packages[0]
    location = find_package(name, config_files)

    if location:
        short_loc = relative_path(location, repo_root)
        printer.success(f"{name} at {short_loc}")
        # Parse file:line format and show snippet
        file_path, line_num = split_location(location)
        if line_num:
            printer.show_snippet(file_path, line_num, context=2)
    else:
        printer.not_found(name, [f"Try: nx info {name}"])

    return 0


def cmd_list(args: Any, printer: Printer, config_files: ConfigFiles) -> int:
    """List all installed packages."""
    packages = find_all_packages(config_files)

    # Filter by source if specified
    if args.list_source:
        source_key = normalize_source_filter(args.list_source)
        if source_key:
            packages = {source_key: packages.get(source_key, [])}
        else:
            printer.error(f"Unknown source: {args.list_source}")
            printer.info(f"Valid sources: {', '.join(valid_source_filters())}")
            return 1

    if _render_list_simple_outputs(args, printer, packages):
        return 0

    _render_list_verbose(args, printer, packages, config_files)

    return 0


def _render_list_simple_outputs(
    args: Any,
    printer: Printer,
    packages: dict[str, list[str]],
) -> bool:
    if args.json:
        print(json.dumps(packages, indent=2))
        return True

    if args.plain:
        for pkgs in packages.values():
            for pkg in sorted(pkgs):
                print(f"{printer.INDENT}{pkg}")
        return True

    return False


def _list_header_subtitle(total: int, list_source: str | None) -> str:
    if list_source:
        return f"{total} packages from {list_source}"
    return f"{total} packages installed"


def _calc_list_max_width(packages: dict[str, list[str]]) -> int:
    max_width = 20
    for pkgs in packages.values():
        for pkg in pkgs:
            max_width = max(max_width, len(pkg))
    return max_width + 2


def _render_list_verbose(
    args: Any,
    printer: Printer,
    packages: dict[str, list[str]],
    config_files: ConfigFiles,
) -> None:
    total = sum(len(pkgs) for pkgs in packages.values())
    subtitle = _list_header_subtitle(total, args.list_source)
    printer.command_header("Installed Packages", subtitle)

    max_width = _calc_list_max_width(packages) if args.verbose else 20

    for source, pkgs in packages.items():
        if not pkgs:
            continue
        display_name = format_source_display(source)
        printer.section(display_name, len(pkgs))
        if args.verbose:
            for pkg in sorted(pkgs):
                loc = find_package(pkg, config_files)
                if loc:
                    short_loc = relative_path(loc, config_files.repo_root)
                    printer.dim(f"{pkg:{max_width}} {short_loc}")
                else:
                    printer.line(pkg)
        else:
            printer.multi_column_list(pkgs)


def _resolve_installed_status(
    name: str,
    config_files: ConfigFiles,
    repo_root: Path,
) -> tuple[str | None, str | None, str | None]:
    location = find_package(name, config_files)
    installed_source = None
    active_overlay = None
    if location:
        installed_source, active_overlay = detect_installed_source(
            location, name, repo_root
        )
    return location, installed_source, active_overlay


def _fetch_package_infos(
    name: str,
    repo_root: Path,
    printer: Printer,
) -> list[PackageInfo]:
    flake_lock = repo_root / "flake.lock"
    with printer.status(f"Fetching info for {name}..."):
        return get_package_info(name, flake_lock_path=flake_lock)


def _build_info_json(
    name: str,
    location: str | None,
    infos: list[PackageInfo],
    repo_root: Path,
    *,
    include_flakehub: bool,
) -> dict[str, Any]:
    hm_info = get_hm_module_info(name, repo_root)
    darwin_info = get_darwin_service_info(name, repo_root)
    flakehub_results = search_flakehub(name) if include_flakehub else []

    return {
        "name": name,
        "installed": location is not None,
        "location": location,
        "sources": [
            {
                "source": info.source,
                "version": info.version,
                "description": info.description,
                "homepage": info.homepage,
                "license": info.license,
                "dependencies": info.dependencies,
                "build_dependencies": info.build_dependencies,
                "caveats": info.caveats,
                "artifacts": info.artifacts,
                "broken": info.broken,
                "insecure": info.insecure,
                "head_available": info.head_available,
            }
            for info in infos
        ],
        "hm_module": {
            "path": hm_info.module_path,
            "example": hm_info.example_config,
            "enabled": hm_info.is_enabled,
        } if hm_info else None,
        "darwin_service": {
            "path": darwin_info.service_path,
            "example": darwin_info.example_config,
            "enabled": darwin_info.is_enabled,
        } if darwin_info else None,
        "flakehub": [
            {
                "name": fh.flake_name,
                "description": fh.description,
                "version": fh.version,
            }
            for fh in flakehub_results[:3]
        ] if flakehub_results else [],
    }


def _format_status_text(
    location: str | None,
    installed_source: str | None,
    active_overlay: str | None,
) -> str:
    if not location:
        return "not installed"
    if active_overlay:
        return f"installed via {active_overlay}"
    return f"installed ({installed_source})"


def _show_install_location(
    location: str | None,
    printer: Printer,
    repo_root: Path,
) -> None:
    if not location:
        return
    short_loc = relative_path(location, repo_root)
    printer.location(short_loc)
    file_path, line_num = split_location(location)
    if line_num:
        printer.show_snippet(file_path, line_num, context=1)


def _render_source_infos(
    infos: list[PackageInfo],
    printer: Printer,
    installed_source: str | None,
) -> None:
    for info in infos:
        _render_info_header(info, printer, installed_source)
        _render_info_metadata(info, printer)
        _render_info_warnings(info, printer)
        _render_info_dependencies(info, printer)
        _render_info_artifacts(info, printer)
        _render_info_caveats(info, printer)


def _render_info_header(
    info: PackageInfo,
    printer: Printer,
    installed_source: str | None,
) -> None:
    source_label = format_info_source_label(info.source, info.name)
    is_current = (installed_source == info.source) or (
        installed_source == "nxs" and info.source == "nxs"
    )
    if is_current:
        printer.section(source_label, tag="current")
    else:
        printer.section(source_label)


def _render_info_metadata(info: PackageInfo, printer: Printer) -> None:
    if info.version:
        printer.line(f"{'Version:':<13} {info.version}")
    if info.description:
        printer.line(f"{'Description:':<13} {info.description}")
    if info.homepage:
        printer.line(f"{'Homepage:':<13} {info.homepage}")
    if info.license:
        printer.line(f"{'License:':<13} {info.license}")
    if info.changelog:
        printer.line(f"{'Changelog:':<13} {info.changelog}")
    if info.head_available:
        printer.line(f"{'HEAD build:':<13} Available (brew install --HEAD)")


def _render_info_warnings(info: PackageInfo, printer: Printer) -> None:
    if info.broken:
        printer.warn("This package is marked as BROKEN")
    if info.insecure:
        printer.warn("This package is marked as INSECURE")


def _render_info_dependencies(info: PackageInfo, printer: Printer) -> None:
    if info.dependencies:
        deps = info.dependencies[:10]
        more = f" (+{len(info.dependencies) - 10} more)" if len(info.dependencies) > 10 else ""
        printer.line(f"{'Dependencies:':<13} {', '.join(deps)}{more}")

    if info.build_dependencies:
        bdeps = info.build_dependencies[:5]
        more = f" (+{len(info.build_dependencies) - 5} more)" if len(info.build_dependencies) > 5 else ""
        printer.line(f"{'Build deps:':<13} {', '.join(bdeps)}{more}")


def _render_info_artifacts(info: PackageInfo, printer: Printer) -> None:
    if info.artifacts:
        printer.line(f"{'Installs:':<13} {', '.join(info.artifacts[:3])}")


def _render_info_caveats(info: PackageInfo, printer: Printer) -> None:
    if not info.caveats:
        return
    print()
    printer.line("Caveats:")
    for line in info.caveats.strip().split("\n")[:5]:
        printer.detail(line)
    if info.caveats.count("\n") > 5:
        printer.detail("...")


def _render_hm_info(name: str, repo_root: Path, printer: Printer) -> None:
    hm_info = get_hm_module_info(name, repo_root)
    if not hm_info:
        return
    tag = "enabled" if hm_info.is_enabled else ""
    printer.section("Home-manager module", tag=tag)
    printer.dim(f"Module: {hm_info.module_path}")
    printer.dim(f"Example: {hm_info.example_config}")


def _render_darwin_info(name: str, repo_root: Path, printer: Printer) -> None:
    darwin_info = get_darwin_service_info(name, repo_root)
    if not darwin_info:
        return
    tag = "enabled" if darwin_info.is_enabled else ""
    printer.section("nix-darwin service", tag=tag)
    printer.dim(f"Service: {darwin_info.service_path}")
    printer.dim(f"Example: {darwin_info.example_config}")


def _render_flakehub_results(flakehub_results: list[FlakeHubResult], printer: Printer) -> None:
    if not flakehub_results:
        return
    printer.section("FlakeHub flakes", len(flakehub_results))
    for fh in flakehub_results[:3]:
        printer.line(fh.flake_name)
        if fh.description:
            printer.detail(f"{fh.description[:60]}...")
        if fh.version:
            printer.detail(f"Latest: {fh.version}")
    print()
    first = flakehub_results[0]
    flake_short = first.flake_name.split("/")[1]
    printer.dim("To use a FlakeHub flake, add to flake.nix inputs:")
    printer.detail(f"{flake_short} = {{")
    printer.detail(f'  url = "https://flakehub.com/f/{first.flake_name}/*.tar.gz";')
    printer.detail("};")
    print()
    printer.detail(f"Then use: inputs.{flake_short}.packages.${{system}}.default")


def _render_install_hints(
    name: str,
    infos: list[PackageInfo],
    location: str | None,
    printer: Printer,
) -> None:
    if location or len(infos) <= 1:
        return
    printer.info("Available from multiple sources. Install with:")
    for info in infos:
        hint = install_hint_for_source(name, info.source)
        if hint:
            printer.detail(hint)


def cmd_info(
    args: Any,
    printer: Printer,
    config_files: ConfigFiles,
    repo_root: Path,
) -> int:
    """Show detailed information about a package.

    Displays:
    - Installation status and location
    - Version, description, homepage
    - License, dependencies
    - Source-specific info (caveats, artifacts, etc.)
    - Bleeding-edge alternatives (flake overlays, NUR, HEAD builds)
    """
    if not args.packages:
        printer.error("No package specified")
        printer.info("Usage: nx info <package>")
        return 1

    name = args.packages[0]

    location, installed_source, active_overlay = _resolve_installed_status(
        name, config_files, repo_root
    )
    infos = _fetch_package_infos(name, repo_root, printer)
    include_flakehub = bool(getattr(args, "bleeding_edge", False))

    if args.json:
        output = _build_info_json(
            name,
            location,
            infos,
            repo_root,
            include_flakehub=include_flakehub,
        )
        print(json.dumps(output, indent=2))
        return 0

    status_text = _format_status_text(location, installed_source, active_overlay)
    printer.command_header(name, status_text)

    _show_install_location(location, printer, repo_root)

    flakehub_results: list = search_flakehub(name) if include_flakehub else []

    if not infos and not flakehub_results:
        printer.not_found(name, [f"Try: nx {name}"])
        return 0

    _render_source_infos(infos, printer, installed_source)
    _render_hm_info(name, repo_root, printer)
    _render_darwin_info(name, repo_root, printer)

    _render_flakehub_results(flakehub_results, printer)
    _render_install_hints(name, infos, location, printer)

    return 0


def cmd_status(printer: Printer, config_files: ConfigFiles) -> int:
    """Show package distribution."""
    packages = find_all_packages(config_files)
    total = sum(len(pkgs) for pkgs in packages.values())

    printer.command_header("Package Status", f"{total} packages installed")
    printer.status_table(packages)

    return 0


def _collect_installed_results(
    names: list[str],
    config_files: ConfigFiles,
) -> dict[str, tuple[str | None, str | None]]:
    results: dict[str, tuple[str | None, str | None]] = {}
    for name in names:
        matched_name, location = find_package_fuzzy(name, config_files)
        results[name] = (matched_name, location)
    return results


def _render_installed_json(
    results: dict[str, tuple[str | None, str | None]],
) -> int:
    json_results: dict[str, dict[str, str | None]] = {}
    for query, (matched, loc) in results.items():
        if loc:
            json_results[query] = {"match": matched, "location": loc}
        else:
            json_results[query] = {"match": None, "location": None}
    print(json.dumps(json_results))
    return 0 if all(v[1] for v in results.values()) else 1


def _render_single_installed(
    name: str,
    results: dict[str, tuple[str | None, str | None]],
    printer: Printer,
    config_files: ConfigFiles,
    show_location: bool,
) -> int:
    matched, loc = results[name]
    if not loc:
        return 1

    if show_location:
        short_loc = relative_path(loc, config_files.repo_root)
        if matched != name:
            printer.success(
                f"{name} → {matched} [dim]({short_loc})[/dim]"
                if printer.has_rich
                else f"{name} → {matched} ({short_loc})"
            )
        else:
            printer.success(
                f"{matched} [dim]({short_loc})[/dim]"
                if printer.has_rich
                else f"{matched} ({short_loc})"
            )
    return 0


def _render_multi_installed(
    results: dict[str, tuple[str | None, str | None]],
    printer: Printer,
    config_files: ConfigFiles,
) -> int:
    installed_count = sum(1 for _, (_, loc) in results.items() if loc)
    printer.command_header("Package Check", f"{installed_count}/{len(results)} installed")

    all_installed = True
    for query, (matched, loc) in results.items():
        if loc:
            short_loc = relative_path(loc, config_files.repo_root)
            if matched != query:
                printer.success(f"{query} → {matched}")
            else:
                printer.success(query)
            printer.dim(f"  {short_loc}")
        else:
            printer.not_installed(query, install_hint=False)
            all_installed = False

    return 0 if all_installed else 1


def cmd_installed(args: Any, printer: Printer, config_files: ConfigFiles) -> int:
    """Check if package(s) are installed.

    Returns exit code 0 if all packages are installed, 1 if any are not.
    Uses fuzzy matching: lua finds lua5_4, python finds python3, etc.
    """
    if not args.packages:
        printer.error("No package specified")
        printer.info("Usage: nx installed <package> [package2 ...]")
        return 1

    results = _collect_installed_results(args.packages, config_files)

    if args.json:
        return _render_installed_json(results)

    if len(args.packages) == 1:
        return _render_single_installed(
            args.packages[0],
            results,
            printer,
            config_files,
            getattr(args, "show_location", False),
        )

    return _render_multi_installed(results, printer, config_files)


def cmd_undo(printer: Printer, repo_root: Path) -> int:
    """Undo last changes via git."""
    # Check git status
    _success, output = run_command(["git", "status", "--porcelain"], cwd=repo_root)

    if not output:
        print()
        printer.line("Nothing to undo.")
        return 0

    modified = [line[3:] for line in output.split("\n") if line.startswith(" M")]

    if not modified:
        print()
        printer.line("Nothing to undo.")
        return 0

    printer.command_header("Undo Changes", f"{len(modified)} files")

    # Show what changed in each file
    for f in modified:
        printer.line(f)
        # Get short diff for this file
        _, diff_output = run_command(["git", "diff", "--stat", f], cwd=repo_root)
        if diff_output:
            # Show just the summary line (e.g., "1 file changed, 2 insertions(+), 1 deletion(-)")
            for line in diff_output.strip().split("\n"):
                if "insertion" in line or "deletion" in line or "changed" in line:
                    printer.detail(line.strip())
                    break

    print()
    if not printer.confirm("Revert all changes?", default=False):
        printer.line("Cancelled.")
        return 0

    # Revert
    for f in modified:
        run_command(["git", "checkout", "--", f], cwd=repo_root)

    printer.complete(f"Reverted {len(modified)} files")
    return 0


# ═══════════════════════════════════════════════════════════════════════════════
# System Commands (update, switch, upgrade)
# ═══════════════════════════════════════════════════════════════════════════════

DARWIN_REBUILD = "/run/current-system/sw/bin/darwin-rebuild"


def _run_indented(
    cmd: list[str],
    indent: str = "  ",
    cwd: Path | None = None,
    printer: Printer | None = None,
) -> int:
    """Run a command with consistently indented, wrapped streaming output."""
    returncode, _ = run_streaming_command(cmd, cwd=cwd, printer=printer, indent=indent)
    return returncode


def _find_untracked_nix_files(repo_root: Path) -> tuple[bool, list[str], str | None]:
    """Find untracked .nix files that flake evaluation would ignore."""
    result = subprocess.run(
        [
            "git",
            "-C",
            str(repo_root),
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            "home",
            "packages",
            "system",
            "hosts",
        ],
        capture_output=True,
        check=False,
        text=True,
    )
    stdout = getattr(result, "stdout", "") or ""
    stderr = getattr(result, "stderr", "") or ""
    returncode = getattr(result, "returncode", 1)

    if returncode != 0:
        err = stderr.strip() or stdout.strip() or "unknown git error"
        return False, [], err

    untracked = [
        line.strip()
        for line in stdout.splitlines()
        if line.strip().endswith(".nix")
    ]
    return True, sorted(untracked), None


def cmd_update(args: Any, printer: Printer, repo_root: Path) -> int:
    """Update flake inputs (nix flake update).

    Equivalent to the old nixupdate script.
    """
    extra_args = list(getattr(args, "passthrough", []) or [])
    success, _ = stream_nix_update(repo_root, printer, extra_args=extra_args)

    print()  # Blank line after command output
    if success:
        printer.success("Flake inputs updated")
        printer.info("Run 'nx rebuild' to rebuild, or 'nx upgrade' for full upgrade")
        return 0
    else:
        printer.error("Flake update failed")
        return 1


def cmd_test(printer: Printer, repo_root: Path) -> int:
    """Run ruff, mypy, and unit tests for nx."""
    steps = [
        ("ruff", ["ruff", "check", "."], repo_root / "scripts" / "nx"),
        ("mypy", ["mypy", "."], repo_root / "scripts" / "nx"),
        (
            "tests",
            ["python3", "-m", "unittest", "discover", "-s", "scripts/nx/tests"],
            repo_root,
        ),
    ]

    for label, cmd, cwd in steps:
        printer.action(f"Running {label}")
        print()
        returncode = _run_indented(cmd, cwd=cwd, printer=printer)
        print()
        if returncode != 0:
            printer.error(f"{label} failed")
            return 1
        printer.success(f"{label} passed")

    return 0


def cmd_rebuild(args: Any, printer: Printer, repo_root: Path) -> int:
    """Rebuild system (darwin-rebuild switch).

    Runs nix flake check first to catch syntax errors early.
    Equivalent to the old nixswitch script.
    """
    # Preflight: flake evaluation only sees git-tracked files.
    printer.action("Checking tracked nix files")
    ok, untracked_nix, preflight_err = _find_untracked_nix_files(repo_root)
    if not ok:
        printer.error("Git preflight failed")
        if preflight_err:
            print(f"  {preflight_err}")
        return 1
    if untracked_nix:
        printer.error("Untracked .nix files would be ignored by flake evaluation")
        print()
        print("  Track these files before rebuild:")
        for rel_path in untracked_nix:
            print(f"  - {rel_path}")
        print()
        print(f"  Run: git -C \"{repo_root}\" add <files>")
        return 1
    printer.success("Git preflight passed")

    # Run flake check first to catch errors early
    printer.action("Checking flake")
    check_result = subprocess.run(
        ["nix", "flake", "check", str(repo_root)],
        capture_output=True,
        check=False,
        text=True,
    )
    if check_result.returncode != 0:
        printer.error("Flake check failed")
        if check_result.stderr:
            print(check_result.stderr)
        return 1
    printer.success("Flake check passed")

    printer.action("Rebuilding system")
    print()  # Blank line before command output

    # Run with indented output (sudo password prompt still works via /dev/tty)
    extra_args = list(getattr(args, "passthrough", []) or [])
    returncode = _run_indented(
        ["sudo", DARWIN_REBUILD, "switch", "--flake", str(repo_root), *extra_args],
        printer=printer,
    )

    print()  # Blank line after command output
    if returncode == 0:
        printer.success("System rebuilt")
        return 0
    else:
        printer.error("Rebuild failed")
        return 1


def cmd_upgrade(args: Any, printer: Printer, repo_root: Path) -> int:
    """Full upgrade with changelogs: update + brew + rebuild + commit.

    Equivalent to the old nixupgrade script.
    """
    success, flake_changes = _run_flake_phase(args, printer, repo_root)
    if not success:
        return 1

    brew_updates = _run_brew_phase(args, printer)

    if args.dry_run:
        printer.info("Dry run complete - no changes made")
        return 0

    if not getattr(args, "skip_rebuild", False):
        if cmd_rebuild(args, printer, repo_root) != 0:
            return 1

    if not getattr(args, "skip_commit", False) and flake_changes:
        _commit_changes(printer, repo_root, flake_changes, brew_updates)

    return 0


# ─────────────────────────────────────────────────────────────────────────────
# Helper functions for upgrade
# ─────────────────────────────────────────────────────────────────────────────


def _display_flake_changes(
    printer: Printer,
    change_infos: list,
    use_ai: bool,
    verbose: bool,
) -> None:
    """Display flake input changes with optional AI summaries."""
    if not change_infos:
        printer.info("No flake inputs changed")
        return

    printer.section("Flake Inputs Changed", count=len(change_infos))

    for info in change_infos:
        change = info.input_change
        commit_str = f" ({info.total_commits} commits)" if info.total_commits else ""

        # Input name as bold header
        printer.command_header(change.name, leading_blank=True)
        printer.detail(f"{change.owner}/{change.repo} {short_rev(change.old_rev)} → {short_rev(change.new_rev)}{commit_str}")

        # Show error if fetch failed
        if info.error:
            printer.warn(info.error)
            continue

        # Show AI summary
        if use_ai:
            with printer.status(f"Analyzing {change.name}..."):
                summary = summarize_change(info)
            if summary:
                printer.line(f"Summary: {summary}")

        # Show verbose details
        if verbose and info.commit_messages:
            printer.line("Recent commits:")
            for msg in info.commit_messages[:5]:
                printer.bullet(msg[:70])


def _display_brew_changes(
    printer: Printer,
    change_infos: list,
    use_ai: bool,
    verbose: bool,
) -> None:
    """Display Homebrew package changes with optional AI summaries."""
    if not change_infos:
        printer.info("No Homebrew packages to update")
        return

    printer.section("Homebrew Outdated", count=len(change_infos))

    for info in change_infos:
        pkg = info.package

        # Package name as bold header
        printer.command_header(pkg.name, leading_blank=True)
        printer.detail(f"{pkg.installed_version} → {pkg.current_version}")

        # Show homepage in path color
        if pkg.homepage:
            printer.detail(pkg.homepage)

        # Show AI summary
        if use_ai and info.releases:
            with printer.status(f"Analyzing {pkg.name}..."):
                summary = summarize_brew_change(info)
            if summary:
                printer.line(f"Summary: {summary}")

        # Show verbose details
        if verbose and info.releases:
            printer.line("Recent releases:")
            for rel in info.releases[:3]:
                tag = rel.get("tag_name", "")
                printer.bullet(tag)


def _display_unchanged(printer: Printer, unchanged: list[str]) -> None:
    """Display list of unchanged inputs (dim, secondary info)."""
    if not unchanged:
        return

    # Compact dim line (no extra blank - next section adds its own)
    names = ", ".join(unchanged[:8])
    if len(unchanged) > 8:
        names += f", +{len(unchanged) - 8} more"
    printer.dim(f"Unchanged ({len(unchanged)}): {names}")


def _run_flake_phase(
    args: Any,
    printer: Printer,
    repo_root: Path,
) -> tuple[bool, list]:
    old_inputs = load_flake_lock(repo_root)

    if args.dry_run:
        printer.dry_run_banner()
        new_inputs = old_inputs
    else:
        extra_args = list(getattr(args, "passthrough", []) or [])
        success, _ = stream_nix_update(repo_root, printer, extra_args=extra_args)
        if not success:
            printer.error("Flake update failed")
            return False, []
        new_inputs = load_flake_lock(repo_root)

    changed, _added, _removed = diff_locks(old_inputs, new_inputs)

    if changed:
        with printer.status("Fetching changelog data..."):
            change_infos = fetch_all_changes(changed)

        flake_changes = [c.input_change for c in change_infos]

        _display_flake_changes(
            printer,
            change_infos,
            not getattr(args, "no_ai", False),
            args.verbose,
        )

        unchanged = [
            name for name in new_inputs.keys()
            if name not in [c.name for c in changed]
        ]
        _display_unchanged(printer, unchanged)
    else:
        flake_changes = []
        printer.success("All flake inputs up to date")

    return True, flake_changes


def _run_brew_phase(args: Any, printer: Printer) -> list:
    if getattr(args, "skip_brew", False):
        return []

    printer.action("Checking Homebrew updates")

    with printer.status("Checking for outdated packages..."):
        outdated = get_outdated()

    if not outdated:
        printer.success("All Homebrew packages up to date")
        return []

    with printer.status("Fetching package info..."):
        outdated = enrich_package_info(outdated)
        brew_change_infos = fetch_all_brew_changelogs(outdated)

    _display_brew_changes(
        printer,
        brew_change_infos,
        not getattr(args, "no_ai", False),
        args.verbose,
    )

    if not args.dry_run:
        pkg_names = [pkg.name for pkg in outdated]
        printer.action(f"Upgrading {len(pkg_names)} Homebrew packages")
        print()
        returncode = _run_indented(["brew", "upgrade", *pkg_names], printer=printer)
        print()
        if returncode == 0:
            printer.success("Homebrew packages upgraded")
        else:
            printer.warn("Some Homebrew upgrades may have failed")

    return outdated


def _commit_changes(
    printer: Printer,
    repo_root: Path,
    flake_changes: list,
    brew_updates: list,
) -> bool:
    """Commit flake.lock changes."""
    # Generate commit message
    message = generate_commit_message(flake_changes, brew_updates)

    # Stage flake.lock
    add_result = subprocess.run(
        ["git", "add", "flake.lock"],
        cwd=repo_root,
        capture_output=True,
        check=False,
    )

    if add_result.returncode != 0:
        printer.error("Failed to stage flake.lock")
        return False

    # Create commit
    commit_result = subprocess.run(
        ["git", "commit", "-m", message],
        cwd=repo_root,
        capture_output=True,
        check=False,
        text=True,
    )

    if commit_result.returncode == 0:
        printer.success(f"Committed: {message}")
        return True
    elif "nothing to commit" in commit_result.stdout or "nothing to commit" in commit_result.stderr:
        printer.info("No changes to commit")
        return True
    else:
        printer.error(f"Commit failed: {commit_result.stderr}")
        return False
