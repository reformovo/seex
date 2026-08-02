"""Verify target U4 typed metric history before public cutover."""

from __future__ import annotations

import pathlib
from typing import Any

import pytest


def _record(root: pathlib.Path) -> Any:
    from seex import _seex

    run = _seex._start_run(project="project-1", dir=root, id="run-1")
    for step in range(5):
        run.log({"loss": float(step)}, step=step, commit=True)
    run.finish()
    return _seex.Api(root).run("project-1/run-1")


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


@pytest.mark.parametrize("value", [True, 1.5, "1"])
def test_step_history_rejects_mismatched_bounds(tmp_path: pathlib.Path, value: object) -> None:
    record = _record(tmp_path)
    with pytest.raises(TypeError, match="step bounds"):
        record.history("loss", start=value)


def test_history_rejects_invalid_range_and_point_limit(tmp_path: pathlib.Path) -> None:
    record = _record(tmp_path)
    with pytest.raises(ValueError, match="non-empty"):
        record.history("loss", start=2, end=2)
    with pytest.raises(ValueError, match="at least 2"):
        record.history("loss", max_points=1)
    with pytest.raises(TypeError, match="max_points"):
        record.history("loss", max_points=True)
