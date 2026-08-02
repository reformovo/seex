"""Accept partition-pruned metric queries against opt-in MinIO storage."""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import time
import urllib.parse
import uuid
from typing import Any, Literal

_CatalogBackend = Literal["duckdb", "sqlite"]
_BACKENDS: tuple[_CatalogBackend, ...] = ("duckdb", "sqlite")
_REQUIRED_ENV = (
    "SEEX_MINIO_ENDPOINT",
    "SEEX_MINIO_BUCKET",
    "SEEX_MINIO_ACCESS_KEY_ID",
    "SEEX_MINIO_SECRET_ACCESS_KEY",
)


@dataclasses.dataclass(frozen=True)
class MinioConfig:
    endpoint: str
    bucket: str
    access_key_id: str
    secret_access_key: str
    region: str
    use_ssl: bool


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-count", type=int, default=3)
    parser.add_argument("--metric-key-count", type=int, default=3)
    parser.add_argument("--steps", type=int, default=100)
    parser.add_argument("--start-step", type=int, default=40)
    parser.add_argument("--end-step", type=int, default=60)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    config = _config_from_environment()
    if shutil.which("mc") is None:
        raise RuntimeError("MinIO client `mc` is required")
    results = [run_backend(config, backend, args) for backend in _BACKENDS]
    print(json.dumps({"acceptance": "minio_partition_pruning", "results": results}, indent=2))
    return 0


def run_backend(
    config: MinioConfig,
    catalog_backend: _CatalogBackend,
    args: argparse.Namespace,
) -> dict[str, Any]:
    if min(args.run_count, args.metric_key_count, args.steps) <= 0:
        raise ValueError("run, metric-key, and step counts must be positive")
    valid_range = 0 <= args.start_step < args.end_step <= args.steps
    if not valid_range:
        raise ValueError("step range must satisfy 0 <= start < end <= steps")
    prefix = f"seex-partition-acceptance/{uuid.uuid4().hex}/{catalog_backend}"
    target_run_id = f"run-{args.run_count // 2}"
    target_key_index = args.metric_key_count // 2
    target_metric_key = f"metric/{target_key_index}"
    try:
        with tempfile.TemporaryDirectory(prefix="seex-partition-acceptance-") as root:
            observed_points, remote = _populate_and_query(
                pathlib.Path(root),
                config,
                prefix,
                catalog_backend,
                args,
                target_run_id,
                target_metric_key,
            )
        parquet_keys = _list_parquet_keys(config, prefix)
        target_partition = f"run_id={target_run_id}/metric_key_encoded=metric%252F{target_key_index}/"
        target_files = sum(target_partition in key for key in parquet_keys)
        expected_files = args.run_count * args.metric_key_count
        if len(parquet_keys) < expected_files or target_files == 0:
            raise RuntimeError("acceptance dataset did not produce the requested files")
        if observed_points != args.end_step - args.start_step:
            raise RuntimeError("partition-pruning query returned unexpected points")
        return {
            "catalog_backend": catalog_backend,
            "run_count": args.run_count,
            "metric_key_count": args.metric_key_count,
            "total_parquet_file_count": len(parquet_keys),
            "target_partition_file_count": target_files,
            "target_run_id": target_run_id,
            "target_metric_key": target_metric_key,
            "step_range": {"start": args.start_step, "end": args.end_step},
            "points_per_query": observed_points,
            "remote": remote,
        }
    finally:
        _remove_prefix(config, prefix)


def _populate_and_query(
    root: pathlib.Path,
    config: MinioConfig,
    prefix: str,
    catalog_backend: _CatalogBackend,
    args: argparse.Namespace,
    target_run_id: str,
    target_metric_key: str,
) -> tuple[int, dict[str, Any]]:
    from seex import _seex

    points: list[_seex.MetricPoint] = []
    events: list[dict[str, Any]] = []
    with _seex.init(
        root,
        data_path=f"s3://{config.bucket}/{prefix}",
        catalog_backend=catalog_backend,
        s3_endpoint=config.endpoint,
        s3_access_key_id=config.access_key_id,
        s3_secret_access_key=config.secret_access_key,
        s3_region=config.region,
        s3_path_style=True,
        s3_use_ssl=config.use_ssl,
    ) as client:
        project = client.create_project("partition acceptance", project_id="acceptance")
        for run_index in range(args.run_count):
            run = client.create_run(project.project_id, "acceptance", run_id=f"run-{run_index}")
            for step in range(args.steps):
                for key_index in range(args.metric_key_count):
                    run.log(f"metric/{key_index}", step, float(step + key_index))
            client.finish_run(run.run_id)
        trace = _start_trace(config, prefix)
        time.sleep(0.25)
        try:
            points = client.query_metric(
                target_run_id,
                target_metric_key,
                start_step=args.start_step,
                end_step=args.end_step,
            )
        finally:
            events = _stop_trace(trace)
    remote = _trace_metrics(events, target_run_id, target_metric_key)
    return len(points), remote


