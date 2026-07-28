"""Benchmark explicit-step ``run.log(...)`` admission throughput."""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

DEFAULT_REPORTS = 100_000
DEFAULT_QUEUE_CAPACITY = 1_048_576


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
    return parser


def parent_main(args: argparse.Namespace) -> int:
    temp_dir: str | None = None
    if args.path is None:
        temp_dir = tempfile.mkdtemp(prefix="pulseon-bench-")
        project_path = Path(temp_dir)
    else:
        project_path = args.path
        project_path.mkdir(parents=True, exist_ok=True)

    try:
        command = [
            sys.executable,
            __file__,
            "--child",
            "--reports",
            str(args.reports),
            "--queue-capacity",
            str(args.queue_capacity),
            "--path",
            str(project_path),
        ]
        completed = subprocess.run(
            command,
            check=False,
            text=True,
            capture_output=True,
        )
        if completed.stdout:
            print(completed.stdout, end="")
        if completed.stderr:
            print(completed.stderr, end="", file=sys.stderr)
        return completed.returncode
    finally:
        if temp_dir is not None and not args.keep_data:
            shutil.rmtree(temp_dir)


def child_main(args: argparse.Namespace) -> int:
    if args.path is None:
        raise argparse.ArgumentTypeError("--path is required in child mode")
    result = run_benchmark(
        project_path=args.path,
        reports=args.reports,
        queue_capacity=args.queue_capacity,
    )
    print_result(result)
    # The parent owns cleanup; skip implicit client drain in this timed benchmark.
    os._exit(0)


def run_benchmark(
    *,
    project_path: Path,
    reports: int,
    queue_capacity: int,
) -> dict[str, Any]:
    import pulseon

    client = pulseon.init(project_path, metric_queue_capacity=queue_capacity)
    project = client.create_project("benchmark", project_id="bench-project")
    run = client.create_run(project.project_id, "throughput", run_id="bench-run")

    started = time.perf_counter()
    for step in range(reports):
        run.log("train/loss", step, float(step))
    elapsed = time.perf_counter() - started

    return {
        "benchmark": "explicit_step_run_log_admission",
        "reports": reports,
        "elapsed_seconds": elapsed,
        "calls_per_second": reports / elapsed,
        "queue_capacity": queue_capacity,
        "diagnostics_after_log": diagnostics_to_dict(client.diagnostics()),
        "environment": environment(getattr(pulseon, "__version__", "unknown")),
        "project_path": str(project_path),
    }


def print_result(result: dict[str, Any]) -> None:
    print(json.dumps(result, indent=2, sort_keys=True), flush=True)


def diagnostics_to_dict(diagnostics: Any) -> dict[str, Any]:
    return {
        "pending_reports": diagnostics.pending_reports,
        "queue_full_errors": diagnostics.queue_full_errors,
        "persisted_reports": diagnostics.persisted_reports,
        "writer_state": diagnostics.writer_state,
        "last_write_error": diagnostics.last_write_error,
        "last_flush_run_id": diagnostics.last_flush_run_id,
        "last_flush_status": diagnostics.last_flush_status,
        "last_flush_error": diagnostics.last_flush_error,
    }


def environment(pulseon_version: str) -> dict[str, str]:
    return {
        "machine": platform.machine(),
        "platform": platform.platform(),
        "processor": platform.processor(),
        "python": sys.version.replace("\n", " "),
        "python_implementation": platform.python_implementation(),
        "pulseon_version": pulseon_version,
        "working_directory": os.getcwd(),
    }


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return parsed


if __name__ == "__main__":
    raise SystemExit(main())
