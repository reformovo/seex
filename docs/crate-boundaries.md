# Crate and Module Boundaries

Seex has one published Rust SDK and three private product crates:

```text
seex (published)
  facade -> private engine -> private storage -> private model
       ^              Reader owns all consumer reads
       +---------------------------------------------+

seex-python (private) ----> seex
seex-app (private) -------> seex + seex-plot
seex-plot (private) ------> no SDK, storage, PyO3, or GPUI dependency
```

Arrows point from a consumer to what it may use. Model does not depend on
storage; storage does not depend on engine; engine does not depend on the
facade. Reverse dependencies, cycles, and storage implementation types in a
consumer API are forbidden. See [ADR 0015](adr/0015-unified-rust-sdk-performance-preserving-migration.md)
and the accepted [SDK design](single-crate-rust-sdk.md).

## Responsibilities

- **The `seex` root facade** owns stable `Client`, `RunHandle`, `Reader`,
  options, public errors, and common product exports. Its private model module
  owns identities, metric evidence, query inputs, and comparison types.
- **The private `seex` storage module** owns DuckDB/DuckLake, catalog and Parquet
  I/O, schema validation, reduction, and storage errors.
- **The private `seex` engine module** owns Run lifecycle, atomic report admission,
  the bounded writer queue, finalization, diagnostics, comparison, and ranking.
- **`seex-python`** maps the facade to the typed Python Run writing surface.
  It contains no product, query, or storage policy.
- **`seex-plot`** owns renderer-independent geometry and interaction. It remains
  independent of storage and GPUI.
- **`seex-app`** composes the facade, plot crate, and GPUI. It derives private
  viewport query options but never depends on DuckDB or storage types.

## Packaging Boundary

Only `seex` is publishable. `seex-python`, `seex-app`, and `seex-plot` set
`publish = false`; the PyO3 extension also opts out of rustdoc because its lib
name intentionally matches the Python module. `cargo package -p seex` must
contain no path dependency and no Python, Desktop, plot, PyO3, or GPUI source.
[`package_smoke.py`](../scripts/package_smoke.py) checks both packaged manifests,
extracts the archive into an independent temporary directory, and runs locked
all-feature tests there.

## Reader Query Contract

The public Reader accepts Step ranges plus a required strict `max_points` from
2 through `MAX_CHART_POINTS` (1024); it never accepts pixels or an unbounded
query. Every implementation applies last-write-wins before reduction and
returns real samples with source count, downsampled state, completeness, and
reasons. Relative-time and timestamp chart entry points are rejected in P0.

Desktop converts its closed viewport into crate-private options while preserving
the 1024 point bound. Step coordinates are derived after last-write-wins.

Full-span and narrow queries use distinct physical plans. The protected
full-span plan remains unchanged while narrow Step selection can filter before
materialization and reduction without discarding replacements. Whole-series
diagnostics are separate from viewport selection. Superseded requests may
interrupt only the cloned connection and current request token that own them;
stale results never enter a Viewer snapshot.

The native project store is the authoritative read source for catalog discovery
and inline plus Parquet-backed facts.

## Viewer Entity Boundary

The macOS viewer follows the Entity ownership pattern used by Zed at commit
`40dc154a7cc28270d2319873b0881ef053dc22b9`:

```text
ViewerApp
  ├─ WorkbenchSession       native reads, views, revisions, persistence
  ├─ WorkbenchInteraction   emphasized Run and hover/locked cursors
  ├─ ProjectSidebar         filter, expansion, paging, menus, width
  ├─ AnalysisViewBar        View menu and rename input
  ├─ AnalysisWorkspace      brush/ruler, track schedule, chart entities
  └─ BottomInspector        sort, columns, scrolling, height
```

Children emit typed commands or events. `ViewerApp` routes them after the
emitting update completes; `WorkbenchSession` publishes immutable revisioned
snapshots; the root then synchronizes child snapshots in one direction. Render
and list callbacks consume plain snapshots and never begin reads, mutate the
Session, or re-enter their owning Entity. Overview and detail projection caches
belong to their chart Entities.

The refactor removed the root-owned navigation/error swap, chart adapter maps,
root chart caches, root repaint flags, `TrackDensity`, receiver convenience
polling, duplicate visible-Run filtering, and the permanent local-error banner.
It also moved shared GPUI fixtures out of production modules. Workbench state
uses only the current schema-v1 TOML document.

At the refactor baseline the Viewer contained 16,932 Rust source lines, with
tests embedded in the 8,905-line `desktop.rs`. The resulting source tree has
19,731 lines split into 15,017 production lines and 4,714 dedicated test/support
lines. `desktop.rs` is 109 lines, `desktop/app.rs` is 410 lines, and the largest
Desktop production module is 1,065 lines. The production-line increase records
the explicit Entity, command, snapshot, and component boundaries; no legacy UI
implementation remains beside them.
