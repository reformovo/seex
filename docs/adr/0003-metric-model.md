# ADR 0003: V1 Metric Model

## Status
Accepted.

## Context
Training metrics need to support many metric keys, long per-metric series, and
efficient local charting. The v1 model should avoid dynamic wide tables and
avoid multi-type values until there is a proven need.

## Decision
V1 uses a numeric long-table metric model.

The minimal metric point schema is:

```text
run_id       string
metric_key   string
step         int64
timestamp    timestamp
value_f64    double
ingested_at  timestamp
```

Rules:
- Metric keys/summaries are write-derived aggregate state, not definitions.
- `phase` is not part of v1. Users should encode grouping in metric keys, such
  as `train/loss` and `val/loss`.
- Only numeric values are supported in `metric_points`.
- Step monotonicity is scoped to `(run_id, metric_key)`.
- Duplicate `(run_id, metric_key, step)` writes are logical last-write-wins by
  internal ingest time. Physical duplicates are allowed only if queries stay
  fast.
- If the user omits `step`, PulseOn may assign the next step for that metric
  series from aggregate state.
- Chart queries sort by `step` and apply last-write-wins by default.
- Aggregates use effective series semantics: count/last/min/max, async repair.

## Consequences
- Non-numeric observations stay outside metric points. Run summaries, range
  selection, and strong `max_points` downsampling are part of v1 query design.
