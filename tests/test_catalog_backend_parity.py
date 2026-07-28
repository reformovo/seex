"""Verify DuckLake catalog backend parity for DuckDB and SQLite."""

from __future__ import annotations

import pathlib
import sqlite3
from typing import Literal

import pytest

from tests import helpers

_CatalogBackend = Literal["duckdb", "sqlite"]
_CATALOG_BACKENDS: tuple[_CatalogBackend, ...] = ("duckdb", "sqlite")


@pytest.mark.parametrize("catalog_backend", _CATALOG_BACKENDS)
def test_catalog_backend_round_trips_native_storage_workflow(
    tmp_path: pathlib.Path,
    catalog_backend: _CatalogBackend,
) -> None:
    import seex

    root_path = tmp_path / catalog_backend / "seex"
    data_path = tmp_path / catalog_backend / "custom-data"
    catalog_path = tmp_path / catalog_backend / "catalog" / "custom-catalog.db"
    client = seex.init(
        root_path,
        data_path=data_path,
        catalog_backend=catalog_backend,
        catalog_path=catalog_path,
    )
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run.log("train/loss", 0, 0.25)
    run.log("train/loss", 1, 0.125)
    run.log("train/loss", 1, 0.0625)
    run.log("eval/accuracy", 0, 0.8)

    active_points = helpers.wait_for_metric_points(
        client,
        run.run_id,
        "train/loss",
        expected_count=2,
    )
    helpers.wait_for_metric_points(
        client,
        run.run_id,
        "eval/accuracy",
        expected_count=1,
    )
    active_metrics = client.list_metrics(run.run_id)
    discovered_projects = client.list_projects()
    discovered_runs = client.list_runs(project.project_id, status="running", limit=1, offset=0)
    ranged_points = client.query_metric(run.run_id, "train/loss", start_step=0, end_step=1)
    finished = client.finish_run(run.run_id)
    client.flush_run_data(run.run_id)
    terminal_points = client.query_metric(run.run_id, "train/loss")
    summaries = client.query_metric_summaries([run.run_id], "train/loss")
    metrics = client.list_metrics(run.run_id)
    diagnostics = client.diagnostics()
    del run
    del client

    reopened = seex.init(
        root_path,
        data_path=data_path,
        catalog_backend=catalog_backend,
        catalog_path=catalog_path,
    )
    reopened_project = reopened.get_project(project.project_id)
    reopened_run = reopened.get_run(finished.run_id)
    reopened_runs = reopened.list_runs(project.project_id)
    reopened_points = reopened.query_metric(finished.run_id, "train/loss")

    assert [point.step for point in active_points] == [0, 1]
    assert [item.project_id for item in discovered_projects] == ["project-1"]
    assert [item.run_id for item in discovered_runs] == ["run-1"]
    assert [point.step for point in ranged_points] == [0]
    assert [metric.metric_key for metric in active_metrics] == [
        "eval/accuracy",
        "train/loss",
    ]
    assert [metric.effective_count for metric in active_metrics] == [1, 2]
    assert finished.status == "finished"
    assert [point.step for point in terminal_points] == [0, 1]
    assert [point.value_f64 for point in terminal_points] == [0.25, 0.0625]
    assert [summary.effective_count for summary in summaries] == [2]
    assert [summary.last_value_f64 for summary in summaries] == [0.0625]
    assert [metric.metric_key for metric in metrics] == [
        "eval/accuracy",
        "train/loss",
    ]
    assert diagnostics.last_flush_status == "succeeded"
    assert any(
        (data_path / "main" / "metric_points" / "run_id=run-1" / "metric_key_encoded=train%252Floss").glob("*.parquet")
    )
    assert catalog_path.is_file()
    assert data_path.is_dir()
    assert reopened_project.name == "local training"
    assert reopened_run.status == "finished"
    assert [stored_run.run_id for stored_run in reopened_runs] == ["run-1"]
    assert [point.value_f64 for point in reopened_points] == [0.25, 0.0625]


