"""Validate bounded Reader behavior from an installed wheel.

Private online and offline LTTB loading is covered by the Rust storage
acceptance test at the boundary that owns extension policy.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import typing

import seex

_POINT_COUNT = 256


def main() -> None:
    """Checks CLI online installation and explicit local-extension loading."""
    with tempfile.TemporaryDirectory(prefix="seex-rc-lttb-") as directory:
        root = pathlib.Path(directory)
        duckdb_home = root / "duckdb-home"
        duckdb_home.mkdir()
        environment = os.environ.copy()
        environment["HOME"] = str(duckdb_home)
        environment.pop("SEEX_LTTB_EXTENSION_PATH", None)
        environment.pop("SEEX_LTTB_AUTO_INSTALL", None)

        project_root = root / "project"
        _seed_store(project_root)
        _assert_downsampled(_query_cli(project_root, environment))

        if list(duckdb_home.rglob("lttb.duckdb_extension")):
            raise RuntimeError("bounded Reader query unexpectedly installed LTTB")

    print("validated installed-wheel bounded Reader path")


def _seed_store(project_root: pathlib.Path) -> None:
    with seex.init(project="project-1", dir=project_root, id="run-1", name="curve") as run:
        for step in range(_POINT_COUNT):
            run.log({"train/loss": float(step % 17)}, step=step)


def _query_cli(project_root: pathlib.Path, environment: dict[str, str]) -> dict[str, object]:
    completed = subprocess.run(
        [
            sys.executable,
            "-m",
            "seex.cli",
            "--path",
            str(project_root),
            "--format",
            "json",
            "metrics",
            "query",
            "run-1",
            "train/loss",
        ],
        check=True,
        capture_output=True,
        text=True,
        env=environment,
    )
    return typing.cast(dict[str, object], json.loads(completed.stdout))


def _assert_downsampled(document: dict[str, object]) -> None:
    meta = document.get("meta")
    data = document.get("data")
    if not isinstance(meta, dict) or not isinstance(data, list):
        raise TypeError("bounded Reader query did not return structured metric points")
    steps = [row["step"] for row in data if isinstance(row, dict)]
    returned_row_count = meta.get("returned_row_count")
    if (
        meta.get("source_row_count") != _POINT_COUNT
        or not isinstance(returned_row_count, int)
        or not 2 <= returned_row_count <= 200
        or returned_row_count != len(data)
        or len(steps) != returned_row_count
        or meta.get("downsampled") is not True
        or steps[0] != 0
        or steps[-1] != _POINT_COUNT - 1
    ):
        raise RuntimeError("Reader query did not preserve the bounded curve contract")


if __name__ == "__main__":
    main()
