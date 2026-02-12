import sys
import unittest
from pathlib import Path


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from cli import make_args  # noqa: E402


class CliArgsTests(unittest.TestCase):
    def test_make_args_defaults(self):
        args = make_args()
        self.assertEqual(args.packages, [])
        self.assertEqual(args.engine, "codex")
        self.assertEqual(args.passthrough, [])

    def test_make_args_overrides(self):
        args = make_args(packages=["ripgrep"], engine="claude", passthrough=["-v"])
        self.assertEqual(args.packages, ["ripgrep"])
        self.assertEqual(args.engine, "claude")
        self.assertEqual(args.passthrough, ["-v"])


if __name__ == "__main__":
    unittest.main()
