import io
import json
import sys
import unittest
from contextlib import redirect_stdout
from dataclasses import dataclass
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from commands import (  # noqa: E402
    cmd_info,
    cmd_installed,
    cmd_list,
    cmd_status,
    cmd_undo,
    cmd_where,
)
from config import ConfigFiles  # noqa: E402
from sources import FlakeHubResult, PackageInfo  # noqa: E402


@dataclass
class Args:
    packages: list[str]
    list_source: str | None = None
    json: bool = False
    plain: bool = False
    verbose: bool = False
    show_location: bool = False
    bleeding_edge: bool = False


class DummyPrinter:
    INDENT = "  "

    def __init__(self):
        self.errors = []
        self.infos = []
        self.status_calls = []
        self.status_table_calls = []
        self.confirm_responses = []

    def error(self, text):
        self.errors.append(text)

    def info(self, text):
        self.infos.append(text)

    def command_header(self, *args, **kwargs):
        pass

    def section(self, *args, **kwargs):
        pass

    def multi_column_list(self, *args, **kwargs):
        pass

    def dim(self, *args, **kwargs):
        pass

    def success(self, *args, **kwargs):
        pass

    def location(self, *args, **kwargs):
        pass

    def show_snippet(self, *args, **kwargs):
        pass

    def status(self, message):
        self.status_calls.append(message)
        class _Dummy:
            def __enter__(self_inner):
                return self_inner
            def __exit__(self_inner, *args):
                return False
        return _Dummy()

    def status_table(self, packages):
        self.status_table_calls.append(packages)

    def confirm(self, prompt, default=True):
        self.confirm_responses.append((prompt, default))
        return False

    def warn(self, *args, **kwargs):
        pass

    def suggestion(self, *args, **kwargs):
        pass

    def line(self, *args, **kwargs):
        pass


