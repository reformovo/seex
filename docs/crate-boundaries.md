# Crate Boundaries

PulseOn workspace crates follow one dependency direction:

```text
pulseon-model <- pulseon-storage <- pulseon-core <- pulseon-python
       ^                 ^                ^
       +---------- pulseon-viewer --------+----> pulseon-chart-core
```

`pulseon-viewer` is the desktop composition root and may depend directly on the
model, storage, core, and chart crates. Its `WorkbenchSession` owns native read
coordination and immutable snapshots; independent GPUI entities own the
sidebar, View bar, analysis workspace, inspector, and charts. All reverse
dependencies, mutual entity subscriptions, and crate cycles are forbidden.

## Responsibilities

- **`pulseon-model`** owns projects, runs, metrics, typed identities, query
  inputs, reduction policies, and query results. It has no storage, Python, or
  rendering dependencies.
- **`pulseon-storage`** owns project configuration, DuckDB/DuckLake catalogs,
  schema bootstrap and validation, encoding, reads, writes, aggregate repair,
  flush, S3 setup, and storage errors. It exposes a narrow metric-reader
  interface implemented by the native project store and Parquet dataset reader.
- **`pulseon-core`** owns client and run lifecycle, report admission, the
  background queue, drain and finalization orchestration, shutdown, diagnostics,
  and comparison use cases. It contains no SQL or Python bindings.
- **`pulseon-python`** owns the PyO3 extension, Python classes and exceptions,
  Arrow capsules, argument conversion, and error mapping. It contains no
  product or storage policy.
- **`pulseon-chart-core`** owns renderer-independent chart series, viewports,
  scales, projected paths, hit testing, and interaction state. Its generic
  chart points intentionally remain distinct from metric points.
- **`pulseon-viewer`** owns source selection, background query scheduling,
  conversion from metric points to chart points, GPUI state, and rendering.

## Shared Query Contract

Both metric readers apply half-open step filtering and last-write-wins effective
series semantics before optional reduction. The storage crate owns full-series,
LTTB, and screen-budgeted extrema query strategies; chart code projects every
point returned by storage and does not downsample again.

Aligned queries are separate from ordinary half-open step queries. They derive
raw-step or elapsed-time coordinates after last-write-wins, use a closed
viewport plus one strict neighbor on each side, and expose full or
screen-budgeted extrema results through the shared metric-reader interface.
The native reader resolves elapsed origin from Run metadata. The standalone
Parquet reader remains fact-only: it supports step alignment and reports
`missing_run_start` for elapsed alignment.

The native project store is the authoritative read source for catalog discovery
and inline plus Parquet-backed facts. A standalone Parquet dataset is a
compatibility source for flushed facts only and does not imply project or run
discovery.

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
It also moved shared GPUI fixtures out of production modules and rejects every
workbench document version except `pulseon-workbench 3`.

At the refactor baseline the Viewer contained 16,932 Rust source lines, with
tests embedded in the 8,905-line `desktop.rs`. The resulting source tree has
19,731 lines split into 15,017 production lines and 4,714 dedicated test/support
lines. `desktop.rs` is 109 lines, `desktop/app.rs` is 410 lines, and the largest
Desktop production module is 1,065 lines. The production-line increase records
the explicit Entity, command, snapshot, and component boundaries; no legacy UI
implementation remains beside them.
