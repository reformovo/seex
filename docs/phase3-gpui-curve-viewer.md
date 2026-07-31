# Phase 3 GPUI Curve Viewer

> Status: pre-1.0 Seex 0.1.x implementation plan. ADR 0011 accepts the
> desktop-first renderer boundary; this plan may change during validation.

> Roadmap note: this document preserves the original single-panel baseline.
> [`ROADMAP.md`](ROADMAP.md) is authoritative for later sequencing: the
> completed multi-project workbench history is archived, and final resource,
> Metal, and macOS ARM64 release qualification is ROADMAP U7.

## Outcome

Phase 3 delivers an unsigned macOS ARM64 `seex-app` binary for comparing
one metric across at most 10 Runs from one local native Seex Project, using
Phase 2 axes, evidence completeness, and reason semantics.

A million points is a storage-source scale, not a rendering target. Storage
reduces every curve before chart-core or GPUI sees it. A Recharts-style brush
narrows the query viewport while keeping the detail budget fixed, revealing
finer source detail without increasing rendered geometry.

The active viewer targets 120 Hz on supported ProMotion displays, following
Zed's [GPUI Metal pipeline work](https://zed.dev/blog/120fps). GPUI owns display
synchronization; the viewer coalesces state changes and does not run a custom
120 Hz repaint timer or force inactive windows to paint.

## Product Boundary

The first viewer release:

- opens a local Seex Project directory using its existing DuckDB or SQLite
  catalog and local data path;
- selects one Project, up to 10 Runs, and one metric from the selected Runs'
  metric union;
- renders complete evidence and usable partial evidence, while retaining but
  not drawing unavailable or invalid evidence;
- refreshes only on open, selection changes, viewport commits, resize, or an
  explicit Refresh command; and
- does not change the Python API, CLI JSON, catalog schema, Parquet schema, or
  comparison semantics.

Standalone Parquet, S3 data, polling, scalar/ranking panels, dashboards, saved
layouts, `.app` packaging, signing, notarization, and auto-update are out.

## Overview and Detail Data Flow

```text
native ProjectConnection (background worker only)
  |
  +-- overview query: full non-negative axis, low fixed screen budget
  |      -> overview series -> full/home x range -> brush
  |
  +-- detail query: brush closed viewport, fixed main-chart screen budget
         -> detail series -> chart-core projection/hit testing -> GPUI paths
```

The overview and detail results are separate immutable snapshots. The viewer
never loads or retains a full raw series.

### Overview Query

- Run when the source, Project, selected Runs, metric, axis, Refresh generation,
  or brush canvas width changes.
- Query `[0, i64::MAX]` with screen-budgeted extrema reduction.
- Use one point per physical brush pixel, clamped to a 500-2,000 point budget
  per series, plus the contract-defined strict viewport neighbors.
- Derive the shared home range from the returned real first and last points.
- Keep the overview snapshot unchanged while only the brush selection moves.

### Detail Query

- Query the brush's closed selected range with screen-budgeted extrema
  reduction.
- Use two points per physical main-chart pixel, clamped to a 2,000-10,000 point
  budget per series, plus strict viewport neighbors.
- Keep the requested budget unchanged as the brush narrows unless the canvas
  itself is resized. A source range smaller than the budget returns all of its
  effective points.
- Recompute the detail y range from finite drawable points inside the selected
  x range. Add 5% padding; for a constant value use
  `max(abs(value) * 5%, 1e-9)`.

## Interaction Contract

The brush is the canonical horizontal viewport. Its state contains the home
range and selected range, clamps selection to home, enforces a minimum width of
one axis unit, and supports:

- dragging either handle to resize the selected range;
- dragging the selected window to pan;
- main-chart drag pan and cursor-anchored wheel/pinch zoom, synchronized back to
  the brush; and
- Reset View, which restores the full home range.

Handles, selection shading, and labels update every frame during a gesture. If
the existing detail snapshot covers the transient selection, it may be
reprojected immediately. Otherwise the previous detail curve remains visible
with a pending indicator. Releasing a drag submits one query; wheel/pinch
submits after 100 ms without another event.

