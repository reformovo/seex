"""Verify the Viewer performance comparison contract."""

import pathlib

import pytest

from scripts import performance_gate


def _capture(p50: float, *, reliable: bool = True, protected: float = 100.0) -> performance_gate.Capture:
    def sample(metric: str, value: float, metric_reliable: bool = True) -> performance_gate.MetricSample:
        return performance_gate.MetricSample(
            metric=metric,
            unit="ns/op",
            batch_iterations=1,
            samples=1,
            raw_samples=[value],
            p50=value,
            p95=value,
            max_batch=value,
            max_single=value,
            relative_mad=0.0,
            reliable=metric_reliable,
        )

    runs: list[dict[str, performance_gate.MetricSample]] = []
    for _ in range(7):
        runs.append({"primary": sample("primary", p50, reliable), "protected": sample("protected", protected)})
    return performance_gate.Capture(
        schema_version=1,
        environment={},
        command=["benchmark"],
        runs=runs,
    )


def test_parse_output_extracts_machine_records() -> None:
    parsed = performance_gate.parse_output(
        'noise\nSEEX_PERF {"metric":"path","unit":"ns/op","batch_iterations":2,'
        '"samples":2,"raw_samples":[40.0,45.0],"p50":42.5,"p95":45.0,'
        '"max_batch":45.0,"max_single":46.0,"relative_mad":0.01,"reliable":true}\n'
    )

    assert parsed["path"]["raw_samples"] == [40.0, 45.0]
    assert parsed["path"]["p95"] == 45.0


def _v2_metric(**updates: object) -> str:
    record: dict[str, object] = {
        "schema_version": 2,
        "record_type": "metric",
        "domain": "query",
        "metric": "duckdb.step.full",
        "unit": "ns/op",
        "direction": "lower",
        "batch_iterations": 2,
        "samples": 3,
        "raw_samples": [10.0, 11.0, 12.0],
        "mad": 1.0,
        "relative_mad": 1.0 / 11.0,
        "p50": 11.0,
        "p95": 12.0,
        "max": 12.0,
        "reliable": False,
    }
    record.update(updates)
    return "SEEX_PERF " + __import__("json").dumps(record)


def test_parse_v2_output_validates_metrics_and_checks() -> None:
    check = (
        'SEEX_PERF {"schema_version":2,"record_type":"check","domain":"query",'
        '"check":"parity","passed":true,"detail":"matched"}'
    )

    parsed = performance_gate.parse_v2_output(f"{_v2_metric()}\n{check}")

    assert parsed["metrics"]["query.duckdb.step.full"]["batch_iterations"] == 2
    assert parsed["checks"] == [performance_gate.V2Check(domain="query", check="parity", passed=True, detail="matched")]


@pytest.mark.parametrize(
    "updates, message",
    [
        ({"schema_version": 1}, "schema_version 2"),
        ({"unit": "milliseconds"}, "unsupported"),
        ({"raw_samples": [10.0]}, "must match"),
        ({"raw_samples": [10.0, float("nan"), 12.0]}, "finite"),
    ],
)
def test_parse_v2_output_rejects_invalid_deciding_records(updates: dict[str, object], message: str) -> None:
    with pytest.raises((TypeError, ValueError), match=message):
        performance_gate.parse_v2_output(_v2_metric(**updates))


def _v2_runs(value: float, **updates: object) -> list[performance_gate.V2Output]:
    values: dict[str, object] = {
        "raw_samples": [value] * 3,
        "p50": value,
        "relative_mad": 0.0,
        "reliable": True,
    }
    values.update(updates)
    return [performance_gate.parse_v2_output(_v2_metric(**values)) for _ in range(7)]


def test_v2_optimization_accepts_six_of_seven_and_five_percent() -> None:
    candidate = _v2_runs(94.0)
    candidate[-1] = _v2_runs(101.0)[0]

    verdict = performance_gate.compare_v2_captures(
        _v2_runs(100.0), candidate, "optimization", primary="query.duckdb.step.full"
    )

    assert verdict["verdict"] == "pass"
    assert verdict["improved_pairs"] == 6


