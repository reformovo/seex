# Native v5 initial performance observations

Status: partial baseline evidence, not a performance improvement or P7 acceptance.
Source: `fd3728d28484b78ea3acf61a07186f04f56ac32f` (append-only semantics and
corrected P0 measurement units). Captured on 2026-10-13. Do not compare these
numbers with historical LWW workloads as semantics-preserving improvements.

## Scope and identity

Two local commits separate the changes:

- `bdd0b69`: Native v5 append-only observations, old-store rejection and acceptance.
- `fd3728d`: P0 measurement correction; no production query optimization.

Both comparisons use the same executable for A and B. They assess repeatability,
not a candidate improvement. During capture, the only tracked change was the
`.gitignore` rule for local v5 artifacts; production source was unchanged.
Builds and fixture preparation occurred outside the measurement. Query and
reporting captures ran sequentially. No noise thresholds were changed, and no
workload was rerun to obtain a better verdict.

Raw captures, manifests, logs, binaries and stores are local artifacts, not Git
content. Keep them under `.tmp/v5-*`. The source commit and executable hashes
identify the measured code even though this evidence document is added later.

## Query: complete Reader request, short series

Prepared with:

```sh
uv run python scripts/p0_workload.py --profile correctness \
  --root .tmp/v5-query-fd3728d-store \
  --output .tmp/v5-query-fd3728d-fixture.json
```

Seed `0x5EEC_0001`, 4 Runs, 3 keys and 12 actual sequences. The selected
`run-000000` / `metric-000000` series contains 15 observations, including repeated
Steps. The adapter asserts `source_count == points == 15` on every capture.
Only this short series is timed; this is not the million-point or 16-curve test.

The release `p0_query` executable measures `Reader.query_metric`, including its
fresh connection path, with Step screen reduction and a 1024-observation cap.
Every child makes one untimed first request, then ten calibrated hot samples.
OS page cache is not cleared. No physical plan or per-operator profile is captured.

Executable SHA-256:
`ffae7d6991aa2e1c0c115e5758e06515c4be1ab9e6dce17bea0a4fa078d66b84`.
Fixture metadata reports Darwin 27.0.0 arm64, Python 3.13.2, Rust/Cargo 1.97.1.
The workload writer uses the installed development SDK; only the Reader is release.
This mixed-build fixture preparation is not a reporting measurement.

A local adapter executes:

```sh
target/release/examples/p0_query .tmp/v5-query-fd3728d-store \
  run-000000 metric-000000
```

It validates ten samples and converts the JSON `hot_samples[].ns_per_op` into one
schema-v3 `SEEX_BENCH` record: domain `query`, metric `reader.short.hot`, unit
`ns/op`, direction `lower`, and the captured iteration count. All four captures
used one iteration per sample; each sample exceeded 25 ms. A future adapter must
preserve iteration counts rather than treating batch time as operation time.

The schema-v3 preservation manifest uses identical adapter commands for A/B,
fixture identity SHA + seed + selected 15 observations, and protected key
`query.reader.short.hot`. No hard floors or optimization primary are selected.

| Capture | A1 | B1 | B2 | A2 |
| --- | ---: | ---: | ---: | ---: |
| Median ms/op | 45.816 | 45.349 | 46.432 | 46.135 |
| Within-capture relative MAD | 0.548% | 1.673% | 1.953% | 0.605% |

Executed once using the required command:

```sh
python scripts/performance_gate.py run \
  --manifest .tmp/v5-query-fd3728d-manifest.json \
  --output .tmp/v5-query-fd3728d-result.json
```

**Original execution: INCONCLUSIVE (missing selected metric).** The manifest
incorrectly protected `reader.short.hot`, omitting the parser's `query.` prefix.
The original manifest and result are retained. A corrected manifest and offline
result reuse the exact same four captures via `read_manifest` and `compare`;
there was no second workload execution. **Offline classification: PASS** under
unchanged thresholds. This is not a formally successful gate invocation or an
improvement claim. Neither exact gate wall time nor user-request p95 was recorded;
the gate enforces the complete 120-second deadline. Ten batch samples do not
establish a robust request p95.

