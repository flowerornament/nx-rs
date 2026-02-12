"""
search.py - Package search and installation helpers for nx.

Functions for searching packages across sources and orchestrating installations.
"""

from __future__ import annotations

import re
import subprocess
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any, Literal

from ai_helpers import build_routing_context, edit_via_codex, route_package_codex_decision
from claude_ops import insert_service_via_claude, insert_via_claude
from config import ConfigFiles
from finder import find_package
from shared import (
    add_flake_input,
    detect_language_package,
    format_source_display,
    relative_path,
    run_command,
    split_location,
)
from sources import SourcePreferences, SourceResult, check_nix_available, search_all_sources

if TYPE_CHECKING:
    from cache import MultiSourceCache
    from nx_printer import NxPrinter as Printer


InstallMode = Literal[
    "nix_manifest",
    "language_with_packages",
    "homebrew_manifest",
    "mas_apps",
]


@dataclass(frozen=True)
class InstallPlan:
    """Single install decision contract consumed by all engine executors."""

    source_result: SourceResult
    package_token: str
    target_file: str
    insertion_mode: InstallMode
    is_brew: bool
    is_cask: bool
    is_mas: bool
    language_info: tuple[str, str, str] | None
    routing_warning: str | None = None


def _installed_result(name: str, location: str, repo_root: Path) -> SourceResult:
    rel_path = relative_path(location, repo_root)
    return SourceResult(
        name=name,
        source="installed",
        attr=rel_path,
        confidence=1.0,
        description=f"Already at {rel_path}",
    )


def _find_existing_for_result(sr: SourceResult, config_files: ConfigFiles) -> str | None:
    for candidate in _lookup_names(sr):
        existing = find_package(candidate, config_files)
        if existing:
            return existing
    return None


def _find_existing_for_candidates(
    candidates: list[SourceResult],
    config_files: ConfigFiles,
) -> str | None:
    for candidate in candidates:
        existing = _find_existing_for_result(candidate, config_files)
        if existing:
            return existing
    return None


def _log_explain_results(printer: Printer, source_results: list[SourceResult]) -> None:
    if not source_results:
        return
    printer.info(f"  Found {len(source_results)} results:")
    for sr in source_results[:5]:
        printer.info(f"    - {sr.source}: {sr.attr} (confidence: {sr.confidence:.2f})")
        if sr.requires_flake_mod:
            printer.info("      ⚠ Requires flake.nix modification")


def _resolve_package_result(
    name: str,
    explain: bool,
    printer: Printer,
    repo_root: Path,
    config_files: ConfigFiles,
    cache: MultiSourceCache,
    source_prefs: SourcePreferences,
    flake_lock: Path,
) -> tuple[SourceResult, list[SourceResult] | None]:
    already = find_package(name, config_files)
    if already:
        return _installed_result(name, already, repo_root), None

    cached_results = cache.get_all(name)
    if cached_results:
        existing = _find_existing_for_candidates(cached_results, config_files)
        if existing:
            return _installed_result(name, existing, repo_root), None
        if explain:
            printer.info(f"\n  Cache hit for '{name}' ({len(cached_results)} sources)")
        alternatives = cached_results if len(cached_results) > 1 else None
        return cached_results[0], alternatives

    if explain:
        print()
        printer.activity("searching", name)

    with printer.status(f"Searching for {name}..."):
        source_results = search_all_sources(name, source_prefs, flake_lock)

    if explain:
        _log_explain_results(printer, source_results)

    if not source_results:
        return SourceResult(
            name=name,
            source="unknown",
            confidence=0.0,
            description="Not found in any source",
        ), None

    best = source_results[0]
    existing = _find_existing_for_candidates(source_results, config_files)
    if existing:
        return _installed_result(name, existing, repo_root), None

    cache.set_many(source_results)
    alternatives = source_results if len(source_results) > 1 else None
    return best, alternatives


def search_packages(
    packages: list[str],
    args: Any,
    printer: Printer,
    repo_root: Path,
    config_files: ConfigFiles,
    cache: MultiSourceCache,
    source_prefs: SourcePreferences,
) -> tuple[list[SourceResult], dict[str, list[SourceResult]]]:
    """Search for packages across all sources."""
    explain = getattr(args, 'explain', False)
    results: list[SourceResult] = []
    alternatives: dict[str, list[SourceResult]] = {}
    flake_lock = repo_root / "flake.lock"

    for name in packages:
        result, alt = _resolve_package_result(
            name=name,
            explain=explain,
            printer=printer,
            repo_root=repo_root,
            config_files=config_files,
            cache=cache,
            source_prefs=source_prefs,
            flake_lock=flake_lock,
        )
        results.append(result)
        if alt:
            alternatives[name] = alt

    return results, alternatives


