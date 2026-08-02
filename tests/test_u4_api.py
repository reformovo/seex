"""Verify target U4 read-only Api discovery before public cutover."""

from __future__ import annotations

import pathlib

import pytest


def _finished_run(root: pathlib.Path) -> None:
    from seex import _seex

    run = _seex._start_run(project="project-1", dir=root, id="run-1", name="baseline")
    run.log({"loss": 0.25})
    run.finish()


def test_api_discovers_projects_and_project_scoped_runs(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    _finished_run(tmp_path)
    api = _seex.Api(tmp_path)

    assert [project.project_id for project in api.projects()] == ["project-1"]
    project = api.project("project-1")
    assert project is not None
    assert project.name == "project-1"
    assert api.project("missing") is None
    assert [run.run_id for run in api.runs("project-1")] == ["run-1"]
    selected = api.run("project-1/run-1")
    assert selected is not None
    assert selected.name == "baseline"
    assert api.run("other/run-1") is None
    assert api.run("project-1/missing") is None
    record = api.run("project-1/run-1")
    assert record is not None
    assert [metric.metric_key for metric in record.metrics()] == ["loss"]
    summary = record.metric_summary("loss")
    assert summary is not None
    assert summary.last_value_f64 == 0.25
    assert record.metric_summary("missing") is None

    with pytest.raises(ValueError, match="project_id/run_id"):
        api.run("run-1")


def test_api_close_is_idempotent_and_invalidates_records(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    _finished_run(tmp_path)
    record: _seex.RunRecord | None = None
    with _seex.Api(tmp_path) as api:
        record = api.run("project-1/run-1")
        assert record is not None
        assert record.run_id == "run-1"

    api.close()
    assert record is not None
    with pytest.raises(_seex.ApiClosedError):
        api.projects()
    with pytest.raises(_seex.ApiClosedError):
        _ = record.run_id


def test_api_compares_and_ranks_run_evidence(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    for run_id, loss in [("candidate", 0.25), ("reference", 0.5)]:
        run = _seex._start_run(project="project-1", dir=tmp_path, id=run_id)
        run.log({"loss": loss})
        run.finish()
    api = _seex.Api(tmp_path)

    comparison = api.compare_runs(
        "candidate",
        "reference",
        metric_key="loss",
        direction="minimize",
    )
    ranking = api.rank_runs(
        ["reference", "candidate"],
        metric_key="loss",
        direction="minimize",
    )

    assert comparison.preference == "candidate"
    assert comparison.candidate.last_value_f64 == 0.25
    assert [entry.evidence.run_id for entry in ranking.entries] == ["candidate", "reference"]
    assert [entry.rank for entry in ranking.entries] == [1, 2]
