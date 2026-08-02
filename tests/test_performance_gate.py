"""Verify the bounded schema-v3 performance comparison contract."""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
from collections.abc import Sequence
from typing import Literal, cast

import pytest

from scripts import performance_gate


def _record(
    value: float = 100.0,
    *,
    metric: str = "reader.narrow",
    unit: str = "ns/op",
    direction: str = "lower",
    batch_iterations: int = 300_000,
    samples: Sequence[float] | None = None,
) -> str:
    values = list(samples) if samples is not None else [value] * 10
    return "SEEX_BENCH " + json.dumps(
        {
            "schema_version": 3,
            "record_type": "metric",
            "domain": "query",
            "metric": metric,
            "unit": unit,
            "direction": direction,
            "batch_iterations": batch_iterations,
            "samples": values,
        }
    )


def _metric(
    name: str,
    value: float,
    *,
    direction: performance_gate.Direction = "lower",
    relative_mad: float = 0.0,
) -> performance_gate.Metric:
    domain, metric = name.split(".", 1)
    return performance_gate.Metric(
        domain=domain,
        metric=metric,
        unit="ns/op",
        direction=direction,
        batch_iterations=300_000,
        samples=[value] * 10,
        median=value,
        p95=value,
        maximum=value,
        relative_mad=relative_mad,
        calibrated=True,
    )


def _capture(
    role: performance_gate.Role,
    primary: float,
    *,
    protected: float = 100.0,
    primary_mad: float = 0.0,
) -> performance_gate.Capture:
    return performance_gate.Capture(
        role=role,
        command=[role],
        metrics={
            "query.reader.narrow": _metric("query.reader.narrow", primary, relative_mad=primary_mad),
            "query.reader.full": _metric("query.reader.full", protected),
        },
        phases={},
    )


def _manifest(
    *,
    kind: Literal["preservation", "optimization"] = "optimization",
    protected: Sequence[str] = (),
    floors: Sequence[performance_gate.HardFloor] = (),
) -> performance_gate.Manifest:
    return performance_gate.Manifest(
        schema_version=3,
        name="reader",
        kind=kind,
        measurement="records",
        domain="query",
        fixture=performance_gate.Fixture(identity="reader-benchmark", scale={"runs": 1, "points": 1_000_000}),
        baseline_command=["baseline"],
        candidate_command=["candidate"],
        primary="query.reader.narrow" if kind == "optimization" else None,
        protected=list(protected),
        hard_floors=list(floors),
        minimum_improvement=0.05,
    )


def _captures(
    baseline_first: float,
    candidate_first: float,
    candidate_second: float,
    baseline_second: float,
    *,
    protected: Sequence[float] = (100.0, 100.0, 100.0, 100.0),
) -> list[performance_gate.Capture]:
    values = (baseline_first, candidate_first, candidate_second, baseline_second)
    roles: tuple[performance_gate.Role, ...] = ("baseline", "candidate", "candidate", "baseline")
    return [
        _capture(role, value, protected=protected_value)
        for role, value, protected_value in zip(roles, values, protected, strict=True)
    ]


def test_parse_records_derives_statistics_from_ten_raw_samples() -> None:
    samples = [100.0] * 5 + [102.0] * 5

    parsed = performance_gate.parse_records(f"noise\n{_record(samples=samples)}\n")

    metric = parsed["query.reader.narrow"]
    assert metric["samples"] == samples
    assert metric["median"] == 101.0
    assert metric["p95"] == 102.0
    assert metric["maximum"] == 102.0
    assert metric["relative_mad"] == pytest.approx(1 / 101)
    assert metric["calibrated"]


