# Overview/Detail curve query performance report

Status: current-state report for the Desktop curve query slice. The sampled SQL
shape is intentionally frozen at this revision; this is not P7 acceptance, a
desktop frame/RSS measurement, or a physical-I/O claim.

- Source revision: `9bb2f25` for the current sampled SQL and stage profile;
  each panel table names its own A/B commits.
- Reported 2026-10-22; measurements captured 2026-10-21/22 on macOS arm64,
  Rust/Cargo 1.97.1, release profile.
- Fixture: `.tmp/v5-panel-million/store`, 16 Runs, each with `loss` Steps
  0..999,999 plus one repeated Step 500,000 (1,000,001 observations per Run),
  62 Parquet files. Point budget: 1,024 per curve.

## Scope

The report covers the Desktop **Detail** and **Overview** sampled queries at
sixteen curves, the production plans those queries build, and the adjacent
summaries guard. Non-sampled `Full` queries, inspector, discovery, writes and
the Python reporting surface are outside this measurement.

The slice changed these observable behaviors:

- Detail returns only observations inside its closed viewport; out-of-view
  neighbors are gone, so an empty viewport renders no curve.
- Detail and Overview share one five-column filtered SQL; only Step bounds
  differ (`Steps{start, end+1}` versus `StepsFrom{0}`).
- `WHERE isfinite(value_f64)` runs in SQL; `source_count` counts finite
  observations in range, not all observations.
- Curve completeness follows Run lifecycle only. Series diagnostics were
  removed, so a missing metric is `Complete` with zero points and a failed Run
  is `Partial`, not `Invalid`.
- Summaries choose the catalog-only or fact-fallback SQL from catalog Run
  status; finished-only requests read no Parquet files.

No connection cache, rollup, storage-layout switch, public Python API change or
new dependency is part of this slice.

## Method

- **Panel comparison:** release `panel_gate` through
  `python scripts/performance_gate.py run`, one bounded A-B-B-A workload,
  ten internally calibrated samples of at least 25 ms per metric/capture, no
  retries and no OS cache clearing. The complete workloads finished in 50.07 s
  (neighbor ablation), 52.90 s (viewport-only Detail) and 70.07 s (one SQL).
- **Storage stage profile:** ignored release
  `storage::metric_query::tests::sampling_stage_profile`, ten repetitions per
  Run summed across sixteen Runs, one reused read-only connection, sequential
  curves. It is a diagnostic, not A-B-B-A.
- **Plans:** ignored release
  `storage::metric_query::tests::production_query_plans`, using the exact SQL
  and bindings production builds and `EXPLAIN (ANALYZE, FORMAT JSON)`.

## Detail panel: viewport-only Detail

Complete sixteen-curve Desktop request, A = `076913b` (neighbor-retaining),
B = viewport-only Detail. Medians in ms, A/B/B/A:

| Range | A1 | B1 | B2 | A2 | Combined improvement |
| --- | ---: | ---: | ---: | ---: | ---: |
| prefix (0..2,048) | 435.06 | 205.66 | 208.17 | 429.03 | 52.11% |
| middle | 527.07 | 293.85 | 292.40 | 526.65 | 44.36% |
| outside (empty) | 421.03 | 87.11 | 86.82 | 426.35 | 79.48% |

Gate **pass**, relative MAD 0.32–1.87%, no protected regression. The win comes
from removing the whole-Run bounds scan that preceded every curve, not from
LTTB. Counts changed from 2,050 to 2,049 per prefix curve, 100,004 to 100,002
per middle curve, and 1 to 0 per empty curve. Full-range Detail is unchanged.

Supporting storage-only neighbor ablation (same idea before production
integration) was **inconclusive** because every metric exceeded 2% relative MAD
in at least one capture; its 16-curve medians were 345.38 -> 149.16 ms (prefix),
436.64 -> 227.64 ms (middle) and 338.53 -> 34.49 ms (outside), A/B. Those
numbers are not a formal pass and do not describe the current SQL.

## Overview and the one-SQL simplification

The one-shape change was compared against the superseded two-shape state on the
sixteen-curve panel. Medians in ms, A/B/B/A:

| Query | A1 | B1 | B2 | A2 |
| --- | ---: | ---: | ---: | ---: |
| detail prefix | 209.17 | 230.27 | 206.68 | 210.10 |
| detail middle | 284.99 | 288.25 | 269.61 | 288.12 |
| detail outside | 88.67 | 101.65 | 83.59 | 87.77 |
| overview full | 694.08 | 693.99 | 683.26 | 684.93 |

Gate **inconclusive**: three Detail captures exceeded 2% relative MAD. There is
no repeatable direction and no regression signal; Overview is flat. The one SQL
is a simplification, not a proven speedup. The earlier eight-to-seven-column
experiment was superseded and is not evidence for column width alone. The two
panel tables come from different revisions and are not additive; each answers
its own A/B question.

## Current storage stage profile

