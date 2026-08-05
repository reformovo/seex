use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use crate::data::query::CurveAxis;
use crate::domain::RunRef;
use crate::workbench::MetricPanel;
use crate::workbench::panel_reads::MetricPanelId;
use gpui::{
    App, Context, EventEmitter, FocusHandle, IntoElement, ListAlignment, ListState, Render, Task,
    Window, div, list, prelude::*, px,
};
use seex::AlignmentAxis;
use seex::MetricKey;

use super::chart::HoverPoint;
use super::components::TextInput;
use super::interaction::InteractionSnapshot;
use super::session::SessionSnapshot;
use super::{ViewerApp, chart};

pub(crate) struct AnalysisWorkspace {
    pub metric_filter_focus: FocusHandle,
    pub metric_filter: gpui::Entity<TextInput>,
    pub metric_picker_open: bool,
    pub axis_picker_open: bool,
    pub metric_sidebar_compact: bool,
    pub overview_chart: gpui::Entity<chart::OverviewChart>,
    pub track_charts: HashMap<MetricPanelId, gpui::Entity<chart::DetailChart>>,
    pub track_hovers: HashMap<MetricPanelId, HoverPoint>,
    pub track_rows: Vec<MetricTrackRowSnapshot>,
    pub metric_scroll: ListState,
    pub metric_resize: Option<MetricResize>,
    pub track_viewport: Rc<RefCell<TrackViewport>>,
    pub overview_logical_width: f32,
    pub overview_width: u32,
    pub drag: Option<DragGesture>,
    pub zoom_task: Option<Task<()>>,
    pub metric_repaint_pending: bool,
    pub detail_refresh_token: u64,
    pub detail_refresh_pending: bool,
    snapshot: Option<Arc<SessionSnapshot>>,
    interaction: InteractionSnapshot,
    visible_runs: Vec<RunRef>,
    available_metrics: Vec<MetricKey>,
    sidebar_visible: bool,
    sidebar_width: gpui::Pixels,
}

#[derive(Clone, Debug)]
pub(crate) enum AnalysisWorkspaceEvent {
    Command(super::command::WorkbenchCommand),
    DismissOtherPopovers,
    ShowInspector(MetricPanelId),
    RequestDetail,
    RequestOverview,
    ScheduleDetails(Vec<ScheduledPanelDetail>),
    Interaction(WorkspaceInteractionEvent),
}

#[derive(Clone, Debug)]
pub(crate) enum WorkspaceInteractionEvent {
    RulerHover(Option<f64>),
    TrackPointerHover(Option<(MetricPanelId, f64)>),
    LockedCursor(Option<f64>),
}

impl EventEmitter<AnalysisWorkspaceEvent> for AnalysisWorkspace {}

