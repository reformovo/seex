# PulseOn Roadmap

> This roadmap tracks current and future work. Shipped release details live in
> `docs/release-notes/`; durable product boundaries live in
> `docs/native-storage-boundary.md` and accepted ADRs in `docs/adr/`.

Pre-1.0 releases do not promise store, API, or machine-output compatibility;
compatibility and migration commitments begin with the future 1.0 release.

## 0.2.x / Desktop Curve Viewer

0.2.x unlocks interactive analysis that 0.1.x's headless read surface cannot
provide, and ships the comparison alignment semantics that the viewer consumes.
See [ADR 0011](adr/0011-desktop-first-curve-viewer.md) for the desktop-first
decision and [ADR 0012](adr/0012-defer-remote-training.md) for the remote
training deferral.

### Phase 1: Workspace Migration and Renderer-Agnostic Chart Core

- [x] One-time workspace migration to a virtual Cargo workspace (root `Cargo.toml`
  holds only `[workspace]`, no `[package]`): move `src/` to
  `crates/pulseon-core/src/`, set `members = ["crates/*"]`, update
  `pyproject.toml`/maturin `manifest-path` and any CI `cargo` invocations. No
  behavior change; `cargo check`, `cargo test`, `uv run maturin develop`,
  `uv run pyright`, and `uv run pytest` must still pass after the move.
- [x] `crates/pulseon-chart-core`: series model, viewport, scales, ticks,
  path projection, path cache, hit testing, selection and zoom state. Must not
  depend on GPUI, egui, Tauri, React, or a browser runtime, and must be unit
  testable without a window.
- [x] `crates/pulseon-data`: Parquet/DuckDB query and PulseOn schema validation,
  viewport-aware query planning, and screen-budgeted point reduction. Reuses the
  existing Parquet schema contract; no schema changes.

### Phase 1.5: Crate Responsibility Realignment

- [x] Extract shared domain and query contracts into `pulseon-model`.
- [x] Split the PyO3 artifact into `pulseon-python`, leaving `pulseon-core` as a
  reusable application library.
- [x] Rename `pulseon-data` to `pulseon-storage` and consolidate native project
  and standalone Parquet reads behind one metric query contract.
- [x] Move DuckDB/DuckLake bootstrap, reads, writes, flush, configuration, and
  storage errors out of Core. Preserve the Python API and Parquet contract.
- [x] Enforce the dependency direction in `docs/crate-boundaries.md` before
  adding the GPUI viewer.

### Phase 2: Comparison Alignment Semantics

Phase 2 is a read-only derived layer over existing Runs and metric facts. It
does not add catalog or Parquet fields, persisted research context or decisions,
source/Git mutation, repetition or significance policy, runtime dependencies,
or a renderer dependency. It may add typed Rust/Python read APIs and replaces
the pre-1.0 CLI JSON envelope with version 2.

#### Phase 2A: Contract and Product Language

- [x] Write `docs/comparison-semantics.md` as the renderer-agnostic 0.2.x
  contract, explicitly marked as changeable before 1.0. Define comparison axis,
  objective metric, comparison evidence, completeness, outcome, and preference
  as general product terms. Candidate and incumbent remain request roles, not
  stored Run identities. Do not create an ADR before 1.0 freezes the contract.
- [x] Lock two axes: raw step and elapsed wall time from `Run.started_at`.
  Elapsed values may repeat but not decrease; negative or non-monotonic axes
  are invalid, and missing Run start metadata is unavailable.
- [x] Define invalid and partial evidence without repairing it. Preserve usable
  numeric evidence from running and failed Runs while keeping their preference
  inconclusive; do not reorder, interpolate, clamp, or replace invalid values.
- [x] Define scalar comparison from the last effective value at the greatest
  step. Raw delta is `candidate - reference`; relative delta divides by the
  absolute reference and is absent for a zero reference. Direction-normalized
  improvement is positive when better. No tolerance, significance, or
  uncertainty claim is made in 0.2.x.

