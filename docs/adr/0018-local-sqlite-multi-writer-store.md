---
status: accepted
---

# Use one local SQLite catalog for concurrent Run writers

The first-release Native store uses one SQLite WAL catalog with DuckLake-managed local Parquet data. Multiple processes may write different Runs while Readers observe committed snapshots; a Run retains one advisory-locked writer. Catalog writes use a short cross-process lock and fresh DuckLake writer connections because another process's inline flush can invalidate an open connection's catalog snapshot. The Python and Rust configuration surfaces reject DuckDB catalog selection and S3 data paths. This narrows the earlier portability and object-storage decisions in ADR 0008 and ADR 0009 to make local crash recovery testable before scale work.

New stores carry a `format_version = 1` marker. Existing stores without a marker or with another version fail on read and write open. A completed Run's inline flush is idempotent and can be retried after a failed finalization or process restart; ordinary writer open does not flush because even an empty DuckLake flush can advance snapshots used by concurrent clients.
