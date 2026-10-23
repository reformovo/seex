# Curve query workload baseline

> Current-state summary: [curve-query-performance-report.md](curve-query-performance-report.md).
> This file remains the chronological evidence log.

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
At that capture, source counts included one actual nearest neighbor per available side, even
outside the data; Desktop Detail no longer requests those neighbors (see below).
## Measurement method
Each case ran in a fresh process: one separately timed first request, an excluded calibration
pilot, then ten internally calibrated samples of at least 25 ms. All cases selected one
iteration per sample. The complete sixteen-case workload took **58.64 seconds**, within a
shared 120-second deadline. First-request timing excludes opening the `ReadSession`.
At that capture, the first request populated session diagnostics and hot samples reused
them, not a cached curve result or a cross-request DuckDB connection. Series diagnostics
were later removed (see below).
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

### First worker-boundary probe (headless)

The initial unit tests in `data::worker::tests` characterized the pre-fix
implementation. Their current names are
`ten_thousand_metric_scroll_keeps_only_retained_pending_requests` and
`completed_event_queue_grows_when_consumer_is_paused`. Run with:

```sh
cargo test -p seex-app --no-default-features --lib data::worker::tests -- --nocapture
```

The original evidence commit is `5987c18`. The first uses production
`PendingRequests` and `RequestRegistry`, with dequeue
paused to model arbitrarily slow reads. A six-identity window (four rows plus two
Overview prefetch rows) visits all 10,000 metrics, jumps to tail, reverses through
middle to head, then drains. It does **not** run the desktop scheduler, request
transport, SQL, or real reader threads. Historical pre-fix counts from `5987c18`:

- Pending requests peaked at **10,000**, despite only six retained identities.
- Cancellation left queued work resident; drain discarded **9,994**, executed six
  synthetic registrations (active peak one), and released all request tickets.
- Before the lifecycle fix, `latest` retained **10,000 entries after drain**.
- The production completion channel retained **10,000 error-only events** when its
  consumer was paused. These were component-boundary observations, not payload-size
  or normal desktop-rate claims.

Current post-fix counts from `e90f3b9` and `d68cc96` are recorded below; they must
not be substituted into the historical evidence above.

The historical debug diagnostic captured enqueue/coalescing/cancellation at
662.506 ms on this machine; it is not a calibrated benchmark or full-list UI
traversal measurement. Current probes assert registry cleanup and pending-queue
cancellation. Cancellation removes the current identity, rather than storing a
tombstone; absence invalidates old tokens. Completion releases only the matching
identity after publication. Retry teardown still retains the identity, and delayed
cleanup/cancellation cannot remove a newer identity.

`cancelled_worker_transport_drains_without_opening_storage` additionally pauses a
real worker's gate, submits/cancels 10,000 distinct metrics through its transport,
asserts zero registry entries after each cancellation, and releases the gate to
drain all tickets without events. `worker_publishes_failure_before_releasing_identity`
checks the non-cancelled error publication path. No valid store or curve payloads
are allocated. Retained panel payloads/points, stale completion repopulation,
full-list traversal/cloning, worker concurrency under slow native reads, desktop
frames, and RSS remain unmeasured. At `54b4213`, pending requests still reached
10,000; that pre-queue-fix result was **not** a bounded scheduling contract.

### Registry cancellation comparison

One preservation A-B-B-A used the exact `python scripts/performance_gate.py run
--manifest <manifest.json> --output <result.json>` entry point. A standalone Rust
harness compiled the verbatim registry methods from `5987c18` and this change with
`rustc -O --cfg test`; key/interrupt types were minimal stand-ins. Each iteration
marks and cancels 10,000 distinct keys, including allocation and registry drop.
This isolates registry lifecycle, not transport, SQL, chart rendering, or RSS.
Each capture has ten internally calibrated samples >=25 ms; the gate enforced the
120-second workload deadline. Metric: `viewer.registry_cancel_10000` (`ns/op`).

A/B/B/A medians: **1.0493 / 0.6015 / 0.6063 / 1.0509 ms** per iteration;
relative MAD: **0.456 / 0.637 / 0.667 / 0.453%**. Preservation classification: pass;
combined change -42.49%. This is not a query-latency improvement claim. No retries.
Raw harness/result files were captured in per-turn scratch (not durable fixtures):
- Baseline harness SHA-256: `ff9259535e7eea2da6348399eb41d2a4916a5d22b6540c985a04284e6824f813`.
- Candidate harness SHA-256: `150acd0c0346c368076843b26fdff32e77e5145799613bf588d7275dcf8c6cd6`.
- Result SHA-256: `08e25fc0b087237bac24c1adafa75e2172771b53d442ebf715d90cb2886b8fd9`.

### Cancellable request queue
The shared mutex/condition-variable queue now coalesces on submit and removes
cancelled pending identities immediately; waiting workers release the queue lock.
Sender teardown clears queued tickets and wakes waiters. Priority is unchanged.
The 10,000-identity scroll/tail/reversal probe now peaks at six pending requests.
The gate-paused real-worker probe asserts outstanding <=1 after every cancellation,
then drains without events. Neither is a hard cap for arbitrary submitters; callers
must cancel offscreen work. Completion events, payload/points, UI and RSS remain open.
One bounded preservation ABBA used the same gate entry point, extracting production
registry/queue methods from `54b4213` and this change (`rustc -O --cfg test`, stand-in
payload types). A paused consumer traverses 10,000 identities with a six-key window,
then drains 10,000 baseline requests versus six candidate requests. No SQL/GUI timing.
`viewer.cancel_queue_scroll_10000` A/B/B/A medians: **214.812 / 3.037 / 3.131 / 216.786 ms**;
MAD **0.361 / 0.706 / 0.503 / 0.449%**. Ten calibrated samples >=25 ms each, <=120 s;
preservation pass, no retries. Earlier `test-support` validation was blocked by missing MetalToolchain; it was installed before the P3 desktop attempt.
Raw scratch is ephemeral; SHA-256 identities:
- Baseline harness: `3f5b5ee264de1b312e39b3559afcfd3310d52745e617016264bfd1b442cbd5f1`.
- Candidate harness: `864011da1ae2b587c2e85e8500d78ba58a0ad99bc447df77e498ea7991e28fdd`.
- Result: `399081c0b55bba41aaa5be7daf8d0d193f74df8b60929fff4a58578d3cc42f73`.

### Completion-event cancellation
`ReadEvent` now carries the metric identity used by the request registry. Worker
cancellation removes matching, not-newer events that are still waiting in the
completion channel. This preserves events for newer generations and avoids
silently dropping unrelated source responses. A headless probe enqueues 10,000
error-only events for distinct metrics, cancels each identity, and asserts the
completion queue reaches zero. This bounds the cancelled-event path, not the
queue for requests that remain current and are never consumed; a separate
backpressure policy would need to preserve panel completeness while bounding that
case. No curve payloads or points are allocated.

### Panel payload residency probe
`workbench::view::tests::ten_thousand_panel_identity_scroll_keeps_payloads_to_retained_window`
creates 10,000 metric identities but only eight synthetic, empty curve snapshots.
It retains six panels, evicts hidden Detail and unretained Overview state, and sends
an older Detail completion to the evicted tail panel. Results: **6 Detail payloads,
6 Overview payloads, 0 stale repopulation**. Each snapshot has zero series and zero
points; this proves ownership/eviction behavior, not chart memory or RSS. The full
10,000-panel identity construction and traversal remains separate from query timing.
The same test now uses the internal batch-selection path and prints diagnostic
timings for identity creation and one full panel slice clone; these are profile
observations only and are not calibrated ABBA metrics. The batch path preserves
metric order, deduplicates input keys, and selects the final key like repeated
single selection. The profile improved identity construction from about 750 ms to
about 14 ms in this test. Earlier timeouts do not establish a scheduling/layout bottleneck.

A release GPUI staircase using the batch path tested 100 and then 500 metrics,
with `/usr/bin/time -l` around each process. At 100: fixture **562 ms**, discovery
**40 ms**, batch selection **3.5 ms**, first visible Overview settle **1.77 s**,
Overview retained **34**, Detail retained **0**, RSS high-water approximately
**191 MiB**. At 500, the first visible Overview settle did not complete within
**120 s**; RSS high-water was approximately **232 MiB**. The 1,000/2,500/5,000/
10,000 levels were not run after the first failure. These are diagnostic desktop
observations, not an ABBA or an SLO. The phase interpretation above is superseded
by the diagnostic correction below: 500 never reached panel selection.

### Combined headless scheduler pressure probe
`data::worker::tests::ten_thousand_scroll_cancel_drain_keeps_worker_queues_bounded`
uses a real `ReadWorker` with a gate-paused reader. It submits 10,000 distinct
metric identities, injects one error-only completion per identity, cancels each
identity, and then releases the gate to drain. At every iteration request depth is
`<= 1`, cancelled completion depth is zero, and final registry/event queues are
empty. This combines transport cancellation and event cleanup, but still uses no
valid store, SQL, curve payload, points, desktop scheduler, frame timing, or RSS.

### P3 diagnostic correction
The following earlier attempts used GPUI `TestAppContext`/`TestPlatform`, **not a
native rendered window**. Withdraw their interpretation as actual desktop/frame
measurements. Before the pagination fix, stage logging showed discovery stopped at **100 available metrics**
for fixtures of 250 and 500: `ReadSession::discover` reads only
`series_keys_page(run, None, 100)`. Neither reaches panel selection. Earlier
500/10,000 timeouts therefore do not establish Overview/layout saturation.
`overview_schedule_diagnostic` (ignored, release, `test-support`, one test thread)
logs discovery and selection separately, with a 15-second diagnostic deadline.
At 100 it settles in 1,066 ms after selection: 103 reconcile calls, 3,333 visible
panel visits, 36 Overview plans, 22,900 row visits, 2.88 ms cumulative reconcile.
Counters are process-global; they do not count Detail plans or full layout time.
RSS below includes fixture/test/Cargo processes, not retained chart allocations;
values previously labelled GiB/MiB were decimal GB/MB approximations. No native
frame-latency or isolated application-RSS result exists yet.

### Earlier GPUI test-platform attempts (superseded interpretation)
MetalToolchain was installed and the desktop/test-support target compiled:
`cargo test -p seex-app --features test-support --lib --no-run` passed. A temporary
release GPUI workload created a one-point fixture with 10,000 metric identities,
selected the Run, selected all metrics, and waited for first visible Overview settle.
It did not reach the scroll phase before the 120-second settle deadline.
`/usr/bin/time -l` reported approximately **2.56 GiB RSS high-water** on the failed
run; this is diagnostic only, not a successful GUI benchmark or an RSS budget.
The first attempt failed earlier during a compact settle path after about 44 seconds
and measured approximately 2.55 GiB; the harness was corrected to use a 10,000-metric
settle predicate before the second attempt. The temporary test was not retained
because it cannot pass its declared workload contract. This is actual desktop
evidence that current 10,000-panel construction/settling is not qualified; headless
bounds must not be substituted for frame/RSS support. After `a36422a` introduced
batch metric selection, the same release GPUI workload still failed the 120-second
first-Overview settle and measured approximately **2.52 GiB RSS high-water**. It did
not reach scrolling, so batch selection is not sufficient evidence of desktop support.

