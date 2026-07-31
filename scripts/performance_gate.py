"""Capture and compare Seex Viewer performance records."""

from __future__ import annotations

import argparse
import datetime
import itertools
import json
import pathlib
import platform
import queue
import shlex
import subprocess
import sys
import threading
import time
from collections.abc import Sequence
from typing import TypedDict, cast

_PREFIX = "SEEX_PERF "
_REQUIRED_PAIRS = 7
_RSS_TREND_NOISE_FLOOR_BYTES = 1024 * 1024


class MetricSample(TypedDict):
    """One machine-readable metric emitted by a release benchmark."""

    metric: str
    unit: str
    batch_iterations: int
    samples: int
    raw_samples: list[float]
    p50: float
    p95: float
    max_batch: float
    max_single: float
    relative_mad: float
    reliable: bool


class Capture(TypedDict):
    """Repeated benchmark output plus its execution environment."""

    schema_version: int
    environment: dict[str, str | bool]
    command: list[str]
    runs: list[dict[str, MetricSample]]


class Verdict(TypedDict):
    """Comparison result consumed by the performance optimization loop."""

    verdict: str
    primary: str
    median_improvement: float
    improved_pairs: int
    regressions: dict[str, float]
    reason: str


class RssResult(TypedDict):
    """RSS stability result for a phase-marked child process."""

    schema_version: int
    samples: list[int]
    warm_index: int
    trend_end_index: int
    final_index: int
    warm_rss_bytes: int
    peak_rss_bytes: int
    final_rss_bytes: int
    allowed_final_rss_bytes: int
    monotonic_growth: bool
    verdict: str


def parse_output(output: str) -> dict[str, MetricSample]:
    """Parses machine records from one benchmark invocation."""
    metrics: dict[str, MetricSample] = {}
    for line in output.splitlines():
        if not line.startswith(_PREFIX):
            continue
        decoded = json.loads(line.removeprefix(_PREFIX))
        if not isinstance(decoded, dict):
            raise TypeError("SEEX_PERF record must be a JSON object")
        metric = decoded.get("metric")
        unit = decoded.get("unit")
        batch_iterations = decoded.get("batch_iterations")
        sample_count = decoded.get("samples")
        raw_samples = decoded.get("raw_samples")
        reliable = decoded.get("reliable")
        if not isinstance(metric, str) or not metric:
            raise ValueError("SEEX_PERF metric must be a non-empty string")
        if not isinstance(unit, str) or not unit:
            raise ValueError("SEEX_PERF unit must be a non-empty string")
        if not isinstance(batch_iterations, int) or isinstance(batch_iterations, bool) or batch_iterations <= 0:
            raise ValueError("SEEX_PERF batch_iterations must be positive")
        if not isinstance(sample_count, int) or isinstance(sample_count, bool) or sample_count <= 0:
            raise ValueError("SEEX_PERF samples must be positive")
        if not isinstance(raw_samples, list) or len(raw_samples) != sample_count:
            raise ValueError("SEEX_PERF raw_samples must match samples")
        numbers: dict[str, float] = {}
        for field in ("p50", "p95", "max_batch", "max_single", "relative_mad"):
            value = decoded.get(field)
            if isinstance(value, bool) or not isinstance(value, int | float) or value < 0:
                raise ValueError(f"SEEX_PERF {field} must be non-negative: {value!r}")
            numbers[field] = float(value)
        if numbers["p50"] <= 0:
            raise ValueError(f"SEEX_PERF p50 must be positive: {numbers['p50']!r}")
        parsed_samples = [
            float(value) for value in raw_samples if isinstance(value, int | float) and not isinstance(value, bool)
        ]
        if len(parsed_samples) != sample_count:
            raise TypeError("SEEX_PERF raw_samples must contain only numbers")
        if not isinstance(reliable, bool):
            raise TypeError("SEEX_PERF reliable must be boolean")
        metrics[metric] = MetricSample(
            metric=metric,
            unit=unit,
            batch_iterations=batch_iterations,
            samples=sample_count,
            raw_samples=parsed_samples,
            p50=numbers["p50"],
            p95=numbers["p95"],
            max_batch=numbers["max_batch"],
            max_single=numbers["max_single"],
            relative_mad=numbers["relative_mad"],
            reliable=reliable,
        )
    if not metrics:
        raise ValueError("benchmark output contained no SEEX_PERF records")
    return metrics


def _command_output(command: Sequence[str]) -> str:
    completed = subprocess.run(command, check=True, capture_output=True, text=True)
    return completed.stdout + completed.stderr


def _environment() -> dict[str, str | bool]:
    revision = _command_output(("git", "rev-parse", "HEAD")).strip()
    dirty = bool(_command_output(("git", "status", "--porcelain")).strip())
    return {
        "captured_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "rustc": _command_output(("rustc", "--version")).strip(),
        "git_revision": revision,
        "dirty": dirty,
    }


