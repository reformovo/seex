# Viewer Performance Validation

## Resource-Optimization Baseline Gate

The pre-optimization baseline was frozen on 2026-07-30 before changing any
Phase 3E resource budget or snapshot ownership. The machine-readable summary is
[`viewer-performance-baselines.json`](viewer-performance-baselines.json); raw
samples and Instruments bundles remain local validation artifacts.

The gate keeps an immutable original baseline and a rolling incumbent. A
candidate must improve its declared primary metric by at least 5% in six of
seven paired runs, may not regress a reliable protected median by more than 3%,
and must retain the p95 8.33 ms and single-operation 16.7 ms CPU limits.
Metrics with relative MAD above 2% are recorded but cannot accept a candidate.

The 10-Run, one-million-point-per-series DuckDB/SQLite baseline retained the
same storage fixture across runs. Overview returned 15,000 points and retained
30,000 point structs; full Detail returned 60,000 and retained 120,000. The
full-Detail shallow point footprint was 6,720,000 bytes. The direct release
test process peaked at 1,115,308,032 bytes RSS and a 911,197,288 byte memory
footprint. These values include native query working memory and are the Stage 4
process baseline, not an estimate of Viewer snapshot ownership.

The existing 2026-07-27 active-display trace remains the original Metal
baseline: it recorded ten presentations spanning two 280 Hz refresh periods,
so the final display gate was already open before resource optimization.

### Stage 1 Result

Stage 1 was accepted against the frozen original baseline. Detail budgets now
derive from logical plot width and cap at 5,000 points per series; Overview caps
at 2,000. Full-Detail returned points fell from 60,000 to 40,000 (33.3%) and
narrow-Detail points fell from 51,020 to 26,520 (48.0%). Corresponding shallow
snapshot bytes fell to 4,480,000 and 2,970,240 while source rows and viewport
neighbors remained unchanged.

The repeated scale process peak RSS fell from 1,115,308,032 to 1,081,982,976
bytes. Query timing remained scan-bound and did not show a consistent protected
regression. The post-change CPU gate passed with uncached path preparation at
2.031 ms p95 and 2.201 ms maximum. Organization-only state changes retained
their loaded snapshots, and display scale changes no longer alter storage
sampling density when logical width is unchanged.

### Stages 2–4 and Automated Stage 5 Candidate

Stages 2–3 and the isolated Stage 4 measurements passed on 2026-07-31.
`CurveSeriesSnapshot` now owns one chart series rather than a second per-point
evidence representation. Full-Detail shallow snapshot storage fell from
6,720,000 to 640,000 bytes (90.5%), and the narrow snapshot fell from 5,714,240
to 424,320 bytes (92.6%). Hover, locked cursor, and baseline delta continue to
index the same retained real sample; the manual hover regression check passed.

Projection compaction is bounded by logical canvas buckets, removed series
evict their projection and GPUI path entries, and repeated hover frames reuse
static Metric chart preparation. Query viewport filtering reduced the final
scale process peak RSS from 1,115,308,032 to 757,563,392 bytes (32.1%). The
10-Run, six-Metric 30-cycle workload held a 258,129,920 byte warm RSS and a
267,714,560 byte peak/final RSS, reported no monotonic growth, and remained
inside the `max(5%, 32 MiB)` final bound.

Four readers now clone one lazily opened DuckDB connection so they share the
database and buffer manager without reducing the four-way concurrency limit.
Seven alternating process pairs all used the same retained fixture and 300 RSS
samples per process. All seven candidate peaks were lower; six improved by at
least 5%, and the median improvement was 12.99%. The candidate also passed the
representative GPUI workload and a manual hover/tooltip/locked-cursor check.

The final retained-fixture query run was reliable for all six backend/query
combinations. DuckDB Overview/full/narrow medians were
1260.253/1267.972/515.609 ms; SQLite medians were
1267.051/1267.993/537.579 ms. Counts, viewport neighbors, exact extrema, and
DuckDB/SQLite evidence parity remained unchanged. The final CPU capture kept
the worst p95 at 2.066 ms and worst single operation at 2.834 ms.