@pytest.mark.parametrize(
    "updates, message",
    [
        ({"schema_version": 2}, "schema_version 3"),
        ({"record_type": "check"}, "schema_version 3"),
        ({"samples": [100.0] * 9}, "exactly 10"),
        ({"samples": [100.0] * 9 + [float("nan")]}, "finite"),
        ({"direction": "neutral"}, "supported unit and direction"),
        ({"batch_iterations": 0}, "positive"),
        ({"p95": 100.0}, "only raw v3 fields"),
    ],
)
def test_parse_records_rejects_invalid_records(updates: dict[str, object], message: str) -> None:
    record = json.loads(_record().removeprefix("SEEX_BENCH "))
    record.update(updates)

    with pytest.raises((TypeError, ValueError), match=message):
        performance_gate.parse_records("SEEX_BENCH " + json.dumps(record))


@pytest.mark.parametrize(
    "unit,batch_iterations,value,calibrated",
    [
        ("ns/op", 250_000, 100.0, True),
        ("ns/op", 249_999, 100.0, False),
        ("ns", 1, 25_000_000.0, True),
        ("points/s", 2_500, 100_000.0, True),
        ("points/s", 2_499, 100_000.0, False),
    ],
)
def test_timing_and_throughput_calibration(unit: str, batch_iterations: int, value: float, calibrated: bool) -> None:
    parsed = performance_gate.parse_records(_record(unit=unit, batch_iterations=batch_iterations, samples=[value] * 10))

    assert parsed["query.reader.narrow"]["calibrated"] is calibrated


def test_read_manifest_validates_fixture_and_defaults_target(tmp_path: pathlib.Path) -> None:
    path = tmp_path / "manifest.json"
    path.write_text(
        json.dumps(
            {
                "schema_version": 3,
                "name": "query preservation",
                "kind": "preservation",
                "measurement": "records",
                "fixture": {"identity": "query-benchmark", "scale": {"runs": 1, "points": 1_000_000}},
                "baseline_command": ["baseline", "--fixture", "prepared"],
                "candidate_command": ["candidate", "--fixture", "prepared"],
                "protected": ["query.reader.full"],
            }
        ),
        encoding="utf-8",
    )

    manifest = performance_gate.read_manifest(path)

    assert manifest["minimum_improvement"] == 0.05
    assert manifest["fixture"]["scale"]["points"] == 1_000_000
    assert manifest["primary"] is None
    assert manifest["domain"] == "viewer"


@pytest.mark.parametrize(
    "updates, message",
    [
        ({"schema_version": 2}, "schema_version 3"),
        ({"fixture": "query-benchmark"}, "fixture must be an object"),
        ({"fixture": {"identity": "query", "scale": {}}}, "positive integer dimensions"),
        ({"primary": "query.reader.full"}, "do not have a primary"),
        ({"protected": []}, "require protected metrics"),
        ({"baseline_command": "benchmark"}, "string array"),
        ({"domain": "storage"}, "domain is unsupported"),
    ],
)
def test_read_manifest_rejects_invalid_contract(
    tmp_path: pathlib.Path, updates: dict[str, object], message: str
) -> None:
    value: dict[str, object] = {
        "schema_version": 3,
        "name": "preservation",
        "kind": "preservation",
        "measurement": "records",
        "fixture": {"identity": "query-benchmark", "scale": {"points": 1}},
        "baseline_command": ["baseline"],
        "candidate_command": ["candidate"],
        "protected": ["query.reader.full"],
    }
    value.update(updates)
    path = tmp_path / "manifest.json"
    path.write_text(json.dumps(value), encoding="utf-8")

    with pytest.raises((TypeError, ValueError), match=message):
        performance_gate.read_manifest(path)


def test_baseline_registry_contains_only_scoped_fields() -> None:
    path = pathlib.Path(__file__).parents[1] / "docs/performance-baselines.json"
    registry = json.loads(path.read_text(encoding="utf-8"))

    assert set(registry) == {"schema_version", "benchmarks"}
    assert registry["schema_version"] == 3
    for benchmark in registry["benchmarks"].values():
        assert set(benchmark) == {"fixture", "revision", "stable_summary", "artifact_sha256"}
        assert set(benchmark["fixture"]) == {"identity", "scale"}


