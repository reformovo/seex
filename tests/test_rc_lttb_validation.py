"""Tests for the installed-wheel LTTB acceptance contract."""

from __future__ import annotations

import pathlib

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


def test_lttb_acceptance_allows_a_result_below_the_strict_limit() -> None:
    rc_lttb_validation._assert_downsampled(_document([0, 64, 128, 192, 255]))


@pytest.mark.parametrize(
    "steps",
    (
        list(range(201)),
        [1, 64, 128, 255],
        [0, 64, 128, 254],
    ),
)
def test_lttb_acceptance_rejects_limit_or_endpoint_violations(steps: list[int]) -> None:
    with pytest.raises(RuntimeError, match="bounded curve contract"):
        rc_lttb_validation._assert_downsampled(_document(steps))


def test_lttb_acceptance_finds_an_extension_in_any_duckdb_home(tmp_path: pathlib.Path) -> None:
    isolated_home = tmp_path / "isolated"
    actual_home = tmp_path / "actual"
    extension = actual_home / "extensions/v1/platform/lttb.duckdb_extension"
    extension.parent.mkdir(parents=True)
    extension.write_bytes(b"extension")

    assert rc_lttb_validation._find_extension(isolated_home, actual_home) == extension