### Complete metric discovery
`ReadSession::discover` now follows the existing exclusive keyset cursor with
100-key pages until a short page; the sorted cross-Run union is unchanged. Tests
cover 0, 100, 101, and 250 keys, exact identities/order, and overlapping Runs.
No curve payloads are loaded; total key metadata still scales with discovered keys.
The same GPUI TestPlatform diagnostic now passes 100/250/500 discovery and visible
Overview readiness. Selection: **3/4/4 ms**; since-selection readiness:
**2,433/1,115/1,275 ms**. Reconcile totals: **2.71/3.89/5.95 ms**; all three
plan 36 Overviews. These are single diagnostic captures, not latency comparisons;
readiness still has two pending Overviews and is not complete worker drain.
One bounded ABBA used the exact `python scripts/performance_gate.py run --manifest
<manifest.json> --output <result.json>` entry point, extracting the complete source
module from `8ac286d` and this change, linked to the real release Seex SDK. A prepared
one-Run, 100-key local store is outside timing; sessions are reused in each process.
`viewer.discovery_100` A/B/B/A medians: **2.787/3.952/3.961/2.743 ms**;
MAD **1.46/1.19/0.80/1.67%**. Ten calibrated samples >=25 ms, <=120 s, no retries.
Classification: **regression, +43.1%**. The exact-page case needs an additional empty
page query to prove completeness; this is a correctness tradeoff, not a speedup.
Scratch raw/harness files are ephemeral; SHA-256 identities:
- Baseline: `105f338e4b33baf9c580094c4a03ff4c0dbd8c46d213d7229af0ec3a6017fc52`.
- Candidate: `eb51228d7f52b14cc05f2f855956a6f1802e9d0e83c1ac036bf1947bd5337a9a`.
- Result: `c2378ebf1d90cc08cbfb91d0c5c99f635f67ae5eabe9006bbed91c50b4fe647d`.

### Post-fix 1,000–10,000 identity diagnostics
On `fca9b6b`, the existing `overview_schedule_diagnostic` ran once per size in
fresh release/test-support processes with `--ignored --exact --nocapture
--test-threads=1` and `SEEX_DESKTOP_PROFILE_METRICS` set to each size below.
This is GPUI **TestPlatform**, one Run, one observation per metric, not a native
window measurement, ABBA, or SLO. Discovery returned all requested metric keys.

| Metrics | Fixture ms | Batch selection ms | Since-selection visible readiness ms | Reconcile total ms | Full row visits |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 1,999 | 5 | 1,106 | 10.67 | 223,000 |
| 2,500 | 5,864 | 7 | 1,188 | 23.28 | 555,000 |
| 5,000 | 16,311 | 19 | 1,704 | 41.76 | 1,135,000 |
| 10,000 | 35,953 | 29 | 1,881 | 75.41 | 2,160,000 |

All sizes reached 33 visible panels with all 33 Overviews present, zero panel
source errors, 103 reconcile calls, 3,333 visible panel visits, and 36 Overview
plans. Each still had two pending Overviews at the observation point: this is
visible readiness, **not complete coverage/drain**. Reconcile time includes work
inside reconcile, not all layout, event processing, or row rebuilds outside it.
Request-plan size stayed fixed while full-list row visits grew with metric count;
this exposes list-scale CPU work but does not establish it as the dominant wait.
Fixture preparation is outside since-selection time. Discovery elapsed time was
not separately emitted. All four diagnostics passed; no retries or scroll/frame/
RSS measurements were made, and transient logs are not durable repository fixtures.
Next validate selected/prefetched coverage and drain during scroll/reversal before
optimizing row rebuilding or claiming native-window support.

### Logical scroll, retained coverage, and drain
The existing `overview_schedule_diagnostic` now visits tail, middle, then head;
each stop must actually contain its target row, cover every Run in the selected
and directional-prefetch Overviews, and finish with no pending Overview/Detail
panel reads and zero worker outstanding tickets, active permits, and completions.
The shared concurrency gate is counted once across sources. Test-only queue accessors
are not synchronized global snapshots; peaks are sampled after the event pump.
Every sample checks View payload residency against visible/prefetch/selected IDs and
counts actual `Series::points()`, not just declared returned counts.

| Metrics | Stop | Visible range | Drain ms | View Overview / Detail snapshots | View chart points | Sampled peak outstanding / active / completions |
| ---: | --- | --- | ---: | --- | ---: | --- |
| 500 | Tail | 467..500 | 998 | 33 / 0 | 33 | 6 / 4 / 0 |
| 500 | Middle | 250..283 | 2,187 | 36 / 0 | 36 | 4 / 4 / 0 |
| 500 | Head | 0..33 | 2,878 | 36 / 0 | 36 | 4 / 4 / 0 |
| 10,000 | Tail | 9967..10000 | 1,571 | 33 / 0 | 33 | 6 / 4 / 0 |
| 10,000 | Middle | 5000..5033 | 2,794 | 36 / 0 | 36 | 4 / 4 / 0 |
| 10,000 | Head | 0..33 | 2,692 | 36 / 0 | 36 | 4 / 4 / 0 |

Both passed with no View offscreen payloads or source errors. This is one fresh
release TestPlatform process per scale, a one-Run/one-point-per-metric fixture,
not ABBA, native frames, RSS, all-process point ownership, or a hard backlog cap.
Each jump settles before the next: rapid reversal with in-flight reads is not
covered, and Detail snapshots were absent in this scenario. Zero sampled completion
depth does not mean zero historical peak. The 10,000 run took 46.96 s including
36.53 s fixture preparation. No production scheduling change was made.
Two earlier 500 attempts using `scroll_to_reveal_item(499)` timed out; the detailed
failure showed visible `9..42` despite covered=true, pending=false, queues zero.
The diagnostic now uses explicit `ListOffset` instead of unmeasured row-height
summaries, and asserts arrival. Those failed attempts are harness evidence, not
read-drain failures or a production scrolling fix.

### Cooperative cancellation after active registration and nonempty Detail eviction
`cancel_registered_read_suppresses_completion_and_preserves_return_request` uses a
real local store and `execute`, pausing at the first query cancellation callback
only after active identity registration. It cancels the old identity, registers a
new generation for the same metric, and releases the pause. The old execute returns
no event, its tracked ticket drains, and the new generation remains current and
successfully reads a curve. This is cooperative cancellation **before the curve SQL
starts**, not evidence of interrupting in-progress DuckDB SQL or a GUI reversal.
`nonempty_detail_payloads_are_evicted_and_stale_results_do_not_repopulate` populates
two synthetic Detail snapshots with two actual chart points each. Eviction drops
the offscreen snapshot's last strong owner (Weak upgrade fails), preserves the
visible two points, and rejects an old completion after eviction without a new
pending generation. No production changes or timing comparison; dense native
Detail reads, rapid GUI reversal, all-owner residency and native frames/RSS remain open.

### Dense Detail reversal integration
`cancelled_detail_reversal_integration` is an ignored release/TestPlatform
scenario with 128 metric identities. `metric-0000` has 4,096 observations so
Overview is downsampled and a selected zoom requests nonempty Detail; the other
metrics have one observation. A test-only pause confirms the Detail request reaches
active identity registration (`entered=1`) before the query body, then tail/middle/
head logical jumps are issued while that old Detail generation is paused. Releasing
it drains in **1,017 ms** in the final assertion-complete diagnostic (earlier
instrumentation-only run: 2,271 ms; intermediate assertion run: 1,064 ms). It asserts arrival at each target, clears the
old offscreen Detail generation/payload, and returns a nonempty Detail with a new
revision and no source errors; pending Detail and worker depths drain to zero.
The pause is before SQL execution: it does not
prove an in-progress DuckDB query is interrupted. It is TestPlatform, not native
frame/RSS, and no ABBA was run. Two initial one-point harness attempts did not
trigger Detail (`entered=0`); these were fixture precondition failures.

### 10,000-identity in-flight reversal extension
`SEEX_DETAIL_REVERSAL_METRICS=10000` now extends the same diagnostic, retaining the
128 default as a small regression run. Tail/middle/head/tail/head jumps occur
without waiting for the paused Detail; actual target arrival and View payload/point
bounds are asserted at every stop and again after complete drain. Only one metric
has 4,096 observations; no million-point curves are prepared.
The final 10,000 capture passed: `entered=1`, 2,855 ms after pause release to drain,
39.23 s total including fixture/setup. Each jump observed one selected Overview
payload with 1,024 points. After drain: **36 View snapshots, 2,082 actual chart
points**, a nonempty new-generation Detail, zero source errors/pending reads, and
zero outstanding/active/completion depths. The 128 run also passed with the same
final snapshot/point counts and 1,018 ms drain. These are functional diagnostic
samples, not calibrated timings or performance improvements. An earlier 10,000
run without the final residency assertion passed at 1,840 ms; the initial attempt
failed fixture preparation because one log mapping exceeded the 8,192-key limit.
Fixture writes now use 1,024-key batches without changing SDK limits.
This verifies sampled **View-owned** residency, not all chart/projection owners,
true historical queue peaks, running SQL interruption, native frames, or RSS.
Run with the existing release/test-support command, exact filter
`desktop::app::tests::performance::cancelled_detail_reversal_integration`,
`--ignored --exact --nocapture --test-threads=1`.

### SQL cancellation connection-affinity gap
`reader::tests::fresh_query_interrupt_handle_is_not_the_session_handle` now
characterizes a missing link: repeated handles from one Reader share the same
DuckDB handle allocation, but both first and retry fresh query attempts have
handles distinct from the session and from each other. Worker active registration
currently obtains `ReadSession::interrupt_handles()` from the session Reader,
whereas `query_metrics_for_desktop` executes through `with_fresh_query_reader`.
The connection-bound session handle therefore is not the running curve SQL handle.
Existing cooperative pre-SQL cancellation tests remain valid, but **do not prove
worker cancellation interrupts SQL on a fresh query connection**.
The direct `read_interrupt_stops_the_bound_connection_query` test passed too; its
25 ms sleep is not deterministic query-start evidence and does not fix this gap.
No production change was made. Next register each fresh attempt's handle against
the request generation/token before SQL, including retries and cancellation that
arrives before registration. Cleanup/late cancellation must not target a new
request. Validate actual executing SQL and subsequent worker-slot reuse separately;
no runtime dependency, connection cache, or storage-layout change is implied.

