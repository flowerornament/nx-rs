import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import perf_nix


class RealMeasurementTests(unittest.TestCase):
    def test_network_activity_is_separate_from_local_copying(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "stderr"
            path.write_text(
                "diagnostic\n"
                + "\n".join(
                    json.dumps(row)
                    for row in [
                        {
                            "action": "start",
                            "type": 101,
                            "fields": ["https://cache/source"],
                        },
                        {"action": "start", "type": 10113, "name": "CopySourcePath"},
                        {"action": "stop", "type": 101},
                    ]
                )
            )
            result = perf_nix.activity_summary(path)
            self.assertEqual(result["download_targets"], [["https://cache/source"]])
            self.assertEqual(result["activity_starts"]["CopySourcePath"], 1)

    def test_timeout_is_retained_and_lock_updates_are_forbidden(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root) / "evidence"
            with (
                patch.object(perf_nix, "identity", return_value="pin"),
                patch.object(subprocess, "check_output", return_value="nix test"),
                patch.object(
                    subprocess, "run", side_effect=subprocess.TimeoutExpired("nix", 1)
                ) as run,
            ):
                perf_nix.record(Path(root), "host", directory, 1, 1)
            manifest = json.loads((directory / "manifest.json").read_text())
            self.assertTrue(manifest["complete"])
            self.assertEqual(len(manifest["samples"]), 3)
            self.assertTrue(
                all(s["timed_out"] and s["code"] is None for s in manifest["samples"])
            )
            for call in run.call_args_list:
                self.assertIn("--no-update-lock-file", call.args[0])
                self.assertNotIn("switch", call.args[0])

    def test_input_drift_leaves_incomplete_evidence(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root) / "evidence"
            with (
                patch.object(perf_nix, "identity", side_effect=["before", "after"]),
                patch.object(subprocess, "check_output", return_value="nix test"),
                patch.object(
                    subprocess,
                    "run",
                    return_value=subprocess.CompletedProcess("nix", 0),
                ),
                self.assertRaisesRegex(ValueError, "changed"),
            ):
                perf_nix.record(Path(root), "host", directory, 1, 1)
            manifest = json.loads((directory / "manifest.json").read_text())
            self.assertFalse(manifest["complete"])
            self.assertEqual(len(manifest["samples"]), 1)


if __name__ == "__main__":
    unittest.main()
