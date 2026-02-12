import sys
import unittest
from pathlib import Path
from unittest.mock import patch


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from upgrade.brew_outdated import BrewChangeInfo, BrewOutdated  # noqa: E402
from upgrade.changelog import ChangeInfo, InputChange  # noqa: E402
from upgrade.summarizer import (  # noqa: E402
    generate_commit_message,
    should_use_detailed_summary,
    summarize_brew_change,
    summarize_change,
    summarize_with_claude,
    summarize_with_codex,
)


class SummarizerTests(unittest.TestCase):
    def test_should_use_detailed_summary(self):
        self.assertTrue(should_use_detailed_summary("nxs", 1))
        self.assertTrue(should_use_detailed_summary("foo", 100))
        self.assertFalse(should_use_detailed_summary("foo", 2))

    @patch("upgrade.summarizer.run_codex", return_value=(True, "line1\nline2"))
    def test_summarize_with_codex(self, run_codex):
        summary = summarize_with_codex(["feat: add"], "foo")
        self.assertEqual(summary, "line1")
        self.assertTrue(run_codex.called)

    @patch("upgrade.summarizer.run_claude", return_value=(True, "one\n\nsecond"))
    def test_summarize_with_claude(self, run_claude):
        summary = summarize_with_claude("foo", ["a"], [{"tag_name": "v1", "body": "b"}])
        self.assertIn("one", summary)
        self.assertTrue(run_claude.called)

    @patch("upgrade.summarizer.run_codex", return_value=(True, "summary"))
    def test_summarize_change_routes_to_codex(self, run_codex):
        change = InputChange(name="foo", owner="o", repo="r", old_rev="a", new_rev="b")
        info = ChangeInfo(input_change=change, commit_messages=["c"])
        summary = summarize_change(info)
        self.assertEqual(summary, "summary")
        self.assertTrue(run_codex.called)

    @patch("upgrade.summarizer.run_claude", return_value=(True, "summary"))
    def test_summarize_change_routes_to_claude(self, run_claude):
        change = InputChange(name="nxs", owner="o", repo="r", old_rev="a", new_rev="b")
        info = ChangeInfo(input_change=change, commit_messages=["c"])
        summary = summarize_change(info)
        self.assertEqual(summary, "summary")
        self.assertTrue(run_claude.called)

    @patch("upgrade.summarizer.run_codex", return_value=(True, "brew summary"))
    def test_summarize_brew_change(self, run_codex):
        pkg = BrewOutdated(name="ripgrep", installed_version="1", current_version="2", is_cask=False)
        info = BrewChangeInfo(package=pkg, releases=[{"tag_name": "v2", "body": "b"}])
        summary = summarize_brew_change(info)
        self.assertEqual(summary, "brew summary")
        self.assertTrue(run_codex.called)

    def test_generate_commit_message(self):
        change = InputChange(name="nxs", owner="o", repo="r", old_rev="a", new_rev="b")
        msg = generate_commit_message([change], [])
        self.assertIn("flake (nxs)", msg)

        pkg = BrewOutdated(name="ripgrep", installed_version="1", current_version="2", is_cask=False)
        msg = generate_commit_message([], [pkg])
        self.assertIn("brew (ripgrep)", msg)

        msg = generate_commit_message([], [])
        self.assertEqual(msg, "Update flake inputs")


if __name__ == "__main__":
    unittest.main()