### Fresh curve-query interrupt registration
The curve worker now registers the **fresh attempt's** `ReaderInterrupt` before
SQL via a doc-hidden desktop Rust entry point; the existing SDK entry point stays
available and Python API is unchanged. The worker accepts registration only while
the request generation/token is current, replacing its active handle with that
specific connection. Retry attempts must register their own new handles. Rejected
registration executes no SQL; cancellation observed after a query error returns
None rather than retrying the interrupted curve SQL. Ordinary storage-error retries
and request-scoped fresh connections are preserved, with no connection cache.
Reader tests check handle identity before SQL, retry replacement, first-registration
rejection and retry-registration rejection. Existing worker generation/late-cleanup
regressions and the 128-metric Detail reversal integration passed (36 View snapshots,
2,082 points after drain). The earlier connection-affinity gap describes pre-fix code.
Deterministic in-progress SQL interruption/slot reuse remains a separate acceptance
step; inspector/discovery query cancellation is not covered by this curve fix.
One bounded ABBA used `python scripts/performance_gate.py run --manifest
<manifest.json> --output <result.json>`, ten calibrated samples >=25 ms, <=120 s,
no retries. Real release SDK binaries, same dependency feature set: baseline was
linked before the production edit; candidate registers the fresh handle into a
mutex slot, not a complete worker registry. A prepared one-Run/4,096-observation
local fixture is outside timing, budget 64, full Step range, one curve. Each request
asserts 64 returned points; sessions are reused, fresh query connections are not.
`query.fresh_interrupt_route` A/B/B/A medians **42.689/43.000/42.529/42.572 ms**;
MAD **0.568/0.991/1.600/1.600%**. Preservation pass, combined change **+0.31%**,
not a speedup or cancellation-latency comparison. Ephemeral scratch SHA-256:
- Baseline binary: `e32150901a73e7adaa8cca1e36207ad6024c47537cf70311e49facae18484952`.
- Candidate binary: `600037f6c54137537053f7b5b5225b3fa7387bcfca75504080a4566c397adafd`.
- Result: `f0dafe431bc12d9456daa71da613b3d40dcb38b394a92f86dd49522e9443b83c`.

### Executing SQL interruption without a sleep-based start assumption
`registered_fresh_handle_interrupts_executing_sql_and_next_attempt_succeeds` uses
`with_fresh_query_reader_registered` against a local fixture. The fresh connection
creates a probe sequence in its in-memory catalog and runs a long aggregate that
advances that sequence. A separate connection to the same database observes
`duckdb_sequences().last_value > 0` before the registered handle is interrupted.
The query must return an explicit INTERRUPT error (not just any Storage failure),
then the same connection must successfully run `SELECT 7`. After that connection
exits, calling the old handle must not affect a later fresh `SELECT 9`.
Progress/result waits have 10-second deadlines and failure cleanup interrupts the
probe too. This is safe Rust, no dependency/production SQL change, and no benchmark.
It proves actual SQL execution and connection recovery on the fresh-handle path;
it does **not** run the native curve query or the complete app worker loop, so
end-to-end worker-permit release/reuse during interrupted SQL remains unverified.

### Worker cancellation of executing SQL and queued-request slot reuse
SDK feature `test-query-probe` is opt-in and enabled only through seex-app's
dev-dependency. Normal builds have no probe branch, state, or watchdog. It matches
one unique test Run/key and inserts a one-shot long sequence aggregate inside the
actual fresh curve-query path, after the real interrupt registration. The guard
observes sequence advancement through a second connection, not a sleep. Guard
cleanup interrupts only on drop; an associated watchdog handles early-failure
races and cannot satisfy the test's interrupt assertion before guard drop.
`cancel_executing_sql_releases_worker_slot_for_queued_curve` uses real ReadWorker
transport, executor, registry and cancellation with a single reader/permit. After
SQL advancement is observed, it queues a different metric (outstanding=2, active=1),
cancels the old request, requires an explicit SQL INTERRUPT result, and receives
only the new generation's successful nonempty curve. Outstanding tickets, active
permits, registry entries and completion queue drain; the reader thread joins.
This is end-to-end worker slot reuse with an injected executing SQL probe, not
proof of interrupt latency for the production LTTB/neighbor SQL operators. No
runtime dependency, Python API, production SQL or connection-cache change; no ABBA
because normal-build behavior is unchanged. The probe feature must remain opt-in
and its global arming should only be used by serialized integration tests.
Reproduce with `cargo test -p seex-app --no-default-features --lib
 data::worker::tests::cancel_executing_sql_releases_worker_slot_for_queued_curve
 -- --exact --nocapture` (app dev-dependency enables the SDK probe).

### Enqueue high-water marks and retired chart/cache owners
Test-support now records pending queue depth immediately after coalescing/enqueue,
completion depth immediately after enqueue, and the existing shared-permit peak.
Marks are cumulative per open worker lifetime, not reset per jump. With one source,
both 128 and 10,000 dense-reversal diagnostics observed **[4 pending, 4 active,
4 completions]**. These are separate maxima, not simultaneous depths or hard caps;
pending excludes requests already taken by readers. For multiple sources the
accessor sums pending/completion maxima (an upper bound) and counts the shared
active gate once. Worker tests check a two-request pending peak and preservation
of the 10,000-event peak after cancellation empties its queue.
After dense Detail returns and all reads drain, the diagnostic captures Weak
references to the actual chart Detail snapshot and its ChartAdapter, confirming
snapshot identity against the View. It then jumps to tail, drains and pumps events;
both Weak upgrades must fail. This checks that no View/row/chart/canvas reference
retains that snapshot or adapter, including the adapter-owned projection/path
caches. It does not count independently cloned snapshots/Series, externally cloned
projected paths, GPUI frame/path copies, bytes, allocator retention, or RSS.
At 10,000, final tail View residency was **34 Overviews / 0 Details / 1,057 points**:
the selected dense Overview (1,024 points) remains intentionally pinned. Old Detail
and cache owner are gone. Release TestPlatform run: 1,799 ms initial reversal drain,
1,921 ms tail drain, 40.12 s total including fixture. The 128 run also passed.
No production-build counters/hooks, scheduling change, benchmark or ABBA; this
extends owner-lifetime evidence, not a full-process memory bound.

### Native macOS window diagnostic (not TestPlatform)
**Historical readiness caveat:** the following original capture checked snapshot
presence and drain, not nonempty current chart paint. Most metrics had only one
point, which need not produce a polyline. Those callback/RSS numbers are startup/
state-drain observations, not evidence of fully loaded/drawn curve scrolling.
See the stricter loading/paint validation below; do not reuse the earlier timing
as time-to-visible-curves or claim low callback latency implies loaded charts.
An explicit `test-support` entry now uses `Application::new()` with a native
1200x800 window. Set `SEEX_NATIVE_PROFILE_ROOT` to a prepared isolated local store,
`SEEX_NATIVE_PROFILE_HOME` to a home inside that fixture root, and
`SEEX_NATIVE_PROFILE_METRICS` to the exact count; run the release `seex-app` binary.
No default CLI/API or scheduling change. Build and fixture preparation precede
measurement; no user's ordinary home configuration is used. The profile selects
one Run/all metrics, waits for visible Overviews and full pending/worker drain,
then logically jumps tail/middle/head, asserting target arrival at every stop.
It requests redraws and records after-frame callback intervals across the run,
not GPU time or display-presentation latency. No continuous wheel-input workload.
An external `ps -o rss= -p <app-pid>` sampler polled about every 50 ms (plus command
cost), tracking only that application process, not Cargo or fixture construction.
RSS is the maximum sampled resident set, not OS peak footprint or allocation bytes.
One Run: one point per metric except metric-0000 with 4,096 observations.

| Metrics | Initial drain ms | Tail ms | Middle ms | Head ms | Callback count | Callback p95 / max ms | Sampled max RSS bytes |
| ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| 500 | 3,416 | 1,099 | 2,183 | 2,250 | 540 | 17.18 / 186.62 | 279,953,408 |
| 10,000 | 2,136 | 748 | 1,358 | 1,507 | 341 | 30.38 / 155.77 | 357,498,880 |

Native result markers reported success; process times 10.07/8.37 s, RSS samples
164/138. The 500 capture preceded adding an explicit discovery cardinality guard;
its logged panel count was 500. These are single diagnostic observations, not
ABBA, comparable scalability ratios, or a frame/RSS SLO. The sampler and raw fixture
are ephemeral scratch artifacts, not repository fixtures. Two earlier native
harness startups aborted before workload: animation-frame request outside render,
then reentrant WindowHandle update inside a frame callback. Both call sites were
corrected; their startup RSS is excluded, and no library/UI workaround was applied.
Next qualify sustained scrolling/in-flight reversal and per-phase callback latency;
these successful logical jumps do not establish broad 10,000-panel desktop support.

### Explicit curve-loaded/painted readiness before native measurement
The native diagnostic now requires the target viewport, Complete per-Run evidence,
at least two actual chart points per visible curve, no source errors/pending reads,
zero worker depths, and a nonempty native paint submission for each current chart
revision **and viewport**, stable for two consecutive callbacks at that target.
Paint completion is instrumented in the canvas paint callback, not projection warmup.
A unit test rejects absent/empty paint, old revisions and mismatched viewports.
It is not a GPU/presentation fence. Two-point fixtures (Steps 0/100) replace the
single-point baseline; metric-0000 still has 4,096 observations. Motion phases use
four-row logical steps every third callback for three seconds, followed by three
seconds of alternating head/tail offsets and final head load/paint/drain. This is
synthetic logical input, not OS wheel events. Motion need not wait for reads;
`unready_motion_frames` reports frames not having all visible current charts painted.

At 500, revision-paint validation passed: initial/tail/middle/head readiness
1,806/449/6,834/1,683 ms. Middle had a **6,645.96 ms** callback outlier (not removed).
Motion callback p95 was 16.68/16.68 ms, but **177/180** and **180/180** callbacks
were not all-charts-painted. Final loaded/painted drain took 1,600 ms. Sampled app
RSS maximum 307,314,688 bytes. These are not successful loaded-curve scrolling rates.
At 10,000 the first strict run timed out at initial readiness (32.56 s process,
RSS 336,363,520 bytes), before motion. A diagnostic rerun adding timeout status
completed (motion unready 177/180 and 180/180, final drain 2,583 ms, RSS 395,739,136
bytes). After additionally enforcing viewport identity and two target-local ready
frames, final validation again failed initial readiness: **loaded=14, painted=13,
visible=0..14, depths=[0,0,0]**, 21.30 s process, RSS 330,596,352 bytes. Keep all
these outcomes; no best-run selection or calibrated ABBA comparison is justified.
The missing paint's cause is unresolved (do not infer a query backlog or fix by
loosening readiness). Ten-thousand-panel fully loaded/drawn native scrolling is
**not qualified**. Old callback-only successes must not override this result.
Normal builds have no paint instrumentation; no production scheduling/SQL change.

