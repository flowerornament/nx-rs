import json
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from upgrade.changelog import (  # noqa: E402
    diff_locks,
    get_github_token,
    load_flake_lock,
    parse_flake_lock,
    short_rev,
    stream_nix_update,
)


class UpgradeChangelogTests(unittest.TestCase):
    def test_parse_flake_lock_sources(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            lock = {
                "nodes": {
                    "root": {
                        "inputs": {
                            "nixpkgs": "nixpkgs",
                            "flakehub-input": "flakehub-input",
                            "file-input": "file-input",
                            "follows-input": ["other", "path"],
                        }
                    },
                    "nixpkgs": {
                        "locked": {
                            "type": "github",
                            "owner": "NixOS",
                            "repo": "nixpkgs",
                            "rev": "abcdef",
                            "lastModified": 1,
                        }
                    },
                    "flakehub-input": {
                        "locked": {
                            "type": "tarball",
                            "url": "https://api.flakehub.com/f/pinned/Foo/Bar/123.tar.gz",
                            "rev": "123",
                            "lastModified": 2,
                        }
                    },
                    "file-input": {
                        "locked": {
                            "type": "file",
                            "rev": "zzz",
                            "lastModified": 3,
                        }
                    },
                }
            }
            lock_path = repo / "flake.lock"
            lock_path.write_text(json.dumps(lock))

            inputs = parse_flake_lock(lock_path)
            self.assertIn("nixpkgs", inputs)
            self.assertIn("flakehub-input", inputs)
            self.assertNotIn("file-input", inputs)
            self.assertEqual(inputs["flakehub-input"].owner, "Foo")
            self.assertEqual(inputs["flakehub-input"].repo, "Bar")

    def test_diff_locks(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            old_lock = {
                "nodes": {
                    "root": {"inputs": {"nixpkgs": "nixpkgs"}},
                    "nixpkgs": {
                        "locked": {
                            "type": "github",
                            "owner": "NixOS",
                            "repo": "nixpkgs",
                            "rev": "old",
                            "lastModified": 1,
                        }
                    },
                }
            }
            new_lock = {
                "nodes": {
                    "root": {"inputs": {"nixpkgs": "nixpkgs", "hm": "hm"}},
                    "nixpkgs": {
                        "locked": {
                            "type": "github",
                            "owner": "NixOS",
                            "repo": "nixpkgs",
                            "rev": "new",
                            "lastModified": 2,
                        }
                    },
                    "hm": {
                        "locked": {
                            "type": "github",
                            "owner": "nix-community",
                            "repo": "home-manager",
                            "rev": "abc",
                            "lastModified": 3,
                        }
                    },
                }
            }
            old_path = repo / "old.lock"
            new_path = repo / "new.lock"
            old_path.write_text(json.dumps(old_lock))
            new_path.write_text(json.dumps(new_lock))

            old_inputs = parse_flake_lock(old_path)
            new_inputs = parse_flake_lock(new_path)
            changed, added, removed = diff_locks(old_inputs, new_inputs)
            self.assertEqual(len(changed), 1)
            self.assertEqual(changed[0].name, "nixpkgs")
            self.assertIn("hm", added)
            self.assertEqual(removed, [])

    def test_short_rev(self):
        self.assertEqual(short_rev("abcdef123"), "abcdef1")
        self.assertEqual(short_rev(""), "")

    def test_load_flake_lock_missing(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self.assertEqual(load_flake_lock(repo), {})

    @patch("upgrade.changelog.run_command", return_value=(True, "token123\n"))
    def test_get_github_token(self, run_command):
        token = get_github_token()
        self.assertEqual(token, "token123")
        self.assertTrue(run_command.called)

    @patch(
        "upgrade.changelog.run_streaming_command",
        return_value=(1, "error: adding a file to a tree builder"),
    )
    @patch("upgrade.changelog._clear_fetcher_cache", return_value=False)
    @patch("upgrade.changelog.get_github_token", return_value="")
    def test_stream_nix_update_cache_corruption_warns(
        self,
        get_token,
        clear_cache,
        run_stream,
    ):
        class DummyPrinter:
            def __init__(self):
                self.actions = []
                self.warns = []

            def action(self, text):
                self.actions.append(text)

            def warn(self, text):
                self.warns.append(text)

        printer = DummyPrinter()
        ok, output = stream_nix_update(Path("/tmp"), printer=printer)

        self.assertFalse(ok)
        self.assertIn("tree builder", output)
        self.assertTrue(run_stream.called)
        self.assertTrue(get_token.called)
        self.assertTrue(clear_cache.called)
        self.assertTrue(printer.warns)


if __name__ == "__main__":
    unittest.main()