Artifacts: `.tmp/v5-query-fd3728d-{fixture.json,adapter.py,manifest.json,result.json,
corrected-manifest.json,offline-result.json}` plus the prepared store.

## Reporting: drain and finalization diagnostic

Release build outside measurement:

```sh
cargo test -p seex --release --test reporting_performance --no-run --message-format=json
```

Extract the test executable from Cargo's artifact output. Both manifest commands
execute that binary with `reporting_durability_phases --ignored --exact
--nocapture --test-threads=1`. The fixture uses 1000 unique-Step reports per Run,
30 fresh Runs per capture and queue capacity 1,048,576. This queue is a diagnostic
configuration, not the product default. It does not measure admission latency,
steady-state overload, four concurrent writers or repeated-Step query cost.

Protected keys: `reporting.rust.drain_persistence` and
`reporting.rust.finalization`, both lower-is-better `ns` records. Each of the ten
samples sums three Runs; the table is **milliseconds per three-Run batch**, not
single-Run latency or points/s. Finalization includes the existing summary/flush
path; phases within it were not isolated.

| Capture | A1 | B1 | B2 | A2 |
| --- | ---: | ---: | ---: | ---: |
| Drain median ms/batch | 261.889 | 267.753 | 269.706 | 273.602 |
| Finish median ms/batch | 689.279 | 698.006 | 696.489 | 718.909 |

Executed once:

```sh
python scripts/performance_gate.py run \
  --manifest .tmp/v5-reporting-fd3728d-manifest.json \
  --output .tmp/v5-reporting-fd3728d-result.json
```

**INCONCLUSIVE: both protected metrics exceeded the noise limit.** Within-capture
relative MAD was 0.589–1.530%, but the two A medians drifted: their relative MAD
was approximately 2.187% for drain and 2.104% for finish. No preservation or
regression classification is claimed. All records met the 25 ms floor; minimum
batch samples were 257.144 ms (drain) and 672.917 ms (finish).

Important measurement-contract limitation: this existing durability probe uses
fixed three-Run batching, not internally adaptive calibration. The gate's
`calibrated` flag only proves the time floor. Treat this run as diagnostic data,
not a fully qualified reporting baseline; correct the calibration mechanism in
a separate measurement-only slice before future formal comparisons.

Rust test-reported times sum to 53.74 seconds; this excludes process-launch/gate
overhead, and exact wall time was not separately recorded. The gate enforces
120 seconds. Environment artifact reports macOS 27.0.1 arm64, Python 3.14.7,
Rust/Cargo 1.97.1; preserve this metadata rather than conflating it with the
query fixture's Python/environment labels.

Executable SHA-256:
`b43381d2cf837091a5d7db597a5076a9ce3b0f90eb302b3abaaf8ba4dd7ad4d4`.
Artifacts: `.tmp/v5-reporting-fd3728d-{build.log,executable.txt,manifest.json,
gate.log,result.json,environment.json,summary.json}`.

## Adaptive reporting follow-up

A measurement-only change replaces fixed three-Run batches with pilot calibration:
try 1, 2, 4, ... up to 32 Runs until both drain and finish reach 25 ms. Exclude
pilot Runs from the ten recorded samples. All samples then use that fixed calibrated
count; a sample below 25 ms fails rather than being silently accepted or retried.
A 25-second per-capture budget is checked between Runs and before publishing
records; it is cooperative, not an interrupt for an in-flight storage call.
The gate still enforces the complete 120-second deadline. Records are `ns/op`
per Run, with `batch_iterations` so the gate can check total elapsed time.
Queue capacity and production persistence behavior remain unchanged.

Built in release outside measurement, then executed exactly once:

```sh
python scripts/performance_gate.py run \
  --manifest .tmp/v5-reporting-adaptive-manifest.json \
  --output .tmp/v5-reporting-adaptive-result.json
```

Source identity: `eb11c4f` plus tracked patch SHA-256
`05810c5b33ec81bc6b687b7b88fde5a9de8104e2674988ada5179dd8959a51b5`.
Release binary SHA-256:
`c2a63f653e123ba4d808dba4b1013fdcb6d3c56f6d6e87d2032fb3d6688c5e3e`.
Both A/B commands use the same executable. All captures calibrated to one Run
per sample; each capture used one pilot Run and ten measured Runs.

