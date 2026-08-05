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

- **Data source**: one imported local native Seex store, possibly containing
  multiple Projects.
- **Analysis View**: a named workspace tab containing a Run selection, metric
  panels, alignment settings, and presentation state.
- **Metric panel**: one aligned chart track for one metric key within an
  Analysis View.

These definitions are recorded without implementation details in `CONTEXT.md`.

## Experience

```text
+ Project / Run sidebar + Analysis workspace --------------------------+
| Seex   + import    | View tabs ...              inspector/refresh |
| Projects              + axis/+ Metric + selected-metric global brush |
| v Project A           |          scrollable Step or Time ruler       |
|   [x] baseline        + metric label + linked viewport chart         |
|   [x] candidate       | metric label + linked viewport chart         |
| v Project B           | metric label + linked viewport chart         |
|   [x] control         +----------------------------------------------+
|   [ ] candidate       | Summary | Ranking | Evidence                 |
+-----------------------+----------------------------------------------+
```

There is no separate global viewer title bar. The Project/Run sidebar is an
independent application-shell region. The View bar, global brush, viewport
ruler, metric tracks, and Bottom inspector are nested inside the Analysis
workspace to its right:

```text
Application shell
  Project/Run sidebar
  Analysis workspace
    View tab bar and toolbar
    Track workspace
      selected-metric global brush
      shared Step or Elapsed time viewport ruler
      fixed metric-label column and linked chart tracks
    Bottom inspector
```

## Zed-Aligned UI Contract