def test_preservation_enforces_combined_and_ordered_limits() -> None:
    manifest = _manifest(kind="preservation", protected=["query.reader.full"])
    accepted = _captures(100.0, 100.0, 100.0, 100.0, protected=(100.0, 102.0, 103.0, 100.0))
    pair_regression = _captures(100.0, 100.0, 100.0, 100.0, protected=(100.0, 106.0, 106.0, 100.0))

    assert performance_gate.compare(manifest, accepted)[0] == "pass"
    verdict, _, details = performance_gate.compare(manifest, pair_regression)
    assert verdict == "regression"
    assert "query.reader.full" in cast(dict[str, object], details["regressions"])


@pytest.mark.parametrize(
    "candidate_first,candidate_second,expected",
    [
        (94.0, 94.0, "pass"),
        (97.0, 97.0, "no_change"),
        (101.0, 101.0, "regression"),
        (106.0, 106.0, "regression"),
    ],
)
def test_optimization_verdicts(candidate_first: float, candidate_second: float, expected: str) -> None:
    verdict, _, _ = performance_gate.compare(_manifest(), _captures(100.0, candidate_first, candidate_second, 100.0))

    assert verdict == expected


def test_optimization_direction_conflict_is_inconclusive() -> None:
    verdict, reason, _ = performance_gate.compare(_manifest(), _captures(100.0, 99.0, 101.0, 100.0))

    assert verdict == "inconclusive"
    assert "directions disagreed" in reason


def test_zero_and_improvement_is_no_change() -> None:
    verdict, _, _ = performance_gate.compare(_manifest(), _captures(100.0, 100.0, 97.0, 100.0))

    assert verdict == "no_change"


def test_protected_direction_changes_within_limits_are_preserved() -> None:
    manifest = _manifest(protected=["query.reader.full"])
    captures = _captures(100.0, 90.0, 90.0, 100.0, protected=(100.0, 99.0, 101.0, 100.0))

    verdict, _, details = performance_gate.compare(manifest, captures)

    assert verdict == "pass"
    assert "query.reader.full" in details


def test_protected_regression_and_hard_floor_classify_optimization() -> None:
    floor = performance_gate.HardFloor(metric="query.reader.narrow", statistic="p95", operator="at_most", value=95.0)
    protected = _manifest(protected=["query.reader.full"])
    floored = _manifest(floors=[floor])

    assert (
        performance_gate.compare(
            protected,
            _captures(100.0, 90.0, 90.0, 100.0, protected=(100.0, 106.0, 106.0, 100.0)),
        )[0]
        == "regression"
    )
    assert performance_gate.compare(floored, _captures(100.0, 96.0, 96.0, 100.0))[0] == "regression"


def test_noise_and_missing_metrics_are_inconclusive() -> None:
    noisy = _captures(100.0, 90.0, 90.0, 100.0)
    noisy[1]["metrics"]["query.reader.narrow"]["relative_mad"] = 0.03
    missing = _captures(100.0, 90.0, 90.0, 100.0)
    del missing[0]["metrics"]["query.reader.narrow"]

    assert performance_gate.compare(_manifest(), noisy)[0] == "inconclusive"
    assert performance_gate.compare(_manifest(), missing)[0] == "inconclusive"


def test_cross_process_noise_is_inconclusive() -> None:
    captures = _captures(90.0, 80.0, 80.0, 110.0)

    verdict, reason, _ = performance_gate.compare(_manifest(), captures)

    assert verdict == "inconclusive"
    assert "noise" in reason


def test_metric_identity_must_not_change_between_captures() -> None:
    captures = _captures(100.0, 90.0, 90.0, 100.0)
    captures[1]["metrics"]["query.reader.narrow"]["direction"] = "higher"

    with pytest.raises(ValueError, match="unit or direction"):
        performance_gate.compare(_manifest(), captures)


