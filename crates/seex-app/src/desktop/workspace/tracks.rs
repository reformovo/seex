use std::ops::Range;

use crate::workbench::panel_reads::{MetricPanelId, PanelReadMode};

pub(crate) const METRIC_TRACK_VERTICAL_PADDING: f32 = 4.;
pub(crate) const METRIC_TRACK_SEPARATOR_WIDTH: f32 = 1.;
pub(crate) const BRUSH_ROW_HEIGHT: f32 = 40.;
pub(crate) const BRUSH_CONTENT_TOP_PADDING: f32 = 6.;
const MAX_PROGRESSIVE_OVERVIEWS: usize = 4;

pub(crate) fn detail_query_width(logical_width: u32, interactive: bool) -> u32 {
    if interactive {
        logical_width
    } else {
        logical_width.div_ceil(2).clamp(256, 1_024)
    }
}

fn prioritized_runs(
    baseline: Option<&RunRef>,
    emphasized: Option<&RunRef>,
    pinned: &[RunRef],
    visible: &[RunRef],
) -> Vec<RunRef> {
    let mut ordered = Vec::with_capacity(visible.len());
    for run in baseline
        .into_iter()
        .chain(emphasized)
        .chain(pinned)
        .chain(visible)
    {
        if visible.contains(run) && !ordered.contains(run) {
            ordered.push(run.clone());
        }
    }
    ordered
}

