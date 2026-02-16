"""
shared.py - Common utilities for nx v3

Consolidates duplicated code from nx, sources.py, router.py.
This module provides a single source of truth for:
- Package name mappings
- Match scoring algorithm
- Result parsing helpers
- Type definitions (TypedDicts)
"""

from __future__ import annotations

import json
import re
import shlex
import shutil
import subprocess
import textwrap
from pathlib import Path
from typing import Any

# ═══════════════════════════════════════════════════════════════════════════════
# Path Helpers
# ═══════════════════════════════════════════════════════════════════════════════


def relative_path(abs_path: str | Path, repo_root: Path) -> str:
    """Convert absolute path to repo-relative path.

    Args:
        abs_path: Absolute path (string or Path)
        repo_root: Repository root path

    Returns:
        Path relative to repo root (e.g., "packages/nix/cli.nix:42")
    """
    return str(abs_path).replace(str(repo_root) + "/", "")


def split_location(location: str) -> tuple[str, int | None]:
    """Split a location string like 'path:line' into path and line number."""
    parts = location.rsplit(":", 1)
    if len(parts) == 2:
        try:
            return parts[0], int(parts[1])
        except ValueError:
            return parts[0], None
    return location, None


# ═══════════════════════════════════════════════════════════════════════════════
# Source Display Names
# ═══════════════════════════════════════════════════════════════════════════════

# Canonical display names for package sources
SOURCE_DISPLAY_NAMES: dict[str, str] = {
    "nxs": "nxs",
    "brews": "homebrew",
    "casks": "casks",
    "homebrew": "Homebrew formula",
    "cask": "Homebrew cask",
    "mas": "Mac App Store",
    "nur": "NUR",
    "flake-input": "Flake overlay",
}

# Source filter aliases (user input -> canonical key)
SOURCE_FILTER_ALIASES: dict[str, str] = {
    "nxs": "nxs",
    "nix": "nxs",
    "homebrew": "brews",
    "brews": "brews",
    "brew": "brews",
    "casks": "casks",
    "cask": "casks",
    "mas": "mas",
    "services": "services",
    "service": "services",
}


def normalize_source_filter(value: str | None) -> str | None:
    """Normalize a user-provided source filter to a canonical key."""
    if not value:
        return None
    return SOURCE_FILTER_ALIASES.get(value.lower())


def valid_source_filters() -> list[str]:
    """Return sorted list of valid source filter aliases."""
    return sorted(set(SOURCE_FILTER_ALIASES.keys()))


def format_source_display(source: str, attr: str | None = None) -> str:
    """Format source for user display.

    Args:
        source: Source identifier (nxs, cask, homebrew, etc.)
        attr: Optional attribute path for nxs sources

    Returns:
        Human-readable source name
    """
    if source == "nxs" and attr:
        return f"nxs (pkgs.{attr})"
    return SOURCE_DISPLAY_NAMES.get(source, source)


def format_info_source_label(source: str, name: str) -> str:
    """Format PackageInfo source label for display."""
    if source == "nxs":
        return format_source_display("nxs", name)
    if source == "homebrew":
        return "Homebrew formula"
    if source == "cask":
        return "Homebrew cask"
    if source == "nur":
        return f"NUR ({name})"
    if source.startswith("flake:"):
        overlay_name = source.split(":", 1)[1]
        return f"Flake overlay ({overlay_name})"
    return source


def install_hint_for_source(name: str, source: str) -> str | None:
    """Return an install hint for a source, if applicable."""
    if source.startswith("flake:"):
        return f"nx --bleeding-edge {name}"

    hints = {
        "nxs": f"nx {name}",
        "nur": f"nx --nur {name}",
        "homebrew": f"nx --source homebrew {name}",
        "cask": f"nx --cask {name}",
        "mas": f"nx --mas {name}",
    }
    return hints.get(source)


