"""Benchmark explicit-step metric persistence and finalization timing."""

from __future__ import annotations

import argparse
import dataclasses
import json
import shutil
import statistics
import sys
import tempfile
import time
import tomllib
from collections.abc import Sequence
from pathlib import Path
from typing import Any

DEFAULT_REPORTS = 1_000
DEFAULT_REPEATS = 30
DEFAULT_QUEUE_CAPACITY = 65_536
DEFAULT_DRAIN_TIMEOUT_SECONDS = 120.0
DEFAULT_DRAIN_POLL_SECONDS = 0.01

_StorageTarget = tuple[Path, dict[str, str], Path | None]


@dataclasses.dataclass(frozen=True)
class ObjectStorageConfig:
    path: Path
    data_path: str


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    project_root, temp_dir = benchmark_root(args.path)

    try:
        print("SEEX_RSS_PHASE warm", flush=True)
        result = run_benchmark(
            project_root=project_root,
            reports=args.reports,
            repeats=args.repeats,
            queue_capacity=args.queue_capacity,
            drain_timeout=args.drain_timeout,
            drain_poll_seconds=args.drain_poll_seconds,
            metric_key=args.metric_key,
            object_storage_configs=args.object_storage_config,
        )
        emit_performance_records(result)
        print("SEEX_RSS_PHASE cycles_done", flush=True)
        print("SEEX_RSS_PHASE final", flush=True)
    finally:
        if temp_dir is not None and not args.keep_data:
            shutil.rmtree(temp_dir)
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=("Measure background writer drain/persistence and finalization timing across repeated Seex runs.")
    )
    parser.add_argument(
        "--reports",
        type=positive_int,
        default=DEFAULT_REPORTS,
        help=f"number of same-key reports per repeat (default: {DEFAULT_REPORTS})",
    )
    parser.add_argument(
        "--repeats",
        type=positive_int,
        default=DEFAULT_REPEATS,
        help=f"number of independent benchmark repeats (default: {DEFAULT_REPEATS})",
    )
    parser.add_argument(
        "--queue-capacity",
        type=positive_int,
        default=DEFAULT_QUEUE_CAPACITY,
        help=(f"metric queue capacity to use (default: {DEFAULT_QUEUE_CAPACITY})"),
    )
    parser.add_argument(
        "--metric-key",
        default="train/loss",
        help="metric key to log for every report (default: train/loss)",
    )
    parser.add_argument(
        "--drain-timeout",
        type=non_negative_float,
        default=DEFAULT_DRAIN_TIMEOUT_SECONDS,
        help=(f"background drain timeout in seconds (default: {DEFAULT_DRAIN_TIMEOUT_SECONDS:g})"),
    )
    parser.add_argument(
        "--drain-poll-seconds",
        type=positive_float,
        default=DEFAULT_DRAIN_POLL_SECONDS,
        help=(f"seconds to sleep between diagnostics polls while draining (default: {DEFAULT_DRAIN_POLL_SECONDS})"),
    )
    parser.add_argument(
        "--object-storage-config",
        action="append",
        default=[],
        type=object_storage_config,
        metavar="CONFIG_TOML",
        help=(
            "project config.toml for an S3-compatible data path; repeat to "
            "benchmark multiple S3/OSS targets alongside local storage"
        ),
    )
    parser.add_argument(
        "--path",
        type=Path,
        default=None,
        help="benchmark root directory to use instead of a temporary directory",
    )
    parser.add_argument(
        "--keep-data",
        action="store_true",
        help="keep the benchmark root directory after completion",
    )
    return parser


def benchmark_root(path: Path | None) -> tuple[Path, str | None]:
    if path is not None:
        path.mkdir(parents=True, exist_ok=True)
        return path, None

    temp_dir = tempfile.mkdtemp(prefix="seex-persistence-bench-")
    return Path(temp_dir), temp_dir


