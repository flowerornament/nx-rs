"""Measure pinned real Nix evaluation and planning without activation or lock updates."""

import argparse
import hashlib
import json
import math
import os
import statistics
import subprocess
import time
from pathlib import Path


def identity(repo):
    # Include dirty tracked content: HEAD alone does not describe a local flake.
    paths = (
        subprocess.check_output(["git", "ls-files", "-z"], cwd=repo)
        .decode()
        .split("\0")
    )
    digest = hashlib.sha256()
    for name in sorted(filter(None, paths)):
        path = repo / name
        digest.update(name.encode() + b"\0")
        digest.update(path.read_bytes() if path.is_file() else b"<missing>")
    return digest.hexdigest()


def source_head(repo):
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()


def activity_summary(path):
    counts = {}
    downloads = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            row = json.loads(line.removeprefix("@nix "))
        except ValueError:
            continue
        if row.get("action") != "start":
            continue
        label = row.get("name") or f"type:{row.get('type', 'unknown')}"
        counts[label] = counts.get(label, 0) + 1
        # Observed Nix download activity, not a count of all HTTP requests.
        if row.get("type") == 101:
            downloads.append(row.get("fields", []))
    return {"activity_starts": counts, "download_targets": downloads}


def record(
    repo, host, directory, repeat, timeout, build=False, compare_eval_cache=False,
    operations=None, max_load_per_cpu=None
):
    directory.mkdir(parents=True, exist_ok=False)
    source = identity(repo)
    head = source_head(repo)
    attr = f"{repo}#darwinConfigurations.{host}.system"
    commands = {
        "evaluate": ["nix", "eval", "--raw", f"{attr}.drvPath"],
        "plan": ["nix", "build", "--dry-run", attr],
        "check": ["nix", "flake", "check", "--no-build", str(repo)],
    }
    if compare_eval_cache:
        commands["plan-without-eval-cache"] = [
            "nix",
            "build",
            "--dry-run",
            "--option",
            "eval-cache",
            "false",
            attr,
        ]
    if build:
        commands["build"] = ["nix", "build", "--no-link", "--print-out-paths", attr]
    if operations:
        unknown = set(operations) - commands.keys()
        if unknown:
            raise ValueError(f"unknown operations: {sorted(unknown)}")
        commands = {name: commands[name] for name in dict.fromkeys(operations)}
    manifest = {
        "version": 1,
        "mode": "real-pinned-config",
        "repo": str(repo),
        "host": host,
        "tracked_content_sha256": source,
        "git_head": head,
        "logical_cpus": os.cpu_count(),
        "max_load_per_cpu": max_load_per_cpu,
        "operations": list(commands),
        "nix_version": subprocess.check_output(["nix", "--version"], text=True).strip(),
        "repeat": repeat,
        "timeout_seconds": timeout,
        "started_at_ns": time.time_ns(),
        "complete": False,
        "samples": [],
        "conditions": "Existing caches retained; first sample is not a cold-cache claim.",
    }
    destination = directory / "manifest.json"

    def save():
        destination.write_text(json.dumps(manifest, indent=2) + "\n")

    def qualify():
        reason = None
        load = os.getloadavg()
        manifest["qualification"] = {"load": load, "logical_cpus": os.cpu_count(), "at_ns": time.time_ns()}
        if identity(repo) != source or source_head(repo) != head:
            reason = "tracked configuration or Git HEAD changed during measurement"
        elif max_load_per_cpu is not None and load[0] / (os.cpu_count() or 1) > max_load_per_cpu:
            reason = "host load exceeds requested load-per-CPU limit"
        if reason:
            manifest["refused_reason"] = reason
            save()
            raise ValueError(reason)

    save()
    for index in range(repeat):
        # Rotate order: the first command can warm shared evaluation caches.
        names = list(commands)
        names = names[index % len(names) :] + names[: index % len(names)]
        for name in names:
            qualify()
            args = commands[name] + [
                "--no-update-lock-file",
                "--log-format",
                "internal-json",
            ]
            stem = f"{index:02}-{name}"
            sample = {
                "operation": name,
                "argv": args,
                "load_start": os.getloadavg(),
                "log_stem": stem,
            }
            start = time.monotonic_ns()
            print(f"{stem}: starting", flush=True)
            with (
                (directory / f"{stem}.stdout").open("wb") as out,
                (directory / f"{stem}.stderr.jsonl").open("wb") as err,
            ):
                try:
                    result = subprocess.run(
                        args, stdout=out, stderr=err, timeout=timeout, check=False
                    )
                    sample.update(code=result.returncode, timed_out=False)
                except subprocess.TimeoutExpired:
                    sample.update(code=None, timed_out=True)
            sample.update(wall_ns=time.monotonic_ns() - start, load_end=os.getloadavg())
            sample["nix_activity"] = activity_summary(
                directory / f"{stem}.stderr.jsonl"
            )
            manifest["samples"].append(sample)
            save()
            print(
                f"{stem}: {sample['wall_ns'] / 1e9:.3f}s code={sample['code']}",
                flush=True,
            )
            qualify()
    manifest.update(complete=True, finished_at_ns=time.time_ns())
    save()
    report(directory)