### Unpainted-panel attribution and valid native fixture order
Test-support now retains canvas prepaint bounds and content-mask bounds; native
diagnostics print unready panel index/ID/key, row/list bounds, evidence/reasons,
point counts, current revision/viewport and last paint. Output occurs once per
phase when all snapshots exist but paint is incomplete, and again on failure.
One newly captured failure was **index 0, metric-0000**, row `(190,100,1010,52)`
fully inside list `(190,100,1010,700)`, with **Invalid evidence, no chart series,
no chart entity**. It was not the partially visible final row. This run later
required external termination at 110 seconds; no performance success is claimed.
Another run of the old fixture completed: intermittent success does not erase the
captured invalid-data failure. Earlier failures without panel details remain
unattributed individually.
The old fixture wrote dense Step 100 before dense Steps 1..99. A direct SDK
reproducer without a timestamp gap sometimes returned Complete; inserting an
explicit 10 ms gap for this fixture-order reproduction returned **Invalid,
DecreasingAxis**. This is a data-validity issue, not a SQL-start timing assumption.
Correct preparation logs all initial Steps 0, then dense Steps 1..4095, then sparse
Step 100 endpoints **excluding the dense key**. The added fixture regression checks
Complete evidence, 4,096 dense / two sparse source points and nonempty samples.
Native diagnostics now fail immediately on visible Invalid evidence and print
reasons, rather than treating snapshot presence as curve readiness. Production
validity, rendering and scheduling rules are unchanged; readiness was not loosened.
With corrected 10,000 fixture, SDK dense preflight was Complete/reasons empty;
initial/tail/middle/head/final readiness took **2,432/1,266/2,466/2,200/1,216 ms**,
ending loaded=14, painted=14, queues zero. Motion still had **178/181** and
**180/180** not-all-painted callbacks. This single diagnostic is not ABBA or a
scrolling-performance pass; no RSS was captured in this final attribution run.
The next performance question is visible-curve catch-up during movement, not a
proven final-row clipping defect. Original raw probes remain ephemeral scratch.

The native trace records first observations per logical target (not every panel
entering the viewport), retaining missing events as None. It observes visible,
Overview-pending, Complete per-Run Overview with >=2 points, and current chart paint.
It reports painted/left-viewport/stage-end outcomes; it does not overwrite first
observations or fabricate pending timestamps for cached data. Times are callback
observations, **not** exact enqueue, worker start, SQL finish, or event-apply times.
An initial 500 capture reused the old timestamp-order fixture (no Invalid surfaced
in that run); valid-order/preflight-Complete fixtures were then used for both scales:

| Metrics / motion | Targets visible | Pending observed | Complete data observed | Current paint observed | Painted / left / stage-end outcomes |
| --- | ---: | ---: | ---: | ---: | --- |
| 500 forward | 59 | 54 | 5 | 5 | 5 / 53 / 1 |
| 500 reverse | 59 | 58 | 0 | 0 | 0 / 58 / 1 |
| 10,000 forward | 60 | 57 | 3 | 3 | 3 / 56 / 1 |
| 10,000 reverse | 60 | 59 | 0 | 0 | 0 / 59 / 1 |

Most incomplete targets left after about 66 ms. Visible/pending were first seen
about 16–18 ms after setting the logical target. Successful data/paint observations
were on the same callback and may be cached; neither gives an uncached latency
estimate. At 10,000, the initial readiness took 10,349 ms with a **7,816 ms callback
outlier**, not discarded. Both final stops passed loaded=14/painted=14/queues zero
(700/1,216 ms final drain at 500/10,000). These single instrumented diagnostics,
with per-target log overhead, are not ABBA, wheel-input validation or a performance
pass. They locate most sampled target failures before usable Overview arrival,
not specifically in paint; queueing/connection/SQL/event-apply shares remain
unknown. Next capture actual per-generation enqueue/start/finish/apply events.
The trace unit test verifies first observations, censored outcomes, and no invented
pending event for cached curves. No production scheduling change or RSS capture. Event traces are only armed by the
native profile after metric selection; normal test-support runs that do not arm
trace have no collection. The cap/drop counter makes missing samples explicit.

### Per-generation read timeline
Test-support event traces now include source, generation, kind and enqueue/start/
finish/event-queued/event-apply/cancel/discard phases, capped at 8,192 events with
a reported drop count. The native profile groups only matching source+generation+
kind events and reports complete-sample counts; incomplete phases are not put into
medians. This is diagnostic wall time, not SQL-only time: fresh connection open,
metadata/diagnostics, query, worker overlap and UI apply are included in the
corresponding intervals. The event is stamped inside the completion queue lock
before wakeup; apply is stamped at coordinator entry.

Valid-order release native traces (500 / 10,000) showed:

| Motion phase | Metrics | Complete timeline samples | Queue→start median ms | Worker start→finish median ms | Event queue→apply median ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Forward | 500 | 131 / 127 / 2 | 22.17 | 66.19 | 0.013 |
| Reverse | 500 | 156 / 152 / 0 | 24.11 | 62.31 | — |
| Forward | 10,000 | 126 / 122 / 0 | 21.41 | 66.20 | — |
| Reverse | 10,000 | 145 / 141 / 0 | 17.34 | 66.99 | — |
| Final stable | 500 | 369 / 369 / 83 | 17.58 | 66.10 | 0.018 |
| Final stable | 10,000 | 355 / 355 / 81 | 14.89 | 69.25 | 0.259 |

No trace events were dropped. Motion observed complete data/paint for only 5/59
forward and 0/59 reverse targets at 500; 3/60 and 0/60 at 10,000. Thus most
motion misses are requests still in queue/worker or not yet applied when targets
leave the viewport; the tiny apply medians are based on few complete samples and
are not a general UI latency result. Next production decision requires a larger
per-generation sample policy or controlled input rate, not immediate SQL batching.


### Per-generation motion-stage timestamps
The native profile now records test-support events for each generation: enqueue,
worker start, worker finish, completion enqueue, and coordinator apply. It reports
medians for enqueue→start, start→finish, and completion→apply at final exit. These
are wall-clock observations from one process, not SQL-only timings; fresh connection
open, query stages, event pumping and chart paint overlap them. Missing phases are excluded from that median and must be reported by event count.
The trace cap is 8,192 events with an explicit dropped count; phase counts are
reported for each stage. Reader integration currently labels fresh connection
open and query start/finish around the complete fresh operation; it does not split
metadata, diagnostics, bounds, native LTTB or Arrow consumption into separate
stamps. This instrumentation is the next step for distinguishing visible-target
misses caused by queueing, worker/SQL, or event application; it is not enabled in
normal builds.

### Sampled Reader stage capture
Trace now selects whole generations by a deterministic mixed-generation hash,
`SEEX_READ_TRACE_STRIDE` (default 8, 1 selects all), before locking/allocating an
event. Selected events use a preallocated 4,096-slot cap with dropped accounting.
This reduces collection, not a measured instrumentation speedup. Summary cohorts
are Overview-only, keyed by source/generation/kind; repeated start/end events
(retries or multiple curves), missing and reversed pairs are excluded and counted.
Empty sets report None, not zero. Sampling tests cover disabled/stride-1/stable
selection; pair tests reject missing, repeated and reversed timestamps.
Valid-order, dense-preflight-Complete 500/10,000 native captures both completed
loaded=14/painted=14/queues zero with 441/420 retained events and no drops.

| All captured Overview generations | 500 median / p95 ms (n) | 10,000 median / p95 ms (n) |
| --- | --- | --- |
| enqueue→worker start | 11.27 / 44.10 (43) | 6.86 / 40.47 (38) |
| worker start→finish | 67.65 / 131.77 (43) | 72.36 / 1,172.17 (38) |
| fresh open | 61.23 / 80.24 (43) | 54.77 / 1,111.12 (38) |
| metadata | 0.606 / 3.493 (12) | 0.733 / 1.363 (12) |
| diagnostics | 26.33 / 37.57 (12) | 74.59 / 113.82 (12) |
| storage operator envelope | 9.56 / 50.99 (12) | 63.56 / 99.32 (12) |
| points conversion | 0.00054 / 0.00083 (12) | 0.00071 / 0.00108 (12) |
| queued event→coordinator entry | 0.015 / 34.25 (11) | 0.016 / 5.93 (10) |

In forward/reverse cohorts, fresh-open medians were 62.66/61.23 ms at 500 and
57.92/55.21 ms at 10,000; most cancelled attempts never reached metadata. Inner
phase sample sets differ from outer ones: **do not sum medians or compute stage
shares**. Kind/source pairing does not correlate requests to a particular motion
target or paint event yet. The 10,000 fresh-open long tail is retained. No retry,
RSS or ABBA; no native LTTB/neighbor-operator decomposition. Scratch log SHA-256:
500 `0677e43d0ad143f7145298c6de311a2d5614cbc090bce82f1c13f820a9be6cb7`;
10,000 `87a7b4285dea76e71bac8a0d86dd7718713386aca8ba555b5bcee39127c67d3c`.
Prior unsampled timeout remains unclassified; success here does not prove trace
caused it. Next investigate connection-open/cancellation cost without connection
caching, retaining explicit loaded/painted and unfinished-request criteria.

### Cancellation before and during fresh open
The desktop request now rechecks current/cancelled state before **each** fresh
open, including retries. Existing non-cancellable helpers retain their behavior;
no API, connection cache, SQL or snapshot policy change. Tests verify an already
cancelled first attempt never opens/registers/queries and a cancelled retry skips
its second open. Post-open rejected registration is now explicitly traced.
Native diagnostics compare a single cancel-call timestamp with a single completed
open interval; repeated/missing intervals remain unclassified. This records the
cancel call, not its registry-lock acquisition or a SQL interrupt timestamp.
With stride 8, valid fixtures and final loaded=14/painted=14/queues zero:

| Metrics / Overview cohort | Cancel called before / during / after open | Post-open registration rejected | Rejected then metadata | Pre-open skips |
| --- | --- | ---: | ---: | ---: |
| 500 forward | 0 / 17 / 0 | 17 | 0 | 0 |
| 500 reverse | 0 / 17 / 1 | 17 | 0 | 0 |
| 500 all captured | 0 / 36 / 2 | 36 | 0 | 0 |
| 10,000 forward | 0 / 16 / 0 | 16 | 0 | 0 |
| 10,000 reverse | 0 / 17 / 0 | 17 | 0 | 0 |
| 10,000 all captured | 0 / 35 / 0 | 35 | 0 | 0 |

No events dropped. No new skips were observed: the pre-open rule fixes a race/
retry boundary, **not** the dominant waste after open has started. Existing rejection
already prevents metadata/storage on those cancelled generations. Motion target
data/paint readiness did not materially improve; do not claim a browsing speedup.
One bounded ABBA used the exact performance_gate.py run entry point, real pre-edit/
candidate release SDK binaries, one prepared local Run, 4,096 observations, full
Step range, budget 64, sessions reused but query connections fresh. Each request
asserts 64 returned points, with mutex handle registration and no cancellation.
`query.preopen_cancel_check` A/B/B/A medians: **41.148/40.794/41.692/40.903 ms**;
MAD **1.173/0.383/2.368/0.936%**. Ten calibrated samples >=25 ms, <=120 s, no retry.
Classification **inconclusive** (candidate noise above limit), not preservation or
acceleration. Ephemeral result SHA-256:
`7aa161c8e0a2c152b147d7449e3ff05c098de6460e14d1187bba182b0a9bed2f`.
### Connection-open subphase attribution
The existing `SEEX_WRITE_TIMING=1` bootstrap timings were joined with the sampled
Reader trace (no new instrumentation): DuckDB open, DuckLake attach, catalog
adapter, format validate, and overall reader connection open. Valid-order native
runs used stride 8; stage sample sets remain non-identical and must not be summed.

| Metrics | DuckDB open median / p95 ms | DuckLake attach median / p95 ms | Adapter median / p95 ms | Format validate median / p95 ms | Reader open median / p95 ms |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 500 | 2.54 / 9.78 (n=351) | **53.03 / 82.10** (351) | 0.079 / 0.256 (351) | 1.62 / 14.00 (351) | **60.97 / 93.68** (350) |
| 10,000 | 2.59 / 10.62 (345) | **51.15 / 78.57** (344) | 0.080 / 0.200 (344) | 1.41 / 9.12 (345) | **56.58 / 86.44** (344) |