def derive_flake_input_name(flake_url: str) -> str:
    """Derive a flake input name from a flake URL."""
    url = flake_url.strip().rstrip("/")

    name = ""
    if "flakehub.com" in url:
        parts = url.split("/")
        if "f" in parts:
            idx = parts.index("f")
            if idx + 2 < len(parts):
                name = parts[idx + 2]
    if not name and ":" in url and "/" in url:
        after = url.split(":", 1)[1]
        name = after.split("/")[-1]
    if not name:
        name = url.split("/")[-1]

    name = re.sub(r"[^A-Za-z0-9_.-]+", "-", name).strip("-").lower()
    return name or "input"


def format_flake_input_attr(name: str) -> str:
    """Format a flake input attribute name for Nix."""
    if re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", name):
        return name
    return f"\"{name}\""


def add_flake_input(flake_path: Path, flake_url: str, input_name: str | None = None) -> tuple[bool, str]:
    """Insert a flake input into flake.nix."""
    if not flake_path.exists():
        return False, "flake.nix not found"

    content = flake_path.read_text()
    input_name = input_name or derive_flake_input_name(flake_url)

    exists_pattern = rf"^\s*(\"{re.escape(input_name)}\"|{re.escape(input_name)})\.url\s*="
    if re.search(exists_pattern, content, flags=re.MULTILINE):
        return True, f"input '{input_name}' already exists"

    lines = content.splitlines(keepends=True)
    start_idx = None
    for i, line in enumerate(lines):
        if re.search(r"\binputs\s*=\s*\{", line):
            start_idx = i
            break
    if start_idx is None:
        return False, "inputs block not found"

    depth = 0
    end_idx = None
    for j in range(start_idx, len(lines)):
        depth += lines[j].count("{") - lines[j].count("}")
        if depth == 0 and j > start_idx:
            end_idx = j
            break
    if end_idx is None:
        return False, "inputs block end not found"

    match = re.match(r"\s*", lines[start_idx])
    base_indent = match.group(0) if match else ""
    indent = f"{base_indent}  "
    attr = format_flake_input_attr(input_name)
    new_line = f"{indent}{attr}.url = \"{flake_url}\";\n"
    lines.insert(end_idx, new_line)

    flake_path.write_text("".join(lines))
    return True, f"added input '{input_name}'"


# ═══════════════════════════════════════════════════════════════════════════════
# Name Mappings
# ═══════════════════════════════════════════════════════════════════════════════

# Consolidated from nx:140-149 and sources.py:56-70
# Maps common package names to their nxs attribute names
NAME_MAPPINGS: dict[str, str] = {
    # Numeric prefix packages (nix doesn't allow attrs starting with numbers)
    "1password-cli": "_1password-cli",
    "1password": "_1password-gui",

    # Editor aliases
    "nvim": "neovim",
    "vim": "neovim",  # When searching for vim, usually want neovim

    # Python aliases
    "python": "python3",
    "python3": "python3",
    "py-yaml": "pyyaml",
    "py_yaml": "pyyaml",

    # Node aliases
    "node": "nodejs",
    "nodejs": "nodejs",

    # Tool aliases
    "rg": "ripgrep",
    "fd-find": "fd",

    # GNU tools (macOS ships BSD versions, nxs has GNU)
    "grep": "gnugrep",
    "sed": "gnused",
    "make": "gnumake",
    "tar": "gnutar",
    "find": "findutils",
}


# ═══════════════════════════════════════════════════════════════════════════════
# Scoring Algorithm
# ═══════════════════════════════════════════════════════════════════════════════

