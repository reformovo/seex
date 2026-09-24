"""Static type contracts for the public Run-only Python API."""

from __future__ import annotations

import pathlib
from typing import Literal, assert_type

import seex


def check_run(settings: seex.Settings, root: pathlib.Path) -> None:
    run = seex.init(
        project="project-1",
        dir=root,
        id="run-1",
        name="baseline",
        resume="allow",
        settings=settings,
    )
    assert_type(run, seex.Run)
    assert_type(run.status, Literal["running", "finished", "failed"])
    run.log({"loss": 1.0, "samples": 10}, step=1, commit=True)
    run.finish()
    diagnostics = run.diagnostics()
    assert_type(diagnostics, seex.Diagnostics)


def check_rejected_mutation(settings: seex.Settings, run: seex.Run) -> None:
    settings.catalog_backend = "sqlite"  # type: ignore[reportAttributeAccessIssue]
    run.run_id = "other"  # type: ignore[reportAttributeAccessIssue]
