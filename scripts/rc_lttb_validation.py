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
        environment["SEEX_LTTB_AUTO_INSTALL"] = "1"

        online_root = root / "online"
        _seed_store(online_root)
        _assert_downsampled(_query_cli(online_root, environment))
        extension_paths = _find_extensions(duckdb_home, pathlib.Path.home() / ".duckdb")

        offline_root = root / "offline"
        _seed_store(offline_root)
        _assert_downsampled(_query_cli_with_local_extension(offline_root, environment, extension_paths))

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
        raise RuntimeError("LTTB query did not preserve the bounded curve contract")


def _find_extensions(*roots: pathlib.Path) -> list[pathlib.Path]:
    candidates: set[pathlib.Path] = set()
    for root in roots:
        candidates.update(root.rglob("lttb.duckdb_extension"))
    if not candidates:
        raise RuntimeError("online LTTB install did not produce a local extension")
    return sorted(candidates, key=lambda path: path.stat().st_mtime_ns, reverse=True)


def _query_cli_with_local_extension(
    project_root: pathlib.Path,
    environment: dict[str, str],
    extension_paths: list[pathlib.Path],
) -> dict[str, object]:
    offline_environment = environment.copy()
    offline_environment.pop("SEEX_LTTB_AUTO_INSTALL", None)
    last_error: subprocess.CalledProcessError | None = None
    for extension_path in extension_paths:
        offline_environment["SEEX_LTTB_EXTENSION_PATH"] = str(extension_path)
        try:
            return _query_cli(project_root, offline_environment)
        except subprocess.CalledProcessError as error:
            last_error = error
    message = f"none of {len(extension_paths)} local LTTB extensions could be loaded"
    if last_error is not None and last_error.stderr:
        message = f"{message}; last failure: {last_error.stderr.strip()}"
    raise RuntimeError(message) from last_error


if __name__ == "__main__":
    main()
