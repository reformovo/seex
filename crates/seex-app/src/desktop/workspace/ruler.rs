use std::sync::Arc;

use crate::data::query::{CurveAxis, CurveSnapshot};
use crate::domain::RunRef;
use crate::workbench::MetricPanel;
use gpui::{
    App, Context, Corner, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ScrollWheelEvent, SharedString, Window, anchored, deferred, div, point, prelude::*, px,
};
use seex::AlignmentAxis;
use seex::EvidenceReason;
use seex_chart_core::{AxisRange, BrushState, Viewport};

use super::super::chart::{self, HoverPoint};
use super::super::command::WorkbenchCommand;
use super::super::components::{self, IconName};
use super::super::theme::ViewerTheme;
use super::{
    AnalysisWorkspace, AnalysisWorkspaceEvent, BRUSH_CONTENT_TOP_PADDING, BRUSH_ROW_HEIGHT,
    DragGesture, WorkspaceInteractionEvent,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BrushMutation {
    ResizeStart(f64),
    ResizeEnd(f64),
    Pan(f64),
}

pub(crate) fn update_brush_drag(
    brush: BrushState,
    gesture: &mut DragGesture,
    axis: f64,
) -> Option<BrushMutation> {
    let selected = brush.selected();
    match gesture {
        DragGesture::BrushStart if axis < selected.end() => Some(BrushMutation::ResizeStart(axis)),
        DragGesture::BrushEnd if axis > selected.start() => Some(BrushMutation::ResizeEnd(axis)),
        DragGesture::BrushWindow { last_axis } => {
            let mutation = BrushMutation::Pan(axis - *last_axis);
            *last_axis = axis;
            Some(mutation)
        }
        DragGesture::BrushStart
        | DragGesture::BrushEnd
        | DragGesture::Ruler { .. }
        | DragGesture::Detail { .. } => None,
    }
}

pub(crate) fn track_chart_frame(
    panel: &MetricPanel,
    selected: Option<AxisRange>,
    visible_runs: &[RunRef],
) -> Option<(Arc<CurveSnapshot>, u64, Viewport)> {
    let detail = panel.detail.as_ref()?;
    let detail_viewport = chart::detail_viewport(detail, selected, Some(visible_runs));
    if detail_viewport
        .is_none_or(|viewport| selected.is_some_and(|selected| viewport.x != selected))
        && let Some(overview) = panel.overview.as_ref()
        && let Some(viewport) = chart::detail_viewport(overview, selected, Some(visible_runs))
        && selected.is_none_or(|selected| viewport.x == selected)
    {
        return Some((
            Arc::clone(overview),
            panel.overview_revision.saturating_mul(2).saturating_add(1),
            viewport,
        ));
    }
    let detail_viewport = detail_viewport?;
    Some((
        Arc::clone(detail),
        panel.detail_revision.saturating_mul(2),
        detail_viewport,
    ))
}

pub(crate) fn axis_menu_item(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    label: &str,
    selected: bool,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    components::popover_menu_item(id, label, Some(icon), theme).when(selected, |item| {
        item.font_weight(gpui::FontWeight::SEMIBOLD)
    })
}

pub(crate) fn format_tick(value: f64) -> String {
    if value.abs() >= 1_000_000. || (value != 0. && value.abs() < 0.001) {
        format!("{value:.2e}")
    } else if value.fract() == 0. {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}

pub(crate) fn format_axis_tick(axis: CurveAxis, value: f64) -> String {
    match axis {
        CurveAxis::Step => format_tick(value),
        CurveAxis::AbsoluteTime => format_utc_clock(value),
    }
}

pub(crate) fn format_ruler_coordinate(axis: CurveAxis, value: f64) -> String {
    match axis {
        CurveAxis::Step if value.is_finite() => format!("{value:.0}"),
        CurveAxis::Step => "—".to_owned(),
        CurveAxis::AbsoluteTime => format_utc_clock(value),
    }
}

pub(crate) fn format_utc_clock(value: f64) -> String {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return "—".to_owned();
    }
    let millis = value.round() as i64;
    let within_day = millis.rem_euclid(86_400_000);
    let hour = within_day / 3_600_000;
    let minute = within_day / 60_000 % 60;
    let second = within_day / 1_000 % 60;
    let millisecond = within_day % 1_000;
    if second == 0 && millisecond == 0 {
        format!("{hour:02}:{minute:02}")
    } else if millisecond == 0 {
        format!("{hour:02}:{minute:02}:{second:02}")
    } else {
        format!("{hour:02}:{minute:02}:{second:02}.{millisecond:03}")
    }
}

pub(crate) fn hover_value_label(
    axis: CurveAxis,
    hover: &HoverPoint,
    delta: Option<f64>,
    run_name: &str,
) -> String {
    let value = delta.map_or_else(
        || format!("{:.2}", hover.value),
        |delta| format!("{:.2} ({})", hover.value, format_signed_delta(delta, 2)),
    );
    let coordinate = match axis {
        CurveAxis::Step => hover.axis_value.to_string(),
        CurveAxis::AbsoluteTime => format_utc_clock(hover.axis_value as f64),
    };
    format!("{coordinate}: {value} {run_name}")
}

pub(crate) fn track_tooltip_width<'a>(labels: impl IntoIterator<Item = &'a str>) -> f32 {
    let characters = labels
        .into_iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or_default();
    (characters as f32 * 6.5 + 28.).clamp(104., 248.)
}