The existing `native_execute` / `native_consume` timing emits only when storage
operations are reached and is not paired one-to-one with sampled generations;
it is not used to derive the rows above. DuckLake attach dominates ordinary fresh
open by these observations, but no cache or reuse is approved: request-scoped
fresh snapshots and cancellation ownership remain product constraints. This is
profiling evidence, not ABBA or an optimization result. A 10,000 run completed
with valid data/paint/queues, but this instrumentation can alter scheduling; no
RSS/frame claim follows. The combined `connection_ducklake_attach` timer includes
**INSTALL + LOAD + ATTACH**, not solely the DuckLake snapshot attach.

An isolated release Rust probe used the existing local panel store with a new
in-memory DuckDB connection each iteration, the same SQLite read-only attach
options/aliases, and no curve reads. It timed INSTALL, LOAD and ATTACH separately.
Ten internally calibrated whole-operation batches per workload were >=25 ms;
all workloads finished within 120 s. This is a profile, not ABBA. The first
sequential capture observed INSTALL 0.07–0.10 ms, LOAD 13–16 ms, ATTACH 9.7–10.4 ms.
A second explicitly synchronized 1/4-thread probe yielded:

| Parallel connections per wave | DuckDB open median / p95 ms | INSTALL median / p95 ms | LOAD median / p95 ms | ATTACH median / p95 ms | Whole wave median ms |
| ---: | --- | --- | --- | --- | ---: |
| 1 | 2.18 / 2.32 | 0.071 / 0.086 | 13.69 / 14.54 | 9.78 / 10.45 | 26.07 |
| 4 | 3.98 / 4.21 | 0.099 / 0.147 | 51.46 / 70.65 | 24.67 / 33.92 | 85.79 |

There were ten calibrated samples, four serial waves/sample versus one parallel
wave/sample, with 40 per-connection observations in each case; minimum complete
sample times were 102.66/80.65 ms. Threads were spawned per wave, unlike persistent
app workers. Wall time includes teardown/thread overhead; subphase sample sets
must not be summed by median. Warm OS/extension caches were not cleared. Raw Rust
probe/fixture references are ephemeral scratch, not repository fixtures.
The duplicate INSTALL is **not** a meaningful latency source, so a LOAD-first/
install-fallback production change is rejected for now. Parallel LOAD/ATTACH wall
times increase, but no internal lock or mechanism is identified. One four-way wave
opens four connections versus one serial connection: latency alone does not justify
lowering production concurrency or asserting a throughput improvement. No cache,
reuse, SQL/validity change or production performance claim. Next test 1/2/4 native
read capacities only via a diagnostic control, preserving loaded/current-painted
curve ratios, cancelled/unfinished cohorts and final drain; do not tune to open
microtimings at the expense of actual visible-curve completion.

### Diagnostic read-capacity control
The native profile has an opt-in `SEEX_NATIVE_READ_CAPACITY` control restricted to
1/2/4; it replaces the registry's shared diagnostic gate only and is not a
production default/concurrency setting. A valid-order 500 fixture completed the
same loaded/painted/drain workflow at all three capacities:

| Capacity | Forward complete data/paint | Reverse complete data/paint | Sampled RSS max |
| ---: | ---: | ---: | ---: |
| 1 | 4/59 | 2/59 | ~220 MiB |
| 2 | 5/60 | 0/60 | ~227 MiB |
| 4 | 4/60 | 0/59 | ~251 MiB |

One capture per capacity; not ABBA or a tuning result. Capacity 1's extra reverse
hits are noise/fixture scheduling, not a claimed improvement. A 10,000 run with
capacity 1 completed initial/tail phases then stopped producing phase markers before
the external 100-second limit; capacity 2 completed through the head phase then
stopped similarly; capacity 4 was not completed before the combined tool deadline.
These are incomplete diagnostics, not failure rates. A test-only watchdog was added
for the native profile (heartbeat location, stage, idle duration, status); the
following valid 10,000/capacity=1 rerun completed stage 0–9 with no watchdog idle
report, so the earlier external 100-second stops are **not a reproduced application
hang**. Do not lower production concurrency. Native read-capacity override remains
test-only; no production behavior or tuning conclusion follows.



### Current Detail range matrix after resource-boundary work
The prepared `.tmp/v5-panel-million/store` fixture was recreated in this workspace
outside query timing (16 Runs, 1,000,001 observations each). The existing
`panel_gate` acceptance path ran all eight Detail ranges at 1 and 16 curves,
release, single test thread, ten calibrated samples per case, each sample >=25 ms;
all 16 cases passed completeness/source/point-count assertions. These are baseline
measurements, not a new ABBA or optimization result; raw logs are ignored local
scratch. Hot medians (ms):

| Range | 1 curve | 16 curves |
| --- | ---: | ---: |
| full | 77.9 | 687.4 |
| prefix | 61.6 | 433.8 |
| middle | 67.2 | 525.1 |
| window | 61.4 | 428.0 |
| small | 61.1 | 430.3 |
| small-middle | 61.2 | 425.3 |
| tail | 61.6 | 428.4 |
| empty | 61.3 | 428.4 |

The one-curve non-full floor is roughly 61–67 ms. Sixteen-curve narrow/outside
cases are roughly 425–434 ms, middle is about 525 ms, and full is about 687 ms.
The matrix does not separate connection/metadata/diagnostics/bounds/storage/LTTB
stages; do not infer the next SQL change from these medians alone. The required
fixture is now local but ignored; it is not a repository fixture or raw evidence
commit.

The corrected request/attempt/Run stage profile now confirms the fixed cost is in
per-Run storage work, not points conversion. Hot medians from the 12-sample
representative traces (all 8 cases were parsed with the same grouping):

| Curves / range | Reader total | Fresh open | Per-Run diagnostics sum | Per-Run storage sum | Points sum |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 / prefix | 58.96 ms | 27.10 ms | 0.83 ms | 30.26 ms | 0.025 ms |
| 1 / full | 73.56 ms | 27.15 ms | 0.88 ms | 44.62 ms | 0.026 ms |
| 16 / prefix | 417.82 ms | 27.51 ms | 15.24 ms | 369.91 ms | 0.414 ms |
| 16 / full | 667.36 ms | 27.28 ms | 18.51 ms | 616.02 ms | 0.430 ms |
| 16 / middle | 510.62 ms | 27.04 ms | 16.74 ms | 461.27 ms | 0.424 ms |
| 16 / empty | 421.49 ms | 27.03 ms | 14.99 ms | 374.29 ms | 0.325 ms |

For each 16-curve request, storage sums are per Run and reader total is the
request wall time; do not sum reader totals or compare first metadata timestamps
as if they were one Run. The 16/full storage envelope is about 92% of matched
reader time; 16/prefix about 89%. Bounds/neighbor work and native LTTB remain
inside this storage envelope. This supports profiling one curve/range storage
plan next; it does not yet select bounds reduction over request-level batching.
Representative traces used `SEEX_PANEL_STAGE_TIMING=1`; they are diagnostics,
not calibrated ABBA, with OS caches not cleared and fresh connections per request.
Raw logs are ignored scratch.

### Target-to-generation correlation (diagnostic infrastructure)
Motion traces now attach the first observed Overview generation/revision to each
logical target where it exists. This is an attribution aid only: a completed
revision can predate the motion target, pending generations can be superseded, and
one generation may cover more than one panel/source partition. The trace unit
coverage still preserves missing generation and paint events; no stage is inferred
from target index alone. Per-generation read phases are sampled separately and
must be joined by source+generation+kind, not by timestamp nearest-neighbor.
The next useful output is a small cohort table of target generation plus enqueue,
worker, storage envelope, apply and paint observations; no production fix is
justified until that join has sufficient complete rows. The native profile now
prints each target/generation cohort's observed states and phase set, plus joined/
missing totals. A generation is joined only by exact source/generation/kind event
identity; target proximity is never used. This output is diagnostic and can omit
sampled generations by design, so it cannot be interpreted as complete workload
coverage until joined counts and trace stride are reported. Cohorts now also carry
source identity, pending-vs-cached origin, and censored end reason (left viewport,
stage end, or painted); phase sets are filtered by exact source+generation+kind.
A stride=1 valid-order 500 capture joined **59/59** forward target cohorts and the
reverse cohort joined **59/60** (one stage-end target had no sampled generation).
The forward cohort had 55 targets observing pending, 4 observing complete data and
4 observing paint; reverse had 58 pending and **0 complete/paint**. Most pending
cohorts contained enqueue/start/fresh-open/cancel/register-rejected/finish without
metadata or storage. A small minority reached metadata/diagnostics/storage before
cancellation and could produce complete data. This is the clearest current
explanation of motion misses: requests are usually cancelled while opening or
before registration, not delayed at coordinator apply. It still does not identify
why fresh open takes its measured time or prove SQL work never began for every
cohort. The 10,000 joined output was also collected; it is not used as a timing
comparison, and raw logs remain ephemeral.

### Detail stage trace: representative cases
With the prepared v5 panel fixture, `SEEX_PANEL_STAGE_TIMING=1` traced four
representative Detail cases (1/16 curves × prefix/full), one fresh process per case,
12 repeated request samples from the existing diagnostic loop. These are profile
observations, not ABBA: trace output is not calibrated timing input and fixture/OS
caches were not controlled.

| Curves / range | Fresh open median ms | Storage envelope median ms | Points conversion median ms |
| --- | ---: | ---: | ---: |
| 1 / prefix | 29.67 | 32.49 | 0.015 |
| 1 / full | 32.10 | 48.76 | 0.016 |
| 16 / prefix | 27.44 | 22.59 | 0.012 |
| 16 / full | 28.51 | 38.19 | 0.015 |

For 16 curves, `query_start→first metadata_start` spans the sequential 16-Run
loop and must not be compared with the one-curve interval. Metadata/diagnostics/
storage phase entries are per Run but the current trace lacks RunId, so their
cross-curve aggregation is intentionally not used for a decision. Points conversion
is negligible in these samples. The useful current conclusion is that fixed fresh
open plus per-Run query work dominate; this does not yet distinguish bounds/neighbor
scans from native LTTB/storage consumption. Next add RunId to the opt-in trace or
profile one curve at a time inside a 16-curve request before choosing bounds versus
request batching. No production SQL change or ABBA was performed.

### Storage operator profile: bounds/neighbor versus native sampling
A release ignored storage profile now captures `EXPLAIN (ANALYZE, FORMAT JSON)` for
outside-only, prefix, middle and full on the local v5 panel fixture. It refuses to
overwrite an existing output directory and validates source/returned counts. This
is plan evidence, not a timed ABBA; operator timings are not summed as wall-clock
and DuckLake `operator_rows_scanned` is not treated as physical row count.

The key plan finding is structural:

- outside-only returns one real neighbor, but its `step_bounds` aggregate scans the
  full one-million-step relation across **62 Parquet files**;
- prefix returns 2,050 source rows from **1 file**, but its bounds discovery still
  scans the full relation across 62 files;
