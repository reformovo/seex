"""Verify the dependency-free read-only Seex CLI."""

from __future__ import annotations

import json
import math
import os
import pathlib
import shutil
import time
import typing
from unittest import mock

import pytest
import seex
from seex import cli


def _seed_run(
    root: pathlib.Path,
    project_id: str,
    run_id: str,
    *,
    name: str | None = None,
    metrics: typing.Iterable[tuple[str, int, float]] = (),
    status: str = "finished",
    settings: seex.Settings | None = None,
) -> None:
    run = seex.init(
        project=project_id,
        dir=root,
        id=run_id,
        name=name,
        settings=settings,
    )
    for metric_key, step, value in metrics:
        run.log({metric_key: value}, step=step)
    if status == "finished":
        run.finish()
    elif status == "failed":
        run.finish(1)
    else:
        deadline = time.monotonic() + 5.0
        while run.diagnostics().pending_reports != 0:
            if time.monotonic() >= deadline:
                raise AssertionError("timed out waiting for CLI fixture metrics")
            time.sleep(0.01)


def test_cli_app_replaces_process_with_seex_app(
    tmp_path: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_path = tmp_path / "project"
    monkeypatch.setattr(shutil, "which", lambda _: "/Applications/Seex.app/Contents/MacOS/seex-app")

    class ExecCalled(Exception):
        pass

    def execv(path: str, arguments: list[str]) -> typing.NoReturn:
        assert path == "/Applications/Seex.app/Contents/MacOS/seex-app"
        assert arguments == [path, os.fspath(project_path)]
        raise ExecCalled

    monkeypatch.setattr(os, "execv", execv)

    with pytest.raises(ExecCalled):
        cli.main(["app", os.fspath(project_path)])


def test_cli_app_reports_missing_binary(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setattr(shutil, "which", lambda _: None)

    assert cli.main(["app"]) == 1
    assert "seex-app was not found on PATH" in capsys.readouterr().err


def test_cli_discovers_running_metric_points_through_all_read_commands(
    tmp_path: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    _seed_run(
        root_path,
        "project-1",
        "run-1",
        name="baseline",
        metrics=(("train/loss", 0, 0.5), ("train/loss", 1, 0.25)),
        status="running",
    )
    monkeypatch.chdir(root_path)

    commands = (
        (["projects", "list"], "project-1"),
        (
            [
                "runs",
                "list",
                "project-1",
                "--status",
                "running",
                "--limit",
                "1",
            ],
            "run-1",
        ),
        (["metrics", "list", "run-1"], "train/loss"),
        (
            [
                "metrics",
                "query",
                "run-1",
                "train/loss",
                "--start-step",
                "0",
                "--end-step",
                "1",
                "--all",
            ],
            "0.5",
        ),
    )
    for argv, expected in commands:
        assert cli.main(argv) == 0
        assert expected in capsys.readouterr().out

    kinds = ("projects", "runs", "metrics", "metric_points")
    for (argv, _), kind in zip(commands, kinds, strict=True):
        assert cli.main(["--format", "json", *argv]) == 0
        document = json.loads(capsys.readouterr().out)
        assert document["schema_version"] == 2
        assert document["kind"] == kind
        assert isinstance(document["data"], list)
        assert "page" in document
        assert "meta" in document
        if kind == "runs":
            assert document["page"] == {
                "offset": 0,
                "limit": 1,
                "returned": 1,
                "has_more": False,
            }
        if kind == "metric_points":
            assert [point["step"] for point in document["data"]] == [0]


def test_cli_comparison_reports_require_baseline_and_preserve_candidate_order(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    for project_id, run_id, value in (
        ("project-1", "baseline", 3.0),
        ("project-2", "candidate-b", 1.0),
        ("project-2", "candidate-a", 2.0),
    ):
        _seed_run(
            root_path,
            project_id,
            run_id,
            metrics=(("loss", 0, value), ("throughput", 0, value * 10)),
        )

    command = [
        "--path",
        str(root_path),
        "metrics",
        "compare",
        "loss",
        "candidate-b",
        "baseline",
        "candidate-a",
        "--baseline",
        "baseline",
        "--direction",
        "minimize",
        "--secondary",
        "throughput",
    ]
    assert cli.main(command) == 0
    table = capsys.readouterr().out
    assert table.index("candidate-b") < table.index("candidate-a")
    assert "baseline" in table
    assert "secondary" in table
    assert "throughput" in table

    assert cli.main(["--format", "json", *command]) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["kind"] == "comparison_reports"
    assert document["meta"] == {"reference_role": "baseline"}
    candidate_ids = [item["primary"]["candidate"]["run_id"] for item in document["data"]]
    assert candidate_ids == ["candidate-b", "candidate-a"]
    preferences = [item["primary"]["preference"] for item in document["data"]]
    assert preferences == ["candidate", "candidate"]
    assert [item["secondary"][0]["metric_key"] for item in document["data"]] == [
        "throughput",
        "throughput",
    ]
    assert document["data"][0]["secondary"][0]["raw_delta"] == -20.0
    assert "points" not in json.dumps(document)


@pytest.mark.parametrize(
    ("run_ids", "baseline", "message"),
    (
        (["candidate"], "baseline", "must be contained"),
        (["candidate", "candidate"], "candidate", "must be unique"),
    ),
)
def test_cli_comparison_rejects_invalid_run_sets(
    run_ids: list[str],
    baseline: str,
    message: str,
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as error_info:
        cli.main(
            [
                "metrics",
                "compare",
                "loss",
                *run_ids,
                "--baseline",
                baseline,
                "--direction",
                "minimize",
            ]
        )

    assert error_info.value.code == 2
    assert message in capsys.readouterr().err


def test_cli_autoresearch_compare_uses_explicit_or_best_incumbent(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    for run_id, value in (("candidate", 2.0), ("best", 1.0), ("worse", 3.0)):
        _seed_run(root_path, "project-1", run_id, metrics=(("loss", 0, value),))
    base = ["--path", str(root_path), "--format", "json", "autoresearch", "compare"]

    explicit = [
        *base,
        "candidate",
        "--metric",
        "loss",
        "--direction",
        "minimize",
        "--against",
        "worse",
    ]
    assert cli.main(explicit) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["data"][0]["primary"]["reference"]["run_id"] == "worse"

    pooled = [
        *base,
        "candidate",
        "--metric",
        "loss",
        "--direction",
        "minimize",
        "--comparator",
        "worse",
        "--comparator",
        "best",
    ]
    assert cli.main(pooled) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["data"][0]["primary"]["reference"]["run_id"] == "best"
    assert document["meta"] == {"reference_role": "incumbent"}

    pooled[pooled.index("minimize")] = "maximize"
    assert cli.main(pooled) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["data"][0]["primary"]["reference"]["run_id"] == "worse"


def test_cli_autoresearch_compare_reports_no_eligible_incumbent(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    _seed_run(
        root_path,
        "project-1",
        "candidate",
        metrics=(("loss", 0, 1.0), ("memory", 0, 2.0)),
        status="running",
    )
    _seed_run(
        root_path,
        "project-1",
        "running",
        metrics=(("loss", 0, 0.5),),
        status="running",
    )

    command = [
        "--path",
        str(root_path),
        "autoresearch",
        "compare",
        "candidate",
        "--metric",
        "loss",
        "--direction",
        "minimize",
        "--comparator",
        "running",
        "--secondary",
        "memory",
    ]
    status = cli.main(["--format", "json", *command])

    assert status == 0
    document = json.loads(capsys.readouterr().out)
    report = document["data"][0]
    assert report["primary"]["candidate"]["last_value"] == 1.0
    assert report["primary"]["candidate"]["completeness"] == "partial"
    assert report["primary"]["candidate"]["reasons"] == ["run_running"]
    assert report["primary"]["reference"] is None
    assert report["primary"]["completeness"] == "unavailable"
    assert report["secondary"][0]["candidate"]["last_value"] == 2.0
    assert report["secondary"][0]["candidate"]["completeness"] == "partial"
    assert report["secondary"][0]["reference"] is None
    assert report["reasons"] == ["no_eligible_incumbent"]
    assert report["preference"] == "inconclusive"

    assert cli.main(command) == 0
    table = capsys.readouterr().out
    assert "run_running" in table
    assert "no_eligible_incumbent" in table
    assert "memory" in table


def test_cli_autoresearch_compare_rejects_candidate_in_pool(
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as error_info:
        cli.main(
            [
                "autoresearch",
                "compare",
                "candidate",
                "--metric",
                "loss",
                "--direction",
                "minimize",
                "--comparator",
                "candidate",
            ]
        )

    assert error_info.value.code == 2
    assert "candidate must not be a comparator" in capsys.readouterr().err


def test_cli_autoresearch_leaderboard_keeps_structured_ineligible_evidence(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    specifications = (
        ("tied-a", 1.0, "finished"),
        ("tied-b", 1.0, "finished"),
        ("worse", 2.0, "finished"),
        ("failed", 0.5, "failed"),
        ("running", 0.25, "running"),
        ("missing", None, "finished"),
        ("invalid", math.inf, "finished"),
    )
    for run_id, value, status in specifications:
        metrics = () if value is None else (("loss", 0, value),)
        _seed_run(root_path, "project-1", run_id, metrics=metrics, status=status)

    command = [
        "--path",
        str(root_path),
        "--format",
        "json",
        "autoresearch",
        "leaderboard",
        "project-1",
        "--metric",
        "loss",
        "--direction",
        "minimize",
        "--all",
    ]
    for run_id in (
        "worse",
        "failed",
        "tied-b",
        "tied-a",
        "running",
        "missing",
        "invalid",
    ):
        command.extend(("--run", run_id))
    assert cli.main(command) == 0
    document = json.loads(capsys.readouterr().out)

    assert document["kind"] == "autoresearch_leaderboard"
    assert document["meta"]["eligible_entries"] == 3
    assert document["page"] == {
        "offset": 0,
        "limit": None,
        "returned": 7,
        "has_more": False,
    }
    assert [item["evidence"]["run_id"] for item in document["data"]] == [
        "tied-a",
        "tied-b",
        "worse",
        "failed",
        "running",
        "missing",
        "invalid",
    ]
    assert [item["rank"] for item in document["data"]] == [
        1,
        1,
        3,
        None,
        None,
        None,
        None,
    ]
    assert document["data"][-1]["evidence"]["last_value"] == "Infinity"
    assert document["data"][-1]["evidence"]["reasons"] == ["non_finite_value"]


def test_cli_autoresearch_leaderboard_paginates_after_ranking(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    for index in range(51):
        _seed_run(
            root_path,
            "project-1",
            f"run-{index}",
            metrics=(("score", 0, float(51 - index)),),
        )
    base = [
        "--path",
        str(root_path),
        "--format",
        "json",
        "autoresearch",
        "leaderboard",
        "project-1",
        "--metric",
        "score",
        "--direction",
        "maximize",
    ]

    assert cli.main([*base, "--limit", "1", "--offset", "2"]) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["data"][0]["rank"] == 3
    assert document["data"][0]["evidence"]["run_id"] == "run-2"
    assert document["page"] == {
        "offset": 2,
        "limit": 1,
        "returned": 1,
        "has_more": True,
    }

    assert cli.main(base) == 0
    document = json.loads(capsys.readouterr().out)
    assert len(document["data"]) == 50
    assert document["page"]["limit"] == 50
    assert document["page"]["has_more"] is True


def test_cli_autoresearch_best_uses_stable_tie_break_and_returns_null(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    for run_id, value in (("zeta", 1.0), ("alpha", 1.0), ("worse", 2.0)):
        _seed_run(root_path, "project-1", run_id, metrics=(("loss", 0, value),))
    _seed_run(root_path, "project-1", "failed", metrics=(("loss", 0, 0.5),), status="failed")
    _seed_run(root_path, "project-2", "ineligible", status="failed")
    base = [
        "--path",
        str(root_path),
        "--format",
        "json",
        "autoresearch",
        "best",
        "project-1",
        "--metric",
        "loss",
        "--direction",
        "minimize",
    ]

    assert cli.main(base) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["kind"] == "autoresearch_best"
    assert document["data"]["best"]["evidence"]["run_id"] == "zeta"

    assert cli.main([*base, "--run", "failed"]) == 0
    document = json.loads(capsys.readouterr().out)
    assert document["data"] == {"best": None}
    assert document["meta"]["eligible_entries"] == 0

    empty = [*base]
    empty[empty.index("project-1")] = "project-2"
    assert cli.main(empty) == 0
    assert json.loads(capsys.readouterr().out)["data"] == {"best": None}
    table_command = [*empty]
    format_index = table_command.index("--format")
    del table_command[format_index : format_index + 2]
    assert cli.main(table_command) == 0
    assert capsys.readouterr().out == "No eligible Run.\n"


def test_cli_autoresearch_ranking_rejects_invalid_run_scopes(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    _seed_run(root_path, "project-1", "placeholder")
    _seed_run(root_path, "project-2", "foreign", status="running")
    base = [
        "--path",
        str(root_path),
        "--format",
        "json",
        "autoresearch",
        "leaderboard",
        "project-1",
        "--metric",
        "loss",
        "--direction",
        "minimize",
    ]

    assert cli.main([*base, "--run", "foreign"]) == 1
    error = json.loads(capsys.readouterr().err)["error"]
    assert error["code"] == "run_project_mismatch"
    assert cli.main([*base, "--run", "unknown"]) == 1
    assert json.loads(capsys.readouterr().err)["error"]["code"] == "storage_error"

    with pytest.raises(SystemExit) as error_info:
        cli.main([*base, "--run", "foreign", "--run", "foreign"])
    assert error_info.value.code == 2
    assert json.loads(capsys.readouterr().err)["error"]["code"] == "cli_usage_error"

    with pytest.raises(SystemExit) as error_info:
        cli.main([*base, "--all", "--offset", "1"])
    assert error_info.value.code == 2
    assert "--all cannot be combined" in capsys.readouterr().err


def test_cli_comparison_reports_partial_and_per_metric_evidence(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    _seed_run(
        root_path,
        "project-1",
        "baseline",
        metrics=(("loss", 0, 2.0), ("memory", 0, 10.0)),
    )
    _seed_run(
        root_path,
        "project-1",
        "running",
        metrics=(("loss", 0, 1.0), ("memory", 0, 5.0)),
        status="running",
    )
    _seed_run(root_path, "project-1", "failed", metrics=(("loss", 0, 3.0),), status="failed")
    _seed_run(root_path, "project-1", "missing", metrics=(("memory", 0, 20.0),))
    _seed_run(
        root_path,
        "project-1",
        "invalid-secondary",
        metrics=(("loss", 0, 4.0), ("memory", 0, math.inf)),
    )

    command = [
        "--path",
        str(root_path),
        "metrics",
        "compare",
        "loss",
        "running",
        "baseline",
        "failed",
        "missing",
        "invalid-secondary",
        "--baseline",
        "baseline",
        "--direction",
        "minimize",
        "--secondary",
        "memory",
    ]
    status = cli.main(["--format", "json", *command])

    assert status == 0
    data = json.loads(capsys.readouterr().out)["data"]
    reports = {item["primary"]["candidate"]["run_id"]: item for item in data}
    running = reports["running"]["primary"]
    assert running["completeness"] == "partial"
    assert running["outcome"] == "improved"
    assert running["preference"] == "inconclusive"
    assert running["candidate"]["reasons"] == ["run_running"]
    failed = reports["failed"]["primary"]
    assert failed["completeness"] == "partial"
    assert failed["outcome"] == "regressed"
    assert failed["candidate"]["reasons"] == ["run_failed"]
    missing = reports["missing"]["primary"]
    assert missing["completeness"] == "unavailable"
    assert missing["candidate"]["reasons"] == ["missing_metric"]
    invalid = reports["invalid-secondary"]
    assert invalid["primary"]["preference"] == "reference"
    assert invalid["secondary"][0]["completeness"] == "invalid"
    assert invalid["secondary"][0]["candidate"]["last_value"] == "Infinity"
    assert invalid["secondary"][0]["candidate"]["reasons"] == ["non_finite_value"]

    assert cli.main(command) == 0
    table = capsys.readouterr().out
    assert "run_running" in table
    assert "non_finite_value" in table


def test_cli_comparison_reports_reject_unknown_run(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    _seed_run(root_path, "project-1", "baseline", metrics=(("loss", 0, 1.0),))

    status = cli.main(
        [
            "--path",
            str(root_path),
            "--format",
            "json",
            "metrics",
            "compare",
            "loss",
            "unknown",
            "baseline",
            "--baseline",
            "baseline",
            "--direction",
            "minimize",
        ]
    )

    assert status == 1
    error = json.loads(capsys.readouterr().err)["error"]
    assert error == {"code": "storage_error", "message": "run not found: unknown"}


def test_cli_missing_store_fails_without_creating_it(
    tmp_path: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "missing"
    root_path.mkdir()
    monkeypatch.chdir(root_path)

    assert cli.main(["projects", "list"]) == 1

    captured = capsys.readouterr()
    assert captured.out == ""
    assert captured.err == "catalog not found: catalog.ducklake\n"
    assert not (root_path / ".seex").exists()


def test_cli_json_operation_errors_are_structured(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "missing"
    root_path.mkdir()

    status = cli.main(["--path", str(root_path), "--format", "json", "projects", "list"])

    assert status == 1
    captured = capsys.readouterr()
    assert captured.out == ""
    assert json.loads(captured.err) == {
        "schema_version": 2,
        "error": {
            "code": "storage_error",
            "message": "catalog not found: catalog.ducklake",
        },
    }


def test_cli_resolves_global_path_overrides_against_project(
    tmp_path: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "workspace" / "project"
    settings = seex.Settings(
        catalog_backend="sqlite",
        catalog_path=root_path / "storage" / "catalog.sqlite",
        data_path=root_path / "storage" / "data",
    )
    _seed_run(root_path, "project-1", "run-1", settings=settings)
    project = seex.Api(root_path, settings).project("project-1")
    assert project is not None
    monkeypatch.chdir(tmp_path)

    status = cli.main(
        [
            "--path",
            "workspace/project",
            "--format",
            "json",
            "--catalog-backend",
            "sqlite",
            "--catalog-path",
            "storage/catalog.sqlite",
            "--data-path",
            "storage/data",
            "projects",
            "list",
        ]
    )

    assert status == 0
    assert json.loads(capsys.readouterr().out) == {
        "schema_version": 2,
        "kind": "projects",
        "data": [
            {
                "created_at": project.created_at,
                "name": "project-1",
                "project_id": "project-1",
            }
        ],
        "page": None,
        "meta": {},
    }


def test_cli_json_includes_pagination_and_metric_query_metadata(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    _seed_run(root_path, "project-1", "run-1", name="first", metrics=(("loss", 0, 0.5),))
    _seed_run(root_path, "project-1", "run-2", name="second", metrics=(("loss", 0, 0.25),))

    global_args = ["--path", str(root_path), "--format", "json"]
    assert cli.main([*global_args, "runs", "list", "project-1", "--limit", "1"]) == 0
    runs_document = json.loads(capsys.readouterr().out)
    assert runs_document["schema_version"] == 2
    assert runs_document["kind"] == "runs"
    assert len(runs_document["data"]) == 1
    assert runs_document["page"] == {
        "offset": 0,
        "limit": 1,
        "returned": 1,
        "has_more": True,
    }
    assert runs_document["meta"] == {}

    assert cli.main([*global_args, "metrics", "query", "run-1", "loss"]) == 0
    query_document = json.loads(capsys.readouterr().out)
    assert query_document["schema_version"] == 2
    assert query_document["kind"] == "metric_points"
    assert query_document["page"] is None
    assert query_document["meta"] == {
        "source_row_count": 1,
        "returned_row_count": 1,
        "downsampled": False,
    }
    assert query_document["data"][0]["step"] == 0


def test_cli_json_normalizes_non_finite_metric_values(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    _seed_run(
        root_path,
        "project-1",
        "run-1",
        name="non-finite",
        metrics=(("loss", step, value) for step, value in enumerate((math.nan, math.inf, -math.inf))),
    )

    global_args = ["--path", str(root_path), "--format", "json"]
    assert cli.main([*global_args, "metrics", "query", "run-1", "loss", "--all"]) == 0
    query = json.loads(capsys.readouterr().out)
    assert [row["value"] for row in query["data"]] == [
        "NaN",
        "Infinity",
        "-Infinity",
    ]

    assert cli.main([*global_args, "metrics", "list", "run-1"]) == 0
    summary = json.loads(capsys.readouterr().out)
    assert all(not isinstance(value, float) or math.isfinite(value) for value in summary["data"][0].values())


def test_cli_preserves_symlinked_project_path(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    real_path = tmp_path / "real-project"
    real_path.mkdir()
    linked_path = tmp_path / "linked-project"
    try:
        linked_path.symlink_to(real_path, target_is_directory=True)
    except OSError as error:
        pytest.skip(f"directory symlinks are unavailable: {error}")
    _seed_run(linked_path, "project-1", "run-1")

    status = cli.main(["--path", str(linked_path), "projects", "list"])

    assert status == 0
    captured = capsys.readouterr()
    assert "project-1" in captured.out
    assert captured.err == ""


def test_cli_metric_query_point_limits_are_mutually_exclusive() -> None:
    parser = cli._build_parser()

    defaults = parser.parse_args(["metrics", "query", "run-1", "loss"])
    all_points = parser.parse_args(["metrics", "query", "run-1", "loss", "--all"])

    assert defaults.max_points == 200
    assert defaults.all is False
    assert all_points.all is True
    with pytest.raises(SystemExit) as error_info:
        parser.parse_args(
            [
                "metrics",
                "query",
                "run-1",
                "loss",
                "--max-points",
                "20",
                "--all",
            ]
        )
    assert error_info.value.code == 2


def test_cli_metric_query_uses_public_history() -> None:
    api = mock.Mock()
    api.projects.return_value = [mock.Mock(project_id="project-1")]
    point = mock.Mock(step=0, value_f64=0.5, timestamp="2026-07-15T00:00:00Z")
    record = mock.Mock()
    record.history.return_value = mock.Mock(points=[point], source_count=1, downsampled=False)
    api.run.return_value = record
    args = cli._build_parser().parse_args(["metrics", "query", "run-1", "loss"])

    cli._run_read(typing.cast(seex.Api, api), args)

    record.history.assert_called_once_with("loss", start=None, end=None, max_points=200)


@pytest.mark.parametrize(
    "argv",
    (
        ["runs", "list", "project-1", "--limit", "-1"],
        ["runs", "list", "project-1", "--offset", "-1"],
        ["metrics", "query", "run-1", "loss", "--max-points", "-1"],
    ),
    ids=("limit", "offset", "max-points"),
)
def test_cli_rejects_negative_unsigned_arguments(argv: list[str], capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit) as error_info:
        cli.main(argv)

    assert error_info.value.code == 2
    captured = capsys.readouterr()
    assert captured.out == ""
    assert "expected a non-negative integer" in captured.err
    assert "Traceback" not in captured.err


def test_cli_json_usage_errors_are_structured(
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as error_info:
        cli.main(
            [
                "--format",
                "json",
                "metrics",
                "query",
                "run-1",
                "loss",
                "--max-points",
                "-1",
            ]
        )

    assert error_info.value.code == 2
    captured = capsys.readouterr()
    assert captured.out == ""
    document = json.loads(captured.err)
    assert document["schema_version"] == 2
    assert document["error"]["code"] == "cli_usage_error"
    assert "expected a non-negative integer" in document["error"]["message"]
    assert "usage:" not in captured.err


def test_cli_table_output_is_deterministic_and_uncolored() -> None:
    first = cli._render_table(("STEP", "VALUE"), ((0, 0.5), (10, 0.25)))
    second = cli._render_table(("STEP", "VALUE"), ((0, 0.5), (10, 0.25)))

    assert first == "STEP  VALUE\n----  -----\n0     0.5\n10    0.25"
    assert second == first
    assert "\x1b" not in first


def test_cli_json_output_is_deterministic() -> None:
    document = {
        "schema_version": 2,
        "data": [{"run_id": "run-1", "relative_delta": None}],
    }

    first = cli._dump_json(document)
    second = cli._dump_json(document)

    assert first == second
    assert first == ('{"data":[{"relative_delta":null,"run_id":"run-1"}],"schema_version":2}')


def test_cli_keeps_s3_credentials_out_of_arguments() -> None:
    help_text = cli._build_parser().format_help()

    assert "secret" not in help_text.lower()
    assert "access-key" not in help_text.lower()
    assert "session-token" not in help_text.lower()


def test_cli_sanitizes_config_credentials_and_catalog_paths(
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    config_path = root_path / ".seex" / "config.toml"
    config_path.parent.mkdir(parents=True)
    secret = "credential-must-not-leak"
    config_path.write_text(
        "schema_version = 1\n"
        'data_path = "s3://private-bucket/metrics"\n'
        "[s3]\n"
        'endpoint = "127.0.0.1:9000"\n'
        'access_key_id = "private-access-key"\n'
        f'secret_access_key = "{secret}"\n',
        encoding="utf-8",
    )
    private_catalog = tmp_path / "private" / "tenant" / "catalog.ducklake"

    status = cli.main(
        [
            "--path",
            str(root_path),
            "--format",
            "json",
            "--catalog-path",
            str(private_catalog),
            "projects",
            "list",
        ]
    )

    assert status == 1
    error = capsys.readouterr().err
    document = json.loads(error)
    assert document["error"] == {
        "code": "storage_error",
        "message": "catalog not found: catalog.ducklake",
    }
    assert secret not in error
    assert str(private_catalog.parent) not in error


def test_cli_metric_query_does_not_load_private_lttb_extension(
    tmp_path: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root_path = tmp_path / "project"
    _seed_run(
        root_path,
        "project-1",
        "run-1",
        name="long",
        metrics=(("loss", step, float(step)) for step in range(201)),
    )
    private_extension = tmp_path / "private" / "tenant" / "missing-lttb.duckdb_extension"
    monkeypatch.setenv("SEEX_LTTB_EXTENSION_PATH", str(private_extension))

    status = cli.main(
        [
            "--path",
            str(root_path),
            "--format",
            "json",
            "metrics",
            "query",
            "run-1",
            "loss",
        ]
    )

    assert status == 0
    captured = capsys.readouterr()
    document = json.loads(captured.out)
    assert document["kind"] == "metric_points"
    assert document["meta"]["downsampled"] is True
    assert document["meta"]["returned_row_count"] <= 200
    assert str(private_extension) not in captured.out
    assert captured.err == ""

    assert (
        cli.main(
            [
                "--path",
                str(root_path),
                "--format",
                "json",
                "metrics",
                "query",
                "run-1",
                "loss",
                "--all",
            ]
        )
        == 0
    )
    document = json.loads(capsys.readouterr().out)
    assert len(document["data"]) == 201
    assert document["meta"]["downsampled"] is False
