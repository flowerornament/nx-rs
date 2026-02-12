import json
import os
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from cache import MultiSourceCache  # noqa: E402
from sources import SourceResult  # noqa: E402


class CacheTests(unittest.TestCase):
    def _write_flake_lock(self, repo: Path) -> None:
        lock = {
            "nodes": {
                "root": {"inputs": {"nixpkgs": "nixpkgs", "nur": "nur"}},
                "nixpkgs": {"locked": {"rev": "abcdef1234567890"}},
                "nur": {"locked": {"rev": "0123456789abcdef"}},
            }
        }
        (repo / "flake.lock").write_text(__import__("json").dumps(lock))

    def test_revision_keying(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp) / "repo"
            repo.mkdir()
            self._write_flake_lock(repo)

            home = Path(tmp) / "home"
            home.mkdir()
            old_home = os.environ.get("HOME")
            os.environ["HOME"] = str(home)
            try:
                cache = MultiSourceCache(repo)
                self.assertEqual(cache.get_revision("nxs"), "abcdef123456")
                self.assertEqual(cache.get_revision("nur"), "0123456789ab")
            finally:
                if old_home is None:
                    os.environ.pop("HOME", None)
                else:
                    os.environ["HOME"] = old_home

    def test_homebrew_only_is_ignored(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp) / "repo"
            repo.mkdir()
            self._write_flake_lock(repo)

            home = Path(tmp) / "home"
            home.mkdir()
            old_home = os.environ.get("HOME")
            os.environ["HOME"] = str(home)
            try:
                cache = MultiSourceCache(repo)
                cache.set(
                    SourceResult(
                        name="ripgrep",
                        source="homebrew",
                        attr="ripgrep",
                        confidence=0.8,
                        description="",
                    )
                )
                self.assertEqual(cache.get_all("ripgrep"), [])
            finally:
                if old_home is None:
                    os.environ.pop("HOME", None)
                else:
                    os.environ["HOME"] = old_home

    def test_nxs_present_returns_results(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp) / "repo"
            repo.mkdir()
            self._write_flake_lock(repo)

            home = Path(tmp) / "home"
            home.mkdir()
            old_home = os.environ.get("HOME")
            os.environ["HOME"] = str(home)
            try:
                cache = MultiSourceCache(repo)
                cache.set(
                    SourceResult(
                        name="ripgrep",
                        source="nxs",
                        attr="ripgrep",
                        confidence=0.9,
                        description="",
                    )
                )
                results = cache.get_all("ripgrep")
                self.assertEqual(len(results), 1)
                self.assertEqual(results[0].source, "nxs")
            finally:
                if old_home is None:
                    os.environ.pop("HOME", None)
                else:
                    os.environ["HOME"] = old_home

    def test_cache_schema_mismatch_invalidates_old_entries(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp) / "repo"
            repo.mkdir()
            self._write_flake_lock(repo)

            home = Path(tmp) / "home"
            home.mkdir()
            old_home = os.environ.get("HOME")
            os.environ["HOME"] = str(home)
            try:
                cache = MultiSourceCache(repo)
                cache.cache_path.write_text(
                    json.dumps(
                        {
                            "schema_version": -1,
                            "ripgrep|nxs|abcdef123456": {
                                "attr": "ripgrep",
                                "version": None,
                                "description": "fast search",
                                "confidence": 0.9,
                                "requires_flake_mod": False,
                                "flake_url": None,
                            },
                        }
                    )
                )

                reloaded = MultiSourceCache(repo)
                self.assertEqual(reloaded.get_all("ripgrep"), [])
            finally:
                if old_home is None:
                    os.environ.pop("HOME", None)
                else:
                    os.environ["HOME"] = old_home

    def test_cache_normalizes_alias_keys(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp) / "repo"
            repo.mkdir()
            self._write_flake_lock(repo)

            home = Path(tmp) / "home"
            home.mkdir()
            old_home = os.environ.get("HOME")
            os.environ["HOME"] = str(home)
            try:
                cache = MultiSourceCache(repo)
                cache.set(
                    SourceResult(
                        name="py-yaml",
                        source="nxs",
                        attr="python3Packages.pyyaml",
                        confidence=0.9,
                        description="YAML parser",
                    )
                )
                results = cache.get_all("pyyaml")
                self.assertEqual(len(results), 1)
                self.assertEqual(results[0].attr, "python3Packages.pyyaml")
            finally:
                if old_home is None:
                    os.environ.pop("HOME", None)
                else:
                    os.environ["HOME"] = old_home


if __name__ == "__main__":
    unittest.main()
