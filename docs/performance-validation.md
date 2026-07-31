# Unified Performance Validation

## U0 baseline identity

The permanent U0 executable baseline is the clean detached worktree at
`e1f248fbb66eed7c49d36ef393e263f6c72df721`. Original and rolling initially
refer to the same revision. The older Viewer-only schema-v1 record remains in
`viewer-performance-baselines.json` as release history and is not a deciding
U1 baseline.

Raw samples and process RSS series are outside the repository under
`/tmp/seex-perf/u0/`. Their SHA-256 values and stable conclusions are recorded
in `performance-baselines.json`. The source worktree was clean; unrelated user
documentation edits in the primary worktree were excluded by measuring from a
detached worktree and independent release target directory. The gate driver ran
under Python 3.14.6; Python/PyO3 workloads and builds used the 3.13.2 project
virtual environment.

## Reporting baseline

The Rust engine and Python/PyO3 workloads cover explicit single-metric,
compatibility implicit single-metric, compatibility eight-metric Mapping,
queue admission, drain/persistence, finalization, and external process RSS.
Compatibility modes use only the shipped explicit-step API; U3 and U4 replace
the adapter beneath the same workload names.

Every correctness check passed: no queue-full failure, zero pending reports
after drain, exactly 1,000 persisted points per durability repeat, and a
successful terminal flush. Python explicit and implicit admission were
reliable across seven processes. Python Mapping relative MAD was 2.0219%, so it
is recorded but non-deciding. Short queue-admission and shutdown phases and the
variable Python finalization phase are also non-deciding.

RSS is an informational original measurement rather than an optimization
verdict. The warm marker precedes lazy DuckDB/DuckLake initialization, so final
RSS legitimately exceeds the Viewer steady-state allowance. Future migration
candidates compare the same named phases rather than treating process startup
as a leak.

Representative commands:

```bash
python scripts/performance_gate.py capture-v2 --runs 1 \
  --output /tmp/seex-perf/u0/reporting-rust-clean.json -- \
  env CARGO_TARGET_DIR=/tmp/seex-perf/targets/u0-original \
  cargo test --release -p seex-core --test reporting_performance \
  -- --ignored --nocapture

python scripts/performance_gate.py capture-v2 --runs 7 \
  --output /tmp/seex-perf/u0/reporting-python-explicit-clean.json -- \
  python scripts/bench_log_throughput.py --mode explicit_single
```

## Query baseline

The immutable query fixture contains DuckDB and SQLite catalogs with 10 Runs,
one `loss` series per Run, and 1,000,000 effective points per series. Its
`lww-spikes-v1` identity covers a late replacement at step 250,000 and a spike
at step 500,000. Each measurement process validated the manifest, aggregate
row count, step/timestamp bounds, data-file count and bytes, and a deterministic
hash of the actual data files before opening the fixture read-only. Each backend
has 10,000,010 physical rows across 20 data files.

Reader-equivalent coverage passed for Step, relative-time, and timestamp axes;
full and narrow half-open ranges; strict `max_points`; native/standalone parity;
left and right real neighbors; completeness and reasons. Standalone relative
time returned explicit missing-run-start evidence. All six Viewer-equivalent
query metrics were reliable inside their seven-sample process capture.

Two independent release targets at the baseline revision ran seven AB/BA
pairs. Original and rolling migration comparisons both returned `pass`, with
no failed checks, floors, or reliable protected regressions. Two of twelve
Reader metrics were reliable across both process series; the other ten had
cross-process relative MAD above 2% and remain non-deciding evidence.

The fresh-process RSS capture recorded warm 294,912 bytes, peak/final
804,290,560 bytes, and no monotonic growth. Its machine verdict is
`regression` because the warm marker precedes lazy DuckDB/DuckLake loading;
this is retained as an informational startup baseline, not represented as a
steady-state pass.

Representative commands:

```bash
python scripts/performance_gate.py capture-v2 --runs 1 \
  --output /tmp/seex-perf/u0/query-axes-e1f248f.json -- \
  env SEEX_APP_SCALE_FIXTURE_ROOT=/tmp/seex-perf/fixtures/query-v3-e1f248f \
  /tmp/seex-perf/targets/u0-original/release/deps/large_series_validation-da5e76e3ced1b854 \
  reader_equivalent_axes_and_ranges --ignored --nocapture

python scripts/performance_gate.py compare-v2 \
  --spec /tmp/seex-perf/u0/query-self-compare-spec-e1f248f.json \
  --original /tmp/seex-perf/u0/query-pair-e1f248f.json \
  --rolling /tmp/seex-perf/u0/query-pair-e1f248f.json
```

## Viewer baseline

The retained Viewer fixture is DuckDB with 10 Runs, six Metrics, and 1,000,000
effective points per series. Its v2 manifest SHA-256 is
`59e48cc44180520f4723cb957a9de95e98a8ed594839045a5a26fe8bd2e4188d`.
The manifest fingerprints 60,000,060 physical rows across 120 data files.
The read-only resource capture validated all 30 requested-budget, source-point,
returned-point, snapshot-point, and snapshot-byte metrics.

