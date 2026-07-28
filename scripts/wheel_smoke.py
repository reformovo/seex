"""Run a minimal lifecycle smoke test against an installed Seex wheel."""

from __future__ import annotations

import pathlib
import tempfile

import seex


def main() -> None:
    """Checks import, initialization, logging, finalization, and querying."""
    if "Client" not in seex.__all__:
        raise RuntimeError("installed seex package does not export Client")

    with (
        tempfile.TemporaryDirectory(prefix="seex-wheel-smoke-") as root,
        seex.init(pathlib.Path(root)) as client,
    ):
        project = client.create_project("wheel smoke", project_id="project-1")
        run = client.create_run(project.project_id, "smoke", run_id="run-1")
        run.log("train/loss", 0, 0.25)
        finished = client.finish_run(run.run_id)
        points = client.query_metric(run.run_id, "train/loss")

        if finished.status != "finished":
            raise RuntimeError(f"run did not finish: {finished.status!r}")
        if [(point.step, point.value_f64) for point in points] != [(0, 0.25)]:
            raise RuntimeError("query did not return the logged metric point")


if __name__ == "__main__":
    main()
