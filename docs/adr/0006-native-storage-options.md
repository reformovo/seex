# ADR 0006: V2 Native Storage Options

## Status
Accepted.

## Context
V2 needs to turn the catalog/data boundary into an implementable native
configuration surface without expanding into shared cloud storage or
multi-client catalog coordination.

## Decision
V2 supports DuckDB as the native DuckLake catalog backend. SQLite remains a
named but explicitly deferred backend until real DuckLake-backed SQLite tests
prove the same schema, transaction, locking, and multi-client behavior.
PostgreSQL is not part of the v2 public surface. Catalog backend selection is
explicit rather than inferred from the catalog path suffix. The Python
initialization API may grow a keyword-based configuration shape:

```python
pulseon.init(
    path,
    data_path=None,
    catalog_backend="duckdb",
    catalog_path=None,
    metric_queue_capacity=65536,
)
```

`metric_queue_capacity` must be between 1 and 1,048,576 inclusive. Invalid
capacity values, unknown catalog backends, deferred catalog backends, and
unsupported data paths raise `InvalidConfigurationError`, a subclass of
`PulseOnError`, before the client starts. The only startup-supported
`catalog_backend` value is the case-sensitive string `"duckdb"`; `"sqlite"` is
recognized only to return an explicit deferred-backend error.

Default paths are backend-specific:

```text
duckdb catalog_path = <project>/.pulseon/catalog.ducklake
data_path           = <project>/.pulseon/data
```

The public v2 `data_path` contract is local filesystem paths only. S3-compatible
object-storage URIs, including local MinIO, are deferred to a post-v2 release.
DuckDB is the v2 validation target. SQLite remains deferred because the same
real DuckLake-backed schema, transaction, locking, and multi-client correctness
tests have not yet passed against it. V2 must not fake backend compatibility by
matching table names only.

V2 local native mode supports one active writer client per run. A local
run-writer lock prevents two clients from concurrently creating, resuming, or
writing the same run. The lock must be an OS advisory file lock, not a PulseOn
lock table, lease protocol, heartbeat, or stale-lock cleanup system. Process
crash should release the OS lock even if the lock file remains on disk, and a
lock file without a held OS lock is not active. Lock acquisition uses try-lock
semantics and must not block waiting for another writer. If the lock is already
held, `create_run(...)` or `resume_run(run_id)` immediately raises non-fatal
`RunAlreadyActiveError` with the conflicting `run_id` in the message, but not
the local lock path by default.
`create_run(user_supplied_run_id)` remains distinct from resume: if the run
already exists, it raises `RunAlreadyExistsError` and callers must use explicit
`resume_run(...)` even when no writer lock is active. `RunAlreadyExistsError`
includes the conflicting `run_id` in the message. This lock is an implementation
guard for local native mode, not a durable shared-catalog coordination protocol.

Run-writer lock files live under:

```text
<project>/.pulseon/locks/runs/<percent-encoded-run-id>.lock
```

They are local runtime state and not catalog tables. The encoded run id uses
the same RFC 3986 percent-encoding rule as `metric_key_encoded`. Filesystem I/O,
permission, or disk failures affecting catalog paths, data paths, lock
directories, or lock files raise `StorageError`, not
`InvalidConfigurationError`. Ordinary `StorageError` messages should include
the failed operation and basename, not full local paths by default. Full local
paths are not exposed through ordinary diagnostics in v2 Phase A; they are
reserved for a future explicit debug/verbose facility or internal chained/source
errors intended for debugging.

Initialization-time `StorageError` is fatal to client startup. Runtime
`StorageError` from direct API operations is an operation error and does not
automatically enter failed writer state unless the background writer also
exhausts its persistence retries.

Flushed `metric_points` Parquet is partitioned by `run_id` and
`metric_key_encoded`; `project_id` is not denormalized into the metric point
fact table. Encoded metric key values use reversible percent-encoding.

After run finalization drains queued reports, it records the terminal run state
and then attempts to make inline `metric_points` visible as Parquet. Flush
failure raises `MetricFlushError`, does not roll back the terminal run state,
and must update runtime-only diagnostics. `flush_run_data(run_id)` retries that
Parquet visibility step for a terminal run without changing lifecycle state. It
returns `None` on success, is idempotent when the terminal run is already
Parquet-visible, and rejects running runs with `InvalidRunStateError`. It
supports an optional timeout; if no timeout is supplied, it waits until flush
succeeds or fails, and a bounded timeout raises `MetricFlushTimeoutError`.
Timeout leaves the terminal run unchanged and may be retried. Terminal-run
flush work is serialized inside one client with a client-wide flush mutex;
waiting for that mutex counts against a supplied timeout. Ordinary
`MetricFlushError` messages should include the failed operation and basename,
not full local paths by default. Repeated
`finish_run(...)` or `fail_run(...)` calls are not flush retry APIs.

V2 is still a development release and does not need to preserve compatibility
with earlier development datasets. A schema/version marker is not required for
v2 planning.

## Consequences
- Project-scoped exports depend on catalog `runs(project_id)` metadata rather
  than `project_id=` Parquet directories.
- PostgreSQL catalog support needs a future decision covering shared catalog
  connection configuration, locking, and multi-client behavior.
- SQLite catalog support is explicitly deferred until real DuckLake-backed
  behavior tests prove parity with DuckDB; v2 must not fake backend
  compatibility by matching table names only.
- S3-compatible object storage needs a post-v2 decision covering credentials,
  HTTPFS configuration, MinIO acceptance tests, and secret-safe test setup.