Storage-only medians for sixteen curves in ms. `execute` includes `row_count()`
materialization; `fetch` is `statement.step()`; `Arrow` is
`RecordBatch::from`; `LTTB push` is a standalone reducer over an equal-length
synthetic stream; `extraction` is `rows_ns - LTTB push` and is approximate.

| Range | Rows/curve | prepare | execute | consume | fetch | Arrow | LTTB push | extraction |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| prefix (0..2,048) | 2,049 | 17.00 | 123.68 | 0.89 | 0.27 | 0.003 | 0.39 | 0.17 |
| middle | 100,002 | 19.26 | 154.74 | 23.79 | 6.21 | 0.03 | 16.91 | 0.09 |
| outside (empty) | 0 | 15.04 | 16.27 | 0.02 | 0.00 | 0.00 | 0.00 | 0.00 |
| overview | 1,000,001 | 25.63 | 314.57 | 260.19 | 76.67 | 0.28 | 169.69 | 8.66 |

The storage-only totals (prepare + execute + consume) are approximately 142 ms
for prefix, 198 ms for middle, 32 ms for outside and 600 ms for Overview. The
full panel values in the earlier table also include a fresh connection
(~27 ms), metadata, point conversion, chart construction and, at the time, the
series diagnostics that were later removed; do not add the two tables together.

Key observations:

- Narrow and middle requests are execute-dominated. Their consume half is
  almost entirely native LTTB.
- Overview consume (260 ms) is about 170 ms LTTB push (65%), 77 ms fetch (30%),
  9 ms extraction (3%) and 0.3 ms Arrow conversion (0.1%). LTTB costs about
  10.6 ns per point.
- Fixed per-statement cost is about 1.0–1.7 ms prepare plus ~1.1 ms empty
  execute, or 32–44 ms per sixteen-curve panel.
- A 20% LTTB improvement would save roughly 34 ms (about 5%) of the ~685 ms
  Overview request. A local-reuse micro-optimization did not show a repeatable
  win and was reverted; no LTTB change is retained.

## Production plans

`EXPLAIN (ANALYZE)` on the exact production statements:

| Query | Files read | Rows out of `DUCKLAKE_SCAN` |
| --- | ---: | ---: |
| Detail prefix (0..2,048 closed) | 1 | 2,049 |
| Detail middle (450,000..550,000) | 8 | 100,002 |
| Detail outside (2M..3M) | 0 | 0 (operator eliminated) |
| Overview (`StepsFrom{0}`) | 62 | 1,000,001 |
| Summaries, sixteen finished Runs | 0 | 0 |

The summaries guard previously read 11 Parquet files / 24,576 rows for the
running-Run branch even when every requested Run was finished; the catalog
choice removes that scan. Plan cardinalities are not physical row counts and
the parallel operator timings are not additive wall-clock shares.

## Decisions

- Keep the current Overview/Detail SQL shape for this slice.
- Do not add request-level batching for the fetch-and-reduce pipeline: it can
  remove at most the prepare and fixed execution overhead, while row scan,
  materialization and LTTB remain.
- Do not micro-optimize LTTB against this evidence; its ceiling is about 5% of
  the Overview request and the attempted change was not repeatable.
- The material remaining lever is reducing executed rows (Rollup P5 or
  shared-sorted layout P3). Request-scoped prepared-statement reuse is a
  separate, smaller candidate (~15–26 ms per panel, more visible on
  narrow/empty ranges) and must not become a cross-request connection cache.

## Follow-up optimization plan

The full request still has non-SQL costs around the sampled statement. One
release panel case per row, ten hot samples, median, diagnostic only (not
A-B-B-A):

| Request | fresh open | metadata | storage | points | sum |
| --- | ---: | ---: | ---: | ---: | ---: |
| Detail prefix, 16 curves | 29.04 | 15.36 | 164.00 | 0.14 | 208.53 |
| Overview, 16 curves | 28.56 | 20.19 | 622.64 | 0.19 | 671.58 |

`storage` here is the Reader's `query_metric` stage and includes the selected
`MetricPoint` conversion and trace overhead; the finer storage split is the
stage table above. The candidates below are ordered by measured gain versus
risk, and none changes the frozen SQL shape.

### 1. Request-scoped query context (recommended next)

Two avoidable per-request costs remain:

- **Metadata:** `runs_for_desktop` already reads the selected Runs, but the
  fresh query reader starts with an empty `run_metadata` cache and calls
  `get_run` again for every curve. Measured 15–20 ms per sixteen-curve panel.
  Seeding the fresh reader from the request's Run snapshot should remove most
  of it. This also gives the panel one consistent Run-status snapshot per
  request; document that status is the discovery snapshot, not a per-curve
  re-read.
- **Prepare:** each curve prepares the same SQL with different parameters.
  Measured 1.0–1.7 ms per curve, 15–26 ms per panel. Reusing one prepared
  statement within the request/attempt should remove most of it. Keep the
  cache per attempt and per connection; a storage retry gets a fresh reader
  and statement, and this must never become a cross-request connection cache.