- middle returns 100,004 source rows from **8 files**, while bounds discovery still
  scans the full relation across 62 files;
- full uses the bounded/full path and reads all observations as expected.

Thus the narrow/empty latency floor is not native LTTB alone: nearest-neighbor
bounds discovery performs a separate full relation scan, and the effective result
path is another scan. This identifies the first candidate optimization boundary.
It does not authorize replacing real neighbors, dropping duplicate Step observations,
or using physical position/LWW. Any fix must preserve append-only v5 semantics and
DuckLake snapshot/file-list ownership. Next compare a minimal bounds-scan reduction
against the current plan, with all eight range correctness cases and one/16 curves;
request-level SQL batching remains a later alternative, not the assumed answer.
The opt-in profile test was `storage::alignment_query::tests::detail_storage_operator_profile`;
raw JSON/SQL output lives only in ignored `.tmp/v5-storage-operators/` scratch. The module
was deleted with the neighbor SQL; reproduce that probe from `076913b`. The static-bounds,
batched-bounds and bounds-gate probes described below were scratch-only and never committed;
their raw plans remain under `.tmp/`, but they no longer exist in the tree.

### Rejected candidate: separate predicate Top-N bounds
Compared baseline `076913b2072b4cbbd3100121fcc6dffac7d1f0c1` with a minimal
replacement of `step_bounds`: independent left/right Step scans, each filtered by
identity and its strict viewport predicate, ordered descending/ascending with
`LIMIT 1`. Scalar subqueries retained NULL for an absent side. The effective-row
scan and native sampling were unchanged. This added a third relation reference;
Parquet/identity parameter bindings were repeated for all three references.
**The production change was withdrawn**, not retained as an optimization.

One bounded A-B-B-A used the exact gate command:

```sh
python scripts/performance_gate.py run \
  --manifest .tmp/v5-bounds-comparison/manifest.json \
  --output .tmp/v5-bounds-comparison/result.json
```

Each capture ran the existing release `panel_gate` for prefix, middle and outside
(`empty`), with one and sixteen curves, validating every measured request.
Each metric has ten calibrated samples >=25 ms; all selected one iteration/sample.
The complete gate finished within its enforced 120-second deadline, excluding
builds, EXPLAIN and separate correctness checks. No retries or OS-cache clearing.
Times below are hot medians in ms, in A/B/B/A order:

| Curves | Range | A1 | B1 | B2 | A2 |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | prefix | 61.24 | 63.62 | 66.88 | 61.09 |
| 1 | middle | 71.05 | 71.31 | 73.59 | 69.56 |
| 1 | outside | 63.95 | 68.14 | 71.66 | 62.69 |
| 16 | prefix | 430.35 | 480.63 | 500.31 | 436.19 |
| 16 | middle | 503.65 | 604.00 | 624.81 | 522.89 |
| 16 | outside | 412.76 | 556.30 | 555.68 | 425.34 |

The gate verdict is **inconclusive**, not a formal regression classification:
several selected metrics exceeded 2% relative MAD. All medians were slower for B;
16-curve middle/outside each stayed below 2% MAD in all four captures. There is no
measured reason to retain the extra scans. Separate candidate `query_panel` checks
passed all eight ranges × one/sixteen curves (four requests/case), including exact
source counts, completeness, budget and chart-point counts. Unit coverage now also
checks prefix/suffix and `i64::MIN`/`i64::MAX` singleton viewports with duplicate and
non-finite observations; selected neighbors must remain whole real observations.

Candidate EXPLAIN confirms the limitation: outside's left scan still emits
1,000,001 Steps from 62 files; prefix's right scan still reads 62 files despite
emitting fewer rows. Middle's bounds read 28 + 29 files. Predicate pushdown and
Top-N do not guarantee early file termination. Effective rows/counts were correct;
this is not evidence of an LTTB bottleneck. Before another production candidate,
verify whether a DuckLake-owned aggregate with a static Step predicate can avoid
fact scans in the actual plan; do not assume `min`/`max` or `LIMIT 1` can do so.

Ignored evidence: `.tmp/v5-bounds-comparison/` (logs, runner, manifest, result and
copied executables), `.tmp/v5-storage-topn-bounds/` (JSON plans and SQL).
SHA-256 identities:
- manifest: `f7f2a86a5bd882fd504e37061e7b05b93fb4b933fdd15ec029d6867ee25a35a4`;
- runner: `29ed38930f45964a0c80bd96632a8f17c4d61083a2bc474bfaa33cad32baadc7`;
- result: `d8c0336a9f801bd7ceccaa08d3e0fde3d6c16f9207e2602cd8b424f687b88c30`;
- baseline executable: `244608756bc69036dc9e0c704a727ca1ac1a3958b18123fd1e1f54b2c40a3b4d`;
- candidate executable: `b3fe457890145f880cae77ea7d6baee5125a3734a89a3644053c0b8c79a881cf`.

### Static aggregate plan validation: no metadata-only shortcut
`detail_static_bounds_operator_profile` captures 76 analyzed plans: all eight
ranges plus outside-left and an unfiltered control, both sides, literal/bound Step
predicates, and paired min/max versus the single nearest aggregate. All boundary
assertions passed. Reproduce with the existing fixture (output must not exist):
```sh
SEEX_PANEL_ROOT="$PWD/.tmp/v5-panel-million/store" \
SEEX_STORAGE_PROFILE_OUT="$PWD/.tmp/v5-static-bounds-plans" \
cargo test -p seex --release --lib storage::alignment_query::tests::detail_static_bounds_operator_profile \
  -- --ignored --exact --nocapture --test-threads=1
```
Nonempty predicates still have `DUCKLAKE_SCAN` feeding the aggregate. Literal/bound
and single/paired forms match: prefix-right **997,952 / 62**, middle-left
**450,000 / 28**, middle-right **449,999 / 29**, tail-left **999,001 / 62**, and
outside-right's left **1,000,001 / 62** (rows / files). The unfiltered control also
scans 1,000,001 Steps / 62 files. Impossible sides become `EMPTY_RESULT`, but do
not remove work on the other side.
This DuckLake version/fixture provides no metadata-only nearest-bound shortcut;
do not proceed with that production rewrite. This is plan evidence, not ABBA or
physical-I/O measurement; no hot-path SQL changed. Consider request batching before
another equivalent per-curve bounds rewrite. Raw SQL/JSON and `SHA256SUMS` live in
ignored `.tmp/v5-static-bounds-plans/`; SHA-256:
`c98d564fffbd953f5844e147f2d435c5909d22224a163f6a5f0ecf77674dde03`.

### Request-batched bounds investigation: fewer nodes, unchanged fact work
The ignored `detail_batched_bounds_operator_profile` compares the incumbent bounds
CTE (extracted verbatim, with the same bindings) against a requested-Run VALUES
relation joined to DuckLake, then grouped by request ordinal and RunId. Scope is
**bounds discovery only**, not effective observation fetch, native LTTB or a panel.
Both plans use bound identities and viewport limits, on the existing read-only
connection. No production query, public API or connection lifetime changed.

```sh
SEEX_PANEL_ROOT="$PWD/.tmp/v5-panel-million/store" \
SEEX_STORAGE_PROFILE_OUT="$PWD/.tmp/v5-batched-bounds-plans-complete" \
cargo test -p seex --release --lib storage::alignment_query::tests::detail_batched_bounds_operator_profile \
  -- --ignored --exact --nocapture --test-threads=1
```

All 72 analyzed plans completed: prefix, middle, outside and a missing-Run control;
each has sixteen single queries and one/16-Run batch queries. Boundary values,
request order/identity and NULL boundaries for the missing Run were asserted.
For each of prefix/middle/outside:

| Shape | DuckLake scan nodes | Scan output Steps | Files read |
| --- | ---: | ---: | ---: |
| Sixteen incumbent queries, summed | 16 | 16,000,016 | 992 |
| One sixteen-Run grouped query | 1 | 16,000,016 | 992 |
| One single-Run grouped query | 1 | 1,000,001 | 62 |

The missing-Run control selects fifteen real Runs plus one absent identity:
both shapes scan 15,000,015 Steps / 930 files and preserve the absent request.
Batching does not reduce fact work here; it adds HASH_JOIN and HASH_GROUP_BY.
Fewer statements may save prepare/dispatch cost, but this profile does not measure
that tradeoff. No ABBA, end-to-end latency, physical-I/O or RSS claim follows.
Raw SQL/JSON and `SHA256SUMS` are in the output directory above; manifest SHA-256:
`138f436b023abfef23c3b91bfb483ebdf9c2c317435a616d79e7c19425c3f8bd`.

Do not yet wire a complete grouped query into Reader. Its current loop qualifies
each Run, chooses narrow/bounded-full/fallback, and shares only one fresh request
connection. The existing Arrow selector accepts exactly one curve and one set of
snapshot counts; concatenating Run streams would corrupt counts and LTTB budgets.
Any full prototype needs explicit request ordinal, per-Run snapshot counts/empty
rows, real nearest observation selection (ties unspecified), and separate reducers
with unchanged per-curve budgets. Preserve duplicate observations, cancellation,
whole-attempt retry and no partial publication. A single batch statement would
share one statement snapshot; do not promise a panel-wide transaction snapshot.
Next, measure a bounded storage-only control of sixteen incumbent bounds calls
versus this grouped query, including prepare, before paying for a grouped fetch/
reducer integration. Mixed Run ranges, non-finite observations, full/Overview,
actual points/budget parity and cancellation remain unvalidated by this probe.

### Bounds-only A-B-B-A: batch candidate improves this boundary
One bounded comparison used the ignored release `detail_bounds_gate`, with the
same executable selecting baseline/candidate via `SEEX_BOUNDS_ROLE`. Baseline
executes sixteen incumbent bounds CTEs; candidate executes one sixteen-Run grouped
query using the same builder as the plan probe. SQL text and parameter allocation,
connection open/attach and EXPLAIN are excluded. Every measured operation includes
prepare, binding, execution, complete result consumption and boundary assertions;
B additionally validates request ordinals/RunIds. The connection is reused within
each capture, not across captures. No cross-request production cache was added.

```sh
python scripts/performance_gate.py run \
  --manifest .tmp/v5-bounds-only-abba/manifest.json \
  --output .tmp/v5-bounds-only-abba/result.json
```

The complete workload took **24.73 s**. All twelve metric captures have ten
calibrated samples >=25 ms, one iteration/sample; one first operation and a
calibration pilot per case are excluded. OS caches were not cleared. No retries.
Hot medians (ms), in A/B/B/A order:

| Range | A1 | B1 | B2 | A2 | Combined improvement |
| --- | ---: | ---: | ---: | ---: | ---: |
| prefix | 222.24 | 103.47 | 104.95 | 225.34 | 53.43% |
| middle | 221.99 | 103.69 | 106.85 | 224.19 | 52.81% |
| outside | 222.03 | 104.24 | 105.53 | 223.66 | 52.93% |

