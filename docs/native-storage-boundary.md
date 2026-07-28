# Native Storage Boundary

This document records the native storage boundary between the catalog database
and the DuckLake data area.

## Decision

Seex treats the catalog database as the primary home for control-plane state
and query index state. The DuckLake data area is primarily for large
Parquet-backed fact data.

```text
catalog database
  DuckLake catalog metadata
  seex_projects
  seex_runs
  seex_metric_aggregates
  small inline data managed by DuckLake

data area / object storage
  partitioned metric_points Parquet files
```

Local native catalogs use DuckDB or SQLite. PostgreSQL is a future
shared-catalog option. Object storage is for the data area, not for the catalog
database file itself.

The default project-local store is:

```text
<project>/.seex/
  config.toml        # optional native project configuration
  catalog.ducklake   # default DuckDB-backed DuckLake catalog
  catalog.sqlite     # SQLite-backed DuckLake catalog
  data/              # default Parquet data path
```

Users may override `catalog_path` and `data_path` independently. `catalog_path`
remains a local filesystem path. `data_path` may be local or S3-compatible,
such as `s3://bucket/prefix`. Backend-specific catalog defaults use
conventional filenames, but an explicit `catalog_path` is used as provided.
The catalog database remains local unless a shared-catalog decision says
otherwise.

Relative paths read from `<project>/.seex/config.toml` are resolved against
the project root. Explicit SDK paths retain their caller-provided resolution
behavior.

## Why

`seex_projects`, `seex_runs`, and `seex_metric_aggregates` are small,
hot, and transactional relative to metric points. They serve project selection,
run lifecycle updates, or metric discovery and summary queries. Keeping them in
the catalog database avoids small Parquet file churn, object-store read/write
amplification, and awkward transactional updates for fields such as run status
and finish time.

`metric_points` is the large append-oriented fact table. It is the table most
likely to grow to object-store scale, and it is the main long-series analytic
workload. Its durable open boundary should remain partitioned and
Parquet-shaped so local and external readers can scan or export metric series
without depending on Seex internals.

`metric_aggregates` is derived index state over the effective metric series. It
may be repaired or rebuilt from `metric_points`, but it should be stored where
lookup and transactional refresh are cheap.

## Catalog Database Responsibilities

- Store DuckLake table, schema, snapshot, and data-file metadata.
- Store small Seex control-plane application tables:
  `seex_projects` and `seex_runs`.
- Store query index state such as `seex_metric_aggregates`.
- Own transactional updates for run lifecycle and aggregate refresh.
- Optionally hold DuckLake inline data for small batches before flush.

Field-level schemas for Seex-owned catalog tables live in
`docs/catalog-application-tables.md`.

Seex SQL addresses catalog application tables through Seex-owned names
or a backend-aware adapter, not through DuckLake's internal metadata alias.
This keeps control-plane and query-index state from becoming coupled to
DuckLake internal catalog naming. The tables live in the same catalog database
file as DuckLake metadata for both DuckDB and SQLite local backends.

## Data Area Responsibilities

- Store large Parquet data files for metric facts.
- Support local filesystem paths in native mode.
- Support a custom local Parquet data path.
- Support S3-compatible object-storage data paths, including local MinIO.
- Keep the metric point schema compatible with the Parquet contract.
- Partition metric point files for export-friendly layout.
- Preserve accepted metric points without making the training hot path wait for
  DuckDB, DuckLake, or Parquet flush work.
- Flush inline metric data to Parquet when a run ends.

## Read Contract

The Seex read surface exposes catalog-backed project and run metadata plus
persisted effective metric points:

- Project discovery reads `seex_projects`.
- Run discovery reads `seex_runs`, including lifecycle status.
- Metric discovery, summaries, and point queries derive their results from
  persisted `dl.metric_points` or from terminal-run aggregate indexes rebuilt
  from those points.

An effective metric series contains at most one point for each
`(run_id, metric_key, step)`. When a step has been written more than once, the
point with the latest writer-assigned `ingested_at` wins; the persisted row
order breaks a remaining timestamp tie. Point queries return this effective
series in ascending step order. Step ranges are half-open: `start_step` is
inclusive and `end_step` is exclusive, so a bounded query selects
`[start_step, end_step)`.