| Capture | A1 | B1 | B2 | A2 |
| --- | ---: | ---: | ---: | ---: |
| Drain median ms/Run | 87.234 | 87.005 | 86.604 | 88.278 |
| Finish median ms/Run | 224.965 | 228.379 | 225.564 | 226.132 |

**Formal gate: PASS (self-comparison repeatability only).** All eight records
met the time floor and unchanged noise rules; within-capture relative MAD ranged
from 0.624% to 1.925%. Test-reported durations sum to 19.32 seconds, not exact
whole-gate wall time. Keep the earlier fixed-batch INCONCLUSIVE as historical
diagnostic evidence. These different measurement contracts do not establish
runtime performance improvement, steady-state throughput, or production RSS.

## Flush phase diagnostic

A diagnostic-only patch on `741049f` adds flush connection-open, direct/recovery
SQL, pending-marker read/clear and lock-release timings under the existing
`SEEX_WRITE_TIMING=1` switch. Transaction, lock scope and error behavior stay the
same. Release single-writer durability and four-writer acceptance workloads ran
once each, sequentially; no performance gate or optimization comparison ran.

| Median ms | Single writer (11 flushes) | Four writers (5 flushes, including setup) |
| --- | ---: | ---: |
| Flush connection open | 28.786 | 28.606 |
| Recovery flush SQL | 25.567 | 19.277 |
| Marker clear | 1.245 | 0.727 |
| Lock release | 0.002 | 0.002 |
| Flush lock hold | 56.675 | 53.584 |
| Flush lock wait | 0.053 | 0.059 |

Marker-read counts are 22/10, because opening connections also checks the marker;
these are not one-to-one with the timed flush requests. Connection-open includes
format validation as well as attach. `inline_flush` includes pending-marker and
SQL work; `flush_lock_hold` includes connection open and inline flush. Durations
are nested, not independent terms. By summed durations, connection open accounts
for approximately 51%/55% of flush hold time, recovery SQL 45%/42%; marker clear
and release are small. Logging overhead is included; these are profiles, not
uninstrumented timings or robust tail distributions.

The first coarse four-writer diagnostic observed max flush lock wait 247.378 ms,
but the refined run observed only 0.159 ms. Thus it did **not** reproduce a stable
flush-wait bottleneck or prove which operation delayed another process. Max append
wait in the refined run was 50.512 ms. Do not claim this decrease as improvement:
there was no lock optimization, and scheduling/workload variation is uncontrolled.

Bootstrap/flush acceptance tests and core check passed; both profiled workloads
passed. Artifacts: `.tmp/v5-flush-phases-{build.log,single.log,four.log,summary.json}`.
The helper used by normal `flush_run_data` is the pending/recovery path; the direct
SQL branch was not exercised by these workloads. No cross-process timestamp trace
was captured, so causal attribution of wait intervals remains unproven.

## Writer connection phase and invalidation diagnostic

A second diagnostic-only patch on `40dfac8` split writer opening into DuckDB
connection creation, DuckLake attach, catalog adapter setup and format
validation. In the single-writer release durability workload, the 99 observed
writer opens had these medians:

| Phase | Median |
| --- | ---: |
| DuckDB connection creation | 2.223 ms |
| DuckLake attach/install/load | 23.840 ms |
| Catalog adapter setup | 0.460 ms |
| Format setup/validation | 2.194 ms |
| Existing aggregate `connection_attach` timing | 26.726 ms |
| Flush fresh-writer open (11 calls) | 28.920 ms |

The timings are nested/overlapping: `connection_attach` excludes or combines
some phases according to the open path and is not their arithmetic sum. The
stable dominant phase is DuckLake attach, not DuckDB object creation or catalog
adapter setup. Artifacts: `.tmp/v5-connection-phases-{build.log,log,summary.json}`.

An ignored release diagnostic test deliberately held one cached writer while a
second writer called `ducklake_flush_inlined_data`. The cached writer then failed
to read with `ducklake_inlined_data_1_1 does not exist` while the fresh writer
could read the catalog. The cached writer's subsequent append returned success,
but this is not evidence of safe reuse; the stale read already violates the
Reader/writer consistency contract. The test is diagnostic and ignored by the
normal suite. This reproduces the historical P1 warning about obsolete inline
table references.

