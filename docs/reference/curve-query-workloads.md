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