def score_match(search_name: str, attr: str, pname: str = "") -> float:
    """Score how well an attribute matches the search name.

    Prefers root-level packages (pkgs.redis) over nested ones
    (pkgs.chickenPackages.eggs.redis).

    Args:
        search_name: The name being searched for
        attr: The full attribute path (e.g., "legacyPackages.aarch64-darwin.ripgrep")
        pname: The package's pname if available

    Returns:
        Confidence score from 0.0 to 1.0
    """
    # Extract the actual package name (after legacyPackages.*.*)
    parts = attr.split(".")
    tail = parts[-1] if parts else attr

    # Check if this is a root-level package (legacyPackages.arch.name)
    # vs nested (legacyPackages.arch.someSet.name)
    is_root = len(parts) == 3 if attr.startswith("legacyPackages.") else len(parts) == 1

    # Nesting penalty: deeper packages get lower scores
    nesting_penalty = 0.0
    if not is_root:
        # Penalize nested packages (language bindings, plugin sets, etc.)
        nesting_depth = len(parts) - 3 if attr.startswith("legacyPackages.") else len(parts) - 1
        nesting_penalty = min(0.3, nesting_depth * 0.1)

    search_lower = search_name.lower()
    tail_lower = tail.lower()

    # Ignore separators so queries like "py-yaml" can match "pyyaml".
    search_norm = re.sub(r"[^a-z0-9]+", "", search_lower)
    tail_norm = re.sub(r"[^a-z0-9]+", "", tail_lower)
    pname_norm = re.sub(r"[^a-z0-9]+", "", pname.lower())

    score = 0.3
    if search_lower in tail_lower:
        score = 0.45
    if tail_lower.startswith(search_lower):
        score = 0.60
    if tail.startswith(search_name):
        score = 0.65
    if tail_lower == search_lower:
        score = 0.75
    if tail == search_name:
        score = 0.98 if is_root else 0.80
    if pname == search_name:
        score = 1.0 if is_root else 0.85

    if search_norm and tail_norm == search_norm:
        score = max(score, 0.95 if is_root else 0.82)
    elif search_norm and tail_norm.startswith(search_norm):
        score = max(score, 0.68)
    elif search_norm and search_norm in tail_norm:
        score = max(score, 0.52)

    if search_norm and pname_norm == search_norm:
        score = max(score, 1.0 if is_root else 0.85)

    return score - nesting_penalty


def clean_attr_path(attr: str) -> str:
    """Clean up attribute path from nix search output.

    Removes the legacyPackages.<arch> prefix to get the actual package path.

    Args:
        attr: Full attribute path (e.g., "legacyPackages.aarch64-darwin.ripgrep")

    Returns:
        Cleaned path (e.g., "ripgrep")
    """
    if attr.startswith("legacyPackages."):
        parts = attr.split(".")
        if len(parts) >= 3:
            return ".".join(parts[2:])  # Remove legacyPackages.arch
    return attr


# ═══════════════════════════════════════════════════════════════════════════════
# Result Parsing Helpers
# ═══════════════════════════════════════════════════════════════════════════════

def parse_nix_search_results(data: Any) -> list[dict[str, Any]]:
    """Parse nix search JSON output into a list of entries.

    Handles both dict format (attrPath -> {pname, description, ...})
    and list format (used by some nix versions).

    Args:
        data: Parsed JSON from nix search --json

    Returns:
        List of package entries with attrPath key
    """
    entries: list[dict[str, Any]] = []

    if isinstance(data, dict):
        for k, v in data.items():
            if isinstance(v, dict):
                entry = dict(v)
                entry.setdefault("attrPath", k)
                entries.append(entry)
            else:
                entries.append({"attrPath": k})
    elif isinstance(data, list):
        entries = list(data)

    return entries


# ═══════════════════════════════════════════════════════════════════════════════
# Subprocess Helpers
# ═══════════════════════════════════════════════════════════════════════════════

def run_command(
    cmd: list[str],
    cwd: Path | None = None,
    timeout: int = 30,
) -> tuple[bool, str]:
    """Run a shell command and return (success, stdout).

    Args:
        cmd: Command and arguments as list
        cwd: Working directory (optional)
        timeout: Timeout in seconds (default 30)

    Returns:
        Tuple of (success: bool, output: str)
        On timeout/error, returns (False, error_message)
    """
    try:
        result = subprocess.run(
            cmd,
            cwd=cwd,
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
        )
        return result.returncode == 0, result.stdout.rstrip()
    except subprocess.TimeoutExpired:
        return False, f"Timeout after {timeout}s"
    except Exception as e:
        return False, str(e)


def run_json_command(
    cmd: list[str],
    cwd: Path | None = None,
    timeout: int = 30,
) -> tuple[bool, dict[str, Any] | None]:
    """Run a command expecting JSON output.

    Args:
        cmd: Command and arguments as list
        cwd: Working directory (optional)
        timeout: Timeout in seconds (default 30)

    Returns:
        Tuple of (success: bool, parsed_data: Optional[Dict])
        On failure, returns (False, None)
    """
    success, output = run_command(cmd, cwd=cwd, timeout=timeout)
    if not success or not output:
        return False, None

    try:
        data = json.loads(output)
        return True, data
    except json.JSONDecodeError:
        return False, None


