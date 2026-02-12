import json
import sys
import unittest
from pathlib import Path
from unittest.mock import patch


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from ai_helpers import (  # noqa: E402
    edit_via_codex,
    route_package_codex,
    route_package_codex_decision,
)
from claude_ops import (  # noqa: E402
    InsertResult,
    insert_via_claude,
    remove_via_claude,
    run_claude_streaming,
)
from config import ConfigFiles  # noqa: E402
from search import InstallPlan, _install_one_claude, _install_one_turbo  # noqa: E402
from sources import SourceResult  # noqa: E402


class DummyPrinter:
    def __init__(self):
        self.activities = []
        self.errors = []
        self.warns = []
        self.completes = []

    def activity(self, kind, text):
        self.activities.append((kind, text))

    def error(self, text):
        self.errors.append(text)

    def warn(self, text):
        self.warns.append(text)

    def complete(self, text):
        self.completes.append(text)

    def info(self, text):
        pass

    def show_dry_run_preview(self, *args, **kwargs):
        pass

    def result_add(self, *args, **kwargs):
        pass

    def show_snippet(self, *args, **kwargs):
        pass


class AiPathTests(unittest.TestCase):
    @staticmethod
    def _plan(
        sr: SourceResult,
        *,
        package_token: str,
        target_file: str,
        insertion_mode: str = "nix_manifest",
        is_brew: bool = False,
        is_cask: bool = False,
        is_mas: bool = False,
        language_info: tuple[str, str, str] | None = None,
    ) -> InstallPlan:
        return InstallPlan(
            source_result=sr,
            package_token=package_token,
            target_file=target_file,
            insertion_mode=insertion_mode,
            is_brew=is_brew,
            is_cask=is_cask,
            is_mas=is_mas,
            language_info=language_info,
        )

    def test_run_claude_streaming_parses_tool_events(self):
        events = [
            {"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": "Edit", "input": {"file_path": "a"}},
                {"type": "tool_use", "name": "Read", "input": {"file_path": "b"}},
            ]}},
            {"type": "result", "result": "ok", "is_error": False},
        ]

        class FakeProc:
            def __init__(self):
                self.stdout = [json.dumps(e) + "\n" for e in events]
                self.stderr = []
            def wait(self):
                return 0

        printer = DummyPrinter()
        with patch("claude_ops.subprocess.Popen", return_value=FakeProc()):
            ok, out = run_claude_streaming("prompt", Path("/tmp"), printer=printer)
        self.assertTrue(ok)
        self.assertEqual(out, "ok")
        self.assertIn(("editing", "a"), printer.activities)
        self.assertIn(("reading", "b"), printer.activities)

    @patch("ai_helpers.run_codex", return_value=(True, "packages/nix/cli.nix"))
    def test_route_package_codex_home_routing(self, run_codex):
        result = route_package_codex("ripgrep", "context", cwd=Path("/tmp"))
        self.assertEqual(result, "packages/nix/cli.nix")
        self.assertTrue(run_codex.called)

    @patch("ai_helpers.run_codex", return_value=(True, "custom/extras.nix"))
    def test_route_package_codex_decision_respects_candidate_list(self, run_codex):
        target, warning = route_package_codex_decision(
            "ripgrep",
            "context",
            cwd=Path("/tmp"),
            candidate_files=["custom/cli-tools.nix", "custom/extras.nix"],
            default_target="custom/cli-tools.nix",
        )
        self.assertEqual(target, "custom/extras.nix")
        self.assertIsNone(warning)

    @patch("ai_helpers.run_codex", return_value=(True, "custom/cli-tools.nix\ncustom/extras.nix"))
    def test_route_package_codex_decision_warns_on_ambiguous_output(self, run_codex):
        target, warning = route_package_codex_decision(
            "ripgrep",
            "context",
            cwd=Path("/tmp"),
            candidate_files=["custom/cli-tools.nix", "custom/extras.nix"],
            default_target="custom/cli-tools.nix",
        )
        self.assertEqual(target, "custom/cli-tools.nix")
        self.assertIn("Ambiguous routing", warning or "")

    def test_route_package_codex_mcp_override(self):
        result = route_package_codex("codex-mcp", "context", cwd=Path("/tmp"))
        self.assertEqual(result, "packages/nix/cli.nix")

    @patch("ai_helpers.run_codex", return_value=(True, "packages/nix/cli.nix"))
    def test_route_package_codex_ignores_legacy_source_flags(self, _run_codex):
        result = route_package_codex("Keynote", "context", cwd=Path("/tmp"), is_mas=True)
        self.assertEqual(result, "packages/nix/cli.nix")

    @patch("ai_helpers.run_codex", return_value=(True, "ok"))
    def test_edit_via_codex_dry_run_language_pkg(self, run_codex):
        ok, msg = edit_via_codex(
            "python3Packages.rich",
            "packages/nix/languages.nix",
            "",
            Path("/tmp"),
            dry_run=True,
        )
        self.assertTrue(ok)
        self.assertIn("python3", msg)
        self.assertFalse(run_codex.called)

    @patch("search.insert_via_claude")
    @patch("search.run_command", return_value=(True, "diff"))
    def test_install_one_claude_dry_run(self, run_command, insert_via_claude):
        sr = SourceResult(name="ripgrep", source="nxs", attr="ripgrep")
        plan = self._plan(
            sr,
            package_token="ripgrep",
            target_file="packages/nix/cli.nix",
        )
        insert_via_claude.return_value = InsertResult(
            success=True,
            message="ok",
            line_num=1,
            file_path="/tmp/file.nix",
            simulated_line="ripgrep",
        )
        args = type("Args", (), {"dry_run": True, "service": False, "model": None})()
        printer = DummyPrinter()
        ok = _install_one_claude(plan, None, Path("/tmp"), printer, args)
        self.assertTrue(ok)
        self.assertEqual(insert_via_claude.call_args.kwargs["package_token"], "ripgrep")
        self.assertEqual(insert_via_claude.call_args.kwargs["target_file"], "packages/nix/cli.nix")

    @patch("claude_ops.shutil.which", return_value="/usr/bin/claude")
    @patch("claude_ops.run_claude", return_value=(True, "FILE: /tmp/a\nLINE: 3\nCODE: ripgrep"))
    def test_insert_via_claude_dry_run_parsing(self, run_claude, _which):
        cfg = ConfigFiles(repo_root=Path("/tmp"))
        sr = SourceResult(name="ripgrep", source="nxs", attr="ripgrep")
        result = insert_via_claude(sr, Path("/tmp"), cfg, dry_run=True)
        self.assertTrue(result.success)
        self.assertEqual(result.file_path, "/tmp/a")
        self.assertEqual(result.line_num, 3)
        self.assertEqual(result.simulated_line, "ripgrep")

    @patch("claude_ops.shutil.which", return_value="/usr/bin/claude")
    @patch("claude_ops.run_claude")
    def test_insert_via_claude_dry_run_uses_install_plan_contract(self, run_claude, _which):
        cfg = ConfigFiles(repo_root=Path("/tmp"))
        sr = SourceResult(name="py-yaml", source="nxs", attr="python3Packages.pyyaml")
        result = insert_via_claude(
            sr,
            Path("/tmp"),
            cfg,
            dry_run=True,
            package_token="python3Packages.pyyaml",
            target_file="packages/nix/languages.nix",
            insertion_mode="language_with_packages",
        )

        self.assertTrue(result.success)
        self.assertEqual(result.file_path, "packages/nix/languages.nix")
        self.assertEqual(result.simulated_line, "pyyaml")
        self.assertFalse(run_claude.called)

    @patch("claude_ops.run_claude_streaming", return_value=(True, "ok"))
    def test_remove_via_claude_dry_run(self, run_streaming):
        ok, msg = remove_via_claude(
            "ripgrep",
            "/tmp/file.nix:1",
            Path("/tmp"),
            dry_run=True,
        )
        self.assertTrue(ok)
        self.assertIn("DRY RUN", msg)

    @patch("search.edit_via_codex", return_value=(True, "ok"))
    @patch("search.run_command", side_effect=[(True, "before"), (True, "after")])
    @patch("search.find_package", return_value="/tmp/file.nix:1")
    def test_install_one_turbo(self, find_package, run_command, edit_via_codex):
        sr = SourceResult(name="ripgrep", source="nxs", attr="ripgrep")
        plan = self._plan(
            sr,
            package_token="ripgrep",
            target_file="packages/nix/cli.nix",
        )
        args = type("Args", (), {"dry_run": False})()
        printer = DummyPrinter()
        ok = _install_one_turbo(plan, None, Path("/tmp"), printer, args)
        self.assertTrue(ok)

    @patch("search.edit_via_codex", return_value=(True, "ok"))
    @patch("search.run_command", side_effect=[(True, "before"), (True, "after")])
    @patch("search.find_package", return_value="/tmp/file.nix:1")
    def test_install_engines_consume_same_plan_contract(
        self,
        find_package,
        run_command,
        edit_via_codex,
    ):
        sr = SourceResult(name="py-yaml", source="nxs", attr="python3Packages.pyyaml")
        plan = self._plan(
            sr,
            package_token="python3Packages.pyyaml",
            target_file="packages/nix/languages.nix",
            insertion_mode="language_with_packages",
            language_info=("pyyaml", "python3", "withPackages"),
        )
        turbo_args = type("Args", (), {"dry_run": False})()
        claude_args = type("Args", (), {"dry_run": True, "service": False, "model": None})()
        printer = DummyPrinter()

        with patch(
            "search.insert_via_claude",
            return_value=InsertResult(success=True, message="ok", file_path="/tmp/file.nix"),
        ) as insert_via_claude:
            claude_ok = _install_one_claude(plan, None, Path("/tmp"), printer, claude_args)

        turbo_ok = _install_one_turbo(plan, None, Path("/tmp"), printer, turbo_args)
        self.assertTrue(claude_ok)
        self.assertTrue(turbo_ok)
        self.assertEqual(insert_via_claude.call_args.kwargs["package_token"], plan.package_token)
        self.assertEqual(insert_via_claude.call_args.kwargs["target_file"], plan.target_file)
        self.assertEqual(edit_via_codex.call_args.args[0], plan.package_token)
        self.assertEqual(edit_via_codex.call_args.args[1], plan.target_file)


if __name__ == "__main__":
    unittest.main()