Combined expected gain is about 30–45 ms per sixteen-curve panel: roughly
14–21% of the 209 ms Detail-prefix request and 4.5–7% of the 672 ms Overview
request. This is the best measured small win and does not involve Arrow, LTTB
or SQL changes. Verify with one bounded A-B-B-A over prefix/middle/outside and
Overview, plus acceptance coverage for retry, cancellation and superseded
requests.

### 2. Snapshot reuse and invalidation (P6)

The Viewer already retains Overview snapshots and Detail coverage
(`needs_detail`, `detail_covers`, `retain_active_detail_coverage`). Audit the
actual hit/miss behavior under zoom, pan, panel switches and source appends
before adding anything: a covered viewport should not re-run a 200–670 ms
query. The remaining work is bounded eviction, a data version/revision for
invalidation, and proving that a stale snapshot cannot win after a write or
compaction. This is the largest interactive win that does not touch the query
shape, but it is app-level cache work, not a storage micro-optimization.

### 3. Executed-row reduction (P5/P3, later phase)

Overview `execute` is 315 ms and prefixes scan 124 ms per sixteen curves. The
only material way to cut that is to execute fewer rows or avoid the sort:
Rollup layers (P5) or a shared-sorted production layout (P3). Both are roadmap
phases with their own write-side validation, append-only/duplicate semantics
and RSS gates; they must not be bundled into a curve-query micro-change.

### 4. Optional experiments

- **Point conversion:** the Reader storage stage is about 22 ms higher than the
  storage-only profile, likely `MetricPoint` timestamp conversion and trace
  overhead. Measure before changing; expected gain is small.
- **Fetch/LTTB micro-optimizations:** 77 ms fetch and 170 ms LTTB per sixteen
  Overview curves, but the LTTB ceiling is about 5% of the request and the
  first attempt did not repeat. Revisit only with a new algorithmic idea.
- **Parallel curve execution:** 16 curves are currently sequential on one
  request connection. Bounded parallel workers could reduce wall time, but
  would add connections, cancellation/retry complexity and RSS contention; the
  earlier native capacity diagnostics closed without evidence to change
  production concurrency. Requires a separate proposal and real desktop
  evidence.

**Do not reopen:** the current SQL shape, request-level batching, LTTB
micro-optimization, connection caching, out-of-view neighbors or series
diagnostics, and the 16-Run/1,024-point limits. These are either measured as
low-value, rejected, or product boundaries; changing them needs a new
proposal.

## Not measured

- Real desktop GPUI frame latency and RSS. The viewport-only Detail tradeoff
  still needs a real desktop confirmation; headless tests cannot substitute.
- Physical I/O, cold-cache latency, allocation peaks, and a 10,000-curve or
  10,000-metric workload.
- The `Full`/raw path and inspector/discovery surfaces.
- Any performance claim for the one-SQL simplification; its gate is
  inconclusive.

## Evidence

- Detailed chronology and SHA-256 identities:
  [curve-query-workloads.md](curve-query-workloads.md).
- Panel evidence: `.tmp/v5-detail-no-neighbor/` (manifest
  `0d20a2282b98e4a8b86e77ca13202a4f732f237352533f10c2355bfaba5a0654`,
  result `f637c6067a5e1b6cabc837e56afeb0b88a82ce72a45f344fdf97c69406485f49`),
  `.tmp/v5-one-shape/artifacts/` (manifest
  `f2cb2c79848c14c4310f64659d69c4803d940c238e5404541d92d63452415d7a`,
  result `8a09d95ad12550af0369b18ef059f6f18ba27988f91fc365493ce121201b0d8d`).
- Stage profile: `.tmp/v5-sampling-stages-fine/sampling-stages.csv`
  (`68c2af7602b1e356b9cb6920fca5adf2dd4c2e2fce9207b5a99c9e430924dfea`),
  `lttb-stages.csv`
  (`a344e2507b20d3d0a158831f55cb6af76c81261e9d6e135477d6cbc33da5bfb7`).
- Full-request stage traces: `.tmp/v5-stage-metadata-prefix.log`
  (`5437c8cebaaf4a68482d01943eff1b4aa51135fd276fa2ee53b46397c1c75b57`) and
  `.tmp/v5-stage-metadata-overview.log`
  (`9271e404d35b2d16dc426b52957ceb451263d8bb34d451df3d93fc8145f5ca35`).
- Plans: `.tmp/v5-production-plans-final/` (SHA-256
  `e329af6d90fc7b01033b88d1d7886c69062cd1d8ffd46831829a7ddf19967542`).
- Code commits: `3366bb7` viewport-only Detail and surface removal, `4a4dda6`
  one filtered SQL, `baef7ad` summaries guard, `3a807ab` production plan
  test, `9bb2f25` stage profile.
