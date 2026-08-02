# Seex V1 Native Architecture

## Scope
V1 validates a local-first loop for individual trainers: create project, create
run, log numeric metrics, query chart-ready data, and compare runs.

V1 value is local ownership with an open Parquet boundary. AI Native, Cloud,
MCP, and agent memory are future work.

## Domain Model
```text
Project 1 ── * Run 1 ── * MetricSeries 1 ── * MetricPoint
```

Project fields: `project_id`, `name`, `created_at`. Run fields: `run_id`,
`project_id`, `name`, `status`, `created_at`, `started_at`, `finished_at`.
Configs, tags, and `updated_at` are not v1 requirements.

Initial run lifecycle:

```text
running -> finished
running -> failed
```

Orphan `running` runs should remain detectable for future recovery.

## Metric Model
Metrics are numeric long-table series keyed by `(run_id, metric_key)`.

```text
run_id       string
metric_key   string
step         int64
timestamp    timestamp
value_f64    double
ingested_at  timestamp
```

Metric keys may be hierarchy-like names such as `train/loss`; path escaping is
implementation detail.

Duplicate `(run_id, metric_key, step)` writes are last-write-wins by internal
ingest time. Queries order chart data by `step`.

If `step` is omitted, Seex assigns the next step for that `(run_id,
metric_key)` series. Existing `run_id` values require explicit resume.

Metric discovery and summaries are materialized-view-like aggregate state over
the effective series: count/last/min/max, with async repair allowed.

Metric reporting is part of the training hot path and must be non-blocking.
`run.log(...)` may buffer metric points, but it must not wait for durable
storage flush, aggregate repair, query index maintenance, downsampling work, or
future upload/export work. When reporting cannot keep up, v1 prefers observable
metric loss or delayed visibility over blocking the training step. An accepted
report means Seex accepted the report into the native in-process buffer; it
does not mean a metric point has been durably stored. Run finalization attempts
a best-effort native writer drain for up to 500 ms before recording the
terminal run status. It must not hang indefinitely; if the drain does not
complete in time, finalization continues and diagnostics remain the place to
inspect delayed or failed metric reports. The explicit client shutdown path uses
the same bounded-drain rule.

## Storage
DuckLake is required in native v1 to avoid custom staging, flush, and compaction
before validation.

The stable product contract is the Seex Parquet schema documented in
`docs/parquet-schema-contract.md`. DuckLake catalog metadata is
implementation detail.

## Query Goals
Both query paths matter for v1: long single-series chart queries with range
  selection and downsampling.
- Many-run comparison queries using run summaries.
- Metric discovery through catalog/index data rather than full fact-table
  scans.

Initial API shape: `run.log(key, value)`, `run.log(key, step, value)`,
`query_metric(..., max_points=None)`, and summary query. `max_points` is strict;
short series stay unchanged and downsampling preserves endpoints via DuckDB LTTB.
Ordinary hot-path `run.log(...)` calls do not raise by default for transient
storage or backpressure failures; those failures must be surfaced through
diagnostics.

Initial scale targets: around 10,000 runs, up to 1,000,000 distinct metric
keys, very long metric series with downsampled chart-ready output, and TB-scale
local datasets.

## Implementation Boundaries
Keep v1 code focused on native mode: no deletion, workspace hierarchy,
config/tag filtering, Cloud skeletons, public `StorageLayer`, agent tables, or MCP.

The Python API should return chart-ready query data only. Built-in plotting,
rendering APIs, and plotting dependencies stay outside Seex v1.

Future architecture documents may preserve Cloud and AI Native constraints, but
they should not drive v1 code shape until the native loop is proven.
