"""Verify target U4 read-only Api discovery before public cutover."""

from __future__ import annotations

import pathlib

import pytest


def _finished_run(root: pathlib.Path) -> None:
    from seex import _seex

    run = _seex._start_run(project="project-1", dir=root, id="run-1", name="baseline")
    run.finish()


def test_api_discovers_projects_and_project_scoped_runs(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    _finished_run(tmp_path)
    api = _seex.Api(tmp_path)

    assert [project.project_id for project in api.projects()] == ["project-1"]
    assert api.project("project-1").name == "project-1"
    assert api.project("missing") is None
    assert [run.run_id for run in api.runs("project-1")] == ["run-1"]
    assert api.run("project-1/run-1").name == "baseline"
    assert api.run("other/run-1") is None
    assert api.run("project-1/missing") is None

    with pytest.raises(ValueError, match="project_id/run_id"):
        api.run("run-1")


def test_api_close_is_idempotent_and_invalidates_records(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    _finished_run(tmp_path)
    with _seex.Api(tmp_path) as api:
        record = api.run("project-1/run-1")
        assert record.run_id == "run-1"

    api.close()
    with pytest.raises(_seex.ApiClosedError):
        api.projects()
    with pytest.raises(_seex.ApiClosedError):
        _ = record.run_id
