# ADR 0007: V2 Logical Schema and Parquet Layout

## Status
Accepted.

## Context
V2 needs the catalog database, DuckLake logical schema, and Parquet data layout
to support high-throughput metric writes without putting project metadata into
the data path.

## Decision
Catalog backends must use the same PulseOn application table shape for project,
run, and query-index state across DuckDB, SQLite, and future PostgreSQL-backed
catalogs. These are catalog application tables, not DuckLake logical tables:
`pulseon_projects`, `pulseon_runs`, and `pulseon_metric_aggregates`.

Project information lives in the catalog database only; the data path must not
store project-scoped directories or project columns for metric facts. The
canonical product term remains `Run`; "experiment" is an informal synonym for a
run and should not appear in schema names.

`metric_points` keeps the user-facing `metric_key` and adds
`metric_key_encoded`, a reversible percent-encoded value used for partition
layout. Flushed Parquet is partitioned by `run_id` and `metric_key_encoded`:

```text
data/main/metric_points/
  run_id=<run_id>/
    metric_key_encoded=<encoded_metric_key>/
      ducklake-*.parquet
```

V2 does not partition metric points by step. Large series may produce multiple
Parquet files within the same run/key partition, but not additional path
levels.

`metric_key_encoded` is part of the public Parquet schema and partition
contract. Its value uses RFC 3986 percent-encoding over UTF-8 bytes: unreserved
ASCII characters `[A-Za-z0-9._~-]` remain literal and all other bytes are
encoded as uppercase `%XX`.

That value is the logical column value. DuckLake may escape physical
Hive-style partition directory values again when writing paths, so the logical
value `train%2Floss` can appear on disk as
`metric_key_encoded=train%252Floss/`.

`metric_aggregates` may be built or refreshed after run finalization rather
than maintained during active metric reporting. During active runs, fresh
queries read persisted `metric_points` through DuckLake.

V2 does not add a durable `run_storage_state` or Parquet visibility table. Run
finalization is expected to drain and flush. If Parquet flush fails after
terminal lifecycle state is written, PulseOn raises `MetricFlushError` and does
not roll back the terminal run state. `flush_run_data(run_id)` retries Parquet
flush for a terminal run without changing lifecycle state. It returns `None`
on success, is idempotent when the terminal run is already Parquet-visible, and
rejects running runs with `InvalidRunStateError`. It supports an optional
timeout; if no timeout is supplied, it waits until flush succeeds or fails, and
a bounded timeout raises `MetricFlushTimeoutError`. Timeout leaves the terminal
run unchanged and may be retried. If the process crashes after a flush succeeds
but before the caller observes success, restart-time recovery still uses
idempotent `flush_run_data(run_id)` rather than durable visibility state.
Terminal-run flush work is serialized inside one client with a client-wide
flush mutex; waiting for that mutex counts against a supplied timeout. Ordinary
`MetricFlushError` messages should include the failed operation and basename,
not full local paths by default. Explicit debug dumps or verbose diagnostics are
post-v2 work. Failed and timed-out flushes update runtime-only client
diagnostics (`last_flush_run_id`, `last_flush_status`, and `last_flush_error`)
for the current process's most recent terminal-run flush attempt; they are not
stored as long-lived catalog state.

Normal `finish_run(...)` and `fail_run(...)` paths must close metric admission
for the run, drain through the current client's enqueue barrier, and only then
flush inline metric data. Later `run.log(...)` calls for the run raise
`RunClosedError`. The boundary is the Rust admission gate order, not Python
call start time: reports admitted before the close barrier are drained; reports
that reach admission after the barrier fail. The drain is intentionally
client-wide because the writer owns one ordered queue per client. If drain
fails or times out, PulseOn must not write terminal run state or proceed to the
flush step. This guarantees that, on the normal API path, reports admitted by
the client before the close barrier are persisted before flush begins. It does
not cover process crash, `SIGKILL`, power loss, or reports that were only
queued and not yet persisted before finalization started.

V2 does not support concurrent writer clients for the same run. A local
run-writer lock is required before a client can create or resume a writable
run handle. The lock is an OS advisory file lock; v2 does not add a lock table,
lease protocol, heartbeat, or stale-lock cleanup system. Lock acquisition is a
try-lock, not a blocking wait. If another client already holds that lock,
`create_run(...)` or `resume_run(run_id)` immediately raises non-fatal
`RunAlreadyActiveError` with the conflicting `run_id` in the message, but not
the local lock path by default. This keeps close barriers,
enqueue order, and writer-assigned `ingested_at` ordering scoped to one writer
pipeline. Lock files live under
`<project>/.pulseon/locks/runs/<percent-encoded-run-id>.lock` and are local
runtime state, not catalog tables. The encoded run id uses the same RFC 3986
percent-encoding rule as `metric_key_encoded`. Filesystem I/O, permission, or
disk failures affecting catalog paths, data paths, lock directories, or lock
files raise `StorageError`, not `InvalidConfigurationError`. Ordinary
`StorageError` messages should include the failed operation and basename, not
full local paths by default. Full local paths are not exposed through ordinary
diagnostics in v2 Phase A; they are reserved for a future explicit debug/verbose
facility or internal chained/source errors intended for debugging.
Initialization-time `StorageError` is fatal to client startup; runtime
`StorageError` from direct API operations does not automatically enter failed
writer state unless the background writer also exhausts persistence retries.

If finalization drain times out, the terminal lifecycle state is not written
and the current client keeps the run-writer lock because the run remains
writable. Once terminal lifecycle state is written, the run-writer lock is
released even if the later Parquet flush raises `MetricFlushError`.

`shutdown()` is client teardown, not run finalization. A bounded shutdown
attempt drains first while client-wide metric admission remains open; if it
times out, shutdown did not complete and the client remains usable. Once drain
succeeds, PulseOn atomically closes client-wide metric admission, stops the
writer, and releases resources. Shutdown does not mark running runs as finished
or failed and does not flush those running runs to Parquet. Later `run.log(...)`
calls through a shut-down client raise `ClientClosedError`. Context-manager
exit follows the same rule and may leave running runs as running/orphaned for
later explicit resume, finish, or fail. Those running runs may have metric data
persisted in DuckLake but not forced Parquet-visible; only terminal runs require
Parquet visibility in v2.

DuckLake remains the v2 writer and catalog path for metric facts. Future
ClickHouse integration may replace or augment the serving/query backend, but
Parquet remains the v2 open compatibility boundary.

`metric_points` does not enforce physical uniqueness. Duplicate
`(run_id, metric_key, step)` rows remain valid physical history, and query
semantics resolve them with logical last-write-wins ordering by writer-assigned
`ingested_at`.

## Consequences
- Project-scoped reads and exports must join or filter through catalog
  `pulseon_runs` metadata before scanning metric point partitions.
- External readers can inspect metric data by run and metric series without
  depending on project metadata in the data path.
- Querying active runs may be more expensive because aggregate state is not the
  freshness source during training.
- Without durable Parquet visibility state, restart-time recovery cannot report
  historical flush failures that happened before process exit.
- Runtime-only flush diagnostics are useful for the current training process,
  logs, and tests, but they describe only the current process's most recent
  terminal-run flush attempt. They are not a recovery protocol and cannot
  answer historical visibility questions after restart.
