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

    def test_evaluation_cache_control_is_per_command(self):
        with tempfile.TemporaryDirectory() as root:
            with (
                patch.object(perf_nix, "identity", return_value="pin"),
                patch.object(subprocess, "check_output", return_value="nix test"),
                patch.object(
                    subprocess,
                    "run",
                    return_value=subprocess.CompletedProcess("nix", 0),
                ) as run,
            ):
                perf_nix.record(
                    Path(root),
                    "host",
                    Path(root) / "evidence",
                    1,
                    1,
                    compare_eval_cache=True,
                )
            calls = [call.args[0] for call in run.call_args_list]
            controls = [args for args in calls if "--option" in args]
            self.assertEqual(len(controls), 1)
            self.assertIn("--dry-run", controls[0])
            self.assertEqual(controls[0][4:6], ["eval-cache", "false"])

    def test_input_drift_leaves_incomplete_evidence(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root) / "evidence"
            with (
                patch.object(perf_nix, "identity", side_effect=["before", "before", "after"]),
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

    def test_head_drift_and_load_refuse_before_running_nix(self):
        for head_values, load, reason in [
            (["old", "new"], 0, "HEAD changed"),
            (["same", "same"], 100, "host load"),
        ]:
            with (
                tempfile.TemporaryDirectory() as root,
                patch.object(perf_nix, "identity", return_value="same"),
                patch.object(perf_nix, "source_head", side_effect=head_values),
                patch.object(subprocess, "check_output", return_value="nix test"),
                patch.object(perf_nix.os, "getloadavg", return_value=(load, 0, 0)),
                patch.object(perf_nix.os, "cpu_count", return_value=2),
                patch.object(subprocess, "run") as run,
            ):
                directory = Path(root) / "evidence"
                with self.assertRaisesRegex(ValueError, reason):
                    perf_nix.record(Path(root), "host", directory, 1, 1, max_load_per_cpu=1)
                run.assert_not_called()
                manifest = json.loads((directory / "manifest.json").read_text())
                self.assertFalse(manifest["complete"])
                self.assertIn("refused_reason", manifest)

    def test_plan_only_does_not_warm_with_explicit_evaluation(self):
        with (
                tempfile.TemporaryDirectory() as root,
            patch.object(perf_nix, "identity", return_value="same"),
            patch.object(subprocess, "check_output", return_value="same"),
            patch.object(subprocess, "run", return_value=subprocess.CompletedProcess("nix", 0)) as run,
        ):
            perf_nix.record(Path(root), "host", Path(root) / "evidence", 1, 1, operations=["plan"])
            self.assertEqual(run.call_count, 1)
            self.assertIn("--dry-run", run.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