def _truncate(text: str, max_len: int) -> str:
    """Truncate text with ellipsis if too long."""
    if not text:
        return ""
    return text[:max_len] + "..." if len(text) > max_len else text


def _get_unique_alternatives(
    name: str,
    alternatives: dict[str, list[SourceResult]],
) -> list[SourceResult]:
    """Get unique alternatives by source type."""
    if name not in alternatives or len(alternatives[name]) <= 1:
        return []
    seen_sources = set()
    unique = []
    for alt in alternatives[name]:
        if alt.source not in seen_sources:
            seen_sources.add(alt.source)
            unique.append(alt)
    return unique if len(unique) > 1 else []


def _show_alternative_option(
    printer: Printer,
    index: int,
    alt: SourceResult,
) -> None:
    """Display a single numbered alternative option."""
    source_display = format_source_display(alt.source, alt.attr)
    printer.numbered_option(index, source_display)
    if alt.version:
        printer.kv_line("Version", alt.version, indent=7)
    if alt.description:
        printer.kv_line("Description", _truncate(alt.description, 60), indent=7)


def show_search_results(
    results: list[SourceResult],
    printer: Printer,
    alternatives: dict[str, list[SourceResult]] | None = None,
) -> None:
    """Display search results to the user."""
    alternatives = alternatives or {}

    # Count installable packages
    installable = [r for r in results if r.source not in ("installed", "unknown")]
    already = [r for r in results if r.source == "installed"]
    not_found = [r for r in results if r.source == "unknown"]

    if installable:
        printer.section("Found", len(installable))

        for sr in installable:
            # Check if this package has alternatives worth showing
            unique_alts = _get_unique_alternatives(sr.name, alternatives)

            if unique_alts and len(installable) == 1:
                # Single package with alternatives: show detailed numbered options
                printer.heading(sr.name)
                for i, alt in enumerate(unique_alts, 1):
                    _show_alternative_option(printer, i, alt)
            else:
                # Single source or multiple packages: compact format
                source_name = format_source_display(sr.source, sr.attr)
                desc = f" - {_truncate(sr.description, 50)}" if sr.description else ""
                printer.package_line(sr.name, source_name, desc)

    if already:
        print()
        for sr in already:
            printer.success(f"{sr.name} already installed ({sr.attr})")

    if not_found:
        print()
        for sr in not_found:
            printer.error(f"{sr.name} not found")


def _warn_cask_alternatives(
    args: Any,
    printer: Printer,
    to_install: list[SourceResult],
    alternatives: dict[str, list[SourceResult]],
) -> None:
    if getattr(args, "cask", False) or getattr(args, "source", None):
        return
    for r in to_install:
        if r.source != "nxs":
            continue
        alts = alternatives.get(r.name, [])
        if any(alt.source == "cask" for alt in alts):
            printer.warn(
                f"{r.name} has a Homebrew cask; consider --cask for better macOS integration"
            )


def _build_alt_map(
    to_install: list[SourceResult],
    alternatives: dict[str, list[SourceResult]],
) -> dict[str, list[SourceResult]]:
    alt_map: dict[str, list[SourceResult]] = {}
    for r in to_install:
        unique_alts = _get_unique_alternatives(r.name, alternatives)
        if unique_alts:
            alt_map[r.name] = unique_alts
    return alt_map


def _confirm_install(
    printer: Printer,
    to_install: list[SourceResult],
    alt_map: dict[str, list[SourceResult]],
) -> list[SourceResult]:
    print()
    if alt_map and len(to_install) == 1:
        return _confirm_install_single(printer, to_install, alt_map)
    return _confirm_install_multi(printer, to_install)


def _confirm_install_single(
    printer: Printer,
    to_install: list[SourceResult],
    alt_map: dict[str, list[SourceResult]],
) -> list[SourceResult]:
    r = to_install[0]
    alts = alt_map[r.name]
    nums = "/".join(str(i) for i in range(1, len(alts) + 1))
    prompt = f"Install? [{nums}/n]: "
    try:
        response = input(f"{printer.INDENT}{prompt}").strip().lower()
    except EOFError:
        response = ""

    if response in {"n", "no"}:
        printer.info("Cancelled.")
        return []
    if not response or response == "1":
        return to_install

    try:
        choice = int(response) - 1
        if 0 <= choice < len(alts):
            to_install[0] = alts[choice]
            return to_install
    except ValueError:
        pass

    printer.info("Cancelled.")
    return []


