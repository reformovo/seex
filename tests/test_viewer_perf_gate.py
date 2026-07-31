"""Verify the Viewer performance comparison contract."""

import pathlib

import pytest

from scripts import viewer_perf_gate


def _capture(p50: float, *, reliable: bool = True, protected: float = 100.0) -> viewer_perf_gate.Capture:
    def sample(metric: str, value: float, metric_reliable: bool = True) -> viewer_perf_gate.MetricSample:
        return viewer_perf_gate.MetricSample(
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

    runs: list[dict[str, viewer_perf_gate.MetricSample]] = []
    for _ in range(7):
        runs.append({"primary": sample("primary", p50, reliable), "protected": sample("protected", protected)})
    return viewer_perf_gate.Capture(
        schema_version=1,
        environment={},
        command=["benchmark"],
        runs=runs,
    )


def test_parse_output_extracts_machine_records() -> None:
    parsed = viewer_perf_gate.parse_output(
        'noise\nSEEX_PERF {"metric":"path","unit":"ns/op","batch_iterations":2,'
        '"samples":2,"raw_samples":[40.0,45.0],"p50":42.5,"p95":45.0,'
        '"max_batch":45.0,"max_single":46.0,"relative_mad":0.01,"reliable":true}\n'
    )

    assert parsed["path"]["raw_samples"] == [40.0, 45.0]
    assert parsed["path"]["p95"] == 45.0


def test_compare_accepts_consistent_improvement() -> None:
    verdict = viewer_perf_gate.compare_captures(
        _capture(100.0),
        _capture(90.0),
        "primary",
        ["protected"],
    )

    assert verdict["verdict"] == "pass"
    assert verdict["improved_pairs"] == 7


def test_compare_rejects_no_change_and_unreliable_samples() -> None:
    unchanged = viewer_perf_gate.compare_captures(_capture(100.0), _capture(96.0), "primary")
    unreliable = viewer_perf_gate.compare_captures(
        _capture(100.0),
        _capture(80.0, reliable=False),
        "primary",
    )

    assert unchanged["verdict"] == "no_change"
    assert unreliable["reason"] == "primary metric contains unreliable samples"


def test_compare_rejects_protected_regression() -> None:
    verdict = viewer_perf_gate.compare_captures(
        _capture(100.0),
        _capture(90.0, protected=104.0),
        "primary",
        ["protected"],
    )

    assert verdict["verdict"] == "regression"
    assert verdict["regressions"]["protected"] > 0.03


def test_capture_cli_strips_remainder_separator(monkeypatch: pytest.MonkeyPatch, tmp_path: pathlib.Path) -> None:
    captured: list[list[str]] = []

    def fake_capture(command: list[str], repeats: int) -> viewer_perf_gate.Capture:
        captured.append(command)
        return viewer_perf_gate.Capture(schema_version=1, environment={}, command=command, runs=[])

    monkeypatch.setattr(viewer_perf_gate, "capture", fake_capture)
    output = tmp_path / "capture.json"

    assert viewer_perf_gate.main(["capture", "--output", str(output), "--", "benchmark"]) == 0
    assert captured == [["benchmark"]]


def test_rss_gate_accepts_stable_memory_and_rejects_growth() -> None:
    stable = viewer_perf_gate.evaluate_rss([100_000_000, 110_000_000, 105_000_000, 103_000_000], 0, 3)
    flat = viewer_perf_gate.evaluate_rss([100_000_000] * 10, 0, 9)
    lazy_page = viewer_perf_gate.evaluate_rss([100_000_000] * 5 + [100_016_384] * 5, 0, 9)
    growing = viewer_perf_gate.evaluate_rss([100_000_000 + index * 2_000_000 for index in range(10)], 0, 9)

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
    result = viewer_perf_gate.evaluate_rss(
        [100_000_000] * 10 + [110_000_000],
        0,
        10,
        trend_end_index=9,
    )

    assert result["verdict"] == "pass"
    assert result["trend_end_index"] == 9
    assert result["final_rss_bytes"] == 110_000_000

    with pytest.raises(ValueError, match="positive"):
        viewer_perf_gate.evaluate_rss([100_000_000, 0], 0, 1)
