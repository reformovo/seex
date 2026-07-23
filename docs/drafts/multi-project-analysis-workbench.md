# Multi-Project Analysis Workbench

> Status: design source for Roadmap Phase 3E. The Roadmap defines committed
> delivery scope; unresolved details in this document remain provisional.

## Outcome

Evolve the single-source, single-metric viewer into a local analysis workbench that:

- retains multiple imported local data sources;
- selects Runs across Projects and data sources;
- supports multiple analysis Views in one window; and
- renders one chart panel for every metric selected in the active View.

Preserve the native storage boundary, evidence semantics, public Python API,
catalog and Parquet schemas, and renderer-independent chart model.

## Proposed Product Language

Existing glossary terms remain authoritative: a **Project** contains related
Runs, a **Run** is one training execution, and a **metric key** names a metric
series. Product code must not introduce `Experiment` as a synonym.

This draft proposes three additional terms:

- **Data source**: one imported local native PulseOn store, possibly containing
  multiple Projects.
- **Analysis View**: a named workspace tab containing a Run selection, metric
  panels, alignment settings, and presentation state.
- **Metric panel**: one chart card for one metric key within an Analysis View.

These definitions are recorded without implementation details in `CONTEXT.md`.

## Experience

```text
+ Projects / Runs ------+ [Overview] [Training] [Ablation] [+] --------+
| + Import Source       | Runs: 6 | Metrics: 4 | Step | Refresh        |
|                       +-----------------------------------------------+
| v Project A           | |<==== shared timeline brush ====>|          |
|   [x] baseline        +----------------------+------------------------+
|   [x] candidate       | train/loss           | train/lr               |
|                       | detail chart         | detail chart           |
|                       +----------------------+------------------------+
| v Project B           | train/accuracy       | eval/loss              |
|   [x] control         | detail chart         | detail chart           |
|   [ ] candidate       |                       |                        |
+-----------------------+-----------------------+------------------------+
```

## Zed-Aligned UI Contract