def _confirm_install_multi(printer: Printer, to_install: list[SourceResult]) -> list[SourceResult]:
    if not printer.confirm("Install these packages?", default=True):
        printer.info("Cancelled.")
        return []
    return to_install


def filter_installable(
    results: list[SourceResult],
    printer: Printer,
    args: Any,
    alternatives: dict[str, list[SourceResult]] | None = None,
) -> list[SourceResult]:
    """Filter results and get user confirmation.

    Args:
        results: Search results
        printer: Printer for output
        args: Parsed arguments (for yes flag)
        alternatives: Dict of alternative sources by package name

    Returns:
        List of packages to install
    """
    alternatives = alternatives or {}

    show_search_results(results, printer, alternatives)

    to_install = [r for r in results if r.source not in ("installed", "unknown")]
    if not to_install:
        return []

    _warn_cask_alternatives(args, printer, to_install, alternatives)

    if args.yes or args.dry_run:
        return to_install

    alt_map = _build_alt_map(to_install, alternatives)
    return _confirm_install(printer, to_install, alt_map)


def install_packages(
    to_install: list[SourceResult],
    config_files: ConfigFiles,
    repo_root: Path,
    printer: Printer,
    args: Any,
    alternatives: dict[str, list[SourceResult]] | None = None,
) -> int:
    """Install packages via Claude (single call per package)."""
    routing_context = build_routing_context(repo_root)
    return _install_packages_impl(
        to_install,
        config_files,
        repo_root,
        printer,
        args,
        install_one=_install_one_claude,
        allow_prompt=True,
        routing_context=routing_context,
        alternatives_by_name=alternatives,
    )


def install_packages_turbo(
    to_install: list[SourceResult],
    config_files: ConfigFiles,
    repo_root: Path,
    printer: Printer,
    args: Any,
    alternatives: dict[str, list[SourceResult]] | None = None,
) -> int:
    """Install packages via Codex (turbo mode - faster than Claude)."""
    routing_context = build_routing_context(repo_root)

    return _install_packages_impl(
        to_install,
        config_files,
        repo_root,
        printer,
        args,
        install_one=_install_one_turbo,
        allow_prompt=False,
        routing_context=routing_context,
        alternatives_by_name=alternatives,
    )


def _install_packages_impl(
    to_install: list[SourceResult],
    config_files: ConfigFiles,
    repo_root: Path,
    printer: Printer,
    args: Any,
    *,
    install_one: Callable[[InstallPlan, ConfigFiles, Path, Printer, Any], bool],
    allow_prompt: bool,
    routing_context: str,
    alternatives_by_name: dict[str, list[SourceResult]] | None = None,
) -> int:
    if args.dry_run:
        printer.section("Analyzing", len(to_install))
    else:
        printer.section("Installing", len(to_install))

    alternatives_by_name = alternatives_by_name or {}

    success_count = 0
    for sr in to_install:
        install_sr = sr
        if not _handle_flake_mod(install_sr, args, printer, repo_root, allow_prompt=allow_prompt):
            continue

        if install_sr.source in ("nxs", "nur", "flake-input") and install_sr.attr:
            with printer.status(f"Checking {install_sr.name} availability..."):
                available, reason = check_nix_available(install_sr.attr)
            if not available:
                fallback = _select_same_source_fallback(
                    install_sr,
                    alternatives_by_name.get(install_sr.name, []),
                )
                if not fallback:
                    printer.error(f"{install_sr.name}: {reason}")
                    continue

                fallback_desc = fallback.attr or fallback.name
                printer.warn(f"{install_sr.name}: {reason}; trying {fallback_desc}")
                install_sr = fallback

        printer.activity("routing", f"Routing {install_sr.name}")
        plan, plan_error = _build_install_plan(install_sr, config_files, repo_root, routing_context)
        if not plan:
            printer.error(plan_error or f"Failed to build install plan for {install_sr.name}")
            continue
        if plan.routing_warning:
            printer.warn(plan.routing_warning)

        did_add = install_one(plan, config_files, repo_root, printer, args)

        if did_add:
            success_count += 1

    return success_count


def _handle_flake_mod(
    sr: SourceResult,
    args: Any,
    printer: Printer,
    repo_root: Path,
    *,
    allow_prompt: bool,
) -> bool:
    if sr.requires_flake_mod and sr.flake_url:
        if not allow_prompt:
            printer.warn(f"{sr.name} requires flake.nix modification - use --engine=claude")
            return False
        printer.warn(f"{sr.name} requires flake.nix modification")
        printer.info(f"  URL: {sr.flake_url}")
        if not args.yes and not printer.confirm("Add flake input?", default=True):
            printer.warn(f"Skipping {sr.name}")
            return False
        if args.dry_run:
            printer.info(f"[DRY RUN] Would add flake input for {sr.name}")
            return True
        ok, msg = add_flake_input(repo_root / "flake.nix", sr.flake_url)
        if not ok:
            printer.error(f"Failed to add flake input: {msg}")
            return False
        if "already exists" in msg:
            printer.info(msg)
    return True