def run_benchmark(
    *,
    project_root: Path,
    reports: int,
    repeats: int,
    queue_capacity: int,
    drain_timeout: float | None,
    drain_poll_seconds: float,
    metric_key: str,
    object_storage_configs: Sequence[ObjectStorageConfig] = (),
) -> dict[str, Any]:
    import seex

    targets: list[_StorageTarget] = [
        (project_root, {"kind": "local"}, None),
    ]
    targets.extend(
        (
            project_root / f"object-storage-{config_index + 1}",
            {
                "kind": "s3_compatible",
                "data_path": config.data_path,
                "config_path": str(config.path),
            },
            config.path,
        )
        for config_index, config in enumerate(object_storage_configs)
    )
    repeat_results_by_target: list[list[dict[str, Any]]] = [[] for _ in targets]
    for repeat_index in range(repeats):
        # Rotate the first target so process warmup does not always favor OSS.
        for target_offset in range(len(targets)):
            target_index = (repeat_index + target_offset) % len(targets)
            target_root, _, config_path = targets[target_index]
            repeat_results_by_target[target_index].append(
                run_once(
                    seex_module=seex,
                    project_path=target_root / f"repeat-{repeat_index + 1}",
                    config_path=config_path,
                    repeat_index=repeat_index,
                    reports=reports,
                    queue_capacity=queue_capacity,
                    drain_timeout=drain_timeout,
                    drain_poll_seconds=drain_poll_seconds,
                    metric_key=metric_key,
                )
            )

    target_results = [
        target_result(
            project_root=target_root,
            storage=storage,
            reports=reports,
            queue_capacity=queue_capacity,
            metric_key=metric_key,
            repeat_results=repeat_results,
        )
        for (target_root, storage, _), repeat_results in zip(targets, repeat_results_by_target, strict=True)
    ]
    result = target_results[0]
    result["object_storage_results"] = target_results[1:]
    result["environment"] = {
        "python": sys.version.replace("\n", " "),
        "seex_version": getattr(seex, "__version__", "unknown"),
    }
    return result


def target_result(
    *,
    project_root: Path,
    storage: dict[str, str],
    reports: int,
    queue_capacity: int,
    metric_key: str,
    repeat_results: list[dict[str, Any]],
) -> dict[str, Any]:
    return {
        "benchmark": "explicit_step_metric_persistence",
        "reports_per_repeat": reports,
        "repeats": len(repeat_results),
        "queue_capacity": queue_capacity,
        "metric_key": metric_key,
        "storage": storage,
        "repeat_results": repeat_results,
        "summary": summarize(repeat_results),
        "project_root": str(project_root),
    }


def run_once(
    *,
    seex_module: Any,
    project_path: Path,
    config_path: Path | None,
    repeat_index: int,
    reports: int,
    queue_capacity: int,
    drain_timeout: float | None,
    drain_poll_seconds: float,
    metric_key: str,
) -> dict[str, Any]:
    staged_config = stage_config(project_path, config_path)
    setup_started = time.perf_counter()
    try:
        settings = seex_module.Settings(metric_queue_capacity=queue_capacity)
        run = seex_module.init(
            project=f"bench-project-{repeat_index + 1}",
            dir=project_path,
            id=f"bench-run-{repeat_index + 1}",
            name="persistence",
            settings=settings,
        )
    finally:
        if staged_config is not None:
            staged_config.unlink(missing_ok=True)
    setup_seconds = time.perf_counter() - setup_started

    admission_started = time.perf_counter()
    for step in range(reports):
        run.log({metric_key: float(step)}, step=step)
    admission_seconds = time.perf_counter() - admission_started
    diagnostics_after_admission = diagnostics_to_dict(run.diagnostics())

    drain_seconds, diagnostics_after_drain = wait_for_drain(
        run,
        timeout_seconds=drain_timeout,
        poll_seconds=drain_poll_seconds,
    )

    finalization_started = time.perf_counter()
    run.finish()
    finalization_seconds = time.perf_counter() - finalization_started
    diagnostics_after_finalization = diagnostics_to_dict(run.diagnostics())
    shutdown_seconds = 0.0
    diagnostics_after_shutdown = diagnostics_after_finalization

    return {
        "repeat": repeat_index + 1,
        "project_path": str(project_path),
        "setup_seconds": setup_seconds,
        "admission_seconds": admission_seconds,
        "admission_calls_per_second": reports / admission_seconds,
        "drain_seconds": drain_seconds,
        "finalization_seconds": finalization_seconds,
        "shutdown_seconds": shutdown_seconds,
        "diagnostics_after_admission": diagnostics_after_admission,
        "diagnostics_after_drain": diagnostics_after_drain,
        "diagnostics_after_finalization": diagnostics_after_finalization,
        "diagnostics_after_shutdown": diagnostics_after_shutdown,
    }


