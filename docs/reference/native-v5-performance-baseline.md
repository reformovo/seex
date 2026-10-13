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

## Next independent slice

Investigate whether one bounded write operation can amortize fresh DuckLake attach
without extending stale connection lifetime: compare current per-operation fresh
writer behavior with a narrowly scoped batch boundary, preserving the catalog
lock and recovery rules. Do not change production behavior until an isolated
correctness probe passes. Long/narrow
Reader queries, diagnostic cache miss/hit, 16 curves, RSS, temporary storage and
large-scale P7 remain unmeasured. Choose one runtime candidate at a time.
Do not introduce Rollup or production layout switches to explain these results.
