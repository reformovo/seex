"""Verify DuckLake catalog backend parity for DuckDB and SQLite."""

from __future__ import annotations

import pathlib
import sqlite3
import time
from typing import Literal

import pytest
import seex

_CatalogBackend = Literal["duckdb", "sqlite"]
_CATALOG_BACKENDS: tuple[_CatalogBackend, ...] = ("duckdb", "sqlite")


@pytest.mark.parametrize("catalog_backend", _CATALOG_BACKENDS)
def test_catalog_backend_round_trips_native_storage_workflow(
    tmp_path: pathlib.Path,
    catalog_backend: _CatalogBackend,
) -> None:
    root_path = tmp_path / catalog_backend / "seex"
    data_path = tmp_path / catalog_backend / "custom-data"
    catalog_path = tmp_path / catalog_backend / "catalog" / "custom-catalog.db"
    settings = seex.Settings(
        data_path=data_path,
        catalog_backend=catalog_backend,
        catalog_path=catalog_path,
    )
    run = seex.init(
        project="project-1",
        dir=root_path,
        id="run-1",
        name="baseline",
        settings=settings,
    )
    run.log({"train/loss": 0.25, "eval/accuracy": 0.8}, step=0)
    run.log({"train/loss": 0.125}, step=1)
    run.log({"train/loss": 0.0625}, step=1)
    run.finish()
    diagnostics = run.diagnostics()

    with seex.Api(root_path, settings) as api:
        projects = api.projects()
        project = api.project("project-1")
        runs = api.runs("project-1")
        record = api.run("project-1/run-1")
        assert record is not None
        terminal_points = record.history("train/loss").points
        ranged_points = record.history("train/loss", start=0, end=1).points
        metrics = record.metrics()
        summary = record.metric_summary("train/loss")
        assert [item.project_id for item in projects] == ["project-1"]
        assert [item.run_id for item in runs] == ["run-1"]
        assert [point.step for point in ranged_points] == [0]
        assert [point.step for point in terminal_points] == [0, 1]
        assert [point.value_f64 for point in terminal_points] == [0.25, 0.0625]
        assert summary is not None
        assert (summary.effective_count, summary.last_value_f64) == (2, 0.0625)
        assert [metric.metric_key for metric in metrics] == [
            "eval/accuracy",
            "train/loss",
        ]
        assert project is not None and project.name == "project-1"
        assert record.status == "finished"

    assert run.status == "finished"
    assert diagnostics.last_flush_status == "succeeded"
    assert any(
        (data_path / "main" / "metric_points" / "run_id=run-1" / "metric_key_encoded=train%252Floss").glob("*.parquet")
    )
    assert catalog_path.is_file()
    assert data_path.is_dir()


@pytest.mark.parametrize("catalog_backend", _CATALOG_BACKENDS)
@pytest.mark.parametrize("terminal_method", ["finish_run", "fail_run"])
def test_short_run_metrics_flush_from_inline_to_parquet(
    tmp_path: pathlib.Path,
    catalog_backend: _CatalogBackend,
    terminal_method: str,
) -> None:
    root_path = tmp_path / catalog_backend / terminal_method / "seex"
    data_path = tmp_path / catalog_backend / terminal_method / "data"
    settings = seex.Settings(data_path=data_path, catalog_backend=catalog_backend)
    run = seex.init(
        project="project-1",
        dir=root_path,
        id="run-1",
        name="baseline",
        settings=settings,
    )
    for step in range(16):
        run.log({"train/loss": float(step)}, step=step)

    _wait_for_drain(run)
    assert not list((data_path / "main" / "metric_points").rglob("*.parquet"))

    run.finish(0 if terminal_method == "finish_run" else 1)
    record = seex.Api(root_path, settings).run("project-1/run-1")
    assert record is not None
    terminal_points = record.history("train/loss").points
    partition_path = data_path / "main" / "metric_points" / "run_id=run-1" / "metric_key_encoded=train%252Floss"

    assert run.status == ("finished" if terminal_method == "finish_run" else "failed")
    assert [point.step for point in terminal_points] == list(range(16))
    assert any(partition_path.glob("*.parquet"))


@pytest.mark.parametrize("catalog_backend", _CATALOG_BACKENDS)
def test_catalog_backend_rejects_invalid_local_storage_configuration(
    tmp_path: pathlib.Path,
    catalog_backend: _CatalogBackend,
) -> None:
    with pytest.raises(seex.InvalidConfigurationError):
        seex.init(
            dir=tmp_path / catalog_backend / "seex",
            settings=seex.Settings(
                catalog_backend=catalog_backend,
                data_path="http://bucket/seex",
            ),
        )


@pytest.mark.parametrize("catalog_backend", _CATALOG_BACKENDS)
def test_catalog_backend_rejects_s3_catalog_path(
    tmp_path: pathlib.Path,
    catalog_backend: _CatalogBackend,
) -> None:
    with pytest.raises(
        seex.InvalidConfigurationError,
        match="configuration is invalid",
    ):
        seex.init(
            dir=tmp_path / catalog_backend / "seex-s3-catalog",
            settings=seex.Settings(
                catalog_backend=catalog_backend,
                catalog_path="s3://bucket/catalog.ducklake",
            ),
        )


def test_sqlite_catalog_file_contains_ducklake_and_seex_state(
    tmp_path: pathlib.Path,
) -> None:
    root_path = tmp_path / "seex"
    catalog_path = root_path / ".seex" / "catalog.sqlite"
    settings = seex.Settings(catalog_backend="sqlite")
    run = seex.init(
        project="project-1",
        dir=root_path,
        id="run-1",
        name="baseline",
        settings=settings,
    )
    run.log({"train/loss": 0.25}, step=0)
    _wait_for_drain(run)

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

    run.finish()

    assert _sqlite_table_count(catalog_path, "seex_metric_aggregates") == 1
    assert _sqlite_table_count(catalog_path, "ducklake_data_file") >= 1


def test_unknown_catalog_backend_is_rejected(tmp_path: pathlib.Path) -> None:
    with pytest.raises(ValueError, match="catalog_backend"):
        seex.init(
            dir=tmp_path / "seex",
            settings=seex.Settings(
                catalog_backend="postgres",  # type: ignore[reportArgumentType]
            ),
        )


def _wait_for_drain(run: seex.Run) -> None:
    deadline = time.monotonic() + 5.0
    while run.diagnostics().pending_reports != 0:
        if time.monotonic() >= deadline:
            raise AssertionError("timed out waiting for metric persistence")
        time.sleep(0.01)


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
