---
status: accepted
---

# Unify the public Rust SDK without hiding performance changes

The fixed three/seven-pair and permanent-original-baseline validation policy in
this ADR is superseded by
[ADR 0016](0016-lightweight-performance-validation.md). The SDK architecture,
migration order, query decisions, and requirement to measure hot-path changes
remain accepted.

Seex will publish one Rust crate named `seex`. The Desktop, Python extension,
and plot implementation remain private workspace crates, while model, storage,
and engine become modules behind the public facade. This replaces Cargo
packages with module visibility, API tests, and review gates as the internal
boundary because a single installable Rust SDK is more useful than publishing
the current implementation split.

ADR 0013's dependency direction remains mandatory:
`model -> storage -> engine -> facade`. The decision here supersedes only its
requirement that those layers remain separate Cargo packages. DuckDB, DuckLake,
PyO3, GPUI, storage errors, and plot types must not leak through the public
facade. DuckLake remains required in native mode, and the catalog and Parquet
schema contracts do not change in this migration.

## Migration order

The migration is performance preserving and stays buildable after every step:

1. Freeze versioned reporting, query, and Viewer baselines.
2. Add an unpublished `seex` facade and the public `Reader`; migrate Desktop
   reads before moving storage source.
3. Move model and storage mechanically, then optimize narrow queries as
   separate candidates.
4. Move the engine and add the Rust Run SDK, followed by the Python Run and
   read-only API.
5. Remove temporary re-export crates, rename the private plot crate, verify
   packaging, and run the final resource and Metal gates.

Source moves and behavior changes must not share a candidate. A mechanical
migration is accepted only when correctness passes and every reliable protected
metric regresses by no more than 3%. An optimization additionally requires 7
alternating baseline/candidate pairs, at least 6 improvements, a median primary
improvement of at least 5%, protected-metric regression of at most 3%, and
relative MAD of at most 2%. The original baseline is permanent; the last
accepted candidate becomes the rolling baseline.

## Query decision

Full-span and narrow queries deliberately use different execution plans. The
incumbent full-span plan remains protected. Narrow Step queries may filter the
viewport before materialization and reduction, but must preserve all same-step
replacements until last-write-wins and return real boundary neighbors. Elapsed
and timestamp queries initially retain the incumbent plan because replacements
can change timestamps.

Whole-series diagnostics are computed outside viewport selection and cached
with explicit storage-generation invalidation. A superseded Viewer request may
interrupt its cloned DuckDB connection only while its request token is current;
interruption is stale cancellation, never an error snapshot. If narrow SQL
cannot improve real RSS, the next candidate is a narrow-only bounded reducer,
then physical Parquet ordering or row-group sizing without a schema change.

## Consequences

The weaker compile-time isolation inside `seex` is an accepted cost. Public API
tests, package-content tests, module visibility, and the migration gates become
the enforcement mechanism. `seex-python`, `seex-app`, and `seex-plot` remain
unpublished, and the final package must contain no path dependency or PyO3,
GPUI, or plot source.

Allocator relief tuning, DuckDB memory/thread caps, generic hash-group extrema,
and removing the beneficial full-span ordered window remain rejected because
measurement did not identify them as solutions to the window/sort working-set
bottleneck. A rejected or inconclusive candidate must be profiled before a new
optimization direction begins.
