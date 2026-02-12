"""
cli.py - Typer-based CLI for nx.

Multi-source package installer and system manager for nix-darwin.
"""

from __future__ import annotations

import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from types import SimpleNamespace
from typing import Annotated, Any, ParamSpec, TypeVar, cast

import typer

from cache import MultiSourceCache
from commands import (
    cmd_info,
    cmd_install,
    cmd_installed,
    cmd_list,
    cmd_rebuild,
    cmd_remove,
    cmd_status,
    cmd_test,
    cmd_undo,
    cmd_update,
    cmd_upgrade,
    cmd_where,
)
from config import ConfigFiles, find_repo_root, get_config_files
from nx_printer import NxPrinter as Printer
from sources import SourcePreferences

# ═══════════════════════════════════════════════════════════════════════════════
# Application State
# ═══════════════════════════════════════════════════════════════════════════════


@dataclass
class AppState:
    """Global application state, initialized in the main callback."""

    printer: Printer | None = None
    repo_root: Path | None = None
    config_files: ConfigFiles | None = None
    cache: MultiSourceCache | None = None
    verbose: bool = False
    json_output: bool = False
    dry_run: bool = False
    yes: bool = False
    passthrough: list[str] = field(default_factory=list)


state = AppState()

# ═══════════════════════════════════════════════════════════════════════════════
# Typer App
# ═══════════════════════════════════════════════════════════════════════════════

app = typer.Typer(
    name="nx",
    help="Multi-source package installer for nix-darwin",
    add_completion=False,
    rich_markup_mode=None,
    no_args_is_help=True,
)

P = ParamSpec("P")
R = TypeVar("R")


def _typed_command(*args: Any, **kwargs: Any) -> Callable[[Callable[P, R]], Callable[P, R]]:
    return cast(Callable[[Callable[P, R]], Callable[P, R]], app.command(*args, **kwargs))


def _typed_callback(*args: Any, **kwargs: Any) -> Callable[[Callable[P, R]], Callable[P, R]]:
    return cast(Callable[[Callable[P, R]], Callable[P, R]], app.callback(*args, **kwargs))


# ═══════════════════════════════════════════════════════════════════════════════
# Type Aliases for Common Options
# ═══════════════════════════════════════════════════════════════════════════════

# Global options
OptYes = Annotated[bool, typer.Option("--yes", "-y", help="Skip confirmations")]
OptDryRun = Annotated[bool, typer.Option("--dry-run", "-n", help="Show what would happen")]
OptPlain = Annotated[bool, typer.Option("--plain", help="Plain text output")]
OptVerbose = Annotated[bool, typer.Option("--verbose", "-v", help="Show detailed info")]

# Install-specific options
OptCask = Annotated[bool, typer.Option("--cask", help="Treat as GUI app (homebrew cask)")]
OptMas = Annotated[bool, typer.Option("--mas", help="Mac App Store app")]
OptService = Annotated[bool, typer.Option("--service", help="Also set up launchd service")]
OptRebuild = Annotated[bool, typer.Option("--rebuild", help="Run darwin-rebuild after")]
OptBleedingEdge = Annotated[bool, typer.Option("--bleeding-edge", help="Prefer nxs-unstable or nightly versions")]
OptNur = Annotated[bool, typer.Option("--nur", help="Search NUR (Nix User Repository)")]
OptSource = Annotated[str | None, typer.Option("--source", help="Force specific source (nxs, unstable, nur, homebrew)")]
OptExplain = Annotated[bool, typer.Option("--explain", help="Show detailed routing decision reasoning")]

# Engine options
OptEngine = Annotated[str | None, typer.Option("--engine", help="Install engine (codex or claude)")]
OptModel = Annotated[str | None, typer.Option("--model", help="Claude model (sonnet, opus, haiku)")]


# Upgrade-specific options
OptSkipRebuild = Annotated[bool, typer.Option("--skip-rebuild", help="Update flake.lock but don't rebuild")]
OptSkipCommit = Annotated[bool, typer.Option("--skip-commit", help="Don't auto-commit changes")]
OptSkipBrew = Annotated[bool, typer.Option("--skip-brew", help="Skip Homebrew changelog check")]
OptNoAi = Annotated[bool, typer.Option("--no-ai", help="Skip AI summarization")]

# Installed-specific options
OptShowLocation = Annotated[bool, typer.Option("--show-location", help="Show file:line location")]


# ═══════════════════════════════════════════════════════════════════════════════
# Helper: Args-like object for backward compatibility with commands.py
# ═══════════════════════════════════════════════════════════════════════════════


