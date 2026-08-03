"""Tests for the installed-wheel LTTB acceptance contract."""

from __future__ import annotations

import os
import pathlib
import subprocess

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


def test_lttb_acceptance_finds_extensions_in_any_duckdb_home(tmp_path: pathlib.Path) -> None:
    isolated_home = tmp_path / "isolated"
    actual_home = tmp_path / "actual"
    isolated_extension = isolated_home / "extensions/v1/platform/lttb.duckdb_extension"
    actual_extension = actual_home / "extensions/v2/platform/lttb.duckdb_extension"
    isolated_extension.parent.mkdir(parents=True)
    actual_extension.parent.mkdir(parents=True)
    isolated_extension.write_bytes(b"isolated")
    actual_extension.write_bytes(b"actual")

    assert set(rc_lttb_validation._find_extensions(isolated_home, actual_home)) == {
        isolated_extension,
        actual_extension,
    }


def test_lttb_acceptance_probes_past_a_newer_incompatible_extension(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    current = tmp_path / "v1.10504.0/osx_arm64/lttb.duckdb_extension"
    incompatible = tmp_path / "v1.4.3/osx_arm64/lttb.duckdb_extension"
    current.parent.mkdir(parents=True)
    incompatible.parent.mkdir(parents=True)
    current.write_bytes(b"current")
    incompatible.write_bytes(b"incompatible")
    current_mtime = current.stat().st_mtime_ns
    os.utime(incompatible, ns=(current_mtime + 1_000_000, current_mtime + 1_000_000))
    attempts: list[pathlib.Path] = []

    def query_cli(project_root: pathlib.Path, environment: dict[str, str]) -> dict[str, object]:
        del project_root
        assert "SEEX_LTTB_AUTO_INSTALL" not in environment
        extension = pathlib.Path(environment["SEEX_LTTB_EXTENSION_PATH"])
        attempts.append(extension)
        if extension == incompatible:
            raise subprocess.CalledProcessError(1, ["seex", "metrics", "query"], stderr="incompatible extension")
        return _document([0, 64, 128, 192, 255])

    monkeypatch.setattr(rc_lttb_validation, "_query_cli", query_cli)

    result = rc_lttb_validation._query_cli_with_local_extension(
        tmp_path / "project",
        {"SEEX_LTTB_AUTO_INSTALL": "1"},
        rc_lttb_validation._find_extensions(tmp_path),
    )

    assert result == _document([0, 64, 128, 192, 255])
    assert attempts == [incompatible, current]
