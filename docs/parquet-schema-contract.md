# Seex Parquet Schema Contract

Seex treats the product-owned `metric_points` Parquet data shape as the
native compatibility boundary. DuckLake catalog metadata, inline data,
temporary files, indexes, query summaries, and extension state are
implementation details.

Catalog application tables such as projects, runs, and metric aggregates are
not part of the Parquet compatibility boundary. Export or migration code that
needs run ownership must join through catalog metadata before writing its own
external format.

## Compatibility Rules

- Additive nullable columns are compatible.
- Removing columns, renaming columns, changing column types, or changing primary
  identity semantics is incompatible.
- Readers must ignore unknown compatible columns.
- Writers must preserve the long metric-point table shape for numeric metrics.
- Native v5 metric queries preserve every observation, including repeated
  `(run_id, metric_key, step)` tuples. Step order is ascending; equal-Step order
  is unspecified. `ingested_at` does not select winners. The physical columns
  are unchanged from v3, but the semantic change is incompatible: older Native
  stores are rejected without migration. See [ADR 0019](adr/0019-append-only-metric-observations.md).
- `metric_key_encoded` is an additive a2 partition column. It must remain a
  reversible RFC 3986 percent-encoding of `metric_key` for partition discovery.

## `metric_points`

| Column | Type | Required | Contract |
| --- | --- | --- | --- |
| `run_id` | string | yes | Owning run identity. |
| `metric_key` | string | yes | User-facing metric key. |
| `metric_key_encoded` | string | yes | Reversible encoded metric key used for Parquet partitioning. |
| `step` | int64 | yes | Metric step. |
| `timestamp` | timestamp | yes | Metric observation timestamp captured when `run.log(...)` enters the enqueue path in 0.2 and later. Earlier writer-time values remain best-effort elapsed evidence. |
| `value_f64` | float64 | yes | Numeric metric value. |
| `ingested_at` | timestamp | yes | Seex ingestion timestamp. |

## Partition Layout

Flushed metric points are partitioned by `run_id` and `metric_key_encoded`:

```text
data/main/metric_points/
  run_id=<run_id>/
    metric_key_encoded=<encoded_metric_key>/
      ducklake-*.parquet
```

DuckLake may apply additional Hive-style escaping to physical directory names.
The logical `metric_key_encoded` column value remains the Seex contract.
