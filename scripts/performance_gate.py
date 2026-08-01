"""Run one bounded Seex performance comparison from a schema-v3 manifest."""

from __future__ import annotations

import argparse
import datetime
import json
import math
import os
import pathlib
import platform
import queue
import statistics
import subprocess
import sys
import threading
import time
from collections.abc import Sequence
from typing import Literal, TypedDict, cast

_PREFIX = "SEEX_BENCH "
_ORDER: tuple[Literal["baseline", "candidate"], ...] = (
    "baseline",
    "candidate",
    "candidate",
    "baseline",
)
_SAMPLES = 10
_MIN_SAMPLE_NS = 25_000_000
_MAX_GATE_SECONDS = 120.0
_MAX_RELATIVE_MAD = 0.02
_MAX_MEDIAN_REGRESSION = 0.03
_MAX_PAIR_REGRESSION = 0.05
_RSS_INTERVAL_SECONDS = 0.05
_DOMAINS = frozenset({"reporting", "query", "viewer"})
_DIRECTIONS = frozenset({"higher", "lower"})
_UNITS = frozenset({"bytes", "count", "ns", "ns/op", "points/s"})

Role = Literal["baseline", "candidate"]
Direction = Literal["higher", "lower"]
Verdict = Literal["running", "pass", "no_change", "regression", "inconclusive", "error"]


class Metric(TypedDict):
    """One validated benchmark metric and its derived summary."""

    domain: str
    metric: str
    unit: str
    direction: Direction
    batch_iterations: int
    samples: list[float]
    median: float
    p95: float
    maximum: float
    relative_mad: float
    calibrated: bool


class Capture(TypedDict):
    """Metrics from one fresh baseline or candidate process."""

    role: Role
    command: list[str]
    metrics: dict[str, Metric]
    phases: dict[str, int]


class HardFloor(TypedDict):
    """An absolute requirement for every candidate capture."""

    metric: str
    statistic: Literal["median", "p95", "maximum"]
    operator: Literal["at_least", "at_most"]
    value: float


class Fixture(TypedDict):
    """Stable identity and cardinality of a prepared benchmark fixture."""

    identity: str
    scale: dict[str, int]


class Manifest(TypedDict):
    """The complete input contract for one performance decision."""

    schema_version: Literal[3]
    name: str
    kind: Literal["preservation", "optimization"]
    measurement: Literal["records", "rss"]
    fixture: Fixture
    baseline_command: list[str]
    candidate_command: list[str]
    primary: str | None
    protected: list[str]
    hard_floors: list[HardFloor]
    minimum_improvement: float


class Result(TypedDict):
    """A checkpointable schema-v3 gate result."""

    schema_version: Literal[3]
    record_type: Literal["gate_result"]
    manifest: Manifest
    environment: dict[str, str | bool]
    execution_order: list[Role]
    captures: list[Capture]
    verdict: Verdict
    reason: str
    comparisons: dict[str, object]


def _number(value: object, field: str) -> float:
    if isinstance(value, bool) or not isinstance(value, int | float) or not math.isfinite(value):
        raise TypeError(f"{field} must be a finite number")
    return float(value)


def _relative_mad(values: Sequence[float]) -> float:
    median = statistics.median(values)
    if median == 0:
        return 0.0 if all(value == 0 for value in values) else math.inf
    return statistics.median(abs(value - median) for value in values) / abs(median)


def _calibrated(unit: str, batch_iterations: int, samples: Sequence[float]) -> bool:
    if unit == "ns/op":
        elapsed = [sample * batch_iterations for sample in samples]
    elif unit == "points/s":
        elapsed = [batch_iterations * 1_000_000_000 / sample for sample in samples]
    elif unit == "ns":
        elapsed = list(samples)
    else:
        return True
    return all(sample >= _MIN_SAMPLE_NS for sample in elapsed)


