use std::sync::Arc;

use gpui::{Bounds, ContentMask, PathBuilder, Pixels, Rgba, Styled, canvas, fill, point, px, size};
use pulseon_chart_core::{AxisRange, BrushState, Viewport};

use crate::data::query::CurveSnapshot;
use crate::domain::RunRef;

use super::{BRUSH_HANDLE_WIDTH, ChartAdapter, PreparedChart, RenderRuns, overview_viewport};
use crate::desktop::app::theme::ViewerTheme;

pub(in crate::desktop::app) fn detail_canvas(
    adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
    snapshot: Arc<CurveSnapshot>,
    revision: u64,
    viewport: Viewport,
    baseline: Option<RunRef>,
    emphasized_run: Option<RunRef>,
    visible_runs: std::rc::Rc<[RunRef]>,
) -> impl gpui::Styled + gpui::IntoElement {
    canvas(
        move |bounds, window, _| {
            let resized = adapter.borrow().detail_bounds != Some(bounds);
            let prepared = adapter.borrow_mut().prepare(
                &snapshot,
                revision,
                viewport,
                bounds,
                window.appearance(),
                RenderRuns {
                    baseline: baseline.as_ref(),
                    emphasized: emphasized_run.as_ref(),
                    visible: Some(&visible_runs),
                },
            );
            if resized {
                window.request_animation_frame();
            }
            prepared
        },
        move |bounds, prepared, window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let grid = prepared.theme.colors.chart_grid;
                for index in 0..=5 {
                    let ratio = index as f32 / 5.;
                    let x = bounds.origin.x + bounds.size.width * ratio;
                    let y = bounds.origin.y + bounds.size.height * ratio;
                    window.paint_quad(fill(
                        Bounds::new(point(x, bounds.origin.y), size(px(1.), bounds.size.height)),
                        grid,
                    ));
                    window.paint_quad(fill(
                        Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, px(1.))),
                        grid,
                    ));
                }
                for (path, color) in prepared.paths {
                    window.paint_path(path, color);
                }
            });
        },
    )
}

pub(in crate::desktop::app) fn cursor_canvas(
    adapter: Option<std::rc::Rc<std::cell::RefCell<ChartAdapter>>>,
    range: AxisRange,
    hover: Option<f64>,
    locked: Option<f64>,
    show_locked_marker: bool,
) -> impl gpui::Styled + gpui::IntoElement {
    canvas(
        move |bounds, window, _| {
            if let Some(adapter) = &adapter {
                adapter.borrow_mut().detail_bounds = Some(bounds);
            }
            ViewerTheme::for_appearance(window.appearance())
        },
        move |bounds, theme, window, _| {
            let x_for = |axis: f64| {
                bounds.origin.x
                    + bounds.size.width
                        * ((axis - range.start()) / range.span()).clamp(0., 1.) as f32
            };
            if let Some(axis) = locked.filter(|axis| *axis >= range.start() && *axis <= range.end())
            {
                let x = x_for(axis);
                window.paint_quad(fill(
                    Bounds::new(point(x, bounds.origin.y), size(px(1.), bounds.size.height)),
                    theme.colors.accent,
                ));
                if show_locked_marker {
                    let mut triangle = PathBuilder::fill();
                    triangle.move_to(point(x - px(5.), bounds.origin.y));
                    triangle.line_to(point(x + px(5.), bounds.origin.y));
                    triangle.line_to(point(x, bounds.origin.y + px(7.)));
                    triangle.close();
                    if let Ok(path) = triangle.build() {
                        window.paint_path(path, theme.colors.accent);
                    }
                }
            }
            if let Some(axis) = hover.filter(|axis| *axis >= range.start() && *axis <= range.end())
            {
                let x = x_for(axis);
                let mut y = bounds.origin.y;
                while y < bounds.bottom() {
                    window.paint_quad(fill(
                        Bounds::new(point(x, y), size(px(1.), px(5.))),
                        theme.colors.accent,
                    ));
                    y += px(9.);
                }
            }
        },
    )
}

