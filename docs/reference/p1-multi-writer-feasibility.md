# P1 SQLite/DuckLake multi-process feasibility

Status: P1 correctness acceptance passes through `82601a0`.
These probes ran on
Darwin 27.0.0 arm64 with Rust 1.97.1. They use one local SQLite WAL catalog, four child processes
writing distinct Runs, and a Reader opened before the writes. Each child writes 32 points.

## Observations

| Probe | Result |
| --- | --- |
| Four clients open together | Some `Client::open()` calls failed with public `Storage`. |
| Open clients one by one, then write together with the normal 8192-row inline limit | `finish()` failed with DuckDB `database is locked` and `ducklake_inlined_data_1_1 does not exist`. |
| Open clients one by one, then serialize each whole Run lifecycle under a test file lock | A later `finish()` still failed because its already-open connection referenced the removed inline table. |
| Same lock and already-open clients, temporarily set inline limit to zero | One four-writer run passed; the Reader saw all 32 points of each finished Run. |
| Inline limit zero, remove the test lock | Concurrent `finish()` calls again failed with `Storage`. |
| Normal inline limit, acquire the test lock before opening each client and hold through shutdown | One four-writer run passed; the continuously open Reader saw all finished Runs and their points. |
| Normal inline limit, catalog lock plus fresh writer/read connections | Four independently opened clients passed concurrently three times; Reader saw 32 points and a 32-point summary for every finished Run. |

The final passing case is reproducible with:

```text
cargo test -p seex --test p1_multi_writer_feasibility \
  serialized_lifecycles_keep_reader_consistent -- --ignored --nocapture
```

The earlier test lock covers the *entire* client lifecycle. It prevents simultaneous training writers and
does not satisfy P1. The zero-inline probes temporarily changed `data_inlining_row_limit` in
`storage/bootstrap.rs` and restored 8192 afterward. Neither probe establishes crash recovery or
bounded lock hold time.

## Coordination design to validate

Use one advisory OS lock per canonical catalog path for all DuckLake/SQLite write transactions:
bootstrap, Project/Run metadata, background metric batches, aggregate publication, and inline flush.
Keep the per-Run writer lock. `run.log()` only admits to its bounded queue; the background worker
waits for the catalog lock. Lock timeout and queue full need distinct typed errors. Establish one
lock order for Project creation, Run ownership, and catalog writes.

An already-open DuckDB connection can retain an obsolete inline-table reference after another
process flushes. The first correctness implementation should acquire the catalog lock, open a fresh
existing-catalog writer connection, perform one bounded write operation, and release both. Connection
reuse needs a separately proven invalidation mechanism. Reader requests must retry a stale inline
failure with a new read connection and must not publish a partial chart. The catalog remains the
only authority for committed facts and file metadata.

Finalization drains the client's admitted queue before taking the catalog lock. Aggregate publication
and terminal status update need one catalog transaction, so a Reader cannot see partial aggregates
for a finished Run. Inline flush follows under the lock; a crash after terminal commit still needs
the existing idempotent flush retry. Measure lock hold time: DuckLake's flush is table-wide, so a
different coordination design may be required.

## P1 gate complete

The four-client write/read snapshot is covered by
`simultaneous_writers_keep_reader_consistent`. The
`crashed_writer_can_restart_and_finish_the_same_run` probe drains a child process, terminates it
without running destructors, reopens the SQLite store, verifies the running Run and persisted point,
resumes the same Run, and finishes it. A unit probe reopens after a terminal commit followed by a
forced flush failure and explicitly retries the idempotent DuckLake inline flush. Missing storage
format markers are rejected by both read-only and read-write opens.

Run-writer conflicts, catalog lock timeout versus queue pressure, four-process opening, concurrent
Reader snapshots, and completed Run flush retries remain covered by the existing acceptance tests.
The first release store is SQLite catalog plus local DuckLake data; S3 and the DuckDB catalog
backend are rejected at configuration resolution.

## Verification and performance observation

The workspace Rust tests, Python tests (99 passed), Pyright, Ruff, wheel build,
package smoke, and Viewer release build passed after the P1 changes. The
freshly built release Viewer passed `scripts/viewer_smoke.py` in global and
Project scopes. `uv run python scripts/viewer_smoke.py` could not read the uv
cache in the sandbox; `.venv/bin/python scripts/viewer_smoke.py` ran the same
script successfully.

One bounded A-B-B-A preservation comparison used the P0 revision `4da28dc`
and the P1 revision `82601a0`, with 1,000 reports in each of 30 Runs per
capture. It finished within 120 seconds, with ten calibrated samples of at
least 25 ms per metric. The result was **inconclusive** because baseline
finalization noise exceeded the 2% limit. Median drain samples were 148/147 ms
before and 379/379 ms after; median finalization samples were 102/108 ms
before and 621/628 ms after. The P1 implementation therefore carries a
substantial observed local write cost, even though this comparison cannot
classify a reliable regression under the gate's noise rule. The manifest and
raw result remain outside the repository at `/tmp/seex-p1-durability-*.json`.