def test_run_gate_uses_abba_and_checkpoints_each_capture(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path
) -> None:
    calls: list[performance_gate.Role] = []
    checkpoint_sizes: list[int] = []
    atomic_write = performance_gate._atomic_write

    def fake_capture(
        role: performance_gate.Role, manifest: performance_gate.Manifest, timeout: float
    ) -> performance_gate.Capture:
        del manifest
        assert timeout > 0
        calls.append(role)
        return _capture(role, 100.0 if role == "baseline" else 94.0)

    def record_checkpoint(path: pathlib.Path, result: performance_gate.Result) -> None:
        checkpoint_sizes.append(len(result["captures"]))
        atomic_write(path, result)

    monkeypatch.setattr(performance_gate, "_capture", fake_capture)
    monkeypatch.setattr(performance_gate, "_environment", dict)
    monkeypatch.setattr(performance_gate, "_atomic_write", record_checkpoint)
    output = tmp_path / "result.json"

    result = performance_gate.run_gate(_manifest(), output)

    checkpoint = json.loads(output.read_text(encoding="utf-8"))
    assert calls == ["baseline", "candidate", "candidate", "baseline"]
    assert checkpoint_sizes == [0, 1, 2, 3, 4, 4]
    assert result["verdict"] == "pass"
    assert checkpoint["verdict"] == "pass"
    assert len(checkpoint["captures"]) == 4
    assert not output.with_suffix(".json.tmp").exists()


