"""Smoke-test release Viewer startup in isolated configuration scopes."""

from __future__ import annotations

import argparse
import os
import pathlib
import subprocess
import tempfile
import time
from collections.abc import Sequence

_OBSERVATION_SECONDS = 1.0


def _write_schema(path: pathlib.Path, schema_version: int = 1) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"schema_version = {schema_version}\n", encoding="utf-8")


def _observe_startup(
    binary: pathlib.Path,
    arguments: Sequence[pathlib.Path],
    home: pathlib.Path,
    label: str,
) -> None:
    environment = os.environ.copy()
    environment["HOME"] = str(home)
    process = subprocess.Popen(
        [str(binary), *(str(argument) for argument in arguments)],
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        time.sleep(_OBSERVATION_SECONDS)
        return_code = process.poll()
        if return_code is not None:
            stdout, stderr = process.communicate()
            raise RuntimeError(
                f"{label} Viewer exited during startup with {return_code}: stdout={stdout!r}, stderr={stderr!r}"
            )
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


def run(binary: pathlib.Path) -> None:
    """Runs global, Project, and invalid-workbench startup observations."""
    if not binary.is_file():
        raise FileNotFoundError(f"release Viewer binary does not exist: {binary}")
    with tempfile.TemporaryDirectory(prefix="seex-viewer-smoke-") as temporary:
        root = pathlib.Path(temporary)

        global_home = root / "global-home"
        _write_schema(global_home / ".seex/config.toml")
        _observe_startup(binary, (), global_home, "global-scope")

        project_home = root / "project-home"
        project = root / "project"
        _write_schema(project_home / ".seex/config.toml")
        _write_schema(project / ".seex/config.toml")
        _observe_startup(binary, (project,), project_home, "project-scope")

        invalid_home = root / "invalid-home"
        _write_schema(invalid_home / ".seex/config.toml")
        _write_schema(invalid_home / ".seex/workbench.toml", schema_version=99)
        _observe_startup(binary, (), invalid_home, "invalid-workbench")


def main() -> None:
    """Parses the release binary path and runs the Viewer smoke."""
    root = pathlib.Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=pathlib.Path,
        default=root / "target/release/seex-app",
    )
    arguments = parser.parse_args()
    run(arguments.binary)
    print("verified release Viewer startup in global and Project scopes")


if __name__ == "__main__":
    main()
