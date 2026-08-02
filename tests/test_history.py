"""Verify public typed metric history."""

from __future__ import annotations

import datetime
import pathlib

import pytest
import seex


def _record(root: pathlib.Path) -> seex.RunRecord:
    run = seex.init(project="project-1", dir=root, id="run-1")
    for step in range(5):
        run.log({"loss": float(step)}, step=step, commit=True)
    run.finish()
    record = seex.Api(root).run("project-1/run-1")
    assert record is not None
    return record


def test_step_history_supports_full_and_one_sided_ranges(tmp_path: pathlib.Path) -> None:
    record = _record(tmp_path)

    full = record.history("loss")
    tail = record.history("loss", start=3)
    head = record.history("loss", end=2)

    assert full.axis == "step"
    assert [point.step for point in full.points] == [0, 1, 2, 3, 4]
    assert [point.step for point in tail.points] == [3, 4]
    assert [point.step for point in head.points] == [0, 1]
    assert full.source_count == 5
    assert full.downsampled is False
    assert full.completeness == "complete"
    assert full.reasons == []
    assert '"arrow_array_stream"' in repr(full.__arrow_c_stream__())
    assert '"arrow_array_stream"' in repr(full.__arrow_c_stream__(requested_schema=None))

    empty = record.history("missing")
    assert empty.points == []
    assert '"arrow_array_stream"' in repr(empty.__arrow_c_stream__())


@pytest.mark.parametrize("value", [True, 1.5, "1"])
def test_step_history_rejects_mismatched_bounds(tmp_path: pathlib.Path, value: object) -> None:
    record = _record(tmp_path)
    with pytest.raises(TypeError, match="step bounds"):
        record.history("loss", start=value)  # type: ignore[reportArgumentType]


def test_history_rejects_invalid_range_and_point_limit(tmp_path: pathlib.Path) -> None:
    record = _record(tmp_path)
    with pytest.raises(ValueError, match="non-empty"):
        record.history("loss", start=2, end=2)
    for max_points in (-1, 0, 1):
        with pytest.raises(ValueError, match="at least 2"):
            record.history("loss", max_points=max_points)
    with pytest.raises(ValueError, match="out of range"):
        record.history("loss", max_points=2**128)
    with pytest.raises(TypeError, match="max_points"):
        record.history("loss", max_points=True)


def test_time_history_accepts_typed_utc_millisecond_ranges(tmp_path: pathlib.Path) -> None:
    record = _record(tmp_path)
    started_at = datetime.datetime.fromisoformat(record.started_at)
    started_at = started_at.replace(microsecond=started_at.microsecond // 1_000 * 1_000)

    relative = record.history(
        "loss",
        x_axis="relative_time",
        end=datetime.timedelta(hours=1),
    )
    timestamp = record.history(
        "loss",
        x_axis="timestamp",
        start=started_at,
    )

    assert relative.axis == "relative_time"
    assert timestamp.axis == "timestamp"
    assert [point.step for point in relative.points] == [0, 1, 2, 3, 4]
    assert [point.step for point in timestamp.points] == [0, 1, 2, 3, 4]


def test_time_history_rejects_mismatched_naive_and_submillisecond_bounds(
    tmp_path: pathlib.Path,
) -> None:
    record = _record(tmp_path)
    with pytest.raises(TypeError, match="timedelta"):
        record.history("loss", x_axis="relative_time", start=0)
    with pytest.raises(TypeError, match="datetime"):
        record.history("loss", x_axis="timestamp", start=datetime.timedelta(0))
    with pytest.raises(ValueError, match="x_axis"):
        record.history("loss", x_axis="loss")  # type: ignore[reportArgumentType]
    with pytest.raises(ValueError, match="millisecond precision"):
        record.history("loss", x_axis="relative_time", start=datetime.timedelta(microseconds=1))
    with pytest.raises(ValueError, match="timezone-aware"):
        record.history(
            "loss",
            x_axis="timestamp",
            start=datetime.datetime.now(),  # noqa: DTZ005 - deliberate naive bound
        )
    with pytest.raises(ValueError, match="millisecond precision"):
        record.history(
            "loss",
            x_axis="timestamp",
            start=datetime.datetime.now(datetime.UTC).replace(microsecond=1),
        )