An experimental grouped-extrema query was rejected before this incumbent: it
lowered an isolated scale RSS measurement but raised the real six-Metric peak
to 9,540,943,872 bytes and broke hover. Only that SQL candidate was reverted;
its exact-extrema correctness assertion remains. The active-display Metal and
final resource gates remain open.

#### Real Multi-View Resource Rejection

The automated GPUI workload did not reproduce native query working memory.
The fixed one-million-point, 10-Run, six-Metric application reached a 5.31 GiB
RSS peak with one View and ended the two-View interval at 3.69 GiB. The two-View
interval peaked at 5.06 GiB without Instruments; an attached Metal trace peaked
at 7.82 GiB before falling to approximately 0.49 GiB during detach. These are
hard-gate failures, so the Stage 4 whole-series split and Stage 5 RSS items are
not accepted despite the isolated scale and shared-connection improvements.

At the stable two-View capture, `heap` found only 135.5 MiB of live malloc
objects and `vmmap` reported a 769.1 MiB physical footprint. Malloc zones held
2.0 GiB, including 1.2 GiB of resident `MALLOC_SMALL (empty)` regions. A
symbolized high-water capture recorded 32.4 million allocations and 130.6 GiB
of cumulative allocation churn. The largest stacks were
`query_curves → query_aligned_metric → PhysicalCTE::Sink` and
`PhysicalWindow → HashedSort/FullSort`, allocating through DuckDB's
`ColumnDataCollection`, `TupleDataAllocator`, and buffer manager.

The current SQL narrows `visible_source`, but only after `ranked`,
`ordered AS MATERIALIZED`, `lag`, and bucket ranking have processed the full
series. The next candidate must split invariant whole-series diagnostics from
viewport selection and remove the full-series materialized CTE before any
further tuning. Safe early Step filtering is preferred; elapsed-time filtering
must preserve last-write-wins when a replacement changes timestamp. Grouped
`arg_min(struct_pack(...))` remains rejected because its real six-Metric peak
was 9.54 GiB and it broke hover.

A subsequent Step-bounds candidate pushed the viewport before last-write-wins,
but obtained bounds with a second source scan. Narrow Detail improved by
approximately 28–31%, while reliable Overview/full medians regressed by
13.6–21.3% across DuckDB and SQLite. This exceeded the 3% protected limit, so
the candidate was rejected before RSS pairing and only its candidate hunks
were removed. The next alternative must retain one source scan: compute Step
bounds and negative-axis state on the same effective stream, filter before
bucket ranks, and eliminate the unnecessary Step `lag` and full-series
materialized CTE.

The single-source-scan follow-up replaced the second scan with global Step
windows on the effective stream. It preserved all deterministic evidence and
produced reliable samples, but added another full-stream window pipeline:
DuckDB medians regressed by 46–72% and SQLite by 46–64%. It was also rejected
before RSS pairing. Since both SQL shapes failed protected latency, the next
alternative is a shared database memory budget: the four cloned Viewer
connections retain application-level concurrency while DuckDB enforces one
buffer-manager limit and spills only when their combined working set exceeds
that limit.

A 1 GiB shared Viewer memory limit then reduced the first smoke peak from
4.66 GB to 2.02 GB, but failed robust pairing. The first five improvements
were +56.6%, +19.1%, -18.9%, +12.2%, and -22.0%; only three improved, so six
of seven became impossible and the remaining runs were stopped. Reliable
single-query medians also regressed by approximately 4–5%. The memory-limit
hunk was removed. Further work must reduce a concrete operator rather than
force spill or tune a cap.

A step-ID extrema candidate then kept only extrema identities in each hash
aggregate and rejoined real rows. Overview/full medians improved by 25–28%,
but narrow Detail reliably regressed by approximately 7%. The candidate was
rejected and removed. Profiling attributed the narrow overhead to
`create_sort_key` blobs, a second hash group used to deduplicate step IDs, and
the row join. A scalar `arg_min`/`arg_max` state plus an `IN` semi-join is the
remaining operator-level alternative; it must retain the full-query gain
without the narrow regression.

