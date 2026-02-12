import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from config import get_config_files  # noqa: E402
from finder import (  # noqa: E402
    _finder_index_build_count,
    _reset_finder_index_cache,
    find_all_packages,
    find_package,
    find_package_fuzzy,
)


class ConfigFinderTests(unittest.TestCase):
    def setUp(self):
        _reset_finder_index_cache()

    def _write(self, root: Path, rel: str, content: str) -> Path:
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        return path

    def test_config_files_discovery(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write(
                repo,
                "packages/nix/cli.nix",
                "# nx: CLI tools and utilities\n{ pkgs, ... }: { }\n",
            )
            self._write(
                repo,
                "system/darwin.nix",
                "# nx: macOS GUI apps\n{ pkgs, ... }: { }\n",
            )
            self._write(repo, "hosts/test.nix", "{ }: { }\n")

            cfg = get_config_files(repo)

            self.assertEqual(cfg.packages, repo / "packages/nix/cli.nix")
            self.assertEqual(cfg.darwin, repo / "system/darwin.nix")
            self.assertIn("CLI tools and utilities", cfg.by_purpose)
            self.assertIn(repo / "packages/nix/cli.nix", cfg.all_files)

    def test_find_package_and_fuzzy(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write(repo, "hosts/test.nix", "{ }: { }\n")
            self._write(
                repo,
                "packages/nix/cli.nix",
                "# nx: CLI tools and utilities\n"
                "{ pkgs, ... }: {\n"
                "  home.packages = with pkgs; [\n"
                "    ripgrep\n"
                "    lua5_4\n"
                "    neovim\n"
                "  ];\n"
                "  vim = \"nvim\";\n"
                "}\n",
            )

            cfg = get_config_files(repo)

            ripgrep_loc = find_package("ripgrep", cfg)
            self.assertTrue(ripgrep_loc.endswith("packages/nix/cli.nix:4"))

            nvim_loc = find_package("nvim", cfg)
            self.assertTrue(nvim_loc.endswith("packages/nix/cli.nix:6"))

            matched, loc = find_package_fuzzy("lua", cfg)
            self.assertEqual(matched, "lua5_4")
            self.assertTrue(loc.endswith("packages/nix/cli.nix:5"))

    def test_find_all_packages(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write(repo, "hosts/test.nix", "{ }: { }\n")
            self._write(
                repo,
                "packages/nix/cli.nix",
                "# nx: CLI tools and utilities\n"
                "{ pkgs, ... }: {\n"
                "  home.packages = with pkgs; [\n"
                "    ripgrep\n"
                "    fd\n"
                "  ];\n"
                "}\n",
            )
            self._write(
                repo,
                "system/darwin.nix",
                "# nx: macOS GUI apps\n"
                "{ pkgs, ... }: {\n"
                "  homebrew.brews = [\n"
                "    \"htop\"\n"
                "  ];\n"
                "  homebrew.casks = [\n"
                "    \"raycast\"\n"
                "  ];\n"
                "  homebrew.masApps = {\n"
                "    \"Xcode\" = 497799835;\n"
                "  };\n"
                "  launchd.agents.foobar = { };\n"
                "}\n",
            )

            cfg = get_config_files(repo)
            packages = find_all_packages(cfg)

            self.assertEqual(packages["nxs"], ["ripgrep", "fd"])
            self.assertEqual(packages["brews"], ["htop"])
            self.assertEqual(packages["casks"], ["raycast"])
            self.assertEqual(packages["mas"], ["Xcode"])
            self.assertEqual(packages["services"], ["foobar"])

    def test_finder_index_reuses_cache_when_files_unchanged(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write(repo, "hosts/test.nix", "{ }: { }\n")
            self._write(
                repo,
                "packages/nix/cli.nix",
                "# nx: CLI tools and utilities\n"
                "{ pkgs, ... }: {\n"
                "  home.packages = with pkgs; [\n"
                "    ripgrep\n"
                "    fd\n"
                "  ];\n"
                "}\n",
            )

            cfg = get_config_files(repo)
            before = _finder_index_build_count()
            _ = find_all_packages(cfg)
            mid = _finder_index_build_count()

            # Repeated operations should reuse index without rebuilding.
            _ = find_all_packages(cfg)
            _ = find_package("ripgrep", cfg)
            _ = find_package("fd", cfg)
            after = _finder_index_build_count()

            self.assertEqual(mid, before + 1)
            self.assertEqual(after, mid)

    def test_finder_index_refreshes_after_file_change(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._write(repo, "hosts/test.nix", "{ }: { }\n")
            cli_file = self._write(
                repo,
                "packages/nix/cli.nix",
                "# nx: CLI tools and utilities\n"
                "{ pkgs, ... }: {\n"
                "  home.packages = with pkgs; [\n"
                "    ripgrep\n"
                "  ];\n"
                "}\n",
            )

            cfg = get_config_files(repo)
            _ = find_all_packages(cfg)
            first_builds = _finder_index_build_count()

            # Modify file (mtime/signature change) and verify index refresh.
            cli_file.write_text(
                "# nx: CLI tools and utilities\n"
                "{ pkgs, ... }: {\n"
                "  home.packages = with pkgs; [\n"
                "    ripgrep\n"
                "    fd\n"
                "  ];\n"
                "}\n",
            )

            packages = find_all_packages(cfg)
            second_builds = _finder_index_build_count()

            self.assertEqual(second_builds, first_builds + 1)
            self.assertEqual(packages["nxs"], ["ripgrep", "fd"])


if __name__ == "__main__":
    unittest.main()
