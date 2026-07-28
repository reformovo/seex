# Comparison Semantics

> Status: renderer-agnostic Seex 0.1.x contract. This may change before 1.0 and is not an ADR until 1.0 freezes it.

## Scope

Comparison is a read-only derived layer over Runs and effective metric facts. It adds no stored Run roles, catalog or Parquet fields, renderer policy, research decision, tolerance, significance, or uncertainty claim.
Effective series use [last-write-wins storage semantics](native-storage-boundary.md), and ownership follows the [crate boundaries](crate-boundaries.md).
Ordinary metric queries remain half-open. Aligned metric queries use a closed viewport, retain one strict neighbor on each side when present, and support full or screen-budgeted extrema results. Seex 0.1.x supports only raw step and elapsed wall time as comparison axes; cumulative-token and normalized-budget axes are out of scope.

## Product Language

| Term | Contract |
| --- | --- |
| Comparison axis | The requested mapping from effective metric points to ordered horizontal coordinates. |
| Objective metric | A primary metric key plus a request-scoped `minimize` or `maximize` direction. |
| Comparison evidence | The values, aligned subsets, counts, Run states, and reasons supporting a comparison. |
| Completeness | The four-state assessment `complete`, `partial`, `unavailable`, or `invalid`. |
| Outcome | The numeric objective relationship `improved`, `regressed`, or `equal`. |
| Preference | Compute-only advice: `candidate`, `reference`, `no_preference`, or `inconclusive`. |
| Comparison report | One candidate/reference primary comparison plus ordered secondary metric evidence. |
| Secondary metric | A requested metric reported as supporting evidence without affecting objective outcome or preference. |

Candidate and reference are request roles. Baseline is an explicitly requested reference; incumbent is the reference role in an autoresearch view.
None is a stored Run identity or lifecycle state, and one Run may occupy different roles in different requests.

## Evidence

| Completeness | Meaning |
| --- | --- |
| `complete` | All required evidence is present and valid. |
| `partial` | A usable numeric subset exists, but requested evidence is incomplete. |
| `unavailable` | No usable evidence exists, including a missing metric or Run start. |
| `invalid` | Present evidence violates an axis or numeric constraint. |

Aggregate precedence is `invalid > unavailable > partial > complete`; all reasons are retained.
Reasons distinguish missing data, negative or decreasing axes, non-finite values, and running or failed Runs.
Evidence is never repaired by interpolation, filling, reordering, clamping, or fallback to an earlier finite value.

## Axes

| Axis | Contract |
| --- | --- |
| Raw step | Apply effective-series deduplication, then use ascending `step`; a negative step is invalid. |
| Elapsed wall time | Use integer `timestamp_millis - Run.started_at_millis`, without rebasing to the first point. Equal values are valid; negative or decreasing values are invalid. Missing Run metadata is `missing_run_start`. |

Axis monotonicity is evaluated in effective objective-step order: a decrease is invalid, while equality on the elapsed axis is retained.
Screen reduction preserves first, last, minimum, and maximum candidates within
each bucket after effective-series and viewport selection. Viewport neighbors
are added outside that budget. Elapsed alignment never uses LTTB.

## Scalar Comparison and Examples

Scalar comparison ignores alignment, viewports, and reduction. It uses each objective series' effective value at its greatest step; a non-finite last value is invalid and does not fall back.
Let `raw = candidate - reference` and `relative = raw / abs(reference)` unless the reference is zero, when relative is absent.
Direction-normalized improvement is `raw` for maximize and `-raw` for minimize; its positive, negative, and zero signs yield improved, regressed, and equal without tolerance.

With complete evidence from two finished Runs, those outcomes prefer candidate, reference, and `no_preference`, respectively.
Partial, unavailable, invalid, running, or failed evidence is `inconclusive`; available partial values may still produce a numeric outcome. Secondary metrics never affect outcome or preference.

A generic report uses an explicitly requested reference and preserves candidate
request order. An autoresearch view calls that reference the incumbent, which
is either explicit or selected from an explicit comparator pool. Roles are not
inferred from Project history. Reported secondary metrics preserve request
order and expose last values, raw and relative deltas, completeness, and
reasons, but have no objective direction, normalized improvement, outcome, or
preference. Each metric item owns its completeness; unavailable or invalid
secondary evidence does not downgrade the primary comparison preference.
When an explicit comparator pool contains no eligible incumbent, the
autoresearch view returns unavailable evidence with `no_eligible_incumbent`
and an inconclusive preference. It does not infer another Run or mutate state.

## Ranking and Machine Output

Ranking is scoped to a required Project and optionally to an explicit Run
subset. Only finished Runs with complete, finite objective evidence are
eligible. Direction-aware competition ranks use `1, 1, 3`; exact ties share a
rank, while selecting one best Run prefers earlier `created_at` and then
lexical `run_id`. Ineligible Runs remain visible with a null rank and their
structured evidence reasons. An empty eligible set successfully returns no
best Run.

Leaderboard ranks are computed over the full selected set before limit/offset
pagination. CLI machine output uses JSON schema version 2 with deterministic
kinds and ordering. Non-finite values are represented as JSON strings, and a
relative delta remains null when its reference is zero. Aligned curve points
remain available only through the typed Rust and Python read APIs.

| Scenario | Result |
| --- | --- |
| Steps `0,1,2` / `-1,0,1` | Valid raw-step axis / invalid negative axis. |
| Elapsed `0,100,100` / `0,100,90` | Valid repeated axis / invalid decreasing axis. |
| Negative elapsed | Invalid, with no repair. |
| Zero reference | Raw delta and outcome remain; relative delta is absent. |
| Equal finished values / running or failed Run | `no_preference` / `inconclusive`. |
| Standalone Parquet elapsed axis | Unavailable with `missing_run_start`; the first point is not an origin. |