Native scalar `arg_min`/`arg_max` removed the sort-key and join overhead and
improved DuckDB Overview/full by 10.6%/6.7%, but applying it to every screen
query made DuckDB narrow Detail 29.2% slower (SQLite approximately 22.8%, with
high MAD). It was rejected and removed. The measured crossover supports SQL
shape selection before execution: retain window ranks for small Step spans and
all elapsed-time queries, and use scalar extrema only when the Step span is
large relative to the point budget.

Adaptive selection retained the query-time improvements and protected narrow
latency, but the real two-View RSS smoke regressed from 2.57 GB to 3.23 GB
(26.0%). It was rejected before pairing. This isolates DuckDB
`HASH_GROUP_BY`, not merely large aggregate state, as the incompatible
operator. The next alternative must stay in the window pipeline while reducing
its four ordered ranks.

A Step-partition window candidate replaced four ordered ranks with two struct
min/max windows. All six reliable query medians improved by approximately
11–30%, but the real two-View RSS smoke regressed from 2.54 GB to 2.76 GB
(8.7%). It was rejected before pairing and removed. Faster SQL operators have
now repeatedly raised the real resource peak, while `vmmap` identifies empty
malloc arenas as the retained memory. The next candidate therefore leaves the
query plan unchanged and asks the macOS allocator to release empty pages after
each Run query has destroyed its temporary DuckDB result.

Calling maximal macOS pressure relief after every Run was then rejected on the
latency gate before RSS measurement. Reliable DuckDB Overview/full/narrow
medians regressed by 4.0%/5.9%/10.0%, and reliable SQLite full Detail regressed
by 4.0%; two other SQLite samples exceeded the 2% relative-MAD limit. Scanning
all malloc zones 10 times per Metric query is therefore too expensive. The
next candidate may request relief once after the complete Metric snapshot,
reducing the call count by 10 while still releasing empty arenas between the
six Metric panels and between Views.

One relief request per complete Metric query passed all six protected latency
checks, with reliable medians improving by approximately 0.5–2.3%. Its paired
two-View smoke lowered peak RSS from 2,780,208 KiB to 2,656,880 KiB (4.44%) and
final RSS by 4.31%. Both are below the 5% acceptance threshold, so the result
is No-change and the candidate was removed before seven-pair validation. The
four workers can finish close together and scan all zones while sibling queries
still allocate. The next alternative should request relief only when the last
outstanding read for a source completes, after the concurrent batch is idle.

Moving relief to that quiescent source boundary kept reliable query medians
within approximately -0.8% to +0.7% of the rolling baseline. Its paired smoke
lowered peak RSS from 2,780,208 KiB to 2,646,736 KiB (4.80%) and final RSS by
2.86%. The peak improvement still missed the 5% threshold, so this candidate
was also classified No-change and removed. The remaining measured compromise
is one mid-query release after five Runs plus one at completion: it can release
empty arenas before the active peak while avoiding the rejected 10-call cost.

That five-Run cadence passed all protected query medians, which stayed within
approximately -0.5% to +1.6% of the rolling baseline. Its smoke peak was
2,653,072 KiB, a 4.57% improvement, and final RSS improved by 4.83%. It did not
outperform the quiescent candidate or reach 5%, so it was rejected and removed.
Allocator relief frequency is no longer a tuning direction: the remaining peak
is active DuckDB operator working memory. The next candidate keeps four Viewer
readers but bounds the shared DuckDB scheduler's internal parallelism.

Setting that shared scheduler to four internal threads was rejected immediately
on protected latency. Reliable DuckDB Overview/full/narrow medians regressed by
approximately 49.5%/40.5%/43.1%, so the run was terminated before SQLite or
RSS measurement and the setting was removed. Neither scheduler limits nor
allocator tuning can satisfy both gates; further work must change how much data
the window/sort operators materialize without changing their exact semantics.

