use std::sync::Arc;

use gpui::{
    App, Context, EventEmitter, MouseButton, MouseDownEvent, MouseMoveEvent, Render, ScrollHandle,
    SharedString, Window, div, prelude::*, px,
};

use crate::data::worker::ReadKind;
use crate::domain::RunRef;
use crate::workbench::panel_reads::MetricPanelId;

use super::ViewerApp;
use super::chart;
use super::command::WorkbenchCommand;
use super::components::{ResizeEdge, resize_handle};
use super::interaction::InteractionSnapshot;
use super::project_sidebar::RunHoverExitPolicy;
use super::session::SessionSnapshot;
use super::theme::ViewerTheme;

mod table;

pub(super) use table::*;

pub(crate) struct BottomInspector {
    pub visible: bool,
    pub height: gpui::Pixels,
    pub resize: Option<InspectorResize>,
    pub sort: Option<InspectorSort>,
    pub column_widths: [f32; INSPECTOR_COLUMNS.len()],
    pub column_resize: Option<InspectorColumnResize>,
    pub horizontal_scroll: ScrollHandle,
    pub vertical_scroll: ScrollHandle,
    snapshot: Option<Arc<SessionSnapshot>>,
    interaction: InteractionSnapshot,
    visible_runs: Vec<RunRef>,
}

#[derive(Clone, Debug)]
pub(crate) enum BottomInspectorEvent {
    HoveredRun {
        run: RunRef,
        region: SharedString,
        hovered: bool,
    },
}

impl EventEmitter<BottomInspectorEvent> for BottomInspector {}

impl BottomInspector {
    pub fn new() -> Self {
        Self {
            visible: false,
            height: px(220.),
            resize: None,
            sort: None,
            column_widths: INSPECTOR_COLUMNS.map(InspectorColumn::default_width),
            column_resize: None,
            horizontal_scroll: ScrollHandle::new(),
            vertical_scroll: ScrollHandle::new(),
            snapshot: None,
            interaction: InteractionSnapshot::default(),
            visible_runs: Vec::new(),
        }
    }

    pub(crate) fn sync(
        &mut self,
        snapshot: Arc<SessionSnapshot>,
        interaction: InteractionSnapshot,
        visible_runs: Vec<RunRef>,
    ) {
        self.snapshot = Some(snapshot);
        self.interaction = interaction;
        self.visible_runs = visible_runs;
    }
}

impl ViewerApp {
    pub(super) fn inspector_visible(&self, cx: &App) -> bool {
        self.bottom_inspector.read(cx).visible
    }

    pub(super) fn show_metric_inspector(
        &mut self,
        panel_id: &MetricPanelId,
        cx: &mut Context<Self>,
    ) {
        self.dispatch_workbench_command(WorkbenchCommand::SelectPanel(panel_id.clone()), cx);
        cx.notify();
    }

    pub(super) fn handle_bottom_inspector_event(
        &mut self,
        event: &BottomInspectorEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            BottomInspectorEvent::HoveredRun {
                run,
                region,
                hovered,
            } => {
                self.set_hovered_run(run, region, *hovered, RunHoverExitPolicy::OneFrameGrace, cx);
            }
        }
    }
}

impl BottomInspector {
    pub(crate) fn toggle_inspector_sort(
        &mut self,
        column: InspectorColumn,
        cx: &mut Context<Self>,
    ) {
        self.sort = match self.sort {
            Some(InspectorSort {
                column: active,
                direction: InspectorSortDirection::Ascending,
            }) if active == column => Some(InspectorSort {
                column,
                direction: InspectorSortDirection::Descending,
            }),
            Some(InspectorSort {
                column: active,
                direction: InspectorSortDirection::Descending,
            }) if active == column => None,
            _ => Some(InspectorSort {
                column,
                direction: InspectorSortDirection::Ascending,
            }),
        };
        cx.notify();
    }

    fn inspector_column_width(&self, column: InspectorColumn) -> f32 {
        self.column_widths[column.index()]
    }

