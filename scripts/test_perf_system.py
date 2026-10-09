import json
import tempfile
import unittest
from pathlib import Path

import perf_system


class TraceReportTests(unittest.TestCase):
    def test_overlap_is_counted_once(self):
        self.assertEqual(perf_system.covered_ns([(2, 8), (5, 10), (12, 15)]), 11)

    def test_incomplete_run_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.jsonl"
            path.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "event": "run-start",
                        "id": 0,
                        "parent": None,
                        "offset_ns": 0,
                        "label": "nx",
                        "code": None,
                    }
                )
                + "\n"
            )
            with self.assertRaisesRegex(ValueError, "incomplete"):
                perf_system.summarize(path)

    def test_parallel_commands_and_failed_child_remain_visible(self):
        rows = [
            ("run-start", 0, None, 0, "nx", None),
            ("phase-start", 1, 0, 1, "summary", None),
            ("command-start", 2, 1, 2, "gh", None),
            ("command-start", 3, 1, 4, "gh", None),
            ("command-end", 2, 1, 8, "gh", 1),
            ("command-end", 3, 1, 10, "gh", 0),
            ("phase-end", 1, 0, 11, "summary", 0),
            ("run-end", 0, None, 12, "nx", 0),
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.jsonl"
            path.write_text(
                "".join(
                    json.dumps(
                        dict(
                            zip(
                                ("event", "id", "parent", "offset_ns", "label", "code"),
                                row,
                            ),
                            version=1,
                        )
                    )
                    + "\n"
                    for row in rows
                )
            )
            report = perf_system.summarize(path)
            self.assertEqual(report["command_covered_ns"], 8)
            self.assertEqual(report["outside_commands_ns"], 4)
            self.assertEqual(report["commands"]["gh"]["failed"], 1)
            self.assertEqual(report["commands"]["gh"]["count"], 2)
            self.assertEqual(report["phases_ns"]["summary"], 10)
            data = [json.loads(line) for line in path.read_text().splitlines()]
            data[4]["code"] = None
            path.write_text("".join(json.dumps(row) + "\n" for row in data))
            unknown = perf_system.summarize(path)["commands"]["gh"]
            self.assertEqual(unknown["failed"], 0)
            self.assertEqual(unknown["unknown"], 1)
            data[-1]["data"] = {"complete": False}
            path.write_text("".join(json.dumps(row) + "\n" for row in data))
            with self.assertRaisesRegex(ValueError, "write failure"):
                perf_system.summarize(path)


if __name__ == "__main__":
    unittest.main()