def stage_config(project_path: Path, config_path: Path | None) -> Path | None:
    if config_path is None:
        return None
    destination = project_path / ".seex" / "config.toml"
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(config_path, destination)
    return destination


def wait_for_drain(
    client: Any,
    *,
    timeout_seconds: float | None,
    poll_seconds: float,
) -> tuple[float, dict[str, Any]]:
    started = time.perf_counter()
    deadline = None if timeout_seconds is None else started + timeout_seconds
    while True:
        diagnostics = client.diagnostics()
        if diagnostics.writer_state == "failed":
            raise RuntimeError(
                f"background metric writer failed while draining; last_write_error={diagnostics.last_write_error!r}"
            )
        if diagnostics.pending_reports == 0:
            return time.perf_counter() - started, diagnostics_to_dict(diagnostics)
        if deadline is not None and time.perf_counter() >= deadline:
            raise TimeoutError(
                f"timed out waiting for background metric writer drain; pending_reports={diagnostics.pending_reports}"
            )
        time.sleep(poll_seconds)


def summarize(repeat_results: list[dict[str, Any]]) -> dict[str, Any]:
    timing_fields = (
        "setup_seconds",
        "admission_seconds",
        "admission_calls_per_second",
        "drain_seconds",
        "finalization_seconds",
        "shutdown_seconds",
    )
    return {field: summarize_values([repeat[field] for repeat in repeat_results]) for field in timing_fields}


def summarize_values(values: list[float]) -> dict[str, float]:
    return {
        "min": min(values),
        "median": statistics.median(values),
        "max": max(values),
    }


def emit_performance_records(result: dict[str, Any]) -> None:
    """Emits raw schema-v3 durability samples for the local target."""
    repeats = result["repeat_results"]
    batch_iterations = 3
    if len(repeats) != 10 * batch_iterations:
        raise ValueError("persistence benchmark requires exactly thirty repeats")
    for label, field in (("drain_persistence", "drain_seconds"), ("finalization", "finalization_seconds")):
        raw = [
            sum(float(repeat[field]) for repeat in repeats[index : index + batch_iterations]) * 1_000_000_000
            for index in range(0, len(repeats), batch_iterations)
        ]
        record = {
            "schema_version": 3,
            "record_type": "metric",
            "domain": "reporting",
            "metric": f"python.{label}",
            "unit": "ns",
            "direction": "lower",
            "batch_iterations": batch_iterations,
            "samples": raw,
        }
        print("SEEX_BENCH " + json.dumps(record, separators=(",", ":")), flush=True)


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


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return parsed


def positive_float(value: str) -> float:
    parsed = float(value)
    if parsed <= 0.0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return parsed


def non_negative_float(value: str) -> float:
    parsed = float(value)
    if parsed < 0.0:
        raise argparse.ArgumentTypeError("must be non-negative")
    return parsed


def object_storage_config(value: str) -> ObjectStorageConfig:
    path = Path(value)
    try:
        config = tomllib.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise argparse.ArgumentTypeError(f"could not read object-storage config {value!r}: {error}") from error
    except tomllib.TOMLDecodeError as error:
        raise argparse.ArgumentTypeError(f"invalid TOML in object-storage config {value!r}: {error}") from error

    data_path = config.get("data_path")
    if not isinstance(data_path, str) or not data_path.startswith("s3://"):
        raise argparse.ArgumentTypeError("object-storage config data_path must be an s3:// URI")
    return ObjectStorageConfig(path=path, data_path=data_path)


if __name__ == "__main__":
    raise SystemExit(main())
