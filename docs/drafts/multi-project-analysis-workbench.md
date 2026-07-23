# Multi-Project Analysis Workbench

> Status: design source for Roadmap Phase 3E. The Roadmap defines committed
> delivery scope; unresolved details in this document remain provisional.

## Outcome

Evolve the single-source, single-metric viewer into a local analysis workbench that:

- retains multiple imported local data sources;
- selects Runs across Projects and data sources;
- supports multiple analysis Views in one window; and
- renders one aligned chart track for every metric selected in the active View.

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
- **Metric panel**: one aligned chart track for one metric key within an
  Analysis View.

These definitions are recorded without implementation details in `CONTEXT.md`.

## Experience

```text
+ Project / Run sidebar + Analysis View tabs / toolbar ----------------+
| + Import Source       + Metric sidebar ----+ Shared timeline / brush |
| v Project A           | train/loss         | loss chart track        |
|   [x] baseline        | train/lr           | lr chart track          |
|   [x] candidate       | eval/loss          | eval chart track        |
| v Project B           +--------------------+-------------------------+
|   [x] control         | Summary | Ranking | Evidence                 |
|   [ ] candidate       | selected metric details                     |
+-----------------------+----------------------------------------------+
```

The Project/Run sidebar is an independent application-shell region. The View
bar, Metric sidebar, chart tracks, and Bottom inspector are nested inside the
Analysis workspace to its right:

```text
Application shell
  Project/Run sidebar
  Analysis workspace
    View tab bar and toolbar
    Track workspace
      Metric sidebar
      shared timeline and chart tracks
    Bottom inspector
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

### Project and Run Sidebar

- Show a searchable, collapsible Project tree following Zed project-panel row,
  disclosure, hover, selection, focus, and context-menu behavior. Flatten
  healthy sources while retaining source identity for errors and disambiguation.
- Keep this sidebar independent of the Analysis workspace and full-height below
  any native window title bar. Hiding it expands the complete Analysis
  workspace rather than only the chart area.
- Show Runs under each Project with selection, lifecycle status, and a compact
  secondary identifier. Checkboxes reflect the active View's selection.
- Continue to allow at most 10 selected Runs per View.
- Provide Import Source, reveal path, refresh, and remove-from-workbench
  actions. Removing a source must not delete native data.
- Keep unavailable sources visible with a reconnect/error treatment rather
  than silently removing their Projects and selections.

### Analysis Workspace and View Bar

- Place the View tab strip and toolbar at the top of the Analysis workspace
  only; they do not extend across the independent Project/Run sidebar. Provide
  create, activate, rename, duplicate, close, and overflow actions.
- Each View owns its name, ordered Runs and metrics, alignment axis, panel
  order, track density, and chart viewport state.
- A new View starts empty or duplicates the active View explicitly; Views must
  never share mutable selection state implicitly.
- The active toolbar exposes metric selection, Step/Elapsed alignment,
  Refresh, Reset View, track density, and pending/error status.
- Closing the last View creates a fresh empty View.

### Metric Sidebar and Chart Tracks

- Place a second sidebar inside the Analysis workspace below the View bar. It
  lists selected metrics and compact availability/status information.
- Align every Metric sidebar row with exactly one chart track. The sidebar and
  chart column share vertical scrolling and row heights; horizontal pan and
  zoom affect only the chart column.
- Selecting either a Metric row or its chart track selects the same Metric
  panel. Reordering or removing a row must not invalidate unaffected tracks.
- The metric picker shows the selected Runs' metric union. A Run without a
  selected metric remains in that track's legend as unavailable evidence.
- The Metric sidebar is resizable and may collapse to a compact presentation
  without clearing the View's metric selection.
- Only tracks in the active View and within one viewport of the visible scroll
  region prepare GPUI geometry or request new detail data.

### Shared Timeline Brush

- Render one sticky timeline brush below the active View toolbar, spanning the
  chart-track column rather than the Project or Metric sidebars. Metric tracks
  do not render individual brushes.
- The timeline is a navigation ruler with ticks and selection shading, not a
  synthetic aggregation of unrelated metric values.
- Its home range is the union of valid overview extents for the active View's
  selected Runs and metrics. Empty panels do not collapse a valid shared range.
- Dragging either handle resizes the shared viewport; dragging the selection
  pans it. All visible tracks reproject against the transient viewport together.
- `Command-+` zooms in, `Command--` zooms out, and `Command-0` resets to the
  home range. Zoom anchors at the pointer's axis position when it is over the
  timeline or a chart, otherwise at the viewport midpoint.
- GPUI actions should expose persistent names such as `ZoomIn`, `ZoomOut`, and
  `ResetView`. Bind both `cmd-=` and `cmd-shift-=` for keyboard layouts that
  produce `+` through Shift, plus `cmd--` for zoom out.
- The selected range may extend through visible tracks as subtle vertical edge
  guides, matching timeline tools without obscuring chart evidence.

### Bottom Inspector

- Place a resizable Bottom inspector below the Track workspace and entirely
  inside the Analysis workspace. It spans the Metric sidebar and chart column
  but never extends under the independent Project/Run sidebar.
- A click without a drag on a Metric row or chart track selects that Metric and
  reveals the inspector. Chart pan, brush drag, wheel, and pinch gestures must
  not reveal or toggle it accidentally.
- Provide `Summary`, `Ranking`, and `Evidence` tabs. Preserve the selected
  Metric and inspector tab when the inspector is hidden or a View is inactive.
- `Summary` uses the same whole-effective-series Metric summary as Python
  `query_metric_summaries` and CLI `metrics list`: effective count, last step,
  last value, minimum, and maximum per Run. It has no viewer-only mean and does
  not derive values from viewport or reduced renderer points.
- `Ranking` requires an explicit minimize/maximize direction. It uses existing
  Core `rank_runs` semantics within each Project: only complete finite Objective
  evidence is eligible, ties receive competition ranks, and stable ordering
  uses Run creation time then Run ID. A cross-Project View shows grouped Project
  rankings rather than inventing one global rank.
- `Evidence` reports the same Objective evidence used by ranking and comparison:
  Run status, last step/value, completeness, and structured reasons, plus
  Project and Data source identity. It does not present chart reduction metadata
  as Objective evidence.
- Chart tracks remain the viewport-scoped surface corresponding to metric point
  queries. Comparison is a separate interaction that must collect explicit
  candidate, baseline/reference, direction, and secondary metric inputs; the
  Evidence tab must not imply that it performed `metrics compare` or
  `autoresearch compare`.
- Expose persistent actions such as `ToggleProjectSidebar`,
  `ToggleMetricSidebar`, `ToggleBottomInspector`, and `ShowMetricInspector`.
  Project sidebar and Bottom inspector visibility are independent.

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
  Project sidebar visibility and width
  Bottom inspector visibility and height

Analysis View
  ordered RunRefs and MetricPanels
  alignment, shared brush, pointer anchor, and presentation settings
  selected Metric panel and inspector tab

Metric panel
  overview/detail snapshots and hover state
  detail/summary/ranking generations, pending state, and renderer cache revision
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
  View-level trailing timer fans out detail work to visible tracks only after
  100 ms without another event, including held-key repeats.
- Running native queries may finish, but stale results cannot replace current
  View or viewport state.
- Inactive Views issue no viewport queries. Off-screen panels may contribute
  their overview extent to the shared home range, but suspend geometry
  preparation and detail refresh work.
- Summary, Ranking, and Evidence use whole-series background Core queries, never
  reduced chart points. Tag results with source, View, Metric panel, direction,
  and generation so stale inspector results cannot replace current selection.
  Pan, brush, and zoom refresh chart detail only; they do not re-query the
  whole-series inspector.

Cross-source aggregation belongs to viewer coordination. Native connections
must not cross worker threads or weaken the storage ownership boundary.

## Persistence

Import implies persistence across restarts. Viewer-owned application data may
store source paths, View definitions, composite selections, selected Metric and
inspector tab, dock visibility and dimensions, presentation state, and safe
preferences. It must not store metric points, query snapshots, credentials,
connections, or renderer geometry.

Loading tolerates unavailable sources, removed Projects or Runs, and unknown
metrics without mutating source data. The viewer owns a dependency-free,
length-safe hexadecimal text document headed by `pulseon-workbench 1`, stored
at `~/Library/Application Support/PulseOn Viewer/workbench.state` on macOS.
`PULSEON_VIEWER_WORKBENCH_PATH` overrides the location for controlled testing.
Writes replace a temporary sibling atomically. Unsupported versions are
reported and left untouched; v1 has no implicit migration path.

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
4. Split chart state into Metric panels and render aligned Metric/sidebar tracks.
5. Add the exact Summary/Ranking/Evidence Bottom inspector and dock controls.
6. Add visible-track scheduling, cross-source merging, and performance gates.
7. Add versioned persistence and unavailable-source recovery.

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
  visible tracks respond immediately while queries stay debounced and coalesced.
- Scroll many tracks; Metric rows remain aligned while off-screen tracks stop
  preparing geometry and do not initiate viewport refreshes.
- Select a chart without dragging; the Bottom inspector opens with the same
  whole-series Metric summary and Objective evidence as the public read
  surfaces. Pan the same chart; the inspector neither toggles nor re-queries.
- Hide Project sidebar and Bottom inspector independently; the Analysis
  workspace reflows and both regions restore their previous dimensions.
- Rank a cross-Project selection; results remain grouped by Project and retain
  the explicitly selected objective direction.
- Remove or move one source; other sources remain usable and saved selections
  reconcile without modifying native data.
- Restart; imported sources and View definitions return while query snapshots
  rebuild from native storage.

## Open Questions

- What explicit migration command should be introduced if a future workbench
  document version cannot be read losslessly?