A report waiting in the current client's in-process queue is not persisted and
is outside query visibility. A successful `run.log(...)` call means the report
was queued, not that a concurrent read must already observe it. Callers that
need visibility during a running run must wait for persistence; run
finalization establishes the drain barrier before terminal state is written.

Persisted inline DuckLake rows and flushed Parquet rows have the same logical
query visibility. Parquet visibility is an export property for terminal runs,
not an additional condition for Seex reads. Catalog metadata and derived
aggregate indexes are not part of the Parquet compatibility boundary.

## Metric Point Partitioning

Metric data means the `metric_points` fact table.
`metric_points` must be partitioned when flushed to Parquet. Seex partitions
by `run_id` and `metric_key_encoded`; it does not denormalize `project_id` into
the metric point fact table. Project-scoped exports must use catalog
`seex_runs(project_id)` metadata.

```text
data/main/metric_points/
  run_id=<run_id>/
    metric_key_encoded=<encoded_metric_key>/
      ducklake-*.parquet
```

This mirrors the DuckLake behavior validated in
`docs/reference/ducklake-archive.md`: DuckLake writes partitioned Parquet using
Hive-style `column=value/` directories after `ALTER TABLE ... SET PARTITIONED
BY (...)`.

Raw metric keys may contain path separators, such as `train/loss`. Seex
therefore adds a public `metric_key_encoded` column containing the reversible
percent-encoded metric key. The user-facing `metric_key` remains unencoded in
the logical fact table, while DuckLake partitions flushed Parquet by `run_id`
and `metric_key_encoded`. The encoding uses RFC 3986 percent-encoding over
UTF-8 bytes: unreserved ASCII characters `[A-Za-z0-9._~-]` remain literal and
all other bytes are encoded as uppercase `%XX`.

`metric_key_encoded` is the logical column value. DuckLake may escape partition
directory values again when it writes Hive-style `column=value/` paths, so a
logical key value such as `train%2Floss` can appear on disk under
`metric_key_encoded=train%252Floss/`.

Seex does not partition by `step`. Very long metric series may produce
multiple Parquet files in one run/key partition, but not additional step-based
directory levels.

## Custom Data Path

Seex has two paths:

- `catalog_path`: the local catalog database path under `<project>/.seex/`.
- `data_path`: the Parquet data area used by DuckLake.

The default `data_path` is `<project>/.seex/data`. Users may override it
with a local filesystem path:

```text
catalog_path = <project>/.seex/catalog.ducklake
data_path    = /mnt/seex/<project>/data/
```

When `data_path` is object storage, DuckLake records relative data-file paths
in the catalog and resolves them against the configured object-store URI.
Accessing S3-compatible storage requires the DuckDB HTTPFS/S3 configuration
outside the catalog file itself.

S3 configuration lives in `<project>/.seex/config.toml`. Explicit
`seex.init(...)` keyword arguments override config-file values. S3 secrets
configure only the current DuckDB connection and must not be copied into
DuckLake or Seex catalog tables. S3-backed `data_path` is supported for both
DuckDB and SQLite catalog backends, while `catalog_path` remains local-only.

## Metric Ingestion Durability

Metric ingestion has two separate requirements:

- Training code must not wait for DuckDB writes, DuckLake snapshot work,
  aggregate refresh, object-storage I/O, or Parquet flush.
- Seex must not silently drop metric points that it has accepted from user
  code.

The storage boundary therefore requires a clear distinction between
queued reports and accepted reports. Admission into a bounded in-process memory
queue alone is not acceptance. A report becomes accepted when the background
batch writer persists it into DuckLake, so accepted reports and persisted
metric points are the same state.

If Seex cannot queue a metric report because the bounded metric queue is
full, that condition must be surfaced as an explicit queue-full failure rather
than a silent drop. Diagnostics stay intentionally small: they expose pending
reports, queue-full failures, persisted reports, writer state, and sanitized
last errors so callers can tell whether data is durable, merely pending, or not
accepted.

## Run-End Flush

DuckLake may keep small metric batches inline in the catalog to avoid creating a
tiny Parquet file per training step. That behavior is desirable during an
active run, especially for high-frequency logging.

When a run ends, Seex must flush inline `metric_points` data to the
configured Parquet data path. The flush is required so export workflows can
observe the completed metric data as partitioned Parquet.