Removing only the semantically redundant Step `lag(axis_value)` then improved
DuckDB narrow Detail by 9.7%, but reliably regressed Overview/full by 7.7%/7.4%.
The materialized ordered window is therefore also providing a favorable plan
shape for full-span consumers. The candidate was stopped before SQLite or RSS
measurement and removed; the next plan must preserve that full-span pipeline
while specializing narrow viewport work before materialization.

## Measurement Record

- Date: 2026-07-27
- Implementation: current Entity-architecture worktree
- Platform: macOS 26.3 (25D125), arm64, Apple M4 Pro
- Rust: 1.97.1
- uv: 0.8.12
- Xcode: 26.6 (17F113)
- Metal compiler: Xcode Metal Toolchain 17.6.109.0
- Display target: ASUS XG27AQWMG at 2560 x 1440 and 280 Hz

The automated scale, final-layout CPU, build, type, and test gates pass. A
continuous Metal System Trace now covers the converged 10-Run, six-Metric
workbench on the configured 280 Hz display. Runtime drawable waits remained
below 1.2 ms and no post-warmup hang was recorded, but ten presentations
spanned two refresh periods. The final display gate therefore remains open in
Roadmap Phase 3E.

## Scale Fixture and Query Contract

The opt-in release validation generated both DuckDB and SQLite native Projects.
Each Project contained 10 finished Runs with 1,000,000 effective `loss` points
per Run. Database-side `range` generation avoided constructing million-point
Rust vectors. The retained local fixtures occupied 143 MiB for DuckDB and
136 MiB for SQLite; they are not repository artifacts.

All assertions passed:

- overview returned at most 2,000 points plus two neighbors per Run and at most
  20,020 points overall;
- detail returned at most 10,000 points plus two neighbors per Run and at most
  100,020 points overall;
- narrowing the detail viewport kept the 10,000-point budget, reduced each
  Run's source rows from 1,000,000 to 100,003, and preserved the requested
  viewport; and
- every viewer chart point matched its storage evidence point, proving that the
  viewer did not crop or resample the reduced result.

Query timings measure request submission through immutable worker snapshot
delivery and exclude rendering. Warm values are five samples.

| Backend | Query | Cold | Warm min / median / max |
| --- | --- | ---: | ---: |
| DuckDB | Overview | 1733.905 ms | 1627.727 / 1672.603 / 1718.678 ms |
| DuckDB | Full detail | 1773.152 ms | 1653.415 / 1666.751 / 1700.821 ms |
| DuckDB | Narrow detail | 686.589 ms | 674.201 / 696.890 / 721.489 ms |
| SQLite | Overview | 1767.398 ms | 1603.606 / 1639.230 / 1762.119 ms |
| SQLite | Full detail | 1714.943 ms | 1620.128 / 1680.154 / 1744.267 ms |
| SQLite | Narrow detail | 778.690 ms | 702.445 / 709.406 / 722.522 ms |

## CPU Budget

The release-only test used 10 series with 10,002 renderer-owned points each.
Every scenario passed p95 <= 8.33 ms and maximum <= 16.7 ms.

| Scenario | Samples | p50 | p95 | Maximum |
| --- | ---: | ---: | ---: | ---: |
| Brush resize | 1,000 | 0.000 ms | 0.000 ms | 0.000 ms |
| Brush pan | 1,000 | 0.000 ms | 0.000 ms | 0.000 ms |
| Brush zoom | 1,000 | 0.000 ms | 0.000 ms | 0.000 ms |
| Cached path preparation | 200 | 0.151 ms | 0.193 ms | 0.314 ms |
| Uncached path preparation | 200 | 1.988 ms | 2.076 ms | 2.167 ms |
| Hit testing | 200 | 0.202 ms | 0.236 ms | 1.996 ms |
| Ruler hover evidence | 1,000 | 0.003 ms | 0.003 ms | 0.092 ms |

