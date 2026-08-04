"""Validate tag, package versions, and isolated release artifact sets."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
from collections.abc import Mapping
from collections.abc import Set as AbstractSet

_CARGO_PACKAGES = ("seex", "seex-python", "seex-app", "seex-plot")
_VIEWER_FILES = frozenset({"seex-app-macos-aarch64", "seex-app-macos-aarch64.sha256"})


def validate_release(
    tag: str,
    packages: Mapping[str, str],
    python_files: AbstractSet[str],
    viewer_files: AbstractSet[str],
) -> None:
    """Validates one release's source identity and artifact channel boundary."""
    missing = [name for name in _CARGO_PACKAGES if name not in packages]
    if missing:
        raise ValueError(f"Cargo metadata is missing packages: {missing}")
    cargo_version = packages["seex"]
    mismatched = [name for name in _CARGO_PACKAGES if packages[name] != cargo_version]
    if mismatched:
        raise ValueError(f"Cargo package versions do not match seex: {mismatched}")
    if tag != f"v{cargo_version}":
        raise ValueError(f"tag {tag!r} does not match Cargo version {cargo_version!r}")
    match = re.fullmatch(r"(\d+\.\d+\.\d+)-beta\.(\d+)", cargo_version)
    if match is None:
        raise ValueError(f"Cargo version is not a beta release: {cargo_version!r}")
    python_version = f"{match.group(1)}b{match.group(2)}"
    invalid_python = sorted(
        name
        for name in python_files
        if not (name.startswith(f"seex-{python_version}-") and name.endswith(".whl"))
        and name != f"seex-{python_version}.tar.gz"
    )
    if not python_files or invalid_python:
        raise ValueError(f"invalid Python release artifacts: {invalid_python}")
    if viewer_files != _VIEWER_FILES:
        raise ValueError(f"invalid Viewer release artifacts: {sorted(viewer_files)}")


def _artifact_names(path: pathlib.Path) -> set[str]:
    return {item.name for item in path.rglob("*") if item.is_file()}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--python-artifacts", type=pathlib.Path, required=True)
    parser.add_argument("--viewer-artifacts", type=pathlib.Path, required=True)
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[1]
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            cwd=root,
            text=True,
        )
    )
    packages = {item["name"]: item["version"] for item in metadata["packages"]}
    validate_release(
        args.tag,
        packages,
        _artifact_names(args.python_artifacts),
        _artifact_names(args.viewer_artifacts),
    )


if __name__ == "__main__":
    main()