    pub(crate) fn begin_inspector_column_resize(
        &mut self,
        column: InspectorColumn,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let start_width = self.inspector_column_width(column);
        self.column_resize = Some(InspectorColumnResize {
            column,
            start_x: event.position.x,
            start_width,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn move_inspector_column_resize(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.column_resize else {
            return;
        };
        let width = (resize.start_width + f32::from(event.position.x - resize.start_x))
            .clamp(resize.column.minimum_width(), 600.);
        self.column_widths[resize.column.index()] = width;
        cx.notify();
    }

    pub(crate) fn finish_inspector_column_resize(&mut self, cx: &mut Context<Self>) {
        if self.column_resize.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn begin_inspector_resize(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.resize = Some(InspectorResize {
            start_y: event.position.y,
            start_height: self.height,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn move_inspector_resize(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.resize else {
            return;
        };
        let maximum = (window.viewport_size().height - px(120.)).max(px(56.));
        self.height =
            (resize.start_height + resize.start_y - event.position.y).clamp(px(56.), maximum);
        cx.notify();
    }

    pub(crate) fn finish_inspector_resize(&mut self, cx: &mut Context<Self>) {
        if self.resize.take().is_some() {
            cx.notify();
        }
    }

    fn render_bottom_inspector(
        &mut self,
        theme: ViewerTheme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let Some(session) = self.snapshot.clone() else {
            return div().id("bottom-inspector");
        };
        let panel = session
            .views
            .active()
            .selected_panel_id
            .as_ref()
            .and_then(|panel_id| session.views.active_panel(panel_id))
            .cloned();
        let interaction = self.interaction.clone();
        let hover_cursor = interaction
            .ruler_hover
            .or_else(|| interaction.track_pointer_hover.map(|(_, axis)| axis));
        let snapshot = panel.as_ref().and_then(|panel| panel.inspector.as_deref());
        let baseline = session.views.active().baseline.clone();
        let pinned = session.views.active().pinned_runs.clone();
        let visible_runs = self.visible_runs.clone();
        let height = self.height;
        let sort = self.sort;
        let resizing = self.resize.is_some();
        let body = if panel
            .as_ref()
            .is_some_and(|panel| panel.is_pending(ReadKind::Inspector))
            && snapshot.is_none()
        {
            div().child("Loading metric summaries and objective evidence…")
        } else {
            let rows = panel
                .as_ref()
                .zip(snapshot)
                .map_or_else(Vec::new, |(panel, snapshot)| {
                    inspector_rows(
                        panel,
                        snapshot,
                        InspectorRowsContext {
                            visible_runs: &visible_runs,
                            baseline: baseline.as_ref(),
                            pinned: &pinned,
                            locked_axis: interaction.locked_cursor,
                            hover_axis: hover_cursor,
                            sort,
                        },
                    )
                });
            self.render_inspector_table(rows, theme, cx)
        };

        div()
            .id("bottom-inspector")
            .debug_selector(|| "bottom-inspector".to_owned())
            .h(height)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .bg(theme.colors.surface)
            .border_t_1()
            .border_color(theme.colors.transparent)
            .child(
                body.id("bottom-inspector-scroll")
                    .debug_selector(|| "bottom-inspector-scroll".to_owned())
                    .flex_1()
                    .min_h(px(0.))
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(theme.colors.text_muted),
            )
            .child(
                resize_handle(
                    SharedString::from("bottom-inspector-resize"),
                    theme,
                    resizing,
                    ResizeEdge::Top,
                )
                .debug_selector(|| "bottom-inspector-resize".to_owned())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.begin_inspector_resize(event, cx);
                    }),
                ),
            )
    }
    fn render_inspector_table(
        &mut self,
        rows: Vec<InspectorRow>,
        theme: ViewerTheme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let emphasized_run = self.interaction.emphasized_run.clone();
        let sort = self.sort;
        let mut column_widths = self.column_widths;
        let resizing_column = self.column_resize.map(|resize| resize.column);
        let column_resize_active = resizing_column.is_some();
        let horizontal_scroll = self.horizontal_scroll.clone();
        let vertical_scroll = self.vertical_scroll.clone();
        column_widths[InspectorColumn::Project.index()] = inspector_project_width(&rows);
        let run_width = column_widths[InspectorColumn::Run.index()];
        let baseline = rows.iter().find(|row| row.baseline).cloned();
        let content_width = INSPECTOR_COLUMNS
            .into_iter()
            .skip(1)
            .map(|column| column_widths[column.index()])
            .sum::<f32>();
        let body_height = px(f32::from(theme.spacing.control_height) * rows.len() as f32);
        let run_header_direction = sort
            .filter(|sort| sort.column == InspectorColumn::Run)
            .map(|sort| sort.direction);
        let run_header = inspector_header_cell(
            InspectorColumn::Run,
            run_width,
            run_header_direction,
            resizing_column == Some(InspectorColumn::Run),
            column_resize_active,
            theme,
            cx,
        )
        .bg(theme.colors.surface);
        let header_content = div()
            .min_w(px(content_width))
            .h(theme.spacing.control_height)
            .flex_none()
            .flex()
            .items_center()
            .children(INSPECTOR_COLUMNS.into_iter().skip(1).map(|column| {
                let active_direction = sort
                    .filter(|sort| sort.column == column)
                    .map(|sort| sort.direction);
                inspector_header_cell(
                    column,
                    column_widths[column.index()],
                    active_direction,
                    resizing_column == Some(column),
                    column_resize_active,
                    theme,
                    cx,
                )
            }));
        let header_scroll = div()
            .id("inspector-header-scroll")
            .debug_selector(|| "inspector-header-scroll".to_owned())
            .flex_1()
            .min_w(px(0.))
            .h(theme.spacing.control_height)
            .overflow_x_scroll()
            .track_scroll(&horizontal_scroll)
            .map(|mut viewport| {
                viewport.style().restrict_scroll_to_axis = Some(true);
                viewport
            })
            .child(header_content);
        let header = div()
            .id("inspector-table-header")
            .debug_selector(|| "inspector-table-header".to_owned())
            .h(theme.spacing.control_height)
            .flex_none()
            .flex()
            .border_b_1()
            .border_color(theme.colors.border)
            .child(run_header)
            .child(header_scroll);
        let run_column = div()
            .id("inspector-sticky-run-column")
            .debug_selector(|| "inspector-sticky-run-column".to_owned())
            .w(px(run_width))
            .h(body_height)
            .flex_none()
            .flex()
            .flex_col()
            .children(rows.iter().map(|row| {
                let run_ref = row.run_ref.clone();
                let hover_region =
                    SharedString::from(format!("inspector-sticky-run:{}", run_ref.cache_key()));
                let color = theme
                    .colors
                    .series_color(chart::series_color_index(&run_ref));
                let highlighted = emphasized_run.as_ref() == Some(&run_ref);
                inspector_run_cell(row, run_width, color, theme)
                    .id(SharedString::from(format!(
                        "inspector-sticky-run:{}",
                        run_ref.cache_key()
                    )))
                    .debug_selector({
                        let run_id = run_ref.run_id.as_str().to_owned();
                        move || format!("inspector-sticky-run:{run_id}")
                    })
                    .h(theme.spacing.control_height)
                    .flex_none()
                    .bg(if highlighted {
                        theme.colors.element_hover
                    } else {
                        theme.colors.surface
                    })
                    .border_r_1()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                        cx.emit(BottomInspectorEvent::HoveredRun {
                            run: run_ref.clone(),
                            region: hover_region.clone(),
                            hovered: *hovered,
                        });
                    }))
            }));
        let table = div()
            .id("inspector-table")
            .debug_selector(|| "inspector-table".to_owned())
            .min_w(px(content_width))
            .h(body_height)
            .flex_none()
            .flex()
            .flex_col()
            .children(rows.iter().map(|row| {
                let run_ref = row.run_ref.clone();
                let hover_region =
                    SharedString::from(format!("inspector-row:{}", run_ref.cache_key()));
                let row_id = run_ref.cache_key();
                let row_run_id = run_ref.run_id.as_str().to_owned();
                let values = INSPECTOR_COLUMNS
                    .into_iter()
                    .skip(1)
                    .map(|column| (column, inspector_cell_text(column, row, baseline.as_ref())));
                let highlighted = emphasized_run.as_ref() == Some(&run_ref);
                div()
                    .id(SharedString::from(format!("inspector-row:{row_id}")))
                    .debug_selector(move || format!("inspector-row:{row_run_id}"))
                    .h(theme.spacing.control_height)
                    .flex_none()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .when(highlighted, |item| item.bg(theme.colors.element_hover))
                    .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                        cx.emit(BottomInspectorEvent::HoveredRun {
                            run: run_ref.clone(),
                            region: hover_region.clone(),
                            hovered: *hovered,
                        });
                    }))
                    .children(values.map(|(column, value)| {
                        div()
                            .w(px(column_widths[column.index()]))
                            .h_full()
                            .flex_none()
                            .px_2()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(column.right_aligned(), |cell| cell.justify_end())
                            .child(value)
                    }))
            }));
        let horizontal_body = div()
            .id("inspector-body-horizontal-scroll")
            .debug_selector(|| "inspector-body-horizontal-scroll".to_owned())
            .flex_1()
            .min_w(px(0.))
            .h(body_height)
            .overflow_x_scroll()
            .track_scroll(&horizontal_scroll)
            .map(|mut viewport| {
                viewport.style().restrict_scroll_to_axis = Some(true);
                viewport
            })
            .child(table);
        let body = div()
            .h(body_height)
            .flex_none()
            .flex()
            .child(run_column)
            .child(horizontal_body);
        let vertical_body = div()
            .id("inspector-body-vertical-scroll")
            .debug_selector(|| "inspector-body-vertical-scroll".to_owned())
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&vertical_scroll)
            .child(body);
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(header)
            .child(vertical_body)
    }
}

impl Render for BottomInspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ViewerTheme::for_appearance(window.appearance());
        self.render_bottom_inspector(theme, cx)
    }
}
