# ADR 0009: V4 S3 Data Path

## Status
Accepted.

V4 supports S3-compatible object storage only for DuckLake `data_path`; the
catalog database remains local. S3 configuration lives in
`<project>/.pulseon/config.toml`, and explicit `pulseon.init(...)` keywords
override config-file values. This preserves the native storage boundary while
allowing large Parquet metric data to live in object storage.

## Consequences

- `data_path = "s3://bucket/prefix"` works with both DuckDB and SQLite catalog
  backends; `catalog_path = "s3://..."` remains invalid.
- The TOML config can include `endpoint`, `access_key_id`,
  `secret_access_key`, optional `session_token`, optional `region`,
  `path_style`, and `use_ssl`.
- S3 secrets configure only the current DuckDB connection and must not be
  persisted into DuckLake or PulseOn catalog tables.
- V4 requires explicit credentials in config. AWS credential-chain and
  environment-variable discovery are later work.
- S3 behavior must match local data-path behavior at the PulseOn API level:
  metric writes, queries, terminal finalization, terminal Parquet visibility,
  and `flush_run_data(run_id)` retry semantics remain the same.
- MinIO is the required S3-compatible acceptance path. The acceptance test is
  opt-in so the default local test suite does not require an object-storage
  service.