def _install_name(sr: SourceResult) -> str:
    """Return the concrete package token to write into config files."""
    return sr.attr or sr.name


def _select_same_source_fallback(
    primary: SourceResult,
    alternatives: list[SourceResult],
) -> SourceResult | None:
    """Pick the next viable candidate from the same source.

    This keeps source selection stable while allowing platform-incompatible
    attrs to fall back to lower-ranked attrs from the same source.
    """
    for candidate in alternatives:
        if candidate.source != primary.source:
            continue
        if candidate.attr == primary.attr:
            continue
        if primary.source in ("nxs", "nur", "flake-input"):
            if not candidate.attr:
                continue
            available, _reason = check_nix_available(candidate.attr)
            if not available:
                continue
        return candidate
    return None


def _path_in_repo(path: Path, repo_root: Path) -> str:
    """Convert an absolute config path to a repo-relative target when possible."""
    path_str = str(path)
    if not path_str:
        return path_str
    return relative_path(path, repo_root)


def _nix_manifest_candidates(config_files: ConfigFiles, repo_root: Path) -> list[str]:
    """Collect discovered Nix manifest candidates for fuzzy/LLM routing."""
    default_target = _path_in_repo(config_files.packages, repo_root)
    language_target = _path_in_repo(config_files.languages, repo_root)
    manifest_parent = Path(default_target).parent

    candidates: list[str] = [default_target]
    discovered = sorted({
        _path_in_repo(path, repo_root)
        for path in config_files.by_purpose.values()
        if path.suffix == ".nix"
    })

    for rel in discovered:
        rel_path = Path(rel)
        if rel == language_target:
            continue
        if rel_path.parent == manifest_parent and rel not in candidates:
            candidates.append(rel)

    return candidates


def _build_install_plan(
    sr: SourceResult,
    config_files: ConfigFiles,
    repo_root: Path,
    routing_context: str,
) -> tuple[InstallPlan | None, str | None]:
    if sr.source in ("nxs", "nur", "flake-input") and not sr.attr:
        return None, f"Missing resolved attribute for {sr.name}; refusing unsafe install"

    package_token = _install_name(sr)
    is_cask = sr.source == "cask"
    is_brew = sr.source in ("homebrew", "brew")
    is_mas = sr.source == "mas"
    language_info = detect_language_package(package_token)

    if is_cask:
        target_file = _path_in_repo(config_files.homebrew_casks, repo_root)
        insertion_mode: InstallMode = "homebrew_manifest"
    elif is_brew:
        target_file = _path_in_repo(config_files.homebrew_brews, repo_root)
        insertion_mode = "homebrew_manifest"
    elif is_mas:
        target_file = _path_in_repo(config_files.darwin, repo_root)
        insertion_mode = "mas_apps"
    elif language_info:
        target_file = _path_in_repo(config_files.languages, repo_root)
        insertion_mode = "language_with_packages"
        routing_warning = None
    else:
        default_target = _path_in_repo(config_files.packages, repo_root)
        target_file, routing_warning = route_package_codex_decision(
            package_token,
            routing_context,
            cwd=repo_root,
            candidate_files=_nix_manifest_candidates(config_files, repo_root),
            default_target=default_target,
        )
        insertion_mode = "nix_manifest"
    if is_cask or is_brew or is_mas:
        routing_warning = None

    return InstallPlan(
        source_result=sr,
        package_token=package_token,
        target_file=target_file,
        insertion_mode=insertion_mode,
        is_brew=is_brew,
        is_cask=is_cask,
        is_mas=is_mas,
        language_info=language_info,
        routing_warning=routing_warning,
    ), None


def _lookup_names(sr: SourceResult) -> list[str]:
    names: list[str] = []
    for candidate in (sr.name, sr.attr):
        if candidate and candidate not in names:
            names.append(candidate)

    if sr.attr:
        lang_info = detect_language_package(sr.attr)
        if lang_info:
            bare_name, _runtime, _method = lang_info
            if bare_name and bare_name not in names:
                names.append(bare_name)

    return names


