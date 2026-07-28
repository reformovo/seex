"""Verify the opt-in MinIO metric-query benchmark contract."""

import pytest

from scripts import bench_minio_metric_query


def test_default_scenario_selects_across_runs_keys_files_and_steps() -> None:
    args = bench_minio_metric_query.build_parser().parse_args([])
    assert min(args.run_count, args.metric_key_count, args.repeats) > 1
    assert 0 < args.start_step < args.end_step < args.steps


def test_trace_metrics_measure_bytes_and_read_amplification() -> None:
    events = [
        {
            "api": "s3.GetObject",
            "path": (
                "/bucket/prefix/main/metric_points/run_id%3Drun-1/metric_key_encoded%3Dmetric%25252Floss/data.parquet"
            ),
            "callStats": {"tx": 480},
        },
        {"api": "s3.HeadObject", "path": "/bucket/prefix", "callStats": {"tx": 20}},
    ]

    measured = bench_minio_metric_query._trace_metrics(events, "run-1", "metric/loss", points_per_query=2, repeats=2)

    assert measured["response_bytes"] == 500
    assert measured["parquet_response_bytes"] == 480
    assert measured["logical_result_bytes"] == 96
    assert measured["read_amplification"] == 5.0
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
        bench_minio_metric_query._trace_metrics(events, "run-1", "metric/loss", points_per_query=1, repeats=1)