impl AnalysisWorkspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let overview_chart = cx.new(|_| chart::OverviewChart::default());
        let metric_scroll = ListState::new(0, ListAlignment::Top, px(480.));
        let workspace = cx.entity().downgrade();
        let scroll = metric_scroll.clone();
        metric_scroll.set_scroll_handler(move |event, _, cx| {
            let visible_len = event.visible_range.len().max(1);
            let scroll = scroll.clone();
            let workspace = workspace.clone();
            cx.defer(move |cx| {
                let start = scroll.logical_scroll_top().item_ix;
                let _ = workspace.update(cx, |workspace, cx| {
                    workspace.update_track_viewport(start, visible_len, cx);
                });
            });
        });
        Self {
            metric_filter_focus: cx.focus_handle().tab_stop(true),
            metric_filter: cx.new(|_| TextInput::default()),
            metric_picker_open: false,
            axis_picker_open: false,
            metric_sidebar_compact: false,
            overview_chart,
            track_charts: HashMap::new(),
            track_hovers: HashMap::new(),
            track_rows: Vec::new(),
            metric_scroll,
            metric_resize: None,
            track_viewport: Rc::new(RefCell::new(TrackViewport::default())),
            overview_logical_width: 1_000.,
            overview_width: 1_000,
            drag: None,
            zoom_task: None,
            metric_repaint_pending: false,
            detail_refresh_token: 0,
            detail_refresh_pending: false,
            snapshot: None,
            interaction: InteractionSnapshot::default(),
            visible_runs: Vec::new(),
            available_metrics: Vec::new(),
            sidebar_visible: true,
            sidebar_width: px(190.),
        }
    }

    pub(crate) fn update_overview_widths(
        &mut self,
        logical_width: f32,
        physical_width: u32,
        cx: &mut Context<Self>,
    ) {
        if self.overview_logical_width == logical_width && self.overview_width == physical_width {
            return;
        }
        let query_width_changed = self.overview_logical_width != logical_width;
        self.overview_logical_width = logical_width;
        self.overview_width = physical_width;
        let mut viewport = self.track_viewport.borrow_mut();
        viewport.logical_width_bits = logical_width.max(1.).to_bits();
        viewport.physical_width = physical_width.max(1);
        drop(viewport);
        if query_width_changed {
            cx.emit(AnalysisWorkspaceEvent::RequestOverview);
        }
        cx.notify();
    }

    pub(crate) fn sync(
        &mut self,
        snapshot: Arc<SessionSnapshot>,
        interaction: InteractionSnapshot,
        visible_runs: Vec<RunRef>,
        available_metrics: Vec<MetricKey>,
        sidebar_visible: bool,
        sidebar_width: gpui::Pixels,
    ) {
        self.snapshot = Some(snapshot);
        self.interaction = interaction;
        self.visible_runs = visible_runs;
        self.available_metrics = available_metrics;
        self.sidebar_visible = sidebar_visible;
        self.sidebar_width = sidebar_width;
        let panel_count = self
            .snapshot
            .as_ref()
            .map_or(0, |snapshot| snapshot.views.active().panels.len());
        if self.metric_scroll.item_count() != panel_count {
            self.metric_scroll.reset(panel_count);
        }
        if self.track_viewport.borrow().overscan.is_empty() && panel_count > 0 {
            *self.track_viewport.borrow_mut() = TrackViewport {
                visible: 0..panel_count.min(INITIAL_VISIBLE_TRACKS),
                overscan: 0..panel_count.min(INITIAL_OVERSCAN_TRACKS),
                logical_width_bits: self.overview_logical_width.max(1.).to_bits(),
                physical_width: self.overview_width.max(1),
            };
        }
    }

    fn active_navigation(&self) -> Option<&crate::domain::ViewNavigation> {
        self.snapshot
            .as_ref()
            .map(|snapshot| &snapshot.views.active().navigation)
    }

    fn curve_axis(&self) -> CurveAxis {
        match self
            .active_navigation()
            .map(crate::domain::ViewNavigation::axis)
            .unwrap_or(AlignmentAxis::Step)
        {
            AlignmentAxis::Step => CurveAxis::Step,
            AlignmentAxis::ElapsedTime => CurveAxis::ElapsedTime,
        }
    }

    fn hover_cursor_axis(&self) -> Option<f64> {
        self.interaction.ruler_hover.or_else(|| {
            self.interaction
                .track_pointer_hover
                .as_ref()
                .map(|(_, axis)| *axis)
        })
    }
}

mod interaction;
mod overview;
mod ruler;
mod tracks;

pub(super) use ruler::*;
pub(super) use tracks::*;

impl ViewerApp {
    pub(crate) fn active_navigation(&self, cx: &App) -> crate::domain::ViewNavigation {
        self.session_snapshot(cx).views.active().navigation.clone()
    }