_DEFAULT_ARGS = {
    "packages": [],
    # Global
    "yes": False,
    "dry_run": False,
    "plain": False,
    "verbose": False,
    "json": False,
    # Install
    "cask": False,
    "mas": False,
    "service": False,
    "rebuild": False,
    "bleeding_edge": False,
    "nur": False,
    "source": None,
    "explain": False,
    # Engine
    "engine": "codex",
    "model": None,
    # Upgrade
    "skip_rebuild": False,
    "skip_commit": False,
    "skip_brew": False,
    "no_ai": False,
    # Installed
    "show_location": False,
    # List
    "list_source": None,
    # Passthrough
    "passthrough": [],
}


def make_args(**overrides: Any) -> SimpleNamespace:
    """Create an Args-like namespace with defaults + overrides."""
    data = dict(_DEFAULT_ARGS)

    packages = overrides.pop("packages", None)
    if packages is not None:
        data["packages"] = list(packages)

    passthrough = overrides.pop("passthrough", None)
    if passthrough is not None:
        data["passthrough"] = list(passthrough)

    data.update(overrides)

    if data.get("engine") is None:
        data["engine"] = _DEFAULT_ARGS["engine"]

    return SimpleNamespace(**data)


def _init_state(
    plain: bool | None = None,
    unicode: bool | None = None,
    minimal: bool | None = None,
    verbose: bool | None = None,
    json_output: bool | None = None,
    dry_run: bool = False,
    yes: bool = False,
    passthrough: list[str] | None = None,
) -> None:
    """Initialize global state (called at start of each command)."""
    use_plain = plain if plain is not None else (state.printer.use_plain if state.printer else False)
    if minimal is not None:
        use_minimal = minimal
    elif state.printer:
        use_minimal = state.printer.use_minimal
    else:
        # Default to minimal glyphs unless another global mode is requested.
        use_minimal = not bool(use_plain or unicode)
    use_unicode = unicode if unicode is not None else (state.printer.use_unicode if state.printer else False)

    if state.printer is None or (
        state.printer.use_plain != use_plain
        or state.printer.use_minimal != use_minimal
        or state.printer.use_unicode != use_unicode
    ):
        state.printer = Printer(
            use_plain=use_plain,
            use_minimal=use_minimal,
            use_unicode=use_unicode,
        )

    if verbose is not None:
        state.verbose = verbose
    if json_output is not None:
        state.json_output = json_output
    state.dry_run = dry_run
    state.yes = yes
    state.passthrough = passthrough or []

    if state.repo_root is None:
        try:
            state.repo_root = find_repo_root()
            state.config_files = get_config_files(state.repo_root)
            state.cache = MultiSourceCache(state.repo_root)
        except Exception as e:
            state.printer.error(str(e))
            raise typer.Exit(1) from None


def _require_state() -> tuple[Printer, Path, ConfigFiles, MultiSourceCache]:
    """Return initialized state or exit if missing."""
    if state.printer is None or state.repo_root is None or state.config_files is None or state.cache is None:
        raise typer.Exit(1)
    return state.printer, state.repo_root, state.config_files, state.cache


def _effective_passthrough(ctx_args: list[str]) -> list[str]:
    """Return CLI passthrough args, falling back to stored state."""
    return ctx_args if ctx_args else state.passthrough


def _effective_flag(value: bool, fallback: bool) -> bool:
    """Return a flag that falls back to an existing value when false."""
    return value or fallback


# ═══════════════════════════════════════════════════════════════════════════════
# Main Callback (global options only)
# ═══════════════════════════════════════════════════════════════════════════════


@_typed_callback()
def main(
    ctx: typer.Context,
    plain: OptPlain = False,
    unicode: Annotated[bool, typer.Option("--unicode", help="Use Unicode glyphs")] = False,
    minimal: Annotated[bool, typer.Option("--minimal", help="Use ASCII glyphs")] = False,
    verbose: OptVerbose = False,
    json_output: Annotated[bool, typer.Option("--json", help="JSON output")] = False,
) -> None:
    """
    Multi-source package installer for nix-darwin.

    Examples:
        nx install ripgrep fd bat   # Install CLI tools
        nx install --cask raycast   # Install GUI app
        nx rm ripgrep               # Remove package
        nx where python             # Find package location
        nx list                     # List all packages
    """
    _init_state(
        plain=plain if plain else None,
        unicode=unicode if unicode else None,
        minimal=minimal if minimal else None,
        verbose=verbose,
        json_output=json_output,
    )


# ═══════════════════════════════════════════════════════════════════════════════
# Subcommands
# ═══════════════════════════════════════════════════════════════════════════════