After the converged renderer was integrated, an initial run failed the
unchanged uncached-path gate at p95 9.018 ms and maximum 23.815 ms. A local
Time Profiler recording identified GPUI/Lyon stroke tessellation and path
construction, rather than storage projection, as the dominant work. Complete
solid series now build their GPUI triangles directly; partial dashed evidence
retains the existing GPUI/Lyon path semantics. Three subsequent standard
release runs passed without changing thresholds. Their worst p95 was 7.485 ms
and worst maximum was 15.362 ms. Local `.trace` bundles remain uncommitted.

Multi-Run, multi-Metric manual testing then exposed two additional costs.
Viewer-only Baseline, Pinned, Archived, and visibility changes were still
invalidating every panel, and hover frames were preparing every static Metric
path again. Organization changes now retain immutable snapshots and merge only
missing Run evidence. Each Metric's static grid and curves use a cached GPUI
child view, while cursors and callouts remain dynamic. GPUI paths preserve the
first and last point plus min/max extrema in two-logical-pixel buckets; storage
evidence, chart series, hit testing, and query budgets remain unchanged. The
latest release run above reduced cached and uncached p95 by approximately 4.0x
and 3.5x respectively. A GPUI regression confirms repeated ruler-hover frames
do not prepare any static Metric chart again.

## High-Refresh Product Check

The release viewer opened a retained DuckDB Project with 10 visible Runs and
six Metric tracks on the ASUS XG27AQWMG configured at 2560 x 1440 and 280 Hz.
After a five-second warmup, a 35.8-second Metal System Trace exercised brush
resize and pan, ruler and chart pan, wheel and keyboard zoom, hover and locked
cursors, and track and inspector scrolling. A separate 20-second trace used two
persisted Views to cover repeated View switching and Bottom inspector toggling;
it reported no hang.

The trace recorded 129 single-period presented handlers at approximately
3.572 ms and ten two-period handlers at approximately 7.144 ms. Post-warmup
drawable waits had a 1.191 ms maximum and the only reported hang was a
163.08 ms startup event before warmup. The strict Phase 3E requirement permits
no viewer-caused presentation spanning two refresh periods, so this run does
not close the gate. A separate real title-bar move to the target display then
held approximately 140.57 FPS and a 7.11 ms HUD frame interval during sustained
10-Run, six-track hover, corroborating the two-period trace samples. Local
trace bundles remain uncommitted.

Generate the retained multi-track DuckDB fixture once, then launch it with the
HUD enabled:

```bash
SEEX_APP_TRACE_FIXTURE_ROOT=/tmp/seex-app-trace \
  cargo test -p seex-app --release --features test-support \
  retained_multi_track_fixture_supports_product_tracing \
  -- --ignored --nocapture

env -i \
  HOME=/tmp/seex-app-trace/isolated-home \
  USER="$USER" LOGNAME="$LOGNAME" \
  PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
  TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-en_US.UTF-8}" \
  MTL_HUD_ENABLED=1 \
  ./target/release/seex-app \
  /tmp/seex-app-trace/duckdb
```

Fixture generation refuses to overwrite a non-empty backend directory. Reuse
the retained directory for repeat traces, or remove it deliberately before
regenerating it. Metal traces embed the target process environment, so the
viewer is launched with an explicit minimal environment and trace bundles must
remain local, uncommitted validation artifacts.

## Automated Workbench Coverage

The multi-project workbench contract is covered by direct behavioral tests:

- `source_registry_reads_duckdb_and_sqlite_together`, composite-identity Core
  tests, and `panel_requests_are_partitioned_by_source_with_full_run_references`
  cover mixed backends and duplicate native identifiers;
- `source_failures_do_not_erase_other_sources_drawable_series` and
  `superseded_and_inactive_view_results_are_ignored` cover partial failure and
  stale cross-source results;