The workbench follows [Zed](https://github.com/zed-industries/zed)'s visual and
interaction language. The initial reference is Zed commit
[`40dc154a`](https://github.com/zed-industries/zed/commit/40dc154a7cc28270d2319873b0881ef053dc22b9),
including its `theme`, `ui`, `title_bar`, `project_panel`, and `workspace`
boundaries. Reference updates must be explicit so a moving upstream `main`
cannot silently change PulseOn's acceptance target.

Zed is also the direct implementation reference, while PulseOn retains a local
component boundary compatible with its pinned GPUI runtime:

- directly adapt relevant Zed component structure, theme tokens, icons, and
  interaction logic where compatible;
- keep the adapted components viewer-owned instead of depending on whole Zed
  application crates whose GPUI revision and dependency graph differ;
- use semantic theme roles instead of feature-level hard-coded RGB values;
- preserve Zed's compact density, typography hierarchy, one-pixel separators,
  restrained elevation, subtle rounded selections, and low-noise surfaces;
- cover inactive, hovered, active, selected, focused, disabled, pending, and
  error states consistently across mouse and keyboard interaction; and
- keep light and dark appearances structurally identical, changing palette
  tokens rather than component layout.

Initial viewer-owned primitives should cover the application chrome, tab bar,
sidebar tree row, toolbar button, icon button, popover, tooltip, status badge,
empty state, and focus ring. Zed's current default-density geometry—roughly
32-pixel tab containers and 28-pixel tree rows—is the starting point, adjusted
only when chart readability or accessibility provides measured evidence.

Visual acceptance compares the shell, tabs, Project tree, toolbar, popovers,
typography, spacing, interaction states, and theme hierarchy side by side with
the pinned reference at representative window sizes and display scale factors.

### Sidebar

- Show a searchable, collapsible Project tree following Zed project-panel row,
  disclosure, hover, selection, focus, and context-menu behavior. Flatten
  healthy sources while retaining source identity for errors and disambiguation.
- Show Runs under each Project with selection, lifecycle status, and a compact
  secondary identifier. Checkboxes reflect the active View's selection.
- Continue to allow at most 10 selected Runs per View.
- Provide Import Source, reveal path, refresh, and remove-from-workbench
  actions. Removing a source must not delete native data.
- Keep unavailable sources visible with a reconnect/error treatment rather
  than silently removing their Projects and selections.

### View Tabs and Toolbar

- Use a compact tab strip with create, activate, rename, duplicate, close, and
  overflow actions.
- Each View owns its name, ordered Runs and metrics, alignment axis, panel
  order, grid density, and chart viewport state.
- A new View starts empty or duplicates the active View explicitly; Views must
  never share mutable selection state implicitly.
- The active toolbar exposes metric selection, Step/Elapsed alignment,
  Refresh, Reset View, grid density, and pending/error status.
- Closing the last View creates a fresh empty View.

### Shared Timeline Brush

- Render one sticky timeline brush below the active View toolbar, spanning the
  full metric-grid width. Metric panels do not render individual brushes.
- The timeline is a navigation ruler with ticks and selection shading, not a
  synthetic aggregation of unrelated metric values.
- Its home range is the union of valid overview extents for the active View's
  selected Runs and metrics. Empty panels do not collapse a valid shared range.
- Dragging either handle resizes the shared viewport; dragging the selection
  pans it. All visible panels reproject against the transient viewport together.
- `Command-+` zooms in, `Command--` zooms out, and `Command-0` resets to the
  home range. Zoom anchors at the pointer's axis position when it is over the
  timeline or a chart, otherwise at the viewport midpoint.
- GPUI actions should expose persistent names such as `ZoomIn`, `ZoomOut`, and
  `ResetView`. Bind both `cmd-=` and `cmd-shift-=` for keyboard layouts that
  produce `+` through Shift, plus `cmd--` for zoom out.
- The selected range may extend through visible panels as subtle vertical edge
  guides, matching timeline tools without obscuring chart evidence.

### Metric Grid

- Render one independently identifiable panel per selected metric in a
  responsive, scrollable grid. Narrow windows fall back to one column without
  shrinking charts below a usable interaction size.
- Each panel keeps its title, evidence legend, detail chart, hover state,
  pending indicator, and panel-level error. Overview evidence still contributes
  to the shared home range but does not require a per-panel overview path.
- Reordering or removing one panel must not invalidate unaffected panels.
- The metric picker shows the selected Runs' metric union. A Run without a
  selected metric remains in that panel's legend as unavailable evidence.
- Only panels in the active View and within one viewport of the visible scroll
  region prepare GPUI geometry or request new detail data.

## Stable Selection Identity

`RunId` alone is insufficient once multiple data sources and Projects are open:

```text
RunRef = DataSourceId + ProjectId + RunId
```

`DataSourceId` is viewer-local, not a native-storage identifier. Series and
requests use the full `RunRef`, so duplicate identifiers cannot collide in
selections, caches, colors, hover results, or stale-result handling. Storage and
model crates retain their existing Project and Run types.

## State Boundaries

```text
Workbench
  imported data sources and source sessions
  ordered Analysis Views and active View identity

Analysis View
  ordered RunRefs and MetricPanels
  alignment, shared brush, pointer anchor, and presentation settings

Metric panel
  overview/detail snapshots and hover state
  generations, pending state, and renderer cache revision
```

Render callbacks consume immutable or plain local snapshots and must not
recursively access the same GPUI entity. Background source events return to the
foreground before changing workbench or panel state.

## Query Coordination

- Keep one native read session, exclusively owned by a background worker, for
  each imported data source.
- Fan a panel request out by source, query that source's Runs, then merge
  immutable evidence snapshots at the viewer boundary.
- Preserve results and errors per source; one failure must not erase drawable
  evidence returned by another source.
- Tag requests with source, View, panel, request kind, and generation. Apply a
  result only while all identities still match current state.
- Coalesce pending work per source and panel. Newer generations replace older
  overview or detail work that has not started.
- Wheel/pinch and keyboard zoom update the shared viewport immediately. One
  View-level trailing timer fans out detail work to visible panels only after
  100 ms without another event, including held-key repeats.
- Running native queries may finish, but stale results cannot replace current
  View or viewport state.
- Inactive Views issue no viewport queries. Off-screen panels may contribute
  their overview extent to the shared home range, but suspend geometry
  preparation and detail refresh work.

Cross-source aggregation belongs to viewer coordination. Native connections
must not cross worker threads or weaken the storage ownership boundary.

## Persistence

Import implies persistence across restarts. Viewer-owned application data may
store source paths, View definitions, composite selections, presentation state,
and safe preferences. It must not store metric points, query snapshots,
credentials, connections, or renderer geometry.

Loading must tolerate unavailable sources, removed Projects or Runs, unknown
metrics, and unsupported state versions without mutating source data. The exact
format, migration policy, and location remain open.

## Performance Contract

- Preserve existing per-series overview and detail point budgets.
- Keep native queries and cross-source merging off the UI thread.
- Prepare visible-panel paths only; cache by panel revision, viewport, canvas,
  and theme.
- Do not prepare duplicate overview paths or query per wheel/key event.
- Bound worker concurrency so sources and panels cannot create unbounded query
  fan-out.
- Measure frames separately from storage latency, then validate a multi-panel View.

## Delivery Slices

1. Add viewer-local data-source and composite selection identities while
   retaining the current single-panel UI.
2. Add retained data sources and the collapsible Project/Run sidebar.
3. Add Analysis View tabs with independent in-memory Run selections.
4. Split chart state into Metric panels and render the responsive metric grid.
5. Add visible-panel scheduling, cross-source merging, and performance gates.
6. Add versioned persistence and unavailable-source recovery.

Each slice uses persistent product names. Roadmap phase identifiers must not
enter source paths, modules, functions, tests, environment variables, or comments.

## Acceptance Scenarios

- Import two stores containing duplicate Project or Run identifiers;
  selections, colors, queries, and hover results remain distinct.
- Select Runs from two Projects and four metrics; receive one panel per metric
  with explicit unavailable evidence where appropriate.
- Switch between Views with different selections; each restores its own brush,
  snapshots, and pending generations.
- Rapidly zoom with wheel, `Command-+`, or `Command--`; the shared viewport and
  visible panels respond immediately while queries stay debounced and coalesced.
- Scroll a large grid; off-screen panels stop preparing geometry and do not
  initiate viewport refreshes.
- Remove or move one source; other sources remain usable and saved selections
  reconcile without modifying native data.
- Restart; imported sources and View definitions return while query snapshots
  rebuild from native storage.

## Open Questions

- What persistence format and application-support path form the compatibility boundary?
