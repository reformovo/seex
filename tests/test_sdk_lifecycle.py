"""Verify Python SDK client, project, and run lifecycle behavior."""

from __future__ import annotations

import pathlib

import pytest

from tests import helpers

_S3_OVERRIDE_CONFIG = """data_path = "s3://bucket/seex"

[s3]
endpoint = "from-config:9000"
access_key_id = "from-config"
secret_access_key = "from-config-secret"
path_style = false
use_ssl = true
"""

_INVALID_S3_BOOL_CONFIG = """data_path = "s3://bucket/seex"

[s3]
endpoint = "127.0.0.1:9000"
access_key_id = "seex"
secret_access_key = "secret"
path_style = "yes"
"""


def _write_project_config(root_path: pathlib.Path, content: str) -> None:
    config_path = root_path / ".seex" / "config.toml"
    config_path.parent.mkdir(parents=True)
    config_path.write_text(f"schema_version = 1\n{content}", encoding="utf-8")


def test_init_returns_client(tmp_path: pathlib.Path) -> None:
    import seex

    client = seex.init(tmp_path / "seex")

    assert isinstance(client, seex.Client)


def test_init_without_path_uses_current_working_directory(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import seex

    monkeypatch.chdir(tmp_path)
    client = seex.init()
    project = client.create_project("local training", project_id="project-1")

    assert isinstance(client, seex.Client)
    assert project.project_id == "project-1"
    assert (tmp_path / ".seex" / "catalog.ducklake").is_file()
    assert (tmp_path / ".seex" / "data").is_dir()


def test_init_accepts_storage_configuration_keywords(tmp_path: pathlib.Path) -> None:
    import seex

    data_path = tmp_path / "custom-data"
    catalog_path = tmp_path / "catalog" / "catalog.ducklake"
    client = seex.init(
        tmp_path / "seex",
        data_path=data_path,
        catalog_backend="duckdb",
        catalog_path=catalog_path,
        metric_queue_capacity=1024,
    )
    project = client.create_project("local training", project_id="project-1")

    assert isinstance(client, seex.Client)
    assert project.project_id == "project-1"
    assert data_path.is_dir()
    assert catalog_path.is_file()


def test_init_uses_configured_data_path(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"
    configured_data_path = tmp_path / "configured-data"
    _write_project_config(root_path, f'data_path = "{configured_data_path.as_posix()}"\n')

    client = seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")

    assert project.project_id == "project-1"
    assert configured_data_path.is_dir()
    assert not (root_path / ".seex" / "data").exists()


def test_init_data_path_keyword_overrides_config(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    explicit_data_path = tmp_path / "explicit-data"
    configured_data_path = tmp_path / "configured-data"
    _write_project_config(root_path, f'data_path = "{configured_data_path.as_posix()}"\n')

    client = seex.init(root_path, data_path=explicit_data_path)
    project = client.create_project("local training", project_id="project-1")

    assert project.project_id == "project-1"
    assert explicit_data_path.is_dir()
    assert not configured_data_path.exists()


def test_init_uses_configured_catalog_backend_and_path(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    catalog_path = tmp_path / "configured" / "catalog.sqlite"
    _write_project_config(
        root_path,
        f'catalog_backend = "sqlite"\ncatalog_path = "{catalog_path.as_posix()}"\n',
    )

    client = seex.init(root_path, catalog_backend=None)
    project = client.create_project("local training", project_id="project-1")

    assert project.project_id == "project-1"
    assert catalog_path.is_file()
    assert not (root_path / ".seex" / "catalog.ducklake").exists()


def test_init_catalog_keywords_override_config(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"
    configured_path = tmp_path / "configured" / "catalog.sqlite"
    explicit_path = tmp_path / "explicit" / "catalog.db"
    _write_project_config(
        root_path,
        f'catalog_backend = "sqlite"\ncatalog_path = "{configured_path.as_posix()}"\n',
    )

    client = seex.init(
        root_path,
        catalog_backend="duckdb",
        catalog_path=explicit_path,
    )
    project = client.create_project("local training", project_id="project-1")

    assert project.project_id == "project-1"
    assert explicit_path.is_file()
    assert not configured_path.exists()


def test_init_resolves_configured_relative_storage_paths_from_project_root(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import seex

    root_path = tmp_path / "project"
    _write_project_config(
        root_path,
        'catalog_backend = "sqlite"\ncatalog_path = "storage/catalog.sqlite"\ndata_path = "storage/data"\n',
    )
    unrelated_path = tmp_path / "unrelated"
    unrelated_path.mkdir()
    monkeypatch.chdir(unrelated_path)

    client = seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")

    assert project.project_id == "project-1"
    assert (root_path / "storage" / "catalog.sqlite").is_file()
    assert (root_path / "storage" / "data").is_dir()
    assert not (unrelated_path / "storage").exists()


def test_init_data_path_keyword_ignores_configured_s3(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    explicit_data_path = tmp_path / "explicit-data"
    _write_project_config(root_path, _S3_OVERRIDE_CONFIG)

    client = seex.init(root_path, data_path=explicit_data_path)
    project = client.create_project("local training", project_id="project-1")

    assert project.project_id == "project-1"
    assert explicit_data_path.is_dir()


def test_init_rejects_missing_required_s3_config(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"

    with pytest.raises(
        seex.InvalidConfigurationError,
        match="s3_endpoint is required when data_path is s3://",
    ):
        seex.init(root_path, data_path="s3://bucket/seex")

    assert not root_path.exists()


def test_init_rejects_invalid_config_toml_s3_setting(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    _write_project_config(root_path, _INVALID_S3_BOOL_CONFIG)

    with pytest.raises(
        seex.InvalidConfigurationError,
        match="config.toml s3.path_style must be a boolean",
    ):
        seex.init(root_path)


def test_init_s3_keyword_override_skips_invalid_config_value(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    _write_project_config(root_path, _INVALID_S3_BOOL_CONFIG)

    try:
        client = seex.init(
            root_path,
            data_path="s3://bucket/seex",
            s3_endpoint="127.0.0.1:9000",
            s3_access_key_id="seex",
            s3_secret_access_key="secret",
            s3_path_style=True,
        )
    except seex.StorageError:
        return

    client.shutdown()


def test_init_rejects_invalid_config_toml_data_path(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    _write_project_config(root_path, "data_path = 123\n")

    with pytest.raises(
        seex.InvalidConfigurationError,
        match="config.toml data_path must be a string",
    ):
        seex.init(root_path)


def test_init_accepts_explicit_duckdb_catalog_path_without_ducklake_suffix(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    catalog_path = tmp_path / "catalog" / "seex-catalog.db"
    client = seex.init(
        tmp_path / "seex",
        catalog_backend="duckdb",
        catalog_path=catalog_path,
    )
    project = client.create_project("local training", project_id="project-1")

    assert project.project_id == "project-1"
    assert catalog_path.is_file()


def test_init_uses_duckdb_catalog_and_data_defaults(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")

    assert project.project_id == "project-1"
    assert (root_path / ".seex" / "catalog.ducklake").is_file()
    assert (root_path / ".seex" / "data").is_dir()


def test_init_uses_sqlite_catalog_and_data_defaults(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path, catalog_backend="sqlite")
    project = client.create_project("local training", project_id="project-1")

    assert project.project_id == "project-1"
    assert (root_path / ".seex" / "catalog.sqlite").is_file()
    assert (root_path / ".seex" / "data").is_dir()


def test_init_rejects_invalid_storage_configuration(tmp_path: pathlib.Path) -> None:
    import seex

    invalid_kwargs = [
        {"metric_queue_capacity": -1},
        {"metric_queue_capacity": 0},
        {"metric_queue_capacity": 1_048_577},
        {"catalog_backend": "postgres"},
        {"data_path": "http://bucket/seex"},
    ]
    for index, kwargs in enumerate(invalid_kwargs):
        root_path = tmp_path / f"seex-{index}"
        with pytest.raises(seex.InvalidConfigurationError):
            seex.init(root_path, **kwargs)
        assert not root_path.exists()


def test_init_rejects_s3_catalog_path(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"

    with pytest.raises(
        seex.InvalidConfigurationError,
        match="catalog_path must be a local filesystem path",
    ):
        seex.init(root_path, catalog_path="s3://bucket/catalog.ducklake")

    assert not root_path.exists()


def test_client_creates_project_and_run(tmp_path: pathlib.Path) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")

    assert isinstance(project, seex.Project)
    assert project.project_id == "project-1"
    assert project.name == "local training"
    assert isinstance(run, seex.Run)
    assert run.run_id == "run-1"
    assert run.project_id == project.project_id
    assert run.name == "baseline"
    assert run.status == "running"


def test_client_selects_existing_project_and_run(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    del client

    reopened_client = seex.init(root_path)
    selected_project = reopened_client.get_project(project.project_id)
    selected_run = reopened_client.get_run(run.run_id)

    assert isinstance(selected_project, seex.Project)
    assert selected_project.project_id == project.project_id
    assert selected_project.name == "local training"
    assert isinstance(selected_run, seex.Run)
    assert selected_run.run_id == run.run_id
    assert selected_run.project_id == selected_project.project_id
    assert selected_run.name == "baseline"
    assert selected_run.status == "running"


def test_client_lists_projects_in_stable_catalog_order(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path)

    assert client.list_projects() == []

    first = client.create_project("local training", project_id="project-1")
    second = client.create_project("sweep", project_id="project-2")
    del client

    projects = seex.init(root_path).list_projects()

    assert [project.project_id for project in projects] == [
        first.project_id,
        second.project_id,
    ]
    assert [project.name for project in projects] == ["local training", "sweep"]


def test_client_resumes_existing_run_for_logging(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run_id = run.run_id
    del run
    del client

    resumed_client = seex.init(root_path)
    resumed_run = resumed_client.resume_run(run_id)
    resumed_run.log("train/loss", 0, 0.25)
    points = helpers.wait_for_metric_points(
        resumed_client,
        resumed_run.run_id,
        "train/loss",
        expected_count=1,
    )

    assert isinstance(resumed_run, seex.Run)
    assert resumed_run.run_id == run_id
    assert [point.value_f64 for point in points] == [0.25]


def test_active_run_lock_conflict_and_release_after_shutdown(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    first_client = seex.init(root_path)
    project = first_client.create_project("local training", project_id="project-1")
    run = first_client.create_run(project.project_id, "baseline", run_id="run-1")
    second_client = seex.init(root_path)

    with pytest.raises(seex.RunAlreadyActiveError, match="run-1"):
        second_client.resume_run(run.run_id)

    first_client.shutdown()
    resumed = second_client.resume_run(run.run_id)

    assert resumed.run_id == run.run_id


def test_create_run_existing_id_requires_explicit_resume(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    first_client = seex.init(root_path)
    project = first_client.create_project("local training", project_id="project-1")
    first_client.create_run(project.project_id, "baseline", run_id="run-1")
    second_client = seex.init(root_path)

    with pytest.raises(seex.RunAlreadyExistsError, match="run-1"):
        second_client.create_run(project.project_id, "duplicate", run_id="run-1")


def test_resume_run_rejects_terminal_runs(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    client.finish_run(run.run_id)

    with pytest.raises(seex.InvalidRunStateError, match="finished -> running"):
        client.resume_run(run.run_id)


def test_leftover_lock_file_does_not_block_resume(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    first_client = seex.init(root_path)
    project = first_client.create_project("local training", project_id="project-1")
    run = first_client.create_run(
        project.project_id,
        "baseline",
        run_id="run/leftover lock",
    )
    first_client.shutdown()
    lock_file = root_path / ".seex" / "locks" / "runs" / "run%2Fleftover%20lock.lock"

    resumed = seex.init(root_path).resume_run(run.run_id)

    assert lock_file.is_file()
    assert resumed.run_id == run.run_id


def test_client_lists_project_runs_for_terminal_summary_queries(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")
    first_run = client.create_run(project.project_id, "baseline", run_id="run-1")
    second_run = client.create_run(project.project_id, "candidate", run_id="run-2")
    first_run.log("train/loss", 0, 0.25)
    second_run.log("train/loss", 0, 0.125)
    helpers.wait_for_metric_points(
        client,
        first_run.run_id,
        "train/loss",
        expected_count=1,
    )
    helpers.wait_for_metric_points(
        client,
        second_run.run_id,
        "train/loss",
        expected_count=1,
    )
    client.finish_run(first_run.run_id)
    client.finish_run(second_run.run_id)
    del first_run
    del second_run
    del client

    reopened_client = seex.init(root_path)
    runs = reopened_client.list_runs(project.project_id)
    summaries = reopened_client.query_metric_summaries(
        [run.run_id for run in runs],
        "train/loss",
    )

    assert [run.run_id for run in runs] == ["run-1", "run-2"]
    assert [run.name for run in runs] == ["baseline", "candidate"]
    assert [summary.run_id for summary in summaries] == ["run-1", "run-2"]
    assert [summary.last_value_f64 for summary in summaries] == [0.25, 0.125]


def test_client_filters_and_paginates_runs_in_stable_created_order(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    first = client.create_run(project.project_id, "first", run_id="run-1")
    second = client.create_run(project.project_id, "second", run_id="run-2")
    third = client.create_run(project.project_id, "third", run_id="run-3")
    client.finish_run(first.run_id)
    client.fail_run(third.run_id)

    runs = client.list_runs(project.project_id)
    page = client.list_runs(project.project_id, limit=1, offset=1)
    tail = client.list_runs(project.project_id, offset=2)

    assert [run.run_id for run in runs] == [first.run_id, second.run_id, third.run_id]
    assert [run.run_id for run in page] == [second.run_id]
    assert [run.run_id for run in tail] == [third.run_id]
    assert [run.run_id for run in client.list_runs(project.project_id, status="running")] == [second.run_id]
    assert [run.run_id for run in client.list_runs(project.project_id, status="finished")] == [first.run_id]
    assert [run.run_id for run in client.list_runs(project.project_id, status="failed")] == [third.run_id]


def test_client_list_runs_rejects_unknown_status(tmp_path: pathlib.Path) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")

    with pytest.raises(ValueError, match="status must be one of"):
        client.list_runs(
            project.project_id,
            status="paused",  # type: ignore[reportArgumentType]
        )


def test_client_detects_orphan_running_runs(tmp_path: pathlib.Path) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path)
    first_project = client.create_project("local training", project_id="project-1")
    second_project = client.create_project("sweep", project_id="project-2")
    first_run = client.create_run(first_project.project_id, "baseline", run_id="run-1")
    second_run = client.create_run(second_project.project_id, "candidate", run_id="run-2")
    first_run_id = first_run.run_id
    second_run_id = second_run.run_id
    del first_run
    del second_run
    del client

    reopened_client = seex.init(root_path)
    all_orphans = reopened_client.list_orphan_runs()
    project_orphans = reopened_client.list_orphan_runs(first_project.project_id)

    assert [run.run_id for run in all_orphans] == [
        first_run_id,
        second_run_id,
    ]
    assert [run.status for run in all_orphans] == ["running", "running"]
    assert [run.run_id for run in project_orphans] == [first_run_id]


def test_client_finalizes_runs_as_finished_or_failed(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    finished_run = client.create_run(project.project_id, "baseline", run_id="run-1")
    failed_run = client.create_run(project.project_id, "candidate", run_id="run-2")

    finished = client.finish_run(finished_run.run_id)
    failed = client.fail_run(failed_run.run_id)
    orphan_runs = client.list_orphan_runs(project.project_id)

    assert finished.run_id == finished_run.run_id
    assert finished.status == "finished"
    assert finished.finished_at is not None
    assert failed.run_id == failed_run.run_id
    assert failed.status == "failed"
    assert failed.finished_at is not None
    assert orphan_runs == []


def test_finalization_closes_run_for_late_logging(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run.log("train/loss", 0, 0.25)

    finished = client.finish_run(run.run_id)

    with pytest.raises(seex.RunClosedError, match="run-1"):
        run.log("train/loss", 1, 0.125)
    points = client.query_metric(run.run_id, "train/loss")
    assert finished.status == "finished"
    assert [point.value_f64 for point in points] == [0.25]


@pytest.mark.parametrize("terminal_method", ["finish_run", "fail_run"])
def test_bounded_finalization_timeout_leaves_run_running(
    tmp_path: pathlib.Path,
    terminal_method: str,
) -> None:
    import seex

    client = seex.init(tmp_path / f"seex-{terminal_method}")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    for step in range(1000):
        run.log("train/loss", step, float(step))

    with pytest.raises(seex.MetricDrainTimeoutError):
        getattr(client, terminal_method)(run.run_id, timeout=0.0)

    selected_run = client.get_run(run.run_id)
    assert selected_run.status == "running"
    assert selected_run.finished_at is None

    getattr(client, terminal_method)(run.run_id)
    terminal_run = client.get_run(run.run_id)
    assert terminal_run.status == ("finished" if terminal_method == "finish_run" else "failed")


def test_finish_run_flushes_partitioned_parquet_and_updates_diagnostics(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run.log("train/loss", 0, 0.25)

    finished = client.finish_run(run.run_id)
    diagnostics = client.diagnostics()
    partition_path = (
        root_path / ".seex" / "data" / "main" / "metric_points" / "run_id=run-1" / "metric_key_encoded=train%252Floss"
    )

    assert finished.status == "finished"
    assert diagnostics.last_flush_run_id == "run-1"
    assert diagnostics.last_flush_status == "succeeded"
    assert any(partition_path.glob("*.parquet"))


def test_flush_run_data_retries_terminal_run_visibility(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    client.finish_run(run.run_id)

    client.flush_run_data(run.run_id)
    diagnostics = client.diagnostics()

    assert diagnostics.last_flush_run_id == "run-1"
    assert diagnostics.last_flush_status == "succeeded"


def test_flush_run_data_rejects_running_runs(tmp_path: pathlib.Path) -> None:
    import seex

    client = seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")

    with pytest.raises(seex.InvalidRunStateError, match="running -> flushed"):
        client.flush_run_data(run.run_id)


def test_shutdown_does_not_finalize_running_runs(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")

    client.shutdown()

    with pytest.raises(seex.ClientClosedError):
        run.log("train/loss", 0, 0.25)
    with pytest.raises(seex.ClientClosedError):
        client.create_project("late project", project_id="late-project")
    with pytest.raises(seex.ClientClosedError):
        client.create_run(project.project_id, "late run", run_id="late-run")
    with pytest.raises(seex.ClientClosedError):
        client.resume_run(run.run_id)
    with pytest.raises(seex.ClientClosedError):
        client.finish_run(run.run_id)
    with pytest.raises(seex.ClientClosedError):
        client.fail_run(run.run_id)
    with pytest.raises(seex.ClientClosedError):
        client.flush_run_data(run.run_id)

    selected_run = client.get_run(run.run_id)
    metric_points = client.query_metric(run.run_id, "train/loss")
    assert selected_run.run_id == run.run_id
    assert metric_points == []

    reopened_client = seex.init(root_path)
    running_run = reopened_client.get_run(run.run_id)
    resumed_run = reopened_client.resume_run(run.run_id)
    assert running_run.status == "running"
    assert running_run.finished_at is None
    assert resumed_run.run_id == run.run_id