The workbench follows [Zed](https://github.com/zed-industries/zed)'s visual and
interaction language. The initial reference is Zed commit
[`40dc154a`](https://github.com/zed-industries/zed/commit/40dc154a7cc28270d2319873b0881ef053dc22b9),
including its `theme`, `ui`, `title_bar`, `project_panel`, and `workspace`
boundaries. Reference updates must be explicit so a moving upstream `main`
cannot silently change Seex's acceptance target.

Zed is also the direct implementation reference, while Seex retains a local
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

All menus and rich information popovers use the same opaque surface, foreground,
border, radius, padding rhythm, and elevation as the Add Metric popover. This
includes View, axis, Project-action, and Project-information popovers; underlying
rows and charts must never show through their surfaces.

Visual acceptance compares the shell, tabs, Project tree, toolbar, popovers,
typography, spacing, interaction states, and theme hierarchy side by side with
the pinned reference at representative window sizes and display scale factors.

### Project and Run Sidebar

- Show a searchable, collapsible Project tree following Zed project-panel row,
  disclosure, hover, selection, focus, and context-menu behavior. Flatten
  healthy sources while retaining source identity for errors and disambiguation.
- Label the sidebar `Seex` and its searchable resource region `Projects`.
  Render `Projects` with the same compact, muted group-label treatment as
  `Pinned` and `Archived`; omit redundant headings and selected-Run counts from
  the permanent chrome.
- Keep this sidebar independent of the Analysis workspace and full-height below
  any native window title bar. Hiding it expands the complete Analysis
  workspace rather than only the chart area.
- Place the hide control inside the visible sidebar header. Only while the
  sidebar is hidden does its reveal control appear at the left edge of the
  Analysis View bar.
- Show Runs under each Project with visibility, lifecycle status, and a compact
  secondary identifier. Run rows use an eye control rather than a checkbox. The
  eye and the three trailing actions have fixed, non-shrinking footprints; only
  the Run label truncates when the row becomes constrained.
- Project rows have no leading visibility eye. Their batch Run-visibility
  control is the first item in the Project ellipsis menu: all-visible offers
  `Hide all Runs`, while none-visible and mixed offer `Show all Runs`. The menu
  reflects the current state with `eye`, `eye-off`, or `eye-closed`. Individual
  Run eyes remain binary and stay on their Run rows.
- Use the Project folder itself as the disclosure control: `folder-open` means
  its Runs are expanded and `folder` means they are collapsed. Do not render a
  separate chevron. Keep the Project row itself compact: its right edge contains
  only the hover/focus ellipsis action, never inline Run counts or placement
  text. Hovering or keyboard-focusing the row opens an anchored information card
  to the right of the sidebar, following the Zed Project-panel treatment. The
  card shows the Project name, total Run count, data source, and a visible
  `Pinned` or `Archived` status when applicable. It is informational and must
  not duplicate the Project action menu. Use a Run/execution icon such as
  `circle-play` for the count rather than an unlabelled circle glyph.
- An expanded Project with no Runs renders the indented `No runs` placeholder;
  it must not collapse into an unexplained empty gap.
- Continue to allow at most 10 visible Runs per View, including its Baseline and
  Pinned Runs.
- Provide Import Source, reveal path, refresh, and remove-from-workbench
  actions. Removing a source must not delete native data.
- Keep Projects and Runs in a vertically scrolling resource region so importing
  more resources never grows or displaces the Analysis workspace controls.
- Keep unavailable sources visible with a reconnect/error treatment rather
  than silently removing their Projects and selections.

#### Viewer-Only Sidebar Organization

Order the sidebar groups as `Baseline`, `Pinned`, `Projects`, then `Archived`.
These are presentation and analysis states owned by the viewer. They must not
write Run metadata, lifecycle state, catalog rows, Parquet data, or any other
native-storage record.

- **Baseline** shows the optional single View baseline for the active Analysis
  View. It is not shared with other Views. Setting a different baseline returns
  the previous one to that View's Project listing.
  Changing it updates curve emphasis and the Bottom inspector but does not
  change stored Objective evidence, ranking eligibility, or ranking order. A
  Baseline row has no eye control and is always visible.
- The Ranking tab marks the baseline row and may show candidate deltas against
  it only when that Run is a valid comparison reference for the visible Runs.
  Comparison actions may prefill it as the reference, but still require the
  existing explicit direction and comparison inputs.
- **Pinned** moves a `RunRef` from the active View's Project listing into that
  View's Pinned group. Other Views keep their own independent Pinned placement.
  Pinned rows have no eye controls and are always visible. Unpinning returns a
  Run to the active View's Project pagination with an open eye.
- Project Run visibility is View-owned. Switching Views restores that View's
  eye states without changing any other View.
- **Archived** is workbench-wide. Archiving a `RunRef` removes it from Projects,
  Baseline, and Pinned in every View and places it in the bottom Archived group.
  Archived rows have no eye controls and are never visible in charts. It does
  not delete the Run or change its native lifecycle. Restoring it returns it to
  normal Project pagination in every View and restores each View's prior eye
  state.
- Within a View, a non-Archived Run occupies exactly one sidebar placement:
  Baseline, Pinned, or its Project listing. Archived overrides those placements
  across all Views. Pinned and Archived have no count limit.
- Use `Archived`, not `Achieved`, for this viewer-only organization state.
- Show at most five Runs initially in each Pinned group, expanded Project, and
  Archived group. Each borderless `Show more` text action reveals the next five
  in that group and disappears when none remain. Filtering searches all Runs
  and must not be limited to the currently revealed page.
- Baseline, Pinned, and Archived Run rows have no leading placement icon;
  Project Run rows retain only their leading visibility eye. On hover or
  keyboard focus, every Run row exposes the original three independent,
  borderless icon actions at the right edge: baseline, pin, and archive. The
  active placement substitutes its inverse action where applicable: clear
  baseline, unpin, or restore. Run rows never expose an ellipsis menu and never
  contain `Remove project`.
- Only a Project row exposes one borderless ellipsis button at its right edge,
  and only while that Project row is hovered or keyboard-focused. Its opaque,
  anchored menu opens below and to the right of the ellipsis so it extends into
  the Analysis workspace instead of covering the left sidebar. After the batch
  Run-visibility item, a normal Project offers `Pin project`, `Archive project`,
  and `Remove project`; a Pinned Project substitutes `Unpin project`; an
  Archived Project substitutes `Restore project` while retaining `Pin project`
  and `Remove project`.
- Project-level Pin and Archive organize imported Projects in the workbench and
  are independent of View-owned Pinned Runs, View baselines, and workbench-wide
  Archived Runs. `Pin project` moves the complete Project tree entry from
  `Projects` into the `Pinned` group; `Archive project` moves it into
  `Archived`. The entry carries its disclosure state and child Run rows with it.
  `Unpin project` and `Restore project` move it back into `Projects`. None of
  these moves changes the child Runs' baseline, pinned, archived, or visibility
  state. `Remove project` removes the imported Project reference without deleting
  native Project, Run, catalog, or Parquet data. Destructive confirmation
  behavior belongs to implementation rather than the hover menu.
- `Show more` remains a borderless text button whose hover changes text tone
  only. Baseline and Pinned remain above Projects; Archived stays at the bottom;
  their group names are static text labels.

### Analysis Workspace and View Bar

- Place the View tab strip and toolbar at the top of the Analysis workspace
  only; they do not extend across the independent Project/Run sidebar.
- Keep the View strip horizontally scrollable, with the New View, Bottom
  inspector, and Refresh controls outside the scrolling region. Resource count
  must never wrap the bar or displace fixed controls.
- Render each View name and its close icon as one tab-shaped control. An
  inactive tab is quiet, hover reveals its close affordance, and the active tab
  retains the affordance. Right-clicking the View name opens Rename and Delete;
  no permanent three-dot button is shown.
- Provide create, activate, rename, duplicate, and close actions while always
  retaining at least one View.
- Each View owns its name, visible and Pinned Runs, optional baseline, ordered
  metrics, alignment axis, panel order, per-Metric row heights, and chart
  viewport state.
- A new View starts empty or duplicates the active View explicitly; Views must
  never share mutable selection state implicitly.
- Keep Bottom inspector and Refresh as icon-only controls at the right edge,
  with accessible labels and tooltips. Metric selection and axis selection
  belong to the brush row rather than the View bar.
- Closing the last View creates a fresh empty View.

### Metric Sidebar and Chart Tracks

- Use one fixed-width metric-label column inside the Track workspace rather
  than a separately docked second sidebar. Align every label row with exactly
  one chart track; labels and charts share vertical scrolling and row heights.
- Default each row to the minimum height that fits the metric name and one
  compact metadata line. A bottom drag handle resizes an individual row within
  bounded minimum and maximum heights.
- Keep horizontal viewport pan and zoom confined to the chart column. Metric
  labels remain fixed while chart evidence reprojects.
- Selecting either a Metric row or its chart track selects the same Metric
  panel. Reordering or removing a row must not invalidate unaffected tracks.
- Selected and unselected Metric plots use the same subtle accent-tinted chart
  background—the background previously used by the selected plot. Selection is
  communicated by the fixed label cell and accessibility state, not by changing
  the chart-field color.
- Place the icon-only Add Metric control at the right edge of the fixed label
  cell in the global brush row. Its opaque, searchable popover shows only
  metrics not already present in the active View, as compact single-line rows;
  selecting anywhere on a row adds the panel and immediately removes that
  candidate. Do not repeat the action with trailing `Add` text.
- The metric picker uses the visible Runs' metric union. A Run without a
  selected metric remains in that track's legend as unavailable evidence.
- Only tracks in the active View and within one viewport of the visible scroll
  region prepare GPUI geometry or request new detail data.

### Global Brush, Viewport Ruler, and Cursor

The brush, ruler, and chart tracks have distinct roles and one shared viewport:

- The **global brush** always renders the complete overview of the currently
  selected Metric panel. Changing the viewport never crops or resamples this
  overview; selecting another Metric replaces it with that Metric's complete
  overview.
- The brush selection window is the active viewport. Dragging either edge
  resizes it; dragging its interior pans it. The viewport ruler and all visible
  chart tracks update together.
- The **viewport ruler** uses exactly one axis mode: Step or Elapsed time. It
  spans the entire Analysis workspace, including the fixed metric-label width,
  while its data origin begins at the chart-column boundary.
- At the left pan limit, step zero or the first elapsed coordinate aligns with
  the right edge of the metric-label column. At the right pan limit, the final
  step or elapsed coordinate aligns with the right edge of the Analysis
  workspace. No blank overscroll is permitted at either boundary.
- Scrolling or dragging the ruler pans the viewport. Wheel/pinch and
  `Command-+`/`Command--` zoom it around the pointer when available and the
  midpoint otherwise. Every change updates the brush window and reprojects all
  visible chart tracks immediately.
- The Step/Elapsed time selector is an icon-only popover trigger at the left
  edge of the brush-row label cell. Its compact opaque menu contains `Step` and
  `Elapsed time`; the selected mode is explicit. Use an upward-arrow icon for
  Step and a clock for Elapsed time, with accessible labels and tooltips.
- Axis selection belongs on the left because it establishes how the whole
  workspace interprets horizontal position. Add Metric belongs on the right
  because it changes the content adjacent to the chart column. The two popovers
  are mutually exclusive.
- Both vertical cursors belong to the viewport only. They align independently
  across the ruler and visible chart tracks but must never cross the global
  brush.
- The hover cursor is always a dashed line following the pointer, whether or not
  a locked cursor exists. Hovering a curve uses that series' nearest actual point
  in the immutable detail snapshot and shows only that Run's value tag. It does
  not interpolate a synthetic value; equal-distance ties choose the earlier axis
  coordinate.
- Render each hover value tag as an outlined callout whose triangular point
  touches the dashed cursor. The point faces left when the callout is to the
  right of the cursor and reverses near the chart's right edge. Candidate values
  include their signed raw difference from the active View baseline using the
  compact form `1.00(+0.55)` or `1.00(−0.55)`; the baseline tag shows only its
  own value. The difference is `candidate − baseline`, independent of objective
  direction. Keep the visible callout numeric-only; its accessible label retains
  the Run identity and explains the delta.
- Hovering the Step/Elapsed time ruler moves the same dashed cursor and shows
  the nearest available point for every drawable Run in every visible Metric
  track. Each value tag identifies the Run and value; when its actual point
  coordinate differs from the shared hover coordinate, the full tooltip also
  reports that point's Step or elapsed coordinate.
- The blue capsule belongs only to the dashed hover cursor and contains only the
  coordinate formatted in the active axis mode, such as `496k` or `00:09:42`. Do
  not prefix a Step coordinate with the word `Step` and do not attach this
  capsule to the locked cursor.
- Clicking either a curve or the ruler places or moves a separate solid locked
  cursor at that coordinate. The solid cursor has a small downward triangle at
  the ruler edge and never displays a value tooltip or coordinate capsule. It
  remains anchored to its Step or elapsed coordinate during pan/zoom while
  pointer hover continues to move the dashed cursor independently. `Escape`
  removes only the locked cursor. Either cursor is hidden while its coordinate
  is outside the viewport and reappears when that coordinate is visible again.
- Pointer movement performs nearest-point lookup only against renderer-owned
  snapshots and indexes. It must not issue storage, reduction, or geometry
  preparation requests, and locking the cursor must not change the viewport.
- The global brush and viewport ruler are compact sticky rows below the View
  bar. Metric tracks do not render individual brushes.
- The home range is the union of valid overview extents for the active View's
  visible Runs and metrics. Empty panels do not collapse a valid shared range.
- GPUI actions should expose persistent names such as `ZoomIn`, `ZoomOut`, and
  `ResetView`. Bind both `cmd-=` and `cmd-shift-=` for keyboard layouts that
  produce `+` through Shift, plus `cmd--` and `cmd-0`.

### Bottom Inspector

- Place a resizable Bottom inspector below the Track workspace and entirely
  inside the Analysis workspace. It spans the metric-label and chart columns
  but never extends under the independent Project/Run sidebar.
- Keep the inspector fixed to the bottom of the Analysis workspace while only
  the metric-track region scrolls vertically. Hiding the inspector returns its
  height to the track region.
- Keep its tab bar, selected-Metric context, table headers, and table cells on
  one line. When the minimum content width exceeds the workspace, scroll the
  inspector horizontally as one coherent surface rather than wrapping labels,
  headers, or values.
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
- A View baseline adds a `Baseline` role and optional delta column to Ranking;
  it does not make ineligible evidence eligible, change competition ranks, or
  combine Projects. Deltas appear only within the baseline Run's Project. The
  corresponding Run's curve is visually emphasized in every metric track where
  that Run has drawable evidence.
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
  archived RunRef placements
  Project sidebar visibility and width
  Bottom inspector visibility and height

Analysis View
  ordered RunRefs and MetricPanels
  optional single View baseline RunRef
  pinned RunRef placements and visible Project RunRefs
  axis mode, home extent, shared viewport, pointer anchor, and presentation settings
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
store source paths, View definitions, per-View visible and pinned RunRefs,
selected Metric and inspector tab, workbench archived RunRefs, per-View
baseline, dock visibility and dimensions, presentation state, and safe
preferences. These remain viewer-owned state and must never be mirrored into
native storage. The
workbench document must not store metric points, query snapshots, credentials,
connections, or renderer geometry.

Loading tolerates unavailable sources, removed Projects or Runs, and unknown
metrics without mutating source data. The viewer owns a dependency-free,
length-safe hexadecimal text document headed by `seex-workbench 1`, stored
at `~/Library/Application Support/Seex/viewer.workbench` on macOS.
Writes replace a temporary sibling atomically. Unsupported versions are
reported and left untouched; earlier documents have no implicit migration path.

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
- Import enough Projects and create enough Views to exceed their regions;
  Projects/Runs scroll vertically and View tabs scroll horizontally while New
  View, inspector, refresh, and chart layout remain fixed.
- Expand a Project containing 13 Runs: five appear initially, the first `Show
  more` reveals five, and the second reveals the remaining three. A filter can
  still find a Run that has not yet been revealed.
- Give two Views different visible Project Runs, Pinned Runs, and baselines.
  Switching Views restores each independent state without modifying the other.
- Archive a Run used by both Views. It appears in the shared Archived group and
  leaves Projects, Pinned, and Baseline in both Views and disappears from every
  chart. Restoring it returns it to both Project listings with each View's prior
  eye state.
- Change the View baseline; its curves become emphasized and Ranking marks the
  valid baseline and candidate deltas without changing Core ranks or eligibility.
- Clear the baseline, unpin a Run, and restore an Archived Run; each returns to
  its Project's paginated listing while existing View selections remain intact.
- Switch between Views with different visible Runs; each restores its own brush,
  snapshots, and pending generations.
- Open Add Metric; only unselected metrics appear as compact single-line
  candidates, and choosing one removes it from the popover immediately.
- Switch the axis popover between Step and Elapsed time; the ruler, cursor
  label, and chart projection change together without showing both axes.
- Rapidly zoom with wheel, `Command-+`, or `Command--`; the shared viewport and
  visible tracks respond immediately while queries stay debounced and coalesced.
- Pan the ruler to both limits; the home-range start aligns with the metric-label
  boundary and the home-range end aligns with the Analysis workspace right edge,
  with no overscroll.
- Select one Metric, then pan and resize its brush window. The brush keeps that
  Metric's complete overview while the ruler and every chart track share the
  changing viewport. Lock a solid cursor, then hover elsewhere: the solid line
  and its top triangle remain fixed while the dashed line, coordinate capsule,
  and hover-only value tags move together. Neither cursor enters the brush.
- Scroll many tracks; Metric rows remain aligned while off-screen tracks stop
  preparing geometry and do not initiate viewport refreshes.
- Select a chart without dragging; the Bottom inspector opens with the same
  whole-series Metric summary and Objective evidence as the public read
  surfaces. Pan the same chart; the inspector neither toggles nor re-queries.
- Narrow the window until inspector columns no longer fit; headings and values
  remain single-line and the inspector scrolls horizontally instead of wrapping.
- Hide Project sidebar and Bottom inspector independently; the Analysis
  workspace reflows and both regions restore their previous dimensions.
- Rank a cross-Project selection; results remain grouped by Project and retain
  the explicitly selected objective direction.
- Remove or move one source; other sources remain usable and saved selections
  reconcile without modifying native data.
- Restart; imported sources and View definitions return while query snapshots
  rebuild from native storage.

Future incompatible workbench versions are rejected rather than migrated by
ordinary application startup.
