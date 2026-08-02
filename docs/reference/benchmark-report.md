# Seex Benchmark Report

## Measurement Record

- Date: 2026-07-13
- Commit: `37da340`
- Platform: macOS 26.3, arm64
- Python: CPython 3.13.2
- Seex lineage: 0.1.0a4 baseline (measured before the identity reset)
- Local object storage: MinIO `RELEASE.2025-09-07T16-13-09Z`, linux/arm64
- Cloud object storage: configured S3-compatible OSS; endpoint, bucket, base
  prefix, and credentials are intentionally omitted.

These measurements are environment-specific regression baselines. The roadmap
defines unrelated partition reads as a hard failure, but does not define
portable latency, throughput, or amplification limits.

## Benchmark Roles

| Script | Measures | Durability included? | Storage comparison |
| --- | --- | --- | --- |
| `bench_log_throughput.py` | Isolated `run.log()` admission ceiling | No | No |
| `bench_log_persistence.py` | Setup, admission, drain, finalization, shutdown | Yes | Local and S3/OSS |
| `bench_minio_metric_query.py` | Query latency, remote bytes, partition pruning, read amplification | Read path | DuckDB/SQLite over MinIO |

The first two scripts intentionally overlap on admission throughput. The first
isolates the hot-path ceiling in a fresh child process; the second retains
admission as one phase of an end-to-end durability workflow. The query script
measures a separate read path and does not duplicate either write benchmark.

## Commands

```bash
uv run python scripts/bench_log_throughput.py --reports 100000

uv run python scripts/bench_log_persistence.py \
  --reports 1000 \
  --repeats 3 \
  --object-storage-config .seex/config.toml

uv run python scripts/bench_minio_metric_query.py
```

The persistence results below redact configured storage paths. The MinIO query
command used an ephemeral local MinIO instance and the required
`SEEX_MINIO_*` environment variables. Cloud OSS query timing used the same
dataset and selection through the SDK because OSS does not expose MinIO admin
trace APIs.

## Admission Throughput

| Reports | Elapsed | Throughput | Queue-full errors |
| ---: | ---: | ---: | ---: |
| 100,000 | 0.052267 s | 1,913,267 calls/s | 0 |

All reports were still pending and zero were persisted when timing ended. This
is an admission-only ceiling, not durable storage throughput. The result shows
that the training hot path can enqueue metrics quickly without waiting for
DuckLake or object storage.

### Phase 2B observation-time follow-up

On 2026-07-20, the observation timestamp was moved from the background writer
to the `run.log()` enqueue path. On the same macOS arm64 host and Python 3.13.2,
the unchanged `100000`-report command measured 1,750,997 calls/s before the
change at commit `e12fd43`. Three runs after the change measured
1,482,733-1,581,208 calls/s, with a median of 1,522,925 calls/s. The measured
median regression was 13.0%, attributable to one wall-clock read per admitted
report. All runs reported zero queue-full errors, and the result remains well
above the original 100,000 calls/s target. `MetricReport` grew from 72 to 80
bytes, adding about 512 KiB at the default 65,536-report queue capacity.

## Persistence Results

Each target ran three independent repeats with 1,000 reports. Values are median
with the observed min–max range.

| Phase | Local | Cloud OSS | OSS/local median |
| --- | ---: | ---: | ---: |
| Setup | 92.068 ms (91.229–98.931) | 110.494 ms (107.841–116.435) | 1.20× |
| Admission | 0.793 ms (0.764–0.813) | 0.871 ms (0.819–0.905) | 1.10× |
| Admission throughput | 1,260,703 calls/s | 1,147,556 calls/s | 0.91× |
| Drain | 57.790 ms (57.765–58.140) | 57.434 ms (47.450–59.121) | 0.99× |
| Finalization | 37.481 ms (35.270–41.057) | 196.309 ms (175.229–248.418) | 5.24× |
| Shutdown | 0.058 ms (0.053–0.059) | 0.057 ms (0.052–0.064) | 0.99× |

All six repeats completed with zero queue-full errors, zero pending reports
after drain, exactly 1,000 persisted reports, successful finalization flushes,
and a closed writer after shutdown. Admission remains effectively independent
of storage; the expected cloud cost is concentrated in terminal Parquet flush.

## Object Storage Query Scenario

For each catalog backend, the dataset contained three runs and four metric keys
per run, with 10,000 steps in every series. It queried `run-1`, `metric/2`, and
the half-open range `[4000, 6000)` five times. Every query returned 2,000 points.

### Local MinIO Results

| Catalog | Query latency min / median / max | Remote bytes | Parquet GETs | Read amplification | Unrelated reads |
| --- | ---: | ---: | ---: | ---: | ---: |
| DuckDB | 18.952 / 25.746 / 68.962 ms | 221,988 | 7 | 0.9250× | 0 |
| SQLite | 28.753 / 49.153 / 73.497 ms | 221,943 | 7 | 0.9248× | 0 |

MinIO trace ran only around the repeated queries. Remote bytes are the sum of
`callStats.tx`. Logical result size is 24 fixed-width bytes per point (step,
timestamp, and value) per repeat; read amplification is Parquet response bytes
divided by that size. Compression and cache reuse can make this workload-level
ratio lower than one.

The structural gate passed for both catalog backends: all seven Parquet GETs
targeted the selected `run_id` and `metric_key_encoded` partition, with zero
unrelated reads.

### Cloud OSS Results

The first query is separated because later queries reuse the client cache.

| Catalog | Client open | Dataset population | Median run finalization | First query | Warm-query median | All-query median |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| DuckDB | 109.8 ms | 7.086 s | 2.354 s | 359.5 ms | 28.6 ms | 34.1 ms |
| SQLite | 136.7 ms | 7.437 s | 2.412 s | 322.5 ms | 30.6 ms | 34.1 ms |

Cloud cold reads were roughly 11–13 times slower than warm-query medians. Once
cached, both catalog backends converged near 29–31 ms. The result shows a large
first object-fetch cost, not a persistent order-of-magnitude slowdown.

OSS does not expose MinIO `admin trace`, and the configured credentials do not
grant `ListBucket`. Cloud response bytes, amplification, object counts, and
independent unrelated-partition evidence are therefore unavailable. Test
objects were retained under
`seex-query-bench-oss/7e0a3c95e02e420eb5d767fab115c236/`.

## Verification and Assessment

The current implementation passed:

- Rust formatting, strict Clippy, `cargo check`, and 80 Rust tests
- Maturin develop install, Pyright with no errors, and 86 Python tests
- real MinIO query gate; 2 opt-in pytest MinIO tests skipped without environment

The implementation meets the current roadmap performance requirements: the
training hot path remains non-blocking, persistence completes without loss or
queue overflow in the measured workload, both catalog backends return correct
query results, and the hard partition-pruning gate passes. The data is not a
production SLO: repeat counts are small, local MinIO runs on the benchmark host,
and cloud OSS lacks request-level trace visibility.
