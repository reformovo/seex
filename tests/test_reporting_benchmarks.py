"""Verify reporting benchmark modes and machine records."""

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


def test_throughput_result_emits_v2_metric(capsys: pytest.CaptureFixture[str]) -> None:
    result: dict[str, Any] = {
        "mode": "explicit_single",
        "reports": 100_000,
        "calls_per_second": 1_000_000.0,
    }

    bench_log_throughput.print_result(result)

    parsed = performance_gate.parse_v2_output(capsys.readouterr().out)
    metric = parsed["metrics"]["reporting.python.explicit_single.admission"]
    assert metric["unit"] == "points/s"
    assert metric["batch_iterations"] == 100_000


def test_mapping_mode_requires_complete_groups() -> None:
    with pytest.raises(ValueError, match="divisible"):
        bench_log_throughput.log_reports(FakeRun(), "mapping_8", 9)


def test_persistence_result_emits_phase_metrics_and_check(capsys: pytest.CaptureFixture[str]) -> None:
    diagnostics = {
        "pending_reports": 0,
        "persisted_reports": 1_000,
        "queue_full_errors": 0,
        "last_flush_status": "succeeded",
        "writer_state": "closed",
    }
    repeat = {
        "admission_seconds": 0.001,
        "drain_seconds": 0.002,
        "finalization_seconds": 0.003,
        "shutdown_seconds": 0.0001,
        "diagnostics_after_drain": diagnostics,
        "diagnostics_after_finalization": diagnostics,
        "diagnostics_after_shutdown": diagnostics,
    }

    bench_log_persistence.emit_performance_records({"reports_per_repeat": 1_000, "repeat_results": [repeat]})

    parsed = performance_gate.parse_v2_output(capsys.readouterr().out)
    assert parsed["metrics"]["reporting.python.drain_persistence"]["p50"] == 2_000_000.0
    assert parsed["checks"][0]["passed"]
