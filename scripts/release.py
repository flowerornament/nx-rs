#!/usr/bin/env python3

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from datetime import date
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+$")
CACHE_NAME = "flowerornament"
CACHE_URI = f"https://{CACHE_NAME}.cachix.org"


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def write_text(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8")


def package_version_from_cargo_toml(text: str) -> str:
    package = re.search(r"(?ms)^\[package\]\s*(.*?)(?:^\[|\Z)", text)
    if package is None:
        fail("could not find [package] section in Cargo.toml")
    match = re.search(r'(?m)^version = "([^"]+)"$', package.group(1))
    if match is None:
        fail("could not find package version in Cargo.toml")
    return match.group(1)


def cargo_version() -> str:
    return package_version_from_cargo_toml(read_text(ROOT / "Cargo.toml"))


def cargo_lock_version() -> str:
    text = read_text(ROOT / "Cargo.lock")
    match = re.search(
        r'name = "nx-rs"\nversion = "([^"]+)"\ndependencies = \[',
        text,
        re.MULTILINE,
    )
    if match is None:
        fail("could not find nx-rs package entry in Cargo.lock")
    return match.group(1)


def flake_version() -> str:
    text = read_text(ROOT / "flake.nix")
    match = re.search(r'(?m)^(\s*)nxVersion = "([^"]+)";$', text)
    if match is None:
        fail("could not find nxVersion in flake.nix")
    return match.group(2)


def flake_package_systems() -> list[str]:
    text = read_text(ROOT / "flake.nix")
    match = re.search(r"(?m)^\s*systems = \[(?P<body>[^]]+)\];$", text)
    if match is None:
        fail("could not find package systems in flake.nix")
    return re.findall(r'"([^"]+)"', match.group("body"))


def cache_workflow_systems() -> list[str]:
    text = read_text(ROOT / ".github/workflows/nix-cache.yml")
    return re.findall(r'"system"\s*:\s*"([^"]+)"', text)


def changelog_text() -> str:
    return read_text(ROOT / "CHANGELOG.md")


def changelog_has_entry(version: str) -> bool:
    pattern = rf"(?m)^## v{re.escape(version)} - \d{{4}}-\d{{2}}-\d{{2}}$"
    return re.search(pattern, changelog_text()) is not None


def changelog_entry(version: str) -> str:
    text = changelog_text()
    heading = re.search(
        rf"(?m)^## v{re.escape(version)} - \d{{4}}-\d{{2}}-\d{{2}}$",
        text,
    )
    if heading is None:
        fail(f"CHANGELOG.md is missing an entry for {version}")

    next_heading = re.search(r"(?m)^## v\d+\.\d+\.\d+ - \d{4}-\d{2}-\d{2}$", text[heading.end() :])
    if next_heading is None:
        return text[heading.end() :]
    return text[heading.end() : heading.end() + next_heading.start()]


def changelog_entry_scaffold(version: str, today: str) -> str:
    return (
        f"## v{version} - {today}\n\n"
        "- TODO: summarize release changes.\n\n"
    )


def insert_changelog_entry(text: str, version: str, today: str) -> str:
    scaffold = changelog_entry_scaffold(version, today)
    unreleased_marker = "## Unreleased\n\n"
    if unreleased_marker in text:
        return text.replace(unreleased_marker, unreleased_marker + scaffold, 1)

    marker = "# Changelog\n\n"
    if marker not in text:
        fail("could not find CHANGELOG.md insertion marker")
    return text.replace(marker, marker + scaffold, 1)


def changelog_insert_entry(version: str) -> None:
    if changelog_has_entry(version):
        return

    today = date.today().isoformat()
    text = changelog_text()
    updated = insert_changelog_entry(text, version, today)
    write_text(ROOT / "CHANGELOG.md", updated)


def changelog_entry_is_ready(version: str) -> bool:
    entry = changelog_entry(version)
    if "TODO:" in entry or "TBD" in entry:
        return False
    return re.search(r"(?m)^- ", entry) is not None


def replace_once(text: str, pattern: str, replacement: str) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.MULTILINE)
    if count != 1:
        fail(f"pattern did not match exactly once: {pattern}")
    return updated


def bump(version: str) -> None:
    if SEMVER_RE.fullmatch(version) is None:
        fail("version must be semver like 1.3.0")

    cargo_toml = ROOT / "Cargo.toml"
    cargo_lock = ROOT / "Cargo.lock"
    flake_nix = ROOT / "flake.nix"

    cargo_text = read_text(cargo_toml)
    cargo_text = replace_once(
        cargo_text,
        r'(?m)^version = "[^"]+"$',
        f'version = "{version}"',
    )
    write_text(cargo_toml, cargo_text)

    lock_text = read_text(cargo_lock)
    lock_text = replace_once(
        lock_text,
        r'name = "nx-rs"\nversion = "[^"]+"\ndependencies = \[',
        f'name = "nx-rs"\nversion = "{version}"\ndependencies = [',
    )
    write_text(cargo_lock, lock_text)

    flake_text = read_text(flake_nix)
    flake_text = replace_once(
        flake_text,
        r'(?m)^(\s*)nxVersion = "[^"]+";$',
        rf'\1nxVersion = "{version}";',
    )
    write_text(flake_nix, flake_text)
    changelog_insert_entry(version)

    print(f"updated release version to {version}")
    print("  - Cargo.toml")
    print("  - Cargo.lock")
    print("  - flake.nix")
    print("  - CHANGELOG.md")


