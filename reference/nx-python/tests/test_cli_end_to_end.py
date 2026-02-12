import io
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest.mock import patch


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

import cli  # noqa: E402


class CliEndToEndTests(unittest.TestCase):
    def setUp(self):
        cli.state.printer = None
        cli.state.repo_root = None
        cli.state.config_files = None
        cli.state.cache = None
        cli.state.verbose = False
        cli.state.json_output = False
        cli.state.dry_run = False
        cli.state.yes = False
        cli.state.passthrough = []

    def tearDown(self):
        self.setUp()
    def _write_repo(self, root: Path) -> None:
        (root / "home").mkdir()
        (root / "system").mkdir()
        (root / "hosts").mkdir()
        (root / "home" / "packages.nix").write_text(
            "# nx: CLI tools\n{ pkgs, ... }: {\n  home.packages = with pkgs; [ ripgrep ];\n}\n"
        )
        (root / "system" / "darwin.nix").write_text(
            "# nx: GUI apps\n{ pkgs, ... }: {\n  homebrew.casks = [ \"raycast\" ];\n}\n"
        )
        (root / "hosts" / "test.nix").write_text("{ }: { }\n")
        (root / "flake.lock").write_text("{}")

    def test_list_plain_output(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write_repo(repo)
            with patch.dict("os.environ", {"B2NIX_REPO_ROOT": str(repo)}):
                buf = io.StringIO()
                with redirect_stdout(buf):
                    with self.assertRaises(SystemExit):
                        cli.app(["list", "--plain"])
                out = buf.getvalue()
                self.assertIn("ripgrep", out)

    def test_where_command(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write_repo(repo)
            with patch.dict("os.environ", {"B2NIX_REPO_ROOT": str(repo)}):
                buf = io.StringIO()
                with redirect_stdout(buf):
                    with self.assertRaises(SystemExit):
                        cli.app(["where", "ripgrep"])
                out = buf.getvalue()
                self.assertIn("ripgrep", out)

    def test_info_json(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write_repo(repo)
            with patch.dict("os.environ", {"B2NIX_REPO_ROOT": str(repo)}):
                with (
                    patch("commands.get_package_info", return_value=[]),
                    patch("commands.search_flakehub", return_value=[]),
                ):
                    buf = io.StringIO()
                    with redirect_stdout(buf):
                        with self.assertRaises(SystemExit):
                            cli.app(["info", "ripgrep", "--json"])
                    out = buf.getvalue()
                    self.assertIn('"name": "ripgrep"', out)

    def test_installed_json(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write_repo(repo)
            with patch.dict("os.environ", {"B2NIX_REPO_ROOT": str(repo)}):
                buf = io.StringIO()
                with redirect_stdout(buf):
                    with self.assertRaises(SystemExit):
                        cli.app(["installed", "ripgrep", "--json"])
                self.assertIn("ripgrep", buf.getvalue())

    @patch("commands.subprocess.run")
    @patch("commands._run_indented", return_value=0)
    def test_rebuild_flow(self, run_indented, subprocess_run):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write_repo(repo)
            subprocess_run.return_value = SimpleNamespace(returncode=0, stderr="")
            with patch.dict("os.environ", {"B2NIX_REPO_ROOT": str(repo)}):
                buf = io.StringIO()
                with redirect_stdout(buf):
                    with self.assertRaises(SystemExit):
                        cli.app(["rebuild"])
                self.assertTrue(run_indented.called)


if __name__ == "__main__":
    unittest.main()
