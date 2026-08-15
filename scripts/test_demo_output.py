import errno
import fcntl
import os
import pathlib
import pty
import re
import select
import signal
import struct
import termios
import time
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
SNAPSHOT = ROOT / "tests/fixtures/output/demo-output.txt"
CSI = re.compile(r"\x1b\[([0-9;?]*)([ -/]*)([@-~])")
DEMO_TIMEOUT_SECONDS = 120


def run_demo() -> str:
    pid, master = pty.fork()
    if pid == 0:
        os.chdir(ROOT)
        env = os.environ.copy()
        env.pop("NO_COLOR", None)
        env.update(TERM="xterm-256color", CARGO_TERM_COLOR="never")
        os.execvpe("bash", ["bash", "scripts/demo-output.sh"], env)

    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 100, 0, 0))
    chunks = []
    deadline = time.monotonic() + DEMO_TIMEOUT_SECONDS
    status = None
    try:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([master], [], [], remaining)[0]:
                raise TimeoutError(f"demo-output exceeded {DEMO_TIMEOUT_SECONDS} seconds")
            try:
                chunk = os.read(master, 65_536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not chunk:
                break
            chunks.append(chunk)
        _, status = os.waitpid(pid, 0)
    finally:
        if status is None:
            try:
                os.killpg(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)
        os.close(master)

    if os.waitstatus_to_exitcode(status) != 0:
        raise AssertionError(b"".join(chunks).decode(errors="replace"))
    return b"".join(chunks).decode(errors="replace")


def terminal_screen(stream: str) -> str:
    lines = []
    line = []
    cursor = 0
    index = 0
    while index < len(stream):
        match = CSI.match(stream, index)
        if match:
            params, _, command = match.groups()
            if command == "K":
                if params in {"", "0"}:
                    del line[cursor:]
                elif params == "1":
                    line[: min(cursor + 1, len(line))] = " " * min(cursor + 1, len(line))
                elif params == "2":
                    line.clear()
            index = match.end()
            continue

        char = stream[index]
        index += 1
        if char == "\r":
            cursor = 0
        elif char == "\n":
            lines.append("".join(line).rstrip())
            line = []
            cursor = 0
        elif char >= " ":
            if cursor == len(line):
                line.append(char)
            elif cursor < len(line):
                line[cursor] = char
            else:
                line.extend(" " * (cursor - len(line)))
                line.append(char)
            cursor += 1

    if line:
        lines.append("".join(line).rstrip())
    return "\n".join(lines).strip("\n") + "\n"


class DemoOutputTest(unittest.TestCase):
    def test_terminal_screen_models_erase_line_modes(self) -> None:
        self.assertEqual(terminal_screen("abc\rx\x1b[0Kz\n"), "xz\n")
        self.assertEqual(terminal_screen("abc\rxy\x1b[2Kz\n"), "  z\n")

    def test_final_terminal_screen_matches_reference(self) -> None:
        stream = run_demo()
        self.assert_native_nix_passthrough(stream)
        screen = terminal_screen(stream)
        self.assertEqual(screen, SNAPSHOT.read_text())
        self.assertEqual(stream.count("error: flake evaluation failed"), 1)
        self.assertEqual(
            line_style(stream, "> Preparing output demo"),
            line_style(stream, "> Checking nx routing metadata"),
        )
        self.assertEqual(
            line_style(stream, "+ Output demo ready"),
            line_style(stream, "+ Routing metadata passed"),
        )
        self.assertEqual(
            line_style(stream, "+ Output demo complete"),
            line_style(stream, "+ Routing metadata passed"),
        )

        lines = screen.splitlines()
        self.assertNotIn(("", ""), zip(lines, lines[1:], strict=False))
        for index, line in enumerate(lines):
            if line.startswith("  $ "):
                self.assertEqual(lines[index - 1], "")
                self.assertNotEqual(lines[index + 1], "")

    def assert_native_nix_passthrough(self, stream: str) -> None:
        expected = {
            native_progress("bar", "flake inputs"): 1,
            native_progress("bar", "flake check"): 3,
            native_progress("bar", "system build"): 1,
            native_progress("default", "activation"): 1,
            native_progress("bar-with-logs", "flake check"): 1,
        }
        for sequence, count in expected.items():
            self.assertEqual(stream.count(sequence), count)

        self.assertNotIn("  [nix-native:", stream)
        self.assertNotIn("@nix ", stream)


def line_style(stream: str, text: str) -> tuple[str, str]:
    match = re.search(
        rf"(\x1b\[[0-9;]*m)(?:\r?\n)?{re.escape(text)}(\x1b\[[0-9;]*m)", stream
    )
    if match is None:
        raise AssertionError(f"missing styled line: {text}")
    return match.group(1), match.group(2)


def native_progress(log_format: str, label: str) -> str:
    return (
        f"\x1b[35m[nix-native:{log_format}]\x1b[0m {label} 1/2\r"
        f"\x1b[2K\r\x1b[32m[nix-native:{log_format}]\x1b[0m {label} complete\r\n"
    )


if __name__ == "__main__":
    unittest.main()
