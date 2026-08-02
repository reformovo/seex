# ADR 0008: V3 Catalog Backend Portability

## Status
Accepted.

V3 promotes SQLite from a deferred catalog backend to a required native
backend, while keeping PostgreSQL deferred. PulseOn-owned catalog application
tables must live in the same catalog database file as DuckLake metadata, but
they must not be addressed through DuckLake's internal metadata alias. V3 should
own their placement through a backend-aware catalog adapter so the table names
and product meaning stay stable across DuckDB and SQLite. Explicit
`catalog_path` values are used as provided rather than validated by filename
suffix. The alpha line does not promise forward compatibility for existing a2
stores, so V3 may require a fresh local store instead of shipping migration
code.

## Consequences

- `catalog_backend="sqlite"` becomes part of the V3 acceptance gate only after
  real DuckLake-backed parity tests pass.
- DuckDB remains the default backend for `pulseon.init()` and
  `catalog_backend="duckdb"`.
- Catalog application tables remain `pulseon_projects`, `pulseon_runs`, and
  `pulseon_metric_aggregates`, but code must stop hard-coding
  `__ducklake_metadata_dl.pulseon_*`.
- V3 SQLite parity covers the current single-client local workflow; multi-client
  lock behavior can be expanded in V4.
- V3 implementation may break a2 development stores; release notes must call
  this out rather than adding migration logic.