    pub(super) fn curve_axis(&self, cx: &App) -> CurveAxis {
        match self.active_navigation(cx).axis() {
            AlignmentAxis::Step => CurveAxis::Step,
            AlignmentAxis::ElapsedTime => CurveAxis::ElapsedTime,
        }
    }
    pub(super) fn interaction_snapshot(&self, cx: &App) -> InteractionSnapshot {
        self.interaction.read(cx).snapshot()
    }
    pub(super) fn handle_workspace_event(
        &mut self,
        event: &AnalysisWorkspaceEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            AnalysisWorkspaceEvent::Command(command) => {
                self.dispatch_workbench_command(command.clone(), cx);
            }
            AnalysisWorkspaceEvent::DismissOtherPopovers => {
                self.project_sidebar.update(cx, |sidebar, cx| {
                    if sidebar.menu.take().is_some() {
                        cx.notify();
                    }
                });
                self.analysis_view_bar.update(cx, |bar, cx| {
                    if bar.menu.take().is_some() {
                        cx.notify();
                    }
                });
            }
            AnalysisWorkspaceEvent::ShowInspector(panel_id) => {
                self.show_metric_inspector(panel_id, cx);
            }
            AnalysisWorkspaceEvent::RequestDetail => self.request_detail(cx),
            AnalysisWorkspaceEvent::RequestOverview => self.request_overview(cx),
            AnalysisWorkspaceEvent::ScheduleDetails(requests) => {
                for request in requests {
                    self.request_panel_detail(
                        &request.panel_id,
                        request.viewport,
                        request.logical_width,
                        cx,
                    );
                }
            }
            AnalysisWorkspaceEvent::Interaction(interaction_event) => {
                self.interaction.update(cx, |interaction, interaction_cx| {
                    let changed = match interaction_event {
                        WorkspaceInteractionEvent::RulerHover(axis) => {
                            interaction.set_ruler_hover(*axis)
                        }
                        WorkspaceInteractionEvent::TrackPointerHover(hover) => {
                            interaction.set_track_pointer_hover(hover.clone())
                        }
                        WorkspaceInteractionEvent::LockedCursor(axis) => {
                            interaction.set_locked_cursor(*axis)
                        }
                    };
                    if changed {
                        interaction_cx.notify();
                    }
                });
                cx.notify();
            }
        }
    }
}

impl AnalysisWorkspace {
    fn render_metric_workspace(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let theme = super::theme::ViewerTheme::for_appearance(window.appearance());
        let Some(session) = self.snapshot.clone() else {
            return div();
        };
        let panels: Rc<[MetricPanel]> = session.views.active().panels.clone().into();
        let selected = panels
            .iter()
            .map(|panel| panel.metric_key.clone())
            .collect::<HashSet<_>>();
        let available = self.available_metrics.clone();
        let metric_picker = self.render_metric_picker(available, &selected, theme, cx);
        let axis_picker = self.render_axis_picker(theme, cx);
        let metric_sidebar_width = self.metric_sidebar_width(theme);
        let timeline = self.render_overview(theme, cx);
        let ruler = self.render_ruler(window, cx);
        let interaction = self.interaction.clone();
        let cursor_layer = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
            .map(|brush| {
                div()
                    .id("metric-cursor-overlay")
                    .debug_selector(|| "metric-cursor-overlay".to_owned())
                    .absolute()
                    .left(metric_sidebar_width)
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .child(
                        chart::cursor_canvas(
                            None,
                            brush.selected(),
                            self.hover_cursor_axis(),
                            interaction.locked_cursor,
                            false,
                        )
                        .absolute()
                        .size_full(),
                    )
            });
        let scroll = self.metric_scroll.clone();
        let workspace = cx.entity().downgrade();
        let track_rows: Rc<[MetricTrackRowSnapshot]> = self.track_rows.clone().into();
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .id("brush-row")
                    .debug_selector(|| "brush-row".to_owned())
                    .h(px(BRUSH_ROW_HEIGHT))
                    .flex()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .id("brush-controls")
                            .debug_selector(|| "brush-controls".to_owned())
                            .w(metric_sidebar_width)
                            .h_full()
                            .flex_shrink_0()
                            .px_1()
                            .pt(px(BRUSH_CONTENT_TOP_PADDING))
                            .bg(theme.colors.panel)
                            .border_r_1()
                            .border_color(theme.colors.border)
                            .relative()
                            .flex()
                            .items_start()
                            .justify_between()
                            .child(axis_picker)
                            .child(metric_picker),
                    )
                    .child(div().h_full().flex_1().child(timeline)),
            )
            .child(
                div()
                    .h(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(ruler),
            )
            .child(
                div()
                    .id("metric-track-scroll")
                    .debug_selector(|| "metric-track-scroll".to_owned())
                    .relative()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        list(scroll, move |index, _, cx| {
                            track_rows.get(index).map_or_else(
                                || div().into_any_element(),
                                |row| {
                                    AnalysisWorkspace::render_metric_row(
                                        row.clone(),
                                        workspace.clone(),
                                        theme,
                                        cx,
                                    )
                                    .into_any_element()
                                },
                            )
                        })
                        .size_full(),
                    )
                    .children(cursor_layer),
            )
    }
}

impl Render for AnalysisWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_metric_workspace(window, cx)
    }
}