The release CPU workload passed the unchanged p95 8.33 ms and single-operation
16.7 ms floors. Reliable p95 values ranged from 27.932 ns for brush zoom to
2.159 ms for uncached path preparation; the maximum observed single operation
was 5.742 ms. Four of seven protected CPU metrics were non-deciding because
cross-process relative MAD was above 2%, and single-operation metrics are
hard-floor evidence rather than timing decisions because they do not form
10 ms batches.

Single and dual View matrix checks both passed with 10 Runs, six Metrics,
1x/2x/3x physical width assertions, peak query concurrency 4, and zero retained
stale snapshots. Each of the 30 zoom-in/out cycles crossed the 100 ms debounce
and settled before continuing. Single View warm/peak/final RSS was
171,540,480/175,849,472/171,524,096 bytes; dual View was
170,164,224/174,145,536/170,147,840 bytes. Both RSS verdicts were `pass` with
no monotonic growth.

An ordinary release binary from the behavior-equivalent pre-correction revision
produced a 90.781-second launch-mode Metal System Trace with Instruments 16.0
(17F113). Before capture,
the persisted workbench was reset to the sole `viewer scale` source and was
verified across an application restart with exactly 10 selected Runs and the
six `accuracy`, `error`, `latency`, `loss`, `memory`, and `throughput` tracks;
each track reported `10 Runs · 10 drawable`. The traced zoom-in/out interaction
was performed and visually verified by the user. An earlier Computer Use
attempt did not reliably change the viewport and is explicitly non-evidence.

After a five-second startup warmup, all 4,976 timed presentations used one
approximately 3.572 ms 280 Hz period; the maximum was 3.579 ms. The 2,605
post-warm drawable waits had a 4.069 ms maximum, with none above 7.15 ms or
16.7 ms. Instruments reported no hang risks. It did report one 1.041-second
potential hang from 3.398 to 4.439 seconds while the initial six tracks were
loading, so the warmup excludes that startup interval rather than hiding it.

At capture time, `system_profiler` identified XG27AQWMG at 2560x1440 and
280 Hz as the main display. Instruments listed the same external display and
refresh rate, but its `device-display-info` table marked the 120 Hz built-in
display as main. The contradiction is retained as an environmental limitation;
the measured 3.572 ms presented-handler cadence is the 280 Hz evidence. The
subsequent corrections changed only test workloads and gate validation, so the
ordinary product binary exercised by this retained trace is behavior-equivalent.

Representative commands:

```bash
python scripts/performance_gate.py rss --interval 0.05 \
  --output /tmp/seex-perf/u0/viewer-dual-rss-e1f248f.json -- \
  env CARGO_TARGET_DIR=/tmp/seex-perf/targets/u0-original \
  SEEX_VIEWER_PERF_VIEWS=2 cargo test --release -p seex-app \
  --features test-support \
  representative_workbench_stays_responsive_while_a_source_is_pending \
  -- --ignored --nocapture

xcrun xctrace record --template 'Metal System Trace' --time-limit 90s \
  --output /tmp/seex-perf/u0/viewer-metal-280hz-10x6-manual-9610248.trace \
  --no-prompt \
  --launch -- /tmp/seex-perf/u0/Seex-9610248-280hz.app/Contents/MacOS/seex-app \
  /tmp/seex-perf/fixtures/viewer-v2-9610248/duckdb
```

## U0 gate closure

Reporting, query, and Viewer each ran seven alternating AB/BA process pairs
from independent release targets at revision
`e1f248fbb66eed7c49d36ef393e263f6c72df721`. Original and rolling comparisons
both returned `pass` in all three domains, with no failed correctness checks,
hard floors, or reliable protected regressions. Reporting had 0/6 reliable,
query 2/12, and Viewer 3/7 protected metrics across the paired process series;
non-deciding metrics remain listed rather than being promoted to evidence.

The alternating instrumentation comparison also passed. Six reliable query
metrics had test-support overhead from -1.139% to +0.609%; three reliable
Viewer CPU metrics had overhead from +0.535% to +1.593%. An ordinary release
feature graph contained only `default` and `desktop`; its dynamic dependencies
were unchanged system frameworks and libraries. A stripped binary scan found
no `SEEX_PERF`, RSS phase, or resource-counter strings.

The final gates passed: Rust fmt, Clippy for all targets/features with warnings
denied, check and tests; Ruff format/check, Pyright, and 142 Python tests with
two opt-in MinIO tests skipped; release maturin develop install and wheel
build. `cargo test` retained one existing default-feature test-only unused
import warning, while the required all-feature Clippy warnings-as-errors gate
passed. U0 is closed; U1 may begin from the frozen original/rolling baseline.