class CommandsTests(unittest.TestCase):
    def test_cmd_list_plain(self):
        printer = DummyPrinter()
        args = Args(packages=[], plain=True)
        with patch("commands.find_all_packages") as find_all:
            find_all.return_value = {"nxs": ["ripgrep"], "brews": []}
            buf = io.StringIO()
            with redirect_stdout(buf):
                rc = cmd_list(args, printer, config_files=None)
            self.assertEqual(rc, 0)
            self.assertIn("  ripgrep", buf.getvalue())

    def test_cmd_list_unknown_source(self):
        printer = DummyPrinter()
        args = Args(packages=[], list_source="bogus")
        with patch("commands.find_all_packages") as find_all:
            find_all.return_value = {"nxs": ["ripgrep"]}
            rc = cmd_list(args, printer, config_files=None)
        self.assertEqual(rc, 1)
        self.assertTrue(printer.errors)

    def test_cmd_installed_json(self):
        printer = DummyPrinter()
        args = Args(packages=["rg"], json=True)
        with patch("commands.find_package_fuzzy") as find_fuzzy:
            find_fuzzy.return_value = ("ripgrep", "packages/nix/cli.nix:42")
            buf = io.StringIO()
            with redirect_stdout(buf):
                rc = cmd_installed(args, printer, config_files=None)
            self.assertEqual(rc, 0)
            data = json.loads(buf.getvalue())
            self.assertIn("rg", data)
            self.assertEqual(data["rg"]["match"], "ripgrep")

    def test_cmd_where_found(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "hosts").mkdir()
            host = repo / "hosts" / "test.nix"
            host.write_text("{ }: { }\n")
            cfg = ConfigFiles(repo_root=repo)
            printer = DummyPrinter()
            args = Args(packages=["ripgrep"])

            with patch("commands.find_package") as find_pkg:
                find_pkg.return_value = str(repo / "packages" / "nix" / "cli.nix") + ":5"
                rc = cmd_where(args, printer, cfg)
            self.assertEqual(rc, 0)

    def test_cmd_info_json(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "flake.lock").write_text("{}")
            printer = DummyPrinter()
            args = Args(packages=["ripgrep"], json=True)

            info = PackageInfo(
                name="ripgrep",
                source="nxs",
                version="13.0.0",
                description="fast search",
            )

            with (
                patch("commands.find_package", return_value="packages/nix/cli.nix:3"),
                patch("commands.detect_installed_source", return_value=("nxs", None)),
                patch("commands.get_package_info", return_value=[info]),
                patch("commands.get_hm_module_info", return_value=None),
                patch("commands.get_darwin_service_info", return_value=None),
                patch("commands.search_flakehub", return_value=[]),
            ):
                buf = io.StringIO()
                with redirect_stdout(buf):
                    rc = cmd_info(args, printer, config_files=None, repo_root=repo)
                self.assertEqual(rc, 0)
                data = json.loads(buf.getvalue())
                self.assertEqual(data["name"], "ripgrep")
                self.assertTrue(data["installed"])
                self.assertEqual(data["sources"][0]["source"], "nxs")

    def test_cmd_info_json_skips_flakehub_by_default(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "flake.lock").write_text("{}")
            printer = DummyPrinter()
            args = Args(packages=["ripgrep"], json=True, bleeding_edge=False)

            info = PackageInfo(
                name="ripgrep",
                source="nxs",
                version="13.0.0",
                description="fast search",
            )

            with (
                patch("commands.find_package", return_value="packages/nix/cli.nix:3"),
                patch("commands.detect_installed_source", return_value=("nxs", None)),
                patch("commands.get_package_info", return_value=[info]),
                patch("commands.get_hm_module_info", return_value=None),
                patch("commands.get_darwin_service_info", return_value=None),
                patch("commands.search_flakehub", side_effect=RuntimeError("network call")),
            ):
                buf = io.StringIO()
                with redirect_stdout(buf):
                    rc = cmd_info(args, printer, config_files=None, repo_root=repo)
                self.assertEqual(rc, 0)
                data = json.loads(buf.getvalue())
                self.assertEqual(data["name"], "ripgrep")

    def test_cmd_info_json_includes_flakehub_when_bleeding_edge(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "flake.lock").write_text("{}")
            printer = DummyPrinter()
            args = Args(packages=["ripgrep"], json=True, bleeding_edge=True)

            info = PackageInfo(
                name="ripgrep",
                source="nxs",
                version="13.0.0",
                description="fast search",
            )

            with (
                patch("commands.find_package", return_value="packages/nix/cli.nix:3"),
                patch("commands.detect_installed_source", return_value=("nxs", None)),
                patch("commands.get_package_info", return_value=[info]),
                patch("commands.get_hm_module_info", return_value=None),
                patch("commands.get_darwin_service_info", return_value=None),
                patch(
                    "commands.search_flakehub",
                    return_value=[
                        FlakeHubResult(
                            flake_name="Org/Tool",
                            description="Tool flake",
                            visibility="public",
                            version="1.0.0",
                        )
                    ],
                ),
            ):
                buf = io.StringIO()
                with redirect_stdout(buf):
                    rc = cmd_info(args, printer, config_files=None, repo_root=repo)
                self.assertEqual(rc, 0)
                data = json.loads(buf.getvalue())
                self.assertEqual(len(data["flakehub"]), 1)
                self.assertEqual(data["flakehub"][0]["name"], "Org/Tool")

    def test_cmd_status_calls_table(self):
        printer = DummyPrinter()
        with patch("commands.find_all_packages") as find_all:
            find_all.return_value = {"nxs": ["ripgrep"], "brews": []}
            rc = cmd_status(printer, config_files=None)
        self.assertEqual(rc, 0)
        self.assertTrue(printer.status_table_calls)

    def test_cmd_undo_nothing_to_undo(self):
        printer = DummyPrinter()
        with patch("commands.run_command", return_value=(True, "")):
            rc = cmd_undo(printer, repo_root=Path("/tmp"))
        self.assertEqual(rc, 0)


if __name__ == "__main__":
    unittest.main()
