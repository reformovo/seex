"""Verify reporting benchmark modes and machine records."""

import argparse
import pathlib
from typing import Any

import pytest

from scripts import bench_log_persistence, bench_log_throughput, performance_gate


class FakeRun:
    def __init__(self) -> None:
        self.calls: list[tuple[str, int, float]] = []

    def log(self, key: str, step: int, value: float) -> None:
        self.calls.append((key, step, value))


@pytest.mark.parametrize("mode", bench_log_throughput.MODES)
def test_admission_modes_count_points(mode: str) -> None:
    run = FakeRun()

    bench_log_throughput.log_reports(run, mode, 16)

    assert len(run.calls) == 16
    if mode == "mapping_8":
        assert {step for _, step, _ in run.calls} == {0, 1}


def test_throughput_result_emits_raw_samples(capsys: pytest.CaptureFixture[str]) -> None:
    result: dict[str, Any] = {
        "mode": "explicit_single",
        "reports": 100_000,
        "samples": [1_000_000.0] * 10,
        "calls_per_second": 1_000_000.0,
    }

    bench_log_throughput.print_result(result)

    metric = performance_gate.parse_records(capsys.readouterr().out)["reporting.python.explicit_single.admission"]
    assert metric["unit"] == "points/s"
    assert metric["batch_iterations"] == 100_000
    assert len(metric["samples"]) == 10


def test_throughput_calibrates_once_then_collects_ten_samples() -> None:
    durations = iter([0.01, 0.02] + [0.03] * 11)
    batches: list[int] = []

    def measure(reports: int) -> float:
        batches.append(reports)
        return next(durations)

    reports, samples = bench_log_throughput.calibrated_samples(measure, 100)

    assert reports == 400
    assert batches == [100, 200] + [400] * 11
    assert samples == pytest.approx([400 / 0.03] * 10)


def test_throughput_parent_aggregates_fresh_sample_children(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: pathlib.Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    paths: list[pathlib.Path] = []

    def sample_child(args: argparse.Namespace, project_path: pathlib.Path, *, reports: int) -> dict[str, Any]:
        del args
        paths.append(project_path)
        batch = 100 if project_path.name == "calibration" else reports
        return {"batch_iterations": batch, "samples": [1_000_000.0]}

    monkeypatch.setattr(bench_log_throughput, "run_child", sample_child)
    args = bench_log_throughput.build_parser().parse_args(["--path", str(tmp_path)])

    assert bench_log_throughput.parent_main(args) == 0

    metric = performance_gate.parse_records(capsys.readouterr().out)["reporting.python.explicit_single.admission"]
    assert metric["batch_iterations"] == 917_504
    assert metric["samples"] == [1_000_000.0] * 10
    assert [path.name for path in paths] == ["calibration"] + [f"sample-{index}" for index in range(10)]


def test_mapping_mode_requires_complete_groups() -> None:
    with pytest.raises(ValueError, match="divisible"):
        bench_log_throughput.log_reports(FakeRun(), "mapping_8", 9)


def test_persistence_emits_only_durability_samples(capsys: pytest.CaptureFixture[str]) -> None:
    diagnostics = {
        "pending_reports": 0,
        "persisted_reports": 1_000,
        "queue_full_errors": 0,
        "last_flush_status": "succeeded",
        "writer_state": "closed",
    }
    repeat = {
        "admission_seconds": 0.001,
        "drain_seconds": 0.025,
        "finalization_seconds": 0.03,
        "shutdown_seconds": 0.0001,
        "diagnostics_after_drain": diagnostics,
        "diagnostics_after_finalization": diagnostics,
        "diagnostics_after_shutdown": diagnostics,
    }

    bench_log_persistence.emit_performance_records({"reports_per_repeat": 1_000, "repeat_results": [repeat] * 30})

    metrics = performance_gate.parse_records(capsys.readouterr().out)
    assert set(metrics) == {"reporting.python.drain_persistence", "reporting.python.finalization"}
    assert metrics["reporting.python.drain_persistence"]["samples"] == pytest.approx([75_000_000.0] * 10)
    assert metrics["reporting.python.drain_persistence"]["batch_iterations"] == 3
    assert metrics["reporting.python.finalization"]["calibrated"]


def test_persistence_rejects_partial_sample_sets() -> None:
    with pytest.raises(ValueError, match="exactly thirty"):
        bench_log_persistence.emit_performance_records({"repeat_results": []})