#### Phase 2B: Observation Time and Aligned Query Foundation

- [x] Capture a metric's observation timestamp on the `run.log(...)` enqueue
  path while leaving `ingested_at` on the background writer. Preserve the
  logging signature, queue admission, drain/finalization behavior, and metric
  schema. Document pre-0.2 timestamps as best-effort elapsed evidence because
  their writer-time origin cannot be detected or migrated safely.
- [x] Add shared alignment request/result types and axis-aware storage queries
  for the native project store and standalone Parquet facts. Alignment uses a
  closed viewport plus one neighboring point on each side. Both axes support
  full and screen-budgeted extrema queries; elapsed queries do not use LTTB.
- [x] Keep standalone Parquet fact-only: step alignment remains available,
  while elapsed alignment reports `missing_run_start` rather than treating the
  first point as the Run origin.

#### Phase 2C: Typed Shared Read API

- [x] Expose typed, renderer-independent alignment, objective, comparison,
  ranking, evidence, and result value objects from the shared Rust
  model/Core boundary. Keep storage execution behind the metric-reader
  contract and rendering conversion outside Core.
- [x] Expose matching read-only Python value objects and Client methods for
  aligned metric queries, Run comparison, and ranking; update the type stub and
  Python type-check fixtures. Do not add autoresearch-specific SDK types: the
  SDK language remains Project, Run, metric, comparison, and ranking.

#### Phase 2D: Generic and Autoresearch Comparison Reports

- [x] Upgrade `metrics compare` to require an explicit baseline contained in
  the requested Run set and an explicit `minimize` or `maximize` direction.
  Permit cross-Project comparison. Preserve input order for candidate reports.
- [x] Add `autoresearch compare` as a role-oriented view over the same Core
  report. Its incumbent is either explicit or the direction-aware best eligible
  Run from an explicit comparator pool; it is never inferred from project
  history. No eligible incumbent yields insufficient evidence, not mutation.
- [x] Report primary and secondary last values, raw and relative deltas,
  normalized improvement, structured completeness/reasons, numeric outcome,
  and compute-only preference. Secondary metrics never affect outcome,
  preference, ranking, or tie-breaking in 0.2.x.
- [x] Allow running and failed Runs to expose available numeric evidence but
  mark their report partial and preference inconclusive. Unknown or duplicate
  Run identities are request errors; missing metrics and non-finite values are
  per-item unavailable/invalid evidence.

#### Phase 2E: Ranking and Machine Output

- [x] Add `autoresearch leaderboard` and `autoresearch best` over a required
  Project with an optional explicit Run subset. Only finished Runs with a
  finite primary objective are eligible; other Runs remain visible with a null
  rank and structured reason.
- [x] Use direction-aware competition ranking (`1, 1, 3`). Exact ties share a
  rank; selecting one best/incumbent prefers the earlier `created_at`, then
  lexical `run_id`. An empty eligible set is a successful `best = null` result.
- [x] Default leaderboard output to 50 entries with limit/offset pagination and
  an explicit all-results option. Compute ranks over the full eligible set
  before pagination.
- [x] Bump every CLI success and error JSON envelope to schema version 2. Keep
  deterministic kinds, ordering, reason codes, pagination metadata, standard
  JSON encoding for non-finite metric values, and null relative deltas when the
  reference is zero. CLI comparison output stays bounded evidence; aligned
  curve points remain on the typed Rust/Python read surface.

#### Phase 2 Validation Gates

- [x] Preserve the native storage and crate dependency boundaries, effective
  last-write-wins series, half-open ordinary step queries, Parquet schema, and
  non-blocking metric-reporting contract.
- [x] Cover native/Parquet alignment parity, negative and decreasing elapsed
  axes, missing Run starts, viewport neighbors, screen budgets, both objective
  directions, zero/non-finite values, partial Runs, cross-Project pairs,
  incumbent pools, ranking ties, empty best, pagination, typed Python use, and
  deterministic JSON.
