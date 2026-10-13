# Catalog Application Tables

Seex stores small control-plane and query-index state in catalog application
tables. These tables are not Parquet compatibility contracts and are not
DuckLake logical data tables.

The table names are prefixed with `seex_` because they are owned by Seex.
V3 keeps the names stable but stops addressing them through DuckLake's internal
metadata alias. Backend-specific qualification belongs in the native catalog
adapter, not in query/write call sites. The tables live in the same catalog
database file as DuckLake metadata for local DuckDB and SQLite backends.

## `seex_projects`

Projects are lightweight namespaces for related runs.

| Column | Type | Required | Contract |
| --- | --- | --- | --- |
| `project_id` | string | yes | Stable project identity. |
| `name` | string | yes | User-facing project name. |
| `created_at` | timestamp | yes | Project creation timestamp. |

## `seex_runs`

Runs carry lifecycle state and project ownership. Project-scoped queries and
exports use `seex_runs(project_id)` rather than denormalizing `project_id`
into metric-point Parquet.

| Column | Type | Required | Contract |
| --- | --- | --- | --- |
| `run_id` | string | yes | Stable run identity. |
| `project_id` | string | yes | Owning project identity. |
| `name` | string | yes | User-facing run name. |
| `status` | string | yes | One of `running`, `finished`, or `failed`. |
| `created_at` | timestamp | yes | Run record creation timestamp. |
| `started_at` | timestamp | yes | Training start timestamp. |
| `finished_at` | timestamp | no | Terminal lifecycle timestamp. |

## `seex_metric_aggregates`

Metric aggregates are derived query-index state over all persisted observations.
They may be repaired or rebuilt from persisted `metric_points`.

| Column | Type | Required | Contract |
| --- | --- | --- | --- |
| `run_id` | string | yes | Owning run identity. |
| `metric_key` | string | yes | User-facing metric key. |
| `effective_count` | uint64 | yes | Count of all observations, including repeated Steps; legacy column name retained. |
| `last_step` | int64 | yes | Highest observed Step. |
| `last_value_f64` | float64 | yes | An observed value at `last_step`; choice among equal-Step observations is unspecified. |
| `min_value_f64` | float64 | yes | Minimum observed value. |
| `max_value_f64` | float64 | yes | Maximum observed value. |