Gate **pass**; relative MAD 0.45–1.38%, protected middle/outside did not regress.
This measures the combined storage boundary, not prepare alone or a complete
Detail request. Fact scan cardinality/files remain unchanged; no physical-I/O,
LTTB, chart, RSS or full/Overview performance conclusion follows. The result
supports a separately scoped full grouped-fetch/reducer prototype, not production
integration yet. Ordinary tests also isolate different Run bounds and an empty
request, independently of the homogeneous million-point fixture.
Ignored raw evidence in the manifest directory includes the source snapshot,
copied runner, build records, gate log, wall time, manifest and result. SHA-256:
- manifest: `4662acc550e16d010f74346fe9af79815b689179e5784e8215eeb741b1747add`;
- result: `af40e7a2082d431671cec7205209003a5e6c29e3ea6978e4818705f9e88b935b`;
- runner: `8010788d60c78771be71ad6c243488ea4b35c8b384399c874e726a315f8f0463`;
- captured source: `d61e44d058ad74d259f0b388918e88ced2f153f2469cb698219ca712f2334ff7`.

### Neighbor ablation: substantial observed cost, noisy comparison
`detail_neighbor_ablation` compares incumbent narrow SQL against test-only viewport
filtering, with snapshot/finite counts and native LTTB in both; curves stay sequential.
Prepare/execute/consume are included; open/attach, metadata and chart conversion are not.
One `python scripts/performance_gate.py run --manifest .tmp/v5-neighbor-ablation/manifest.json --output .tmp/v5-neighbor-ablation/result.json`
finished in **50.07 s**, ten calibrated samples >=25 ms per metric/capture, no
retries. **Inconclusive**: every metric exceeded 2% relative MAD in at least one
capture (0.35–4.40% overall). Medians in ms, A/B/B/A:

| Curves | Range | A1 | B1 | B2 | A2 |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | prefix | 21.63 | 9.06 | 9.07 | 21.96 |
| 1 | middle | 26.96 | 13.79 | 13.61 | 27.99 |
| 1 | outside | 22.42 | 2.16 | 2.19 | 21.59 |
| 16 | prefix | 345.38 | 149.16 | 139.72 | 354.35 |
| 16 | middle | 436.64 | 227.64 | 230.59 | 447.80 |
| 16 | outside | 338.53 | 34.49 | 33.98 | 361.32 |

Separate traced one-Run diagnostics (three hot observations, not ABBA) show:
prefix prepare/execute/consume **1.59/21.04/0.065 → 1.20/8.28/0.064 ms**;
middle **1.77/25.27/1.68 → 1.33/11.37/1.61 ms**;
outside **1.41/20.21/0.018 → 1.04/1.28/0.013 ms**.
The candidate removes the million-Step/62-file bounds scan. Prefix reads 2,049
observations/one file, middle 100,002/eight files, outside becomes EMPTY_RESULT.
SQL stats and neighbor Top-N are small in operator evidence; parallel operator
times are not additive wall-clock costs. Full-range requests are not covered.
Observed savings support considering desktop Detail without neighbors, not a
formal performance pass or deleting other storage/API contracts. Budget remains
1,024; outside changes from one real point to empty. No production change yet.
Raw logs/source/runner/result: ignored `.tmp/v5-neighbor-ablation/`; result SHA-256
`6cb62add2b31e54308c37195abff13873a3d538a5d9128839a392511ae8d98a7`.
Plans: `.tmp/v5-neighbor-profile-{baseline,candidate}/`, each with `SHA256SUMS`.

### Desktop Detail drops out-of-view neighbors
The ablation above led to a deliberate product change, approved as a UX tradeoff:
Desktop **Detail** now queries only observations inside its closed viewport.
Out-of-view neighbors previously forced a whole-Run bounds scan for every curve,
even when the viewport held 2,048 of 1,000,001 Steps or none at all.

Implementation: the panel path and the single-curve `query_metric_for_desktop`
adapter no longer request neighbors at all. Detail reuses the existing
viewport-filtered storage SQL (`step >= ? AND step < ?`, real snapshot counts,
native LTTB, per-curve budget) instead of inventing another SQL shape. Stable
`query_metric`, duplicate observations, budgets, retry, cancellation and
no-partial-publication rules are untouched. No connection cache, rollup,
dependency, partition or public Python API change. The follow-up section below
records how Overview and the remaining storage shapes were resolved.

One bounded A-B-B-A measured the **complete Detail panel request**: sixteen
curves, fresh request connection, metadata/diagnostics, per-Run SQL, Arrow
consumption, native LTTB and chart-series construction.

```sh
python scripts/performance_gate.py run \
  --manifest .tmp/v5-detail-no-neighbor/manifest.json \
  --output .tmp/v5-detail-no-neighbor/result.json
```

A is commit `076913b` (neighbor-retaining Detail); B is this change. Each
build's binary was verified by its own expected source counts before timing.
Three invalid attempts are not used: two never completed all cases (the first
because a shared Cargo target directory silently reused the candidate test
artifact for the baseline; the second because the runner requested an unsupported
range name), and one completed capture set whose manifest keys did not match the
emitted metric names (kept as `result-invalid-metric-keys.json`). The valid gate
finished in **52.90 s**; every metric has ten calibrated samples >=25 ms in all
four captures; no reruns of the reported workload. Hot medians (ms), A/B/B/A:

| 16 curves | A1 | B1 | B2 | A2 | Combined improvement |
| --- | ---: | ---: | ---: | ---: | ---: |
| prefix (0..2,048) | 435.06 | 205.66 | 208.17 | 429.03 | 52.11% |
| middle | 527.07 | 293.85 | 292.40 | 526.65 | 44.36% |
| outside (empty) | 421.03 | 87.11 | 86.82 | 426.35 | 79.48% |

Gate **pass**; relative MAD 0.32–1.87%, no protected regression. Empty viewports
now return zero observations instead of one neighbor, so they render no curve
instead of a single edge sample. Sparse curves may start/end at their first/last
in-view observation, and a segment crossing a viewport with no observation inside
no longer draws. That is the accepted cost; it needs real desktop confirmation,
which this session did not run. Counts changed accordingly: prefix 2,050 -> 2,049,
middle 100,004 -> 100,002, outside 1 -> 0 per curve; full detail is unchanged.

Correctness: `panel_contract_and_values`, `native_read_pipeline` (exact counts,
completeness, budget, first/last chart points), reader parity for repeated Step
observations without neighbors, and the affected Reader unit test all pass. The
narrow-Step/bounds SQL no longer sits on any path and was removed afterwards (see
the next section); the previously reported request-batching gain does not apply.
Ignored raw evidence in `.tmp/v5-detail-no-neighbor/`; SHA-256:
- manifest: `0d20a2282b98e4a8b86e77ca13202a4f732f237352533f10c2355bfaba5a0654`;
- result: `f637c6067a5e1b6cabc837e56afeb0b88a82ce72a45f344fdf97c69406485f49`;
- baseline executable: `244608756bc69036dc9e0c704a727ca1ac1a3958b18123fd1e1f54b2c40a3b4d`;
- candidate executable: `03fb5dad6d1c5cb827a7aaf60f5e944f01327e1778ef3183b735d0b87aa60968`.

### Unused storage surfaces removed
Overview no longer has a separate aligned/neighbor path. An intermediate attempt
kept two shapes (viewport with statistics, whole series without) and registered
Overview as `StepsFrom{0}` in the second. Using the statistics shape for Overview
was measured and **rejected**: sixteen curves regressed **13.2%** (687/686 ->
781/777 ms). That shape added an aggregate, a per-row `LEFT JOIN`, and extra
`UBIGINT` columns; the run did not isolate their individual shares, so "column
width" was never proven to be the sole cause. The regression file is kept as
`.tmp/v5-overview-unified/result-8col-regression.json`. The final resolution is
below: Detail and Overview share the same source, so they now also share one SQL.

Removed with this change: the neighbor/aligned SQL and its `AlignmentQuery`,
`AlignmentQueryResult`, `AlignmentReduction`, `AlignmentReason`,
`AlignedMetricPoint`, `AlignedMetricResult` model types; the narrow-bounds plan;
`query_aligned_metric` on `ProjectMetricReader`, `ParquetMetricReader`,
`StandaloneMetricReader` and the `MetricReader` trait; the Parquet timestamp and
ElapsedTime adapters; and the test-only Engine aligned entry point. Kept:
`AlignmentAxis`, `AlignmentViewport`, `AlignmentQueryError` (desktop viewport and
axis state) and summaries. Series diagnostics were removed separately below.

### One filtered sampling SQL for Detail and Overview
Both panels read the same fact relation, so neither needs per-row snapshot
statistics. The sampled query is now a single shape:

```sql
WITH effective AS (
    SELECT run_id, metric_key, step, timestamp, value_f64, ingested_at
    FROM dl.metric_points
    WHERE run_id = ? AND metric_key = ? AND metric_key_encoded = ?
      AND (? IS NULL OR step >= ?)
      AND (? IS NULL OR step < ?)
)
SELECT step, epoch_ms(timestamp), value_f64, epoch_ms(ingested_at), step
FROM effective WHERE isfinite(value_f64) ORDER BY step
```

`WHERE isfinite` runs at the SQL boundary, so the materialized row count is
exactly the reducible point count and `Statement::row_count()` sizes the streaming
LTTB. The `source_stats` CTE, `LEFT JOIN`, count columns, empty-result metadata
row, seven-column protocol, and the Overview-only finite gate are all gone.
Detail and Overview differ only in bounds: `Steps{start, end+1}` versus
`StepsFrom{0}`. Non-sampled `Full` queries keep returning raw observations.

Semantics change: `source_count` now counts finite observations in range rather
than all observations including NaN/Inf. The desktop `source_row_count` feeds
`downsampled()` and test-support resource snapshots, not displayed text; evidence
completeness follows Run lifecycle only (see "Series diagnostics removed" below).
This matches ClickHouse's own `lttb`, which drops
NaN at the aggregate's `add()` and sizes buckets from the buffered finite count
(it buffers O(N) instead of streaming, which is why it needs no pre-count).

One bounded A-B-B-A compared the previous two-shape state with this one on
the sixteen-curve panel: Detail prefix/middle/outside plus a whole-series
Overview. It finished in **70.07 s** with ten calibrated samples >=25 ms per
metric/capture. Medians in ms, A/B/B/A:

| Query | A1 | B1 | B2 | A2 |
| --- | ---: | ---: | ---: | ---: |
| detail prefix | 209.17 | 230.27 | 206.68 | 210.10 |
| detail middle | 284.99 | 288.25 | 269.61 | 288.12 |
| detail outside | 88.67 | 101.65 | 83.59 | 87.77 |
| overview full | 694.08 | 693.99 | 683.26 | 684.93 |

Gate verdict **inconclusive**: three Detail captures exceeded 2% relative MAD
(up to 3.83%), so no classification is claimed. There is no repeatable direction
and no regression signal; Overview is flat. This is a simplification, not an
optimization. Raw evidence `.tmp/v5-one-shape/artifacts/`; SHA-256:
- manifest: `f2cb2c79848c14c4310f64659d69c4803d940c238e5404541d92d63452415d7a`;
- result: `8a09d95ad12550af0369b18ef059f6f18ba27988f91fc365493ce121201b0d8d`;
- two-shape baseline: `2a2edda6df54897fb75f5c17c2d1729bfcfc4f67405402177cd2368027e80065`;
- one-shape candidate: `1810e6e613a9f442c20ec5543f3091af724809e135c52616192a4e42263eb1c9`.

