"""Verify the opt-in MinIO partition-pruning acceptance contract."""

import pytest

from scripts import accept_minio_partition_pruning


def test_default_scenario_selects_across_runs_keys_files_and_steps_once() -> None:
    args = accept_minio_partition_pruning.build_parser().parse_args([])
    assert min(args.run_count, args.metric_key_count) > 1
    assert 0 < args.start_step < args.end_step < args.steps
    assert not hasattr(args, "repeats")


def test_trace_accepts_only_the_target_partition() -> None:
    events = [
        {
            "api": "s3.GetObject",
            "path": (
                "/bucket/prefix/main/metric_points/run_id%3Drun-1/metric_key_encoded%3Dmetric%25252Floss/data.parquet"
            ),
        },
        {"api": "s3.HeadObject", "path": "/bucket/prefix"},
    ]

    measured = accept_minio_partition_pruning._trace_metrics(events, "run-1", "metric/loss")

    assert measured["parquet_get_count"] == 1
    assert measured["unrelated_partition_reads"] == 0


def test_trace_metrics_reject_unrelated_partitions() -> None:
    events = [
        {
            "api": "s3.GetObject",
            "path": "/bucket/run_id=other/metric_key_encoded=metric%252Floss/data.parquet",
            "callStats": {"tx": 1},
        }
    ]

    with pytest.raises(RuntimeError, match="unrelated run or metric-key"):
        accept_minio_partition_pruning._trace_metrics(events, "run-1", "metric/loss")
