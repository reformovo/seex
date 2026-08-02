"""Verify target U4 Run initialization before the public API cutover."""

from __future__ import annotations

import pathlib

import pytest


def test_target_run_uses_explicit_and_default_identities(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    explicit = _seex._start_run(project="project-1", dir=tmp_path, id="run-1", name="baseline")
    generated = _seex._start_run(project="project-2", dir=tmp_path)

    assert (explicit.project_id, explicit.run_id, explicit.name, explicit.status) == (
        "project-1",
        "run-1",
        "baseline",
        "running",
    )
    assert generated.project_id == "project-2"
    assert generated.run_id
    assert generated.name == generated.run_id


def test_target_run_validates_resume_contract(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    with pytest.raises(ValueError, match="resume"):
        _seex._start_run(dir=tmp_path, resume="auto")
    with pytest.raises(TypeError, match="resume"):
        _seex._start_run(dir=tmp_path, resume=1)
    with pytest.raises(ValueError, match="`id`"):
        _seex._start_run(dir=tmp_path, resume="must")