def run(cmd: list[str]) -> None:
    print(f"+ {' '.join(cmd)}")
    subprocess.run(cmd, cwd=ROOT, check=True)


def capture(cmd: list[str]) -> str:
    result = subprocess.run(
        cmd,
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return result.stdout.strip()


def command_succeeds(cmd: list[str]) -> bool:
    return (
        subprocess.run(
            cmd,
            cwd=ROOT,
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode
        == 0
    )


def nix_output_path(system: str) -> str:
    return capture(
        [
            "nix",
            "eval",
            "--accept-flake-config",
            "--raw",
            f".#packages.{system}.default.outPath",
        ]
    )


def cache_contains(path: str) -> bool:
    # Publication probes the same path before and after pushing, so bypass cached misses.
    return command_succeeds(
        [
            "nix",
            "path-info",
            "--store",
            CACHE_URI,
            "--option",
            "narinfo-cache-negative-ttl",
            "0",
            path,
        ]
    )


def verify_release_cache() -> None:
    missing = [
        (system, output)
        for system in flake_package_systems()
        if not cache_contains(output := nix_output_path(system))
    ]
    if missing:
        details = "\n".join(f"  - {system}: {output}" for system, output in missing)
        fail(
            "release outputs are missing from the public Cachix cache:\n"
            f"{details}\n"
            "Wait for the Nix Cache workflow for this commit to succeed, then retry."
        )
    print("all advertised Nix package outputs are present in Cachix")


def update_release_branch(tag_name: str) -> None:
    run(["git", "branch", "-f", "release", tag_name])
    run(
        [
            "git",
            "push",
            "--force-with-lease",
            "origin",
            "refs/heads/release:refs/heads/release",
        ]
    )


def dirty_worktree_entries(status: str) -> list[str]:
    return [line for line in status.splitlines() if line.strip()]


def require_clean_worktree() -> None:
    status = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    entries = dirty_worktree_entries(status.stdout)
    if not entries:
        return

    preview = "\n".join(f"  {entry}" for entry in entries[:10])
    suffix = "" if len(entries) <= 10 else f"\n  ... and {len(entries) - 10} more"
    fail(
        "release verification must run from a clean jj working copy; "
        "commit release-prep changes with `jj commit` first\n"
        f"{preview}{suffix}"
    )


def verify() -> None:
    versions = {
        "Cargo.toml": cargo_version(),
        "Cargo.lock": cargo_lock_version(),
        "flake.nix": flake_version(),
    }
    unique_versions = set(versions.values())
    if len(unique_versions) != 1:
        details = ", ".join(f"{name}={version}" for name, version in versions.items())
        fail(f"release versions do not match: {details}")

    package_systems = flake_package_systems()
    cache_systems = cache_workflow_systems()
    if cache_systems != package_systems:
        fail(
            "Nix cache systems do not match flake.nix: "
            f"flake={package_systems}, workflow={cache_systems}"
        )

    version = unique_versions.pop()
    if not changelog_entry_is_ready(version):
        fail(
            "CHANGELOG.md must contain a release entry for "
            f"{version} with at least one bullet and no TODO/TBD placeholders"
        )

    require_clean_worktree()
    run(["just", "ci"])
    run(["just", "test-system"])
    run(["just", "build"])
    run(["bash", "scripts/test-home-manager-module.sh"])
    run(["bash", "scripts/test-nix-package-consumer.sh"])
    run(["nix", "build", "--accept-flake-config", "."])
    run(["nix", "run", "--accept-flake-config", ".", "--", "--help"])
    run(["./target/release/nx", "--help"])

    print(f"release verification passed for {version}")


def tag(version: str) -> None:
    if SEMVER_RE.fullmatch(version) is None:
        fail("version must be semver like 1.3.0")
    current = cargo_version()
    if current != version:
        fail(f"Cargo.toml version is {current}, expected {version}")

    require_clean_worktree()

    tag_name = f"v{version}"
    tags = subprocess.run(
        ["git", "tag", "--list", tag_name],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    if tags.stdout.strip():
        fail(f"tag {tag_name} already exists")

    verify_release_cache()

    run(["git", "tag", "-a", tag_name, "-m", tag_name])
    run(["git", "push", "origin", tag_name])
    update_release_branch(tag_name)


def main() -> None:
    parser = argparse.ArgumentParser(description="Release helper for nx-rs")
    subparsers = parser.add_subparsers(dest="command", required=True)

    bump_parser = subparsers.add_parser("bump", help="update release versions")
    bump_parser.add_argument("version")

    subparsers.add_parser("verify", help="run release readiness checks")

    tag_parser = subparsers.add_parser("tag", help="create and push a release tag")
    tag_parser.add_argument("version")

    subparsers.add_parser(
        "cache-verify", help="verify every advertised Nix package output is cached"
    )

    args = parser.parse_args()
    if args.command == "bump":
        bump(args.version)
    elif args.command == "verify":
        verify()
    elif args.command == "tag":
        tag(args.version)
    else:
        verify_release_cache()


if __name__ == "__main__":
    main()
