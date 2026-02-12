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

from upgrade.brew_outdated import (  # noqa: E402
    BrewOutdated,
    enrich_package_info,
    extract_github_info,
    fetch_brew_changelog,
    filter_releases_by_version,
    get_outdated,
)


class BrewOutdatedTests(unittest.TestCase):
    @patch("upgrade.brew_outdated.run_command")
    def test_get_outdated_parses_json(self, run_command):
        data = {
            "formulae": [
                {
                    "name": "ripgrep",
                    "installed_versions": ["1"],
                    "current_version": "2",
                }
            ],
            "casks": [
                {
                    "name": "raycast",
                    "installed_versions": "1",
                    "current_version": "2",
                }
            ],
        }
        run_command.return_value = (True, json.dumps(data))
        outdated = get_outdated()
        self.assertEqual(len(outdated), 2)
        self.assertTrue(any(p.is_cask for p in outdated))
        self.assertTrue(any(not p.is_cask for p in outdated))

    @patch("upgrade.brew_outdated.run_command")
    def test_enrich_package_info(self, run_command):
        packages = [
            BrewOutdated(name="ripgrep", installed_version="1", current_version="2", is_cask=False),
            BrewOutdated(name="raycast", installed_version="1", current_version="2", is_cask=True),
        ]

        def side_effect(cmd, timeout=60):
            if "--cask" in cmd:
                data = {"casks": [{"token": "raycast", "homepage": "h2", "desc": "d2"}]}
            else:
                data = {"formulae": [{"name": "ripgrep", "homepage": "h1", "desc": "d1"}]}
            return True, json.dumps(data)

        run_command.side_effect = side_effect
        enriched = enrich_package_info(packages)
        self.assertEqual(enriched[0].homepage, "h1")
        self.assertEqual(enriched[1].description, "d2")

    def test_extract_github_info(self):
        self.assertEqual(extract_github_info("https://github.com/owner/repo"), ("owner", "repo"))
        self.assertEqual(extract_github_info("https://github.com/owner/repo.git"), ("owner", "repo"))
        self.assertIsNone(extract_github_info("https://example.com"))

    def test_filter_releases_by_version(self):
        releases = [
            {"tag_name": "v3"},
            {"tag_name": "v2"},
            {"tag_name": "v1"},
        ]
        filtered = filter_releases_by_version(releases, "1", "3")
        self.assertEqual([r["tag_name"] for r in filtered], ["v3", "v2", "v1"])

    @patch("upgrade.brew_outdated.fetch_github_releases")
    def test_fetch_brew_changelog(self, fetch_releases):
        fetch_releases.return_value = [
            {"tag_name": "v2", "body": "notes"},
            {"tag_name": "v1", "body": "older"},
        ]
        pkg = BrewOutdated(
            name="ripgrep",
            installed_version="1",
            current_version="2",
            is_cask=False,
            homepage="https://github.com/BurntSushi/ripgrep",
        )
        info = fetch_brew_changelog(pkg)
        self.assertTrue(info.releases)
        self.assertIn("v2", info.release_notes)


if __name__ == "__main__":
    unittest.main()
