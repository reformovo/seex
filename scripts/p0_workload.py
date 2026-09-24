"""Generate and measure the deterministic P0 native workload."""

from __future__ import annotations

import argparse
import json
import os
import platform
import random
import resource
import shutil
import subprocess
import tempfile
import time
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
    return profile


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
    sequence_budget = profile["actual_sequences"]
    sequence_count = 0
    writer_peak_rss = 0
    first_run = "run-000000"
    first_metric = "metric-000000"
    for run_index in range(runs):
        run_id = f"run-{run_index:06d}"
        run = seex.init(project=project, dir=root, id=run_id)
        for metric_index in range(keys):
            if sequence_count >= sequence_budget:
                break
            metric_key = f"metric-{metric_index:06d}"
            if run_index == 0 and metric_index == 0:
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
    command = [
        "cargo",
        "run",
        "--quiet",
        "-p",
        "seex",
        "--example",
        "p0_query",
        "--",
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
            "schema_version": 1,
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
        hot_samples = [int(value) for value in probe["hot_samples_ns"]]
        if len(hot_samples) != 10 or any(value < MIN_SAMPLE_NS for value in hot_samples):
            raise ValueError("hot query samples did not meet the 25 ms calibration floor")
        baseline = {
            "schema_version": 1,
            "workload": "p0",
            "profile": profile_name,
            "seed": SEED,
            "distribution": {
                **profile,
                "generated_sequences": distribution["actual_sequences"],
                "patterns": ["short", "long", "sparse_step", "duplicate_step_lww", "out_of_order"],
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
                "normalized": "step_lttb_1024",
            },
            "latency_ns": {
                "cold_query": int(probe["cold_ns"]),
                "cold_process": process_ns,
                "hot_query": hot_samples,
            },
            "peak_rss_bytes": {
                "writer": distribution["writer_peak_rss"],
                "reader": (
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
    parser.add_argument("--output", type=Path, default=Path("docs/reference/p0-baseline.json"))
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
