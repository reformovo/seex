"""Tests for the installed-wheel bounded Reader acceptance contract."""

from __future__ import annotations

import pytest

from scripts import rc_lttb_validation


def _document(steps: list[int]) -> dict[str, object]:
    return {
        "data": [{"step": step} for step in steps],
        "meta": {
            "downsampled": True,
            "returned_row_count": len(steps),
            "source_row_count": 256,
        },
    }


def test_reader_acceptance_allows_a_result_below_the_strict_limit() -> None:
    rc_lttb_validation._assert_downsampled(_document([0, 64, 128, 192, 255]))


@pytest.mark.parametrize(
    "steps",
    (
        list(range(201)),
        [1, 64, 128, 255],
        [0, 64, 128, 254],
    ),
)
def test_reader_acceptance_rejects_limit_or_endpoint_violations(steps: list[int]) -> None:
    with pytest.raises(RuntimeError, match="bounded curve contract"):
        rc_lttb_validation._assert_downsampled(_document(steps))
