# Viewer Performance Validation

## Measurement Record

- Date: 2026-07-22
- Base commit: `ec89fab`, plus the validation changes in the working tree
- Platform: macOS 26.3 (25D125), arm64, Apple M4 Pro
- Rust: 1.97.1
- Xcode: 26.6 (17F113)
- Metal compiler: Xcode Metal Toolchain 17.6.109.0
- Display target: external display configured at 280 Hz; model and resolution
  were not recorded

The automated scale, CPU, build, type, and test gates pass. An exploratory
single-panel run displayed 280 FPS on the external 280 Hz display, but the
persistent record does not yet contain the required gesture-by-gesture missed
presentation analysis. Because the planned shared timeline and multi-panel
workbench replace this hot path, the final display gate is carried into Roadmap
Phase 3E rather than closing against the transitional single-panel UI.

## Scale Fixture and Query Contract

The opt-in release validation generated both DuckDB and SQLite native Projects.
Each Project contained 10 finished Runs with 1,000,000 effective `loss` points
per Run. Database-side `range` generation avoided constructing million-point
Rust vectors. The retained local fixtures occupied 143 MiB for DuckDB and
136 MiB for SQLite; they are not repository artifacts.

All assertions passed:

- overview returned at most 2,000 points plus two neighbors per Run and at most
  20,020 points overall;
- detail returned at most 10,000 points plus two neighbors per Run and at most
  100,020 points overall;
- narrowing the detail viewport kept the 10,000-point budget, reduced each
  Run's source rows from 1,000,000 to 100,003, and preserved the requested
  viewport; and
- every viewer chart point matched its storage evidence point, proving that the
  viewer did not crop or resample the reduced result.

Query timings measure request submission through immutable worker snapshot
delivery and exclude rendering. Warm values are five samples.

| Backend | Query | Cold | Warm min / median / max |
| --- | --- | ---: | ---: |
| DuckDB | Overview | 1733.905 ms | 1627.727 / 1672.603 / 1718.678 ms |
| DuckDB | Full detail | 1773.152 ms | 1653.415 / 1666.751 / 1700.821 ms |
| DuckDB | Narrow detail | 686.589 ms | 674.201 / 696.890 / 721.489 ms |
| SQLite | Overview | 1767.398 ms | 1603.606 / 1639.230 / 1762.119 ms |
| SQLite | Full detail | 1714.943 ms | 1620.128 / 1680.154 / 1744.267 ms |
| SQLite | Narrow detail | 778.690 ms | 702.445 / 709.406 / 722.522 ms |

## CPU Budget

The release-only test used 10 series with 10,002 renderer-owned points each.
Every scenario passed p95 <= 8.33 ms and maximum <= 16.7 ms.

| Scenario | Samples | p50 | p95 | Maximum |
| --- | ---: | ---: | ---: | ---: |
| Brush resize | 1,000 | 0.000 ms | 0.000 ms | 0.000 ms |
| Brush pan | 1,000 | 0.000 ms | 0.000 ms | 0.000 ms |
| Brush zoom | 1,000 | 0.000 ms | 0.000 ms | 0.000 ms |
| Cached path preparation | 200 | 0.339 ms | 0.410 ms | 0.539 ms |
| Uncached path preparation | 200 | 7.456 ms | 7.817 ms | 8.412 ms |
| Hit testing | 200 | 0.190 ms | 0.207 ms | 0.223 ms |

## High-Refresh Product Check

The release viewer opened the retained 10-million-point DuckDB Project with
`MTL_HUD_ENABLED=1`, and Metal HUD initialized frame interval, present delay,
FPS, and logical FPS metrics. The observed display rate was 280 FPS on the
external 280 Hz display. This confirms that the release binary and HUD can run
against the scale fixture, but it is not the final multi-panel interaction
evidence.

To close the Phase 3E gate, capture a Metal System Trace after the shared
timeline and Metric panel grid are implemented, initial detail loading has
finished, and the UI has warmed for five seconds. Exercise shared-brush resize
and pan, chart pan, wheel/pinch and keyboard zoom, hover, grid scrolling, and
View switching. Record the display's configured rate and require no
viewer-caused presentation spanning two refresh periods. At 280 Hz, two periods
are approximately 7.14 ms. Record the trace conclusion here; do not commit the
local trace bundle.

## Automated Workbench Coverage

The multi-project workbench contract is covered by direct behavioral tests:

- `source_registry_reads_duckdb_and_sqlite_together`, composite-identity Core
  tests, and `panel_requests_are_partitioned_by_source_with_full_run_references`
  cover mixed backends and duplicate native identifiers;
- `source_failures_do_not_erase_other_sources_drawable_series` and
  `superseded_and_inactive_view_results_are_ignored` cover partial failure and
  stale cross-source results;
- `shared_timeline_unions_extents_from_multiple_sources`, Analysis View
  lifecycle/isolation tests, and the keyboard zoom tests cover cross-Project
  selection, shared viewport synchronization, and View isolation;
