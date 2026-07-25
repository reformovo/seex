# Viewer Performance Validation

## Measurement Record

- Date: 2026-07-26
- Implementation commit: `3e2be8d`, plus this validation record update
- Platform: macOS 26.3 (25D125), arm64, Apple M4 Pro
- Rust: 1.97.1
- uv: 0.8.12
- Xcode: 26.6 (17F113)
- Metal compiler: Xcode Metal Toolchain 17.6.109.0
- Display target: external display configured at 280 Hz; model and resolution
  were not recorded

The automated scale, final-layout CPU, build, type, and test gates pass. An
earlier exploratory run displayed 280 FPS on the external 280 Hz display, but
the persistent record does not yet contain the required gesture-by-gesture
missed-presentation analysis for the converged multi-panel workbench. The final
display gate therefore remains open in Roadmap Phase 3E.

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
| Cached path preparation | 200 | 0.673 ms | 0.778 ms | 2.171 ms |
| Uncached path preparation | 200 | 7.122 ms | 7.368 ms | 7.966 ms |
| Hit testing | 200 | 0.200 ms | 0.216 ms | 0.272 ms |
| Ruler hover evidence | 1,000 | 0.002 ms | 0.002 ms | 0.016 ms |

After the converged renderer was integrated, an initial run failed the
unchanged uncached-path gate at p95 9.018 ms and maximum 23.815 ms. A local
Time Profiler recording identified GPUI/Lyon stroke tessellation and path
construction, rather than storage projection, as the dominant work. Complete
solid series now build their GPUI triangles directly; partial dashed evidence
retains the existing GPUI/Lyon path semantics. Three subsequent standard
release runs passed without changing thresholds. Their worst p95 was 7.485 ms
and worst maximum was 15.362 ms. Local `.trace` bundles remain uncommitted.

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

Generate the retained multi-track DuckDB fixture once, then launch it with the
HUD enabled:

```bash
PULSEON_VIEWER_TRACE_FIXTURE_ROOT=/tmp/pulseon-viewer-trace \
  cargo test -p pulseon-viewer --release retained_multi_track_fixture \
  -- --ignored --nocapture

env -i \
  HOME="$HOME" USER="$USER" LOGNAME="$LOGNAME" \
  PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
  TMPDIR="${TMPDIR:-/tmp}" LANG="${LANG:-en_US.UTF-8}" \
  PULSEON_VIEWER_WORKBENCH_PATH=/tmp/pulseon-viewer-trace/workbench.state \
  MTL_HUD_ENABLED=1 \
  ./target/release/pulseon-viewer \
  /tmp/pulseon-viewer-trace/duckdb
```

Fixture generation refuses to overwrite a non-empty backend directory. Reuse
the retained directory for repeat traces, or remove it deliberately before
regenerating it. Metal traces embed the target process environment, so the
viewer is launched with an explicit minimal environment and trace bundles must
remain local, uncommitted validation artifacts.

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
- `metric_click_opens_a_resizable_inspector_without_gesture_toggles`,
  whole-series Metric summary and Objective evidence assertions, retained
  inspector snapshots across zoom, and Project-scoped ranking tests cover
  click-versus-drag, Summary/Ranking/Evidence, and explicit objective direction;
  and
- workbench document round trips plus healthy, removed-Run, unknown-Metric,
  duplicate-identity, unsupported-version, and missing-source recovery tests
  cover persistence and unavailable-source reconciliation without native
  writes;
- converged GPUI tests cover Project menu anchoring and real placement,
  five-item pagination, Metric candidate removal and independent resizing,
  compact single-line Run rows with fixed controls, shared-ruler pan/zoom
  boundaries, simultaneous hover/locked cursors, dense evidence-callout
  separation and full point context, baseline deltas, icon-control tooltips,
  and narrow-window horizontal overflow of the tabular Bottom inspector; and
- final convergence regressions additionally cover full-catalog Run filtering
  beyond the revealed five-item page, all-Source refresh from an empty View,
  the converged shell in a newly created empty View, Baseline/Pinned immunity
  from Project batch visibility, exact brush/ruler/track horizontal geometry,
  compact-row plot height, and distinct logical versus physical plot widths.
- completion-audit regressions cover persisted Removed Project placement and
  cross-View cleanup, 8 px window containment for rich popovers, keyboard-only
  Run action exposure with stable icon widths, panel-generation cancellation
  when Views deactivate, and the distinction between Project-name pagination
  and Run-only search results.

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
boundary. The final audit tightened Metric tracks to the reference's 52–180 px
range, adopted icon-only toolbar controls, made View close controls contextual,
compacted Run rows, and converted the inspector to fixed table columns.
The final audit also reduced the complete brush row to 40 logical pixels,
removed redundant selected-range text, and added semantic text tooltips to
icon-only controls without changing their compact geometry.

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

The release CPU command was rerun against the final layout and direct solid
path implementation. Every p95 remained below 8.33 ms and every maximum
remained below 16.7 ms across three consecutive runs; the final run is recorded
in the CPU table above.
The representative gate is opt-in and was run with
`cargo test -p pulseon-viewer --release --features test-support
representative_workbench_stays_responsive_while_a_source_is_pending --
--ignored --nocapture --test-threads=1` so hardware-sensitive GPUI window
teardown is not part of ordinary debug test runs.

## Verification

Passed against implementation commit `3e2be8d` on 2026-07-26:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo check`
- `cargo test`
- `cargo test -p pulseon-viewer --features test-support`
- `cargo build -p pulseon-viewer --release`
- `cargo test -p pulseon-viewer --release interactive_chart_cpu_budget --
  --ignored --nocapture`
- `cargo test -p pulseon-viewer --release --features test-support
  representative_workbench_stays_responsive_while_a_source_is_pending --
  --ignored --nocapture --test-threads=1`
- `uv run maturin develop --uv`
- `uv run pyright` (zero errors)
- `uv run pytest` (106 passed, 2 opt-in MinIO tests skipped)
- `uv run maturin build --out dist`

The first chained invocation of `cargo test -p pulseon-viewer --features
test-support` reported every test as passed, then the GPUI test process received
SIGSEGV during process teardown. An immediate standalone rerun of the exact
command passed, including 43 library tests, 53 binary tests with two ignored
hardware gates, three native-pipeline tests, and doc tests. The serialized GPUI
suite also passed throughout implementation. No assertion, storage worker, or
viewer runtime failure was observed; the one teardown fault was not reproduced.
The latest full test-support run also passed without the teardown fault.

The retained six-metric trace fixture is 89 MiB. At the time of the automated
gate, `system_profiler` reported two connected Mi Monitor displays at
3840 x 2160 and 2160 x 3840, each using a 60 Hz logical mode. These are not the
280 Hz target display; the separate high-refresh checkbox remains open until
the user reconnects that display and records the required Metal System Trace.

The Rust build emitted existing future-incompatibility warnings for `block`
0.1.6 and `proc-macro-error2` 2.0.1; warnings were not produced by PulseOn code
and did not bypass the strict Clippy gate.