#[derive(Clone, Debug)]
pub(crate) enum DragGesture {
    BrushStart,
    BrushEnd,
    BrushWindow {
        last_axis: f64,
    },
    Ruler {
        origin_x: f64,
        last_x: f64,
        moved: bool,
    },
    Detail {
        panel_id: MetricPanelId,
        origin_x: f64,
        last_x: f64,
        moved: bool,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct MetricResize {
    pub panel_id: MetricPanelId,
    pub start_y: gpui::Pixels,
    pub start_height: f32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrackViewport {
    pub visible: Range<usize>,
    pub overview: Range<usize>,
    pub logical_width_bits: u32,
    pub physical_width: u32,
}

#[derive(Clone, Debug)]
pub(crate) enum ScheduledPanelRead {
    Overview {
        panel_id: MetricPanelId,
        runs: Vec<RunRef>,
        mode: PanelReadMode,
    },
    Detail {
        panel_id: MetricPanelId,
        viewport: AlignmentViewport,
        logical_width: u32,
    },
}

#[derive(Clone)]
pub(crate) struct MetricTrackRowSnapshot {
    panel: MetricPanel,
    chart: Option<gpui::Entity<chart::DetailChart>>,
    tooltip: Option<MetricTrackTooltip>,
    selected: bool,
    resizing: bool,
    metric_sidebar_compact: bool,
    visible_run_count: usize,
    resolved_run_count: usize,
    drawable_run_count: usize,
    frame_quality: Option<CurveFrameQuality>,
}

#[derive(Clone)]
struct MetricTrackTooltip {
    x: gpui::Pixels,
    y: gpui::Pixels,
    align_left: bool,
    color_index: usize,
    label: String,
}

fn snapshot_has_density(
    snapshot: &CurveSnapshot,
    selected: seex_plot::AxisRange,
    visible_runs: &[RunRef],
    target: u32,
) -> bool {
    if visible_runs
        .iter()
        .any(|run| !snapshot.series.iter().any(|curve| &curve.run_ref == run))
    {
        return false;
    }
    snapshot
        .series
        .iter()
        .filter(|curve| visible_runs.contains(&curve.run_ref))
        .filter_map(|curve| curve.chart_series.as_ref().map(|series| (curve, series)))
        .all(|(curve, series)| {
            !curve.downsampled()
                || series
                    .points()
                    .iter()
                    .filter(|point| point.x >= selected.start() && point.x <= selected.end())
                    .take(target as usize)
                    .count()
                    >= target as usize
        })
}

pub(crate) fn panel_detail_needs_query(
    panel: &MetricPanel,
    viewport: AlignmentViewport,
    query_width: u32,
    selected: seex_plot::AxisRange,
    visible_runs: &[RunRef],
) -> bool {
    if panel.is_pending(ReadKind::Detail) {
        return false;
    }
    let target = detail_budget(query_width);
    if panel.detail_covers(viewport)
        && panel
            .detail
            .as_ref()
            .is_some_and(|detail| snapshot_has_density(detail, selected, visible_runs, target))
    {
        return false;
    }
    if panel.requested_detail_viewport == Some(viewport) && panel.logical_width == query_width {
        return false;
    }
    if panel.detail.is_none() && !panel.needs_detail(viewport, query_width) {
        return false;
    }
    !panel
        .overview
        .as_ref()
        .is_some_and(|overview| snapshot_has_density(overview, selected, visible_runs, target))
}

use std::collections::HashSet;
use std::rc::Rc;

use crate::data::query::{CurveSnapshot, detail_budget, overview_budget};
use crate::data::worker::ReadKind;
use crate::domain::RunRef;
use crate::workbench::MetricPanel;
use gpui::{
    App, Context, Corner, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ScrollWheelEvent, SharedString, WeakEntity, Window, anchored, deferred, div, point, prelude::*,
    px,
};
use seex::AlignmentViewport;
use seex::MetricKey;
use seex_plot::CanvasSize;

use super::super::chart;
use super::super::command::WorkbenchCommand;
use super::super::components::{self, IconName, ResizeEdge, TextInput, resize_handle};
use super::{
    CurveFrameQuality, baseline_delta, hover_value_label, track_chart_frame, track_tooltip_width,
};

impl super::AnalysisWorkspace {
    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn metric_row_counts(&self, panel_id: &MetricPanelId) -> Option<(usize, usize)> {
        self.track_rows
            .iter()
            .find(|row| &row.panel.panel_id == panel_id)
            .map(|row| (row.visible_run_count, row.drawable_run_count))
    }

    pub(crate) fn reset_metric_track_schedule(&mut self, cx: &mut Context<Self>) {
        let panel_count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.views.active().panels.len());
        self.metric_scroll.reset(panel_count);
        *self.track_viewport.borrow_mut() = TrackViewport::default();
        cx.notify();
    }

    pub(crate) fn update_track_viewport(
        &mut self,
        start: usize,
        visible_len: usize,
        cx: &mut Context<Self>,
    ) {
        let panel_count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.views.active().panels.len());
        let start = start.min(panel_count);
        let visible = start..start.saturating_add(visible_len).min(panel_count);
        let current = self.track_viewport.borrow();
        let scrolls_down = current.visible.is_empty()
            || visible.start > current.visible.start
            || (visible.start == current.visible.start
                && current.overview.start >= current.visible.start);
        let overview = if visible.is_empty() {
            Range::default()
        } else if scrolls_down {
            visible.start..visible.end.saturating_add(2).min(panel_count)
        } else {
            visible.start.saturating_sub(2)..visible.end
        };
        drop(current);
        let next = TrackViewport {
            visible,
            overview,
            logical_width_bits: self.overview_logical_width.max(1.).to_bits(),
            physical_width: self.overview_width.max(1),
        };
        if *self.track_viewport.borrow() == next {
            return;
        }
        let (visible_panel_ids, mut overview_panel_ids) = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                let view = snapshot.views.active();
                let visible = view.panels[next.visible.clone()]
                    .iter()
                    .map(|panel| panel.panel_id.clone())
                    .collect::<Vec<_>>();
                let overview = view.panels[next.overview.clone()]
                    .iter()
                    .map(|panel| panel.panel_id.clone())
                    .collect::<Vec<_>>();
                (visible, overview)
            })
            .unwrap_or_default();
        if let Some(selected) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.views.active().selected_panel_id.clone())
            && !overview_panel_ids.contains(&selected)
        {
            overview_panel_ids.push(selected);
        }
        *self.track_viewport.borrow_mut() = next;
        cx.emit(super::AnalysisWorkspaceEvent::VisibleCurvesChanged {
            details: visible_panel_ids,
            overviews: overview_panel_ids,
        });
        let requests = self.reconcile_track_schedule(cx);
        if !requests.is_empty() {
            cx.emit(super::AnalysisWorkspaceEvent::ScheduleReads(requests));
        }
        cx.notify();
    }

    pub(crate) fn sync_track_viewport_from_layout(&mut self, cx: &mut Context<Self>) {
        let panel_count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.views.active().panels.len());
        let viewport = self.metric_scroll.viewport_bounds();
        if panel_count == 0 || viewport.size.height <= gpui::Pixels::ZERO {
            return;
        }
        let mut visible: Option<Range<usize>> = None;
        for index in self.metric_scroll.logical_scroll_top().item_ix..panel_count {
            let Some(bounds) = self.metric_scroll.bounds_for_item(index) else {
                break;
            };
            if bounds.top() >= viewport.bottom() {
                break;
            }
            if bounds.bottom() > viewport.top() {
                visible = Some(match visible {
                    Some(range) => range.start..index + 1,
                    None => index..index + 1,
                });
            }
        }
        let visible = visible.unwrap_or_default();
        self.update_track_viewport(visible.start, visible.len(), cx);
    }

    pub(crate) fn metric_sidebar_width(
        &self,
        theme: super::super::theme::ViewerTheme,
    ) -> gpui::Pixels {
        if self.metric_sidebar_compact {
            px(48.)
        } else {
            theme.spacing.sidebar_width
        }
    }

    fn select_metric(&mut self, metric_key: MetricKey, cx: &mut Context<Self>) {
        self.metric_picker_open = false;
        self.metric_filter.update(cx, |input, _| input.clear());
        cx.emit(super::AnalysisWorkspaceEvent::Command(
            WorkbenchCommand::SelectMetric(metric_key),
        ));
        cx.notify();
    }

    fn set_metric_picker_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.metric_picker_open = open;
        self.axis_picker_open = false;
        self.metric_filter.update(cx, |input, _| input.clear());
        cx.emit(super::AnalysisWorkspaceEvent::DismissOtherPopovers);
        if open {
            self.metric_filter_focus.focus(window);
        }
        cx.notify();
    }

    pub(super) fn dismiss_popovers(&mut self, cx: &mut Context<Self>) -> bool {
        let dismissed = self.metric_picker_open || self.axis_picker_open;
        self.metric_picker_open = false;
        self.axis_picker_open = false;
        dismissed.then(|| cx.notify()).is_some()
    }

    pub(crate) fn begin_metric_resize(
        &mut self,
        panel_id: MetricPanelId,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(start_height) = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .views
                .active_panel(&panel_id)
                .map(|panel| panel.row_height)
        }) else {
            return;
        };
        self.metric_resize = Some(MetricResize {
            panel_id,
            start_y: event.position.y,
            start_height,
        });
        cx.stop_propagation();
        cx.notify();
    }
    pub(crate) fn move_metric_resize(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(resize) = self.metric_resize.clone() else {
            return;
        };
        let height = resize.start_height + f32::from(event.position.y - resize.start_y);
        cx.emit(super::AnalysisWorkspaceEvent::Command(
            WorkbenchCommand::ResizeMetric {
                panel_id: resize.panel_id,
                height,
            },
        ));
        cx.notify();
    }
    pub(crate) fn finish_metric_resize(&mut self, cx: &mut Context<Self>) {
        if self.metric_resize.take().is_some() {
            cx.notify();
        }
    }
    pub(crate) fn remove_metric_panel(&mut self, panel_id: &MetricPanelId, cx: &mut Context<Self>) {
        cx.emit(super::AnalysisWorkspaceEvent::Command(
            WorkbenchCommand::RemoveMetric(panel_id.clone()),
        ));
        cx.notify();
    }
    pub(crate) fn on_metric_filter_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let metric_filter = self.metric_filter.clone();
        let filter_empty = metric_filter.read(cx).text().is_empty();
        match event.keystroke.key.as_str() {
            "escape" if filter_empty => {
                self.metric_picker_open = false;
                self.metric_filter_focus.focus(window);
            }
            "escape" => {
                metric_filter.update(cx, |input, _| {
                    input.clear();
                });
            }
            _ if metric_filter.update(cx, |input, cx| {
                let handled = input.edit(event).handled;
                if handled {
                    cx.notify();
                }
                handled
            }) => {}
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub(crate) fn render_metric_picker(
        &mut self,
        available: Vec<MetricKey>,
        selected: &HashSet<MetricKey>,
        theme: super::super::theme::ViewerTheme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let available = available
            .into_iter()
            .filter(|metric| !selected.contains(metric))
            .collect::<Vec<_>>();
        let metric_filter_entity = self.metric_filter.clone();
        let filter_focus = self.metric_filter_focus.clone();
        let picker_open = self.metric_picker_open;
        let metric_filter = metric_filter_entity.read(cx).text().to_owned();
        let query = metric_filter.trim().to_lowercase();
        let candidates = available
            .iter()
            .filter(|metric| query.is_empty() || metric.as_str().to_lowercase().contains(&query))
            .cloned()
            .collect::<Vec<_>>();
        let mut picker = div().relative().flex().items_center().child(
            components::top_bar_icon_button("add-metric", theme, picker_open, false)
                .size(theme.spacing.control_height)
                .debug_selector(|| "add-metric".to_owned())
                .tooltip(components::label_tooltip("Add Metric", theme))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.set_metric_picker_open(!picker_open, window, cx);
                        cx.stop_propagation();
                    }),
                )
                .on_click(cx.listener(move |this, event, window, cx| {
                    if matches!(event, gpui::ClickEvent::Keyboard(_)) {
                        this.set_metric_picker_open(!picker_open, window, cx);
                    }
                }))
                .child(components::icon(IconName::Plus, theme)),
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
                                .id("metric-picker")
                                .debug_selector(|| "metric-picker".to_owned())
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    if this.dismiss_popovers(cx) {
                                        cx.notify();
                                    }
                                }))
                                .w(px(180.))
                                .max_h(px(280.))
                                .p_1()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .text_xs()
                                .child(
                                    div()
                                        .id("metric-filter")
                                        .debug_selector(|| "metric-filter".to_owned())
                                        .track_focus(&filter_focus)
                                        .relative()
                                        .h(theme.spacing.control_height)
                                        .px_2()
                                        .border_1()
                                        .border_color(theme.colors.border)
                                        .rounded(theme.spacing.corner_radius)
                                        .flex()
                                        .items_center()
                                        .cursor_text()
                                        .on_key_down(cx.listener(Self::on_metric_filter_key))
                                        .text_color(if metric_filter.is_empty() {
                                            theme.colors.text_muted
                                        } else {
                                            theme.colors.text
                                        })
                                        .child(if metric_filter.is_empty() {
                                            "Filter available metrics".to_owned()
                                        } else {
                                            metric_filter.clone()
                                        })
                                        .child(TextInput::cursor_target(
                                            metric_filter_entity.clone(),
                                            filter_focus.clone(),
                                            px(8.),
                                        )),
                                )
                                .child(
                                    div()
                                        .id("metric-candidates")
                                        .debug_selector(|| "metric-candidates".to_owned())
                                        .max_h(px(220.))
                                        .overflow_y_scroll()
                                        .flex()
                                        .flex_col()
                                        .children(candidates.iter().map(|metric| {
                                            let action_metric = metric.clone();
                                            div()
                                                .id(SharedString::from(format!(
                                                    "metric-candidate:{}",
                                                    metric.as_str()
                                                )))
                                                .debug_selector({
                                                    let metric = metric.clone();
                                                    move || {
                                                        format!(
                                                            "metric-candidate:{}",
                                                            metric.as_str()
                                                        )
                                                    }
                                                })
                                                .h(px(24.))
                                                .flex_none()
                                                .px_2()
                                                .rounded(theme.spacing.corner_radius)
                                                .flex()
                                                .items_center()
                                                .cursor_pointer()
                                                .hover(|style| style.bg(theme.colors.element_hover))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.select_metric(action_metric.clone(), cx);
                                                }))
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .child(metric.as_str().to_owned())
                                        }))
                                        .children(candidates.is_empty().then(|| {
                                            div()
                                                .h(px(24.))
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .text_color(theme.colors.text_muted)
                                                .child("No matching metrics")
                                        })),
                                ),
                        ),
                )),
            );
        }
        picker
    }
}