def _report_package_change(
    sr: SourceResult,
    config_files: ConfigFiles,
    repo_root: Path,
    printer: Printer,
    before_diff: str,
) -> bool:
    _, after_diff = run_command(["git", "diff"], cwd=repo_root)
    if after_diff != before_diff:
        location = None
        for candidate in _lookup_names(sr):
            location = find_package(candidate, config_files)
            if location:
                break
        if location:
            file_path, line_num = split_location(location)
            file_name = Path(file_path).name
            printer.complete(f"{sr.name} added to {file_name}")
            if line_num:
                printer.show_snippet(file_path, line_num, context=1)
        else:
            printer.complete(f"{sr.name} added")
        return True

    printer.info(f"{sr.name} already configured (no changes needed)")
    return False


def _install_one_claude(
    plan: InstallPlan,
    config_files: ConfigFiles,
    repo_root: Path,
    printer: Printer,
    args: Any,
) -> bool:
    sr = plan.source_result
    if not args.dry_run:
        _, before_diff = run_command(["git", "diff"], cwd=repo_root)

    model = getattr(args, "model", None)
    result = insert_via_claude(
        sr,
        repo_root,
        config_files,
        printer=printer,
        dry_run=args.dry_run,
        model=model,
        package_token=plan.package_token,
        target_file=plan.target_file,
        insertion_mode=plan.insertion_mode,
    )
    if not result.success:
        printer.error(f"Failed to add {sr.name}: {result.message}")
        return False

    if args.dry_run:
        dry_run_file = result.file_path or plan.target_file
        file_name = Path(dry_run_file).name if dry_run_file else "config"
        if result.file_path and result.line_num and result.simulated_line:
            printer.show_dry_run_preview(result.file_path, result.line_num, result.simulated_line)
        if plan.language_info:
            bare_name, runtime, _ = plan.language_info
            printer.result_add(f"Would add '{bare_name}' to {runtime}.withPackages in {plan.target_file}")
        else:
            printer.result_add(f"Would add {sr.name} to {file_name}")
        did_add = True
    else:
        did_add = _report_package_change(sr, config_files, repo_root, printer, before_diff)

    if args.service:
        ok2, msg2 = insert_service_via_claude(sr.name, config_files, repo_root, dry_run=args.dry_run)
        if ok2:
            if args.dry_run:
                printer.result_add(f"Would add launchd.agents.{sr.name}")
            else:
                printer.complete(f"launchd.agents.{sr.name} added")
        else:
            printer.warn(f"Service setup failed: {msg2}")

    return did_add


def _install_one_turbo(
    plan: InstallPlan,
    config_files: ConfigFiles,
    repo_root: Path,
    printer: Printer,
    args: Any,
) -> bool:
    sr = plan.source_result
    package_name = plan.package_token
    target_file = plan.target_file

    if args.dry_run:
        target_path = Path(repo_root) / target_file if not Path(target_file).is_absolute() else Path(target_file)
        if target_path.exists():
            lines = target_path.read_text().split("\n")
            insert_line = None
            for i, line in enumerate(lines, 1):
                if re.match(r'^\s{4}\w[\w-]*\s*(#.*)?$', line) and not line.strip().startswith('#'):
                    insert_line = i
            if insert_line:
                desc = (sr.description or "")[:40]
                if desc:
                    desc = f"  # {desc}..."
                simulated = f"{package_name}{desc}"
                printer.show_dry_run_preview(str(target_path), insert_line, simulated, context=1)

        lang_info = plan.language_info
        if lang_info:
            bare_name, runtime, _ = lang_info
            printer.result_add(f"Would add '{bare_name}' to {runtime}.withPackages in {target_file}")
        else:
            printer.result_add(f"Would add {package_name} to {target_file}")
        return True

    _, before_diff = run_command(["git", "diff"], cwd=repo_root)

    printer.activity("adding", f"Adding {sr.name} to {target_file}")
    success, msg = edit_via_codex(
        package_name,
        target_file,
        sr.description or "",
        repo_root,
        dry_run=False,
        is_brew=plan.is_brew,
        is_cask=plan.is_cask,
        is_mas=plan.is_mas,
    )

    if not success:
        printer.error(f"Failed to add {sr.name}: {msg}")
        return False

    return _report_package_change(sr, config_files, repo_root, printer, before_diff)


def run_post_install_actions(
    success_count: int,
    repo_root: Path,
    args: Any,
    printer: Printer,
) -> None:
    """Run post-installation actions (rebuild, messages)."""
    if success_count > 0 and not args.dry_run:
        print()
        printer.dim("Run: nx rebuild")

        if args.rebuild:
            printer.activity("running", "rebuild")
            subprocess.run(
                ["/run/current-system/sw/bin/darwin-rebuild", "switch", "--flake", str(repo_root)],
                check=False
            )
