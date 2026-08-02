"""Verify public Run initialization and lifecycle behavior."""

from __future__ import annotations

import decimal
import fractions
import pathlib

import pytest
import seex


def test_run_uses_explicit_and_default_identities(tmp_path: pathlib.Path) -> None:

    explicit = seex.init(project="project-1", dir=tmp_path, id="run-1", name="baseline")
    generated = seex.init(project="project-2", dir=tmp_path)

    assert (explicit.project_id, explicit.run_id, explicit.name, explicit.status) == (
        "project-1",
        "run-1",
        "baseline",
        "running",
    )
    assert generated.project_id == "project-2"
    assert generated.run_id
    assert generated.name == generated.run_id


def test_run_validates_resume_contract(tmp_path: pathlib.Path) -> None:

    with pytest.raises(ValueError, match="resume"):
        seex.init(
            dir=tmp_path,
            resume="auto",  # type: ignore[reportArgumentType]
        )
    with pytest.raises(TypeError, match="resume"):
        seex.init(
            dir=tmp_path,
            resume=1,  # type: ignore[reportArgumentType]
        )
    with pytest.raises(ValueError, match="`id`"):
        seex.init(dir=tmp_path, resume="must")


def test_run_rejects_invalid_options_before_creating_storage(tmp_path: pathlib.Path) -> None:
    invalid_resume = tmp_path / "invalid-resume"
    with pytest.raises(ValueError, match="resume"):
        seex.init(
            dir=invalid_resume,
            resume="auto",  # type: ignore[reportArgumentType]
        )
    assert not invalid_resume.exists()

    missing_id = tmp_path / "missing-id"
    with pytest.raises(ValueError, match="`id`"):
        seex.init(dir=missing_id, resume="must")
    assert not missing_id.exists()

    empty_project = tmp_path / "empty-project"
    with pytest.raises(ValueError, match="`project`"):
        seex.init(dir=empty_project, project="")
    assert not empty_project.exists()

    empty_id = tmp_path / "empty-id"
    with pytest.raises(ValueError, match="`id`"):
        seex.init(dir=empty_id, id="")
    assert not empty_id.exists()


def test_run_context_finishes_or_fails(tmp_path: pathlib.Path) -> None:
    finished_path = tmp_path / "finished"
    with seex.init(dir=finished_path, id="finished") as finished:
        assert finished.status == "running"

    failed_path = tmp_path / "failed"
    with (
        pytest.raises(RuntimeError, match="training failed"),
        seex.init(dir=failed_path, id="failed"),
    ):
        raise RuntimeError("training failed")

    finished = seex.Api(finished_path).run("uncategorized/finished")
    failed = seex.Api(failed_path).run("uncategorized/failed")
    assert finished is not None and finished.status == "finished"
    assert failed is not None and failed.status == "failed"


def test_run_retries_matching_outcome_and_rejects_conflict(tmp_path: pathlib.Path) -> None:
    run = seex.init(dir=tmp_path, id="run-1")
    run.finish()
    run.finish(0)

    with pytest.raises(seex.InvalidRunStateError, match="conflicts"):
        run.finish(1)


def test_run_logs_atomic_numeric_mappings(tmp_path: pathlib.Path) -> None:
    run = seex.init(dir=tmp_path, id="run-1")
    run.log({"loss": 2, "accuracy": 0.5})
    run.log({"loss": 1.0})
    run.finish()

    record = seex.Api(tmp_path).run("uncategorized/run-1")
    assert record is not None
    assert [(point.step, point.value_f64) for point in record.history("loss").points] == [
        (0, 2.0),
        (1, 1.0),
    ]
    assert run.diagnostics().persisted_reports == 3


def test_run_applies_explicit_step_and_commit_cursor_rules(tmp_path: pathlib.Path) -> None:
    run = seex.init(dir=tmp_path, id="run-1")
    run.log({"loss": 9.0}, step=5)
    run.log({"loss": 0.0})
    run.log({"loss": 4.0}, step=4, commit=True)
    run.log({"loss": 5.0})
    run.finish()

    record = seex.Api(tmp_path).run("uncategorized/run-1")
    assert record is not None
    points = record.history("loss").points
    assert [(point.step, point.value_f64) for point in points] == [
        (0, 0.0),
        (4, 4.0),
        (5, 5.0),
    ]


def test_run_rejects_oversized_mapping_atomically(tmp_path: pathlib.Path) -> None:
    run = seex.init(dir=tmp_path, id="run-1")
    with pytest.raises(ValueError, match="maximum is 8192"):
        run.log({f"metric-{index}": float(index) for index in range(8_193)})
    run.log({"accepted": 1.0})
    run.finish()

    record = seex.Api(tmp_path).run("uncategorized/run-1")
    assert record is not None
    assert record.metrics()[0].metric_key == "accepted"


def test_finished_run_releases_native_resources_before_python_drop(tmp_path: pathlib.Path) -> None:
    first = seex.init(project="project-1", dir=tmp_path, id="first")
    first.finish()
    second = seex.init(project="project-1", dir=tmp_path, id="second")
    second.finish()

    api = seex.Api(tmp_path)
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
        ({"loss": decimal.Decimal("1.0")}, TypeError),
        ({"loss": fractions.Fraction(1, 2)}, TypeError),
        ({1: 1.0}, TypeError),
    ],
)
def test_run_rejects_invalid_metric_mappings(
    tmp_path: pathlib.Path,
    data: object,
    error_type: type[Exception],
) -> None:

    run = seex.init(dir=tmp_path)
    with pytest.raises(error_type, match="Mapping"):
        run.log(data)  # type: ignore[reportArgumentType]