def parse_records(output: str) -> dict[str, Metric]:
    """Parse strict raw schema-v3 benchmark records from one process."""
    metrics: dict[str, Metric] = {}
    for line in output.splitlines():
        if not line.startswith(_PREFIX):
            continue
        record = json.loads(line.removeprefix(_PREFIX))
        if not isinstance(record, dict) or record.get("schema_version") != 3 or record.get("record_type") != "metric":
            raise ValueError("SEEX_BENCH records require schema_version 3")
        domain = record.get("domain")
        metric = record.get("metric")
        unit = record.get("unit")
        direction = record.get("direction")
        batch_iterations = record.get("batch_iterations")
        raw_samples = record.get("samples")
        if domain not in _DOMAINS or not isinstance(metric, str) or not metric:
            raise ValueError("SEEX_BENCH requires a supported domain and metric")
        if unit not in _UNITS or direction not in _DIRECTIONS:
            raise ValueError("SEEX_BENCH requires a supported unit and direction")
        if not isinstance(batch_iterations, int) or isinstance(batch_iterations, bool) or batch_iterations <= 0:
            raise ValueError("SEEX_BENCH batch_iterations must be positive")
        if not isinstance(raw_samples, list) or len(raw_samples) != _SAMPLES:
            raise ValueError(f"SEEX_BENCH requires exactly {_SAMPLES} samples")
        samples = [_number(value, "SEEX_BENCH sample") for value in raw_samples]
        if any(value <= 0 for value in samples):
            raise ValueError("SEEX_BENCH samples must be positive")
        ordered = sorted(samples)
        key = f"{domain}.{metric}"
        if key in metrics:
            raise ValueError(f"duplicate SEEX_BENCH metric {key!r}")
        metrics[key] = Metric(
            domain=cast(str, domain),
            metric=metric,
            unit=cast(str, unit),
            direction=cast(Direction, direction),
            batch_iterations=batch_iterations,
            samples=samples,
            median=float(statistics.median(ordered)),
            p95=ordered[math.ceil(len(ordered) * 0.95) - 1],
            maximum=ordered[-1],
            relative_mad=_relative_mad(samples),
            calibrated=_calibrated(cast(str, unit), batch_iterations, samples),
        )
    if not metrics:
        raise ValueError("benchmark output contained no SEEX_BENCH records")
    return metrics


def _command(value: object, field: str) -> list[str]:
    if not isinstance(value, list) or not value or any(not isinstance(item, str) or not item for item in value):
        raise TypeError(f"{field} must be a non-empty string array")
    return cast(list[str], value)


def _fixture(value: object) -> Fixture:
    if not isinstance(value, dict):
        raise TypeError("manifest fixture must be an object")
    identity = value.get("identity")
    scale = value.get("scale")
    if not isinstance(identity, str) or not identity.strip():
        raise ValueError("manifest fixture identity is required")
    if (
        not isinstance(scale, dict)
        or not scale
        or any(
            not isinstance(key, str) or not key or not isinstance(item, int) or isinstance(item, bool) or item <= 0
            for key, item in scale.items()
        )
    ):
        raise TypeError("manifest fixture scale must contain positive integer dimensions")
    return Fixture(identity=identity, scale=cast(dict[str, int], scale))