- `shared_timeline_unions_extents_from_multiple_sources`, Analysis View
  lifecycle/isolation tests, and the keyboard zoom tests cover cross-Project
  selection, shared viewport synchronization, and View isolation;
- worker coalescing, visible-overscan scheduling, aligned Metric row/track, and
  dock action tests cover query pressure, panel visibility, synchronized tracks,
  and independent dock visibility;
- `metric_click_opens_a_resizable_inspector_without_gesture_toggles`,
  whole-series Metric summary and Objective evidence assertions, retained
  inspector snapshots across zoom, and Project-scoped ranking tests cover
  click-versus-drag, Summary/Ranking/Evidence, and explicit objective direction;
  and
- workbench document round trips plus healthy, removed-Run, unknown-Metric,
  duplicate-identity, unsupported-version, and missing-source recovery tests
  cover persistence and unavailable-source reconciliation without native
  writes;
- converged GPUI tests cover Project menu anchoring and real placement,
  five-item pagination, Metric candidate removal and independent resizing,
  compact single-line Run rows with fixed controls, shared-ruler pan/zoom
  boundaries, simultaneous hover/locked cursors, dense evidence-callout
  separation and full point context, baseline deltas, icon-control tooltips,
  and narrow-window horizontal overflow of the tabular Bottom inspector; and
- final convergence regressions additionally cover full-catalog Run filtering
  beyond the revealed five-item page, all-Source refresh from an empty View,
  the converged shell in a newly created empty View, Baseline/Pinned immunity
  from Project batch visibility, exact brush/ruler/track horizontal geometry,
  compact-row plot height, and distinct logical versus physical plot widths.
- completion-audit regressions cover persisted Removed Project placement and
  cross-View cleanup, 8 px window containment for rich popovers, keyboard-only
  Run action exposure with stable icon widths, panel-generation cancellation
  when Views deactivate, and the distinction between Project-name pagination
  and Run-only search results.

## Zed Reference Audit

The workbench was compared against Zed commit
`40dc154a7cc28270d2319873b0881ef053dc22b9` using the durable source map in
`viewer-zed-ui-reference.md`. The review covered the same semantic roles and
component boundaries at compact (600 x 520), default (800 x 600), and expanded
(1440 x 900) logical window sizes.

| Concern | Pinned-reference expectation | Viewer evidence | Result |
| --- | --- | --- | --- |
| Application shell | Full-height Project panel independent of the workspace; tabs remain inside the workspace | GPUI bounds keep the Project sidebar left of the Analysis workspace and keep the tab bar exactly within the Analysis bounds at all three sizes | Pass |
| Tabs and toolbars | 32 px tab container, compact 28 px controls, one-pixel separators | The tab bar is 32 px, its selected interior is 31 px below the separator, and toolbar controls are 28 px | Pass |
| Project tree and overlays | 28 px hierarchical rows, rounded selection, disclosure icons, searchable tree, restrained popover surface | Viewer-owned tree-row and popover primitives use the pinned tokens; the source action opens its in-panel popover in the GPUI interaction test | Pass |
| Typography and spacing | Compact type hierarchy, muted secondary labels, 4 px radius, semantic panel spacing | Section labels, metadata, status, tooltip, and control text use shared theme roles; default-density tests pin 28/28/32 px geometry and the 4 px radius | Pass |
| Interaction states | Hover, active, selected, focus, disabled, pending, and error use consistent semantic roles | Shared tab, tree-row, toolbar, icon, status, tooltip, and focus-ring primitives own these states; existing keyboard, unavailable-source, loading, selection-limit, and source-error tests exercise them | Pass |
| Light and dark hierarchy | Identical structure with appearance-specific semantic palettes | Theme tests prove identical spacing and distinct window/panel/surface, text, focus, status, and series roles in light and dark appearances | Pass |
| Display scale | Logical layout remains stable while storage/render budgets use physical pixels | The renderer scale test maps a 400 logical-pixel plot to 800 physical pixels at 2x without changing layout tokens | Pass |