def _start_trace(config: MinioConfig, prefix: str) -> subprocess.Popen[str]:
    return subprocess.Popen(
        [
            "mc",
            "admin",
            "trace",
            "--json",
            "--path",
            f"{config.bucket}/{prefix}/*",
            "seex",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=_mc_environment(config),
    )


def _stop_trace(trace: subprocess.Popen[str]) -> list[dict[str, Any]]:
    time.sleep(0.25)
    trace.terminate()
    stdout, _ = trace.communicate(timeout=5)
    return [json.loads(line) for line in stdout.splitlines() if line]


def _trace_metrics(
    events: list[dict[str, Any]],
    run_id: str,
    metric_key: str,
) -> dict[str, Any]:
    parquet_reads = [
        event
        for event in events
        if event.get("api") == "s3.GetObject" and str(event.get("path", "")).endswith(".parquet")
    ]
    encoded_key = urllib.parse.quote(metric_key, safe="-_.~").replace("%", "%25")
    expected = (f"/run_id={run_id}/", f"/metric_key_encoded={encoded_key}/")
    unrelated = [
        event
        for event in parquet_reads
        if not all(fragment in urllib.parse.unquote(str(event.get("path", ""))) for fragment in expected)
    ]
    if not parquet_reads:
        raise RuntimeError("query trace captured no Parquet reads")
    if unrelated:
        raise RuntimeError("query read an unrelated run or metric-key partition")
    return {
        "request_count": len(events),
        "parquet_get_count": len(parquet_reads),
        "unrelated_partition_reads": 0,
    }


def _list_parquet_keys(config: MinioConfig, prefix: str) -> list[str]:
    completed = _run_mc(config, "ls", "--recursive", "--json", _mc_target(config, prefix))
    entries = (json.loads(line) for line in completed.stdout.splitlines() if line)
    return [entry["key"] for entry in entries if entry.get("type") == "file" and entry["key"].endswith(".parquet")]


def _remove_prefix(config: MinioConfig, prefix: str) -> None:
    _run_mc(config, "rm", "--recursive", "--force", _mc_target(config, prefix))


def _run_mc(config: MinioConfig, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["mc", *arguments],
        check=True,
        text=True,
        capture_output=True,
        env=_mc_environment(config),
    )


def _mc_environment(config: MinioConfig) -> dict[str, str]:
    env = os.environ.copy()
    scheme = "https" if config.use_ssl else "http"
    user = urllib.parse.quote(config.access_key_id, safe="")
    password = urllib.parse.quote(config.secret_access_key, safe="")
    env["MC_HOST_seex"] = f"{scheme}://{user}:{password}@{config.endpoint}"
    return env


def _mc_target(config: MinioConfig, prefix: str) -> str:
    return f"seex/{config.bucket}/{prefix}/main/metric_points/"


def _config_from_environment() -> MinioConfig:
    missing = [name for name in _REQUIRED_ENV if not os.environ.get(name)]
    if missing:
        raise RuntimeError("set MinIO environment variables: " + ", ".join(missing))
    return MinioConfig(
        endpoint=os.environ["SEEX_MINIO_ENDPOINT"],
        bucket=os.environ["SEEX_MINIO_BUCKET"],
        access_key_id=os.environ["SEEX_MINIO_ACCESS_KEY_ID"],
        secret_access_key=os.environ["SEEX_MINIO_SECRET_ACCESS_KEY"],
        region=os.environ.get("SEEX_MINIO_REGION", "us-east-1"),
        use_ssl=os.environ.get("SEEX_MINIO_USE_SSL", "false").lower() == "true",
    )


if __name__ == "__main__":
    raise SystemExit(main())