@_typed_command("install", context_settings={"allow_extra_args": True})
def install_cmd(
    ctx: typer.Context,
    packages: Annotated[list[str], typer.Argument(help="Packages to install")],
    yes: OptYes = False,
    dry_run: OptDryRun = False,
    cask: OptCask = False,
    mas: OptMas = False,
    service: OptService = False,
    rebuild: OptRebuild = False,
    bleeding_edge: OptBleedingEdge = False,
    nur: OptNur = False,
    source: OptSource = None,
    explain: OptExplain = False,
    engine: OptEngine = None,
    model: OptModel = None,
) -> None:
    """Install packages."""
    _init_state(dry_run=dry_run, yes=yes, passthrough=ctx.args)

    printer, repo_root, config_files, cache = _require_state()

    resolved_engine = engine or "codex"
    resolved_model = model
    if resolved_model and resolved_engine != "claude":
        printer.warn("--model is only used with --engine=claude")

    source_prefs = SourcePreferences(
        bleeding_edge=bleeding_edge,
        nur=nur,
        force_source=source,
        is_cask=cask,
        is_mas=mas,
    )

    args = make_args(
        packages=list(packages),
        yes=_effective_flag(yes, state.yes),
        dry_run=_effective_flag(dry_run, state.dry_run),
        cask=cask,
        mas=mas,
        service=service,
        rebuild=rebuild,
        bleeding_edge=bleeding_edge,
        nur=nur,
        source=source,
        explain=explain,
        engine=resolved_engine,
        model=resolved_model,
        passthrough=state.passthrough,
    )

    result = cmd_install(
        args,
        printer,
        repo_root,
        config_files,
        cache,
        source_prefs,
    )
    raise typer.Exit(result)


def _remove_impl(
    packages: list[str],
    yes: bool = False,
    dry_run: bool = False,
    model: str | None = None,
) -> None:
    """Remove packages from configuration (implementation)."""
    _init_state(dry_run=dry_run, yes=yes)

    args = make_args(
        packages=list(packages),
        yes=_effective_flag(yes, state.yes),
        dry_run=_effective_flag(dry_run, state.dry_run),
        model=model,
    )

    printer, repo_root, config_files, _cache = _require_state()
    result = cmd_remove(args, printer, repo_root, config_files)
    raise typer.Exit(result)


@_typed_command("remove")
def remove_cmd(
    packages: Annotated[list[str], typer.Argument(help="Packages to remove")],
    yes: OptYes = False,
    dry_run: OptDryRun = False,
    model: OptModel = None,
) -> None:
    """Remove packages from configuration."""
    _remove_impl(packages, yes, dry_run, model)


@_typed_command("rm")
def rm_cmd(
    packages: Annotated[list[str], typer.Argument(help="Packages to remove")],
    yes: OptYes = False,
    dry_run: OptDryRun = False,
    model: OptModel = None,
) -> None:
    """Remove packages from configuration (alias for remove)."""
    _remove_impl(packages, yes, dry_run, model)


@_typed_command("where")
def where_cmd(
    package: Annotated[str, typer.Argument(help="Package to find")],
) -> None:
    """Find where a package is configured."""
    _init_state()

    args = make_args(packages=[package])
    printer, _repo_root, config_files, _cache = _require_state()
    result = cmd_where(args, printer, config_files)
    raise typer.Exit(result)


@_typed_command("list")
def list_cmd(
    source: Annotated[str | None, typer.Argument(help="Filter by source (nxs, brews, casks, mas)")] = None,
    verbose: OptVerbose = False,
    json_output: Annotated[bool, typer.Option("--json", help="JSON output")] = False,
    plain: OptPlain = False,
) -> None:
    """List all installed packages."""
    _init_state(
        verbose=verbose if verbose else None,
        json_output=json_output if json_output else None,
        plain=plain if plain else None,
    )

    args = make_args(
        list_source=source,
        verbose=_effective_flag(verbose, state.verbose),
        json=_effective_flag(json_output, state.json_output),
        plain=plain,
    )

    printer, _repo_root, config_files, _cache = _require_state()
    result = cmd_list(args, printer, config_files)
    raise typer.Exit(result)


@_typed_command("info")
def info_cmd(
    package: Annotated[str, typer.Argument(help="Package to get info for")],
    json_output: Annotated[bool, typer.Option("--json", help="JSON output")] = False,
    bleeding_edge: OptBleedingEdge = False,
    verbose: OptVerbose = False,
) -> None:
    """Show detailed information about a package."""
    _init_state(
        verbose=verbose if verbose else None,
        json_output=json_output if json_output else None,
    )

    args = make_args(
        packages=[package],
        json=_effective_flag(json_output, state.json_output),
        bleeding_edge=bleeding_edge,
        verbose=_effective_flag(verbose, state.verbose),
    )

    printer, repo_root, config_files, _cache = _require_state()
    result = cmd_info(args, printer, config_files, repo_root)
    raise typer.Exit(result)