`application_shell_preserves_pinned_geometry_at_representative_sizes` is the
repeatable shell audit. The GPUI test host does not emulate switching the
macOS window appearance or display scale, so appearance hierarchy is verified
directly from theme roles and physical scaling is verified at the renderer
boundary. The final audit tightened Metric tracks to the reference's 52–180 px
range, adopted icon-only toolbar controls, made View close controls contextual,
compacted Run rows, and converted the inspector to fixed table columns.
The final audit also reduced the complete brush row to 40 logical pixels,
removed redundant selected-range text, and added semantic text tooltips to
icon-only controls without changing their compact geometry.

## Multi-Track Workbench Gate

`representative_workbench_stays_responsive_while_a_source_is_pending` opens a
2560 x 1800 logical-pixel workbench with 10 selected Runs, six simultaneously
visible compact Metric tracks, and the Bottom inspector. Every track receives
all 10 series. For each track, the test checks the storage-owned detail budget
`clamp(physical_width * 2, 2,000, 10,000)` and allows only the two documented
neighbor points per series. The million-point dual-backend validation above
continues to prove the same budget boundary at production scale.

The test then imports a second healthy native source. While its registry state
is still `Loading`, keyboard zoom changes the shared viewport immediately, all
10 Run selections and six panels remain intact, and the first track remains
rendered. This uncovered and corrected a pre-existing import path that reset
the active Core before background discovery; additional imports now discover
their catalog without changing the active View.

Cross-source reads remain bounded to four concurrent worker sessions by the
shared registry gate, while each source worker serializes its own native
connection. `concurrency_gate_blocks_reads_beyond_its_limit` verifies that an
extra read waits for a permit, and pending-request tests verify latest-only
coalescing per Metric panel.

The release CPU command was rerun against the final layout and direct solid
path implementation. Every p95 remained below 8.33 ms and every maximum
remained below 16.7 ms across three consecutive runs; the final run is recorded
in the CPU table above.
The representative gate is opt-in and was run with
`cargo test -p seex-app --release --features test-support
representative_workbench_stays_responsive_while_a_source_is_pending --
--ignored --nocapture --test-threads=1` so hardware-sensitive GPUI window
teardown is not part of ordinary debug test runs.

## Verification

Passed against implementation commit `f7bc0de` on 2026-07-26:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo check`
- `cargo test`
- `cargo test -p seex-app --features test-support`
- `cargo build -p seex-app --release`
- `cargo test -p seex-app --release interactive_chart_cpu_budget --
  --ignored --nocapture`
- `cargo test -p seex-app --release --features test-support
  representative_workbench_stays_responsive_while_a_source_is_pending --
  --ignored --nocapture --test-threads=1`
- `uv run maturin develop --uv`
- `uv run pyright` (zero errors)
- `uv run pytest` (106 passed, 2 opt-in MinIO tests skipped)
- `uv run maturin build --out dist`

The first chained invocation of `cargo test -p seex-app --features
test-support` reported every test as passed, then the GPUI test process received
SIGSEGV during process teardown. An immediate standalone rerun of the exact
command passed, including 43 library tests, 53 binary tests with two ignored
hardware gates, three native-pipeline tests, and doc tests. The serialized GPUI
suite also passed throughout implementation. No assertion, storage worker, or
viewer runtime failure was observed; the one teardown fault was not reproduced.
The latest full test-support run also passed without the teardown fault: 47
library tests, 58 binary tests with two ignored hardware gates, three
native-pipeline tests, and doc tests.

The retained six-metric trace fixture is a local artifact. On 2026-07-27,
`system_profiler` reported the connected XG27AQWMG at 2560 x 1440 and
280.00 Hz. The high-refresh checkbox remains open because the recorded
interaction run contained ten two-period presentations, not because the target
display was unavailable.

The Rust build emitted existing future-incompatibility warnings for `block`
0.1.6 and `proc-macro-error2` 2.0.1; warnings were not produced by Seex code
and did not bypass the strict Clippy gate.
