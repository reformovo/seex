"""Benchmark Python/PyO3 ``run.log(...)`` admission throughput."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

DEFAULT_REPORTS = 100_000
DEFAULT_QUEUE_CAPACITY = 1_048_576
MODES = ("explicit_single", "implicit_single", "mapping_8")
SAMPLES = 10
MINIMUM_SAMPLE_SECONDS = 0.025


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    if args.child:
        return child_main(args)
    return parent_main(args)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Measure one-thread Python explicit-step run.log(...) admission "
            "throughput. The timed section excludes client setup and shutdown."
        )
    )
    parser.add_argument(
        "--reports",
        type=positive_int,
        default=DEFAULT_REPORTS,
        help=f"number of explicit-step reports to log (default: {DEFAULT_REPORTS})",
    )
    parser.add_argument("--mode", choices=MODES, default="explicit_single")
    parser.add_argument(
        "--queue-capacity",
        type=positive_int,
        default=DEFAULT_QUEUE_CAPACITY,
        help=(f"metric queue capacity to use (default: {DEFAULT_QUEUE_CAPACITY})"),
    )
    parser.add_argument(
        "--path",
        type=Path,
        default=None,
        help="project directory to use instead of a temporary directory",
    )
    parser.add_argument(
        "--keep-data",
        action="store_true",
        help="keep the benchmark project directory after completion",
    )
    parser.add_argument("--child", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--samples", type=positive_int, default=SAMPLES, help=argparse.SUPPRESS)
    return parser


def parent_main(args: argparse.Namespace) -> int:
    temp_dir: str | None = None
    if args.path is None:
        temp_dir = tempfile.mkdtemp(prefix="seex-bench-")
        project_path = Path(temp_dir)
    else:
        project_path = args.path
        project_path.mkdir(parents=True, exist_ok=True)

    try:
        calibration = run_child(
            args,
            project_path / "calibration",
            reports=args.reports,
        )
        reports = int(calibration["batch_iterations"]) * 3
        if reports > args.queue_capacity:
            raise RuntimeError("calibrated admission sample exceeds the metric queue capacity")
        samples = [
            float(run_child(args, project_path / f"sample-{index}", reports=reports)["samples"][0])
            for index in range(SAMPLES)
        ]
        print_result(
            {
                "benchmark": "run_log_admission",
                "mode": args.mode,
                "requested_reports": args.reports,
                "reports": reports,
                "samples": samples,
                "calls_per_second": statistics.median(samples),
                "queue_capacity": args.queue_capacity,
            }
        )
        return 0
    finally:
        if temp_dir is not None and not args.keep_data:
            shutil.rmtree(temp_dir)


def child_main(args: argparse.Namespace) -> int:
    if args.path is None:
        raise argparse.ArgumentTypeError("--path is required in child mode")
    result, _client = run_benchmark(
        project_path=args.path,
        reports=args.reports,
        queue_capacity=args.queue_capacity,
        mode=args.mode,
        sample_count=args.samples,
    )
    print_result(result)
    # The parent owns cleanup; skip implicit client drain in this timed benchmark.
    os._exit(0)


def run_benchmark(
    *,
    project_path: Path,
    reports: int,
    queue_capacity: int,
    mode: str,
    sample_count: int = SAMPLES,
) -> tuple[dict[str, Any], Any]:
    import seex

    client = seex.init(project_path, metric_queue_capacity=queue_capacity)
    project = client.create_project("benchmark", project_id="bench-project")
    run = client.create_run(project.project_id, "throughput", run_id="bench-run")

    calibrated_reports, samples = calibrated_samples(
        lambda batch_reports: log_reports(run, mode, batch_reports), reports, sample_count
    )

    return (
        {
            "benchmark": "run_log_admission",
            "mode": mode,
            "requested_reports": reports,
            "reports": calibrated_reports,
            "samples": samples,
            "calls_per_second": statistics.median(samples),
            "queue_capacity": queue_capacity,
        },
        client,
    )


def run_child(args: argparse.Namespace, project_path: Path, *, reports: int) -> dict[str, Any]:
    command = [
        sys.executable,
        __file__,
        "--child",
        "--reports",
        str(reports),
        "--queue-capacity",
        str(args.queue_capacity),
        "--mode",
        args.mode,
        "--samples",
        "1",
        "--path",
        str(project_path),
    ]
    completed = subprocess.run(command, check=False, text=True, capture_output=True)
    if completed.returncode != 0:
        if completed.stdout:
            print(completed.stdout, end="", file=sys.stderr)
        if completed.stderr:
            print(completed.stderr, end="", file=sys.stderr)
        raise subprocess.CalledProcessError(completed.returncode, command)
    records = [
        json.loads(line.removeprefix("SEEX_BENCH "))
        for line in completed.stdout.splitlines()
        if line.startswith("SEEX_BENCH ")
    ]
    if len(records) != 1:
        raise RuntimeError("admission sample child did not emit exactly one metric")
    record = records[0]
    if record.get("batch_iterations") != reports and project_path.name != "calibration":
        raise RuntimeError("admission sample missed the calibrated batch size")
    return record


def print_result(result: dict[str, Any]) -> None:
    print(json.dumps(result, indent=2, sort_keys=True), flush=True)
    mode = str(result["mode"])
    print(
        "SEEX_BENCH "
        + json.dumps(
            {
                "schema_version": 3,
                "record_type": "metric",
                "domain": "reporting",
                "metric": f"python.{mode}.admission",
                "unit": "points/s",
                "direction": "higher",
                "batch_iterations": int(result["reports"]),
                "samples": result["samples"],
            },
            separators=(",", ":"),
        ),
        flush=True,
    )


def calibrated_samples(
    measure: Callable[[int], float], initial_reports: int, sample_count: int = SAMPLES
) -> tuple[int, list[float]]:
    """Collect throughput samples after reaching the 25 ms timing floor."""
    reports = initial_reports
    elapsed = float(measure(reports))
    while elapsed < MINIMUM_SAMPLE_SECONDS:
        reports *= 2
        elapsed = float(measure(reports))
    samples = [reports / elapsed]
    for _ in range(sample_count - 1):
        elapsed = float(measure(reports))
        if elapsed < MINIMUM_SAMPLE_SECONDS:
            raise RuntimeError("calibrated reporting sample completed in less than 25 ms")
        samples.append(reports / elapsed)
    return reports, samples


def log_reports(run: Any, mode: str, reports: int) -> float:
    """Runs one compatibility workload and returns admission wall time."""
    if mode not in MODES:
        raise ValueError(f"unsupported reporting mode: {mode}")
    if mode == "mapping_8" and reports % 8 != 0:
        raise ValueError("mapping_8 reports must be divisible by eight")
    started = time.perf_counter()
    if mode == "mapping_8":
        for step in range(reports // 8):
            for metric in range(8):
                run.log(f"metric-{metric}", step, float(step))
    elif mode == "implicit_single":
        for selected_step, value in enumerate(float(index) for index in range(reports)):
            run.log("train/loss", selected_step, value)
    else:
        for step in range(reports):
            run.log("train/loss", step, float(step))
    return time.perf_counter() - started


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return parsed


if __name__ == "__main__":
    raise SystemExit(main())