@_typed_command("status")
def status_cmd() -> None:
    """Show package distribution status."""
    _init_state()

    printer, _repo_root, config_files, _cache = _require_state()
    result = cmd_status(printer, config_files)
    raise typer.Exit(result)


@_typed_command("installed")
def installed_cmd(
    packages: Annotated[list[str], typer.Argument(help="Packages to check")],
    json_output: Annotated[bool, typer.Option("--json", help="JSON output")] = False,
    show_location: OptShowLocation = False,
) -> None:
    """Check if package(s) are installed (exit code 0 if all installed)."""
    _init_state(json_output=json_output if json_output else None)

    args = make_args(
        packages=list(packages),
        json=_effective_flag(json_output, state.json_output),
        show_location=show_location,
    )

    printer, _repo_root, config_files, _cache = _require_state()
    result = cmd_installed(args, printer, config_files)
    raise typer.Exit(result)


@_typed_command("undo")
def undo_cmd() -> None:
    """Undo last changes via git."""
    _init_state()

    printer, repo_root, _config_files, _cache = _require_state()
    result = cmd_undo(printer, repo_root)
    raise typer.Exit(result)


@_typed_command("update", context_settings={"allow_extra_args": True})
def update_cmd(ctx: typer.Context) -> None:
    """Update flake inputs (nix flake update)."""
    _init_state(passthrough=ctx.args)

    args = make_args(passthrough=_effective_passthrough(ctx.args))
    printer, repo_root, _config_files, _cache = _require_state()
    result = cmd_update(args, printer, repo_root)
    raise typer.Exit(result)


@_typed_command("test")
def test_cmd() -> None:
    """Run ruff, mypy, and unit tests for nx."""
    _init_state()
    printer, repo_root, _config_files, _cache = _require_state()
    result = cmd_test(printer, repo_root)
    raise typer.Exit(result)


@_typed_command("rebuild", context_settings={"allow_extra_args": True})
def rebuild_cmd(ctx: typer.Context) -> None:
    """Rebuild system (darwin-rebuild switch)."""
    _init_state(passthrough=ctx.args)

    args = make_args(passthrough=_effective_passthrough(ctx.args))
    printer, repo_root, _config_files, _cache = _require_state()
    result = cmd_rebuild(args, printer, repo_root)
    raise typer.Exit(result)


@_typed_command("upgrade", context_settings={"allow_extra_args": True})
def upgrade_cmd(
    ctx: typer.Context,
    dry_run: OptDryRun = False,
    verbose: OptVerbose = False,
    skip_rebuild: OptSkipRebuild = False,
    skip_commit: OptSkipCommit = False,
    skip_brew: OptSkipBrew = False,
    no_ai: OptNoAi = False,
) -> None:
    """Full upgrade with changelogs: update + brew + rebuild + commit."""
    _init_state(
        dry_run=dry_run,
        verbose=verbose if verbose else None,
        passthrough=ctx.args,
    )

    args = make_args(
        dry_run=_effective_flag(dry_run, state.dry_run),
        verbose=_effective_flag(verbose, state.verbose),
        skip_rebuild=skip_rebuild,
        skip_commit=skip_commit,
        skip_brew=skip_brew,
        no_ai=no_ai,
        passthrough=_effective_passthrough(ctx.args),
    )

    printer, repo_root, _config_files, _cache = _require_state()
    result = cmd_upgrade(args, printer, repo_root)
    raise typer.Exit(result)


# ═══════════════════════════════════════════════════════════════════════════════
# CLI Entry Point
# ═══════════════════════════════════════════════════════════════════════════════

# Known commands (for preprocessing in nx shim)
COMMANDS = {
    "install", "remove", "rm", "where", "list", "info",
    "status", "installed", "undo", "update", "test", "rebuild", "upgrade",
}


def run_cli() -> None:
    """Entry point that preprocesses arguments for default install command."""
    argv = sys.argv[1:]

    # If first arg looks like a package name (not a command, not an option),
    # inject 'install' as the command
    if argv and argv[0] not in COMMANDS and not argv[0].startswith("-"):
        argv = ["install", *argv]

    # Run Typer with potentially modified args
    sys.argv = [sys.argv[0], *argv]
    app()


if __name__ == "__main__":
    run_cli()