- worker coalescing, visible-overscan scheduling, aligned Metric row/track, and
  dock action tests cover query pressure, panel visibility, synchronized tracks,
  and independent dock visibility;
- `metric_click_opens_a_resizable_inspector_without_gesture_toggles`, exact
  inspector assertions, and Project-scoped ranking tests cover click-versus-drag,
  Summary/Ranking/Evidence, and explicit objective direction; and
- workbench document round trips plus healthy, removed-Run, unknown-Metric,
  duplicate-identity, unsupported-version, and missing-source recovery tests
  cover persistence and unavailable-source reconciliation without native writes.

## Zed Reference Audit

The workbench was compared against Zed commit
`40dc154a7cc28270d2319873b0881ef053dc22b9` using the durable source map in
`viewer-zed-ui-reference.md`. The review covered the same semantic roles and
component boundaries at compact (600 x 520), default (800 x 600), and expanded
(1440 x 900) logical window sizes.

| Concern | Pinned-reference expectation | Viewer evidence | Result |
| --- | --- | --- | --- |
| Application shell | Full-height Project panel independent of the workspace; tabs remain inside the workspace | GPUI bounds keep the Project sidebar left of the Analysis workspace and keep the tab bar exactly within the Analysis bounds at all three sizes | Pass |
| Tabs and toolbars | 32 px tab container, compact 28 px controls, one-pixel separators | The tab bar is 32 px, its selected interior is 31 px below the separator, and toolbar controls are 28 px | Pass |
| Project tree and overlays | 28 px hierarchical rows, rounded selection, disclosure icons, searchable tree, restrained popover surface | Viewer-owned tree-row and popover primitives use the pinned tokens; the source action opens its in-panel popover in the GPUI interaction test | Pass |
| Typography and spacing | Compact type hierarchy, muted secondary labels, 4 px radius, semantic panel spacing | Section labels, metadata, status, tooltip, and control text use shared theme roles; default-density tests pin 28/28/32 px geometry and the 4 px radius | Pass |
| Interaction states | Hover, active, selected, focus, disabled, pending, and error use consistent semantic roles | Shared tab, tree-row, toolbar, icon, status, tooltip, and focus-ring primitives own these states; existing keyboard, unavailable-source, loading, selection-limit, and source-error tests exercise them | Pass |
| Light and dark hierarchy | Identical structure with appearance-specific semantic palettes | Theme tests prove identical spacing and distinct window/panel/surface, text, focus, status, and series roles in light and dark appearances | Pass |
| Display scale | Logical layout remains stable while storage/render budgets use physical pixels | The renderer scale test maps a 400 logical-pixel plot to 800 physical pixels at 2x without changing layout tokens | Pass |

`application_shell_preserves_pinned_geometry_at_representative_sizes` is the
repeatable shell audit. The GPUI test host does not emulate switching the
macOS window appearance or display scale, so appearance hierarchy is verified
directly from theme roles and physical scaling is verified at the renderer
boundary. No visual-token or layout correction was required; stable element
identifiers were added only so the shell regions can be measured.

## Multi-Track Workbench Gate

`representative_workbench_stays_responsive_while_a_source_is_pending` opens a
2560 x 1800 logical-pixel workbench with 10 selected Runs, six simultaneously
visible compact Metric tracks, and the Bottom inspector. Every track receives
all 10 series. For each track, the test checks the storage-owned detail budget
`clamp(physical_width * 2, 2,000, 10,000)` and allows only the two documented
neighbor points per series. The million-point dual-backend validation above
continues to prove the same budget boundary at production scale.

The test then imports a second healthy native source. While its registry state
is still `Loading`, keyboard zoom changes the shared viewport immediately, all
10 Run selections and six panels remain intact, and the first track remains
rendered. This uncovered and corrected a pre-existing import path that reset
the active Core before background discovery; additional imports now discover
their catalog without changing the active View.

Cross-source reads remain bounded to four concurrent worker sessions by the
shared registry gate, while each source worker serializes its own native
connection. `concurrency_gate_blocks_reads_beyond_its_limit` verifies that an
extra read waits for a permit, and pending-request tests verify latest-only
coalescing per Metric panel.

The release CPU command was rerun after the multi-track workbench test was
added. Every p95 remained below 8.33 ms and every maximum remained below
16.7 ms; the current measurements are recorded in the CPU table above.
The representative gate is opt-in and was run with
`cargo test -p pulseon-viewer --release --features test-support
representative_workbench -- --ignored --nocapture` so hardware-sensitive GPUI
window teardown is not part of ordinary debug test runs.

## Verification

Passed:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo check`
- `cargo test`
- `cargo test -p pulseon-viewer --features test-support`
- `cargo build -p pulseon-viewer --release`
- `uv run maturin develop --uv`
- `uv run pyright` (zero errors)
- `uv run pytest` (106 passed, 2 opt-in MinIO tests skipped)
- `uv run maturin build --out dist`

The Rust build emitted existing future-incompatibility warnings for `block`
0.1.6 and `proc-macro-error2` 2.0.1; warnings were not produced by PulseOn code
and did not bypass the strict Clippy gate.
