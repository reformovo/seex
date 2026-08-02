# Performance validation

This is the only current Seex performance contract. Historical v1/v2 reports,
stress workloads, and baseline JSON live under [`reference/`](reference/).

## Verification types

- **Acceptance** verifies deterministic behavior, structure, compatibility, or
  release readiness. It is the only blocking verification type.
- **Benchmark** emits raw measurements and never makes a verdict.
- **Performance comparison** compares the rolling baseline and candidate and
  classifies the observation as `pass`, `no_change`, `regression`, or
  `inconclusive`. The classification never blocks a commit or milestone.
- **Profile** is manual diagnostic evidence from plans, Samply, Tracy,
  Instruments, `heap`, or `vmmap`; it never blocks a commit or milestone.

Correctness records do not belong in benchmark output. Small fixtures cover
parity, last-write-wins, neighbors, strict budgets, stale snapshots,
cancellation, and concurrency. Stress and profile scenarios run only when a
specific diagnosis needs them.

## Schema v3

Run exactly one comparison with:

```bash
python scripts/performance_gate.py run \
  --manifest candidate.json \
  --output result.json
```

The manifest contains `schema_version: 3`, a name, `kind` (`preservation` or
`optimization`), `measurement` (`records` or `rss`), fixture identity and
positive integer scale, baseline/candidate command arrays, protected metrics,
optional hard floors, and an optimization primary. RSS manifests may set
`domain` to `reporting`, `query`, or `viewer`; omitted values retain the
historical `viewer` default. The default primary target is 5%.

A records benchmark writes one line per metric:

```text
SEEX_BENCH {"schema_version":3,"record_type":"metric",...}
```

The record contains only domain, metric, unit, direction, batch iterations,
and exactly ten raw samples. The runner derives median, p95, maximum, and
relative MAD. Timing and throughput batches must represent at least 25 ms per
sample, giving at least 250 ms of measurement per capture. RSS workloads emit
`warm`, `cycles_done`, and `final` phase markers; the runner samples peak RSS.

## Execution and verdicts

The only process order is A-B-B-A: two fresh rolling-baseline processes and two
fresh candidate processes. Fixture preparation and a completed release build
are outside the 120-second workload deadline. After every child process, the
runner atomically checkpoints the result. Timeout or interruption terminates
the active child and preserves completed captures.

Selected metrics are Inconclusive when relative MAD exceeds 2% or a deciding
metric is missing. An optimization primary is also Inconclusive when its A/B
and B/A directions conflict. Protected metrics use only the preservation
limits below. The runner never adds samples or processes automatically.

- Preservation passes when the combined protected regression is at most 3%
  and neither order pair regresses more than 5%.
- Optimization passes when both order pairs improve, the combined primary
  improvement reaches its target, and protected metrics meet the 3%/5% limits.
- A smaller non-regressing improvement is No-change.
- A reliable regression or hard-floor failure is Regression.

Every completed comparison exits 0, regardless of classification. Exit 4 is
reserved for a runner, tool, or workload error. The rolling baseline is the
comparison reference and advances only on Pass; other classifications do not
reject a change or close a milestone. Original revisions remain historical
trend observations.

## Scoped workloads

- Reporting: `bench_log_throughput.py` is the sole admission benchmark. Run
  only the affected Rust or Python boundary and mode. `bench_log_persistence.py`
  emits local drain/persistence and finalization samples; S3/OSS is observation
  only.
- Query: the Reader benchmark uses 1 Run × 1 Metric × 1,000,000 points. Select
  only the affected backend, axis, and full/narrow range. Narrow Step changes
  normally use DuckDB narrow as primary and DuckDB full as protected.
- Viewer RSS: 4 Runs × 2 Metrics × 100,000 points, dual View, three zoom
  round-trips, and the four-way read path. CPU remains a separate calibrated
  chart benchmark.

The MinIO partition-pruning script is Acceptance: each catalog backend performs
one query and must return the expected result while reading no unrelated
Run/Metric partition. LTTB validation and wheel smoke are release Acceptance.

## Historical entry observation

The completed U2 single-View seven-pair artifact is retained only as history:
`/tmp/seex-perf/u2/viewer-real-single-pair-e1f248f-17a0cc4.json`, SHA-256
`997accac2eb0946e52c3dafd8a9481c911bacd0bf4ee1a97080a46fd8a5ed474`.
The interrupted dual-View attempt produced no result JSON and is not rerun.