def test_v2_migration_checks_hard_floors_and_protected_regressions() -> None:
    floor = performance_gate.HardFloor(
        metric="query.duckdb.step.full", statistic="p95", operator="at_most", value=110.0
    )
    candidate = _v2_runs(104.0, p95=111.0)
    candidate[0]["checks"].append(
        performance_gate.V2Check(domain="query", check="parity", passed=False, detail="mismatch")
    )

    verdict = performance_gate.compare_v2_captures(
        _v2_runs(100.0),
        candidate,
        "migration",
        primary=None,
        protected=["query.duckdb.step.full"],
        hard_floors=[floor],
    )

    assert verdict["verdict"] == "regression"
    assert verdict["failed_checks"] == ["query.parity"]
    assert verdict["failed_floors"] == ["query.duckdb.step.full"]
    assert verdict["regressions"]["query.duckdb.step.full"] > 0.03


def test_v2_unreliable_primary_is_no_change() -> None:
    candidate = _v2_runs(80.0, relative_mad=0.021, reliable=False)

    verdict = performance_gate.compare_v2_captures(
        _v2_runs(100.0), candidate, "optimization", primary="query.duckdb.step.full"
    )

    assert verdict["verdict"] == "no_change"
    assert verdict["unreliable"] == ["query.duckdb.step.full"]


def test_compare_accepts_consistent_improvement() -> None:
    verdict = performance_gate.compare_captures(
        _capture(100.0),
        _capture(90.0),
        "primary",
        ["protected"],
    )

    assert verdict["verdict"] == "pass"
    assert verdict["improved_pairs"] == 7


def test_compare_rejects_no_change_and_unreliable_samples() -> None:
    unchanged = performance_gate.compare_captures(_capture(100.0), _capture(96.0), "primary")
    unreliable = performance_gate.compare_captures(
        _capture(100.0),
        _capture(80.0, reliable=False),
        "primary",
    )

    assert unchanged["verdict"] == "no_change"
    assert unreliable["reason"] == "primary metric contains unreliable samples"


def test_compare_rejects_protected_regression() -> None:
    verdict = performance_gate.compare_captures(
        _capture(100.0),
        _capture(90.0, protected=104.0),
        "primary",
        ["protected"],
    )

    assert verdict["verdict"] == "regression"
    assert verdict["regressions"]["protected"] > 0.03


def test_capture_cli_strips_remainder_separator(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    captured: list[list[str]] = []

    def fake_capture(command: list[str], repeats: int) -> performance_gate.Capture:
        captured.append(command)
        return performance_gate.Capture(schema_version=1, environment={}, command=command, runs=[])

    monkeypatch.setattr(performance_gate, "capture", fake_capture)
    output = tmp_path / "capture.json"

    assert performance_gate.main(["capture", "--output", str(output), "--", "benchmark"]) == 0
    assert captured == [["benchmark"]]


def test_rss_gate_accepts_stable_memory_and_rejects_growth() -> None:
    stable = performance_gate.evaluate_rss([100_000_000, 110_000_000, 105_000_000, 103_000_000], 0, 3)
    flat = performance_gate.evaluate_rss([100_000_000] * 10, 0, 9)
    lazy_page = performance_gate.evaluate_rss([100_000_000] * 5 + [100_016_384] * 5, 0, 9)
    growing = performance_gate.evaluate_rss([100_000_000 + index * 2_000_000 for index in range(10)], 0, 9)

    assert stable["verdict"] == "pass"
    assert stable["samples"] == [100_000_000, 110_000_000, 105_000_000, 103_000_000]
    assert (stable["warm_index"], stable["final_index"]) == (0, 3)
    assert stable["peak_rss_bytes"] == 110_000_000
    assert flat["verdict"] == "pass"
    assert not flat["monotonic_growth"]
    assert lazy_page["verdict"] == "pass"
    assert not lazy_page["monotonic_growth"]
    assert growing["verdict"] == "regression"
    assert growing["monotonic_growth"]


def test_rss_trend_excludes_final_settle_allocation() -> None:
    result = performance_gate.evaluate_rss(
        [100_000_000] * 10 + [110_000_000],
        0,
        10,
        trend_end_index=9,
    )

    assert result["verdict"] == "pass"
    assert result["trend_end_index"] == 9
    assert result["final_rss_bytes"] == 110_000_000

    with pytest.raises(ValueError, match="positive"):
        performance_gate.evaluate_rss([100_000_000, 0], 0, 1)
