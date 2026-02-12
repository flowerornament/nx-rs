import sys
import unittest
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from config import ConfigFiles  # noqa: E402
from search import (  # noqa: E402
    _build_install_plan,
    _get_unique_alternatives,
    _handle_flake_mod,
    _install_packages_impl,
    filter_installable,
    search_packages,
    show_search_results,
)
from sources import SourcePreferences, SourceResult  # noqa: E402


class DummyPrinter:
    INDENT = "  "

    def __init__(self):
        self.warns = []
        self.sections = []
        self.numbered = []

    def section(self, title, count=0, tag=""):
        self.sections.append((title, count, tag))

    def heading(self, text):
        pass

    def numbered_option(self, num, text):
        self.numbered.append((num, text))

    def kv_line(self, key, value, indent=0):
        pass

    def package_line(self, name, source, desc=""):
        pass

    def success(self, text):
        pass

    def error(self, text):
        pass

    def warn(self, text):
        self.warns.append(text)

    def confirm(self, prompt, default=True):
        return True

    def info(self, text):
        pass

    class _Status:
        def __enter__(self):
            return self

        def __exit__(self, *_):
            return False

    def status(self, _message):
        return self._Status()

    def activity(self, *_args):
        pass


class DummyCache:
    def __init__(self):
        self.set_many_calls = 0

    def get_all(self, _name):
        return []

    def set_many(self, _results):
        self.set_many_calls += 1


@dataclass
class Args:
    yes: bool = True
    dry_run: bool = False
    cask: bool = False
    source: str | None = None


