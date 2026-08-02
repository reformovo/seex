"""Verify typed alignment, comparison, and ranking Python APIs."""

from __future__ import annotations

import math
import pathlib

import pytest
import seex
from seex import _seex

from tests import helpers


def _create_objective_run(
    client: _seex.Client,
    project_id: str,
    run_id: str,
    value: float | None,
    *,
    terminal: str | None = "finished",
) -> _seex.Run:
    run = client.create_run(project_id, run_id, run_id=run_id)
    if value is not None:
        run.log("loss", 1, value)
        helpers.wait_for_metric_points(client, run_id, "loss", expected_count=1)
    if terminal == "finished":
        client.finish_run(run_id)
    elif terminal == "failed":
        client.fail_run(run_id)
    return run


def test_aligned_metric_queries_step_and_elapsed_evidence(
    tmp_path: pathlib.Path,
) -> None:
    client = _seex.init(tmp_path / "seex")
    project = client.create_project("alignment", project_id="project-1")
    run = client.create_run(project.project_id, "curve", run_id="run-1")
    for step, value in enumerate([4.0, 3.0, 2.0, 1.0]):
        run.log("loss", step, value)
    helpers.wait_for_metric_points(client, run.run_id, "loss", expected_count=4)

    step_result = client.query_aligned_metric(
        run.run_id,
        "loss",
        axis="step",
        start=1,
        end=2,
    )
    elapsed_result = client.query_aligned_metric(
        run.run_id,
        "loss",
        axis="elapsed_time",
        start=0,
        end=10_000,
        pixel_width=1,
        points_per_pixel=1,
    )

    assert isinstance(step_result, _seex.AlignedMetricResult)
    assert all(isinstance(point, _seex.AlignedMetricPoint) for point in step_result.points)
    assert [point.axis_value for point in step_result.points] == [0, 1, 2, 3]
    assert step_result.completeness == "partial"
    assert step_result.reasons == ["run_running"]
    assert elapsed_result.points[0].axis_value >= 0
    assert elapsed_result.source_row_count == 4
    with pytest.raises(AttributeError):
        step_result.completeness = "complete"  # type: ignore[reportAttributeAccessIssue]


def test_aligned_metric_rejects_invalid_public_arguments(
    tmp_path: pathlib.Path,
) -> None:
    client = _seex.init(tmp_path / "seex")

    with pytest.raises(ValueError, match="axis must be"):
        client.query_aligned_metric(
            "run-1",
            "loss",
            axis="epoch",  # type: ignore[reportArgumentType]
            start=0,
            end=1,
        )
    with pytest.raises(ValueError, match="provided together"):
        client.query_aligned_metric("run-1", "loss", axis="step", start=0, end=1, pixel_width=100)
    with pytest.raises(ValueError, match="non-decreasing"):
        client.query_aligned_metric("run-1", "loss", axis="step", start=2, end=1)


def test_aligned_metric_marks_non_finite_evidence_invalid(
    tmp_path: pathlib.Path,
) -> None:
    client = _seex.init(tmp_path / "seex")
    project = client.create_project("alignment", project_id="project-1")
    run = client.create_run(project.project_id, "curve", run_id="run-1")
    run.log("loss", 0, float("nan"))
    helpers.wait_for_metric_points(client, run.run_id, "loss", expected_count=1)
    client.finish_run(run.run_id)

    result = client.query_aligned_metric(
        run.run_id,
        "loss",
        axis="step",
        start=0,
        end=0,
    )

    assert result.completeness == "invalid"
    assert result.reasons == ["non_finite_value"]
    assert math.isnan(result.points[0].value_f64)


def test_compare_runs_reports_complete_partial_and_unavailable_evidence(
    tmp_path: pathlib.Path,
) -> None:
    client = _seex.init(tmp_path / "seex")
    project = client.create_project("comparison", project_id="project-1")
    reference = _create_objective_run(client, project.project_id, "reference", 0.0)
    candidate = _create_objective_run(client, project.project_id, "candidate", 1.0)
    running = _create_objective_run(client, project.project_id, "running", 2.0, terminal=None)
    missing = _create_objective_run(client, project.project_id, "missing", None)

    complete = client.compare_runs(
        candidate.run_id,
        reference.run_id,
        metric_key="loss",
        direction="maximize",
    )
    partial = client.compare_runs(
        running.run_id,
        candidate.run_id,
        metric_key="loss",
        direction="maximize",
    )
    unavailable = client.compare_runs(
        missing.run_id,
        candidate.run_id,
        metric_key="loss",
        direction="maximize",
    )

    assert complete.raw_delta == 1.0
    assert complete.relative_delta is None
    assert complete.outcome == "improved"
    assert complete.preference == "candidate"
    assert partial.outcome == "improved"
    assert partial.preference == "inconclusive"
    assert partial.candidate.reasons == ["run_running"]
    assert unavailable.outcome is None
    assert unavailable.completeness == "unavailable"
    assert unavailable.candidate.reasons == ["missing_metric"]


def test_rank_runs_keeps_ineligible_entries_and_rejects_duplicates(
    tmp_path: pathlib.Path,
) -> None:
    client = _seex.init(tmp_path / "seex")
    project = client.create_project("ranking", project_id="project-1")
    tied_a = _create_objective_run(client, project.project_id, "tied-a", 1.0)
    tied_b = _create_objective_run(client, project.project_id, "tied-b", 1.0)
    worse = _create_objective_run(client, project.project_id, "worse", 2.0)
    failed = _create_objective_run(client, project.project_id, "failed", 0.5, terminal="failed")

    result = client.rank_runs(
        [worse.run_id, failed.run_id, tied_b.run_id, tied_a.run_id],
        metric_key="loss",
        direction="minimize",
    )

    assert [entry.evidence.run_id for entry in result.entries] == [
        "tied-a",
        "tied-b",
        "worse",
        "failed",
    ]
    assert [entry.rank for entry in result.entries] == [1, 1, 3, None]
    assert result.entries[-1].evidence.reasons == ["run_failed"]
    with pytest.raises(seex.SeexError, match="duplicate run identity"):
        client.rank_runs(
            [tied_a.run_id, tied_a.run_id],
            metric_key="loss",
            direction="minimize",
        )
    with pytest.raises(seex.StorageError, match="run not found: unknown"):
        client.rank_runs(
            [tied_a.run_id, "unknown"],
            metric_key="loss",
            direction="minimize",
        )