- [x] Pass `cargo check`, `cargo test`, `uv run maturin develop --uv`,
  `uv run pyright`, and `uv run pytest`; run the logging throughput benchmark
  after moving timestamp capture and document any measurable regression.

### Phase 3: GPUI Desktop Viewer

The original single-panel implementation and validation contract lives in
[`docs/phase3-gpui-curve-viewer.md`](phase3-gpui-curve-viewer.md). The
multi-project workbench extension is defined in
[`docs/drafts/multi-project-analysis-workbench.md`](drafts/multi-project-analysis-workbench.md).
A million points is a storage-source scale; GPUI renders only fixed-budget
storage reductions selected through a shared viewport.

#### Phase 3A: Renderer-Independent Brush Contract

- [x] Add a chart-core brush state with home and selected x ranges, one-axis-unit
  minimum width, handle resize, selected-window pan, cursor-anchored zoom,
  home clamping, and reset. Keep the existing zoom API intact.
- [x] Add nearest-rendered-real-point hit testing with stable point indexes and
  screen-distance ties; keep the existing segment interpolation API intact.
- [x] Add visible finite y-range calculation with 5% padding and a defined
  constant-value fallback. Do not add query, reduction, or renderer policy to
  chart-core.
- [x] Cover brush transforms, invalid inputs, range boundaries, repeated x
  values, hit-test ties, empty inputs, and constant y values with windowless
  chart-core tests.

#### Phase 3B: Native Read Session and Query Pipeline

- [x] Add `pulseon-viewer` as a workspace binary with model, storage, Core, and
  chart-core dependencies. Pin GPUI 0.2.2 only for macOS; keep a non-macOS
  unsupported entrypoint so Linux workspace checks continue to compile.
- [x] Open existing local native Projects through the shared configuration and
  storage bootstrap. Support DuckDB/SQLite and custom local paths; reject S3
  before credential resolution and do not construct a writable `NativeClient`.
- [x] Add one background worker that owns `ProjectConnection`, discovers
  Projects/Runs/metrics, coalesces pending requests, and returns immutable
  generation-tagged snapshots. The GPUI thread must never execute storage work.
- [x] Add separate overview and detail query requests. Overview uses the full
  non-negative axis and a 500-2,000 point budget; detail uses the brush's closed
  viewport and a 2,000-10,000 point budget. Both reuse Phase 2 aligned evidence
  and storage-side extrema reduction without retaining raw full series.
- [x] Reconcile manual Refresh results, discard stale generations, retain the
  current detail snapshot while a replacement is pending, and cover source
  errors, both catalog backends, fixed budgets, neighbors, and evidence states.

#### Phase 3C: GPUI Curve Comparison Experience

- [x] Add the macOS application shell, zero-or-one-path command-line contract,
  native directory picker, Open/Refresh/Reset/Step/Elapsed commands, and clear
  empty, loading, pending, and error states.
- [x] Add one-Project selection, a virtualized newest-first Run browser with
  name/id/status filtering and a 10-Run hard limit, plus a one-metric selector
  over the selected Runs' metric union.
- [x] Add the detail chart renderer with axes, grid, evidence-aware legend,
  fixed series colors, path caching, automatic visible y range, and nearest
  real-point tooltip. Draw partial evidence explicitly; do not draw invalid or
  unavailable evidence.
- [x] Add the fixed-height overview renderer, selection shade, two handles,
  selected-window drag, main-chart pan/zoom synchronization, and reset. Commit
  drag queries on release and wheel/pinch queries after 100 ms idle.
- [x] Verify GPUI types stay inside the adapter and test selection transitions,
  picker cancellation, command behavior, resize budgets, brush synchronization,
  pending results, hover, and stale-result rejection.

#### Phase 3D: Scale and Automated Performance Baseline