class SearchTests(unittest.TestCase):
    def test_build_install_plan_rejects_missing_attr(self):
        repo_root = Path("/tmp/repo")
        config_files = ConfigFiles(repo_root=repo_root)
        sr = SourceResult(name="py-yaml", source="nxs", attr=None, confidence=0.95)

        plan, error = _build_install_plan(sr, config_files, repo_root, "context")

        self.assertIsNone(plan)
        self.assertIn("Missing resolved attribute", error or "")

    @patch("search.route_package_codex_decision")
    def test_build_install_plan_language_targets_languages_manifest(self, route_package_codex_decision):
        repo_root = Path("/tmp/repo")
        config_files = ConfigFiles(repo_root=repo_root)
        sr = SourceResult(name="py-yaml", source="nxs", attr="python3Packages.pyyaml", confidence=0.95)

        plan, error = _build_install_plan(sr, config_files, repo_root, "context")

        self.assertIsNone(error)
        assert plan is not None
        self.assertEqual(plan.target_file, "packages/nix/languages.nix")
        self.assertEqual(plan.insertion_mode, "language_with_packages")
        self.assertEqual(plan.language_info, ("pyyaml", "python3", "withPackages"))
        route_package_codex_decision.assert_not_called()

    def test_build_install_plan_cask_targets_cask_manifest(self):
        repo_root = Path("/tmp/repo")
        config_files = ConfigFiles(repo_root=repo_root)
        sr = SourceResult(name="raycast", source="cask", attr="raycast", confidence=0.95)

        plan, error = _build_install_plan(sr, config_files, repo_root, "context")

        self.assertIsNone(error)
        assert plan is not None
        self.assertEqual(plan.target_file, "packages/homebrew/casks.nix")
        self.assertEqual(plan.insertion_mode, "homebrew_manifest")
        self.assertTrue(plan.is_cask)

    @patch("search.route_package_codex_decision", return_value=("packages/nix/cli.nix", None))
    def test_build_install_plan_routes_general_nix_packages(self, route_package_codex_decision):
        repo_root = Path("/tmp/repo")
        config_files = ConfigFiles(repo_root=repo_root)
        sr = SourceResult(name="ripgrep", source="nxs", attr="ripgrep", confidence=0.95)

        plan, error = _build_install_plan(sr, config_files, repo_root, "routing context")

        self.assertIsNone(error)
        assert plan is not None
        self.assertEqual(plan.target_file, "packages/nix/cli.nix")
        self.assertEqual(plan.insertion_mode, "nix_manifest")
        route_package_codex_decision.assert_called_once()

    @patch("search.route_package_codex_decision", return_value=("custom/cli-tools.nix", None))
    def test_build_install_plan_uses_discovered_manifest_candidates(
        self,
        route_package_codex_decision,
    ):
        repo_root = Path("/tmp/repo")
        config_files = ConfigFiles(
            repo_root=repo_root,
            by_purpose={
                "CLI tools and utilities": repo_root / "custom" / "cli-tools.nix",
                "extra tools bucket": repo_root / "custom" / "extras.nix",
                "language runtimes and toolchains": repo_root / "custom" / "languages.nix",
            },
        )
        sr = SourceResult(name="ripgrep", source="nxs", attr="ripgrep", confidence=0.95)

        plan, error = _build_install_plan(sr, config_files, repo_root, "routing context")

        self.assertIsNone(error)
        assert plan is not None
        self.assertEqual(plan.target_file, "custom/cli-tools.nix")
        kwargs = route_package_codex_decision.call_args.kwargs
        self.assertEqual(kwargs["default_target"], "custom/cli-tools.nix")
        self.assertEqual(kwargs["candidate_files"], ["custom/cli-tools.nix", "custom/extras.nix"])

    @patch(
        "search.route_package_codex_decision",
        return_value=("packages/nix/cli.nix", "Ambiguous routing for rg; using fallback packages/nix/cli.nix"),
    )
    def test_build_install_plan_surfaces_routing_warning(self, route_package_codex_decision):
        repo_root = Path("/tmp/repo")
        config_files = ConfigFiles(repo_root=repo_root)
        sr = SourceResult(name="rg", source="nxs", attr="ripgrep", confidence=0.95)

        plan, error = _build_install_plan(sr, config_files, repo_root, "routing context")

        self.assertIsNone(error)
        assert plan is not None
        self.assertIsNotNone(plan.routing_warning)
        self.assertIn("Ambiguous routing", plan.routing_warning or "")
        route_package_codex_decision.assert_called_once()

    def test_get_unique_alternatives(self):
        alts = {
            "ripgrep": [
                SourceResult(name="ripgrep", source="nxs", attr="ripgrep"),
                SourceResult(name="ripgrep", source="nxs", attr="ripgrep"),
                SourceResult(name="ripgrep", source="cask", attr="ripgrep"),
            ]
        }
        unique = _get_unique_alternatives("ripgrep", alts)
        sources = {u.source for u in unique}
        self.assertEqual(sources, {"nxs", "cask"})

    def test_filter_installable_warns_on_cask_alt(self):
        printer = DummyPrinter()
        args = Args(yes=True)
        results = [
            SourceResult(name="ripgrep", source="nxs", attr="ripgrep"),
            SourceResult(name="unknown", source="unknown"),
        ]
        alternatives = {
            "ripgrep": [
                SourceResult(name="ripgrep", source="nxs", attr="ripgrep"),
                SourceResult(name="ripgrep", source="cask", attr="ripgrep"),
            ]
        }

        to_install = filter_installable(results, printer, args, alternatives)
        self.assertEqual(len(to_install), 1)
        self.assertTrue(any("Homebrew cask" in w for w in printer.warns))

    def test_filter_installable_selects_alternative(self):
        printer = DummyPrinter()
        args = Args(yes=False)
        results = [SourceResult(name="rg", source="nxs", attr="ripgrep")]
        alternatives = {
            "rg": [
                SourceResult(name="rg", source="nxs", attr="ripgrep"),
                SourceResult(name="rg", source="cask", attr="ripgrep"),
            ]
        }

        with patch("builtins.input", return_value="2"):
            to_install = filter_installable(results, printer, args, alternatives)
        self.assertEqual(to_install[0].source, "cask")

    def test_handle_flake_mod_blocks_without_prompt(self):
        printer = DummyPrinter()
        args = Args(yes=True)
        sr = SourceResult(name="pkg", source="nur", requires_flake_mod=True, flake_url="url")
        ok = _handle_flake_mod(sr, args, printer, Path("/tmp"), allow_prompt=False)
        self.assertFalse(ok)
        self.assertTrue(printer.warns)

    def test_handle_flake_mod_adds_input(self):
        printer = DummyPrinter()
        args = Args(yes=True)
        sr = SourceResult(name="pkg", source="nur", requires_flake_mod=True, flake_url="url")
        with patch("search.add_flake_input", return_value=(True, "ok")) as add_input:
            ok = _handle_flake_mod(sr, args, printer, Path("/tmp"), allow_prompt=True)
        self.assertTrue(ok)
        add_input.assert_called_once()

    def test_show_search_results_renders_found_section(self):
        printer = DummyPrinter()
        results = [SourceResult(name="ripgrep", source="nxs", attr="ripgrep")]
        show_search_results(results, printer, alternatives={})
        self.assertIn(("Found", 1, ""), printer.sections)

    @patch("search.search_all_sources")
    @patch("search.find_package")
    def test_search_packages_marks_alias_installed_via_resolved_attr(self, find_package, search_all_sources):
        repo_root = Path("/tmp/repo")
        location = str(repo_root / "packages" / "nix" / "languages.nix") + ":43"

        def find_side_effect(name, _config_files):
            if name == "pyyaml":
                return location
            return None

        find_package.side_effect = find_side_effect
        search_all_sources.return_value = [
            SourceResult(
                name="py-yaml",
                source="nxs",
                attr="python3Packages.pyyaml",
                confidence=0.95,
            )
        ]

        printer = DummyPrinter()
        cache = DummyCache()
        args = SimpleNamespace(explain=False)

        results, alternatives = search_packages(
            packages=["py-yaml"],
            args=args,
            printer=printer,
            repo_root=repo_root,
            config_files=object(),
            cache=cache,
            source_prefs=SourcePreferences(),
        )

        self.assertEqual(results[0].source, "installed")
        self.assertEqual(results[0].attr, "packages/nix/languages.nix:43")
        self.assertEqual(alternatives, {})
        self.assertEqual(cache.set_many_calls, 0)

    @patch("search.search_all_sources")
    @patch("search.find_package")
    def test_search_packages_checks_installed_across_all_candidates(
        self,
        find_package,
        search_all_sources,
    ):
        repo_root = Path("/tmp/repo")
        location = str(repo_root / "packages" / "nix" / "languages.nix") + ":43"

        def find_side_effect(name, _config_files):
            if name in {"pyyaml", "python3Packages.pyyaml"}:
                return location
            return None

        find_package.side_effect = find_side_effect
        search_all_sources.return_value = [
            SourceResult(
                name="py-yaml",
                source="nxs",
                attr="python3Packages.aspy-yaml",
                confidence=0.99,
            ),
            SourceResult(
                name="py-yaml",
                source="nxs",
                attr="python3Packages.pyyaml",
                confidence=0.95,
            ),
        ]

        printer = DummyPrinter()
        cache = DummyCache()
        args = SimpleNamespace(explain=False)

        results, alternatives = search_packages(
            packages=["py-yaml"],
            args=args,
            printer=printer,
            repo_root=repo_root,
            config_files=object(),
            cache=cache,
            source_prefs=SourcePreferences(),
        )

        self.assertEqual(results[0].source, "installed")
        self.assertEqual(results[0].attr, "packages/nix/languages.nix:43")
        self.assertEqual(alternatives, {})
        self.assertEqual(cache.set_many_calls, 0)

    @patch("search.search_all_sources")
    @patch("search.find_package")
    def test_search_packages_checks_installed_across_cached_candidates(
        self,
        find_package,
        search_all_sources,
    ):
        repo_root = Path("/tmp/repo")
        location = str(repo_root / "packages" / "nix" / "languages.nix") + ":43"

        def find_side_effect(name, _config_files):
            if name in {"pyyaml", "python3Packages.pyyaml"}:
                return location
            return None

        class CacheWithCandidates(DummyCache):
            def get_all(self, _name):
                return [
                    SourceResult(
                        name="py-yaml",
                        source="nxs",
                        attr="python3Packages.aspy-yaml",
                        confidence=0.99,
                    ),
                    SourceResult(
                        name="py-yaml",
                        source="nxs",
                        attr="python3Packages.pyyaml",
                        confidence=0.95,
                    ),
                ]

        find_package.side_effect = find_side_effect
        search_all_sources.return_value = []

        printer = DummyPrinter()
        cache = CacheWithCandidates()
        args = SimpleNamespace(explain=False)

        results, alternatives = search_packages(
            packages=["py-yaml"],
            args=args,
            printer=printer,
            repo_root=repo_root,
            config_files=object(),
            cache=cache,
            source_prefs=SourcePreferences(),
        )

        self.assertEqual(results[0].source, "installed")
        self.assertEqual(results[0].attr, "packages/nix/languages.nix:43")
        self.assertEqual(alternatives, {})
        search_all_sources.assert_not_called()


    @patch("search.check_nix_available", return_value=(False, "not available on aarch64-darwin (only: x86_64-linux)"))
    def test_install_skips_unavailable_platform(self, _check):
        printer = DummyPrinter()
        args = Args(yes=True)

        sr = SourceResult(name="roc", source="nxs", attr="roc", confidence=0.95)
        install_calls = []

        def fake_install_one(plan, config_files, repo_root, printer, args):
            install_calls.append(plan.source_result.name)
            return True

        count = _install_packages_impl(
            [sr],
            object(),  # config_files
            Path("/tmp"),
            printer,
            args,
            install_one=fake_install_one,
            allow_prompt=True,
            routing_context="context",
        )

        self.assertEqual(count, 0)
        self.assertEqual(install_calls, [])

    @patch("search.route_package_codex_decision", return_value=("packages/nix/cli.nix", None))
    @patch("search.check_nix_available", return_value=(True, None))
    def test_install_proceeds_for_available_platform(self, _check, _route_package_codex_decision):
        printer = DummyPrinter()
        args = Args(yes=True)

        sr = SourceResult(name="ripgrep", source="nxs", attr="ripgrep", confidence=0.95)
        install_calls = []

        def fake_install_one(plan, config_files, repo_root, printer, args):
            install_calls.append(plan.source_result.name)
            return True

        count = _install_packages_impl(
            [sr],
            ConfigFiles(repo_root=Path("/tmp")),
            Path("/tmp"),
            printer,
            args,
            install_one=fake_install_one,
            allow_prompt=True,
            routing_context="context",
        )

        self.assertEqual(count, 1)
        self.assertEqual(install_calls, ["ripgrep"])

    @patch("search.check_nix_available")
    def test_install_uses_same_source_fallback_when_primary_is_unavailable(self, check_nix_available):
        printer = DummyPrinter()
        args = Args(yes=True)
        primary = SourceResult(
            name="py-yaml",
            source="nxs",
            attr="python3Packages.aspy-yaml",
            confidence=0.99,
        )
        fallback = SourceResult(
            name="py-yaml",
            source="nxs",
            attr="python3Packages.pyyaml",
            confidence=0.95,
        )

        def availability_side_effect(attr: str):
            if attr == "python3Packages.aspy-yaml":
                return False, "not available on aarch64-darwin (only: x86_64-linux)"
            if attr == "python3Packages.pyyaml":
                return True, None
            return True, None

        check_nix_available.side_effect = availability_side_effect

        install_attrs = []

        def fake_install_one(plan, config_files, repo_root, printer, args):
            install_attrs.append(plan.source_result.attr)
            return True

        count = _install_packages_impl(
            [primary],
            ConfigFiles(repo_root=Path("/tmp")),
            Path("/tmp"),
            printer,
            args,
            install_one=fake_install_one,
            allow_prompt=True,
            routing_context="context",
            alternatives_by_name={"py-yaml": [primary, fallback]},
        )

        self.assertEqual(count, 1)
        self.assertEqual(install_attrs, ["python3Packages.pyyaml"])
        self.assertTrue(any("trying python3Packages.pyyaml" in warn for warn in printer.warns))

    @patch(
        "search.route_package_codex_decision",
        return_value=("packages/nix/cli.nix", "Ambiguous routing for rg; using fallback packages/nix/cli.nix"),
    )
    @patch("search.check_nix_available", return_value=(True, None))
    def test_install_warns_on_ambiguous_routing_fallback(
        self,
        _check,
        _route_package_codex_decision,
    ):
        printer = DummyPrinter()
        args = Args(yes=True)

        sr = SourceResult(name="rg", source="nxs", attr="ripgrep", confidence=0.95)

        def fake_install_one(plan, config_files, repo_root, printer, args):
            return True

        count = _install_packages_impl(
            [sr],
            ConfigFiles(repo_root=Path("/tmp")),
            Path("/tmp"),
            printer,
            args,
            install_one=fake_install_one,
            allow_prompt=True,
            routing_context="context",
        )

        self.assertEqual(count, 1)
        self.assertTrue(any("Ambiguous routing" in warn for warn in printer.warns))


if __name__ == "__main__":
    unittest.main()