def read_manifest(path: pathlib.Path) -> Manifest:
    """Read and validate the only supported performance manifest version."""
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or value.get("schema_version") != 3:
        raise ValueError("performance manifests require schema_version 3")
    kind = value.get("kind")
    measurement = value.get("measurement")
    primary = value.get("primary")
    protected = value.get("protected", [])
    floors = value.get("hard_floors", [])
    improvement = _number(value.get("minimum_improvement", 0.05), "minimum_improvement")
    if kind not in {"preservation", "optimization"} or measurement not in {"records", "rss"}:
        raise ValueError("manifest kind or measurement is unsupported")
    if not isinstance(value.get("name"), str) or not value["name"].strip():
        raise ValueError("manifest name is required")
    if kind == "optimization" and (not isinstance(primary, str) or not primary):
        raise ValueError("optimization manifests require a primary metric")
    if kind == "preservation" and primary is not None:
        raise ValueError("preservation manifests do not have a primary metric")
    if primary is not None and not isinstance(primary, str):
        raise TypeError("manifest primary must be a string or null")
    if not isinstance(protected, list) or any(not isinstance(item, str) or not item for item in protected):
        raise TypeError("manifest protected must be a string array")
    if kind == "preservation" and not protected:
        raise ValueError("preservation manifests require protected metrics")
    if not isinstance(floors, list):
        raise TypeError("manifest hard_floors must be an array")
    parsed_floors: list[HardFloor] = []
    for floor in floors:
        if not isinstance(floor, dict):
            raise TypeError("hard floor must be an object")
        metric, statistic, operator = floor.get("metric"), floor.get("statistic"), floor.get("operator")
        if not isinstance(metric, str) or not metric:
            raise TypeError("hard floor metric is required")
        if statistic not in {"median", "p95", "maximum"} or operator not in {"at_least", "at_most"}:
            raise ValueError("hard floor statistic or operator is unsupported")
        parsed_floors.append(
            HardFloor(
                metric=metric,
                statistic=cast(Literal["median", "p95", "maximum"], statistic),
                operator=cast(Literal["at_least", "at_most"], operator),
                value=_number(floor.get("value"), "hard floor value"),
            )
        )
    if not 0 < improvement < 1:
        raise ValueError("minimum_improvement must be between zero and one")
    return Manifest(
        schema_version=3,
        name=cast(str, value["name"]),
        kind=cast(Literal["preservation", "optimization"], kind),
        measurement=cast(Literal["records", "rss"], measurement),
        fixture=_fixture(value.get("fixture")),
        baseline_command=_command(value.get("baseline_command"), "baseline_command"),
        candidate_command=_command(value.get("candidate_command"), "candidate_command"),
        primary=cast(str | None, primary),
        protected=cast(list[str], protected),
        hard_floors=parsed_floors,
        minimum_improvement=improvement,
    )


def _environment() -> dict[str, str | bool]:
    return {
        "captured_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "git_revision": subprocess.run(
            ["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True
        ).stdout.strip(),
        "dirty": bool(
            subprocess.run(["git", "status", "--porcelain"], check=True, capture_output=True, text=True).stdout
        ),
        "machine": platform.machine(),
        "platform": platform.platform(),
        "python": platform.python_version(),
    }