Decision: do not cache writer connections, move attach outside the catalog lock,
or use append success as an invalidation signal. Any future reuse candidate must
prove stale-snapshot invalidation and crash/reopen behavior with real SQLite /
DuckLake connections before measuring speed.

## Default-queue pipeline diagnostic

A release-only ignored diagnostic measured one default-queue pipeline without
changing production code or queue capacity. It calibrated from 4,096 to at most
65,536 reports, but `run.log` admission never reached the 25 ms floor at the
queue limit, so `pipeline_calibrated=false`. Five samples were retained for
orientation only; this is not a formal performance gate.

| Metric | Samples | Median |
|---|---:|---:|
| Admission | 4.29–6.57M reports/s | **5.48M reports/s** |
| Admission through pending drain | 19.23–19.52k reports/s | **19.33k reports/s** |
| Finish after drain | approximately 0.94 s / 65,536 reports | not a throughput metric |

Each sample started with 65,536 pending reports before drain and ended at zero.
Thus the admission path can enqueue substantially faster than the background
writer persists; queue capacity is a backlog bound, not evidence of durable
throughput. The persistence rate is measured from the beginning of admission to
pending reaching zero, so it includes admission time and should not be compared
directly with the earlier drain-only per-Run measurements. Finish includes the
existing terminal summary/flush path. Artifacts:
`.tmp/v5-default-queue-pipeline.log`.

The diagnostic is useful for direction only: it used five samples, a single Run,
65,536 queued reports and no four-writer overload. It does not establish sustained
throughput, queue-full behavior under a long run, RSS, or fairness between writers.

## Four-writer 16K shared-store diagnostic

Four release child processes wrote 32,768 reports each to the same SQLite/DuckLake
store and different Runs. Each used the 16K batch limit and a 65,536 report queue.
This is a concurrency diagnostic, not an A-B-B-A comparison. Each child admitted
in about 7.6–7.8 ms; admission is not the bottleneck in this fixture.

| Observation | Range |
|---|---:|
| Admission per writer | 7.6–7.8 ms |
| Admission→persistence | 117–567 ms |
| Maximum append lock wait | 318 ms |
| Maximum summary lock wait | 278 ms |
| Maximum flush lock wait | 62.6 ms |
| Finish per writer | 1.27–3.70 s |

The large finish spread and append/summary lock waits show that single-writer
16K persistence results do not generalize to concurrent writers. Flush wait was
not the dominant stable tail in this run; catalog append and summary serialization
were more concerning. Attach remained roughly 25–51 ms per writer. Artifacts:
`.tmp/v5-four16-shared.log` and `.tmp/v5-four16-build.log`.

The diagnostic does not establish a regression against 8K under concurrency:
there is no matched four-writer 8K run yet. Next compare 8K and 16K with the same
four-child orchestration, capturing lock wait/hold, finish tail, pending backlog
and process RSS before considering any further batch change.

## 8K versus 16K batch candidate

A single candidate changed `METRIC_BATCH_MAX_REPORTS` from 8,192 to 16,384.
The ordinary 1,000-report durability test cannot exercise either limit, so the
comparison used the 65,536-report default-queue pipeline and two release test
binaries. Four captures ran in one bounded A-B-B-A sequence; total gate budget
was respected. The test used 10 samples per capture and a 60-second cooperative
per-test diagnostic budget.

| Median | 8K A1 | 16K B1 | 16K B2 | 8K A2 |
|---|---:|---:|---:|---:|
| Admission reports/s | 6,344k | 5,886k | 5,682k | 5,676k |
| Admission→drain reports/s | **19.32k** | **308.67k** | **303.27k** | **18.95k** |

The raw candidate signal is large, but the formal result is **INCONCLUSIVE**:
the protected admission metric exceeded the 2% noise limit. The corrected offline
classification reused the exact four captures and did not rerun the workload;
it remained INCONCLUSIVE. The original gate invocation had the usual metric-key
prefix mistake (`reporting.` is part of the parser key); the corrected result
retains the original captures and explicitly records that limitation.