def test_timeout_preserves_completed_checkpoint(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    calls = 0

    def fake_capture(
        role: performance_gate.Role, manifest: performance_gate.Manifest, timeout: float
    ) -> performance_gate.Capture:
        nonlocal calls
        del manifest
        calls += 1
        if calls == 2:
            raise subprocess.TimeoutExpired(role, timeout)
        return _capture(role, 100.0)

    monkeypatch.setattr(performance_gate, "_capture", fake_capture)
    monkeypatch.setattr(performance_gate, "_environment", dict)
    output = tmp_path / "result.json"

    with pytest.raises(subprocess.TimeoutExpired):
        performance_gate.run_gate(_manifest(), output)

    checkpoint = json.loads(output.read_text(encoding="utf-8"))
    assert checkpoint["verdict"] == "error"
    assert len(checkpoint["captures"]) == 1


@pytest.mark.parametrize("verdict", ["pass", "no_change", "inconclusive", "regression"])
def test_main_does_not_block_for_completed_comparisons(
    monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path, verdict: str
) -> None:
    manifest = _manifest()
    result = performance_gate.Result(
        schema_version=3,
        record_type="gate_result",
        manifest=manifest,
        environment={},
        execution_order=["baseline", "candidate", "candidate", "baseline"],
        captures=[],
        verdict=cast(performance_gate.Verdict, verdict),
        reason="test",
        comparisons={},
    )
    monkeypatch.setattr(performance_gate, "read_manifest", lambda path: manifest)
    monkeypatch.setattr(performance_gate, "run_gate", lambda loaded, output: result)

    assert (
        performance_gate.main(
            ["run", "--manifest", str(tmp_path / "manifest.json"), "--output", str(tmp_path / "result.json")]
        )
        == 0
    )


def test_cli_returns_four_for_tool_errors(tmp_path: pathlib.Path) -> None:
    manifest = tmp_path / "invalid.json"
    manifest.write_text("{}", encoding="utf-8")

    completed = subprocess.run(
        [
            sys.executable,
            str(pathlib.Path(performance_gate.__file__)),
            "run",
            "--manifest",
            str(manifest),
            "--output",
            str(tmp_path / "result.json"),
        ],
        check=False,
        capture_output=True,
        text=True,
    )

    assert completed.returncode == 4
    assert "schema_version 3" in completed.stderr


class _InterruptedProcess:
    pid = 42
    returncode: int | None = None

    def __init__(self) -> None:
        self.terminated = False

    def communicate(self, timeout: float) -> tuple[str, None]:
        del timeout
        raise KeyboardInterrupt

    def poll(self) -> int | None:
        return self.returncode

    def terminate(self) -> None:
        self.terminated = True
        self.returncode = -15

    def wait(self, timeout: float | None = None) -> int:
        del timeout
        assert self.returncode is not None
        return self.returncode


def test_sigint_stops_active_process_group(monkeypatch: pytest.MonkeyPatch) -> None:
    process = _InterruptedProcess()
    monkeypatch.setattr(performance_gate.subprocess, "Popen", lambda *args, **kwargs: process)
    monkeypatch.setattr(
        performance_gate.os,
        "killpg",
        lambda pid, signal_number: process.terminate(),
    )

    with pytest.raises(KeyboardInterrupt):
        performance_gate._run_output(["benchmark"], 1.0)

    assert process.terminated


def test_timeout_stops_workload_descendants(tmp_path: pathlib.Path) -> None:
    grandchild_pid = tmp_path / "grandchild.pid"
    command = [
        sys.executable,
        "-c",
        (
            "import pathlib, subprocess, sys, time; "
            "child = subprocess.Popen(['sleep', '30']); "
            "pathlib.Path(sys.argv[1]).write_text(str(child.pid)); "
            "time.sleep(30)"
        ),
        str(grandchild_pid),
    ]

    with pytest.raises(subprocess.TimeoutExpired):
        performance_gate._run_output(command, 0.5)

    process_id = grandchild_pid.read_text(encoding="utf-8")
    state = subprocess.run(
        ["ps", "-o", "stat=", "-p", process_id],
        check=False,
        capture_output=True,
        text=True,
    ).stdout.strip()
    assert not state or state.startswith("Z")


def test_rss_sampler_records_compact_workload_phases() -> None:
    command = [
        sys.executable,
        "-c",
        (
            "import time; "
            "print('SEEX_RSS_PHASE warm', flush=True); time.sleep(.06); "
            "print('SEEX_RSS_PHASE cycles_done', flush=True); time.sleep(.06); "
            "print('SEEX_RSS_PHASE final', flush=True); time.sleep(.06)"
        ),
    ]

    metrics, phases = performance_gate._sample_rss(command, 2.0)

    assert set(phases) == {"warm", "cycles_done", "final"}
    assert metrics["viewer.rss.peak"]["median"] > 0
    assert metrics["viewer.rss.final"]["median"] > 0


def test_rss_sampler_attributes_metrics_to_manifest_domain() -> None:
    command = [
        sys.executable,
        "-c",
        (
            "import time; "
            "print('SEEX_RSS_PHASE warm', flush=True); time.sleep(.06); "
            "print('SEEX_RSS_PHASE cycles_done', flush=True); time.sleep(.06); "
            "print('SEEX_RSS_PHASE final', flush=True); time.sleep(.06)"
        ),
    ]

    metrics, _ = performance_gate._sample_rss(command, 2.0, "reporting")

    assert set(metrics) == {
        "reporting.rss.warm",
        "reporting.rss.peak",
        "reporting.rss.final",
    }
    assert {metric["domain"] for metric in metrics.values()} == {"reporting"}


def test_rss_lookup_converts_child_exit_race(monkeypatch: pytest.MonkeyPatch) -> None:
    completed = subprocess.CompletedProcess(["ps"], 1, stdout="", stderr="missing")
    monkeypatch.setattr(performance_gate.subprocess, "run", lambda *args, **kwargs: completed)

    with pytest.raises(ProcessLookupError, match="no RSS sample"):
        performance_gate._rss_bytes(42)

    zero = subprocess.CompletedProcess(["ps"], 0, stdout="0\n", stderr="")
    monkeypatch.setattr(performance_gate.subprocess, "run", lambda *args, **kwargs: zero)
    with pytest.raises(ProcessLookupError, match="no live RSS sample"):
        performance_gate._rss_bytes(42)
