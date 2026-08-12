#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import io
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parent.parent
RELEASE_PATH = ROOT / "scripts" / "release.py"


def load_release_module():
    spec = importlib.util.spec_from_file_location("release", RELEASE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load release.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


release = load_release_module()


class ReleaseHelperTests(unittest.TestCase):
    def test_package_version_comes_from_package_section(self) -> None:
        version = release.package_version_from_cargo_toml(
            """
[package]
name = "nx-rs"
version = "1.2.3"

[dependencies]
some-crate = { version = "9.9.9" }
"""
        )

        self.assertEqual(version, "1.2.3")

    def test_package_version_requires_package_section(self) -> None:
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            release.package_version_from_cargo_toml(
                """
[dependencies]
some-crate = "9.9.9"
"""
            )

    def test_dirty_worktree_entries_ignores_blank_lines(self) -> None:
        entries = release.dirty_worktree_entries("\n M Cargo.toml\n\n?? scratch\n")

        self.assertEqual(entries, [" M Cargo.toml", "?? scratch"])

    def test_changelog_entry_is_inserted_after_unreleased(self) -> None:
        updated = release.insert_changelog_entry(
            "# Changelog\n\n## Unreleased\n\n## v1.0.0 - 2026-01-01\n\n- old\n",
            "1.1.0",
            "2026-02-03",
        )

        self.assertIn(
            "## Unreleased\n\n## v1.1.0 - 2026-02-03\n\n"
            "- TODO: summarize release changes.\n\n## v1.0.0",
            updated,
        )

    def test_update_release_branch_moves_and_pushes_release_ref(self) -> None:
        calls: list[list[str]] = []

        with patch.object(release, "run", side_effect=calls.append):
            release.update_release_branch("v1.2.3")

        self.assertEqual(
            calls,
            [
                ["git", "branch", "-f", "release", "v1.2.3"],
                [
                    "git",
                    "push",
                    "--force-with-lease",
                    "origin",
                    "refs/heads/release:refs/heads/release",
                ],
            ],
        )


class NixCacheReleaseTests(unittest.TestCase):
    def test_cache_lookup_bypasses_stale_negative_narinfo(self) -> None:
        with patch.object(release, "command_succeeds", return_value=True) as run:
            self.assertTrue(release.cache_contains("/nix/store/nx"))

        run.assert_called_once_with(
            [
                "nix",
                "path-info",
                "--store",
                release.CACHE_URI,
                "--option",
                "narinfo-cache-negative-ttl",
                "0",
                "/nix/store/nx",
            ]
        )

    def test_missing_cache_output_names_system_and_remediation(self) -> None:
        error = io.StringIO()
        with (
            patch.object(
                release, "flake_package_systems", return_value=["aarch64-darwin"]
            ),
            patch.object(
                release,
                "nix_output_path",
                return_value="/nix/store/example-nx-1.5.34",
            ),
            patch.object(release, "cache_contains", return_value=False),
            redirect_stderr(error),
            self.assertRaises(SystemExit),
        ):
            release.verify_release_cache()

        message = error.getvalue()
        self.assertIn("aarch64-darwin", message)
        self.assertIn("/nix/store/example-nx-1.5.34", message)
        self.assertIn("Wait for the Nix Cache workflow", message)

    def test_failed_cache_gate_cannot_mutate_git(self) -> None:
        clean_result = release.subprocess.CompletedProcess(
            args=[], returncode=0, stdout="", stderr=""
        )
        with (
            patch.object(release, "cargo_version", return_value="1.5.34"),
            patch.object(release.subprocess, "run", return_value=clean_result),
            patch.object(release, "verify_release_cache", side_effect=SystemExit(1)),
            patch.object(release, "run") as mutate_git,
            self.assertRaises(SystemExit),
        ):
            release.tag("1.5.34")

        mutate_git.assert_not_called()

    def test_cache_workflow_preserves_substitution_proof(self) -> None:
        workflow = (ROOT / ".github/workflows/nix-cache-package.yml").read_text()

        self.assertIn('if nix path-info "$expected"', workflow)
        self.assertIn("--max-jobs 0", workflow)
        self.assertIn('--option substituters "$CACHE_URI https://cache.nixos.org/"', workflow)

if __name__ == "__main__":
    unittest.main()