The 15–16x persistence difference is much larger than the simple expectation of
halving the number of 8K batches. It may reflect DuckLake inline-table/attach
state and batch interaction, not just the batch count. A follow-up timing run
reported 96 `batch_append` calls for 8K and 49 for 16K, but those counts include
pilot calibration and are not aligned formal samples. Their aggregate timing is
therefore not a candidate comparison: it is diagnostic evidence that pilot work,
inline state and flush must be separated before attribution.

Do not accept 16K as a production optimization until a clean candidate measurement
uses fresh stores, identical fixed batch counts, excludes pilot runs from all
reported metrics, and repeats the signal with stable protected admission,
per-stage attach/flush timings, RSS, queue backlog and finish latency. The
A clean follow-up replaced the confounded pipeline with a fresh-store bounded
workload: every sample created a new store, wrote exactly 32,768 reports, drained
fully, finished the Run, and excluded pilot calibration. The queue was 65,536;
admission remained below the 25 ms floor and was recorded diagnostically, not as
a protected timing metric. Ten samples per capture were used.

| Median | 8K A1 | 16K B1 | 16K B2 | 8K A2 |
|---|---:|---:|---:|---:|
| Admission | 6.22M | 5.44M | 5.34M | 5.93M reports/s |
| Admission→persistence | **19.7k** | **293.9k** | **291.1k** | **19.3k reports/s** |
| Finish | 275.8 | 216.4 | 218.1 | 279.0 ms |

Formal gate on persistence primary with finish protected: **PASS**. The
candidate reduced the persistence median by roughly 15x in this controlled
fresh-store workload, while finish stayed below the paired 8K medians. Admission
was intentionally not protected because its samples were shorter than 25 ms;
there is no admission-preservation claim. The earlier same-store 15x signal was
therefore not wholly attributable, but the fresh-store result reproduces the
large direction with aligned setup and fixed report count.

The 16K batch limit is accepted for this bounded candidate. Additional 16K
checks passed: full `cargo test -p seex` (167 passed, 4 ignored), low-rate
single-observation persistence (10 observations, median enqueue-to-persisted
55.0 ms, range 53.4–57.5 ms), and release RSS diagnostics. The low-rate process
reported 79,446,016 bytes peak RSS (~75.7 MiB); the 10k x 5-cycle RSS workload
reported 85,049,344 bytes (~81.1 MiB). These RSS values include the test process
and are diagnostic, not product limits.

The reporting acceptance tests also prove that a mapping batch is kept atomic:
the collection threshold is a report-count target, not a hard point cap, and one
admitted mapping may overshoot it. This matters for memory budgeting and is why
16K is not described as a strict maximum.

This does not prove benefit for default queue-full behavior, four writers, long
runs, or all DuckLake data states. Follow-up acceptance must cover queue pressure,
cancellation, retry replay, four writers and finish/flush visibility.

A matched four-child shared-store diagnostic then compared 8K and 16K, with four
processes writing 32,768 reports to different Runs. It was not a formal gate:
process scheduling and DuckLake state were not controlled tightly enough for
paired protected metrics. The 8K run observed append-wait max 3.18 s, summary-wait
median 706 ms (max 1.20 s), and flush-wait median 533 ms (max 1.76 s). The 16K
run observed append-wait median 85.8 ms (max 544 ms), summary-wait median 149.6 ms
(max 334 ms), and flush-wait median 20.2 ms (max 64 ms). The direction favors 16K,
but the variance is too large to claim a concurrency improvement. Artifacts:
`.tmp/v5-four8-comparison.log`, `.tmp/v5-four16-comparison.log`.

Keep 16K as the accepted single-writer bounded candidate, but treat its
four-writer behavior as INCONCLUSIVE. Do not increase to 32K until a repeated
four-writer comparison has stable lock-tail and RSS protection.

The diagnostic still reports `pipeline_calibrated=false`: admission did not reach
25 ms at the 65,536 queue limit. It is therefore a bounded stress comparison,
not a universal sustained-throughput claim. Artifacts:
`.tmp/v5-pipeline-{manifest.json,result.json,corrected-manifest.json,offline-result.json}`
and the 8K/16K binaries and build logs.