- [x] Build a deterministic fixture with 10 Runs and 1,000,000 effective source
  points per series. Assert that the viewer receives only the requested
  overview/detail budgets plus contract-defined neighbors.
- [x] Validate that narrowing the brush keeps the detail budget fixed, narrows
  the storage viewport, and never crops or resamples viewer-owned points.
- [x] Measure DuckDB and SQLite overview, full-detail, and narrow-detail query
  latency separately from rendering, including cold and warm samples.
- [x] In a macOS ARM64 release build, verify cached brush, pan, zoom, path
  preparation, and hit testing at p95 <= 8.33 ms with no sample above 16.7 ms.
- [x] Pass formatting, workspace Clippy, Rust tests, viewer release build,
  maturin develop/build, Pyright, and pytest; document any exact environmental
  blocker, including a missing Xcode Metal Toolchain.

Automated measurements are recorded in
[`viewer-performance-validation.md`](viewer-performance-validation.md). The
single-panel UI is no longer the durable product surface, so its outstanding
manual display trace is carried into Phase 3E and must validate the final
shared-timeline, multi-panel interaction path.

#### Phase 3E: Multi-Project Analysis Workbench

The workbench implements the product and architecture contract in
[`multi-project-analysis-workbench.md`](drafts/multi-project-analysis-workbench.md).
It does not change the public Python API, native catalog or Parquet schemas,
comparison semantics, runtime dependency set, CI, or release behavior without
separate approval. Product code uses durable workbench terminology and never
roadmap phase identifiers.

##### Zed-Aligned Visual Foundation

