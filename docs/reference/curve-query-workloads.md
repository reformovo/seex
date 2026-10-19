# Curve query workload baseline
This is a workload baseline, **not an A-B-B-A optimization comparison or a performance PASS**.
It measures native LTTB Detail requests at eight inclusive Step ranges, with one or sixteen
Run curves per metric panel. No production SQL, scheduling, or selection limit changed.
Summary, inspector, and discovery are outside this measurement.

Capture: `ab45bee9ff7a13e2de49290ede62eb1f077d31f9` plus the archived test patch.
Local raw evidence: ignored `.tmp/v5-detail-range-matrix/` contains identities, patch, runner,
Parquet hashes, sixteen case logs, results, and wall time; these are not repository fixtures.
- Patch SHA-256: `f6bc4d2e46ba5d6808bf8a6b69da28889fd7fda06353b8f03d430ed13fa36900`.
- Runner SHA-256: `62829df654496aa9065541c2f9d4aea86708adb57de62d29db21eb8b6467386d`.
- Parquet manifest SHA-256: `25004002f6714bdc974ac1545962f49d01d607eff8ff8a82a8aa22d7a0237d34`.

Fixture: `.tmp/v5-panel-million/store`, sixteen Runs, each with `loss` Steps 0..999,999 plus
an additional observation at Step 500,000 (1,000,001 observations). Budget: 1,024 points/curve.
Source counts include one actual nearest neighbor per available side, even outside the data.
## Measurement method
Each case ran in a fresh process: one separately timed first request, an excluded calibration
pilot, then ten internally calibrated samples of at least 25 ms. All cases selected one
iteration per sample. The complete sixteen-case workload took **58.64 seconds**, within a
shared 120-second deadline. First-request timing excludes opening the `ReadSession`.
The first request populates session diagnostics; hot samples reuse those diagnostics,
not a cached curve result or a cross-request DuckDB connection.
OS caches were **not cleared**: “first” does not mean cold disk.
RSS is the process high-water mark from macOS `/usr/bin/time -l`, including first request,
pilot, and samples; it is neither retained chart memory nor a per-request allocation delta.

The existing opt-in `panel_gate` test supplies the samples. To reproduce an individual case
against an already prepared fixture (fixture preparation is outside query timing):

```sh
SEEX_PANEL_ROOT="$PWD/.tmp/v5-panel-million/store" \
SEEX_PANEL_RANGE=detail SEEX_PANEL_DETAIL_RANGE=prefix SEEX_PANEL_CURVES=16 \
cargo test -p seex-app --no-default-features --release --test panel_scaling \
  panel_gate -- --ignored --exact --nocapture --test-threads=1
```

The capture invoked the built runner directly under the shared deadline, excluding build time.
Every request asserted completeness, exact source/curve counts, and `min(source_count, 1024)`
returned points (finite fixture only). Earlier `.tmp/v5-detail-ranges/` failed tail-count
expectations are not successful matrix samples.
## Results
Times are milliseconds; hot is the median of ten samples. MAD is median absolute deviation
relative to that median. RSS is MiB. Counts are **per curve**, not panel totals.

