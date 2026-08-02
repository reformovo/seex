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
    with (
        pytest.raises(RuntimeError, match="training failed"),
        _seex._start_run(dir=failed_path, id="failed"),
    ):
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


def test_target_run_logs_atomic_numeric_mappings(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    run = _seex._start_run(dir=tmp_path, id="run-1")
    run.log({"loss": 2, "accuracy": 0.5})
    run.log({"loss": 1.0})
    run.finish()

    reader = _seex.init(tmp_path, _must_exist=True)
    assert [(point.step, point.value_f64) for point in reader.query_metric("run-1", "loss")] == [
        (0, 2.0),
        (1, 1.0),
    ]
    assert run.diagnostics().persisted_reports == 3


def test_target_run_applies_explicit_step_and_commit_cursor_rules(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    run = _seex._start_run(dir=tmp_path, id="run-1")
    run.log({"loss": 9.0}, step=5)
    run.log({"loss": 0.0})
    run.log({"loss": 4.0}, step=4, commit=True)
    run.log({"loss": 5.0})
    run.finish()

    reader = _seex.init(tmp_path, _must_exist=True)
    points = reader.query_metric("run-1", "loss")
    assert [(point.step, point.value_f64) for point in points] == [
        (0, 0.0),
        (4, 4.0),
        (5, 5.0),
    ]


def test_target_run_rejects_oversized_mapping_atomically(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    run = _seex._start_run(dir=tmp_path, id="run-1")
    with pytest.raises(ValueError, match="maximum is 8192"):
        run.log({f"metric-{index}": float(index) for index in range(8_193)})
    run.log({"accepted": 1.0})
    run.finish()

    reader = _seex.init(tmp_path, _must_exist=True)
    assert reader.list_metrics("run-1")[0].metric_key == "accepted"


def test_finished_run_releases_native_resources_before_python_drop(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    first = _seex._start_run(project="project-1", dir=tmp_path, id="first")
    first.finish()
    second = _seex._start_run(project="project-1", dir=tmp_path, id="second")
    second.finish()

    api = _seex.Api(tmp_path)
    assert [run.run_id for run in api.runs("project-1")] == ["first", "second"]
    assert first.status == "finished"
    assert first.diagnostics().writer_state == "closed"


@pytest.mark.parametrize(
    ("data", "error_type"),
    [
        ([], TypeError),
        ({}, ValueError),
        ({"": 1.0}, ValueError),
        ({"loss": True}, TypeError),
        ({"loss": "bad"}, TypeError),
        ({1: 1.0}, TypeError),
    ],
)
def test_target_run_rejects_invalid_metric_mappings(
    tmp_path: pathlib.Path,
    data: object,
    error_type: type[Exception],
) -> None:
    from seex import _seex

    run = _seex._start_run(dir=tmp_path)
    with pytest.raises(error_type, match="Mapping"):
        run.log(data)
