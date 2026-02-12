import json
import sys
import time
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from sources import (  # noqa: E402
    SourcePreferences,
    SourceResult,
    _parallel_search,
    check_nix_available,
    check_overlay_active,
    get_current_system,
    get_darwin_service_info,
    get_hm_module_info,
    get_package_set_info,
    search_all_sources,
    search_flake_inputs,
    search_homebrew,
    search_nur,
    search_nxs,
)


class SourcesTests(unittest.TestCase):
    @patch("sources.shutil.which", return_value="/usr/bin/nix")
    @patch("sources.run_json_command")
    def test_search_nxs_parses_results(self, run_json_command, _which):
        data = {
            "legacyPackages.aarch64-darwin.ripgrep": {
                "pname": "ripgrep",
                "version": "13.0.0",
                "description": "fast search",
            }
        }
        run_json_command.return_value = (True, data)
        results = search_nxs("ripgrep")
        self.assertTrue(results)
        self.assertEqual(results[0].source, "nxs")
        self.assertEqual(results[0].attr, "ripgrep")

    @patch("sources.search_nxs", return_value=[])
    def test_parallel_search_handles_as_completed_timeout(self, _search_nxs):
        with (
            self.assertLogs("sources", level="WARNING") as logs,
            patch("sources.as_completed", side_effect=TimeoutError("timed out")),
        ):
            results = _parallel_search("ripgrep", SourcePreferences())
        self.assertEqual(results, [])
        self.assertTrue(any("Timed out waiting" in line for line in logs.output))

    @patch("sources.as_completed", side_effect=TimeoutError("timed out"))
    @patch("sources.search_nxs")
    def test_parallel_search_timeout_does_not_block_on_slow_sources(
        self,
        search_nxs,
        _as_completed,
    ):
        def slow_search(_name):
            time.sleep(1.0)
            return []

        search_nxs.side_effect = slow_search
        start = time.perf_counter()
        with self.assertLogs("sources", level="WARNING"):
            _parallel_search("ripgrep", SourcePreferences())
        elapsed = time.perf_counter() - start
        self.assertLess(elapsed, 0.8)

    @patch("sources.search_nur")
    @patch("sources.search_nxs")
    def test_parallel_search_returns_partial_results_when_one_source_fails(
        self,
        search_nxs,
        search_nur,
    ):
        search_nxs.side_effect = RuntimeError("nxs failure")
        search_nur.return_value = [
            SourceResult(
                name="ripgrep",
                source="nur",
                attr="nur.repos.owner.ripgrep",
                confidence=0.8,
            )
        ]
        with self.assertLogs("sources", level="WARNING") as logs:
            results = _parallel_search("ripgrep", SourcePreferences(nur=True))

        self.assertEqual(len(results), 1)
        self.assertEqual(results[0].source, "nur")
        self.assertTrue(any("failed" in line.lower() for line in logs.output))

    @patch("sources.shutil.which", return_value="/usr/bin/nix")
    @patch("sources.run_json_command")
    def test_search_nxs_prefers_normalized_alias_match(self, run_json_command, _which):
        def side_effect(cmd, timeout=30):
            term = cmd[-1]
            if term == "pyyaml":
                return True, {
                    "legacyPackages.aarch64-darwin.python313Packages.pyyaml": {
                        "pname": "pyyaml",
                        "description": "YAML parser",
                    }
                }
            if term == "py-yaml":
                return True, {
                    "legacyPackages.aarch64-darwin.python313Packages.aspy-yaml": {
                        "pname": "aspy-yaml",
                        "description": "Few extensions to pyyaml",
                    }
                }
            return False, None

        run_json_command.side_effect = side_effect
        results = search_nxs("py-yaml")
        self.assertTrue(results)
        self.assertEqual(results[0].attr, "python313Packages.pyyaml")
        search_terms = [call.args[0][-1] for call in run_json_command.call_args_list]
        self.assertIn("pyyaml", search_terms)
        self.assertIn("py-yaml", search_terms)

    @patch("sources.shutil.which", return_value="/usr/bin/nix")
    @patch("sources.run_json_command")
    def test_search_nur_requires_flake_mod(self, run_json_command, _which):
        data = {
            "packages.x86_64-linux.ripgrep": {
                "pname": "ripgrep",
                "description": "fast search",
            }
        }
        run_json_command.return_value = (True, data)
        results = search_nur("ripgrep")
        self.assertTrue(results)
        self.assertTrue(results[0].requires_flake_mod)
        self.assertEqual(results[0].flake_url, "github:nix-community/NUR")

    @patch("sources.shutil.which", return_value="/usr/bin/brew")
    @patch("sources.run_json_command")
    def test_search_homebrew_formula(self, run_json_command, _which):
        run_json_command.return_value = (
            True,
            {
                "formulae": [
                    {
                        "name": "ripgrep",
                        "versions": {"stable": "13.0.0"},
                        "desc": "fast search",
                    }
                ]
            },
        )
        results = search_homebrew("ripgrep")
        self.assertEqual(results[0].source, "homebrew")
        self.assertEqual(results[0].attr, "ripgrep")

    @patch("sources.shutil.which", return_value="/usr/bin/brew")
    @patch("sources.run_json_command")
    def test_search_homebrew_falls_back_to_cask(self, run_json_command, _which):
        def side_effect(cmd, timeout=15):
            if "--cask" in cmd:
                return True, {"casks": [{"token": "raycast", "desc": "launcher"}]}
            return False, None

        run_json_command.side_effect = side_effect
        results = search_homebrew("raycast")
        self.assertEqual(results[0].source, "cask")
        self.assertEqual(results[0].attr, "raycast")

    def test_search_flake_inputs(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            lock = {
                "nodes": {
                    "root": {"inputs": {"neovim-nightly-overlay": "neovim-nightly-overlay"}},
                    "neovim-nightly-overlay": {"locked": {"rev": "abc"}},
                }
            }
            lock_path = repo / "flake.lock"
            lock_path.write_text(json.dumps(lock))

            results = search_flake_inputs("neovim", lock_path)
            self.assertTrue(results)
            self.assertEqual(results[0].source, "flake-input")

    def test_search_all_sources_shortcuts(self):
        prefs = SourcePreferences(is_cask=True)
        results = search_all_sources("raycast", prefs)
        self.assertEqual(results[0].source, "cask")

        prefs = SourcePreferences(is_mas=True)
        results = search_all_sources("xcode", prefs)
        self.assertEqual(results[0].source, "mas")

        prefs = SourcePreferences()
        results = search_all_sources("python3Packages.requests", prefs)
        self.assertEqual(results[0].source, "nxs")
        self.assertEqual(results[0].attr, "python3Packages.requests")

    @patch("sources.shutil.which", return_value=None)
    @patch("sources._parallel_search", return_value=[])
    @patch("sources.search_homebrew", return_value=[])
    def test_search_all_sources_language_override_requires_validation(
        self,
        _search_homebrew,
        _parallel_search,
        _which,
    ):
        results = search_all_sources(
            "python3Packages.this-package-should-never-exist-xyz123",
            SourcePreferences(),
        )
        self.assertEqual(results, [])

    @patch("sources.search_homebrew")
    @patch("sources._parallel_search")
    @patch("sources._validate_language_override", return_value=(True, None))
    def test_search_all_sources_uses_language_override_when_valid(
        self,
        _validate_language_override,
        _parallel_search,
        search_homebrew,
    ):
        results = search_all_sources("python3Packages.requests", SourcePreferences())
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0].source, "nxs")
        self.assertEqual(results[0].attr, "python3Packages.requests")
        _parallel_search.assert_not_called()
        search_homebrew.assert_not_called()

    @patch("sources.search_homebrew", return_value=[])
    @patch(
        "sources._parallel_search",
        return_value=[SourceResult(name="foo", source="nxs", attr="foo", confidence=0.6)],
    )
    @patch(
        "sources._validate_language_override",
        return_value=(False, "attribute not found in nixpkgs"),
    )
    def test_search_all_sources_falls_back_when_language_override_invalid(
        self,
        _validate_language_override,
        _parallel_search,
        _search_homebrew,
    ):
        results = search_all_sources(
            "python3Packages.this-package-should-never-exist-xyz123",
            SourcePreferences(),
        )
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0].attr, "foo")
        _parallel_search.assert_called_once()

    def test_get_package_set_info(self):
        info = get_package_set_info("nerd-fonts")
        self.assertIsNotNone(info)
        self.assertEqual(info.source, "nxs-set")

    def test_get_hm_module_info_enabled(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "home").mkdir()
            (repo / "home" / "git.nix").write_text("programs.git.enable = true;\n")
            info = get_hm_module_info("git", repo)
            self.assertIsNotNone(info)
            self.assertTrue(info.is_enabled)

    def test_get_darwin_service_info_enabled(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "system").mkdir()
            (repo / "system" / "darwin.nix").write_text("services.yabai.enable = true;\n")
            info = get_darwin_service_info("yabai", repo)
            self.assertIsNotNone(info)
            self.assertTrue(info.is_enabled)

    def test_check_overlay_active(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "home").mkdir()
            (repo / "home" / "common.nix").write_text(
                "nxs.overlays = [ inputs.neovim-nightly-overlay.overlays.default ];\n"
            )
            lock = {
                "nodes": {
                    "root": {"inputs": {"neovim-nightly-overlay": "neovim-nightly-overlay"}},
                    "neovim-nightly-overlay": {"locked": {"rev": "abc"}},
                }
            }
            (repo / "flake.lock").write_text(json.dumps(lock))

            overlay = check_overlay_active("neovim", repo)
            self.assertEqual(overlay, "neovim-nightly-overlay")


    def test_get_current_system_returns_valid_format(self):
        system = get_current_system()
        parts = system.split("-")
        self.assertEqual(len(parts), 2)
        self.assertIn(parts[0], ("aarch64", "x86_64"))
        self.assertIn(parts[1], ("darwin", "linux"))

    @patch("sources.shutil.which", return_value="/usr/bin/nix")
    @patch("sources._eval_nix_attr")
    def test_check_nix_available_rejects_unsupported_platform(self, eval_attr, _which):
        eval_attr.return_value = (True, ["x86_64-linux"])
        with patch("sources.get_current_system", return_value="aarch64-darwin"):
            available, reason = check_nix_available("roc")
        self.assertFalse(available)
        self.assertIn("not available", reason)
        self.assertIn("x86_64-linux", reason)

    @patch("sources.shutil.which", return_value="/usr/bin/nix")
    @patch("sources._eval_nix_attr")
    def test_check_nix_available_allows_supported_platform(self, eval_attr, _which):
        eval_attr.return_value = (True, ["x86_64-linux", "aarch64-darwin"])
        with patch("sources.get_current_system", return_value="aarch64-darwin"):
            available, reason = check_nix_available("ripgrep")
        self.assertTrue(available)
        self.assertIsNone(reason)

    @patch("sources.shutil.which", return_value="/usr/bin/nix")
    @patch("sources._eval_nix_attr")
    def test_check_nix_available_allows_on_eval_failure(self, eval_attr, _which):
        eval_attr.return_value = (False, None)
        available, reason = check_nix_available("some-package")
        self.assertTrue(available)
        self.assertIsNone(reason)

    @patch("sources.shutil.which", return_value=None)
    def test_check_nix_available_allows_without_nix(self, _which):
        available, reason = check_nix_available("any-package")
        self.assertTrue(available)
        self.assertIsNone(reason)

    @patch("sources.shutil.which", return_value="/usr/bin/nix")
    @patch("sources._eval_nix_attr")
    def test_check_nix_available_ignores_structured_platform_specs(self, eval_attr, _which):
        # meta.platforms with only structured specs (no strings) - be permissive
        eval_attr.return_value = (True, [{"cpu": {"family": "x86_64"}}])
        with patch("sources.get_current_system", return_value="aarch64-darwin"):
            available, reason = check_nix_available("some-package")
        self.assertTrue(available)
        self.assertIsNone(reason)


if __name__ == "__main__":
    unittest.main()