The flush operation should preserve the non-blocking hot-path rule for
`run.log(...)`. It belongs to the run finalization path, not to individual
metric report calls. A bounded flush may delay Parquet visibility, but it must
not discard accepted metric points. Flush failure after terminal lifecycle state
is written must raise `MetricFlushError`, must not roll back the run's terminal
state, and must update runtime-only diagnostics. Ordinary `MetricFlushError`
messages must include the failed operation and basename, not full local paths by
default; explicit debug dumps or verbose diagnostics are future work. Users
retry that visibility step with `flush_run_data(run_id)`, not by calling
`finish_run(...)` or `fail_run(...)` again.

Diagnostics are exposed through `client.diagnostics()` as a public read-only
runtime snapshot for users, tests, and operational debugging. They describe the
current client process only. They are not query results, durable history,
catalog truth, or a recovery protocol. Seex does not add per-run diagnostics
queries, diagnostics event history, telemetry export, or durable diagnostic
tables. `client.diagnostics()` remains callable after `shutdown()` and returns
the last runtime snapshot. Diagnostics reads are safe, but Seex does not
promise cross-field atomic consistency; callers that need lifecycle or drain
guarantees must use explicit APIs. Report counters and `writer_state` are
runtime-only current-client state. They are not catalog state, not durable
history, and are not restored when another process opens the same project.

Normal finalization must drain through the current client's enqueue barrier
before the flush step. Finalization first closes metric admission for the run,
so later `run.log(...)` calls for that run raise `RunClosedError` instead of
creating new queued reports during drain or flush. Seex then waits until all
reports admitted by the client before that close barrier are persisted, and
only then writes terminal run state and flushes inline data to Parquet. This
client-wide barrier is intentional: the writer owns one ordered queue per
client, not independent per-run queues.

If drain fails or times out, Seex must not proceed to terminal state or
flush. This guarantees that, on the normal API path, the run has no queued
metric reports left before flush begins. The guarantee does not cover process
crash, `SIGKILL`, power loss, or reports that were only queued and not yet
persisted before finalization started.

`shutdown()` is different from run finalization. It is client teardown. A
bounded shutdown attempt drains first while client-wide metric admission remains
open; if that attempt times out, shutdown did not complete and the client
remains usable. Once drain succeeds, Seex atomically closes client-wide
metric admission, stops the background writer, and releases resources. Shutdown
does not decide whether active runs are `finished` or `failed`, does not write
terminal run state for running runs, and therefore does not flush running runs
to Parquet. Context-manager exit follows the same resource-release rule. If the
user block is already raising an exception,
Seex must not mask that original exception with a writer-failed or
drain-timeout teardown error; the user exception takes priority, with the
teardown error attached as exception context when practical. Seex must not
add a new logging dependency for teardown errors; diagnostics remain the
observable state if exception chaining is too costly. Even when a user
exception is active, context-manager exit still performs best-effort resource
release. If drain
times out during context-manager exit, shutdown did not complete: metric
admission stays open, the run-writer lock remains held, and the client remains
usable if the caller still has a reference. With no user exception active,
Seex raises `MetricDrainTimeoutError`; with a user exception active, the
user exception remains primary and the timeout is observable through
diagnostics or exception context when practical.

Users who want terminal-run Parquet visibility must call `finish_run(...)` or
`fail_run(...)`; users recovering from a prior flush failure call
`flush_run_data(run_id)`. Calling `run.log(...)`
through a shut-down client raises `ClientClosedError`; this is separate from
`RunClosedError` because the underlying run may still be running and resumable.
Calling `flush_run_data(run_id)` for a running run raises
`InvalidRunStateError`. If a terminal run is already Parquet-visible,
`flush_run_data(run_id)` is idempotent and returns `None`. Timeout leaves the
terminal run unchanged and may be retried. Within one client, terminal-run
flush work is serialized by a client-wide flush mutex; waiting for that mutex
counts against a supplied timeout. `Run` handles from a shut-down client are
permanently unusable; users must create a new client and resume a running run
to continue logging. `resume_run(run_id)` rejects terminal runs with
`InvalidRunStateError`; terminal runs remain queryable through client query
APIs. `finish_run(...)` and `fail_run(...)` reject already terminal runs with
`InvalidRunStateError`, including repeated calls with the same target terminal
state.

