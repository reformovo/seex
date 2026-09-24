# P1 SQLite/DuckLake multi-process feasibility

Status: investigation; the P1 multi-writer gate is **not met**. These probes ran at `4da28dc` on
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

The final passing case is reproducible with:

```text
cargo test -p seex --test p1_multi_writer_feasibility \
  serialized_lifecycles_keep_reader_consistent -- --ignored --nocapture
```

The test lock covers the *entire* client lifecycle. It prevents simultaneous training writers and
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

## P1 gate still required

Prove four concurrently active clients can report to distinct Runs while a Reader queries them.
Verify per-query snapshots, lock timeout versus queue full, same-Run conflict, process kill at each
commit/flush boundary, restart, and terminal flush retry. Compare the affected writer/reader boundary
with one bounded A-B-B-A workload. Keep P2–P6 paused and retain other catalog and S3 paths until
these checks pass.
