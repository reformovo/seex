"""Generate and measure the deterministic P0 native workload."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import random
import resource
import shutil
import subprocess
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

SEED = 0x5EEC_0001
MAX_CHART_POINTS = 1024
MAX_SELECTED_RUNS = 16
MIN_SAMPLE_NS = 25_000_000
SCALE_DEFAULTS = {
    "runs": 1_000_000,
    "metric_keys": 1_000_000,
    "actual_sequences": 10_000_000,
}


def _profile(name: str, overrides: dict[str, int] | None = None) -> dict[str, int]:
    if name == "correctness":
        profile = {"runs": 4, "metric_keys": 3, "actual_sequences": 12}
    elif name == "scale":
        profile = dict(SCALE_DEFAULTS)
    else:
        raise ValueError(f"unsupported profile: {name}")
    if overrides:
        profile.update(overrides)
    if any(value <= 0 for value in profile.values()):
        raise ValueError("profile dimensions must be positive")
    if (
        not max(profile["runs"], profile["metric_keys"])
        <= profile["actual_sequences"]
        <= profile["runs"] * profile["metric_keys"]
    ):
        raise ValueError("actual_sequences must cover every Run and key without duplicate sequences")
    return profile


def _series_for_run(run_index: int, profile: dict[str, int]) -> Iterator[tuple[int, int]]:
    """Yield (sequence index, key index) for actual series in one Run."""
    runs = profile["runs"]
    keys = profile["metric_keys"]
    budget = profile["actual_sequences"]
    if runs >= keys:
        for sequence_index in range(run_index, budget, runs):
            yield sequence_index, (run_index + sequence_index // runs) % keys
    else:
        for batch in range((budget + keys - 1) // keys):
            first_key = (run_index - batch) % runs
            for metric_index in range(first_key, keys, runs):
                sequence_index = batch * keys + metric_index
                if sequence_index < budget:
                    yield sequence_index, metric_index


def _steps(rng: random.Random, count: int, *, sparse: bool = False) -> list[int]:
    """Return mixed short, long, sparse, duplicate, and out-of-order steps."""
    values = [index * 3 for index in range(count)] if sparse else list(range(count))
    if count >= 8:
        values.extend([2, count // 2, 2])
        rng.shuffle(values)
    return values


def _rss_bytes() -> int:
    value = int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
    return value if platform.system() == "Darwin" else value * 1024


def _write_workload(root: Path, profile: dict[str, int], *, seed: int) -> dict[str, Any]:
    import seex

    rng = random.Random(seed)
    project = "p0"
    runs = profile["runs"]
    keys = profile["metric_keys"]
    sequence_count = 0
    writer_peak_rss = 0
    first_run = "run-000000"
    first_metric = "metric-000000"
    for run_index in range(runs):
        run_id = f"run-{run_index:06d}"
        run = seex.init(project=project, dir=root, id=run_id)
        for sequence_index, metric_index in _series_for_run(run_index, profile):
            metric_key = f"metric-{metric_index:06d}"
            if sequence_index == 0:
                point_count = 12
            elif (run_index + metric_index) % 4 == 0:
                point_count = 256
            elif (run_index + metric_index) % 4 == 1:
                point_count = 8
            else:
                point_count = 32
            for step in _steps(rng, point_count, sparse=(run_index + metric_index) % 4 == 2):
                run.log({metric_key: rng.random()}, step=step)
            sequence_count += 1
        run.finish()
        writer_peak_rss = max(writer_peak_rss, _rss_bytes())
    return {
        "runs": runs,
        "metric_keys": keys,
        "actual_sequences": sequence_count,
        "seed": seed,
        "writer_peak_rss": writer_peak_rss,
        "first_run": first_run,
        "first_metric": first_metric,
    }


def _query_probe(root: Path, run_id: str, metric_key: str) -> tuple[dict[str, Any], int]:
    subprocess.run(["cargo", "build", "--release", "-p", "seex", "--example", "p0_query"], check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    command = [
        os.fspath(target / "release/examples/p0_query"),
        os.fspath(root),
        run_id,
        metric_key,
    ]
    started = time.perf_counter_ns()
    completed = subprocess.run(command, check=True, capture_output=True, text=True)
    process_ns = time.perf_counter_ns() - started
    result = json.loads(completed.stdout)
    if not isinstance(result, dict):
        raise TypeError("p0 query probe did not return an object")
    return result, process_ns


def _validate_hot_samples(probe: dict[str, Any]) -> list[dict[str, Any]]:
    if probe.get("schema_version") != 2 or probe.get("record_type") != "query_probe":
        raise ValueError("unsupported query probe schema")
    samples = probe["hot_samples"]
    if not isinstance(samples, list) or len(samples) != 10:
        raise ValueError("hot query requires ten calibrated samples")
    for sample in samples:
        iterations, elapsed, per_op = sample["iterations"], sample["elapsed_ns"], sample["ns_per_op"]
        if type(iterations) is not int or iterations <= 0 or type(elapsed) is not int or elapsed < MIN_SAMPLE_NS:
            raise ValueError("hot query samples did not meet the 25 ms calibration floor")
        if (
            not isinstance(per_op, (int, float))
            or not math.isfinite(per_op)
            or not math.isclose(per_op, elapsed / iterations, rel_tol=1e-9)
        ):
            raise ValueError("hot query ns_per_op does not match batch elapsed/iterations")
    return samples


def _file_stats(root: Path) -> dict[str, int]:
    files = [path for path in root.rglob("*") if path.is_file()]
    data_files = [path for path in files if path.suffix == ".parquet"]
    return {
        "file_count": len(data_files),
        "bytes": sum(path.stat().st_size for path in data_files),
        "catalog_bytes": sum(path.stat().st_size for path in files if "catalog" in path.name),
    }


def _tool_versions() -> dict[str, str]:
    versions: dict[str, str] = {"python": platform.python_version()}
    for tool in ("rustc", "cargo"):
        executable = shutil.which(tool)
        if executable is None:
            continue
        output = subprocess.run([executable, "--version"], check=True, capture_output=True, text=True)
        versions[tool] = output.stdout.strip()
    return versions


def run(
    profile_name: str,
    output: Path,
    root: Path | None = None,
    overrides: dict[str, int] | None = None,
) -> dict[str, Any]:
    profile = _profile(profile_name, overrides)
    if profile_name == "scale" and root is None:
        result = {
            "schema_version": 2,
            "workload": "p0",
            "profile": profile_name,
            "seed": SEED,
            "scale": profile,
            "status": "parameterized",
        }
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return result
    temporary = tempfile.TemporaryDirectory(prefix="seex-p0-") if root is None else None
    if root is None:
        assert temporary is not None
        workload_root = Path(temporary.name)
    else:
        workload_root = root
    workload_root.mkdir(parents=True, exist_ok=True)
    try:
        distribution = _write_workload(workload_root, profile, seed=SEED)
        probe, process_ns = _query_probe(workload_root, distribution["first_run"], distribution["first_metric"])
        hot_samples = _validate_hot_samples(probe)
        baseline = {
            "schema_version": 2,
            "workload": "p0",
            "profile": profile_name,
            "seed": SEED,
            "semantics": "append_only",
            "native_format_version": 5,
            "reader_build": "release",
            "tracked_patch_sha256": hashlib.sha256(
                subprocess.run(["git", "diff", "HEAD", "--binary"], check=True, capture_output=True).stdout
            ).hexdigest(),
            "distribution": {
                **profile,
                "generated_sequences": distribution["actual_sequences"],
                "patterns": ["short", "long", "sparse_step", "duplicate_step", "out_of_order"],
            },
            "git_revision": subprocess.run(
                ["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True
            ).stdout.strip(),
            "platform": {"system": platform.system(), "release": platform.release(), "machine": platform.machine()},
            "tool_versions": _tool_versions(),
            "storage": _file_stats(workload_root),
            "query_plan": {
                "axis": "Step",
                "max_points": MAX_CHART_POINTS,
                "max_selected_runs": MAX_SELECTED_RUNS,
                "api": "Reader.query_metric",
                "reduction": "screen_budget",
                "physical_plan": "not_captured",
            },
            "latency_ns": {
                "cold_query": int(probe["cold_query_ns"]),
                "probe_process": process_ns,
                "hot_query_batch": [
                    {
                        "iterations": int(sample["iterations"]),
                        "elapsed_ns": int(sample["elapsed_ns"]),
                        "ns_per_op": float(sample["ns_per_op"]),
                    }
                    for sample in hot_samples
                ],
            },
            "measurement_notes": {
                "cold_query": "first Reader request; OS page cache not cleared",
                "probe_process": "entire probe process including cold and hot queries; excludes build",
                "scale": "legacy sparse generator, not the P7 milestone distribution",
            },
            "peak_rss_bytes": {
                "workload_process": distribution["writer_peak_rss"],
                "all_children_including_build": (
                    int(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)
                    if platform.system() == "Darwin"
                    else int(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) * 1024
                ),
            },
        }
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(baseline, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        return baseline
    finally:
        if temporary is not None:
            temporary.cleanup()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("correctness", "scale"), default="correctness")
    parser.add_argument("--output", type=Path, default=Path(".tmp/p0-v5-baseline.json"))
    parser.add_argument("--root", type=Path)
    parser.add_argument("--runs", type=int)
    parser.add_argument("--metric-keys", type=int)
    parser.add_argument("--actual-sequences", type=int)
    arguments = parser.parse_args()
    overrides = {
        name: value
        for name, value in {
            "runs": arguments.runs,
            "metric_keys": arguments.metric_keys,
            "actual_sequences": arguments.actual_sequences,
        }.items()
        if value is not None
    }
    result = run(arguments.profile, arguments.output, arguments.root, overrides)
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