def _stop_process(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def _run_output(command: Sequence[str], timeout: float) -> str:
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    try:
        output, _ = process.communicate(timeout=timeout)
    except BaseException:
        _stop_process(process)
        raise
    if process.returncode != 0:
        raise subprocess.CalledProcessError(process.returncode, command, output=output)
    if output:
        print(output, end="")
    return output


def _rss_bytes(process_id: int) -> int:
    process = subprocess.run(["ps", "-o", "rss=", "-p", str(process_id)], check=False, capture_output=True, text=True)
    output = process.stdout.strip()
    if process.returncode != 0 or not output:
        raise ProcessLookupError(f"process {process_id} has no RSS sample")
    rss_bytes = int(output) * 1024
    if rss_bytes <= 0:
        raise ProcessLookupError(f"process {process_id} has no live RSS sample")
    return rss_bytes


def _sample_rss(command: Sequence[str], timeout: float) -> tuple[dict[str, Metric], dict[str, int]]:
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    stdout = process.stdout
    if stdout is None:
        raise RuntimeError("RSS child stdout pipe was not created")
    lines: queue.Queue[str | None] = queue.Queue()

    def read_output() -> None:
        for line in stdout:
            lines.put(line)
        lines.put(None)

    threading.Thread(target=read_output, name="seex-rss-output", daemon=True).start()
    samples: list[int] = []
    phases: dict[str, int] = {}
    output_closed = False
    deadline = time.monotonic() + timeout
    try:
        while process.poll() is None or not output_closed:
            if time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired(command, timeout)
            try:
                line = lines.get(timeout=min(_RSS_INTERVAL_SECONDS, max(deadline - time.monotonic(), 0.001)))
            except queue.Empty:
                line = ""
            if line is None:
                output_closed = True
            elif line:
                print(line, end="")
                marker = line.partition("SEEX_RSS_PHASE ")[2].strip()
                if marker:
                    phases[marker] = max(len(samples) - 1, 0)
            if process.poll() is None:
                try:
                    samples.append(_rss_bytes(process.pid))
                except ProcessLookupError:
                    pass
    except BaseException:
        _stop_process(process)
        raise
    if process.returncode != 0:
        raise subprocess.CalledProcessError(process.returncode, command)
    if not samples or not {"warm", "cycles_done", "final"}.issubset(phases):
        raise ValueError("RSS workload must emit warm, cycles_done, and final phases")
    values = {
        "viewer.rss.warm": samples[phases["warm"]],
        "viewer.rss.peak": max(samples),
        "viewer.rss.final": samples[phases["final"]],
    }
    metrics = {
        key: Metric(
            domain="viewer",
            metric=key.removeprefix("viewer."),
            unit="bytes",
            direction="lower",
            batch_iterations=1,
            samples=[float(value)],
            median=float(value),
            p95=float(value),
            maximum=float(value),
            relative_mad=0.0,
            calibrated=True,
        )
        for key, value in values.items()
    }
    return metrics, phases


def _capture(role: Role, manifest: Manifest, timeout: float) -> Capture:
    command = manifest["baseline_command"] if role == "baseline" else manifest["candidate_command"]
    if manifest["measurement"] == "rss":
        metrics, phases = _sample_rss(command, timeout)
    else:
        metrics, phases = parse_records(_run_output(command, timeout)), {}
    return Capture(role=role, command=command, metrics=metrics, phases=phases)


def _change(before: float, after: float, direction: Direction) -> float:
    improvement = (before - after) / before if direction == "lower" else (after - before) / before
    return -improvement


def _improvement(before: float, after: float, direction: Direction) -> float:
    return -_change(before, after, direction)


def _directions_conflict(values: Sequence[float]) -> bool:
    return any(value < 0 for value in values) and any(value > 0 for value in values)


def _selected_metrics(manifest: Manifest) -> list[str]:
    selected = list(manifest["protected"])
    if manifest["primary"] is not None:
        selected.append(manifest["primary"])
    selected.extend(floor["metric"] for floor in manifest["hard_floors"])
    return sorted(set(selected))


def compare(manifest: Manifest, captures: Sequence[Capture]) -> tuple[Verdict, str, dict[str, object]]:
    """Compare two baseline and two candidate captures without extending the workload."""
    if [capture["role"] for capture in captures] != list(_ORDER):
        raise ValueError("performance captures must use A-B-B-A order")
    baseline = [capture for capture in captures if capture["role"] == "baseline"]
    candidate = [capture for capture in captures if capture["role"] == "candidate"]
    comparisons: dict[str, object] = {}
    selected = _selected_metrics(manifest)
    missing = [metric for metric in selected if any(metric not in capture["metrics"] for capture in captures)]
    if missing:
        return "inconclusive", "selected metrics were missing", {"missing": missing}
    for metric in selected:
        identities = {
            (capture["metrics"][metric]["unit"], capture["metrics"][metric]["direction"]) for capture in captures
        }
        if len(identities) != 1:
            raise ValueError(f"metric {metric!r} changed unit or direction between captures")
    unreliable: list[str] = []
    for metric in selected:
        records = [capture["metrics"][metric] for capture in captures]
        if any(not record["calibrated"] or record["relative_mad"] > _MAX_RELATIVE_MAD for record in records):
            unreliable.append(metric)
            continue
        for role_captures in (baseline, candidate):
            if _relative_mad([capture["metrics"][metric]["median"] for capture in role_captures]) > _MAX_RELATIVE_MAD:
                unreliable.append(metric)
                break
    if unreliable:
        return "inconclusive", "selected metrics exceeded the noise limit", {"unreliable": sorted(set(unreliable))}
    floor_failures: list[str] = []
    for floor in manifest["hard_floors"]:
        for capture in candidate:
            value = capture["metrics"][floor["metric"]][floor["statistic"]]
            failed = value < floor["value"] if floor["operator"] == "at_least" else value > floor["value"]
            if failed:
                floor_failures.append(floor["metric"])
                break
    if floor_failures:
        return "regression", "candidate failed a hard floor", {"failed_floors": sorted(set(floor_failures))}
    regressions: dict[str, object] = {}
    direction_conflicts: dict[str, object] = {}
    for metric in manifest["protected"]:
        direction = baseline[0]["metrics"][metric]["direction"]
        before = [capture["metrics"][metric]["median"] for capture in baseline]
        after = [capture["metrics"][metric]["median"] for capture in candidate]
        pair_changes = [_change(left, right, direction) for left, right in zip(before, after, strict=True)]
        combined = _change(statistics.median(before), statistics.median(after), direction)
        comparisons[metric] = {"pair_changes": pair_changes, "combined_change": combined}
        if _directions_conflict(pair_changes):
            direction_conflicts[metric] = comparisons[metric]
        if combined > _MAX_MEDIAN_REGRESSION or any(change > _MAX_PAIR_REGRESSION for change in pair_changes):
            regressions[metric] = comparisons[metric]
    if direction_conflicts:
        return "inconclusive", "A/B and B/A directions disagreed", {"direction_conflicts": direction_conflicts}
    if regressions:
        return "regression", "a protected metric regressed", {"regressions": regressions}
    if manifest["kind"] == "preservation":
        return "pass", "selected performance was preserved", comparisons
    primary = cast(str, manifest["primary"])
    direction = baseline[0]["metrics"][primary]["direction"]
    before = [capture["metrics"][primary]["median"] for capture in baseline]
    after = [capture["metrics"][primary]["median"] for capture in candidate]
    pairs = [_improvement(left, right, direction) for left, right in zip(before, after, strict=True)]
    combined = _improvement(statistics.median(before), statistics.median(after), direction)
    comparisons[primary] = {"pair_improvements": pairs, "combined_improvement": combined}
    if _directions_conflict(pairs):
        return "inconclusive", "A/B and B/A directions disagreed", comparisons
    if any(value < 0 for value in pairs):
        return "regression", "the optimization primary reliably regressed", comparisons
    if all(value > 0 for value in pairs) and combined >= manifest["minimum_improvement"]:
        return "pass", "the primary improved without a protected regression", comparisons
    return "no_change", "the primary did not reach its improvement target", comparisons


def _atomic_write(path: pathlib.Path, result: Result) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def run_gate(manifest: Manifest, output: pathlib.Path) -> Result:
    """Run the bounded comparison and checkpoint after every completed process."""
    result = Result(
        schema_version=3,
        record_type="gate_result",
        manifest=manifest,
        environment=_environment(),
        execution_order=list(_ORDER),
        captures=[],
        verdict="running",
        reason="capture in progress",
        comparisons={},
    )
    _atomic_write(output, result)
    deadline = time.monotonic() + _MAX_GATE_SECONDS
    try:
        for role in _ORDER:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired(manifest["name"], _MAX_GATE_SECONDS)
            result["captures"].append(_capture(role, manifest, remaining))
            _atomic_write(output, result)
        verdict, reason, comparisons = compare(manifest, result["captures"])
        result.update(verdict=verdict, reason=reason, comparisons=comparisons)
    except BaseException as error:
        result.update(verdict="error", reason=f"{type(error).__name__}: {error}")
        _atomic_write(output, result)
        raise
    _atomic_write(output, result)
    return result


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    run = commands.add_parser("run")
    run.add_argument("--manifest", type=pathlib.Path, required=True)
    run.add_argument("--output", type=pathlib.Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    result = run_gate(read_manifest(args.manifest), args.output)
    return {"pass": 0, "no_change": 2, "inconclusive": 2, "regression": 3}[result["verdict"]]


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (
        KeyboardInterrupt,
        OSError,
        TypeError,
        ValueError,
        json.JSONDecodeError,
        subprocess.SubprocessError,
    ) as error:
        print(f"performance gate failed: {error}", file=sys.stderr)
        raise SystemExit(4) from error