The earlier eight-to-seven-column experiment is superseded by this change; its
raw evidence remains in `.tmp/v5-cols-baseline/artifacts/`.

### Production plan evidence
`storage::metric_query::tests::production_query_plans` captures the plans of the
SQL that production actually builds: the shared sampled shape (four ranges) and
the summaries query. It validates result counts first, then runs
`EXPLAIN (ANALYZE, FORMAT JSON)` with the same bindings.
Reproduce against the prepared fixture (output must not exist):

```sh
SEEX_PANEL_ROOT="$PWD/.tmp/v5-panel-million/store" \
SEEX_STORAGE_PROFILE_OUT="$PWD/.tmp/v5-production-plans-final" \
cargo test -p seex --release --lib storage::metric_query::tests::production_query_plans \
  -- --ignored --exact --nocapture --test-threads=1
```

| Query | Files read | Rows out of `DUCKLAKE_SCAN` |
| --- | ---: | ---: |
| Detail prefix (0..2,048 closed) | 1 | 2,049 |
| Detail middle (450,000..550,000) | 8 | 100,002 |
| Detail outside (2M..3M) | 0 | 0 (operator eliminated) |
| Overview (`StepsFrom{0}`) | 62 | 1,000,001 |
| Summaries, sixteen finished Runs | 0 | 0 |

This closes the earlier gap: the `(? IS NULL OR step >= ?)` form prunes files,
and the non-finite filter does not force a full scan. Raw SQL/JSON and
`SHA256SUMS` live in the ignored output directory; SHA-256:
`e329af6d90fc7b01033b88d1d7886c69062cd1d8ffd46831829a7ddf19967542`.

### Summaries: catalog guard skips the fact fallback
The summaries query used to include the running-Run branch unconditionally.
With sixteen finished Runs it still read **11 Parquet files / 24,576 rows** for a
branch that could not match. Restructuring with a `MATERIALIZED` running set did
not help (planner still scanned the probe side), so the branch is now chosen from
the catalog: one `count(*)` over the requested Run ids decides which SQL is built.
The finished-only statement reads **0 files**, verified by the plan test above.
The guard also fixed a real hazard: a test fixture's in-memory `Run` said
`Finished` while the catalog row still said `running`; caller-supplied statuses
were therefore not trusted, and the catalog is the single source of truth.
This is plan-level evidence, not an ABBA; the added lookup is a small catalog
count, and summaries remain an inspector/discovery operation outside the hot
Detail path.

### Series diagnostics removed
Before removal, every curve performed one whole-Run `lag` window per Reader
generation to derive validity evidence (negative Step, decreasing timestamp,
non-finite value, missing metric). Batching that window into one partitioned
query was measured first and **rejected**: sixteen single queries took
**365.20 / 363.69 ms** versus **469.13 / 450.36 ms** for the batch (storage-only
A-B-B-A, 21.33 s, eight of ten samples calibrated per capture; gate
`inconclusive` because one capture's MAD was 3.9%). The combined partition sort
lost the parallelism of independent Run-scoped queries.

The feature was then removed instead of optimized. `SeriesDiagnostics`,
`query_series_diagnostics`, the Reader diagnostics cache, and the generation
refresh call are gone. Completeness now follows Run lifecycle only:
`Finished -> Complete`, `Running -> Partial + RunRunning`,
`Failed -> Partial + RunFailed`. The curve path no longer emits `Invalid`,
`Unavailable`, `MissingMetric`, `NegativeAxis`, `DecreasingAxis`,
`NonFiniteValue`, or `DiagnosticsUnavailable`. Non-finite values are still
filtered in SQL, so they never reach LTTB, and a Run without the metric now
reports `Complete` with zero points rather than `Unavailable`. This is a
product semantic change, not just a cleanup; callers that need data-quality
flags would have to maintain them at write/flush time in a separate proposal.
Raw rejected-batch evidence `.tmp/v5-diagnostics-batch/`; SHA-256:
- manifest: `2e4edb3a112b3e916bc3bfec40349b96c6a95ecc255e22c91d718406afef9d80`;
- result: `010c826a46ca256d2dae7d8dafb3a3099e1f2099087feccb8077b34ed172b3e2`;
- runner: `eb892822dd58495078a6876f623db36b86a64a89c9eae44e32d859d05b8dd9bd`.

Evidence retained from the superseded two-shape intermediate:
`.tmp/v5-overview-unified/`; SHA-256:
- manifest: `0f69a55e8dcd3eec16b23c3bb8afc868377f9fd8ba105d04fb4ac1d4d155366d`;
- result: `21b734809f62bca0519e5e9e9db7f1b0c23b3a4ce114f5f98512b9e835f7d932`;
- rejected 8-column result: `24410485336a85d08a63dee5d98aef2c2bbc9608c5cede319625864372129476`;
- candidate executable: `3f0afcc78e4ea3e9558e5aa0b873da2d99c667929def25ba65a8cdc1a99f0395`.

### Per-statement fixed cost versus row volume
`storage::metric_query::tests::sampling_stage_profile` measures the production sampled
statement on one reused read-only connection: `prepare`, then `execute` (including
`row_count()` materialization), then `consume` (Arrow batch conversion, payload extraction,
native LTTB). It runs ten repetitions per Run and sums the per-Run medians for sixteen
curves; the desktop panel also reuses one connection and processes curves sequentially.
This is a storage-only diagnostic, not A-B-B-A: it excludes the fresh request open,
metadata, point conversion and chart construction, and it does not measure RSS.

```sh
SEEX_PANEL_ROOT="$PWD/.tmp/v5-panel-million/store" \
SEEX_STORAGE_PROFILE_OUT="$PWD/.tmp/v5-sampling-stages-fine" \
cargo test -p seex --release --lib storage::metric_query::tests::sampling_stage_profile \
  -- --ignored --exact --nocapture --test-threads=1
```

Medians for sixteen curves in ms. `execute` includes `row_count()` materialization;
`fetch` is the `statement.step()` time; `Arrow` is `RecordBatch::from`; `LTTB push` is
the standalone reducer over a synthetic equal-length stream; `extraction` is
`rows_ns - LTTB push` and is therefore approximate.

| Range | Rows/curve | prepare | execute | fetch | Arrow | LTTB push | extraction |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| prefix (0..2,048) | 2,049 | 17.00 | 123.68 | 0.27 | 0.003 | 0.39 | 0.17 |
| middle | 100,002 | 19.26 | 154.74 | 6.21 | 0.03 | 16.91 | 0.09 |
| outside (empty) | 0 | 15.04 | 16.27 | 0.00 | 0.00 | 0.00 | 0.00 |
| overview | 1,000,001 | 25.63 | 314.57 | 76.67 | 0.28 | 169.69 | 8.66 |

`prepare` is 1.0–1.7 ms per statement, or 15–26 ms for a sixteen-curve panel. The empty
viewport gives the pure statement fixed cost: about 2.2 ms per curve, 32 ms per panel.
Narrow and middle requests are execute-dominated, and their consume half is almost
entirely native LTTB. Overview splits roughly 55/45 between execute (315 ms) and consume
(260 ms); inside consume, LTTB push is about 170 ms (65%), fetch 77 ms (30%), extraction
9 ms (3%), and Arrow conversion 0.3 ms (0.1%). LTTB costs about 10.6 ns per point.

**Decisions:**

- Request-level batching is not the next optimization. It could remove at most the
  per-statement prepare and fixed execution overhead (~32–44 ms per sixteen-curve panel),
  while leaving row scan, materialization and Arrow/LTTB work unchanged. The earlier 53%
  bounds-only batching win measured a different SQL shape and does not transfer here.
- No LTTB micro-optimization is retained. A 20% LTTB gain would save only ~34 ms (~5%) of
  the ~685 ms sixteen-curve Overview request, and the first local-reuse attempt did not
  show a repeatable win (standalone medians moved within machine drift), so it was
  reverted.
- The material remaining costs are `execute` (315 ms: scan, sort and materialization) and
  the fixed per-statement prepare (25.6 ms). The next material lever is reducing executed
  rows through Rollup or layout work (P5/P3); request-scoped prepared-statement reuse is a
  separate, much smaller candidate (~15–26 ms per panel, more visible on narrow/empty
  ranges) and must not become a cross-request connection cache.

Raw evidence: `.tmp/v5-sampling-stages-fine/sampling-stages.csv` (SHA-256
`68c2af7602b1e356b9cb6920fca5adf2dd4c2e2fce9207b5a99c9e430924dfea`),
`lttb-stages.csv` (SHA-256
`a344e2507b20d3d0a158831f55cb6af76c81261e9d6e135477d6cbc33da5bfb7`) and the
log (SHA-256
`a5066638caaf03dfb2768ee6cc6472b3426e89971e36eeb3e0e26c858228e91d`). The
coarse `.tmp/v5-sampling-stages/` run and the preliminary
`.tmp/v5-sampling-stage-profile/` run are superseded.

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
2. **The one sampling SQL is measured; batching is not next.** Detail and Overview share a
   filtered viewport query and its production plans are captured. The per-statement profile
   shows 1.0–1.7 ms prepare and ~2 ms fixed execution per curve (32–44 ms per sixteen-curve
   panel), while row scan/materialization dominates narrow and middle ranges. Request-level
   batching therefore targets a small share and is deferred. Do not reintroduce neighbors or
   duplicate-observation loss for speed, and do not bypass DuckLake's file/snapshot
   ownership.
3. **Data-quality flags are out of the query path.** Series diagnostics were removed;
   completeness reflects Run lifecycle only. Summaries keep the catalog guard that skips the
   fact fallback. If validity evidence (negative Step, decreasing timestamp, non-finite value,
   missing metric) is needed again, maintain it at write/flush time in the catalog, with its
   own proposal, migration story, and measurement.
4. **Overview is measured; do not micro-optimize the reducer.** The sixteen-curve profile
   splits storage time into roughly 315 ms execute and 260 ms consume, of which native LTTB
   is 170 ms and Arrow conversion is negligible. No LTTB change is retained: its ceiling is
   about 5% of the request. Reduce executed rows through Rollup/layout work (P5/P3), or
   recover per-statement prepare with request-scoped statement reuse; keep native LTTB as
   the default and do not reintroduce Arrow+screen as an alternate policy.
5. **Defer other discovery surfaces.** Give each its own workload and baseline later; do not
   mix them into curve-query comparisons. Reassess the selected-Run limit separately only if a
   concrete comparison UX requires more than sixteen Runs.

Each production change needs focused acceptance coverage and one affected-boundary, bounded
A-B-B-A comparison (ten calibrated samples >=25 ms, total <=120 s). Cover all eight Detail
ranges in correctness tests; include representative narrow/full ranges and one/sixteen curves
in performance coverage. If necessary, split independent boundaries into explicitly separate
workloads rather than silently exceeding the deadline. Report noise, regressions, and RSS
tradeoffs; only acceptance tests block a commit. No connection cache, rollup, public API,
new dependency, or storage-layout change is implied by this roadmap.