def capture(command: Sequence[str], repeats: int = _REQUIRED_PAIRS) -> Capture:
    """Runs a benchmark repeatedly and returns its versioned capture."""
    if repeats < _REQUIRED_PAIRS:
        raise ValueError(f"performance capture requires at least {_REQUIRED_PAIRS} runs")
    runs = [parse_output(_command_output(command)) for _ in range(repeats)]
    return Capture(
        schema_version=1,
        environment=_environment(),
        command=list(command),
        runs=runs,
    )


def _samples(capture_record: Capture, metric: str) -> list[MetricSample]:
    samples: list[MetricSample] = []
    for run in capture_record["runs"]:
        try:
            samples.append(run[metric])
        except KeyError as error:
            raise ValueError(f"capture is missing metric {metric!r}") from error
    if len(samples) < _REQUIRED_PAIRS:
        raise ValueError(f"metric {metric!r} has fewer than {_REQUIRED_PAIRS} samples")
    return samples


def _median(values: Sequence[float]) -> float:
    ordered = sorted(values)
    return ordered[len(ordered) // 2]


def compare_captures(
    baseline: Capture,
    candidate: Capture,
    primary: str,
    protected: Sequence[str] = (),
) -> Verdict:
    """Applies the 5%, six-of-seven, and 3% regression policy."""
    baseline_primary = _samples(baseline, primary)
    candidate_primary = _samples(candidate, primary)
    paired = list(zip(baseline_primary, candidate_primary, strict=True))
    if any(not before["reliable"] or not after["reliable"] for before, after in paired):
        return Verdict(
            verdict="no_change",
            primary=primary,
            median_improvement=0.0,
            improved_pairs=0,
            regressions={},
            reason="primary metric contains unreliable samples",
        )
    improvements = [(before["p50"] - after["p50"]) / before["p50"] for before, after in paired]
    median_improvement = _median(improvements)
    improved_pairs = sum(improvement > 0 for improvement in improvements)
    regressions: dict[str, float] = {}
    for metric in protected:
        before = _samples(baseline, metric)
        after = _samples(candidate, metric)
        if any(not sample["reliable"] for sample in [*before, *after]):
            continue
        regression = (
            _median([sample["p50"] for sample in after]) / _median([sample["p50"] for sample in before])
        ) - 1.0
        if regression > 0.03:
            regressions[metric] = regression
    if regressions:
        verdict = "regression"
        reason = "one or more protected metrics regressed by more than 3%"
    elif improved_pairs < 6 or median_improvement < 0.05:
        verdict = "no_change"
        reason = "primary metric did not improve by 5% in six of seven pairs"
    else:
        verdict = "pass"
        reason = "primary metric improved without a protected regression"
    return Verdict(
        verdict=verdict,
        primary=primary,
        median_improvement=median_improvement,
        improved_pairs=improved_pairs,
        regressions=regressions,
        reason=reason,
    )


def _read_capture(path: pathlib.Path) -> Capture:
    return cast(Capture, json.loads(path.read_text(encoding="utf-8")))


def _write_json(path: pathlib.Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def evaluate_rss(
    samples: Sequence[int],
    warm_index: int,
    final_index: int,
    *,
    trend_end_index: int | None = None,
) -> RssResult:
    """Evaluates the warm/peak/final RSS contract."""
    if trend_end_index is None:
        trend_end_index = final_index
    if not samples or any(sample <= 0 for sample in samples):
        raise ValueError("RSS samples must be positive")
    if not 0 <= warm_index <= trend_end_index <= final_index < len(samples):
        raise ValueError("RSS phase indexes do not match the samples")
    warm = samples[warm_index]
    final = samples[final_index]
    trend = samples[warm_index : trend_end_index + 1]
    meaningful_growth = trend[-1] - trend[0] > max(
        trend[0] // 100,
        _RSS_TREND_NOISE_FLOOR_BYTES,
    )
    monotonic = (
        len(trend) >= 8 and meaningful_growth and all(before <= after for before, after in itertools.pairwise(trend))
    )
    allowed = max(int(warm * 1.05), warm + 32 * 1024 * 1024)
    verdict = "pass" if final <= allowed and not monotonic else "regression"
    return RssResult(
        schema_version=1,
        samples=list(samples),
        warm_index=warm_index,
        trend_end_index=trend_end_index,
        final_index=final_index,
        warm_rss_bytes=warm,
        peak_rss_bytes=max(samples),
        final_rss_bytes=final,
        allowed_final_rss_bytes=allowed,
        monotonic_growth=monotonic,
        verdict=verdict,
    )


def _rss_bytes(process_id: int) -> int:
    output = _command_output(("ps", "-o", "rss=", "-p", str(process_id))).strip()
    if not output:
        raise ProcessLookupError(f"process {process_id} has no RSS sample")
    rss = int(output) * 1024
    if rss <= 0:
        raise ProcessLookupError(f"process {process_id} has no positive RSS sample")
    return rss


def sample_rss(command: Sequence[str], interval: float) -> RssResult:
    """Samples a child that emits warm/cycles_done/final phase markers."""
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if process.stdout is None:
        raise RuntimeError("RSS child stdout pipe was not created")
    stdout = process.stdout
    lines: queue.Queue[str | None] = queue.Queue()

    def read_output() -> None:
        for line in stdout:
            lines.put(line)
        lines.put(None)

    reader = threading.Thread(target=read_output, name="seex-rss-output", daemon=True)
    reader.start()
    samples: list[int] = []
    warm_index: int | None = None
    trend_end_index: int | None = None
    final_seen = False
    output_closed = False
    while process.poll() is None or not output_closed:
        while True:
            try:
                line = lines.get_nowait()
            except queue.Empty:
                break
            if line is None:
                output_closed = True
                break
            sys.stdout.write(line)
            if "SEEX_RSS_PHASE warm" in line:
                warm_index = len(samples)
            elif "SEEX_RSS_PHASE cycles_done" in line:
                trend_end_index = len(samples) - 1
            elif "SEEX_RSS_PHASE final" in line:
                final_seen = True
        if process.poll() is None:
            try:
                samples.append(_rss_bytes(process.pid))
            except ProcessLookupError:
                if process.poll() is None:
                    raise
            time.sleep(interval)
    reader.join()
    if process.returncode != 0:
        raise subprocess.CalledProcessError(process.returncode, command)
    if warm_index is None or trend_end_index is None or not final_seen:
        raise ValueError("RSS child did not emit warm, cycles_done, and final phase markers")
    return evaluate_rss(
        samples,
        warm_index,
        len(samples) - 1,
        trend_end_index=trend_end_index,
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    capture_parser = commands.add_parser("capture")
    capture_parser.add_argument("--output", type=pathlib.Path, required=True)
    capture_parser.add_argument("--runs", type=int, default=_REQUIRED_PAIRS)
    capture_parser.add_argument("command", nargs=argparse.REMAINDER)
    compare_parser = commands.add_parser("compare")
    compare_parser.add_argument("--baseline", type=pathlib.Path, required=True)
    compare_parser.add_argument("--candidate", type=pathlib.Path, required=True)
    compare_parser.add_argument("--primary", required=True)
    compare_parser.add_argument("--protected", action="append", default=[])
    pair_parser = commands.add_parser("pair")
    pair_parser.add_argument("--baseline-command", required=True)
    pair_parser.add_argument("--candidate-command", required=True)
    pair_parser.add_argument("--primary", required=True)
    pair_parser.add_argument("--protected", action="append", default=[])
    pair_parser.add_argument("--output", type=pathlib.Path, required=True)
    rss_parser = commands.add_parser("rss")
    rss_parser.add_argument("--output", type=pathlib.Path, required=True)
    rss_parser.add_argument("--interval", type=float, default=0.05)
    rss_parser.add_argument("command", nargs=argparse.REMAINDER)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Runs capture or comparison and returns a stable process status."""
    args = _parser().parse_args(argv)
    command: list[str] = []
    if args.action in {"capture", "rss"}:
        command = args.command[1:] if args.command[:1] == ["--"] else args.command
        if not command:
            raise ValueError(f"{args.action} requires a command after --")
    if args.action == "capture":
        _write_json(args.output, capture(command, args.runs))
        return 0
    if args.action == "rss":
        result = sample_rss(command, args.interval)
        _write_json(args.output, result)
        return 0 if result["verdict"] == "pass" else 3
    if args.action == "pair":
        baseline_runs: list[dict[str, MetricSample]] = []
        candidate_runs: list[dict[str, MetricSample]] = []
        baseline_command = shlex.split(args.baseline_command)
        candidate_command = shlex.split(args.candidate_command)
        for _ in range(_REQUIRED_PAIRS):
            baseline_runs.append(parse_output(_command_output(baseline_command)))
            candidate_runs.append(parse_output(_command_output(candidate_command)))
        environment = _environment()
        baseline = Capture(
            schema_version=1,
            environment=environment,
            command=baseline_command,
            runs=baseline_runs,
        )
        candidate = Capture(
            schema_version=1,
            environment=environment,
            command=candidate_command,
            runs=candidate_runs,
        )
        verdict = compare_captures(baseline, candidate, args.primary, args.protected)
        _write_json(args.output, {"baseline": baseline, "candidate": candidate, "verdict": verdict})
    else:
        verdict = compare_captures(
            _read_capture(args.baseline),
            _read_capture(args.candidate),
            args.primary,
            args.protected,
        )
        print(json.dumps(verdict, sort_keys=True))
    return {"pass": 0, "no_change": 2, "regression": 3}[verdict["verdict"]]


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, TypeError, ValueError, json.JSONDecodeError, subprocess.CalledProcessError) as error:
        print(f"viewer performance gate failed: {error}", file=sys.stderr)
        sys.exit(4)