pub(in crate::desktop::app) fn callout_shell(
    points_right: bool,
    pointer_center: Pixels,
    background: Rgba,
    border: Rgba,
) -> impl gpui::Styled + gpui::IntoElement {
    canvas(
        move |_, _, _| (points_right, pointer_center, background, border),
        move |bounds, (points_right, pointer_center, background, border), window, _| {
            for (inset, color) in [(0., border), (1., background)] {
                let inset = px(inset);
                let left = bounds.origin.x + inset;
                let right = bounds.right() - inset;
                let top = bounds.origin.y + inset;
                let bottom = bounds.bottom() - inset;
                let arrow_width = px(9.);
                let corner_radius = px(2.) - inset / 2.;
                let middle = (bounds.origin.y + pointer_center)
                    .max(top + corner_radius)
                    .min(bottom - corner_radius);
                let (rectangle_left, rectangle_right) = if points_right {
                    (left, right - arrow_width)
                } else {
                    (left + arrow_width, right)
                };
                let mut shell = PathBuilder::fill();
                if points_right {
                    shell.move_to(point(rectangle_left + corner_radius, top));
                    shell.line_to(point(rectangle_right, top));
                    shell.line_to(point(right, middle));
                    shell.line_to(point(rectangle_right, bottom));
                    shell.line_to(point(rectangle_left + corner_radius, bottom));
                    shell.curve_to(
                        point(rectangle_left, bottom - corner_radius),
                        point(rectangle_left, bottom),
                    );
                    shell.line_to(point(rectangle_left, top + corner_radius));
                    shell.curve_to(
                        point(rectangle_left + corner_radius, top),
                        point(rectangle_left, top),
                    );
                } else {
                    shell.move_to(point(rectangle_left, top));
                    shell.line_to(point(rectangle_right - corner_radius, top));
                    shell.curve_to(
                        point(rectangle_right, top + corner_radius),
                        point(rectangle_right, top),
                    );
                    shell.line_to(point(rectangle_right, bottom - corner_radius));
                    shell.curve_to(
                        point(rectangle_right - corner_radius, bottom),
                        point(rectangle_right, bottom),
                    );
                    shell.line_to(point(rectangle_left, bottom));
                    shell.line_to(point(left, middle));
                    shell.line_to(point(rectangle_left, top));
                }
                shell.close();
                if let Ok(path) = shell.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size_full()
}

pub(in crate::desktop::app) fn timeline_canvas(
    adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
    brush: BrushState,
    snapshot: Option<Arc<CurveSnapshot>>,
    revision: u64,
    emphasized_run: Option<RunRef>,
    visible_runs: std::rc::Rc<[RunRef]>,
) -> impl gpui::Styled + gpui::IntoElement {
    canvas(
        move |bounds, window, _| {
            let mut adapter = adapter.borrow_mut();
            let resized = adapter.overview_bounds != Some(bounds);
            adapter.overview_bounds = Some(bounds);
            if resized {
                window.request_animation_frame();
            }
            let Some(snapshot) = snapshot.as_deref() else {
                return PreparedChart {
                    paths: Vec::new(),
                    theme: ViewerTheme::for_appearance(window.appearance()),
                };
            };
            let Some(viewport) = overview_viewport(snapshot, brush.home(), &visible_runs) else {
                return PreparedChart {
                    paths: Vec::new(),
                    theme: ViewerTheme::for_appearance(window.appearance()),
                };
            };
            let detail_bounds = adapter.detail_bounds;
            let prepared = adapter.prepare(
                snapshot,
                revision,
                viewport,
                bounds,
                window.appearance(),
                RenderRuns {
                    baseline: None,
                    emphasized: emphasized_run.as_ref(),
                    visible: Some(&visible_runs),
                },
            );
            adapter.detail_bounds = detail_bounds;
            prepared
        },
        move |bounds, prepared, window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for index in 0..=6 {
                    let ratio = index as f32 / 6.;
                    let x = bounds.origin.x + bounds.size.width * ratio;
                    window.paint_quad(fill(
                        Bounds::new(point(x, bounds.origin.y), size(px(1.), bounds.size.height)),
                        prepared.theme.colors.chart_grid,
                    ));
                }
                for (path, color) in prepared.paths {
                    window.paint_path(path, color);
                }
                let start_ratio = ((brush.selected().start() - brush.home().start())
                    / brush.home().span()) as f32;
                let end_ratio =
                    ((brush.selected().end() - brush.home().start()) / brush.home().span()) as f32;
                let start = bounds.origin.x + bounds.size.width * start_ratio;
                let end = bounds.origin.x + bounds.size.width * end_ratio;
                window.paint_quad(fill(
                    Bounds::new(
                        point(start, bounds.origin.y),
                        size(end - start, bounds.size.height),
                    ),
                    prepared.theme.colors.brush_selection,
                ));
                for x in [start, end] {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x - px(BRUSH_HANDLE_WIDTH / 2.), bounds.origin.y),
                            size(px(BRUSH_HANDLE_WIDTH), bounds.size.height),
                        ),
                        prepared.theme.colors.accent,
                    ));
                }
            });
        },
    )
}
