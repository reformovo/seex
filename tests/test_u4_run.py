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


def test_target_run_context_finishes_or_fails(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    finished_path = tmp_path / "finished"
    with _seex._start_run(dir=finished_path, id="finished") as finished:
        assert finished.status == "running"

    failed_path = tmp_path / "failed"
    with pytest.raises(RuntimeError, match="training failed"):
        with _seex._start_run(dir=failed_path, id="failed"):
            raise RuntimeError("training failed")

    finished_reader = _seex.init(finished_path, _must_exist=True)
    failed_reader = _seex.init(failed_path, _must_exist=True)
    assert finished_reader.get_run("finished").status == "finished"
    assert failed_reader.get_run("failed").status == "failed"


def test_target_run_retries_matching_outcome_and_rejects_conflict(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    run = _seex._start_run(dir=tmp_path, id="run-1")
    run.finish()
    run.finish(0)

    with pytest.raises(_seex.InvalidRunStateError, match="conflicts"):
        run.finish(1)