- [x] Use [Zed](https://github.com/zed-industries/zed) as the visual and
  interaction reference, initially pinned to commit
  [`40dc154a`](https://github.com/zed-industries/zed/commit/40dc154a7cc28270d2319873b0881ef053dc22b9).
  Record any deliberate reference update. Do not follow a moving `main` during
  implementation. The durable source map and update procedure live in
  [`viewer-zed-ui-reference.md`](viewer-zed-ui-reference.md).
- [x] Add viewer-owned semantic theme and spacing tokens modeled on Zed's UI
  roles: window, panel, elevated surface, border, text, muted text, hover,
  active, focus, disabled, accent, and status. Remove feature-level hard-coded
  RGB values and preserve the same hierarchy in light and dark appearance.
- [x] Build viewer-owned tab bar, sidebar tree row, toolbar/icon button,
  popover, tooltip, status badge, empty state, and focus-ring primitives with
  Zed-consistent compact geometry, typography, one-pixel separators, selected
  surfaces, and complete hover/active/focused/disabled states.
- [x] Use Zed's `theme`, `ui`, `title_bar`, `project_panel`, and `workspace`
  crates as direct implementation references. Adapt relevant component
  structure, tokens, icons, and interaction logic into viewer-owned code. Do
  not depend on whole Zed application crates because their GPUI revision and
  dependency graph differ from the viewer's pinned runtime.

##### Workbench Identity and Multi-Source Reads

- [x] Add viewer-local `DataSourceId` and composite
  `RunRef = DataSourceId + ProjectId + RunId` identities. Use the full identity
  for selection, series colors, caches, hover, requests, and stale-result
  reconciliation while preserving existing storage/model identities.
- [x] Add a retained source registry and bounded read coordination for multiple
  local native stores. Support DuckDB and SQLite together, keep each native
  connection worker-owned, lazily activate source sessions, and preserve
  source-specific loading and error state.
- [x] Fan panel reads out by source and merge immutable evidence at the viewer
  boundary. One source failure must not erase another source's drawable
  results, and inactive or superseded View/panel generations must be ignored.

##### Project and Run Sidebar

- [x] Replace the single-source selectors with a searchable, collapsible
  Project/Run sidebar over all imported sources. Keep it as an independent,
  full-height application-shell region outside the Analysis workspace, qualify
  name collisions with source identity, and retain unavailable sources with
  actionable state.
- [x] Add Import Source, reveal path, refresh, and remove-from-workbench
  actions plus `ToggleProjectSidebar`. Removing an import must never delete or
  mutate native data; hiding the sidebar expands the complete Analysis
  workspace.
- [x] Make Run checkboxes reflect the active Analysis View and retain the
  existing limit of 10 selected Runs per View across Project/source boundaries.

##### Analysis Views

- [x] Add top tabs for creating an empty View, duplicating the active View,
  activating, renaming, and closing Views. Place this bar inside the Analysis
  workspace so it never spans the independent Project/Run sidebar. Closing the
  last View creates a new empty View.
- [x] Give each View independent ordered Runs and metrics, alignment axis, track
  density, shared viewport, snapshots, pending generations, and errors. View
  switching must not share mutable selection or brush state implicitly.

##### Metric Sidebar, Shared Timeline, and Tracks

- [x] Render one sticky shared timeline brush per View. Its home range is the
  union of valid selected Run/metric extents; it renders navigation ticks and
  selection rather than a synthetic metric aggregation and spans only the
  chart-track column.
- [x] Support handle resize, selected-window pan, wheel/pinch zoom,
  `Command-+`, `Command--`, and `Command-0`. Reproject cached evidence
  immediately, then use one View-level 100 ms trailing debounce before
  requesting visible-track detail.
- [x] Render a Metric sidebar inside the Analysis workspace and one aligned
  detail chart track per selected metric. Synchronize row heights and vertical
  scrolling, keep horizontal navigation in the chart column, and preserve
  unavailable evidence, independent y ranges, hover, errors, ordering, and
  removal.
- [x] Derive each visible track's storage budget from its own physical plot
  width and independently reduce every Run/metric series. Prepare visible
  tracks plus one viewport of overscan; off-screen tracks contribute extents
  but do not issue detail queries or prepare GPUI paths.

##### Bottom Inspector and Dock Visibility

- [x] Add a resizable Bottom inspector inside the Analysis workspace, spanning
  the Metric sidebar and chart column but not the independent Project/Run
  sidebar. A click without a drag on a Metric row or chart track selects it and
  opens `Summary`, `Ranking`, and `Evidence`; pan/zoom/brush gestures never
  toggle the inspector.
- [x] Query exact viewport-scoped Summary statistics in the background rather
  than aggregating reduced renderer points. Ranking requires explicit
  minimize/maximize direction and remains grouped by Project for cross-Project
  Views. Tag inspector results for stale-result rejection.
- [x] Hide and restore Project sidebar and Bottom inspector independently,
  retain their previous width/height, and let the Metric sidebar resize or
  collapse compactly without clearing selections. Expose durable toggle/show
  actions and keep focus restoration keyboard-accessible.

##### Persistence and Recovery

- [x] Persist a versioned, viewer-owned workbench document containing imported
  source paths, Views, composite selections, selected Metric/inspector tab,
  dock visibility and dimensions, and presentation settings. Do not persist
  metric points, query snapshots, credentials, native connections, or renderer
  geometry.
- [x] Restore state without mutating native stores and reconcile moved or
  missing sources, removed Projects/Runs, duplicate identifiers, unknown
  metrics, and unsupported document versions explicitly.

##### Validation Gates

- [ ] Cover mixed DuckDB/SQLite sources, duplicate Project/Run identifiers,
  partial source failure, cross-Project selection, View isolation, shared
  viewport synchronization, keyboard zoom, query coalescing, panel visibility,
  synchronized Metric tracks, click-versus-drag inspector behavior, exact
  Summary/Ranking evidence, independent dock visibility, stale results,
  persistence round trips, and unavailable-source recovery.
- [ ] Compare the application shell, tabs, Project tree, toolbars, popovers,
  interaction states, typography, spacing, and light/dark hierarchy against the
  pinned Zed reference at representative window sizes and display scales.
- [ ] Validate a representative View with 10 Runs and at least six visible
  Metric tracks plus the Bottom inspector. Preserve storage point budgets, the
  Phase 3D CPU thresholds,
  bounded query concurrency, and responsive interaction while sources are
  pending.
- [ ] On the active high-refresh display, record its configured refresh rate
  and a Metal System Trace for shared-brush resize/pan, chart pan, wheel/pinch
  and keyboard zoom, hover, scrolling, and View switching. Sustain the
  configured rate after warm-up with no viewer-caused presentation spanning
  two refresh periods; at 280 Hz that boundary is approximately 7.14 ms.
- [ ] Pass formatting, workspace Clippy, Rust tests, viewer release build,
  maturin develop/build, Pyright, and pytest, and update the persistent
  performance record with exact commands, machine, display, and conclusions.

#### Phase 3F: macOS ARM64 Release

- [ ] Add a macOS ARM64 viewer CI job that installs or verifies the Xcode Metal
  Toolchain, runs viewer tests, and builds the unsigned release binary without
  changing the Python wheel matrix or PyPI dependency graph.
- [ ] On tags, produce `pulseon-viewer-macos-aarch64` and its SHA-256 checksum,
  attest both artifacts, and attach them to the corresponding GitHub Release.
- [ ] Preserve the existing Python wheel and sdist release behavior and verify
  the release job cannot publish a viewer artifact to PyPI accidentally.

### Out of 0.2.x Scope

- Cumulative-token and normalized-budget comparison axes.
- Stable Contract / compatibility ADR / schema version marker / deprecation
  policy (deferred to 1.0).
- Retry-safe migration command (mutates state; deferred to 1.0).
- Persisted research decisions / durable research context / lineage / decisions
  in catalog state (ADR-gated, later).
- Research driver with Git/source mutation (ADR-gated, later).
- Remote training service delivery (deferred per ADR 0012).
- Repetition / significance / uncertainty policies (after deterministic
  policies are validated).

## Later Backlog

### Local Coordination

- [ ] Define and validate multi-client SQLite run-writer coordination before
  expanding the current single-writer native contract.

### Credentials and Remote Training

- [ ] Add environment-variable or AWS credential-chain discovery for S3
  credentials when explicit config-file credentials are insufficient.
- [ ] Revisit the [remote control-service boundary](drafts/remote-training-architecture-notes.md)
  when local training is complete and a real rented-GPU workflow exists, per
  [ADR 0012](adr/0012-defer-remote-training.md). Produce a remote training ADR
  before adding remote writers or shared catalog coordination.
- [ ] Consider PostgreSQL catalog support only when remote service scale or
  availability requires it.

### Analysis and Agent Workflows

- [ ] Evaluate the [research driver](drafts/autoresearch-control-loop-notes.md)
  without moving source or Git mutation into PulseOn Core.
- [ ] Design config/tag filtering, export, Web UI, MCP, and other agent-facing
  surfaces as independently reviewable roadmap phases after the local analysis
  workbench is validated.

## 1.0 / Stable Contract

1.0 freezes the surfaces proven by 0.2.x. It is the first release with a
compatibility commitment; pre-1.0 releases make none.

- [ ] Accept an ADR defining 1.0 compatibility for the typed Python API,
  versioned CLI JSON, catalog application schema, and Parquet schema.
- [ ] Add an explicit store schema/version marker without changing the metric
  point Parquet compatibility boundary.
- [ ] Support an explicit `0.1.0a5` store upgrade; diagnose older unversioned
  stores without promising direct a1-a4 migration.
- [ ] Document additive changes, deprecation, breaking changes, and the support
  window for stable stores and machine-readable output.
- [ ] Add an explicit, retry-safe migration command that backs up catalog state
  and never rewrites a store during ordinary initialization. Cover DuckDB and
  SQLite stores, backend/config mismatches, mixed legacy artifacts, interrupted
  migration, retry, and backup recovery.
