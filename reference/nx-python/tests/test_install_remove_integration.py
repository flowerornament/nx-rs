import io
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

import cli  # noqa: E402
from sources import SourceResult  # noqa: E402


class InstallRemoveIntegrationTests(unittest.TestCase):
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

    def _write_repo(self, root: Path) -> None:
        (root / "home").mkdir()
        (root / "system").mkdir()
        (root / "hosts").mkdir()
        (root / "home" / "packages.nix").write_text(
            "# nx: CLI tools\n"
            "{ pkgs, ... }: {\n"
            "  home.packages = with pkgs; [\n"
            "    ripgrep\n"
            "  ];\n"
            "}\n"
        )
        (root / "system" / "darwin.nix").write_text(
            "# nx: GUI apps\n{ pkgs, ... }: {\n  homebrew.casks = [ \"raycast\" ];\n}\n"
        )
        (root / "hosts" / "test.nix").write_text("{ }: { }\n")
        (root / "flake.lock").write_text("{}")

    @patch("search.search_all_sources")
    def test_install_dry_run(self, search_all_sources):
        sr = SourceResult(name="bat", source="nxs", attr="bat")
        search_all_sources.return_value = [sr]
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write_repo(repo)
            with patch.dict("os.environ", {"B2NIX_REPO_ROOT": str(repo)}):
                buf = io.StringIO()
                with redirect_stdout(buf):
                    with self.assertRaises(SystemExit):
                        cli.app(["install", "bat", "--dry-run", "--yes", "--engine", "codex"])
                out = buf.getvalue()
                self.assertIn("Dry Run", out)

    def test_remove_dry_run(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write_repo(repo)
            with patch.dict("os.environ", {"B2NIX_REPO_ROOT": str(repo)}):
                buf = io.StringIO()
                with redirect_stdout(buf):
                    with self.assertRaises(SystemExit):
                        cli.app(["rm", "ripgrep", "--dry-run", "--yes"])
                out = buf.getvalue()
                self.assertIn("Would remove", out)


if __name__ == "__main__":
    unittest.main()