Every request has a monotonically increasing generation. The single background
worker coalesces requests not yet started, and the UI accepts results only for
its current generation. DuckDB/DuckLake work never runs on the GPUI thread.

Hover selects the nearest rendered real detail point within eight logical
pixels. It never reports a segment interpolation or triggers a raw-point query.
The tooltip includes Run, metric, raw step or elapsed value, and stored metric
value.

## Delivery Phases

### 3A: Renderer-Independent Brush Primitives

- Add brush range resize, pan, anchor zoom, clamp, and reset state to
  `seex-chart-core`.
- Add nearest-real-point hit testing and visible y-range calculation.
- Preserve existing zoom and segment-hit APIs and keep chart-core windowless.

### 3B: Native Read Session and Query Pipeline

- Add the viewer crate and a single worker-owned `ProjectConnection` opened via
  existing config resolution and `open_existing` behavior.
- Reuse Project/Run/metric discovery, aligned queries, and evidence conversion;
  do not construct a writable `NativeClient`.
- Implement overview/detail snapshots, fixed budgets, generations, request
  coalescing, S3 rejection, and manual refresh reconciliation.

### 3C: GPUI Shell and Rendering Adapter

- Build the project/run/metric selectors, evidence legend, detail canvas, and
  fixed-height overview brush.
- Cache GPUI overview and detail paths by series revision, viewport, canvas,
  and theme; GPUI types must not cross into chart-core.
- Expose Open Project, Refresh, Reset View, Step, Elapsed, and Quit commands.
  `seex-app` accepts zero or one Project path; more arguments return usage
  with exit status 2.

### 3D: Performance and Product Validation

- Validate 10 series with 1,000,000 effective source points each without ever
  constructing million-point viewer or renderer vectors.
- Verify evidence states, fixed budgets, brush synchronization, stale-result
  rejection, both catalog backends, local path resolution, and error states.
- Record storage query timings separately from renderer frame timings.

### Original macOS ARM64 Release Plan (Roadmap U7)

- Pin GPUI 0.2.2 as a macOS-only dependency with default features disabled and
  `font-kit` enabled; do not use `gpui-component`, runtime shaders, or Blade.
- Require the Xcode Metal Toolchain so GPUI embeds a build-time metallib.
- Add a macOS ARM64 viewer CI job without changing the Python wheel matrix or
  PyPI publication graph.
- Attach `seex-app-macos-aarch64`, its SHA-256 file, and attestation to a
  tag's GitHub Release.

## Validation Gates

- Overview returns no more than its requested budget plus neighbors; detail
  does the same independently.
- Narrowing the brush keeps the detail budget constant and narrows the storage
  viewport rather than cropping or resampling viewer-owned points.
- GPUI receives at most roughly 20,020 overview points and 100,020 detail
  points for 10 selected Runs, never 10,000,000 raw points.
- Cached brush, pan, zoom, path preparation, and hit testing target a CPU-side
  p95 of 8.33 ms and no individual sample above 16.7 ms in a release build.
- On a 120 Hz ProMotion display, Metal HUD or equivalent frame tracing must
  demonstrate stable 120 FPS after initial load while brush, pan, zoom, and
  hover are active. CI without a 120 Hz display validates only the CPU budget.
- Background queries do not block brush movement, commands, or painting; actual
  query latency is reported but is not treated as a frame-rate measurement.
- Invalid/unavailable evidence remains visible in the legend with all reasons
  but does not enter chart-core; partial running/failed evidence remains
  drawable with an explicit status treatment.
- Replacing GPUI requires a renderer-adapter rewrite, not a query, evidence, or
  chart data-model rewrite.

## Required Verification

Run `cargo fmt --all --check`, workspace Clippy with warnings denied,
`cargo check`, `cargo test`, `cargo build -p seex-app --release`,
`uv run maturin develop --uv`, `uv run pyright`, `uv run pytest`, and
`uv run maturin build --out dist`. Document exact blockers for any command that
cannot run, including a missing Xcode Metal Toolchain.
