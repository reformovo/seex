"""Verify SDK configuration reads never mutate app-owned documents."""

from __future__ import annotations

import pathlib

import pytest
import seex


def _fixture(path: str) -> bytes:
    return (pathlib.Path(__file__).parent / "fixtures" / path).read_bytes()


def _write(path: pathlib.Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)


def test_sdk_lifecycle_leaves_config_and_workbench_bytes_unchanged(
    tmp_path: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    home = tmp_path / "home"
    root = tmp_path / "project"
    global_config = home / ".seex" / "config.toml"
    global_workbench = home / ".seex" / "workbench.toml"
    project_config = root / ".seex" / "config.toml"
    project_workbench = root / ".seex" / "workbench.toml"
    _write(global_config, _fixture("config/v1-global.toml"))
    global_config.chmod(0o600)
    _write(global_workbench, _fixture("workbench/v1.toml"))
    _write(project_config, _fixture("config/v1-project.toml"))
    _write(project_workbench, _fixture("workbench/v1.toml"))
    documents = [global_config, global_workbench, project_config, project_workbench]
    before = {path: path.read_bytes() for path in documents}
    monkeypatch.setenv("HOME", str(home))

    with seex.init(project="project-1", dir=root, id="run-1", name="run") as run:
        run.log({"loss": 1.0}, step=0)

    assert {path: path.read_bytes() for path in documents} == before
