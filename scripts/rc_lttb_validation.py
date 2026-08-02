"""Validate online and offline LTTB paths from an installed wheel."""

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

        online_root = root / "online"
        _seed_store(online_root)
        _assert_downsampled(_query_cli(online_root, environment))
        extension_path = _find_extension(duckdb_home)

        offline_root = root / "offline"
        _seed_store(offline_root)
        environment["SEEX_LTTB_EXTENSION_PATH"] = str(extension_path)
        _assert_downsampled(_query_cli(offline_root, environment))

    print("validated online and explicit offline LTTB paths")


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
        raise TypeError("LTTB query did not return structured metric points")
    steps = [row["step"] for row in data if isinstance(row, dict)]
    if (
        meta.get("source_row_count") != _POINT_COUNT
        or meta.get("returned_row_count") != 200
        or meta.get("downsampled") is not True
        or steps[0] != 0
        or steps[-1] != _POINT_COUNT - 1
    ):
        raise RuntimeError("LTTB query did not preserve the expected curve endpoints")


def _find_extension(duckdb_home: pathlib.Path) -> pathlib.Path:
    candidates = list(duckdb_home.rglob("lttb.duckdb_extension"))
    if not candidates:
        raise RuntimeError("online LTTB install did not produce a local extension")
    return max(candidates, key=lambda path: path.stat().st_mtime_ns)


if __name__ == "__main__":
    main()
