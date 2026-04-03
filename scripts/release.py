#!/usr/bin/env python3

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tomllib
from datetime import date
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+$")


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def write_text(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8")


def cargo_version() -> str:
    data = tomllib.loads(read_text(ROOT / "Cargo.toml"))
    return data["package"]["version"]


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


def workflow_targets() -> list[str]:
    text = read_text(ROOT / ".github/workflows/release.yml")
    return re.findall(r"- target: ([^\n]+)", text)


def installer_targets() -> list[str]:
    text = read_text(ROOT / "install.sh")
    match = re.search(
        r"SUPPORTED_RELEASE_TARGETS=\(\n(?P<body>(?:\s+\"[^\"]+\"\n)+)\)",
        text,
    )
    if match is None:
        fail("could not find SUPPORTED_RELEASE_TARGETS in install.sh")
    return re.findall(r'"([^"]+)"', match.group("body"))


def readme_targets() -> list[str]:
    text = read_text(ROOT / "README.md")
    match = re.search(r"Binaries available for: (.+)\.", text)
    if match is None:
        fail("could not find release target list in README.md")
    return re.findall(r"`([^`]+)`", match.group(1))


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


def changelog_insert_entry(version: str) -> None:
    if changelog_has_entry(version):
        return

    today = date.today().isoformat()
    scaffold = (
        f"## v{version} - {today}\n\n"
        "- TODO: summarize release changes.\n\n"
    )

    text = changelog_text()
    marker = "# Changelog\n\n"
    if marker not in text:
        fail("could not find CHANGELOG.md insertion marker")
    updated = text.replace(marker, marker + scaffold, 1)
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


def host_target() -> str:
    completed = subprocess.run(
        ["bash", "install.sh", "--print-target"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return completed.stdout.strip()


def installer_smoke() -> None:
    dist_dir = ROOT / "target" / "release-installer"
    install_root = ROOT / "target" / "release-installer-smoke"
    dist_dir.mkdir(parents=True, exist_ok=True)
    install_root.mkdir(parents=True, exist_ok=True)

    target = host_target()
    archive = dist_dir / f"nx-{target}.tar.gz"
    subprocess.run(
        ["tar", "czf", str(archive), "-C", str(ROOT / "target" / "release"), "nx"],
        cwd=ROOT,
        check=True,
    )

    env = dict(os.environ)
    env["NX_RS_INSTALL_BASE_URL"] = dist_dir.resolve().as_uri()
    env["INSTALL_DIR"] = str(install_root / "bin")
    subprocess.run(
        ["bash", "install.sh", "--tag", "local"],
        cwd=ROOT,
        check=True,
        env=env,
    )
    subprocess.run(
        [str(install_root / "bin" / "nx"), "--help"],
        cwd=ROOT,
        check=True,
        stdout=subprocess.DEVNULL,
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

    workflow = workflow_targets()
    installer = installer_targets()
    readme = readme_targets()
    if workflow != installer or workflow != readme:
        fail(
            "release targets do not match across release.yml, install.sh, and README.md: "
            f"workflow={workflow}, install={installer}, readme={readme}"
        )

    version = unique_versions.pop()
    if not changelog_entry_is_ready(version):
        fail(
            "CHANGELOG.md must contain a release entry for "
            f"{version} with at least one bullet and no TODO/TBD placeholders"
        )

    run(["just", "ci"])
    run(["just", "test-system"])
    run(["just", "build"])
    run(["bash", "scripts/test-home-manager-module.sh"])
    run(["./target/release/nx", "--help"])
    run(["bash", "install.sh", "--help"])
    run(["bash", "install.sh", "--tag", "local", "--dry-run"])
    installer_smoke()

    print(f"release verification passed for {version}")
    print(f"release targets: {', '.join(workflow)}")


def tag(version: str) -> None:
    if SEMVER_RE.fullmatch(version) is None:
        fail("version must be semver like 1.3.0")
    current = cargo_version()
    if current != version:
        fail(f"Cargo.toml version is {current}, expected {version}")

    status = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    if status.stdout.strip():
        fail("git working tree must be clean before tagging")

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

    run(["git", "tag", "-a", tag_name, "-m", tag_name])
    run(["git", "push", "origin", tag_name])


def main() -> None:
    parser = argparse.ArgumentParser(description="Release helper for nx-rs")
    subparsers = parser.add_subparsers(dest="command", required=True)

    bump_parser = subparsers.add_parser("bump", help="update release versions")
    bump_parser.add_argument("version")

    subparsers.add_parser("verify", help="run release readiness checks")

    tag_parser = subparsers.add_parser("tag", help="create and push a release tag")
    tag_parser.add_argument("version")

    args = parser.parse_args()
    if args.command == "bump":
        bump(args.version)
    elif args.command == "verify":
        verify()
    else:
        tag(args.version)


if __name__ == "__main__":
    main()