Current native mode supports only one active writer client per run. A local
run-writer lock prevents two clients from concurrently logging to the same
running run.
`create_run(...)` and `resume_run(run_id)` must acquire the lock before
returning a writable `Run` handle. The lock is an OS advisory file lock;
Seex does not add a lock table, lease protocol, heartbeat, or stale-lock
cleanup system.
Lock acquisition is a try-lock, not a blocking wait. If the lock is already
held, `create_run(...)` or `resume_run(run_id)` immediately raises
non-fatal `RunAlreadyActiveError` with the conflicting `run_id` in the message,
but not the local lock path by default. This preserves the
meaning of close barriers, enqueue order, and writer-assigned `ingested_at` for
last-write-wins queries.
`create_run(user_supplied_run_id)` remains distinct from resume: if the run
already exists, it raises `RunAlreadyExistsError` and callers must use explicit
`resume_run(...)` even when no writer lock is active. `RunAlreadyExistsError`
includes the conflicting `run_id` in the message.

Run-writer lock files live under:

```text
<project>/.seex/locks/runs/<percent-encoded-run-id>.lock
```

They are local runtime state and are not catalog tables. Finalization drain
timeout keeps the lock held because the run remains writable by the current
client. Writer failure also keeps the lock held until the user explicitly calls
`shutdown()` or the process exits; Seex does not auto-release locks on
writer failure. After failed-client shutdown releases resources, a new client
may resume the non-terminal run if it can acquire the writer lock, but reports
left pending in the failed client were not durably admitted and are lost. Once
terminal lifecycle state is written, the lock is released even if the later
Parquet flush raises `MetricFlushError`. Lock cleanup may remove the lock file
only when it can prove that it is deleting the original file for the released
writer. If that cannot be proven safely, Seex must leave the file on disk.
The encoded run id uses the same
RFC 3986 percent-encoding rule as `metric_key_encoded`. Filesystem I/O,
permission, or disk failures affecting catalog paths, data paths, lock
directories, or lock files raise `StorageError`, not
`InvalidConfigurationError`. Ordinary `StorageError` messages should include
the failed operation and basename, not full local paths by default. Full local
paths are not exposed through ordinary diagnostics; they are reserved for an
explicit debug/verbose facility or internal chained/source errors intended for
debugging.

Initialization-time `StorageError` prevents the client from starting. Runtime
`StorageError` from direct API operations is an operation error and does not
automatically put the client into failed writer state unless the background
writer also exhausts persistence retries.

Because `shutdown()` does not finalize runs, it may leave running-run metric
data persisted in DuckLake but not forced into Parquet files. That is accepted
in current native mode: only terminal runs require Parquet visibility.

## Implications

The Parquet schema contract documents stable table shapes for reading and
export, but it does not require every Seex table to use Parquet as its
primary storage location. `seex_projects`, `seex_runs`, and
`seex_metric_aggregates` are catalog application tables and are not DuckLake
logical tables or DuckLake internal tables.

Seex may build or refresh `metric_aggregates` after a run is finalized
instead of maintaining it synchronously during active metric reporting.
Active-run queries that require fresh data should read persisted
`metric_points` through DuckLake.

DuckLake may create directories for its logical tables under the data area. That
physical layout is an implementation detail, but Seex should not model
project, run, or aggregate application state as DuckLake logical tables.

Small metric batches may also remain inline in the catalog before run end. This
is acceptable because it avoids small-file churn. After run end, inline
`metric_points` data must be flushed to the configured data path.

Seex does not store durable Parquet visibility state such as a
`run_storage_state` table. Finalization is expected to drain the run and flush
terminal-run data; shutdown drains queued reports but does not force running
runs into Parquet. Flush failures are surfaced by the operation or by
runtime-only client diagnostics (`last_flush_run_id`, `last_flush_status`, and
`last_flush_error`) rather than persisted as long-lived catalog state. Those
diagnostics describe only the current client process's most recent terminal-run
flush attempt. They help the current process observe and test flush behavior;
they are reset by process restart and are not a recovery protocol.

## Non-Goals

- This does not introduce a public `StorageLayer`.
- This does not require PostgreSQL for current local native mode.
- This does not make DuckLake catalog internals part of the Seex API.
