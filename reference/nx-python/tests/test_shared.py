import io
import os
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

from ai_helpers import build_routing_context, detect_mcp_tool  # noqa: E402
from shared import (  # noqa: E402
    add_flake_input,
    clean_attr_path,
    derive_flake_input_name,
    detect_language_package,
    format_flake_input_attr,
    format_info_source_label,
    format_source_display,
    install_hint_for_source,
    normalize_source_filter,
    parse_nix_search_results,
    relative_path,
    run_streaming_command,
    score_match,
    split_location,
    valid_source_filters,
)


class SharedTests(unittest.TestCase):
    def test_split_location(self):
        path, line = split_location("packages/nix/cli.nix:42")
        self.assertEqual(path, "packages/nix/cli.nix")
        self.assertEqual(line, 42)

        path, line = split_location("packages/nix/cli.nix")
        self.assertEqual(path, "packages/nix/cli.nix")
        self.assertIsNone(line)

        path, line = split_location("packages/nix/cli.nix:abc")
        self.assertEqual(path, "packages/nix/cli.nix")
        self.assertIsNone(line)

        path, line = split_location("packages/nix/cli.nix:12:34")
        self.assertEqual(path, "packages/nix/cli.nix:12")
        self.assertEqual(line, 34)

    def test_score_match_prefers_root(self):
        root = score_match("ripgrep", "legacyPackages.aarch64-darwin.ripgrep")
        nested = score_match("ripgrep", "legacyPackages.aarch64-darwin.python3Packages.ripgrep")
        self.assertGreater(root, nested)
        self.assertGreaterEqual(root, 0.9)

    def test_score_match_normalizes_separators(self):
        exact = score_match("py-yaml", "legacyPackages.aarch64-darwin.python313Packages.pyyaml")
        partial = score_match("py-yaml", "legacyPackages.aarch64-darwin.python313Packages.aspy-yaml")
        self.assertGreater(exact, partial)

    def test_clean_attr_path(self):
        cleaned = clean_attr_path("legacyPackages.aarch64-darwin.ripgrep")
        self.assertEqual(cleaned, "ripgrep")
        self.assertEqual(clean_attr_path("pkgs.ripgrep"), "pkgs.ripgrep")

    def test_parse_nix_search_results(self):
        data = {"foo": {"pname": "foo"}, "bar": {"pname": "bar"}}
        entries = parse_nix_search_results(data)
        self.assertEqual(len(entries), 2)
        self.assertIn("attrPath", entries[0])

        data_list = [{"attrPath": "baz"}]
        entries = parse_nix_search_results(data_list)
        self.assertEqual(entries, data_list)

    def test_detect_language_package(self):
        info = detect_language_package("python3Packages.rich")
        self.assertEqual(info, ("rich", "python3", "withPackages"))
        versioned = detect_language_package("python313Packages.pyyaml")
        self.assertEqual(versioned, ("pyyaml", "python3", "withPackages"))
        self.assertIsNone(detect_language_package("ripgrep"))

    def test_detect_mcp_tool(self):
        self.assertTrue(detect_mcp_tool("codex-mcp"))
        self.assertTrue(detect_mcp_tool("mcp-server-foo"))
        self.assertFalse(detect_mcp_tool("ripgrep"))

    def test_format_source_display(self):
        self.assertEqual(format_source_display("nxs", "ripgrep"), "nxs (pkgs.ripgrep)")
        self.assertEqual(format_source_display("brews"), "homebrew")
        self.assertEqual(format_source_display("cask"), "Homebrew cask")

    def test_install_hint_for_source(self):
        self.assertEqual(install_hint_for_source("rg", "nxs"), "nx rg")
        self.assertEqual(install_hint_for_source("rg", "cask"), "nx --cask rg")
        self.assertIsNone(install_hint_for_source("rg", "unknown"))

    def test_format_info_source_label(self):
        self.assertEqual(format_info_source_label("nxs", "ripgrep"), "nxs (pkgs.ripgrep)")
        self.assertEqual(format_info_source_label("homebrew", "rg"), "Homebrew formula")
        self.assertEqual(format_info_source_label("flake:overlay", "rg"), "Flake overlay (overlay)")

    def test_derive_flake_input_name(self):
        self.assertEqual(derive_flake_input_name("github:nix-community/NUR"), "nur")
        self.assertEqual(derive_flake_input_name("https://flakehub.com/f/Org/Project/1"), "project")

    def test_format_flake_input_attr(self):
        self.assertEqual(format_flake_input_attr("nixpkgs"), "nixpkgs")
        self.assertEqual(format_flake_input_attr("foo-bar"), "\"foo-bar\"")

    def test_add_flake_input(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            flake = repo / "flake.nix"
            flake.write_text(
                "{\n"
                "  inputs = {\n"
                "    nixpkgs.url = \"github:NixOS/nixpkgs\";\n"
                "  };\n"
                "}\n"
            )
            ok, _ = add_flake_input(flake, "github:nix-community/NUR", input_name="nur")
            self.assertTrue(ok)
            self.assertIn("nur.url", flake.read_text())
            ok2, _ = add_flake_input(flake, "github:nix-community/NUR", input_name="nur")
            self.assertTrue(ok2)

    def test_valid_source_filters_and_normalize(self):
        self.assertIn("cask", valid_source_filters())
        self.assertEqual(normalize_source_filter("CASK"), "casks")
        self.assertIsNone(normalize_source_filter("nope"))

    def test_relative_path(self):
        repo_root = Path("/tmp/repo")
        path = Path("/tmp/repo/packages/nix/cli.nix:42")
        self.assertEqual(relative_path(path, repo_root), "packages/nix/cli.nix:42")

    def test_build_routing_context(self):
        with TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "home").mkdir()
            (repo / "packages" / "nix").mkdir(parents=True)
            (repo / "system").mkdir()
            (repo / "hosts").mkdir()
            (repo / "packages" / "nix" / "cli.nix").write_text("# nx: CLI tools\n{ }: { }\n")
            (repo / "system" / "darwin.nix").write_text("# nx: GUI apps\n{ }: { }\n")
            (repo / "hosts" / "test.nix").write_text("{ }: { }\n")

            context = build_routing_context(repo)
            self.assertIn("packages/nix/cli.nix → CLI tools", context)
            self.assertIn("system/darwin.nix → GUI apps", context)

    def test_run_streaming_command_uses_printer_stream_line(self):
        class CapturePrinter:
            def __init__(self):
                self.lines = []

            def stream_line(self, text, indent="  "):
                self.lines.append((text, indent))

        printer = CapturePrinter()
        rc, output = run_streaming_command(
            [sys.executable, "-c", "print('one');print('two')"],
            printer=printer,
        )
        self.assertEqual(rc, 0)
        self.assertEqual(output, "one\ntwo")
        self.assertEqual(printer.lines, [("one", "  "), ("two", "  ")])

    def test_run_streaming_command_plain_wrap_preserves_indent(self):
        with patch("shared.shutil.get_terminal_size", return_value=os.terminal_size((30, 24))):
            buf = io.StringIO()
            with redirect_stdout(buf):
                rc, _ = run_streaming_command(
                    [sys.executable, "-c", "print('https://example.com/' + 'a' * 80)"],
                    printer=None,
                )

        self.assertEqual(rc, 0)
        lines = [line for line in buf.getvalue().splitlines() if line]
        self.assertGreater(len(lines), 1)
        for line in lines:
            self.assertTrue(line.startswith("  "), line)


if __name__ == "__main__":
    unittest.main()
