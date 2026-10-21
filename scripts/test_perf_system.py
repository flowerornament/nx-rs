import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

import perf_system


class TraceReportTests(unittest.TestCase):
    def test_invalid_activity_ids_cannot_crash_or_form_intervals(self):
        events = []
        for identifier in [None, [], {}, "bad", True, -1, 2**64]:
            events.extend(
                {
                    "parent": 1,
                    "offset_ns": offset,
                    "data": {"action": action, "id": identifier},
                }
                for action, offset in [("start", 0), ("stop", 10)]
            )
        rows, unmatched = perf_system.nix_activity_intervals(events)
        self.assertEqual(len(rows), 7)
        self.assertTrue(
            all(row["end_ns"] is None and row["invalid_id"] for row in rows)
        )
        self.assertEqual(unmatched, 7)

    def test_colliding_open_nix_ids_are_unknown_instead_of_fabricated_intervals(self):
        starts = [
            {"parent": 1, "offset_ns": offset, "data": {"action": "start", "id": 7}}
            for offset in [0, 10]
        ]
        stop = {"parent": 1, "offset_ns": 20, "data": {"action": "stop", "id": 7}}
        rows, unmatched = perf_system.nix_activity_intervals(starts + [stop, stop])
        self.assertEqual(len(rows), 2)
        self.assertTrue(all(row["end_ns"] is None and row["ambiguous"] for row in rows))
        self.assertEqual(unmatched, 2)

    def test_incomplete_activities_do_not_hide_the_longest_completed_work(self):
        events = [
            {
                "parent": 1,
                "offset_ns": 0,
                "data": {"action": "start", "id": identifier, "name": "unfinished"},
            }
            for identifier in range(25)
        ] + [
            {
                "parent": 1,
                "offset_ns": 10,
                "data": {"action": "start", "id": 99, "name": "FinishedWork"},
            },
            {"parent": 1, "offset_ns": 20, "data": {"action": "stop", "id": 99}},
        ]
        output = io.StringIO()
        with (
            patch.object(
                perf_system,
                "summarize",
                return_value={"spans": [], "nix_activities": events},
            ),
            redirect_stdout(output),
        ):
            perf_system.nix_activities(Path("unused"))
        rendered = output.getvalue()
        self.assertLess(rendered.index("FinishedWork"), rendered.index("unfinished"))
        self.assertIn("Activities: 26; unfinished: 25; unmatched stops: 0", rendered)

    def test_nix_activity_ids_are_scoped_to_commands_and_incomplete_pairs_stay_unknown(
        self,
    ):
        def event(command, action, identifier, offset, **fields):
            return {
                "parent": command,
                "offset_ns": offset,
                "data": {"action": action, "id": identifier, **fields},
            }

        rows, unmatched = perf_system.nix_activity_intervals(
            [
                event(1, "start", 7, 0, name="EvaluateFlakeDerivationOutput"),
                event(2, "start", 7, 10, name="CopySourcePath"),
                event(2, "stop", 7, 30),
                event(1, "result", 7, 35, fields=[1, 2]),
                event(1, "stop", 7, 100),
                event(2, "start", 8, 110, name="QueryMissing"),
                event(2, "stop", 99, 120),
            ]
        )
        self.assertEqual(
            [(r["parent"], r["end_ns"], r["offset_ns"]) for r in rows],
            [(2, 30, 10), (1, 100, 0), (2, None, 110)],
        )
        self.assertEqual(unmatched, 1)
        self.assertEqual(rows[0]["data"]["name"], "CopySourcePath")

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
            self.assertEqual(report["command_work_ns"], 12)
            self.assertEqual(report["command_overlap_ns"], 4)
            self.assertEqual(report["peak_concurrent_commands"], 2)
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
