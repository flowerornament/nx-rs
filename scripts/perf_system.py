#!/usr/bin/env python3
"""Reuse correctness scenarios and retain comparable release-binary measurements.

Dependency-free like the existing release and gate helpers.
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import os
import platform
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def covered_ns(intervals: list[tuple[int, int]]) -> int:
    end = 0
    covered = 0
    for start, stop in sorted(intervals):
        if stop < start:
            raise ValueError("negative command interval")
        covered += max(0, stop - max(start, end))
        end = max(end, stop)
    return covered


def summarize(path: Path) -> dict:
    events = [json.loads(line) for line in path.read_text().splitlines()]
    if (
        not events
        or events[0]["event"] != "run-start"
        or events[-1]["event"] != "run-end"
    ):
        raise ValueError(f"incomplete trace: {path}")
    if any(event["version"] != 1 for event in events):
        raise ValueError(f"unsupported trace version: {path}")
    if any(
        left["offset_ns"] > right["offset_ns"]
        for left, right in itertools.pairwise(events)
    ):
        raise ValueError(f"out-of-order trace: {path}")
    if events[-1].get("data", {}).get("complete") is False:
        raise ValueError(f"trace write failure: {path}")
    if events[-1]["code"] is None:
        raise ValueError(f"unfinished run: {path}")
    opened = {}
    intervals = []
    commands = {}
    phases = {}
    activities = []
    for event in events:
        kind = event["event"]
        identifier = event["id"]
        if kind == "nix-activity":
            if event["parent"] not in opened:
                raise ValueError(f"orphan Nix activity: {path}")
            activities.append(event)
            continue
        if kind.endswith("-start"):
            if identifier in opened or (
                event["parent"] is not None and event["parent"] not in opened
            ):
                raise ValueError(f"invalid span parent or duplicate start: {path}")
            opened[identifier] = event
            continue
        start = opened.pop(identifier)
        if (
            start["event"].replace("-start", "-end") != kind
            or start["parent"] != event["parent"]
        ):
            raise ValueError(f"mismatched span: {path}")
        duration = event["offset_ns"] - start["offset_ns"]
        if kind == "command-end":
            intervals.append((start["offset_ns"], event["offset_ns"]))
            row = commands.setdefault(
                event["label"],
                {"count": 0, "duration_ns": 0, "failed": 0, "unknown": 0},
            )
            row["count"] += 1
            row["duration_ns"] += duration
            row["failed"] += event["code"] is not None and event["code"] != 0
            row["unknown"] += event["code"] is None
        elif kind == "phase-end":
            phases[event["label"]] = phases.get(event["label"], 0) + duration
    if opened:
        raise ValueError(f"unclosed spans: {path}")
    total = events[-1]["offset_ns"] - events[0]["offset_ns"]
    covered = covered_ns(intervals)
    # Offsets are measured from the same clock; clip the tiny pre-run-start prefix.
    outside = max(0, total - covered)
    return {
        "total_ns": total,
        "command_covered_ns": covered,
        "outside_commands_ns": outside,
        "exit_code": events[-1]["code"],
        "commands": commands,
        "phases_ns": phases,
        "nix_activities": activities,
    }


def report(directory: Path) -> dict:
    cases = {}
    for path in sorted(directory.glob("repeat-*/traced/*.trace.jsonl")):
        case = path.name.removesuffix(".trace.jsonl")
        run = summarize(path)
        measurement = json.loads(path.with_name(f"{case}.measurement.json").read_text())
        control = json.loads(
            (path.parent.parent / "untraced" / f"{case}.measurement.json").read_text()
        )
        run.update(
            process_wall_ns=measurement["process_wall_ns"],
            control_wall_ns=control["process_wall_ns"],
        )
        cases.setdefault(case, []).append(run)
    if not cases:
        raise ValueError(f"no retained traces in {directory}")
    output = {}
    for case, runs in cases.items():
        output[case] = {
            "runs": runs,
            "samples": len(runs),
            "median_ms": statistics.median(run["process_wall_ns"] for run in runs)
            / 1e6,
            "min_ms": min(run["process_wall_ns"] for run in runs) / 1e6,
            "max_ms": max(run["process_wall_ns"] for run in runs) / 1e6,
            "median_trace_delta_ms": statistics.median(
                run["process_wall_ns"] - run["control_wall_ns"] for run in runs
            )
            / 1e6,
            "median_outside_commands_ms": statistics.median(
                run["outside_commands_ns"] for run in runs
            )
            / 1e6,
        }
    (directory / "summary.json").write_text(json.dumps(output, indent=2) + "\n")
    print(
        "scenario                                  median      range       outside commands   trace delta"
    )
    for case, row in output.items():
        print(
            f"{case:40} {row['median_ms']:8.1f}ms {row['min_ms']:7.1f}–{row['max_ms']:.1f}ms {row['median_outside_commands_ms']:8.1f}ms {row['median_trace_delta_ms']:8.1f}ms"
        )
    return output


def digest(paths: list[Path]) -> str:
    value = hashlib.sha256()
    for path in sorted(paths):
        value.update(str(path.relative_to(ROOT)).encode() + b"\0" + path.read_bytes())
    return value.hexdigest()


def record(directory: Path, repeat: int) -> None:
    directory.mkdir(parents=True, exist_ok=False)
    source = subprocess.check_output(
        ["jj", "log", "-r", "@", "--no-graph", "-T", "commit_id"], cwd=ROOT, text=True
    ).strip()
    subprocess.run(["just", "build"], cwd=ROOT, check=True)
    after = subprocess.check_output(
        ["jj", "log", "-r", "@", "--no-graph", "-T", "commit_id"], cwd=ROOT, text=True
    ).strip()
    if source != after:
        raise ValueError("source changed during the benchmark build")
    binary = ROOT / "target/release/nx"
    suite = [ROOT / "tests/system_upgrade.rs", ROOT / "scripts/perf_system.py"]
    suite += list((ROOT / "tests/support").glob("*.rs"))
    suite += [p for p in (ROOT / "tests/fixtures/system").rglob("*") if p.is_file()]
    manifest = {
        "version": 1,
        "harness": "system-upgrade-trace-v2",
        "mode": "structured-stub",
        "source_commit": source,
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "suite_sha256": digest(suite),
        "repeat": repeat,
        "platform": platform.platform(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
        "load_start": os.getloadavg(),
        "started_at_ns": time.time_ns(),
        "complete": False,
    }
    manifest_path = directory / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    for index in range(repeat):
        # Alternate pair order to reduce systematic warming/order bias.
        modes = ("traced", "untraced") if index % 2 == 0 else ("untraced", "traced")
        for mode in modes:
            destination = directory / f"repeat-{index:02}" / mode
            env = os.environ.copy()
            env.update(
                NX_TEST_BINARY=str(binary),
                NX_PERF_DIR=str(destination),
                NX_PERF_TRACE="1" if mode == "traced" else "0",
            )
            env.pop("NX_TRACE_PATH", None)
            with (directory / f"repeat-{index:02}-{mode}.log").open("w") as log:
                subprocess.run(
                    [
                        "cargo",
                        "test",
                        "--test",
                        "system_upgrade",
                        "system_upgrade_flows",
                        "--",
                        "--exact",
                    ],
                    cwd=ROOT,
                    env=env,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    check=True,
                )
    if (
        hashlib.sha256(binary.read_bytes()).hexdigest() != manifest["binary_sha256"]
        or digest(suite) != manifest["suite_sha256"]
    ):
        raise ValueError("binary or suite changed during measurement")
    report(directory)
    manifest.update(
        complete=True, load_end=os.getloadavg(), finished_at_ns=time.time_ns()
    )
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Artifacts: {directory}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--trace", type=Path, help="summarize one real or controlled Nx trace"
    )
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--report", type=Path, help="report a retained run without executing commands"
    )
    args = parser.parse_args()
    if args.trace:
        print(json.dumps(summarize(args.trace), indent=2))
        return
    if args.report:
        report(args.report)
        return
    if args.repeat < 1:
        parser.error("--repeat must be positive")
    directory = args.output or ROOT / ".nx/perf" / time.strftime("%Y%m%dT%H%M%S")
    record(directory.resolve(), args.repeat)


if __name__ == "__main__":
    main()
