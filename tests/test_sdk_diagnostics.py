"""Verify Python SDK diagnostics and shutdown behavior."""

from __future__ import annotations

import pathlib

import pytest
from seex import _seex

from tests import helpers


def test_shutdown_closes_logging_and_preserves_diagnostics(
    tmp_path: pathlib.Path,
) -> None:

    root_path = tmp_path / "seex"
    client = _seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")

    assert client.shutdown() is None
    diagnostics = client.diagnostics()
    assert diagnostics.pending_reports == 0
    assert diagnostics.writer_state == "closed"
    assert diagnostics.last_write_error is None

    with pytest.raises(_seex.ClientClosedError):
        run.log("train/loss", 0, 0.25)
    assert client.diagnostics().writer_state == "closed"

    with _seex.init(root_path) as context_client:
        selected_project = context_client.get_project(project.project_id)

        assert selected_project.project_id == project.project_id


def test_context_manager_preserves_user_exception(
    tmp_path: pathlib.Path,
) -> None:

    with (
        pytest.raises(ValueError, match="user failure"),
        _seex.init(tmp_path / "seex"),
    ):
        raise ValueError("user failure")


def test_explicit_shutdown_drain_timeout_keeps_client_usable(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    root_path = tmp_path / "seex"
    client = _seex.init(root_path)
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    for step in range(1000):
        run.log("train/loss", step, float(step))

    with pytest.raises(seex.MetricDrainTimeoutError):
        client.shutdown(timeout=0.0)

    assert client.diagnostics().writer_state != "closed"
    second_client = _seex.init(root_path)
    with pytest.raises(seex.RunAlreadyActiveError):
        second_client.resume_run(run.run_id)
    run.log("train/loss", 1000, 0.125)
    finished = client.finish_run(run.run_id)

    assert finished.status == "finished"


def test_explicit_shutdown_drain_timeout_can_be_retried_unbounded(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = _seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    for step in range(1000):
        run.log("train/loss", step, float(step))

    with pytest.raises(seex.MetricDrainTimeoutError):
        client.shutdown(timeout=0.0)

    assert client.shutdown() is None
    with pytest.raises(_seex.ClientClosedError):
        run.log("train/loss", 1000, 0.125)


def test_diagnostics_fields_are_read_only(
    tmp_path: pathlib.Path,
) -> None:

    client = _seex.init(tmp_path / "seex")
    diagnostics = client.diagnostics()

    assert diagnostics.pending_reports == 0
    assert diagnostics.queue_full_errors == 0
    assert diagnostics.persisted_reports == 0
    assert diagnostics.writer_state == "drained"
    assert diagnostics.last_write_error is None
    assert diagnostics.last_flush_run_id is None
    assert diagnostics.last_flush_status == "none"
    assert diagnostics.last_flush_error is None
    for field in helpers.DIAGNOSTIC_FIELDS:
        assert hasattr(diagnostics, field)
    for removed_field in helpers.UNSUPPORTED_DIAGNOSTIC_FIELDS:
        assert not hasattr(diagnostics, removed_field)
    with pytest.raises(AttributeError):
        diagnostics.pending_reports = 1  # pyright: ignore[reportAttributeAccessIssue]
