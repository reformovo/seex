# ADR 0019: Append-only metric observations

## Status

Accepted. Supersedes the duplicate-Step replacement portions of ADRs 0003,
0005, 0007 and 0015, and older Reader/comparison descriptions. Historical
release notes and performance evidence describe their original versions.

## Decision

Every persisted report is an independent observation. Equal
`(run_id, metric_key, step)` tuples remain separate observations. Queries order
by ascending Step; equal-Step order is unspecified. Neither `ingested_at` nor
physical position selects a replacement winner. No durable write-order
allocator or business deduplication compaction is introduced.

Summaries count all observations and compute extrema over all their values.
The legacy `effective_count` field name remains, with observation-count meaning.
`last_value_f64` is an unspecified observation at the maximum Step. Existing
non-finite-value diagnostics and catalog representation rules remain in force.

Reduction may select a bounded subset, but returns actual observations without
fabricating payloads or collapsing equal Steps into a winner. Query-local row
ordinals may identify selected observations; they are not persisted identities
or ordering promises. Every chart remains bounded to 1024 observations and a
panel to 16 curves. Range and enqueue-versus-persistence contracts are unchanged.

Native format v5 marks the semantic change; v4 remains reserved for the archived
stable-LWW baseline. Reject v1–v4 and missing markers without migration or data
rewrites. The v3 Parquet columns remain unchanged. DuckLake retains ownership of
facts, snapshots and physical file inventory; SQLite WAL retains application
metadata. Local multi-writer coordination and Run ownership remain unchanged.

## Consequences

Repeated reports contribute separately to counts and extrema. Consumers must
not interpret repeated Steps as corrections or infer which equal-Step report
was last. Training-step uniqueness is not enforced at admission.

Delete LWW first as a correctness change. Establish a new append-only baseline
before claiming performance improvements; a comparison with old winner queries
is not a semantics-preserving optimization. Measure reporting, persistence,
finish and complete Reader requests independently. Rollup, production layout
switches and incremental business compaction are outside this change.

## Update (2026-10-22): query-time series diagnostics removed

The non-finite-value diagnostics mentioned above were implemented as a per-curve
`lag` window over the whole Run and have since been removed. Sampled queries
filter `isfinite(value_f64)` in SQL before native LTTB, and curve completeness
now follows Run lifecycle only (`Finished -> Complete`; `Running` and `Failed`
-> `Partial` with a lifecycle reason). A missing metric is a lifecycle-complete
series with zero points, and non-finite observations are no longer returned by
sampled queries or used to mark a curve invalid.

Append-only semantics, duplicate observations, summaries counting every
observation, and DuckLake/catalog ownership are unchanged. If data-quality
flags are needed again, maintain them at write/flush time (for example in the
catalog or summaries) rather than scanning the whole Run per query; that
requires its own proposal, migration story, and measurement.
