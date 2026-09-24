"""Run a minimal lifecycle smoke test against an installed Seex wheel."""

from __future__ import annotations

import pathlib
import tempfile

import seex


def main() -> None:
    """Checks import, initialization, logging, finalization, and diagnostics."""
    if "Run" not in seex.__all__ or "Api" in seex.__all__:
        raise RuntimeError("installed seex package does not export the Run-only API")

    with tempfile.TemporaryDirectory(prefix="seex-wheel-smoke-") as root:
        project_root = pathlib.Path(root)
        with seex.init(
            project="project-1",
            dir=project_root,
            id="run-1",
            name="smoke",
        ) as run:
            run.log({"train/loss": 0.25}, step=0)

        if run.status != "finished":
            raise RuntimeError(f"run did not finish: {run.status!r}")
        diagnostics = run.diagnostics()
        if diagnostics.writer_state not in {"drained", "closed"}:
            raise RuntimeError(f"writer did not drain: {diagnostics.writer_state!r}")


if __name__ == "__main__":
    main()
