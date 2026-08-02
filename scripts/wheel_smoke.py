"""Run a minimal lifecycle smoke test against an installed Seex wheel."""

from __future__ import annotations

import pathlib
import tempfile

import seex


def main() -> None:
    """Checks import, initialization, logging, finalization, and querying."""
    if "Run" not in seex.__all__ or "Client" in seex.__all__:
        raise RuntimeError("installed seex package does not export the beta Run API")

    with tempfile.TemporaryDirectory(prefix="seex-wheel-smoke-") as root:
        project_root = pathlib.Path(root)
        points: list[seex.MetricPoint] = []
        with seex.init(
            project="project-1",
            dir=project_root,
            id="run-1",
            name="smoke",
        ) as run:
            run.log({"train/loss": 0.25}, step=0)

        if run.status != "finished":
            raise RuntimeError(f"run did not finish: {run.status!r}")
        with seex.Api(project_root) as api:
            record = api.run("project-1/run-1")
            if record is None:
                raise RuntimeError("query did not find the finished Run")
            points = record.history("train/loss").points
        if [(point.step, point.value_f64) for point in points] != [(0, 0.25)]:
            raise RuntimeError("query did not return the logged metric point")


if __name__ == "__main__":
    main()