pub(crate) fn format_signed_delta(delta: f64, precision: usize) -> String {
    let sign = if delta.is_sign_negative() { '−' } else { '+' };
    format!("{sign}{:.precision$}", delta.abs())
}

pub(crate) fn baseline_delta(
    panel: &MetricPanel,
    baseline: &RunRef,
    hover: &HoverPoint,
) -> Option<f64> {
    if &hover.run_ref == baseline {
        return None;
    }
    let curve = panel
        .detail
        .as_ref()?
        .series
        .iter()
        .find(|curve| &curve.run_ref == baseline)?;
    let point = curve
        .chart_series
        .as_ref()?
        .points()
        .iter()
        .min_by_key(|point| (point.x as i64).abs_diff(hover.axis_value))?;
    Some(hover.value - point.y)
}

pub(crate) fn reasons_label(reasons: &[EvidenceReason]) -> String {
    if reasons.is_empty() {
        return String::new();
    }
    format!(
        " · {}",
        reasons
            .iter()
            .map(|reason| format!("{reason:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

impl AnalysisWorkspace {
    fn set_axis_picker_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.axis_picker_open = open;
        self.metric_picker_open = false;
        self.metric_filter.update(cx, |input, _| input.clear());
        cx.emit(AnalysisWorkspaceEvent::DismissOtherPopovers);
        cx.notify();
    }

    pub(crate) fn render_axis_picker(
        &mut self,
        theme: ViewerTheme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let absolute = self.curve_axis() == CurveAxis::AbsoluteTime;
        let picker_open = self.axis_picker_open;
        let mut picker = div().relative().flex().items_center().child(
            components::top_bar_icon_button("axis-picker", theme, picker_open, false)
                .size(theme.spacing.control_height)
                .debug_selector(|| "axis-picker".to_owned())
                .tooltip(components::label_tooltip(
                    if absolute { "Absolute time" } else { "Step" },
                    theme,
                ))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.set_axis_picker_open(!picker_open, cx);
                        cx.stop_propagation();
                    }),
                )
                .on_click(cx.listener(move |this, event, _, cx| {
                    if matches!(event, gpui::ClickEvent::Keyboard(_)) {
                        this.set_axis_picker_open(!picker_open, cx);
                    }
                }))
                .child(components::icon(
                    if absolute {
                        IconName::Clock
                    } else {
                        IconName::ArrowUp
                    },
                    theme,
                )),
        );
        if picker_open {
            picker = picker.child(
                div().absolute().top_0().left_0().child(deferred(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .snap_to_window_with_margin(px(8.))
                        .offset(point(
                            px(0.),
                            px(BRUSH_ROW_HEIGHT - BRUSH_CONTENT_TOP_PADDING + 4.),
                        ))
                        .child(
                            components::popover(theme)
                                .id("axis-menu")
                                .debug_selector(|| "axis-menu".to_owned())
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    if this.dismiss_popovers(cx) {
                                        cx.notify();
                                    }
                                }))
                                .w(px(160.))
                                .p_1()
                                .flex()
                                .flex_col()
                                .text_xs()
                                .child(
                                    axis_menu_item(
                                        "axis-step",
                                        IconName::ArrowUp,
                                        "Step",
                                        !absolute,
                                        theme,
                                    )
                                    .debug_selector(|| "axis-step".to_owned())
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.axis_picker_open = false;
                                            cx.emit(AnalysisWorkspaceEvent::Command(
                                                WorkbenchCommand::SelectAxis(AlignmentAxis::Step),
                                            ));
                                            cx.notify();
                                        },
                                    )),
                                )
                                .child(
                                    axis_menu_item(
                                        "axis-time",
                                        IconName::Clock,
                                        "Absolute time",
                                        absolute,
                                        theme,
                                    )
                                    .debug_selector(|| "axis-time".to_owned())
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.axis_picker_open = false;
                                            cx.emit(AnalysisWorkspaceEvent::Command(
                                                WorkbenchCommand::SelectAxis(
                                                    AlignmentAxis::ElapsedTime,
                                                ),
                                            ));
                                            cx.notify();
                                        },
                                    )),
                                ),
                        ),
                )),
            );
        }
        picker
    }
    pub(crate) fn render_ruler(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = ViewerTheme::for_appearance(window.appearance());
        let (selected, home) = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
            .map_or((None, None), |brush| {
                (Some(brush.selected()), Some(brush.home()))
            });
        let mut ticks = selected
            .map(|range| seex_chart_core::linear_ticks(range, 6))
            .unwrap_or_default();
        if let (Some(home), Some(step)) = (
            home,
            ticks
                .windows(2)
                .next()
                .map(|pair| pair[1] - pair[0])
                .filter(|step| step.is_finite() && *step > 0.),
        ) && let Some(first) = ticks.first().copied()
        {
            let mut preceding = Vec::new();
            let mut value = first - step;
            while value >= home.start() - step * 1e-10 && preceding.len() < 8 {
                preceding.push(if value == -0. { 0. } else { value });
                value -= step;
            }
            if first - home.start() > step * 1e-10
                && first - home.start() <= step * 8.
                && preceding
                    .last()
                    .is_none_or(|tick| (*tick - home.start()).abs() > step * 1e-10)
            {
                preceding.push(home.start());
            }
            preceding.reverse();
            preceding.extend(ticks);
            ticks = preceding;
        }
        let minor_ticks = ticks
            .windows(2)
            .flat_map(|pair| {
                let step = (pair[1] - pair[0]) / 5.;
                (1..5).map(move |index| pair[0] + step * f64::from(index))
            })
            .collect::<Vec<_>>();
        let axis = self.curve_axis();
        let interaction = self.interaction.clone();
        let hover_axis = self.hover_cursor_axis();
        let locked_cursor = interaction.locked_cursor;
        let plot_left = self.metric_sidebar_width(theme);
        let plot_left_px = f32::from(plot_left);
        let (_, plot_width) = self.ruler_plot_geometry(window, cx);
        let plot_width = f32::from(plot_width);
        let ruler_width = plot_left_px + plot_width;
        div()
            .id("viewport-ruler")
            .debug_selector(|| "viewport-ruler".to_owned())
            .flex_1()
            .h_full()
            .relative()
            .flex()
            .cursor_crosshair()
            .child(
                div()
                    .absolute()
                    .size_full()
                    .relative()
                    .text_xs()
                    .text_color(theme.colors.text_muted)
                    .children(selected.into_iter().flat_map(|range| {
                        minor_ticks
                            .iter()
                            .copied()
                            .enumerate()
                            .filter_map(move |(index, tick)| {
                                let ratio = ((tick - range.start()) / range.span()) as f32;
                                let left = plot_left_px + ratio * plot_width;
                                (-0.5..=ruler_width + 0.5).contains(&left).then(|| {
                                    div()
                                        .id(SharedString::from(format!("ruler-minor-tick-{index}")))
                                        .debug_selector(move || format!("ruler-minor-tick-{index}"))
                                        .absolute()
                                        .left(px(left.clamp(0., ruler_width)))
                                        .bottom_0()
                                        .w(px(1.))
                                        .h(px(5.))
                                        .bg(theme.colors.border)
                                })
                            })
                    }))
                    .children(selected.into_iter().flat_map(|range| {
                        ticks
                            .iter()
                            .copied()
                            .enumerate()
                            .filter_map(move |(index, tick)| {
                                let ratio = ((tick - range.start()) / range.span()) as f32;
                                let left = plot_left_px + ratio * plot_width;
                                let label_offset = if ratio > 0.9 { px(-56.) } else { px(4.) };
                                (-0.5..=ruler_width + 0.5).contains(&left).then(|| {
                                    div()
                                        .id(SharedString::from(format!("ruler-major-tick-{index}")))
                                        .debug_selector(move || format!("ruler-major-tick-{index}"))
                                        .absolute()
                                        .left(px(left.clamp(0., ruler_width)))
                                        .top_0()
                                        .bottom_0()
                                        .w(px(1.))
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "ruler-major-mark-{index}"
                                                )))
                                                .debug_selector(move || {
                                                    format!("ruler-major-mark-{index}")
                                                })
                                                .absolute()
                                                .bottom_0()
                                                .w(px(1.))
                                                .h(px(8.))
                                                .bg(theme.colors.text_muted),
                                        )
                                        .child(
                                            div()
                                                .absolute()
                                                .top(px(1.))
                                                .ml(label_offset)
                                                .whitespace_nowrap()
                                                .child(format_axis_tick(axis, tick)),
                                        )
                                })
                            })
                    })),
            )
            .children(selected.map(|range| {
                div()
                    .absolute()
                    .size_full()
                    .child(
                        chart::cursor_canvas(None, range, hover_axis, locked_cursor, true)
                            .absolute()
                            .left(plot_left)
                            .right_0()
                            .top_0()
                            .bottom_0(),
                    )
                    .child(
                        div()
                            .id("ruler-hit-area")
                            .occlude()
                            .debug_selector(|| "ruler-hit-area".to_owned())
                            .absolute()
                            .size_full()
                            .on_scroll_wheel(cx.listener(
                                |this, event: &ScrollWheelEvent, window, cx| {
                                    this.scroll_ruler(event, window, cx);
                                },
                            ))
                            .on_mouse_move(cx.listener(
                                move |this, event: &MouseMoveEvent, window, cx| {
                                    cx.emit(AnalysisWorkspaceEvent::Interaction(
                                        WorkspaceInteractionEvent::TrackPointerHover(None),
                                    ));
                                    this.track_hovers.clear();
                                    let hover = this.ruler_axis_at(event.position, window, cx);
                                    cx.emit(AnalysisWorkspaceEvent::Interaction(
                                        WorkspaceInteractionEvent::RulerHover(hover),
                                    ));
                                    if event.dragging() {
                                        this.move_ruler_drag(event, window, cx);
                                    } else {
                                        cx.notify();
                                    }
                                },
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    this.begin_ruler_drag(event, cx);
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                                    this.finish_ruler_drag(Some(event.position), window, cx);
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                                    this.finish_ruler_drag(Some(event.position), window, cx);
                                }),
                            )
                            .on_click(cx.listener(move |this, event, window, cx| {
                                let position = match event {
                                    gpui::ClickEvent::Mouse(event) => event.up.position,
                                    gpui::ClickEvent::Keyboard(_) => return,
                                };
                                this.drag.take();
                                let cursor = this.ruler_axis_at(position, window, cx);
                                cx.emit(AnalysisWorkspaceEvent::Interaction(
                                    WorkspaceInteractionEvent::LockedCursor(cursor),
                                ));
                                cx.notify();
                            }))
                            .on_hover(cx.listener(|_, hovered, _, cx| {
                                if !hovered {
                                    cx.emit(AnalysisWorkspaceEvent::Interaction(
                                        WorkspaceInteractionEvent::RulerHover(None),
                                    ));
                                    cx.notify();
                                }
                            })),
                    )
            }))
            .children(selected.zip(hover_axis).map(|(range, value)| {
                let ratio = ((value - range.start()) / range.span()).clamp(0., 1.) as f32;
                let label = format_ruler_coordinate(axis, value);
                let width = px((label.chars().count() as f32 * 7. + 14.).clamp(36., 160.));
                let offset = if ratio < 0.08 {
                    px(0.)
                } else if ratio > 0.92 {
                    -width
                } else {
                    -width / 2.
                };
                components::tooltip(theme)
                    .id("ruler-hover-tooltip")
                    .debug_selector(|| "ruler-hover-tooltip".to_owned())
                    .absolute()
                    .top(px(5.))
                    .left(px(plot_left_px + ratio * plot_width))
                    .ml(offset)
                    .w(width)
                    .h(px(18.))
                    .rounded(px(9.))
                    .border_color(theme.colors.accent)
                    .bg(theme.colors.accent)
                    .text_color(theme.colors.accent_text)
                    .text_xs()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px_2()
                    .py_0()
                    .child(label)
            }))
    }
    pub(crate) fn ruler_axis_at(
        &self,
        position: gpui::Point<gpui::Pixels>,
        window: &Window,
        _cx: &App,
    ) -> Option<f64> {
        let range = self.active_navigation()?.brush()?.selected();
        let (left, width) = self.ruler_plot_geometry(window, _cx);
        let ratio = (f64::from(position.x - left) / f64::from(width)).clamp(0., 1.);
        Some(range.start() + range.span() * ratio)
    }
    pub(crate) fn ruler_plot_geometry(
        &self,
        window: &Window,
        _cx: &App,
    ) -> (gpui::Pixels, gpui::Pixels) {
        let theme = ViewerTheme::for_appearance(window.appearance());
        let left = if self.sidebar_visible {
            self.sidebar_width
        } else {
            px(0.)
        } + self.metric_sidebar_width(theme);
        let width = (window.viewport_size().width - left).max(px(1.));
        (left, width)
    }
    pub(crate) fn scroll_ruler(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(16.));
        let delta = if delta.x.abs() > delta.y.abs() {
            f32::from(delta.x)
        } else {
            f32::from(delta.y)
        };
        let Some(before) = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
            .map(|brush| brush.selected())
        else {
            return;
        };
        let zooming = event.modifiers.platform || event.modifiers.control;
        let Some(mut preview) = self.active_navigation().cloned() else {
            return;
        };
        let transformed = if zooming {
            self.ruler_axis_at(event.position, window, cx)
                .is_some_and(|anchor| {
                    let factor = f64::from((-delta * 0.002).exp().clamp(0.5, 2.));
                    if preview.zoom_at(anchor, factor) {
                        cx.emit(AnalysisWorkspaceEvent::Command(
                            WorkbenchCommand::ZoomViewport { anchor, factor },
                        ));
                        true
                    } else {
                        false
                    }
                })
        } else {
            let (_, width) = self.ruler_plot_geometry(window, cx);
            let axis_delta = -f64::from(delta) * before.span() / f64::from(width);
            if preview.pan_by(axis_delta) {
                cx.emit(AnalysisWorkspaceEvent::Command(
                    WorkbenchCommand::PanViewport(axis_delta),
                ));
                true
            } else {
                false
            }
        };
        if transformed {
            if zooming {
                self.defer_metric_repaint(cx);
            }
            self.schedule_detail_refresh(cx);
            cx.stop_propagation();
        }
    }
    pub(crate) fn begin_ruler_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let x = f64::from(event.position.x);
        self.drag = Some(DragGesture::Ruler {
            origin_x: x,
            last_x: x,
            moved: false,
        });
        cx.notify();
    }
    pub(crate) fn move_ruler_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(DragGesture::Ruler {
            origin_x,
            last_x,
            mut moved,
        }) = self.drag.clone()
        else {
            return;
        };
        let current_x = f64::from(event.position.x);
        if !moved && (current_x - origin_x).abs() < 3. {
            return;
        }
        moved = true;
        let previous = point(px(last_x as f32), event.position.y);
        let delta = self
            .ruler_axis_at(previous, window, cx)
            .zip(self.ruler_axis_at(event.position, window, cx))
            .map(|(previous, current)| previous - current);
        if let Some(delta) = delta {
            cx.emit(AnalysisWorkspaceEvent::Command(
                WorkbenchCommand::PanViewport(delta),
            ));
        }
        self.drag = Some(DragGesture::Ruler {
            origin_x,
            last_x: current_x,
            moved,
        });
        cx.notify();
    }
    pub(crate) fn finish_ruler_drag(
        &mut self,
        position: Option<gpui::Point<gpui::Pixels>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let drag = self.drag.take();
        cx.notify();
        let Some(DragGesture::Ruler { moved, .. }) = drag else {
            return;
        };
        if moved {
            cx.emit(AnalysisWorkspaceEvent::RequestDetail);
        } else if let Some(position) = position {
            let cursor = self.ruler_axis_at(position, window, cx);
            cx.emit(AnalysisWorkspaceEvent::Interaction(
                WorkspaceInteractionEvent::LockedCursor(cursor),
            ));
        }
        cx.notify();
    }
}
