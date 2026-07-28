# GPUI Curve Viewer Spike

This is a temporary design note for evaluating a Seex-native curve viewer
based on GPUI. It is not an accepted architecture decision.

## Context

Seex's product boundary is the Parquet metric schema, while the core package
intentionally avoids built-in plotting dependencies. A curve viewer should
therefore live outside the Python/Rust SDK surface and consume Seex Parquet
data through a separate query and rendering layer.

The candidate direction is to build a desktop-first curve viewer with GPUI,
while keeping the chart data model and rendering algorithms independent of GPUI
or egui. The `gpui-component` project is useful as a reference for chart
composition, but should not be imported as a product dependency before the
viewer proves its own requirements.

Relevant references:

- GPUI: <https://github.com/zed-industries/zed/tree/main/crates/gpui>
- gpui-component: <https://github.com/longbridge/gpui-component>
- gpui-component line chart:
  <https://github.com/longbridge/gpui-component/blob/main/crates/ui/src/chart/line_chart.rs>

## Working Position

Use GPUI only as a candidate desktop shell and renderer adapter. Do not make
GPUI the data model, query model, or chart algorithm boundary.

The preferred exploratory split is:

```text
seex-chart-core
  series model
  viewport model
  scales and ticks
  downsampling
  path cache
  hit testing
  selection and zoom state

seex-data
  Parquet/DuckDB query
  Seex schema validation
  viewport-aware query planning

seex-gpui-app
  desktop layout
  file/directory picking
  panels and commands
  GPUI rendering adapter
```

`seex-chart-core` must not depend on GPUI, egui, Tauri, React, or a browser
runtime. That boundary keeps the viewer free to target a GPUI desktop app first
without closing the door on an egui prototype or future web/Tauri viewer.

## Why Not Directly Depend On gpui-component

`gpui-component` has useful UI ideas: plot scales, axes, grid, line shapes, and
tooltip state are separated clearly enough to learn from. It also has desktop
components that map well to an experiment analysis workbench.

Direct dependency is premature for Seex because the line chart is a generic
chart widget, while Seex needs an experiment-curve engine:

- Seex x values are usually continuous `step` or `timestamp`, not category
  labels.
- Large runs require viewport-aware downsampling and cached paths instead of
  rebuilding all geometry from the original data on every paint.
- Multi-run comparison needs hit testing across uneven series, missing points,
  and different step ranges.
- The product needs brush zoom, range selection, linked tooltips, metric/run
  legends, and stable behavior for many series.
- The core series representation should be compact typed data, not generic
  per-point access through closures.

The useful lesson is the layering, not the dependency.

## GPUI Versus egui

GPUI is the stronger candidate for a polished native desktop product. It is
better aligned with a modern, panel-heavy experiment analysis tool, especially
if Seex wants a desktop workbench rather than only a debug utility.

egui remains useful for quick Rust-native prototypes and possible WASM demos.
It is simpler to wire up and iterate on, but a refined product UI will likely
require more custom component work.

Neither GPUI nor egui is currently the best primary route for one shared
browser, desktop, and mobile application. If that cross-platform product
promise becomes the top priority, a web core with Tauri remains the safer
route.

## Spike Scope

The spike should answer whether GPUI can support the desktop curve experience
without forcing bad boundaries into the data and chart core.

Minimum scenario:

- Read Seex-compatible Parquet metric data with DuckDB.
- Load at least 10 metric series for comparison.
- Support raw series with 1,000,000 points each.
- Downsample the visible viewport to 2,000-10,000 points per visible series.
- Render line charts with axes, grid, legend, hover tooltip, and nearest-point
  marker.
- Support pan and zoom without re-querying more data than necessary.
- Keep chart-core independent from GPUI-specific types.

Out of scope for the spike:

- Packaging and auto-update.
- Cloud auth and remote object-store credentials.
- Mobile support.
- Full experiment dashboard workflows.
- Public Python API changes.
- Changes to the Seex Parquet schema.

## Validation Gates

The spike is promising only if all of these hold:

- GPUI can render 10 visible series smoothly after viewport downsampling.
- Pan, zoom, and hover remain responsive with million-point source series.
- The chart core can be unit tested without a GPUI window.
- Data loading and drawing are separated by a typed series boundary.
- Replacing GPUI with egui would require a renderer adapter rewrite, not a data
  model rewrite.
- The implementation does not require importing `gpui-component` wholesale.

The spike should be rejected or narrowed if:

- Performance depends on keeping GPUI-specific state inside chart-core.
- Hit testing becomes coupled to the renderer instead of the viewport model.
- Every interaction requires full-series geometry rebuilds.
- Browser/PWA delivery is still a first-release requirement.

## Grilling Questions

Before promoting this to an ADR, answer these directly:

1. Is Seex building a desktop workbench first, or a browser-first viewer?
2. What is the largest first-release dataset that must feel interactive?
3. Is cloud Parquet a first-release requirement, or can the GPUI spike stay
   local-filesystem only?
4. Are comparison semantics based on `step`, `timestamp`, or both?
5. Does the viewer need exact raw points on hover, or is downsampled hover good
   enough until the user zooms in?
6. Should downsampling happen in DuckDB queries, chart-core, or both?
7. What must remain stable if GPUI is abandoned after the spike?

## Temporary Recommendation

Proceed with a GPUI desktop spike only if the immediate product bet is a native
desktop experiment analysis tool. Keep the browser/Tauri route separate until
the product priority is explicit.

For implementation discipline, start with `seex-chart-core` and a narrow
GPUI adapter. Treat `gpui-component` as reference material for chart layering,
not as a dependency.
