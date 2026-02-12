import io
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path


def _add_nx_path():
    nx_root = Path(__file__).resolve().parents[1]
    if str(nx_root) not in sys.path:
        sys.path.insert(0, str(nx_root))


_add_nx_path()

from printer import Printer  # noqa: E402


class PrinterLayoutTests(unittest.TestCase):
    def test_detail_wrap_preserves_indent(self):
        printer = Printer()
        if not printer.has_rich or not printer.console:
            self.skipTest("rich not available")

        printer.console.width = 40
        buf = io.StringIO()
        with redirect_stdout(buf):
            printer.detail("https://example.com/" + ("a" * 90))

        lines = [line for line in buf.getvalue().splitlines() if line]
        self.assertGreater(len(lines), 1)
        for line in lines:
            self.assertTrue(line.startswith("    "), line)

    def test_stream_line_wrap_preserves_indent(self):
        printer = Printer()
        if not printer.has_rich or not printer.console:
            self.skipTest("rich not available")

        printer.console.width = 36
        buf = io.StringIO()
        with redirect_stdout(buf):
            printer.stream_line("https://example.com/" + ("b" * 80))

        lines = [line for line in buf.getvalue().splitlines() if line]
        self.assertGreater(len(lines), 1)
        for line in lines:
            self.assertTrue(line.startswith("  "), line)


if __name__ == "__main__":
    unittest.main()
