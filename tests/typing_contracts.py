"""Static type contracts for the public Python read APIs."""

from __future__ import annotations

from typing import Literal, assert_type

import seex


def check_arrow_table_queries(client: seex.Client) -> None:
    points = client.query_metric_table(
        "run-1",
        "train/loss",
        start_step=10,
        end_step=20,
        max_points=5,
    )
    summaries = client.query_metric_summaries_table(["run-1", "run-2"], "train/loss")

    assert_type(points, seex.ArrowTable)
    assert_type(summaries, seex.ArrowTable)
    assert_type(points.row_count, int)
    assert_type(points.source_row_count, int)
    assert_type(points.downsampled, bool)
    assert_type(points.column_names, list[str])
    assert_type(points.__arrow_c_stream__(), object)


def check_comparison_reads(client: seex.Client) -> None:
    aligned = client.query_aligned_metric(
        "run-1",
        "loss",
        axis="elapsed_time",
        start=0,
        end=10_000,
        pixel_width=800,
        points_per_pixel=2,
    )
    comparison = client.compare_runs(
        "candidate",
        "reference",
        metric_key="loss",
        direction="minimize",
    )
    ranking = client.rank_runs(
        ["run-1", "run-2"],
        metric_key="accuracy",
        direction="maximize",
    )

    assert_type(aligned, seex.AlignedMetricResult)
    assert_type(aligned.points, list[seex.AlignedMetricPoint])
    assert_type(aligned.points[0].axis_value, int)
    assert_type(
        aligned.completeness,
        Literal["complete", "partial", "unavailable", "invalid"],
    )
    assert_type(aligned.reasons, list[str])
    assert_type(comparison, seex.ComparisonResult)
    assert_type(comparison.objective, seex.ObjectiveMetric)
    assert_type(comparison.candidate, seex.ObjectiveEvidence)
    assert_type(comparison.raw_delta, float | None)
    assert_type(
        comparison.outcome,
        Literal["improved", "regressed", "equal"] | None,
    )
    assert_type(ranking, seex.RankingResult)
    assert_type(ranking.entries, list[seex.RankingEntry])
    assert_type(ranking.entries[0].evidence, seex.ObjectiveEvidence)
    assert_type(ranking.entries[0].rank, int | None)


def check_rejected_comparison_calls(client: seex.Client) -> None:
    client.query_aligned_metric(
        "run-1",
        "loss",
        axis="epoch",  # type: ignore[reportArgumentType]
        start=0,
        end=1,
    )
    result = client.compare_runs(
        "candidate",
        "reference",
        metric_key="loss",
        direction="lower",  # type: ignore[reportArgumentType]
    )
    result.preference = "candidate"  # type: ignore[reportAttributeAccessIssue]