impl super::AnalysisWorkspace {
    pub(crate) fn sync_interaction(
        &mut self,
        interaction: super::InteractionSnapshot,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.interaction == interaction {
            return false;
        }
        let emphasized_changed = self.interaction.emphasized_run != interaction.emphasized_run;
        self.interaction = interaction;
        if emphasized_changed {
            self.sync_track_charts(cx);
        } else {
            self.rebuild_track_rows(cx);
        }
        cx.notify();
        emphasized_changed
    }

    fn should_schedule_panel_detail(
        &self,
        panel: &MetricPanel,
        viewport: AlignmentViewport,
        logical_width: u32,
        interactive: bool,
        selected: seex_plot::AxisRange,
        visible_runs: &[RunRef],
    ) -> bool {
        if self.detail_refresh_pending {
            return false;
        }
        panel_detail_needs_query(
            panel,
            viewport,
            detail_query_width(logical_width, interactive),
            selected,
            visible_runs,
        )
    }

    pub(crate) fn reconcile_track_schedule(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<ScheduledPanelRead> {
        let state = self.track_viewport.borrow().clone();
        if state.visible.is_empty() || state.logical_width_bits == 0 {
            return Vec::new();
        }
        let Some(session) = self.snapshot.clone() else {
            return Vec::new();
        };
        let panel_count = session.views.active().panels.len();
        let scheduled = state.visible.start.min(panel_count)..state.visible.end.min(panel_count);
        let panels = session.views.active().panels[scheduled].to_vec();
        let mut prefetched = session.views.active().panels
            [state.overview.start.min(panel_count)..state.overview.end.min(panel_count)]
            .iter()
            .filter(|panel| {
                !panels
                    .iter()
                    .any(|visible| visible.panel_id == panel.panel_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        let visible_runs = self.visible_runs.clone();
        let scheduled_ids = panels
            .iter()
            .map(|panel| panel.panel_id.clone())
            .collect::<HashSet<_>>();
        self.track_charts
            .retain(|panel_id, _| scheduled_ids.contains(panel_id));
        self.track_hovers
            .retain(|panel_id, _| scheduled_ids.contains(panel_id));
        let logical_width = f32::from_bits(state.logical_width_bits) as f64;
        let query_width = logical_width.ceil().max(1.) as u32;
        let baseline = session.views.active().baseline.clone();
        let emphasized_run = self
            .interaction
            .emphasized_run
            .clone()
            .filter(|run| visible_runs.contains(run));
        let ordered_runs = prioritized_runs(
            baseline.as_ref(),
            emphasized_run.as_ref(),
            &session.views.active().pinned_runs,
            &visible_runs,
        );
        if ordered_runs.is_empty() {
            return Vec::new();
        }
        let selected_panel = session.views.active().selected_panel_id.clone();
        let hovered_panel = self
            .interaction
            .track_pointer_hover
            .as_ref()
            .map(|(panel_id, _)| panel_id.clone());
        let interactive_panel = hovered_panel.as_ref().or(selected_panel.as_ref());
        let mut coverage_panels = panels.clone();
        if let Some(selected) = selected_panel.as_ref()
            && !coverage_panels
                .iter()
                .any(|panel| &panel.panel_id == selected)
            && let Some(panel) = session.views.active_panel(selected)
        {
            coverage_panels.push(panel.clone());
        }
        prefetched.retain(|panel| {
            !coverage_panels
                .iter()
                .any(|retained| retained.panel_id == panel.panel_id)
        });
        let visible_runs: Rc<[RunRef]> = visible_runs.into();
        for panel in &panels {
            let canvas_height = f64::from(panel.row_height)
                - f64::from(METRIC_TRACK_VERTICAL_PADDING * 2. + METRIC_TRACK_SEPARATOR_WIDTH);
            let canvas = CanvasSize::new(logical_width, canvas_height.max(1.)).ok();
            if let Some(frame) = track_chart_frame(
                panel,
                self.active_navigation()
                    .and_then(|navigation| navigation.brush())
                    .map(|brush| brush.selected()),
                &visible_runs,
            ) && let Some(canvas) = canvas
            {
                let snapshot = frame.snapshot;
                let revision = frame.revision;
                let viewport = frame.viewport;
                let cached_chart = self.track_charts.get(&panel.panel_id).cloned();
                let chart = if let Some(chart) = cached_chart {
                    chart.update(cx, |chart, cx| {
                        if chart.update(
                            snapshot.clone(),
                            revision,
                            viewport,
                            baseline.clone(),
                            emphasized_run.clone(),
                            Rc::clone(&visible_runs),
                        ) {
                            cx.notify();
                        }
                    });
                    chart
                } else {
                    let chart = cx.new(|_| {
                        chart::DetailChart::new(
                            snapshot,
                            revision,
                            viewport,
                            baseline.clone(),
                            emphasized_run.clone(),
                            Rc::clone(&visible_runs),
                        )
                    });
                    self.track_charts
                        .insert(panel.panel_id.clone(), chart.clone());
                    chart
                };
                chart.read(cx).warm_projection(canvas);
            } else {
                self.track_charts.remove(&panel.panel_id);
                self.track_hovers.remove(&panel.panel_id);
            }
        }
        self.rebuild_track_rows(cx);
        let target_budget = overview_budget(query_width);
        let resolved_count = |panel: &MetricPanel| {
            ordered_runs
                .iter()
                .filter(|run| panel.overview_run_resolved(run, target_budget))
                .count()
        };
        let next_run = |panel: &MetricPanel| {
            ordered_runs
                .iter()
                .find(|run| !panel.overview_run_resolved(run, target_budget))
                .cloned()
        };
        let pending_overviews = coverage_panels
            .iter()
            .chain(&prefetched)
            .filter(|panel| panel.is_pending(ReadKind::Overview))
            .count();
        let available_slots = MAX_PROGRESSIVE_OVERVIEWS.saturating_sub(pending_overviews);
        let schedule_overview = |panel: &MetricPanel| {
            next_run(panel).map(|run| ScheduledPanelRead::Overview {
                panel_id: panel.panel_id.clone(),
                runs: vec![run],
                mode: if panel.overview.is_some() {
                    PanelReadMode::Merge
                } else {
                    PanelReadMode::Replace
                },
            })
        };
        if coverage_panels
            .iter()
            .any(|panel| resolved_count(panel) == 0)
        {
            let mut candidates = coverage_panels
                .iter()
                .filter(|panel| resolved_count(panel) == 0 && !panel.is_pending(ReadKind::Overview))
                .collect::<Vec<_>>();
            candidates.sort_by_key(|panel| interactive_panel != Some(&panel.panel_id));
            return candidates
                .into_iter()
                .take(available_slots)
                .filter_map(schedule_overview)
                .collect();
        }
        if prefetched.iter().any(|panel| resolved_count(panel) == 0) {
            return prefetched
                .iter()
                .filter(|panel| resolved_count(panel) == 0 && !panel.is_pending(ReadKind::Overview))
                .take(available_slots)
                .filter_map(schedule_overview)
                .collect();
        }
        let coverage_incomplete = coverage_panels
            .iter()
            .any(|panel| resolved_count(panel) < ordered_runs.len());
        if coverage_incomplete {
            let minimum = coverage_panels
                .iter()
                .map(&resolved_count)
                .min()
                .unwrap_or(0);
            let mut scheduled_ids = HashSet::new();
            let mut requests = Vec::new();
            if let Some(interactive) = interactive_panel
                .and_then(|panel_id| {
                    coverage_panels
                        .iter()
                        .find(|panel| &panel.panel_id == panel_id)
                })
                .filter(|panel| {
                    available_slots > 0
                        && !panel.is_pending(ReadKind::Overview)
                        && resolved_count(panel) < ordered_runs.len()
                        && resolved_count(panel) <= minimum.saturating_add(1)
                })
                && let Some(request) = schedule_overview(interactive)
            {
                scheduled_ids.insert(interactive.panel_id.clone());
                requests.push(request);
            }
            let mut candidates = coverage_panels
                .iter()
                .filter(|panel| {
                    !panel.is_pending(ReadKind::Overview)
                        && !scheduled_ids.contains(&panel.panel_id)
                        && resolved_count(panel) < ordered_runs.len()
                })
                .collect::<Vec<_>>();
            candidates.sort_by_key(|panel| resolved_count(panel));
            requests.extend(
                candidates
                    .into_iter()
                    .take(available_slots.saturating_sub(requests.len()))
                    .filter_map(schedule_overview),
            );
            return requests;
        }
        let Some(detail_viewport) = self
            .active_navigation()
            .and_then(crate::domain::ViewNavigation::selected_viewport)
        else {
            return Vec::new();
        };
        let selected = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
            .map(|brush| brush.selected())
            .expect("detail scheduling requires an active brush");
        let is_interactive = |panel: &MetricPanel| {
            selected_panel.as_ref() == Some(&panel.panel_id)
                || hovered_panel.as_ref() == Some(&panel.panel_id)
        };
        let interactive = panels
            .iter()
            .filter(|panel| is_interactive(panel))
            .collect::<Vec<_>>();
        if interactive.iter().any(|panel| {
            panel.is_pending(ReadKind::Detail)
                || self.should_schedule_panel_detail(
                    panel,
                    detail_viewport,
                    query_width,
                    true,
                    selected,
                    &visible_runs,
                )
        }) {
            return interactive
                .into_iter()
                .filter(|panel| {
                    self.should_schedule_panel_detail(
                        panel,
                        detail_viewport,
                        query_width,
                        true,
                        selected,
                        &visible_runs,
                    )
                })
                .map(|panel| ScheduledPanelRead::Detail {
                    panel_id: panel.panel_id.clone(),
                    viewport: detail_viewport,
                    logical_width: detail_query_width(query_width, true),
                })
                .collect();
        }
        let passive = panels
            .iter()
            .filter(|panel| !is_interactive(panel))
            .collect::<Vec<_>>();
        if passive.iter().any(|panel| {
            panel.is_pending(ReadKind::Detail)
                || self.should_schedule_panel_detail(
                    panel,
                    detail_viewport,
                    query_width,
                    false,
                    selected,
                    &visible_runs,
                )
        }) {
            return passive
                .into_iter()
                .filter(|panel| {
                    self.should_schedule_panel_detail(
                        panel,
                        detail_viewport,
                        query_width,
                        false,
                        selected,
                        &visible_runs,
                    )
                })
                .map(|panel| ScheduledPanelRead::Detail {
                    panel_id: panel.panel_id.clone(),
                    viewport: detail_viewport,
                    logical_width: detail_query_width(query_width, false),
                })
                .collect();
        }
        Vec::new()
    }

    fn rebuild_track_rows(&mut self, cx: &App) {
        let Some(session) = self.snapshot.clone() else {
            self.track_rows.clear();
            return;
        };
        let selected_panel = session.views.active().selected_panel_id.as_ref();
        let catalog_backed_runs = self
            .visible_runs
            .iter()
            .filter(|run| session.contains_catalog_run(run))
            .cloned()
            .collect::<Vec<_>>();
        let visible_run_count = catalog_backed_runs.len();
        let selected = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
            .map(|brush| brush.selected());
        self.track_rows = session
            .views
            .active()
            .panels
            .iter()
            .map(|panel| {
                let frame = track_chart_frame(panel, selected, &catalog_backed_runs);
                let chart = self.track_charts.get(&panel.panel_id).cloned();
                let tooltip = chart
                    .as_ref()
                    .zip(frame.as_ref())
                    .and_then(|(chart, frame)| {
                        self.track_tooltip(&panel.panel_id, frame.snapshot.as_ref(), chart, cx)
                    });
                MetricTrackRowSnapshot {
                    panel: panel.clone(),
                    chart,
                    tooltip,
                    selected: selected_panel == Some(&panel.panel_id),
                    resizing: self
                        .metric_resize
                        .as_ref()
                        .is_some_and(|resize| resize.panel_id == panel.panel_id),
                    metric_sidebar_compact: self.metric_sidebar_compact,
                    visible_run_count,
                    resolved_run_count: catalog_backed_runs
                        .iter()
                        .filter(|run| panel.overview_coverage.contains_key(run))
                        .count(),
                    drawable_run_count: drawable_run_count(
                        frame.as_ref().map(|frame| frame.snapshot.as_ref()),
                        &catalog_backed_runs,
                    ),
                    frame_quality: frame.map(|frame| frame.quality),
                }
            })
            .collect();
    }

    fn track_tooltip(
        &self,
        panel_id: &MetricPanelId,
        snapshot: &CurveSnapshot,
        chart: &gpui::Entity<chart::DetailChart>,
        cx: &App,
    ) -> Option<MetricTrackTooltip> {
        let session = self.snapshot.as_ref()?;
        let baseline = session.views.active().baseline.as_ref();
        let emphasized_run = self
            .interaction
            .emphasized_run
            .as_ref()
            .filter(|run| self.visible_runs.contains(run));
        let locked_sidebar_callout =
            emphasized_run
                .zip(self.interaction.locked_cursor)
                .and_then(|(run, axis)| {
                    chart
                        .read(cx)
                        .points_at_axis(axis)
                        .into_iter()
                        .find(|hover| &hover.run_ref == run)
                });
        let sidebar_locked = locked_sidebar_callout.is_some();
        let hover = self.track_hovers.get(panel_id).cloned();
        let mut callouts = locked_sidebar_callout
            .map_or_else(
                || {
                    if emphasized_run.is_some() {
                        Vec::new()
                    } else {
                        hover.map_or_else(
                            || {
                                self.interaction.ruler_hover.map_or_else(Vec::new, |axis| {
                                    chart.read(cx).points_at_axis(axis)
                                })
                            },
                            |hover| vec![hover],
                        )
                    }
                },
                |hover| vec![hover],
            )
            .into_iter()
            .map(|hover| {
                let delta =
                    baseline.and_then(|baseline| baseline_delta(snapshot, baseline, &hover));
                (hover, delta)
            })
            .collect::<Vec<_>>();
        if !sidebar_locked {
            let pinned = &session.views.active().pinned_runs;
            callouts.sort_by_key(|(hover, _)| {
                if baseline == Some(&hover.run_ref) {
                    0
                } else if pinned.contains(&hover.run_ref) {
                    1
                } else {
                    2
                }
            });
        }
        let (hover, delta) = callouts.into_iter().next()?;
        let run_name = snapshot
            .series
            .iter()
            .find(|curve| curve.run_ref == hover.run_ref)?
            .run
            .name
            .as_str();
        Some(MetricTrackTooltip {
            x: hover.canvas_position.x,
            y: px(METRIC_TRACK_VERTICAL_PADDING) + hover.canvas_position.y,
            align_left: hover.align_left,
            color_index: chart::series_color_index(&hover.run_ref),
            label: hover_value_label(self.curve_axis(), &hover, delta, run_name),
        })
    }
}

pub(crate) fn drawable_run_count(
    snapshot: Option<&CurveSnapshot>,
    visible_runs: &[RunRef],
) -> usize {
    snapshot.map_or(0, |snapshot| {
        snapshot
            .series
            .iter()
            .filter(|series| {
                visible_runs.contains(&series.run_ref) && series.chart_series.is_some()
            })
            .count()
    })
}

pub(crate) fn metric_metadata(
    visible_run_count: usize,
    resolved_run_count: usize,
    drawable_count: usize,
    quality: Option<CurveFrameQuality>,
    pending: bool,
) -> String {
    if resolved_run_count < visible_run_count {
        return format!("{resolved_run_count}/{visible_run_count} runs · loading");
    }
    match (quality, pending) {
        (Some(CurveFrameQuality::Detail), true) => {
            format!("{resolved_run_count}/{visible_run_count} runs · refining")
        }
        (Some(CurveFrameQuality::Detail), false) => {
            format!("{resolved_run_count}/{visible_run_count} runs · {drawable_count} drawable")
        }
        (Some(CurveFrameQuality::Overview), true) => {
            format!("{resolved_run_count}/{visible_run_count} runs · refining")
        }
        (Some(CurveFrameQuality::Overview), false) => {
            format!("{resolved_run_count}/{visible_run_count} runs · preview")
        }
        (None, true) => format!("{resolved_run_count}/{visible_run_count} runs · loading"),
        (None, false) => format!("{resolved_run_count}/{visible_run_count} runs"),
    }
}

fn empty_metric_chart(panel_id: &MetricPanelId) -> gpui::Div {
    let panel_id = panel_id.clone();
    div()
        .debug_selector(move || format!("empty-metric-chart:{}", panel_id.as_str()))
        .size_full()
}

impl super::AnalysisWorkspace {
    pub(crate) fn render_metric_row(
        row: MetricTrackRowSnapshot,
        workspace: WeakEntity<Self>,
        theme: super::super::theme::ViewerTheme,
        cx: &mut App,
    ) -> gpui::Div {
        let panel = row.panel.clone();
        let row_height = px(panel.row_height);
        let panel_id = panel.panel_id.clone();
        let remove_id = panel_id.clone();
        let select_id = panel_id.clone();
        let drag_id = panel_id.clone();
        let finish_id = panel_id.clone();
        let resize_id = panel_id.clone();
        let resizing = row.resizing;
        let selected = row.selected;
        let visible_run_count = row.visible_run_count;
        let drawable_count = row.drawable_run_count;
        let metadata = metric_metadata(
            visible_run_count,
            row.resolved_run_count,
            drawable_count,
            row.frame_quality,
            panel.is_pending(ReadKind::Overview) || panel.is_pending(ReadKind::Detail),
        );
        let track = Self::render_metric_track(&row, workspace.clone(), theme, cx);
        let metric_sidebar_width = if row.metric_sidebar_compact {
            px(48.)
        } else {
            theme.spacing.sidebar_width
        };
        let select_workspace = workspace.clone();
        let remove_workspace = workspace.clone();
        let drag_workspace = workspace.clone();
        let move_workspace = workspace.clone();
        let click_workspace = workspace.clone();
        let up_workspace = workspace.clone();
        let up_out_workspace = workspace.clone();
        let resize_workspace = workspace;
        div()
            .relative()
            .flex()
            .w_full()
            .h(row_height)
            .min_h(row_height)
            .border_b_1()
            .border_color(theme.colors.transparent)
            .child(
                div()
                    .id(SharedString::from(format!(
                        "metric-sidebar-row:{}",
                        panel_id.as_str()
                    )))
                    .debug_selector({
                        let panel_id = panel_id.clone();
                        move || format!("metric-sidebar-row:{}", panel_id.as_str())
                    })
                    .w(metric_sidebar_width)
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_start()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .bg(if selected {
                        theme.colors.element_active
                    } else {
                        theme.colors.panel
                    })
                    .border_r_1()
                    .border_color(theme.colors.border)
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let _ = select_workspace.update(cx, |_, cx| {
                            cx.emit(super::AnalysisWorkspaceEvent::ShowInspector(
                                select_id.clone(),
                            ));
                        });
                    })
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .gap_0()
                            .text_sm()
                            .child(panel.metric_key.as_str().to_owned())
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "metric-metadata:{}",
                                        panel_id.as_str()
                                    )))
                                    .debug_selector({
                                        let panel_id = panel_id.clone();
                                        move || format!("metric-metadata:{}", panel_id.as_str())
                                    })
                                    .text_xs()
                                    .text_color(theme.colors.text_muted)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(metadata),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "remove-metric:{}",
                                panel_id.as_str()
                            )))
                            .size(px(20.))
                            .flex_none()
                            .border_1()
                            .border_color(theme.colors.transparent)
                            .rounded(theme.spacing.corner_radius)
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(0.62)
                            .hover(|style| style.opacity(1.))
                            .tab_index(0)
                            .focus(|style| style.opacity(1.))
                            .tooltip(components::label_tooltip("Remove Metric", theme))
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                let _ = remove_workspace.update(cx, |workspace, cx| {
                                    workspace.remove_metric_panel(&remove_id, cx);
                                    cx.stop_propagation();
                                });
                            })
                            .child(components::icon(IconName::Close, theme)),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "metric-track:{}",
                        panel_id.as_str()
                    )))
                    .debug_selector(move || format!("metric-track:{}", panel_id.as_str()))
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .bg(theme.colors.window)
                    .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                        let _ = drag_workspace.update(cx, |workspace, cx| {
                            workspace.begin_track_drag(drag_id.clone(), event, cx);
                        });
                    })
                    .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                        if event.dragging() {
                            let _ = move_workspace.update(cx, |workspace, cx| {
                                workspace.move_detail_drag(event, cx);
                            });
                        }
                    })
                    .on_click(move |event, _, cx| {
                        let _ = click_workspace.update(cx, |workspace, cx| {
                            workspace.finish_track_click(&finish_id, event, cx);
                        });
                    })
                    .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
                        let _ = up_workspace.update(cx, |workspace, cx| {
                            workspace.finish_moved_track_drag(cx);
                        });
                    })
                    .on_mouse_up_out(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
                        let _ = up_out_workspace.update(cx, |workspace, cx| {
                            workspace.finish_moved_track_drag(cx);
                        });
                    })
                    .child(track),
            )
            .child(
                resize_handle(
                    SharedString::from(format!("metric-resize:{}", resize_id.as_str())),
                    theme,
                    resizing,
                    ResizeEdge::Bottom,
                )
                .debug_selector({
                    let resize_id = resize_id.clone();
                    move || format!("metric-resize:{}", resize_id.as_str())
                })
                .on_mouse_down(
                    MouseButton::Left,
                    move |event: &MouseDownEvent, _, cx| {
                        let _ = resize_workspace.update(cx, |workspace, cx| {
                            workspace.begin_metric_resize(resize_id.clone(), event, cx);
                        });
                    },
                ),
            )
    }
    pub(crate) fn render_metric_track(
        row: &MetricTrackRowSnapshot,
        workspace: WeakEntity<Self>,
        theme: super::super::theme::ViewerTheme,
        _cx: &mut App,
    ) -> gpui::Div {
        let panel = &row.panel;
        let panel_id = panel.panel_id.clone();
        if row.visible_run_count == 0 {
            return empty_metric_chart(&panel_id);
        }
        if row.chart.is_none() {
            let message = if panel.is_pending(ReadKind::Detail) {
                Some("Loading viewport…")
            } else if panel.is_pending(ReadKind::Overview) {
                Some("Loading metric extent…")
            } else {
                None
            };
            let Some(message) = message else {
                return empty_metric_chart(&panel_id);
            };
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(theme.colors.text_muted)
                .child(message);
        }
        if row.drawable_run_count == 0 {
            return empty_metric_chart(&panel_id);
        }
        let Some(chart) = row.chart.clone() else {
            return empty_metric_chart(&panel_id);
        };
        let hit_panel = panel_id.clone();
        let zoom_panel = panel_id.clone();
        let leave_panel = panel_id.clone();
        let hover_workspace = workspace.clone();
        let zoom_workspace = workspace.clone();
        let leave_workspace = workspace;
        let callout_panel = panel_id.clone();
        let tooltip = row.tooltip.clone().map(move |tooltip| {
            let x = tooltip.x;
            let y = tooltip.y;
            let align_left = tooltip.align_left;
            let height = px(22.);
            let width = px(track_tooltip_width([tooltip.label.as_str()]));
            let arrow_width = px(9.);
            let total_width = width + arrow_width;
            let top = (y - height / 2.)
                .max(px(0.))
                .min((px(panel.row_height) - height).max(px(0.)));
            let left = if align_left {
                (x - total_width).max(px(0.))
            } else {
                x
            };
            div()
                .id(SharedString::from(format!(
                    "track-hover-callout:{}",
                    callout_panel.as_str()
                )))
                .debug_selector(|| "track-hover-callout".to_owned())
                .absolute()
                .left(left)
                .top(top)
                .w(total_width)
                .h(height)
                .text_color(theme.colors.text)
                .child(
                    chart::callout_shell(
                        align_left,
                        y - top,
                        theme.colors.surface,
                        theme.colors.text_muted,
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .when(align_left, |content| content.left_0().right(arrow_width))
                        .when(!align_left, |content| content.left(arrow_width).right_0())
                        .px_2()
                        .child(
                            div()
                                .h_full()
                                .min_w(px(0.))
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .child(
                                    div()
                                        .debug_selector(|| "track-tooltip-color".to_owned())
                                        .size(px(7.))
                                        .flex_none()
                                        .rounded(px(3.5))
                                        .bg(theme.colors.series_color(tooltip.color_index)),
                                )
                                .child(
                                    div()
                                        .debug_selector(|| "track-tooltip-label".to_owned())
                                        .flex_1()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(tooltip.label),
                                ),
                        ),
                )
        });
        div()
            .relative()
            .size_full()
            .py(px(METRIC_TRACK_VERTICAL_PADDING))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "metric-canvas:{}",
                        panel_id.as_str()
                    )))
                    .debug_selector({
                        let panel_id = panel_id.clone();
                        move || format!("metric-canvas:{}", panel_id.as_str())
                    })
                    .size_full()
                    .cursor_crosshair()
                    .child(chart::cached_detail_chart(chart))
                    .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                        if !event.dragging() {
                            let _ = hover_workspace.update(cx, |workspace, cx| {
                                workspace.update_track_hover(&hit_panel, event, cx);
                            });
                        }
                    })
                    .on_scroll_wheel(move |event: &ScrollWheelEvent, _, cx| {
                        let _ = zoom_workspace.update(cx, |workspace, cx| {
                            workspace.zoom_track(&zoom_panel, event, cx);
                        });
                    })
                    .on_hover(move |hovered, _, cx| {
                        if !hovered {
                            let _ = leave_workspace.update(cx, |workspace, cx| {
                                workspace.track_hovers.remove(&leave_panel);
                                if workspace
                                    .interaction
                                    .track_pointer_hover
                                    .as_ref()
                                    .is_some_and(|(panel_id, _)| panel_id == &leave_panel)
                                {
                                    cx.emit(super::AnalysisWorkspaceEvent::Interaction(
                                        super::WorkspaceInteractionEvent::TrackPointerHover(None),
                                    ));
                                }
                                cx.notify();
                            });
                        }
                    }),
            )
            .children(tooltip)
    }
    pub(crate) fn sync_track_charts(&mut self, cx: &mut Context<Self>) {
        let visible_runs: Rc<[RunRef]> = self.visible_runs.clone().into();
        let selected = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
            .map(|brush| brush.selected());
        let Some(session) = self.snapshot.clone() else {
            return;
        };
        let baseline = session.views.active().baseline.clone();
        let emphasized_run = self
            .interaction
            .emphasized_run
            .clone()
            .filter(|run| visible_runs.contains(run));
        if let Some(brush) = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
        {
            let (snapshot, revision) = session
                .views
                .active()
                .selected_panel_id
                .as_ref()
                .and_then(|panel_id| session.views.active_panel(panel_id))
                .map_or((None, 0), |panel| {
                    (panel.overview.clone(), panel.overview_revision)
                });
            self.overview_chart.update(cx, |chart, cx| {
                if chart.update(
                    brush,
                    snapshot,
                    revision,
                    emphasized_run.clone(),
                    Rc::clone(&visible_runs),
                ) {
                    cx.notify();
                }
            });
        }
        let panels = session.views.active().panels.clone();
        for panel in panels {
            let Some(chart) = self.track_charts.get(&panel.panel_id).cloned() else {
                continue;
            };
            let Some(frame) = track_chart_frame(&panel, selected, &visible_runs) else {
                continue;
            };
            chart.update(cx, |chart, cx| {
                let changed = chart.update(
                    frame.snapshot,
                    frame.revision,
                    frame.viewport,
                    baseline.clone(),
                    emphasized_run.clone(),
                    Rc::clone(&visible_runs),
                );
                if changed {
                    cx.notify();
                }
            });
        }
        self.rebuild_track_rows(cx);
    }
}
