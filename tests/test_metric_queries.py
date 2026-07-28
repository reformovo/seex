"""Verify Python SDK metric query, discovery, and max-points behavior."""

from __future__ import annotations

import pathlib

import pytest

from tests import helpers


def test_client_queries_metric_points_and_terminal_summaries(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run.log("train/loss", 0, 0.25)
    run.log("train/loss", 1, 0.125)

    points = helpers.wait_for_metric_points(
        client,
        run.run_id,
        "train/loss",
        expected_count=2,
    )
    client.finish_run(run.run_id)
    summaries = client.query_metric_summaries([run.run_id], "train/loss")
    diagnostics = client.diagnostics()

    assert [point.step for point in points] == [0, 1]
    assert [point.value_f64 for point in points] == [0.25, 0.125]
    assert isinstance(points[0], seex.MetricPoint)
    assert len(summaries) == 1
    assert isinstance(summaries[0], seex.MetricSummary)
    assert summaries[0].effective_count == 2
    assert summaries[0].last_step == 1
    assert summaries[0].last_value_f64 == 0.125
    assert diagnostics.pending_reports == 0
    assert diagnostics.persisted_reports >= 2
    assert diagnostics.writer_state == "drained"
    assert diagnostics.last_write_error is None


def test_table_queries_preserve_object_query_results(tmp_path: pathlib.Path) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run.log("train/loss", 0, 0.25)
    helpers.wait_for_metric_points(client, run.run_id, "train/loss", expected_count=1)

    points = client.query_metric(run.run_id, "train/loss")
    point_table = client.query_metric_table(run.run_id, "train/loss")
    summary_table = client.query_metric_summaries_table([run.run_id], "train/loss")

    assert [point.value_f64 for point in points] == [0.25]
    assert point_table.row_count == 1
    assert point_table.source_row_count == 1
    assert point_table.downsampled is False
    assert point_table.column_names == [
        "run_id",
        "metric_key",
        "step",
        "timestamp",
        "value_f64",
        "ingested_at",
    ]
    assert summary_table.row_count == 1
    assert summary_table.source_row_count == 1
    assert summary_table.downsampled is False
    assert '"arrow_array_stream"' in repr(point_table.__arrow_c_stream__())
    assert '"arrow_array_stream"' in repr(point_table.__arrow_c_stream__(requested_schema=None))
    assert '"arrow_array_stream"' in repr(summary_table.__arrow_c_stream__())


def test_empty_arrow_tables_preserve_public_schemas(tmp_path: pathlib.Path) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    point_table = client.query_metric_table("missing-run", "train/loss")
    summary_table = client.query_metric_summaries_table([], "train/loss")

    assert point_table.row_count == 0
    assert point_table.source_row_count == 0
    assert point_table.column_names == [
        "run_id",
        "metric_key",
        "step",
        "timestamp",
        "value_f64",
        "ingested_at",
    ]
    assert summary_table.row_count == 0
    assert summary_table.source_row_count == 0
    assert summary_table.column_names == [
        "run_id",
        "metric_key",
        "effective_count",
        "last_step",
        "last_value_f64",
        "min_value_f64",
        "max_value_f64",
    ]


def test_active_run_discovery_and_summaries_use_persisted_points(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run.log("eval/accuracy", 0, 0.8)
    run.log("train/loss", 0, 0.25)
    helpers.wait_for_metric_points(
        client,
        run.run_id,
        "eval/accuracy",
        expected_count=1,
    )
    helpers.wait_for_metric_points(
        client,
        run.run_id,
        "train/loss",
        expected_count=1,
    )

    points = client.query_metric(run.run_id, "train/loss")
    metrics = client.list_metrics(run.run_id)
    summaries = client.query_metric_summaries([run.run_id], "train/loss")

    assert [point.value_f64 for point in points] == [0.25]
    assert [metric.metric_key for metric in metrics] == ["eval/accuracy", "train/loss"]
    assert [metric.last_value_f64 for metric in metrics] == [0.8, 0.25]
    assert [summary.last_value_f64 for summary in summaries] == [0.25]


def test_terminal_run_metric_discovery_uses_rebuilt_aggregate_state(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run.log("eval/accuracy", 0, 0.8)
    run.log("train/loss", 0, 0.25)

    client.finish_run(run.run_id)
    metrics = client.list_metrics(run.run_id)

    assert [metric.metric_key for metric in metrics] == [
        "eval/accuracy",
        "train/loss",
    ]
    assert [metric.effective_count for metric in metrics] == [1, 1]
    assert isinstance(metrics[0], seex.MetricSummary)


def test_summary_comparison_preserves_mixed_run_order(tmp_path: pathlib.Path) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    terminal = client.create_run(project.project_id, "terminal", run_id="terminal")
    terminal.log("train/loss", 0, 0.5)
    client.finish_run(terminal.run_id)
    running = client.create_run(project.project_id, "running", run_id="running")
    running.log("train/loss", 0, 0.25)
    helpers.wait_for_metric_points(
        client,
        running.run_id,
        "train/loss",
        expected_count=1,
    )

    summaries = client.query_metric_summaries(
        [running.run_id, terminal.run_id],
        "train/loss",
    )

    assert [summary.run_id for summary in summaries] == ["running", "terminal"]
    assert [summary.last_value_f64 for summary in summaries] == [0.25, 0.5]


def test_client_query_metric_applies_range_filters_and_short_max_points(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    for step in range(100):
        run.log("train/loss", step, float(step))
    helpers.wait_for_metric_points(
        client,
        run.run_id,
        "train/loss",
        expected_count=100,
    )

    ranged_points = client.query_metric(
        run.run_id,
        "train/loss",
        start_step=10,
        end_step=15,
    )
    empty_points = client.query_metric(
        run.run_id,
        "train/loss",
        start_step=10,
        end_step=10,
    )
    unchanged_points = client.query_metric(
        run.run_id,
        "train/loss",
        max_points=100,
    )

    assert [point.step for point in ranged_points] == [10, 11, 12, 13, 14]
    assert empty_points == []
    assert len(unchanged_points) == 100
    assert unchanged_points[0].step == 0
    assert unchanged_points[-1].step == 99


def test_sdk_downsampling_does_not_install_lttb_without_opt_in(
    tmp_path: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import seex

    isolated_home = tmp_path / "home"
    isolated_home.mkdir()
    monkeypatch.setenv("HOME", str(isolated_home))
    monkeypatch.delenv("SEEX_LTTB_AUTO_INSTALL", raising=False)
    monkeypatch.delenv("SEEX_LTTB_EXTENSION_PATH", raising=False)
    client = seex.init(tmp_path / "project")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    for step in range(201):
        run.log("train/loss", step, float(step))
    client.finish_run(run.run_id)

    with pytest.raises(seex.StorageError) as error_info:
        client.query_metric(run.run_id, "train/loss", max_points=200)

    message = str(error_info.value)
    assert "will not download it automatically" in message
    assert "SEEX_LTTB_AUTO_INSTALL=1" in message
    client.shutdown()