| Case | Inclusive Step range | Source / returned |
| --- | --- | ---: |
| full | 0..1,000,000 | 1,000,001 / 1,024 |
| prefix | 0..2,048 | 2,050 / 1,024 |
| middle | 450,000..550,000 | 100,004 / 1,024 |
| window | 500,000..502,048 | 2,052 / 1,024 |
| small | 0..511 | 513 / 513 |
| small-middle | 500,000..500,511 | 515 / 515 |
| tail | 999,000..1,100,000 | 1,001 / 1,001 |
| empty | 2,000,000..3,000,000 | 1 / 1 |
| Curves | Case | First ms | Hot ms | Hot min–max ms | MAD % | RSS MiB |
| ---: | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | full | 110.4 | 75.1 | 73.9–81.2 | 1.24 | 214.7 |
| 1 | prefix | 90.0 | 60.5 | 59.2–67.1 | 0.84 | 177.9 |
| 1 | middle | 95.3 | 66.5 | 65.4–78.4 | 1.25 | 191.4 |
| 1 | window | 89.7 | 68.0 | 59.8–78.5 | 5.56 | 175.3 |
| 1 | small | 98.5 | 61.7 | 59.5–76.5 | 2.52 | 174.8 |
| 1 | small-middle | 89.0 | 60.4 | 59.3–63.4 | 1.39 | 174.4 |
| 1 | tail | 94.5 | 65.3 | 60.2–72.3 | 4.09 | 182.3 |
| 1 | empty | 89.0 | 60.2 | 59.1–61.0 | 0.98 | 174.1 |
| 16 | full | 1,039.3 | 672.7 | 657.0–721.8 | 1.51 | 259.8 |
| 16 | prefix | 818.2 | 439.2 | 426.1–506.4 | 2.25 | 212.3 |
| 16 | middle | 910.1 | 537.3 | 515.8–660.3 | 4.00 | 224.9 |
| 16 | window | 817.4 | 445.6 | 429.8–584.0 | 2.26 | 208.5 |
| 16 | small | 824.0 | 433.3 | 420.9–505.5 | 2.00 | 212.0 |
| 16 | small-middle | 824.5 | 429.5 | 420.2–451.4 | 1.14 | 216.2 |
| 16 | tail | 856.7 | 437.0 | 429.9–465.4 | 1.22 | 239.3 |
| 16 | empty | 805.1 | 425.4 | 416.7–444.1 | 1.05 | 211.5 |
Several cases exceed 2% MAD; original samples are retained without retries. This is not a
stable small-improvement comparison or cross-machine SLO. Sixteen curves at 0..2,048 still cost
439 ms; outside-only neighbors cost 425 ms. Full-partition neighbor-bound discovery warrants
investigation, but this capture has no operator profile quantifying its share. A 10% viewport
alone is insufficient coverage. No new ABBA was performed for `ab45bee`'s scalar bounds change;
older Top-N/stats profiles and comparisons must not be attributed to it.
## Separate workload: browsing 10,000 metric panels
The sixteen limit protects selected Runs (overlaid curves in a panel); it is not a cap on
metric panels. Removing it does not solve large metric-list browsing and would increase
per-panel work. Keep it while measuring the independent scrolling workload.

Virtual rows, visible Detail scheduling, directional Overview prefetch, and offscreen eviction
are foundations, **not evidence that 10,000 panels are bounded**. Next stress scenario:
select 10,000 metrics, scroll both ways, jump to tail, reverse, then stop to drain work.
Include slow reads/cancellation; record visible/retained panels, pending/active requests,
queued completions, resident points, RSS, and UI scheduling/frame latency. Verify backlog
drains and offscreen payload release; residency should follow visible-plus-prefetch size.
Measure full-list traversal/cloning separately. No 10,000-panel or GUI frame test is included.
Backpressure, cancellation, and stale completions remain unverified at this scale.

## Next optimization roadmap

Priority is bounded interactive work first, narrow-range latency second, and full-range
throughput third. The range matrix demonstrates a latency floor, not its precise cause;
virtualization demonstrates scheduling intent, not bounded resource usage.

1. **Establish the 10,000-metric resource contract.** Start with a headless scheduling stress
   test using controlled slow reads and scroll/reversal/drain sequences. Measure pending and
   completed work, active workers, retained payloads, and full-list traversal costs. Define
   explicit bounds from visible rows, prefetch, and worker capacity, independent of total
   metric count. Fix the first demonstrated unbounded path in a separate small change.
   Check superseded/cancelled work cannot repopulate evicted panels. This proves scheduling
   behavior only; real GUI frame latency and RSS require a separate desktop run. Do not
   load 10,000 million-point curves simply to exercise 10,000 panel identities.
2. **Profile Detail's fixed cost, then choose one SQL change.** Capture current plans and
   request-stage timings for outside-only, sub-budget, 2,048-Step, 10%, and full viewports,
   with one and sixteen curves. Investigate bounds discovery before considering request-level
   batched SQL. Prefer reducing unnecessary scans if plans support it; batching adds query
   complexity and must preserve per-curve budgets, retries, and cancellation. Neither is
   pre-approved as the answer. Do not trade correct nearest neighbors or duplicate observations
   for speed, or bypass DuckLake's authoritative file/snapshot ownership.
3. **Revisit full-range Overview only after those findings.** Profile SQL/Arrow consumption
   and native LTTB separately on the current revision. Optimize the measured dominant stage;
   keep native LTTB as the default and do not reintroduce Arrow+screen as an alternate policy.
4. **Defer summary, inspector, and discovery.** Give each its own workload and baseline later;
   do not mix them into curve-query comparisons. Reassess the selected-Run limit separately
   only if a concrete comparison UX requires more than sixteen Runs.

Each production change needs focused acceptance coverage and one affected-boundary, bounded
A-B-B-A comparison (ten calibrated samples >=25 ms, total <=120 s). Cover all eight Detail
ranges in correctness tests; include representative narrow/full ranges and one/sixteen curves
in performance coverage. If necessary, split independent boundaries into explicitly separate
workloads rather than silently exceeding the deadline. Report noise, regressions, and RSS
tradeoffs; only acceptance tests block a commit. No connection cache, rollup, public API,
new dependency, or storage-layout change is implied by this roadmap.
