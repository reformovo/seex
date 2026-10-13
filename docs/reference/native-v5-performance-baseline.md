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

## Next independent slice

Fix reporting calibration and retain explicit batch versus per-operation units,
without increasing queue capacity or changing persistence behavior. Then diagnose
connection/attach, catalog lock wait/hold, summary and flush costs before choosing
one runtime candidate. Long/narrow Reader queries, diagnostic cache miss/hit,
16 curves, RSS, temporary storage and large-scale P7 remain unmeasured here.
Do not introduce Rollup or production layout switches to explain these results.