## Batch versus inline-limit path matrix

The ignored `batch_inline_path_matrix` diagnostic uses actual
`ProjectConnection::with_catalog_write_lock` and `append_metric_batch`, including
fresh writers. Every cell has a new native SQLite WAL store, the production
Run/key partition layout, 16,384 observations, and one or 32 keys. Batch sizes
4096/8192/16384 are crossed with inline limits 1024/4096/8192/16384. Read-only connections
verify every commit's total count; diagnostics count only numeric inline data
tables (not similarly named metadata tables) and on-disk Parquet files. Final
flush must preserve count/sum and leave zero inline rows. No physical manifest
or file deletion authority is introduced.

Release run: DuckDB v1.5.4, DuckLake `d318a545`, 73.82 s for 32 cells. Each cell
runs once, not a performance comparison. The initial development run used an
unconfigured `ProjectConnection::new` and is not the fresh-writer evidence;
retain `.tmp/v5-inline-matrix-fresh.log` as the authoritative corrected run.

| Keys | Inline limit | Batch | Inline rows before flush | Parquet files before→after flush | Median append ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 8192 | 8192 | 16384 | 0→1 | 349.95 |
| 1 | 8192 | 16384 | 0 | 1→1 | 50.09 |
| 1 | 16384 | 16384 | 16384 | 0→1 | 857.45 |
| 32 | 8192 | 8192 | 16384 | 0→32 | 365.89 |
| 32 | 8192 | 16384 | 0 | 32→32 | 88.19 |
| 32 | 16384 | 16384 | 16384 | 0→32 | 833.91 |
| 32 | 0 | 256 | 0 | 2048→2048 | 78.19 |
| 32 | 8192 | 256 | 16384 | 0→32 | 61.13 |
| 32 | 1024 | 4096/8192/16384 | 16384 | 128/64/32→same | flush 52/47/48 ms |
| 32 | 4096 | 4096/8192/16384 | 16384 | 32/64/32→same | flush 175/47/57 ms |
| 32 | 8192 | 4096/8192/16384 | 16384 | 32/32/32→same | flush 175/176/49 ms |

The expanded grid shows `inline=1024` is the first candidate that retains a
small-batch inline option while sending these medium batches to Parquet. At 32
keys it produces 32 files for a 256-row batch, 128/64/32 files for 4K/8K/16K
batches, versus 2048 files when inline is disabled at 256-row batches. The
catalog was about 1.68 MiB for inline-heavy 32-key cells, but only about 0.24–0.29
MiB for inline=1024 after direct Parquet commits; the WAL was zero at the final
measurement point after close/flush. The 4K/8K inline-limit cells show large
flush variance (~175–182 ms for 32 keys), so file count alone is not a selection
criterion.

This directly confirms a batch/inline path interaction: at the configured limit
an 8192-row insert stays inline, while a 16384-row insert with limit8192 writes
Parquet. Raising the limit to16384 moves that same batch back into inline and
restores expensive append work. Observed behavior is based on the aggregate
insert even when its 32 partitions each contain only512 rows. It is not simply
"half as many attach calls". The prior15x result has a concrete path-switch
explanation, although exact cost attribution remains diagnostic, not a profile
of DuckLake internals.

Inline solves small-file churn: 32-key/256-row commits with inline disabled
produce2048 files (~3.02MiB), versus32 files (~0.20MiB) after inline flush.
The limit determines eligibility of a write, not a cap on accumulated inline
rows: multiple writes can leave16384 rows inline with limit8192. Inline facts
are persisted and query-visible before Parquet flush; they are not an additional
Seex memory queue. Benefits must be balanced against SQLite/WAL writes, metadata
lock contention, later table-wide flush and stale-connection handling.

## Next independent slice

Keep batch16384 and production inline8192 unchanged for now. Evaluate inline1024
as one isolated candidate: retain genuinely small writes inline, send medium
batches to Parquet. Include low-rate/many-key file counts, append+finish latency,
WAL/catalog growth, four-writer correctness and RSS. Do not disable inline
universally or raise it along with batch size. DuckLake retains file ownership;
layout switches, Rollup and large-scale P7 remain outside this slice.