def report(directory):
    manifest = json.loads((directory / "manifest.json").read_text())
    print(f"Recording complete: {manifest['complete']}")
    for name in dict.fromkeys(s["operation"] for s in manifest["samples"]):
        samples = [s for s in manifest["samples"] if s["operation"] == name]
        success = [s["wall_ns"] / 1e9 for s in samples if s["code"] == 0]
        if success:
            print(
                f"{name}: median {statistics.median(success):.3f}s, range {min(success):.3f}–{max(success):.3f}s; {len(success)}/{len(samples)} succeeded"
            )
        else:
            print(f"{name}: no successful samples; inspect retained stderr")
    for index, sample in enumerate(manifest["samples"]):
        stem = sample.get("log_stem", f"{index // 3:02}-{sample['operation']}")
        activity = sample.get("nix_activity") or activity_summary(
            directory / f"{stem}.stderr.jsonl"
        )
        print(
            f"{sample['operation']}: {len(activity['download_targets'])} download activities, "
            f"{activity['activity_starts'].get('CopySourcePath', 0)} local source-copy activities"
        )
    print(f"Artifacts: {directory}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.home() / ".nix-config")
    parser.add_argument("--host")
    parser.add_argument("--report", type=Path)
    parser.add_argument(
        "--build",
        action="store_true",
        help="Also realize the system without linking or activation",
    )
    parser.add_argument(
        "--compare-eval-cache",
        action="store_true",
        help="Compare planning with the per-command evaluation cache disabled; preserve all existing caches",
    )
    parser.add_argument("--operations", nargs="+", help="Run only named operations in this order, e.g. plan check; avoids an implicit evaluate warmup")
    parser.add_argument("--max-load-per-cpu", type=float, help="Refuse samples above this 1-minute load / logical CPU ratio")
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.report:
        report(args.report.resolve())
        return
    if not args.host:
        parser.error("--host is required when recording")
    if args.repeat < 1 or args.timeout < 1:
        parser.error("repeat and timeout must be positive")
    if args.max_load_per_cpu is not None and (not math.isfinite(args.max_load_per_cpu) or args.max_load_per_cpu <= 0):
        parser.error("max-load-per-cpu must be positive")
    output = args.output or Path(".nx/perf") / time.strftime("real-%Y%m%dT%H%M%S")
    try:
        record(
            args.repo.resolve(), args.host, output.resolve(), args.repeat, args.timeout,
            args.build, args.compare_eval_cache, args.operations, args.max_load_per_cpu,
        )
    except ValueError as error:
        parser.exit(1, f"Measurement refused: {error}\nArtifacts: {output.resolve()}\n")



if __name__ == "__main__":
    main()
