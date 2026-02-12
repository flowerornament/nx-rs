import sys
import unittest
from pathlib import Path


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

import cli  # noqa: E402


class CliArgTests(unittest.TestCase):
    def test_run_cli_injects_install(self):
        original_app = cli.app
        original_argv = sys.argv[:]
        seen = {}

        def fake_app():
            seen["argv"] = sys.argv[:]

        try:
            cli.app = fake_app  # type: ignore[assignment]
            sys.argv = ["nx", "ripgrep"]
            cli.run_cli()
            self.assertEqual(seen["argv"], ["nx", "install", "ripgrep"])
        finally:
            cli.app = original_app  # type: ignore[assignment]
            sys.argv = original_argv

    def test_run_cli_preserves_command(self):
        original_app = cli.app
        original_argv = sys.argv[:]
        seen = {}

        def fake_app():
            seen["argv"] = sys.argv[:]

        try:
            cli.app = fake_app  # type: ignore[assignment]
            sys.argv = ["nx", "list"]
            cli.run_cli()
            self.assertEqual(seen["argv"], ["nx", "list"])
        finally:
            cli.app = original_app  # type: ignore[assignment]
            sys.argv = original_argv


if __name__ == "__main__":
    unittest.main()
