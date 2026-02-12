import io
import sys
import unittest
from contextlib import redirect_stdout
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from commands import cmd_rebuild, cmd_test, cmd_update, cmd_upgrade  # noqa: E402


@dataclass
class Args:
    passthrough: list[str] | None = None
    dry_run: bool = False
    verbose: bool = False
    skip_rebuild: bool = False
    skip_commit: bool = False
    skip_brew: bool = False
    no_ai: bool = False


class DummyPrinter:
    def __init__(self):
        self.actions = []
        self.errors = []
        self.successes = []
        self.infos = []

    def action(self, text):
        self.actions.append(text)

    def success(self, text):
        self.successes.append(text)

    def info(self, text):
        self.infos.append(text)

    def error(self, text):
        self.errors.append(text)

    def dry_run_banner(self):
        pass

    def status(self, message):
        class _Dummy:
            def __enter__(self_inner):
                return self_inner
            def __exit__(self_inner, *args):
                return False
        return _Dummy()


class CommandsSystemTests(unittest.TestCase):
    @patch("commands.stream_nix_update", return_value=(True, ""))
    def test_cmd_update_success(self, stream_update):
        printer = DummyPrinter()
        args = Args(passthrough=[])
        rc = cmd_update(args, printer, repo_root=Path("/tmp"))
        self.assertEqual(rc, 0)
        self.assertTrue(stream_update.called)

    @patch("commands.stream_nix_update", return_value=(False, ""))
    def test_cmd_update_failure(self, stream_update):
        printer = DummyPrinter()
        args = Args(passthrough=[])
        rc = cmd_update(args, printer, repo_root=Path("/tmp"))
        self.assertEqual(rc, 1)
        self.assertTrue(printer.errors)

    @patch("commands._find_untracked_nix_files", return_value=(True, [], None))
    @patch("commands._run_indented", return_value=0)
    @patch("commands.subprocess.run")
    def test_cmd_rebuild_success(self, subprocess_run, run_indented, _preflight):
        printer = DummyPrinter()
        args = Args(passthrough=[])
        subprocess_run.return_value = SimpleNamespace(returncode=0, stderr="")
        rc = cmd_rebuild(args, printer, repo_root=Path("/tmp"))
        self.assertEqual(rc, 0)
        self.assertTrue(run_indented.called)

    @patch("commands._find_untracked_nix_files", return_value=(True, [], None))
    @patch("commands.subprocess.run")
    def test_cmd_rebuild_flake_check_failure(self, subprocess_run, _preflight):
        printer = DummyPrinter()
        args = Args(passthrough=[])
        subprocess_run.return_value = SimpleNamespace(returncode=1, stderr="err")
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = cmd_rebuild(args, printer, repo_root=Path("/tmp"))
        self.assertEqual(rc, 1)
        self.assertTrue(printer.errors)

    @patch("commands._run_indented", return_value=0)
    @patch("commands.subprocess.run")
    @patch("commands._find_untracked_nix_files", return_value=(True, ["home/new-module.nix"], None))
    def test_cmd_rebuild_untracked_nix_preflight_blocks(self, preflight, subprocess_run, run_indented):
        printer = DummyPrinter()
        args = Args(passthrough=[])
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = cmd_rebuild(args, printer, repo_root=Path("/tmp/repo"))
        self.assertEqual(rc, 1)
        self.assertTrue(preflight.called)
        self.assertIn("home/new-module.nix", buf.getvalue())
        self.assertTrue(printer.errors)
        self.assertFalse(subprocess_run.called)
        self.assertFalse(run_indented.called)

    @patch("commands._run_indented", return_value=0)
    @patch("commands.subprocess.run")
    @patch("commands._find_untracked_nix_files", return_value=(False, [], "git failed"))
    def test_cmd_rebuild_git_preflight_failure(self, preflight, subprocess_run, run_indented):
        printer = DummyPrinter()
        args = Args(passthrough=[])
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = cmd_rebuild(args, printer, repo_root=Path("/tmp/repo"))
        self.assertEqual(rc, 1)
        self.assertTrue(preflight.called)
        self.assertIn("git failed", buf.getvalue())
        self.assertTrue(printer.errors)
        self.assertFalse(subprocess_run.called)
        self.assertFalse(run_indented.called)

    @patch("commands._run_indented", return_value=0)
    def test_cmd_test_success(self, run_indented):
        printer = DummyPrinter()
        rc = cmd_test(printer, repo_root=Path("/tmp"))
        self.assertEqual(rc, 0)
        self.assertEqual(run_indented.call_count, 3)

    @patch("commands.stream_nix_update")
    @patch("commands.fetch_all_changes", return_value=[])
    @patch("commands.diff_locks", return_value=([], [], []))
    @patch("commands.load_flake_lock", return_value={})
    def test_cmd_upgrade_dry_run_skips_update_and_brew(
        self, load_flake_lock, diff_locks, fetch_all_changes, stream_nix_update
    ):
        printer = DummyPrinter()
        args = Args(dry_run=True, skip_brew=True, skip_rebuild=True, skip_commit=True)
        rc = cmd_upgrade(args, printer, repo_root=Path("/tmp"))
        self.assertEqual(rc, 0)
        self.assertFalse(stream_nix_update.called)

    @patch("commands.cmd_rebuild", return_value=0)
    @patch("commands.fetch_all_changes", return_value=[])
    @patch("commands.diff_locks", return_value=([], [], []))
    @patch("commands.load_flake_lock", return_value={})
    @patch("commands.get_outdated", return_value=[])
    def test_cmd_upgrade_rebuild_path(
        self, get_outdated, load_flake_lock, diff_locks, fetch_all_changes, cmd_rebuild
    ):
        printer = DummyPrinter()
        args = Args(dry_run=False, skip_brew=False, skip_rebuild=False, skip_commit=True)
        with patch("commands.stream_nix_update", return_value=(True, "")):
            rc = cmd_upgrade(args, printer, repo_root=Path("/tmp"))
        self.assertEqual(rc, 0)
        self.assertTrue(cmd_rebuild.called)


if __name__ == "__main__":
    unittest.main()
