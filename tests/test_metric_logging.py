"""Verify Python SDK metric logging behavior."""

from __future__ import annotations

import pathlib

import pytest
from seex import _seex

from tests import helpers


def test_log_requires_explicit_step_and_reports_diagnostics(
    tmp_path: pathlib.Path,
) -> None:
    import seex

    client = _seex.init(tmp_path / "seex")
    project = client.create_project("local training", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")

    run.log("train/loss", 0, 0.25)
    with pytest.raises(TypeError):
        run.log("train/loss", 0.125)  # type: ignore[reportCallIssue]
    diagnostics = client.diagnostics()

    assert isinstance(diagnostics, seex.Diagnostics)
    assert diagnostics.pending_reports >= 0
    assert diagnostics.queue_full_errors == 0
    assert diagnostics.persisted_reports >= 0
    assert diagnostics.writer_state in {"running", "drained"}
    assert diagnostics.last_write_error is None
    assert diagnostics.last_flush_run_id is None
    assert diagnostics.last_flush_status == "none"
    assert diagnostics.last_flush_error is None
    for field in helpers.DIAGNOSTIC_FIELDS:
        assert hasattr(diagnostics, field)
    for removed_field in helpers.UNSUPPORTED_DIAGNOSTIC_FIELDS:
        assert not hasattr(diagnostics, removed_field)
