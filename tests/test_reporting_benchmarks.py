"""Verify reporting benchmark modes and machine records."""

import json
from typing import Any

import pytest

from scripts import bench_log_persistence, bench_log_throughput


def _legacy_records(output: str) -> list[dict[str, Any]]:
    return [
        json.loads(line.removeprefix("SEEX_PERF ")) for line in output.splitlines() if line.startswith("SEEX_PERF ")
    ]


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

    metric = _legacy_records(capsys.readouterr().out)[0]
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

    records = _legacy_records(capsys.readouterr().out)
    drain = next(record for record in records if record.get("metric") == "python.drain_persistence")
    assert drain["p50"] == 2_000_000.0
    assert records[-1]["passed"]
