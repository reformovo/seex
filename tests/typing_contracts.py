"""Static type contracts for the public Python API."""

from __future__ import annotations

import datetime as dt
import pathlib
from typing import Literal, assert_type

import seex


def check_run(settings: seex.Settings, root: pathlib.Path) -> None:
    run = seex.init(
        project="project-1",
        dir=root,
        id="run-1",
        name="baseline",
        resume="allow",
        settings=settings,
    )
    assert_type(run, seex.Run)
    assert_type(run.status, Literal["running", "finished", "failed"])
    run.log({"loss": 1.0, "samples": 10}, step=1, commit=True)
    run.finish()


def check_read_api(api: seex.Api, record: seex.RunRecord) -> None:
    assert_type(api.projects(), list[seex.Project])
    assert_type(api.project("project-1"), seex.Project | None)
    assert_type(api.runs("project-1"), list[seex.RunRecord])
    assert_type(api.run("project-1/run-1"), seex.RunRecord | None)
    assert_type(record.metrics(), list[seex.MetricSummary])
    assert_type(record.metric_summary("loss"), seex.MetricSummary | None)

    series = record.history(
        "loss",
        x_axis="timestamp",
        start=dt.datetime.now(dt.UTC),
        max_points=200,
    )
    assert_type(series, seex.MetricSeries)
    assert_type(series.points, list[seex.MetricPoint])
    assert_type(series.source_count, int)
    assert_type(series.downsampled, bool)
    assert_type(series.__arrow_c_stream__(), object)


def check_analysis(api: seex.Api) -> None:
    comparison = api.compare_runs(
        "candidate",
        "reference",
        metric_key="loss",
        direction="minimize",
    )
    ranking = api.rank_runs(
        ["run-1", "run-2"],
        metric_key="accuracy",
        direction="maximize",
    )
    assert_type(comparison, seex.ComparisonResult)
    assert_type(comparison.objective, seex.ObjectiveMetric)
    assert_type(comparison.candidate, seex.ObjectiveEvidence)
    assert_type(ranking, seex.RankingResult)
    assert_type(ranking.entries, list[seex.RankingEntry])


def check_rejected_calls(api: seex.Api, record: seex.RunRecord) -> None:
    record.history("loss", x_axis="epoch")  # type: ignore[reportArgumentType]
    api.compare_runs(
        "candidate",
        "reference",
        metric_key="loss",
        direction="lower",  # type: ignore[reportArgumentType]
    )


def check_rejected_mutation(
    settings: seex.Settings,
    run: seex.Run,
    record: seex.RunRecord,
    point: seex.MetricPoint,
    summary: seex.MetricSummary,
    project: seex.Project,
) -> None:
    settings.catalog_backend = "sqlite"  # type: ignore[reportAttributeAccessIssue]
    run.run_id = "other"  # type: ignore[reportAttributeAccessIssue]
    record.status = "failed"  # type: ignore[reportAttributeAccessIssue]
    point.step = 2  # type: ignore[reportAttributeAccessIssue]
    summary.effective_count = 0  # type: ignore[reportAttributeAccessIssue]
    project.name = "other"  # type: ignore[reportAttributeAccessIssue]