def _print_wrapped_plain_line(line: str, indent: str) -> None:
    """Print one line with wrapped continuation aligned to indent."""
    term_width = shutil.get_terminal_size(fallback=(80, 24)).columns
    wrapper = textwrap.TextWrapper(
        width=max(len(indent) + 20, term_width),
        initial_indent=indent,
        subsequent_indent=indent,
        replace_whitespace=False,
        drop_whitespace=False,
        expand_tabs=False,
    )
    print(wrapper.fill(line))


def run_streaming_command(
    cmd: list[str],
    cwd: Path | None = None,
    printer: Any = None,
    indent: str = "  ",
    skip_blank_lines: bool = False,
    raise_nofile: int | None = None,
) -> tuple[int, str]:
    """Run a command and stream output with consistent indentation.

    Args:
        cmd: Command and arguments.
        cwd: Optional working directory.
        printer: Optional printer with ``stream_line`` method.
        indent: Left padding applied to streamed output.
        skip_blank_lines: If True, suppress blank output lines.
        raise_nofile: Optional soft RLIMIT_NOFILE target for the child process.

    Returns:
        Tuple of (returncode, collected_output).
    """
    process_cmd = cmd
    if raise_nofile:
        quoted_cmd = " ".join(shlex.quote(part) for part in cmd)
        process_cmd = [
            "bash",
            "-lc",
            f"ulimit -n {raise_nofile} >/dev/null 2>&1 || true; exec {quoted_cmd}",
        ]

    process = subprocess.Popen(
        process_cmd,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    assert process.stdout is not None

    output_lines: list[str] = []
    try:
        for raw_line in process.stdout:
            line = raw_line.rstrip("\n")
            stripped = line.rstrip()

            if not stripped:
                if not skip_blank_lines:
                    print()
                continue

            output_lines.append(stripped)
            if printer and hasattr(printer, "stream_line"):
                printer.stream_line(line, indent=indent)
            else:
                _print_wrapped_plain_line(line, indent)
    finally:
        process.stdout.close()

    process.wait()
    return process.returncode, "\n".join(output_lines)


def _read_nx_comment(file_path: Path) -> str | None:
    """Read the # nx: comment from the first line of a file."""
    try:
        with open(file_path) as f:
            first_line = f.readline().strip()
            if first_line.startswith("# nx:"):
                return first_line[5:].strip()
    except Exception:
        pass
    return None


# Language package prefixes that need withPackages treatment
LANG_PACKAGE_PREFIXES = {
    "python3Packages.": ("python3", "withPackages"),
    # Versioned python package sets still belong in the repo's python3.withPackages block.
    "python311Packages.": ("python3", "withPackages"),
    "python312Packages.": ("python3", "withPackages"),
    "python313Packages.": ("python3", "withPackages"),
    "python314Packages.": ("python3", "withPackages"),
    "luaPackages.": ("lua5_4", "withPackages"),
    "lua51Packages.": ("lua5_1", "withPackages"),
    "lua52Packages.": ("lua5_2", "withPackages"),
    "lua53Packages.": ("lua5_3", "withPackages"),
    "lua54Packages.": ("lua5_4", "withPackages"),
    "perlPackages.": ("perl", "withPackages"),
    "rubyPackages.": ("ruby", "withPackages"),
    "haskellPackages.": ("haskellPackages.ghc", "withPackages"),
}


def detect_language_package(package_name: str) -> tuple[str, str, str] | None:
    """Detect if a package is a language-specific package.

    Args:
        package_name: Full package name (e.g., "python3Packages.rich")

    Returns:
        Tuple of (bare_name, runtime, method) or None if not a language package
        e.g., ("rich", "python3", "withPackages")
    """
    for prefix, (runtime, method) in LANG_PACKAGE_PREFIXES.items():
        if package_name.startswith(prefix):
            bare_name = package_name[len(prefix):]
            return (bare_name, runtime, method)
    return None
