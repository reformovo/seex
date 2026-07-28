# Viewer Zed UI Reference

## Reference Pin

PulseOn's analysis workbench uses
[Zed](https://github.com/zed-industries/zed) as its direct visual,
interaction, and component-implementation reference.

- Repository: `zed-industries/zed`
- Commit: [`40dc154a7cc28270d2319873b0881ef053dc22b9`](https://github.com/zed-industries/zed/commit/40dc154a7cc28270d2319873b0881ef053dc22b9)
- Recorded: 2026-07-23
- Scope: application chrome, tabs, project tree, controls, popovers, tooltips,
  theme roles, spacing, typography, focus, and interaction states

Implementation and visual review use this commit, never a moving upstream
`main`. Updating the pin is a deliberate documentation change with its own
review and validation.

## Upstream Sources

| Concern | Zed source at the pinned commit | PulseOn use |
| --- | --- | --- |
| Theme roles | `crates/theme/src/styles.rs`, `crates/theme/src/styles/` | Semantic color roles and appearance hierarchy |
| UI density | `crates/theme/src/ui_density.rs` | Compact/default/comfortable spacing model |
| Spacing and type | `crates/ui/src/styles/spacing.rs`, `typography.rs`, `units.rs` | Token names, sizing rhythm, and text hierarchy |
| Tabs | `crates/ui/src/components/tab.rs`, `tab_bar.rs` | Analysis View tab geometry and selected/inactive treatment |
| Tree rows | `crates/ui/src/components/tree_view_item.rs` | Project/Run disclosure, indentation, selection, and focus |
| Buttons | `crates/ui/src/components/button/` | Toolbar and icon-button states |
| Overlays | `crates/ui/src/components/popover.rs`, `tooltip.rs`, `context_menu.rs` | Metric picker, commands, help, and contextual actions |
| Window chrome | `crates/title_bar/src/title_bar.rs` | Title bar composition and window-level actions |
| Project panel | `crates/project_panel/src/project_panel.rs` | Searchable hierarchical sidebar behavior |
| Workspace | `crates/workspace/src/pane.rs`, `toolbar.rs`, `workspace.rs` | Tab/toolbar/content composition and focus routing |

Paths name upstream implementation evidence; they are not Cargo dependencies.
Relevant component structure, tokens, icons, and interaction logic may be
adapted directly into viewer-owned code compatible with the repository's pinned
GPUI release.

## Local Adoption

- `desktop/theme.rs` adapts semantic appearance roles, series palettes, and the
  pinned default-density geometry.
- `desktop/components/` adapts tab, tree-row, button, overlay, status, focus,
  text-input, and resize primitives into viewer-owned GPUI elements.
- `desktop/assets.rs` embeds the pinned Zed refresh, plus, close, chevron, and
  ellipsis SVG paths behind the viewer's `AssetSource`.
- `desktop/chart/{detail,projection,canvas}.rs` keeps projection caches and
  paint state inside `OverviewChart` and `DetailChart` entities while consuming
  theme-owned chart, brush, and series colors without a second visual system.

The local modules keep stable PulseOn names and compile against GPUI 0.2.2;
they do not import Zed's application dependency graph.

## Initial Geometry

The pinned Zed implementation establishes the starting density:

- tab containers are approximately 32 logical pixels high;
- tree rows are approximately 28 logical pixels high;
- separators and selected-tab edges are one logical pixel;
- controls use compact spacing and restrained corner radii;
- panel hierarchy relies on semantic surfaces and borders, not heavy shadows.

PulseOn changes these values only when chart readability, platform behavior, or
accessibility provides concrete evidence. Changes remain token-driven rather
than feature-level constants.

## Interaction Contract

Every adapted interactive primitive defines these states where applicable:

- inactive and enabled;
- hovered;
- pressed or active;
- selected;
- keyboard-focused with a visible focus treatment;
- disabled;
- pending; and
- error.

Mouse and keyboard paths invoke the same actions. Tooltips describe icon-only
controls. Context menus and popovers preserve focus restoration. Project tree
rows expose disclosure, selection, and hierarchy as separate behavior rather
than making indentation decorative.

## Update Procedure

1. Resolve the intended Zed commit explicitly.
2. Review changes in every upstream source listed above between the old and new
   pins.
3. Record which PulseOn tokens or primitives should change and which should
   remain stable.
4. Update the pin in this document, the Roadmap, and the workbench design draft.
5. Run GPUI interaction tests and the visual comparison matrix before accepting
   the new reference.

The comparison matrix covers light and dark appearance, representative window
widths, display scale factors, hover/active/focus states, long labels, empty
states, pending work, and errors.
