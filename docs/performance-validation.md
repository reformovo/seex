# Unified Performance Validation

## U0 baseline identity

The permanent U0 executable baseline is the clean detached worktree at
`961024801569e28507041e079e4d47c413f728a5`. Original and rolling initially
refer to the same revision. The older Viewer-only schema-v1 record remains in
`viewer-performance-baselines.json` as release history and is not a deciding
U1 baseline.

Raw samples and process RSS series are outside the repository under
`/tmp/seex-perf/u0/`. Their SHA-256 values and stable conclusions are recorded
in `performance-baselines.json`. The source worktree was clean; unrelated user
documentation edits in the primary worktree were excluded by measuring from a
detached worktree and independent release target directory.

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
row count, and last step before opening the fixture read-only.

Reader-equivalent coverage passed for Step, relative-time, and timestamp axes;
full and narrow half-open ranges; strict `max_points`; native/standalone parity;
left and right real neighbors; completeness and reasons. Standalone relative
time returned explicit missing-run-start evidence. All six Viewer-equivalent
query metrics were reliable inside their seven-sample process capture.

Two independent release targets at the baseline revision ran seven AB/BA
pairs. Original and rolling migration comparisons both returned `pass`, with
no failed checks, floors, or reliable protected regressions. Three of twelve
Reader metrics were reliable across both process series; the other nine had
cross-process relative MAD above 2% and remain non-deciding evidence.

The fresh-process RSS capture recorded warm 294,912 bytes, peak/final
804,290,560 bytes, and no monotonic growth. Its machine verdict is
`regression` because the warm marker precedes lazy DuckDB/DuckLake loading;
this is retained as an informational startup baseline, not represented as a
steady-state pass.

Representative commands:

```bash
python scripts/performance_gate.py capture-v2 --runs 1 \
  --output /tmp/seex-perf/u0/query-axes-9610248.json -- \
  env SEEX_APP_SCALE_FIXTURE_ROOT=/tmp/seex-perf/fixtures/query-v2-ce0bbf5 \
  /tmp/seex-perf/targets/u0-original/release/deps/large_series_validation-da5e76e3ced1b854 \
  reader_equivalent_axes_and_ranges --ignored --nocapture

python scripts/performance_gate.py compare-v2 \
  --spec /tmp/seex-perf/u0/query-self-compare-spec.json \
  --original /tmp/seex-perf/u0/query-pair-9610248.json \
  --rolling /tmp/seex-perf/u0/query-pair-9610248.json
```

## Viewer baseline

The retained Viewer fixture is DuckDB with 10 Runs, six Metrics, and 1,000,000
effective points per series. Its v2 manifest SHA-256 is
`7883a5597abd69c7d1fb6b169ed81f9a22be2becb6eec5cd2d326f1e6a13f827`.
The read-only resource capture validated all 30 requested-budget, source-point,
returned-point, snapshot-point, and snapshot-byte metrics.

The release CPU workload passed the unchanged p95 8.33 ms and single-operation
16.7 ms floors. Reliable p95 values ranged from 25 ns for brush zoom to
2.030 ms for uncached path preparation; the maximum observed single operation
was 2.277 ms. Brush resize and pan were non-deciding because relative MAD was
above 2%, and single-operation metrics are hard-floor evidence rather than
timing decisions because they do not form 10 ms batches.

Single and dual View matrix checks both passed with 10 Runs, six Metrics,
1x/2x/3x physical width assertions, peak query concurrency 4, and zero retained
stale snapshots. Fresh processes completed 30 zoom-in/out cycles. Single View
warm/peak/final RSS was 169,213,952/173,260,800/169,213,952 bytes; dual View was
168,378,368/172,376,064/168,378,368 bytes. Both RSS verdicts were `pass` with
no monotonic growth.

An ordinary release binary from the baseline revision produced a 10.835-second
Metal System Trace with Instruments 16.0 (17F113). The exported trace records
57 presented handlers and zero hang risks, potential hangs, or drawable waits.
It enumerated the built-in 3024x1964 120 Hz display and two external 3840x2160
60 Hz displays. Non-interactive launch did not establish which display owned
the window, and the 280 Hz display used by the historical Viewer trace was not
connected. Therefore this capture proves trace reproducibility and metadata,
but high-refresh behavior remains an explicit non-deciding blocker.

Representative commands:

```bash
python scripts/performance_gate.py rss --interval 0.05 \
  --output /tmp/seex-perf/u0/viewer-dual-rss-9610248.json -- \
  env CARGO_TARGET_DIR=/tmp/seex-perf/targets/u0-original \
  SEEX_VIEWER_PERF_VIEWS=2 cargo test --release -p seex-app \
  --features test-support \
  representative_workbench_stays_responsive_while_a_source_is_pending \
  -- --ignored --nocapture

xcrun xctrace record --template 'Metal System Trace' --time-limit 10s \
  --output /tmp/seex-perf/u0/viewer-metal-60hz-9610248.trace \
  --launch -- /tmp/seex-perf/targets/u0-product/release/seex-app \
  /tmp/seex-perf/fixtures/viewer-v2-9610248/duckdb
```

Ordinary-release instrumentation isolation and the complete three-domain
self-comparison remain open. U1 must not begin until both close and the 280 Hz
environmental limitation is accepted or reproduced.