@pytest.mark.parametrize("catalog_backend", _CATALOG_BACKENDS)
@pytest.mark.parametrize("terminal_method", ["finish_run", "fail_run"])
def test_short_run_metrics_flush_from_inline_to_parquet(
    tmp_path: pathlib.Path,
    catalog_backend: _CatalogBackend,
    terminal_method: str,
) -> None:
    import seex

    root_path = tmp_path / catalog_backend / terminal_method / "seex"
    data_path = tmp_path / catalog_backend / terminal_method / "data"
    client = seex.init(
        root_path,
        data_path=data_path,
        catalog_backend=catalog_backend,
    )
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    for step in range(16):
        run.log("train/loss", step, float(step))

    active_points = helpers.wait_for_metric_points(
        client,
        run.run_id,
        "train/loss",
        expected_count=16,
    )
    assert [point.step for point in active_points] == list(range(16))
    assert not list((data_path / "main" / "metric_points").rglob("*.parquet"))

    terminal_run = getattr(client, terminal_method)(run.run_id)
    terminal_points = client.query_metric(run.run_id, "train/loss")
    partition_path = data_path / "main" / "metric_points" / "run_id=run-1" / "metric_key_encoded=train%252Floss"

    assert terminal_run.status == ("finished" if terminal_method == "finish_run" else "failed")
    assert [point.step for point in terminal_points] == list(range(16))
    assert any(partition_path.glob("*.parquet"))


@pytest.mark.parametrize("catalog_backend", _CATALOG_BACKENDS)
def test_catalog_backend_rejects_invalid_local_storage_configuration(
    tmp_path: pathlib.Path,
    catalog_backend: _CatalogBackend,
) -> None:
    import seex

    with pytest.raises(seex.InvalidConfigurationError):
        seex.init(
            tmp_path / catalog_backend / "seex",
            catalog_backend=catalog_backend,
            data_path="http://bucket/seex",
        )


@pytest.mark.parametrize("catalog_backend", _CATALOG_BACKENDS)
def test_catalog_backend_rejects_s3_catalog_path(
    tmp_path: pathlib.Path,
    catalog_backend: _CatalogBackend,
) -> None:
    import seex

    with pytest.raises(
        seex.InvalidConfigurationError,
        match="catalog_path must be a local filesystem path",
    ):
        seex.init(
            tmp_path / catalog_backend / "seex-s3-catalog",
            catalog_backend=catalog_backend,
            catalog_path="s3://bucket/catalog.ducklake",
        )


def test_sqlite_catalog_file_contains_ducklake_and_seex_state(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    catalog_path = root_path / ".seex" / "catalog.sqlite"
    client = seex.init(root_path, catalog_backend="sqlite")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run.log("train/loss", 0, 0.25)
    helpers.wait_for_metric_points(
        client,
        run.run_id,
        "train/loss",
        expected_count=1,
    )

    tables_before_flush = _sqlite_table_names(catalog_path)
    inline_tables = [table for table in tables_before_flush if table.startswith("ducklake_inlined_data_")]
    assert "ducklake_metadata" in tables_before_flush
    assert "ducklake_table" in tables_before_flush
    assert "seex_projects" in tables_before_flush
    assert "seex_runs" in tables_before_flush
    assert "seex_metric_aggregates" in tables_before_flush
    assert inline_tables
    assert _sqlite_table_count(catalog_path, "seex_projects") == 1
    assert _sqlite_table_count(catalog_path, "seex_runs") == 1
    assert sum(_sqlite_table_count(catalog_path, table) for table in inline_tables) >= 1

    client.finish_run(run.run_id)

    assert _sqlite_table_count(catalog_path, "seex_metric_aggregates") == 1
    assert _sqlite_table_count(catalog_path, "ducklake_data_file") >= 1


def test_unknown_catalog_backend_is_rejected(tmp_path: pathlib.Path) -> None:
    import seex

    with pytest.raises(seex.InvalidConfigurationError, match="postgres"):
        seex.init(
            tmp_path / "seex",
            catalog_backend="postgres",  # type: ignore[reportArgumentType]
        )


def _sqlite_table_names(catalog_path: pathlib.Path) -> set[str]:
    with sqlite3.connect(catalog_path) as connection:
        rows = connection.execute("SELECT name FROM sqlite_master WHERE type = 'table'").fetchall()
    return {str(row[0]) for row in rows}


def _sqlite_table_count(catalog_path: pathlib.Path, table_name: str) -> int:
    if not table_name.replace("_", "").isalnum():
        raise ValueError(f"invalid SQLite table name: {table_name}")
    with sqlite3.connect(catalog_path) as connection:
        count = connection.execute(f'SELECT count(*) FROM "{table_name}"').fetchone()
    if count is None:
        raise AssertionError(f"missing SQLite count for {table_name}")
    return int(count[0])
