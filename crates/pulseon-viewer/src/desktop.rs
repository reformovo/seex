use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{cell::RefCell, rc::Rc};

use gpui::{
    App, Application, Bounds, Context, Corner, FocusHandle, KeyBinding, KeyDownEvent,
    ListAlignment, ListState, Menu, MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathPromptOptions, Render, ScrollHandle, ScrollWheelEvent, SharedString,
    SystemMenuType, Task, Transformation, Window, WindowBounds, WindowOptions, actions, anchored,
    deferred, div, list, point, prelude::*, px, size,
};
use pulseon_chart_core::{AxisRange, BrushState, CanvasSize, Viewport};
use pulseon_model::alignment::{AlignmentAxis, AlignmentViewport};
use pulseon_model::comparison::{EvidenceCompleteness, EvidenceReason};
use pulseon_model::metric::MetricKey;
use pulseon_model::run::{Run, RunStatus};
use pulseon_model::types::{Project, ProjectId};
use pulseon_viewer::coordination::AnalysisViewId;
use pulseon_viewer::coordination::{
    MetricPanelId, PanelReadCoordinator, PanelReadMode, PanelReadOutcome, PanelReadRequest,
    PanelReadTag,
};
use pulseon_viewer::core::{DataSourceId, MAX_SELECTED_RUNS, RunRef, ViewNavigation};
use pulseon_viewer::model::DiscoveryRequest;
use pulseon_viewer::query::{CurveAxis, CurveSnapshot, InspectorRunSnapshot, InspectorSnapshot};
use pulseon_viewer::registry::SourceRegistry;
#[cfg(test)]
use pulseon_viewer::registry::SourceStatus;
use pulseon_viewer::workbench::{AnalysisViews, MetricPanel, ProjectRef};
use pulseon_viewer::workbench_document::{
    SavedAnalysisView, SavedProjectRef, SavedRunRef, WorkbenchDocument,
};
use pulseon_viewer::worker::{Generation, ReadEvent, ReadEventReceiver, ReadKind, ReadRequest};

mod assets;
mod components;
mod renderer;
mod sidebar;
mod theme;
mod view_bar;

use assets::ViewerAssets;
use components::{IconName, StatusTone};
use renderer::{ChartAdapter, HoverPoint};
use theme::ViewerTheme;

const METRIC_TRACK_VERTICAL_PADDING: f32 = 4.;
const METRIC_TRACK_SEPARATOR_WIDTH: f32 = 1.;
const BRUSH_ROW_HEIGHT: f32 = 40.;
const BRUSH_CONTENT_TOP_PADDING: f32 = 6.;
const INITIAL_VISIBLE_TRACKS: usize = 4;
const INITIAL_OVERSCAN_TRACKS: usize = 8;

#[derive(Clone, Debug)]
enum DragGesture {
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

#[derive(Clone, Copy, Debug)]
struct InspectorResize {
    start_y: gpui::Pixels,
    start_height: gpui::Pixels,
}

#[derive(Clone, Copy, Debug)]
struct SidebarResize {
    start_x: gpui::Pixels,
    start_width: gpui::Pixels,
}

#[derive(Clone, Copy, Debug)]
struct InspectorColumnResize {
    column: InspectorColumn,
    start_x: gpui::Pixels,
    start_width: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectorColumn {
    Run,
    LastValue,
    Minimum,
    Maximum,
    Locked,
    Hover,
    Count,
    LastStep,
    Status,
    Evidence,
    Project,
}

const INSPECTOR_COLUMNS: [InspectorColumn; 11] = [
    InspectorColumn::Run,
    InspectorColumn::LastValue,
    InspectorColumn::Minimum,
    InspectorColumn::Maximum,
    InspectorColumn::Locked,
    InspectorColumn::Hover,
    InspectorColumn::Count,
    InspectorColumn::LastStep,
    InspectorColumn::Status,
    InspectorColumn::Evidence,
    InspectorColumn::Project,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectorSortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InspectorSort {
    column: InspectorColumn,
    direction: InspectorSortDirection,
}

#[derive(Clone)]
struct InspectorRow {
    run_ref: RunRef,
    run_label: String,
    project_label: String,
    status: RunStatus,
    evidence: EvidenceCompleteness,
    evidence_label: String,
    count: Option<u64>,
    last_step: Option<i64>,
    last_value: Option<f64>,
    minimum: Option<f64>,
    maximum: Option<f64>,
    locked: Option<(i64, f64)>,
    hover: Option<(i64, f64)>,
    baseline: bool,
    pinned: bool,
    original_order: usize,
}

struct InspectorRowsContext<'a> {
    visible_runs: &'a [RunRef],
    baseline: Option<&'a RunRef>,
    pinned: &'a [RunRef],
    locked_axis: Option<f64>,
    hover_axis: Option<f64>,
    sort: Option<InspectorSort>,
}

#[derive(Clone, Debug)]
struct MetricResize {
    panel_id: MetricPanelId,
    start_y: gpui::Pixels,
    start_height: f32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TrackViewport {
    visible: Range<usize>,
    overscan: Range<usize>,
    logical_width_bits: u32,
    physical_width: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectPlacement {
    Pinned,
    Projects,
    Archived,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunPlacement {
    Baseline,
    Pinned,
    Projects,
    Archived,
}

#[derive(Clone)]
struct SidebarProject {
    source_index: usize,
    project_index: usize,
    project_ref: ProjectRef,
    project: Project,
    runs: Vec<Run>,
    source_label: String,
    placement: ProjectPlacement,
}

fn update_brush_drag(brush: &mut BrushState, gesture: &mut DragGesture, axis: f64) {
    let selected = brush.selected();
    match gesture {
        DragGesture::BrushStart if axis < selected.end() => {
            let _ = brush.resize_start(axis);
        }
        DragGesture::BrushEnd if axis > selected.start() => {
            let _ = brush.resize_end(axis);
        }
        DragGesture::BrushWindow { last_axis } => {
            let _ = brush.pan_by(axis - *last_axis);
            *last_axis = axis;
        }
        DragGesture::BrushStart
        | DragGesture::BrushEnd
        | DragGesture::Ruler { .. }
        | DragGesture::Detail { .. } => {}
    }
}

fn track_chart_frame(
    panel: &MetricPanel,
    selected: Option<AxisRange>,
    visible_runs: &[RunRef],
) -> Option<(Arc<CurveSnapshot>, u64, Viewport)> {
    let detail = panel.detail.as_ref()?;
    let detail_viewport = renderer::detail_viewport(detail, selected, Some(visible_runs));
    if detail_viewport
        .is_none_or(|viewport| selected.is_some_and(|selected| viewport.x != selected))
        && let Some(overview) = panel.overview.as_ref()
        && let Some(viewport) = renderer::detail_viewport(overview, selected, Some(visible_runs))
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

fn previous_text_cursor(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_text_cursor(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map_or(cursor, |character| cursor + character.len_utf8())
}

actions!(
    pulseon_viewer,
    [
        OpenProject,
        Refresh,
        ResetView,
        ToggleProjectSidebar,
        ToggleMetricSidebar,
        ToggleBottomInspector,
        ShowMetricInspector,
        ClearLockedCursor,
        ZoomIn,
        ZoomOut,
        UseStep,
        UseElapsed,
        ActivateSelection,
        Quit
    ]
);

const SELECTABLE_CONTEXT: &str = "ViewerSelectable";
const WORKBENCH_PATH_ENV: &str = "PULSEON_VIEWER_WORKBENCH_PATH";

fn default_workbench_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(WORKBENCH_PATH_ENV) {
        return Some(PathBuf::from(path));
    }
    #[cfg(all(target_os = "macos", not(any(test, feature = "test-support"))))]
    {
        return std::env::var_os("HOME").map(|home| {
            PathBuf::from(home).join("Library/Application Support/PulseOn Viewer/workbench.state")
        });
    }
    #[allow(unreachable_code)]
    None
}

pub fn run(project_path: Option<PathBuf>) {
    Application::new()
        .with_assets(ViewerAssets)
        .run(move |cx: &mut App| {
            cx.bind_keys([
                KeyBinding::new("cmd-o", OpenProject, None),
                KeyBinding::new("cmd-r", Refresh, None),
                KeyBinding::new("cmd-shift-b", ToggleProjectSidebar, None),
                KeyBinding::new("cmd-shift-m", ToggleMetricSidebar, None),
                KeyBinding::new("cmd-j", ToggleBottomInspector, None),
                KeyBinding::new("cmd-0", ResetView, None),
                KeyBinding::new("cmd-=", ZoomIn, None),
                KeyBinding::new("cmd-shift-=", ZoomIn, None),
                KeyBinding::new("cmd-+", ZoomIn, None),
                KeyBinding::new("cmd--", ZoomOut, None),
                KeyBinding::new("escape", ClearLockedCursor, None),
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("enter", ActivateSelection, Some(SELECTABLE_CONTEXT)),
                KeyBinding::new("space", ActivateSelection, Some(SELECTABLE_CONTEXT)),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus(menus());
            let bounds = Bounds::centered(None, size(px(1_200.), px(800.)), cx);
            let result = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..WindowOptions::default()
                },
                move |window, cx| cx.new(|cx| ViewerApp::new(project_path, window, cx)),
            );
            if let Err(error) = result {
                eprintln!("failed to open pulseon-viewer window: {error}");
                cx.quit();
            } else {
                cx.activate(true);
            }
        });
}

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "PulseOn".into(),
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit PulseOn Viewer", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Import Source…", OpenProject),
                MenuItem::action("Refresh", Refresh),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Reset View", ResetView),
                MenuItem::action("Zoom In", ZoomIn),
                MenuItem::action("Zoom Out", ZoomOut),
                MenuItem::separator(),
                MenuItem::action("Toggle Project Sidebar", ToggleProjectSidebar),
                MenuItem::action("Toggle Metric Sidebar", ToggleMetricSidebar),
                MenuItem::action("Toggle Bottom Inspector", ToggleBottomInspector),
                MenuItem::action("Show Metric Inspector", ShowMetricInspector),
                MenuItem::separator(),
                MenuItem::action("Step", UseStep),
                MenuItem::action("Absolute Time", UseElapsed),
            ],
        },
    ]
}

struct ViewerApp {
    theme: ViewerTheme,
    focus: FocusHandle,
    filter_focus: FocusHandle,
    metric_filter_focus: FocusHandle,
    view_name_focus: FocusHandle,
    run_filter: String,
    run_filter_cursor: usize,
    filter_cursor_visible: bool,
    filter_cursor_epoch: u64,
    metric_filter: String,
    expanded_projects: HashSet<(DataSourceId, ProjectId)>,
    project_focuses: HashMap<ProjectRef, FocusHandle>,
    run_focuses: HashMap<RunRef, FocusHandle>,
    project_menu: Option<ProjectRef>,
    hovered_project: Option<ProjectRef>,
    hovered_run: Option<RunRef>,
    project_run_limits: HashMap<ProjectRef, usize>,
    pinned_run_limits: HashMap<AnalysisViewId, usize>,
    archived_run_limit: usize,
    views: AnalysisViews,
    panel_reads: PanelReadCoordinator,
    renaming_view: Option<AnalysisViewId>,
    view_menu: Option<AnalysisViewId>,
    view_name_draft: String,
    view_name_cursor: usize,
    view_name_select_all: bool,
    view_name_cursor_visible: bool,
    view_name_cursor_epoch: u64,
    metric_picker_open: bool,
    axis_picker_open: bool,
    project_sidebar_visible: bool,
    project_sidebar_width: gpui::Pixels,
    sidebar_resize: Option<SidebarResize>,
    metric_sidebar_compact: bool,
    bottom_inspector_visible: bool,
    bottom_inspector_height: gpui::Pixels,
    inspector_resize: Option<InspectorResize>,
    inspector_sort: Option<InspectorSort>,
    inspector_column_widths: [f32; INSPECTOR_COLUMNS.len()],
    inspector_column_resize: Option<InspectorColumnResize>,
    inspector_horizontal_scroll: ScrollHandle,
    inspector_vertical_scroll: ScrollHandle,
    sources: SourceRegistry,
    event_tasks: HashMap<DataSourceId, Task<()>>,
    navigation: ViewNavigation,
    next_generation: u64,
    local_error: Option<String>,
    chart_adapter: Rc<RefCell<ChartAdapter>>,
    track_adapters: HashMap<MetricPanelId, Rc<RefCell<ChartAdapter>>>,
    track_charts: HashMap<MetricPanelId, gpui::Entity<renderer::DetailChart>>,
    track_hovers: HashMap<MetricPanelId, HoverPoint>,
    metric_scroll: ListState,
    metric_resize: Option<MetricResize>,
    track_viewport: Rc<RefCell<TrackViewport>>,
    overview_logical_width: f32,
    overview_width: u32,
    ruler_hover: Option<f64>,
    track_pointer_hover: Option<(MetricPanelId, f64)>,
    locked_cursor: Option<f64>,
    drag: Option<DragGesture>,
    zoom_task: Option<Task<()>>,
    metric_repaint_pending: bool,
    detail_refresh_token: u64,
    detail_refresh_pending: bool,
    workbench_path: Option<PathBuf>,
    last_saved_workbench: Option<String>,
}

impl ViewerApp {
    fn new(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let mut app = Self {
            theme: ViewerTheme::for_appearance(window.appearance()),
            focus,
            filter_focus: cx.focus_handle().tab_stop(true),
            metric_filter_focus: cx.focus_handle().tab_stop(true),
            view_name_focus: cx.focus_handle().tab_stop(true),
            run_filter: String::new(),
            run_filter_cursor: 0,
            filter_cursor_visible: false,
            filter_cursor_epoch: 0,
            metric_filter: String::new(),
            expanded_projects: HashSet::new(),
            project_focuses: HashMap::new(),
            run_focuses: HashMap::new(),
            project_menu: None,
            hovered_project: None,
            hovered_run: None,
            project_run_limits: HashMap::new(),
            pinned_run_limits: HashMap::new(),
            archived_run_limit: 5,
            views: AnalysisViews::default(),
            panel_reads: PanelReadCoordinator::default(),
            renaming_view: None,
            view_menu: None,
            view_name_draft: String::new(),
            view_name_cursor: 0,
            view_name_select_all: false,
            view_name_cursor_visible: false,
            view_name_cursor_epoch: 0,
            metric_picker_open: false,
            axis_picker_open: false,
            project_sidebar_visible: true,
            project_sidebar_width: px(190.),
            sidebar_resize: None,
            metric_sidebar_compact: false,
            bottom_inspector_visible: false,
            bottom_inspector_height: px(220.),
            inspector_resize: None,
            inspector_sort: None,
            inspector_column_widths: INSPECTOR_COLUMNS.map(InspectorColumn::default_width),
            inspector_column_resize: None,
            inspector_horizontal_scroll: ScrollHandle::new(),
            inspector_vertical_scroll: ScrollHandle::new(),
            sources: SourceRegistry::default(),
            event_tasks: HashMap::new(),
            navigation: ViewNavigation::default(),
            next_generation: 1,
            local_error: None,
            chart_adapter: Rc::new(RefCell::new(ChartAdapter::default())),
            track_adapters: HashMap::new(),
            track_charts: HashMap::new(),
            track_hovers: HashMap::new(),
            metric_scroll: ListState::new(0, ListAlignment::Top, px(480.)),
            metric_resize: None,
            track_viewport: Rc::new(RefCell::new(TrackViewport::default())),
            overview_logical_width: 1_000.,
            overview_width: 1_000,
            ruler_hover: None,
            track_pointer_hover: None,
            locked_cursor: None,
            drag: None,
            zoom_task: None,
            metric_repaint_pending: false,
            detail_refresh_token: 0,
            detail_refresh_pending: false,
            workbench_path: default_workbench_path(),
            last_saved_workbench: None,
        };
        cx.on_focus(&app.filter_focus, window, |this, _, cx| {
            this.start_filter_cursor_blink(cx);
        })
        .detach();
        cx.on_blur(&app.filter_focus, window, |this, _, cx| {
            this.stop_filter_cursor_blink(cx);
        })
        .detach();
        cx.on_blur(&app.view_name_focus, window, |this, _, cx| {
            this.finish_rename_analysis_view(true, cx);
        })
        .detach();
        if let Some(path) = app.workbench_path.clone() {
            match WorkbenchDocument::load(&path) {
                Ok(Some(document)) => app.restore_workbench(document, cx),
                Ok(None) => {}
                Err(error) => {
                    app.local_error = Some(error.to_string());
                    app.workbench_path = None;
                }
            }
        }
        if let Some(path) = project_path {
            app.open_source(path, cx);
        }
        app
    }

    fn restore_workbench(&mut self, document: WorkbenchDocument, cx: &mut Context<Self>) {
        self.project_sidebar_visible = document.project_sidebar_visible;
        self.project_sidebar_width = px(document.project_sidebar_width.clamp(160., 600.));
        self.metric_sidebar_compact = document.metric_sidebar_compact;
        self.bottom_inspector_height = px(document.bottom_inspector_height.clamp(56., 2_000.));
        let mut source_paths = document.sources.clone();
        let referenced_paths = document
            .pinned_projects
            .iter()
            .chain(&document.archived_projects)
            .map(|project| &project.source_path)
            .chain(document.archived_runs.iter().map(|run| &run.source_path))
            .chain(document.views.iter().flat_map(|view| {
                view.runs
                    .iter()
                    .chain(&view.pinned_runs)
                    .chain(view.baseline.iter())
                    .map(|run| &run.source_path)
            }));
        for path in referenced_paths {
            if !source_paths.contains(path) {
                source_paths.push(path.clone());
            }
        }
        for path in &source_paths {
            let source_id = self.sources.import(path.clone());
            if !path.is_dir() {
                self.sources.mark_unavailable(
                    &source_id,
                    format!("source path is unavailable: {}", path.display()),
                );
            }
        }
        let (views, issues) = AnalysisViews::restore(&document);
        self.views = views;
        self.restore_active_view_state();
        self.bottom_inspector_visible =
            document.bottom_inspector_visible && self.views.active().selected_panel_id.is_some();
        self.last_saved_workbench = Some(document.encode());
        if !issues.is_empty() {
            self.local_error = Some(issues.join("; "));
        }
        let source_ids = self
            .sources
            .sources()
            .filter(|source| source.root_path.is_dir())
            .map(|source| source.source_id.clone())
            .collect::<Vec<_>>();
        for source_id in source_ids {
            let request = self.discovery_request_for_source(&source_id);
            self.submit_to_source(source_id, ReadRequest::Discover(request), cx);
        }
    }

    fn open_source(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let source_id = self.sources.import(path);
        self.local_error = None;
        let request = self.discovery_request_for_source(&source_id);
        self.submit_to_source(source_id, ReadRequest::Discover(request), cx);
    }

    fn refresh_catalog(&mut self, cx: &mut Context<Self>) {
        let mut source_ids = self
            .active_visible_runs()
            .into_iter()
            .map(|run| run.source_id)
            .collect::<HashSet<_>>();
        if source_ids.is_empty() {
            source_ids.extend(
                self.sources
                    .sources()
                    .map(|source| source.source_id.clone()),
            );
        }
        for source_id in source_ids {
            let request = self.discovery_request_for_source(&source_id);
            self.submit_to_source(source_id, ReadRequest::Discover(request), cx);
        }
    }

    fn refresh_all_sources(&mut self, cx: &mut Context<Self>) {
        let source_ids = self
            .sources
            .sources()
            .map(|source| source.source_id.clone())
            .collect::<Vec<_>>();
        for source_id in source_ids {
            let request = self.discovery_request_for_source(&source_id);
            self.submit_to_source(source_id, ReadRequest::Discover(request), cx);
        }
    }

    fn discovery_request_for_source(&self, source_id: &DataSourceId) -> DiscoveryRequest {
        let visible = self
            .active_visible_runs()
            .into_iter()
            .filter(|run| &run.source_id == source_id)
            .collect::<Vec<_>>();
        DiscoveryRequest {
            project_id: None,
            selected_run_ids: Vec::new(),
            metric_runs: visible
                .into_iter()
                .map(|run| (run.project_id, run.run_id))
                .collect(),
        }
    }

    fn submit_to_source(
        &mut self,
        source_id: DataSourceId,
        request: ReadRequest,
        cx: &mut Context<Self>,
    ) {
        let generation = Generation(self.next_generation);
        self.next_generation = self.next_generation.saturating_add(1);
        self.submit_source_generation(source_id, generation, request, cx);
    }

    fn submit_source_generation(
        &mut self,
        source_id: DataSourceId,
        generation: Generation,
        request: ReadRequest,
        cx: &mut Context<Self>,
    ) {
        match self.sources.activate(&source_id) {
            Ok(Some(events)) => self.listen_for_events(source_id.clone(), events, cx),
            Ok(None) => {}
            Err(error) => {
                self.local_error = Some(error.to_string());
                return;
            }
        }
        match self.sources.submit(&source_id, generation, request) {
            Ok(()) => {}
            Err(error) => self.local_error = Some(error.to_string()),
        }
    }

    fn listen_for_events(
        &mut self,
        source_id: DataSourceId,
        events: ReadEventReceiver,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.apply_event(event, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.event_tasks.insert(source_id, task);
    }

    fn apply_event(&mut self, event: ReadEvent, cx: &mut Context<Self>) {
        if let Ok(pulseon_viewer::worker::ReadSnapshot::Catalog(snapshot)) = &event.result {
            let available_runs = snapshot
                .runs
                .iter()
                .map(|run| (run.project_id.clone(), run.run_id.clone()))
                .collect::<Vec<_>>();
            let removed = self
                .views
                .reconcile_source_runs(&event.source_id, &available_runs);
            if !removed.is_empty() {
                self.local_error = Some(format!(
                    "{} persisted Run selection(s) are no longer available",
                    removed.len()
                ));
            }
        }
        self.sources.apply_event(&event);
        let kind = event.kind;
        if matches!(
            kind,
            ReadKind::Overview | ReadKind::Detail | ReadKind::Inspector
        ) {
            if let PanelReadOutcome::Completed(completed) = self.panel_reads.apply(event) {
                if completed.tag.view_id != self.views.active().view_id {
                    return;
                }
                let panel_id = completed.tag.panel_id.clone();
                let metric_key = self
                    .views
                    .active_panel(&panel_id)
                    .map(|panel| panel.metric_key.clone());
                if !completed.source_errors.is_empty() {
                    self.local_error = Some(
                        completed
                            .source_errors
                            .iter()
                            .map(|failure| format!("{}: {}", failure.source_id, failure.message))
                            .collect::<Vec<_>>()
                            .join("; "),
                    );
                }
                let accepted = if kind == ReadKind::Inspector {
                    self.views.complete_active_inspector_read(
                        &panel_id,
                        completed.tag.generation,
                        completed.inspector,
                        completed.source_errors,
                    )
                } else {
                    self.views.complete_active_panel_read(
                        &panel_id,
                        kind,
                        completed.tag.generation,
                        completed.tag.mode,
                        completed.curves,
                        completed.source_errors,
                    )
                };
                if accepted && kind == ReadKind::Overview {
                    let extent = self
                        .views
                        .active_panel(&panel_id)
                        .and_then(|panel| panel.overview.as_ref())
                        .and_then(|snapshot| snapshot.real_range);
                    if let Some(metric_key) = metric_key
                        && let Some(home) =
                            self.views.record_active_metric_extent(metric_key, extent)
                    {
                        self.navigation.set_timeline_home(home);
                    }
                    if let Some(viewport) = self.navigation.selected_viewport()
                        && self.panel_is_scheduled(&panel_id)
                    {
                        let physical_width = self.track_viewport.borrow().physical_width.max(1);
                        if self.should_schedule_panel_detail(&panel_id, viewport, physical_width) {
                            self.request_panel_detail(&panel_id, viewport, physical_width, cx);
                        }
                    }
                }
                if accepted && matches!(kind, ReadKind::Overview | ReadKind::Detail) {
                    self.defer_metric_repaint(cx);
                }
            }
            return;
        }
        if event.result.is_err() {
            return;
        }
        match kind {
            ReadKind::Catalog
                if !self.active_visible_runs().is_empty()
                    && !self.views.active().panels.is_empty() =>
            {
                self.request_missing_panel_curves(cx);
                if self.bottom_inspector_visible {
                    self.request_inspector(cx);
                }
            }
            ReadKind::Overview | ReadKind::Detail | ReadKind::Inspector => {}
            ReadKind::Catalog => {}
        }
    }

    fn request_overview(&mut self, cx: &mut Context<Self>) {
        let panel_ids = self
            .views
            .active()
            .panels
            .iter()
            .map(|panel| panel.panel_id.clone())
            .collect::<Vec<_>>();
        for panel_id in panel_ids {
            self.request_panel_overview(&panel_id, cx);
        }
    }

    fn curve_axis(&self) -> CurveAxis {
        match self.navigation.axis() {
            AlignmentAxis::Step => CurveAxis::Step,
            AlignmentAxis::ElapsedTime => CurveAxis::AbsoluteTime,
        }
    }

    fn hover_cursor_axis(&self) -> Option<f64> {
        self.ruler_hover
            .or_else(|| self.track_pointer_hover.as_ref().map(|(_, axis)| *axis))
    }

    fn request_panel_overview(&mut self, panel_id: &MetricPanelId, cx: &mut Context<Self>) {
        let runs = self.active_visible_runs();
        self.request_panel_overview_for_runs(panel_id, runs, PanelReadMode::Replace, cx);
    }

    fn request_panel_overview_for_runs(
        &mut self,
        panel_id: &MetricPanelId,
        runs: Vec<RunRef>,
        mode: PanelReadMode,
        cx: &mut Context<Self>,
    ) {
        let Some(metric_key) = self
            .views
            .active_panel(panel_id)
            .map(|panel| panel.metric_key.clone())
        else {
            return;
        };
        if runs.is_empty() {
            return;
        }
        let generation = Generation(self.next_generation);
        self.next_generation = self.next_generation.saturating_add(1);
        let tag = PanelReadTag {
            view_id: self.views.active().view_id.clone(),
            panel_id: panel_id.clone(),
            generation,
            mode,
        };
        let planned = match self.panel_reads.begin(
            tag,
            PanelReadRequest::Overview {
                runs,
                metric_key,
                axis: self.curve_axis(),
                physical_width: self.overview_width,
            },
        ) {
            Ok(planned) => planned,
            Err(error) => {
                self.local_error = Some(error.to_string());
                return;
            }
        };
        self.views
            .begin_active_panel_read(panel_id, ReadKind::Overview, generation);
        for read in planned {
            self.submit_source_generation(read.source_id, read.generation, read.request, cx);
        }
    }

    fn request_detail(&mut self, cx: &mut Context<Self>) {
        let Some(viewport) = self.navigation.selected_viewport() else {
            return;
        };
        let viewport_state = self.track_viewport.borrow().clone();
        let panel_count = self.views.active().panels.len();
        let scheduled = viewport_state.overscan.start.min(panel_count)
            ..viewport_state.overscan.end.min(panel_count);
        let panel_ids = self.views.active().panels[scheduled]
            .iter()
            .map(|panel| panel.panel_id.clone())
            .collect::<Vec<_>>();
        for panel_id in panel_ids {
            self.request_panel_detail(
                &panel_id,
                viewport,
                viewport_state.physical_width.max(1),
                cx,
            );
        }
    }

    fn request_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(panel_id) = self.views.active().selected_panel_id.clone() else {
            return;
        };
        let runs = self.active_visible_runs();
        let Some(metric_key) = self
            .views
            .active_panel(&panel_id)
            .map(|panel| panel.metric_key.clone())
        else {
            return;
        };
        if runs.is_empty() {
            return;
        }
        let generation = Generation(self.next_generation);
        self.next_generation = self.next_generation.saturating_add(1);
        let tag = PanelReadTag {
            view_id: self.views.active().view_id.clone(),
            panel_id: panel_id.clone(),
            generation,
            mode: PanelReadMode::Replace,
        };
        let planned = match self
            .panel_reads
            .begin(tag, PanelReadRequest::Inspector { runs, metric_key })
        {
            Ok(planned) => planned,
            Err(error) => {
                self.local_error = Some(error.to_string());
                return;
            }
        };
        self.views
            .begin_active_panel_read(&panel_id, ReadKind::Inspector, generation);
        for read in planned {
            self.submit_source_generation(read.source_id, read.generation, read.request, cx);
        }
    }

    fn request_panel_detail(
        &mut self,
        panel_id: &MetricPanelId,
        viewport: AlignmentViewport,
        physical_width: u32,
        cx: &mut Context<Self>,
    ) {
        let runs = self.active_visible_runs();
        self.request_panel_detail_for_runs(
            panel_id,
            viewport,
            physical_width,
            runs,
            PanelReadMode::Replace,
            cx,
        );
    }

    fn request_panel_detail_for_runs(
        &mut self,
        panel_id: &MetricPanelId,
        viewport: AlignmentViewport,
        physical_width: u32,
        runs: Vec<RunRef>,
        mode: PanelReadMode,
        cx: &mut Context<Self>,
    ) {
        let Some(metric_key) = self
            .views
            .active_panel(panel_id)
            .map(|panel| panel.metric_key.clone())
        else {
            return;
        };
        if runs.is_empty() {
            return;
        }
        let generation = Generation(self.next_generation);
        self.next_generation = self.next_generation.saturating_add(1);
        let tag = PanelReadTag {
            view_id: self.views.active().view_id.clone(),
            panel_id: panel_id.clone(),
            generation,
            mode,
        };
        let planned = match self.panel_reads.begin(
            tag,
            PanelReadRequest::Detail {
                runs,
                metric_key,
                axis: self.curve_axis(),
                viewport,
                physical_width,
            },
        ) {
            Ok(planned) => planned,
            Err(error) => {
                self.local_error = Some(error.to_string());
                return;
            }
        };
        self.views
            .begin_active_panel_detail(panel_id, generation, viewport, physical_width);
        for read in planned {
            self.submit_source_generation(read.source_id, read.generation, read.request, cx);
        }
    }

    fn open_picker(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from("Import PulseOn Source")),
        });
        cx.spawn(async move |this, cx| {
            let result = prompt.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(paths)) => {
                        if let Some(path) = picked_directory(paths) {
                            this.open_source(path, cx);
                        }
                    }
                    Ok(Err(error)) => this.local_error = Some(error.to_string()),
                    Err(error) => this.local_error = Some(error.to_string()),
                }
                cx.notify();
            })
        })
        .detach();
    }

    fn error(&self) -> Option<&str> {
        self.local_error.as_deref()
    }

    fn dismiss_popovers(&mut self) -> bool {
        let dismissed = self.project_menu.is_some()
            || self.view_menu.is_some()
            || self.metric_picker_open
            || self.axis_picker_open;
        self.project_menu = None;
        self.view_menu = None;
        self.metric_picker_open = false;
        self.axis_picker_open = false;
        dismissed
    }

    fn set_metric_picker_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_popovers();
        self.metric_picker_open = open;
        if open {
            self.metric_filter.clear();
            self.metric_filter_focus.focus(window);
        }
        cx.notify();
    }

    fn set_axis_picker_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.dismiss_popovers();
        self.axis_picker_open = open;
        self.metric_filter.clear();
        cx.notify();
    }

    fn on_open(&mut self, _: &OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(cx);
    }

    fn on_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.local_error = None;
        self.refresh_all_sources(cx);
        self.request_overview(cx);
        if self.bottom_inspector_visible {
            self.request_inspector(cx);
        }
        cx.notify();
    }

    fn on_toggle_project_sidebar(
        &mut self,
        _: &ToggleProjectSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.project_sidebar_visible = !self.project_sidebar_visible;
        self.project_menu = None;
        self.hovered_project = None;
        self.hovered_run = None;
        self.focus.focus(window);
        cx.notify();
    }

    fn on_toggle_metric_sidebar(
        &mut self,
        _: &ToggleMetricSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.metric_sidebar_compact = !self.metric_sidebar_compact;
        self.focus.focus(window);
        cx.notify();
    }

    fn on_toggle_bottom_inspector(
        &mut self,
        _: &ToggleBottomInspector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.bottom_inspector_visible {
            self.bottom_inspector_visible = false;
        } else if self.views.active().selected_panel_id.is_some() {
            self.bottom_inspector_visible = true;
            self.request_inspector(cx);
        }
        self.focus.focus(window);
        cx.notify();
    }

    fn on_show_metric_inspector(
        &mut self,
        _: &ShowMetricInspector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel_id) = self.views.active().selected_panel_id.clone() {
            self.show_metric_inspector(&panel_id, cx);
        }
        self.focus.focus(window);
    }

    fn metric_sidebar_width(&self) -> gpui::Pixels {
        if self.metric_sidebar_compact {
            px(48.)
        } else {
            self.theme.spacing.sidebar_width
        }
    }

    fn create_analysis_view(&mut self, cx: &mut Context<Self>) {
        self.store_active_view_state();
        self.views.create_empty();
        self.restore_active_view_state();
        self.refresh_catalog(cx);
        self.renaming_view = None;
        cx.notify();
    }

    fn duplicate_analysis_view(&mut self, cx: &mut Context<Self>) {
        self.store_active_view_state();
        self.views.duplicate_active();
        self.restore_active_view_state();
        self.refresh_catalog(cx);
        self.renaming_view = None;
        cx.notify();
    }

    fn activate_analysis_view(&mut self, view_id: &AnalysisViewId, cx: &mut Context<Self>) {
        if self.views.active().view_id == *view_id {
            return;
        }
        self.store_active_view_state();
        if self.views.activate(view_id) {
            self.restore_active_view_state();
            self.refresh_catalog(cx);
            self.renaming_view = None;
            cx.notify();
        }
    }

    fn close_analysis_view(&mut self, view_id: &AnalysisViewId, cx: &mut Context<Self>) {
        let active = self.views.active().view_id == *view_id;
        if active {
            self.store_active_view_state();
        }
        if self.views.close(view_id) {
            if active {
                self.restore_active_view_state();
                self.refresh_catalog(cx);
            }
            self.renaming_view = None;
            cx.notify();
        }
    }

    fn store_active_view_state(&mut self) {
        let view_id = self.views.active().view_id.clone();
        self.panel_reads.deactivate_view(&view_id);
        self.views.cancel_active_panel_reads();
        let view = self.views.active_mut();
        view.navigation = std::mem::take(&mut self.navigation);
        view.local_error = self.local_error.take();
    }

    fn restore_active_view_state(&mut self) {
        let view = self.views.active_mut();
        self.navigation = std::mem::take(&mut view.navigation);
        self.local_error = view.local_error.take();
        self.chart_adapter.borrow_mut().clear();
        self.track_adapters.clear();
        self.track_charts.clear();
        self.track_hovers.clear();
        self.metric_scroll = ListState::new(
            self.views.active().panels.len(),
            ListAlignment::Top,
            px(480.),
        );
        *self.track_viewport.borrow_mut() = TrackViewport::default();
        self.ruler_hover = None;
        self.track_pointer_hover = None;
        self.locked_cursor = None;
        self.drag = None;
        self.cancel_detail_refresh();
    }

    fn begin_rename_analysis_view(
        &mut self,
        view_id: AnalysisViewId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self
            .views
            .views()
            .iter()
            .find(|view| view.view_id == view_id)
        else {
            return;
        };
        self.view_name_draft.clone_from(&view.name);
        self.view_name_cursor = self.view_name_draft.len();
        self.renaming_view = Some(view_id);
        self.view_name_select_all = true;
        self.start_view_name_cursor_blink(cx);
        let focus = self.view_name_focus.clone();
        window.defer(cx, move |window, _| focus.focus(window));
    }

    fn on_view_name_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => self.finish_rename_analysis_view(true, cx),
            "escape" => self.finish_rename_analysis_view(false, cx),
            "left" => {
                if self.view_name_select_all {
                    self.view_name_cursor = 0;
                } else {
                    self.view_name_cursor =
                        previous_text_cursor(&self.view_name_draft, self.view_name_cursor);
                }
                self.view_name_select_all = false;
                self.start_view_name_cursor_blink(cx);
            }
            "right" => {
                if self.view_name_select_all {
                    self.view_name_cursor = self.view_name_draft.len();
                } else {
                    self.view_name_cursor =
                        next_text_cursor(&self.view_name_draft, self.view_name_cursor);
                }
                self.view_name_select_all = false;
                self.start_view_name_cursor_blink(cx);
            }
            "backspace" => {
                if self.view_name_select_all {
                    self.view_name_draft.clear();
                    self.view_name_cursor = 0;
                } else {
                    let previous =
                        previous_text_cursor(&self.view_name_draft, self.view_name_cursor);
                    self.view_name_draft
                        .replace_range(previous..self.view_name_cursor, "");
                    self.view_name_cursor = previous;
                }
                self.view_name_select_all = false;
                self.start_view_name_cursor_blink(cx);
            }
            _ if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.chars().any(char::is_control)
                {
                    if self.view_name_select_all {
                        self.view_name_draft.clear();
                        self.view_name_cursor = 0;
                    }
                    self.view_name_draft.insert_str(self.view_name_cursor, text);
                    self.view_name_cursor += text.len();
                    self.view_name_select_all = false;
                    self.start_view_name_cursor_blink(cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn finish_rename_analysis_view(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(view_id) = self.renaming_view.take() else {
            return;
        };
        if commit {
            self.views.rename(&view_id, &self.view_name_draft);
        }
        self.view_name_cursor = 0;
        self.view_name_select_all = false;
        self.stop_view_name_cursor_blink(cx);
    }

    fn start_view_name_cursor_blink(&mut self, cx: &mut Context<Self>) {
        self.view_name_cursor_epoch = self.view_name_cursor_epoch.saturating_add(1);
        self.view_name_cursor_visible = true;
        self.schedule_view_name_cursor_blink(cx);
        cx.notify();
    }

    fn schedule_view_name_cursor_blink(&mut self, cx: &mut Context<Self>) {
        let epoch = self.view_name_cursor_epoch;
        let timer = cx.background_executor().timer(Duration::from_millis(500));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.view_name_cursor_epoch != epoch || this.renaming_view.is_none() {
                    return;
                }
                this.view_name_cursor_visible = !this.view_name_cursor_visible;
                this.schedule_view_name_cursor_blink(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn stop_view_name_cursor_blink(&mut self, cx: &mut Context<Self>) {
        self.view_name_cursor_epoch = self.view_name_cursor_epoch.saturating_add(1);
        self.view_name_cursor_visible = false;
        cx.notify();
    }

    fn on_reset(&mut self, _: &ResetView, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel_detail_refresh();
        if self.navigation.reset_view() {
            self.sync_track_charts(cx);
            self.request_detail(cx);
        }
        cx.notify();
    }

    fn on_zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_from_keyboard(1.25, cx);
    }

    fn on_zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_from_keyboard(0.8, cx);
    }

    fn on_clear_locked_cursor(
        &mut self,
        _: &ClearLockedCursor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.locked_cursor.take().is_some() {
            cx.notify();
        }
    }

    fn zoom_from_keyboard(&mut self, factor: f64, cx: &mut Context<Self>) {
        let Some(selected) = self.navigation.brush().map(|brush| brush.selected()) else {
            return;
        };
        let anchor = selected.start() + selected.span() / 2.;
        if self
            .navigation
            .brush_mut()
            .is_none_or(|brush| brush.zoom_at(anchor, factor).is_err())
        {
            return;
        }
        self.defer_metric_repaint(cx);
        self.schedule_detail_refresh(cx);
    }

    fn on_step(&mut self, _: &UseStep, _: &mut Window, cx: &mut Context<Self>) {
        self.axis_picker_open = false;
        self.views.clear_active_timeline_extents();
        self.navigation.select_axis(AlignmentAxis::Step);
        self.request_overview(cx);
        cx.notify();
    }

    fn on_elapsed(&mut self, _: &UseElapsed, _: &mut Window, cx: &mut Context<Self>) {
        self.axis_picker_open = false;
        self.views.clear_active_timeline_extents();
        self.navigation.select_axis(AlignmentAxis::ElapsedTime);
        self.request_overview(cx);
        cx.notify();
    }

    fn activate_tree_project(
        &mut self,
        source_id: DataSourceId,
        project_id: ProjectId,
        cx: &mut Context<Self>,
    ) {
        let key = (source_id, project_id);
        if !self.expanded_projects.insert(key.clone()) {
            self.expanded_projects.remove(&key);
        }
        cx.notify();
    }

    fn toggle_tree_run(&mut self, run_ref: RunRef, cx: &mut Context<Self>) {
        self.toggle_run(run_ref, cx);
    }

    fn toggle_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        match self.views.toggle_active_run(run.clone()) {
            Ok(selected) => {
                self.local_error = None;
                if selected {
                    self.refresh_catalog(cx);
                    self.request_missing_panel_curves(cx);
                }
                if self.bottom_inspector_visible {
                    self.request_inspector(cx);
                }
            }
            Err(error) => self.local_error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn request_missing_panel_curves(&mut self, cx: &mut Context<Self>) {
        let runs = self.active_visible_runs();
        let viewport = self.navigation.selected_viewport();
        let viewport_state = self.track_viewport.borrow().clone();
        let panels = self.views.active().panels.clone();
        for (index, panel) in panels.into_iter().enumerate() {
            let missing_overview = runs
                .iter()
                .filter(|run| {
                    panel.overview.as_ref().is_none_or(|snapshot| {
                        !snapshot.series.iter().any(|curve| &curve.run_ref == *run)
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing_overview.is_empty() {
                self.request_panel_overview_for_runs(
                    &panel.panel_id,
                    missing_overview,
                    PanelReadMode::Merge,
                    cx,
                );
            }
            let Some(viewport) = viewport else {
                continue;
            };
            if !viewport_state.overscan.contains(&index) {
                continue;
            }
            let missing_detail = runs
                .iter()
                .filter(|run| {
                    panel.detail.as_ref().is_none_or(|snapshot| {
                        !snapshot.series.iter().any(|curve| &curve.run_ref == *run)
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing_detail.is_empty() {
                self.request_panel_detail_for_runs(
                    &panel.panel_id,
                    viewport,
                    viewport_state.physical_width.max(1),
                    missing_detail,
                    PanelReadMode::Merge,
                    cx,
                );
            }
        }
    }

    fn active_visible_runs(&self) -> Vec<RunRef> {
        self.views
            .active()
            .runs
            .iter()
            .filter(|run| !self.views.archived_runs().contains(run))
            .cloned()
            .collect()
    }

    fn available_metric_keys(&self) -> Vec<MetricKey> {
        let source_ids = self
            .active_visible_runs()
            .into_iter()
            .map(|run| run.source_id)
            .collect::<HashSet<_>>();
        self.sources
            .sources()
            .filter(|source| source_ids.contains(&source.source_id))
            .flat_map(|source| source.catalog.metric_keys.iter().cloned())
            .map(|metric| (metric.as_str().to_owned(), metric))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect()
    }

    fn set_run_baseline(&mut self, run: RunRef, cx: &mut Context<Self>) {
        let was_visible = self.active_visible_runs().contains(&run);
        if self.views.archived_runs().contains(&run) {
            self.views.restore_run(&run);
        }
        let baseline = (self.views.active().baseline.as_ref() != Some(&run)).then_some(run);
        if let Err(error) = self.views.set_active_baseline(baseline) {
            self.local_error = Some(error.to_string());
        } else {
            if !was_visible {
                self.request_missing_panel_curves(cx);
            }
            if self.bottom_inspector_visible {
                self.request_inspector(cx);
            }
        }
        cx.notify();
    }

    fn toggle_pinned_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        let was_visible = self.active_visible_runs().contains(&run);
        if self.views.archived_runs().contains(&run) {
            self.views.restore_run(&run);
        }
        if let Err(error) = self.views.toggle_active_pinned_run(run) {
            self.local_error = Some(error.to_string());
        } else if !was_visible {
            self.request_missing_panel_curves(cx);
        }
        cx.notify();
    }

    fn archive_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        let restoring = self.views.archived_runs().contains(&run);
        if restoring {
            self.views.restore_run(&run);
        } else {
            self.views.archive_run(run);
        }
        if restoring {
            self.request_missing_panel_curves(cx);
        }
        if self.bottom_inspector_visible {
            self.request_inspector(cx);
        }
        cx.notify();
    }

    fn set_project_placement(&mut self, project: ProjectRef, placement: ProjectPlacement) {
        match placement {
            ProjectPlacement::Pinned => self.views.pin_project(project),
            ProjectPlacement::Projects => {
                self.views.unpin_project(&project);
                self.views.restore_project(&project);
            }
            ProjectPlacement::Archived => self.views.archive_project(project),
        }
        self.project_menu = None;
    }

    fn remove_project(&mut self, project: ProjectRef, cx: &mut Context<Self>) {
        self.project_focuses.remove(&project);
        self.run_focuses.retain(|run, _| {
            run.source_id != project.source_id || run.project_id != project.project_id
        });
        self.views.remove_project(project.clone());
        self.project_menu = None;
        self.refresh_catalog(cx);
        self.request_overview(cx);
        self.request_inspector(cx);
        cx.notify();
    }

    fn project_listing_runs(&self, project: &SidebarProject) -> Vec<RunRef> {
        let baseline = self.views.active().baseline.as_ref();
        let pinned = &self.views.active().pinned_runs;
        let archived = self.views.archived_runs();
        project
            .runs
            .iter()
            .map(|run| {
                RunRef::new(
                    project.project_ref.source_id.clone(),
                    run.project_id.clone(),
                    run.run_id.clone(),
                )
            })
            .filter(|run| baseline != Some(run) && !pinned.contains(run) && !archived.contains(run))
            .collect()
    }

    fn toggle_project_runs(&mut self, project: &SidebarProject, cx: &mut Context<Self>) {
        let movable = self.project_listing_runs(project);
        let hide = movable
            .iter()
            .all(|run| self.views.active().runs.contains(run));
        for run in movable {
            let selected = self.views.active().runs.contains(&run);
            if selected == hide
                && let Err(error) = self.views.toggle_active_run(run)
            {
                self.local_error = Some(error.to_string());
                break;
            }
        }
        self.project_menu = None;
        if !hide {
            self.refresh_catalog(cx);
            self.request_missing_panel_curves(cx);
        }
        if self.bottom_inspector_visible {
            self.request_inspector(cx);
        }
        cx.notify();
    }

    fn select_metric(&mut self, metric_key: MetricKey, cx: &mut Context<Self>) {
        self.metric_picker_open = false;
        self.metric_filter.clear();
        let panel_id = self.views.select_active_metric(metric_key);
        self.reset_metric_track_schedule();
        self.request_panel_overview(&panel_id, cx);
        if self.bottom_inspector_visible {
            self.request_inspector(cx);
        }
        cx.notify();
    }

    fn reset_metric_track_schedule(&mut self) {
        let panel_count = self.views.active().panels.len();
        self.metric_scroll.reset(panel_count);
        *self.track_viewport.borrow_mut() = if panel_count == 0 {
            TrackViewport::default()
        } else {
            TrackViewport {
                visible: 0..panel_count.min(INITIAL_VISIBLE_TRACKS),
                overscan: 0..panel_count.min(INITIAL_OVERSCAN_TRACKS),
                logical_width_bits: self.overview_logical_width.max(1.).to_bits(),
                physical_width: self.overview_width.max(1),
            }
        };
    }

    fn show_metric_inspector(&mut self, panel_id: &MetricPanelId, cx: &mut Context<Self>) {
        if !self.views.select_active_panel(panel_id) {
            return;
        }
        self.bottom_inspector_visible = true;
        self.request_inspector(cx);
        cx.notify();
    }

    fn toggle_inspector_sort(&mut self, column: InspectorColumn, cx: &mut Context<Self>) {
        self.inspector_sort = match self.inspector_sort {
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
        self.inspector_column_widths[column.index()]
    }

    fn begin_inspector_column_resize(
        &mut self,
        column: InspectorColumn,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.inspector_column_resize = Some(InspectorColumnResize {
            column,
            start_x: event.position.x,
            start_width: self.inspector_column_width(column),
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn move_inspector_column_resize(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(resize) = self.inspector_column_resize else {
            return;
        };
        let width = (resize.start_width + f32::from(event.position.x - resize.start_x))
            .clamp(resize.column.minimum_width(), 600.);
        self.inspector_column_widths[resize.column.index()] = width;
        cx.notify();
    }

    fn finish_inspector_column_resize(&mut self, cx: &mut Context<Self>) {
        if self.inspector_column_resize.take().is_some() {
            cx.notify();
        }
    }

    fn begin_inspector_resize(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.inspector_resize = Some(InspectorResize {
            start_y: event.position.y,
            start_height: self.bottom_inspector_height,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn move_inspector_resize(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.inspector_resize else {
            return;
        };
        let maximum = (window.viewport_size().height - px(120.)).max(px(56.));
        self.bottom_inspector_height =
            (resize.start_height + resize.start_y - event.position.y).clamp(px(56.), maximum);
        cx.notify();
    }

    fn finish_inspector_resize(&mut self, cx: &mut Context<Self>) {
        if self.inspector_resize.take().is_some() {
            cx.notify();
        }
    }

    fn begin_sidebar_resize(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.sidebar_resize = Some(SidebarResize {
            start_x: event.position.x,
            start_width: self.project_sidebar_width,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn move_sidebar_resize(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.sidebar_resize else {
            return;
        };
        let maximum = (window.viewport_size().width - px(320.)).max(px(160.));
        self.project_sidebar_width = (resize.start_width + event.position.x - resize.start_x)
            .clamp(px(160.), maximum.min(px(600.)));
        cx.notify();
    }

    fn finish_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_resize.take().is_some() {
            cx.notify();
        }
    }

    fn begin_metric_resize(
        &mut self,
        panel_id: MetricPanelId,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.views.active_panel(&panel_id) else {
            return;
        };
        self.metric_resize = Some(MetricResize {
            panel_id,
            start_y: event.position.y,
            start_height: panel.row_height,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn move_metric_resize(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(resize) = self.metric_resize.clone() else {
            return;
        };
        let height = resize.start_height + f32::from(event.position.y - resize.start_y);
        if self.views.set_active_panel_height(&resize.panel_id, height)
            && let Some(index) = self
                .views
                .active()
                .panels
                .iter()
                .position(|panel| panel.panel_id == resize.panel_id)
        {
            self.metric_scroll.splice(index..index + 1, 1);
        }
        cx.notify();
    }

    fn finish_metric_resize(&mut self, cx: &mut Context<Self>) {
        if self.metric_resize.take().is_some() {
            cx.notify();
        }
    }

    fn remove_metric_panel(&mut self, panel_id: &MetricPanelId, cx: &mut Context<Self>) {
        if self.views.remove_active_panel(panel_id) {
            self.reset_metric_track_schedule();
            self.track_adapters.remove(panel_id);
            self.track_charts.remove(panel_id);
            self.track_hovers.remove(panel_id);
            if let Some(home) = self
                .views
                .active()
                .timeline_extents
                .values()
                .copied()
                .reduce(|left, right| {
                    AlignmentViewport::new(
                        left.start().min(right.start()),
                        left.end().max(right.end()),
                    )
                    .expect("valid panel extents must have a valid union")
                })
            {
                self.navigation.set_timeline_home(home);
            } else {
                self.navigation.clear_timeline();
            }
            cx.notify();
        }
    }

    fn on_filter_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "left" => {
                self.run_filter_cursor =
                    previous_text_cursor(&self.run_filter, self.run_filter_cursor);
            }
            "right" => {
                self.run_filter_cursor = next_text_cursor(&self.run_filter, self.run_filter_cursor);
            }
            "backspace" => {
                let previous = previous_text_cursor(&self.run_filter, self.run_filter_cursor);
                self.run_filter
                    .replace_range(previous..self.run_filter_cursor, "");
                self.run_filter_cursor = previous;
            }
            "escape" => {
                self.run_filter.clear();
                self.run_filter_cursor = 0;
            }
            _ if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.chars().any(char::is_control)
                {
                    self.run_filter.insert_str(self.run_filter_cursor, text);
                    self.run_filter_cursor += text.len();
                }
            }
            _ => return,
        }
        self.start_filter_cursor_blink(cx);
        cx.stop_propagation();
    }

    fn start_filter_cursor_blink(&mut self, cx: &mut Context<Self>) {
        self.filter_cursor_epoch = self.filter_cursor_epoch.saturating_add(1);
        self.filter_cursor_visible = true;
        self.schedule_filter_cursor_blink(cx);
        cx.notify();
    }

    fn schedule_filter_cursor_blink(&mut self, cx: &mut Context<Self>) {
        let epoch = self.filter_cursor_epoch;
        let timer = cx.background_executor().timer(Duration::from_millis(500));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.filter_cursor_epoch != epoch {
                    return;
                }
                this.filter_cursor_visible = !this.filter_cursor_visible;
                this.schedule_filter_cursor_blink(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn stop_filter_cursor_blink(&mut self, cx: &mut Context<Self>) {
        self.filter_cursor_epoch = self.filter_cursor_epoch.saturating_add(1);
        self.filter_cursor_visible = false;
        cx.notify();
    }

    fn on_metric_filter_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "backspace" => {
                self.metric_filter.pop();
            }
            "escape" if self.metric_filter.is_empty() => {
                self.metric_picker_open = false;
                self.focus.focus(window);
            }
            "escape" => self.metric_filter.clear(),
            _ if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.chars().any(char::is_control)
                {
                    self.metric_filter.push_str(text);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn render_project_sidebar(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        self.render_converged_project_sidebar(window, cx)
    }

    fn render_workspace(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        self.render_metric_workspace(window, cx)
    }
    fn render_metric_workspace(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        self.reconcile_track_schedule(cx);
        self.sync_track_charts(cx);
        let theme = self.theme;
        let panels: Rc<[MetricPanel]> = self.views.active().panels.clone().into();
        let selected = panels
            .iter()
            .map(|panel| panel.metric_key.clone())
            .collect::<HashSet<_>>();
        let available = self.available_metric_keys();
        let metric_picker = self.render_metric_picker(available, &selected, cx);
        let axis_picker = self.render_axis_picker(cx);
        let metric_sidebar_width = self.metric_sidebar_width();
        let timeline = self.render_overview(cx);
        let ruler = self.render_ruler(window, cx);
        let cursor_layer = self.navigation.brush().map(|brush| {
            div()
                .id("metric-cursor-overlay")
                .debug_selector(|| "metric-cursor-overlay".to_owned())
                .absolute()
                .left(metric_sidebar_width)
                .right_0()
                .top_0()
                .bottom_0()
                .child(
                    renderer::cursor_canvas(
                        None,
                        brush.selected(),
                        self.hover_cursor_axis(),
                        self.locked_cursor,
                        false,
                    )
                    .absolute()
                    .size_full(),
                )
        });
        let list_panels = Rc::clone(&panels);
        let panel_count = panels.len();
        let scroll = self.metric_scroll.clone();
        if scroll.item_count() != panel_count {
            scroll.reset(panel_count);
        }
        let physical_width = self.overview_width.max(1);
        let logical_width = self.overview_logical_width.max(1.);
        if self.track_viewport.borrow().overscan.is_empty() && panel_count > 0 {
            let visible = 0..panel_count.min(INITIAL_VISIBLE_TRACKS);
            *self.track_viewport.borrow_mut() = TrackViewport {
                overscan: 0..panel_count.min(INITIAL_OVERSCAN_TRACKS),
                visible,
                logical_width_bits: logical_width.to_bits(),
                physical_width,
            };
        }
        let viewport_state = Rc::clone(&self.track_viewport);
        let viewer_id = cx.entity().entity_id();
        let handler_scroll = scroll.clone();
        scroll.set_scroll_handler(move |event, _, cx| {
            let visible_len = event.visible_range.len().max(1);
            let scroll = handler_scroll.clone();
            let viewport_state = Rc::clone(&viewport_state);
            cx.defer(move |cx| {
                let start = scroll.logical_scroll_top().item_ix.min(panel_count);
                let visible = start..start.saturating_add(visible_len).min(panel_count);
                let next = TrackViewport {
                    overscan: visible.start.saturating_sub(visible_len)
                        ..visible.end.saturating_add(visible_len).min(panel_count),
                    visible,
                    logical_width_bits: logical_width.to_bits(),
                    physical_width,
                };
                if *viewport_state.borrow() != next {
                    *viewport_state.borrow_mut() = next;
                    cx.notify(viewer_id);
                }
            });
        });
        let viewer = cx.entity().downgrade();
        let render_panels = Rc::clone(&list_panels);
        let inspector = self
            .bottom_inspector_visible
            .then(|| self.render_bottom_inspector(cx));

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
                            let panel = render_panels.get(index).cloned();
                            viewer
                                .update(cx, |this, cx| {
                                    panel.map_or_else(
                                        || div().into_any_element(),
                                        |panel| {
                                            let height = px(panel.row_height);
                                            this.render_metric_row(panel, height, cx)
                                                .into_any_element()
                                        },
                                    )
                                })
                                .unwrap_or_else(|_| div().into_any_element())
                        })
                        .size_full(),
                    )
                    .children(cursor_layer),
            )
            .children(inspector)
    }

    fn render_metric_picker(
        &mut self,
        available: Vec<MetricKey>,
        selected: &HashSet<MetricKey>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme;
        let available = available
            .into_iter()
            .filter(|metric| !selected.contains(metric))
            .collect::<Vec<_>>();
        let query = self.metric_filter.trim().to_lowercase();
        let candidates = available
            .iter()
            .filter(|metric| query.is_empty() || metric.as_str().to_lowercase().contains(&query))
            .cloned()
            .collect::<Vec<_>>();
        let filter_focus = self.metric_filter_focus.clone();
        let picker_open = self.metric_picker_open;
        let mut picker = div().relative().flex().items_center().child(
            components::top_bar_icon_button("add-metric", theme, self.metric_picker_open, false)
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
        if self.metric_picker_open {
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
                                    if this.dismiss_popovers() {
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
                                        .h(theme.spacing.control_height)
                                        .px_2()
                                        .border_1()
                                        .border_color(theme.colors.border)
                                        .rounded(theme.spacing.corner_radius)
                                        .flex()
                                        .items_center()
                                        .cursor_text()
                                        .on_key_down(cx.listener(Self::on_metric_filter_key))
                                        .on_click(move |_, window, _| filter_focus.focus(window))
                                        .text_color(if self.metric_filter.is_empty() {
                                            theme.colors.text_muted
                                        } else {
                                            theme.colors.text
                                        })
                                        .child(if self.metric_filter.is_empty() {
                                            "Filter available metrics".to_owned()
                                        } else {
                                            self.metric_filter.clone()
                                        }),
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

    fn render_axis_picker(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let absolute = self.curve_axis() == CurveAxis::AbsoluteTime;
        let picker_open = self.axis_picker_open;
        let mut picker = div().relative().flex().items_center().child(
            components::top_bar_icon_button("axis-picker", theme, self.axis_picker_open, false)
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
        if self.axis_picker_open {
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
                                    if this.dismiss_popovers() {
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
                                            this.views.clear_active_timeline_extents();
                                            this.navigation.select_axis(AlignmentAxis::Step);
                                            this.request_overview(cx);
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
                                            this.views.clear_active_timeline_extents();
                                            this.navigation.select_axis(AlignmentAxis::ElapsedTime);
                                            this.request_overview(cx);
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

    fn render_ruler(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let (selected, home) = self.navigation.brush().map_or((None, None), |brush| {
            (Some(brush.selected()), Some(brush.home()))
        });
        let mut ticks = selected
            .map(|range| pulseon_chart_core::linear_ticks(range, 6))
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
        let hover_axis = self.hover_cursor_axis();
        let locked_cursor = self.locked_cursor;
        let plot_left = self.metric_sidebar_width();
        let plot_left_px = f32::from(plot_left);
        let (_, plot_width) = self.ruler_plot_geometry(window);
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
                        renderer::cursor_canvas(None, range, hover_axis, locked_cursor, true)
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
                                    this.track_pointer_hover = None;
                                    this.track_hovers.clear();
                                    this.ruler_hover = this.ruler_axis_at(event.position, window);
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
                                this.locked_cursor = this.ruler_axis_at(position, window);
                                cx.notify();
                            }))
                            .on_hover(cx.listener(|this, hovered, _, cx| {
                                if !hovered {
                                    this.ruler_hover = None;
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

    fn ruler_axis_at(&self, position: gpui::Point<gpui::Pixels>, window: &Window) -> Option<f64> {
        let range = self.navigation.brush()?.selected();
        let (left, width) = self.ruler_plot_geometry(window);
        let ratio = (f64::from(position.x - left) / f64::from(width)).clamp(0., 1.);
        Some(range.start() + range.span() * ratio)
    }

    fn ruler_plot_geometry(&self, window: &Window) -> (gpui::Pixels, gpui::Pixels) {
        let left = if self.project_sidebar_visible {
            self.project_sidebar_width
        } else {
            px(0.)
        } + self.metric_sidebar_width();
        let width = (window.viewport_size().width - left).max(px(1.));
        (left, width)
    }

    fn scroll_ruler(&mut self, event: &ScrollWheelEvent, window: &Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(16.));
        let delta = if delta.x.abs() > delta.y.abs() {
            f32::from(delta.x)
        } else {
            f32::from(delta.y)
        };
        let Some(before) = self.navigation.brush().map(|brush| brush.selected()) else {
            return;
        };
        let zooming = event.modifiers.platform || event.modifiers.control;
        let transformed = if zooming {
            self.ruler_axis_at(event.position, window)
                .zip(self.navigation.brush_mut())
                .is_some_and(|(anchor, brush)| {
                    let factor = f64::from((-delta * 0.002).exp().clamp(0.5, 2.));
                    brush.zoom_at(anchor, factor).is_ok()
                })
        } else {
            let (_, width) = self.ruler_plot_geometry(window);
            let axis_delta = -f64::from(delta) * before.span() / f64::from(width);
            self.navigation
                .brush_mut()
                .is_some_and(|brush| brush.pan_by(axis_delta).is_ok())
        };
        if transformed
            && self
                .navigation
                .brush()
                .is_some_and(|brush| brush.selected() != before)
        {
            if zooming {
                self.defer_metric_repaint(cx);
            } else {
                self.sync_track_charts(cx);
                cx.notify();
            }
            self.schedule_detail_refresh(cx);
            cx.stop_propagation();
        }
    }

    fn begin_ruler_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let x = f64::from(event.position.x);
        self.drag = Some(DragGesture::Ruler {
            origin_x: x,
            last_x: x,
            moved: false,
        });
        cx.notify();
    }

    fn move_ruler_drag(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
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
            .ruler_axis_at(previous, window)
            .zip(self.ruler_axis_at(event.position, window))
            .map(|(previous, current)| previous - current);
        if let Some(delta) = delta
            && let Some(brush) = self.navigation.brush_mut()
        {
            let _ = brush.pan_by(delta);
        }
        self.sync_track_charts(cx);
        self.drag = Some(DragGesture::Ruler {
            origin_x,
            last_x: current_x,
            moved,
        });
        cx.notify();
    }

    fn finish_ruler_drag(
        &mut self,
        position: Option<gpui::Point<gpui::Pixels>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(DragGesture::Ruler { moved, .. }) = self.drag.take() else {
            return;
        };
        if moved {
            self.request_detail(cx);
        } else if let Some(position) = position {
            self.locked_cursor = self.ruler_axis_at(position, window);
        }
        cx.notify();
    }

    fn render_bottom_inspector(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let panel = self
            .views
            .active()
            .selected_panel_id
            .as_ref()
            .and_then(|panel_id| self.views.active_panel(panel_id))
            .cloned();
        let hover_cursor = self.hover_cursor_axis();
        let snapshot = panel.as_ref().and_then(|panel| panel.inspector.as_deref());
        let baseline = self.views.active().baseline.clone();
        let pinned = self.views.active().pinned_runs.clone();
        let visible_runs = self.active_visible_runs();
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
                            locked_axis: self.locked_cursor,
                            hover_axis: hover_cursor,
                            sort: self.inspector_sort,
                        },
                    )
                });
            self.render_inspector_table(rows, cx)
        };

        div()
            .id("bottom-inspector")
            .debug_selector(|| "bottom-inspector".to_owned())
            .h(self.bottom_inspector_height)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .bg(theme.colors.surface)
            .border_t_1()
            .border_color(theme.colors.border)
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
                horizontal_resize_handle(
                    SharedString::from("bottom-inspector-resize"),
                    theme,
                    self.inspector_resize.is_some(),
                    true,
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
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme;
        let sort = self.inspector_sort;
        let mut column_widths = self.inspector_column_widths;
        column_widths[InspectorColumn::Project.index()] = inspector_project_width(&rows);
        let resizing_column = self.inspector_column_resize.map(|resize| resize.column);
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
            theme,
            cx,
        )
        .bg(theme.colors.surface)
        .border_r_1()
        .border_color(theme.colors.border);
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
            .track_scroll(&self.inspector_horizontal_scroll)
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
                let color = theme
                    .colors
                    .series_color(renderer::series_color_index(&run_ref));
                let highlighted = self.hovered_run.as_ref() == Some(&run_ref);
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
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        if *hovered {
                            this.hovered_run = Some(run_ref.clone());
                        } else if this.hovered_run.as_ref() == Some(&run_ref) {
                            this.hovered_run = None;
                        }
                        cx.notify();
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
                let row_id = run_ref.cache_key();
                let row_run_id = run_ref.run_id.as_str().to_owned();
                let values = INSPECTOR_COLUMNS
                    .into_iter()
                    .skip(1)
                    .map(|column| (column, inspector_cell_text(column, row, baseline.as_ref())));
                let highlighted = self.hovered_run.as_ref() == Some(&run_ref);
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
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        if *hovered {
                            this.hovered_run = Some(run_ref.clone());
                        } else if this.hovered_run.as_ref() == Some(&run_ref) {
                            this.hovered_run = None;
                        }
                        cx.notify();
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
            .track_scroll(&self.inspector_horizontal_scroll)
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
            .track_scroll(&self.inspector_vertical_scroll)
            .child(body);
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(header)
            .child(vertical_body)
    }

    fn panel_is_scheduled(&self, panel_id: &MetricPanelId) -> bool {
        let state = self.track_viewport.borrow();
        self.views
            .active()
            .panels
            .iter()
            .enumerate()
            .any(|(index, panel)| &panel.panel_id == panel_id && state.overscan.contains(&index))
    }

    fn should_schedule_panel_detail(
        &self,
        panel_id: &MetricPanelId,
        viewport: AlignmentViewport,
        physical_width: u32,
    ) -> bool {
        self.views.active_panel(panel_id).is_some_and(|panel| {
            panel.needs_detail(viewport, physical_width)
                && (!self.detail_refresh_pending
                    || panel.detail.is_none()
                    || panel.physical_width != physical_width)
        })
    }

    fn reconcile_track_schedule(&mut self, cx: &mut Context<Self>) {
        let state = self.track_viewport.borrow().clone();
        if state.overscan.is_empty() || state.logical_width_bits == 0 {
            return;
        }
        let Some(detail_viewport) = self.navigation.selected_viewport() else {
            return;
        };
        let panel_count = self.views.active().panels.len();
        let scheduled = state.overscan.start.min(panel_count)..state.overscan.end.min(panel_count);
        let panels = self.views.active().panels[scheduled].to_vec();
        let visible_runs = self.active_visible_runs();
        let scheduled_ids = panels
            .iter()
            .map(|panel| panel.panel_id.clone())
            .collect::<HashSet<_>>();
        self.track_adapters
            .retain(|panel_id, _| scheduled_ids.contains(panel_id));
        self.track_charts
            .retain(|panel_id, _| scheduled_ids.contains(panel_id));
        self.track_hovers
            .retain(|panel_id, _| scheduled_ids.contains(panel_id));
        let logical_width = f32::from_bits(state.logical_width_bits) as f64;
        for panel in panels {
            let canvas_height = f64::from(panel.row_height)
                - f64::from(METRIC_TRACK_VERTICAL_PADDING * 2. + METRIC_TRACK_SEPARATOR_WIDTH);
            let canvas = CanvasSize::new(logical_width, canvas_height.max(1.)).ok();
            if let Some((snapshot, revision, viewport)) = track_chart_frame(
                &panel,
                self.navigation.brush().map(|brush| brush.selected()),
                &visible_runs,
            ) && let Some(canvas) = canvas
            {
                self.track_adapters
                    .entry(panel.panel_id.clone())
                    .or_insert_with(|| Rc::new(RefCell::new(ChartAdapter::default())))
                    .borrow_mut()
                    .warm_projection(&snapshot, revision, viewport, canvas, &visible_runs);
            }
            if self.should_schedule_panel_detail(
                &panel.panel_id,
                detail_viewport,
                state.physical_width.max(1),
            ) {
                self.request_panel_detail(
                    &panel.panel_id,
                    detail_viewport,
                    state.physical_width.max(1),
                    cx,
                );
            }
        }
    }

    fn render_metric_row(
        &mut self,
        panel: MetricPanel,
        row_height: gpui::Pixels,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme;
        let panel_id = panel.panel_id.clone();
        let remove_id = panel_id.clone();
        let select_id = panel_id.clone();
        let drag_id = panel_id.clone();
        let finish_id = panel_id.clone();
        let resize_id = panel_id.clone();
        let resizing = self
            .metric_resize
            .as_ref()
            .is_some_and(|resize| resize.panel_id == panel_id);
        let selected = self.views.active().selected_panel_id.as_ref() == Some(&panel_id);
        let visible_run_count = self.active_visible_runs().len();
        let error_count = panel.source_errors.len();
        let drawable_count = panel.detail.as_ref().map_or(0, |snapshot| {
            snapshot
                .series
                .iter()
                .filter(|series| series.chart_series.is_some())
                .count()
        });
        let metadata = if error_count > 0 {
            format!("{visible_run_count} Runs · {error_count} source errors")
        } else if panel.detail.is_none() {
            format!("{visible_run_count} Runs · loading")
        } else {
            format!("{visible_run_count} Runs · {drawable_count} drawable")
        };
        let track = self.render_metric_track(&panel, cx);
        div()
            .relative()
            .flex()
            .w_full()
            .h(row_height)
            .min_h(row_height)
            .border_b_1()
            .border_color(theme.colors.border)
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
                    .w(self.metric_sidebar_width())
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_metric_inspector(&select_id, cx);
                    }))
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
                                    .text_color(if error_count > 0 {
                                        theme.colors.error_text
                                    } else {
                                        theme.colors.text_muted
                                    })
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
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_metric_panel(&remove_id, cx);
                                cx.stop_propagation();
                            }))
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.begin_track_drag(drag_id.clone(), event, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if event.dragging() {
                            this.move_detail_drag(event, cx);
                        }
                    }))
                    .on_click(cx.listener(move |this, event, _, cx| {
                        this.finish_track_click(&finish_id, event, cx);
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                            this.finish_moved_track_drag(cx);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                            this.finish_moved_track_drag(cx);
                        }),
                    )
                    .child(track),
            )
            .child(
                horizontal_resize_handle(
                    SharedString::from(format!("metric-resize:{}", resize_id.as_str())),
                    theme,
                    resizing,
                    false,
                )
                .debug_selector({
                    let resize_id = resize_id.clone();
                    move || format!("metric-resize:{}", resize_id.as_str())
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.begin_metric_resize(resize_id.clone(), event, cx);
                    }),
                ),
            )
    }

    fn render_metric_track(&mut self, panel: &MetricPanel, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let panel_id = panel.panel_id.clone();
        let adapter = Rc::clone(
            self.track_adapters
                .entry(panel_id.clone())
                .or_insert_with(|| Rc::new(RefCell::new(ChartAdapter::default()))),
        );
        if panel.detail.is_none() {
            let message = if panel.is_pending(ReadKind::Detail) {
                "Loading viewport…"
            } else if panel.overview.is_some() {
                "No drawable evidence is available for this metric."
            } else {
                "Loading metric extent…"
            };
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.text_muted)
                .child(message);
        }
        let visible_runs: Rc<[RunRef]> = self.active_visible_runs().into();
        let selected = self.navigation.brush().map(|brush| brush.selected());
        let Some((snapshot, revision, viewport)) =
            track_chart_frame(panel, selected, &visible_runs)
        else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("No drawable evidence is available in this viewport.");
        };
        let paint_adapter = Rc::clone(&adapter);
        let hit_panel = panel_id.clone();
        let zoom_panel = panel_id.clone();
        let leave_panel = panel_id.clone();
        let baseline = self.views.active().baseline.clone();
        let emphasized_run = self
            .hovered_run
            .clone()
            .filter(|run| visible_runs.contains(run));
        let chart = if let Some(chart) = self.track_charts.get(&panel_id).cloned() {
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
                renderer::DetailChart::new(
                    Rc::clone(&paint_adapter),
                    snapshot.clone(),
                    revision,
                    viewport,
                    baseline.clone(),
                    emphasized_run.clone(),
                    Rc::clone(&visible_runs),
                )
            });
            self.track_charts.insert(panel_id.clone(), chart.clone());
            chart
        };
        let callout_axis = self.curve_axis();
        let locked_sidebar_callout =
            emphasized_run
                .as_ref()
                .zip(self.locked_cursor)
                .and_then(|(run, axis)| {
                    adapter
                        .borrow()
                        .points_at_axis(&snapshot, viewport, axis, &visible_runs)
                        .into_iter()
                        .find(|hover| &hover.run_ref == run)
                });
        let sidebar_locked = locked_sidebar_callout.is_some();
        let hover = self.track_hovers.get(&panel_id).cloned();
        let mut callouts = locked_sidebar_callout
            .map_or_else(
                || {
                    if emphasized_run.is_some() {
                        Vec::new()
                    } else {
                        hover.map_or_else(
                            || {
                                self.ruler_hover.map_or_else(Vec::new, |axis| {
                                    adapter.borrow().points_at_axis(
                                        &snapshot,
                                        viewport,
                                        axis,
                                        &visible_runs,
                                    )
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
                let delta = baseline
                    .as_ref()
                    .and_then(|baseline| baseline_delta(panel, baseline, &hover));
                (hover, delta)
            })
            .collect::<Vec<_>>();
        if !sidebar_locked {
            let pinned = &self.views.active().pinned_runs;
            callouts.sort_by_key(|(hover, _)| {
                if baseline.as_ref() == Some(&hover.run_ref) {
                    0
                } else if pinned.contains(&hover.run_ref) {
                    1
                } else {
                    2
                }
            });
        }
        callouts.truncate(1);
        let tooltip_anchor = callouts.first().map(|(hover, _)| {
            (
                hover.canvas_position.x,
                px(METRIC_TRACK_VERTICAL_PADDING) + hover.canvas_position.y,
                hover.align_left,
            )
        });
        let tooltip_rows = callouts
            .first()
            .map(|(hover, delta)| {
                vec![(
                    Some(
                        theme
                            .colors
                            .series_color(renderer::series_color_index(&hover.run_ref)),
                    ),
                    hover_value_label(callout_axis, hover, *delta),
                )]
            })
            .unwrap_or_default();
        let callout_panel = panel_id.clone();
        let tooltip = tooltip_anchor
            .zip((!tooltip_rows.is_empty()).then_some(tooltip_rows))
            .map(move |((x, y, align_left), rows)| {
                let height = px(22.);
                let width = px(track_tooltip_width(
                    rows.iter().map(|(_, label)| label.as_str()),
                ));
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
                        renderer::callout_shell(
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
                            .children(rows.into_iter().map(|(color, label)| {
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
                                            .bg(color.unwrap_or(theme.colors.transparent)),
                                    )
                                    .child(
                                        div()
                                            .debug_selector(|| "track-tooltip-label".to_owned())
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .child(label),
                                    )
                            })),
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
                    .child(renderer::cached_detail_chart(chart))
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                        if !event.dragging() {
                            this.update_track_hover(&hit_panel, event, cx);
                        }
                    }))
                    .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                        this.zoom_track(&zoom_panel, event, cx);
                    }))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        if !hovered {
                            this.track_hovers.remove(&leave_panel);
                            if this
                                .track_pointer_hover
                                .as_ref()
                                .is_some_and(|(panel_id, _)| panel_id == &leave_panel)
                            {
                                this.track_pointer_hover = None;
                            }
                            cx.notify();
                        }
                    })),
            )
            .children(tooltip)
    }

    fn sync_track_charts(&mut self, cx: &mut Context<Self>) {
        let visible_runs: Rc<[RunRef]> = self.active_visible_runs().into();
        let selected = self.navigation.brush().map(|brush| brush.selected());
        let baseline = self.views.active().baseline.clone();
        let emphasized_run = self
            .hovered_run
            .clone()
            .filter(|run| visible_runs.contains(run));
        let panels = self.views.active().panels.clone();
        for panel in panels {
            let Some(chart) = self.track_charts.get(&panel.panel_id).cloned() else {
                continue;
            };
            let Some((snapshot, revision, viewport)) =
                track_chart_frame(&panel, selected, &visible_runs)
            else {
                continue;
            };
            chart.update(cx, |chart, cx| {
                let changed = chart.update(
                    snapshot,
                    revision,
                    viewport,
                    baseline.clone(),
                    emphasized_run.clone(),
                    Rc::clone(&visible_runs),
                );
                if changed {
                    cx.notify();
                }
            });
        }
    }

    fn defer_metric_repaint(&mut self, cx: &mut Context<Self>) {
        if self.metric_repaint_pending {
            return;
        }
        self.metric_repaint_pending = true;
        // CONTEXT: The next foreground turn mirrors the repaint caused by a later input event,
        // after GPUI has finished dispatching the viewport-changing event.
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| {
                this.metric_repaint_pending = false;
                this.sync_track_charts(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn render_overview(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let Some(brush) = self.navigation.brush() else {
            return div().h_full();
        };
        let (snapshot, revision) = self
            .views
            .active()
            .selected_panel_id
            .as_ref()
            .and_then(|panel_id| self.views.active_panel(panel_id))
            .map_or((None, 0), |panel| {
                (panel.overview.clone(), panel.overview_revision)
            });
        let adapter = Rc::clone(&self.chart_adapter);
        let visible_runs: Rc<[RunRef]> = self.active_visible_runs().into();
        let emphasized_run = self
            .hovered_run
            .clone()
            .filter(|run| visible_runs.contains(run));
        div().h_full().child(
            div()
                .id("overview-chart")
                .debug_selector(|| "overview-chart".to_owned())
                .focusable()
                .h_full()
                .w_full()
                .relative()
                .cursor_pointer()
                .bg(theme.colors.surface)
                .child(
                    renderer::timeline_canvas(
                        adapter,
                        brush,
                        snapshot,
                        revision,
                        emphasized_run,
                        visible_runs,
                    )
                    .size_full(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.begin_brush_drag(event, cx);
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    if event.dragging() {
                        this.move_brush_drag(event, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| this.finish_drag(cx)),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| this.finish_drag(cx)),
                ),
        )
    }

    fn begin_brush_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(brush) = self.navigation.brush() else {
            return;
        };
        let adapter = self.chart_adapter.borrow();
        self.drag = match adapter.brush_target(brush, event.position) {
            Some(renderer::BrushDragTarget::Start) => Some(DragGesture::BrushStart),
            Some(renderer::BrushDragTarget::End) => Some(DragGesture::BrushEnd),
            Some(renderer::BrushDragTarget::Window) => adapter
                .overview_axis_at(brush, event.position)
                .map(|last_axis| DragGesture::BrushWindow { last_axis }),
            None => None,
        };
        cx.notify();
    }

    fn move_brush_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(mut gesture) = self.drag.clone() else {
            return;
        };
        let Some(brush) = self.navigation.brush() else {
            return;
        };
        let Some(axis) = self
            .chart_adapter
            .borrow()
            .overview_axis_at(brush, event.position)
        else {
            return;
        };
        if let Some(brush) = self.navigation.brush_mut() {
            update_brush_drag(brush, &mut gesture, axis);
        }
        self.drag = Some(gesture);
        self.sync_track_charts(cx);
        cx.notify();
    }

    fn begin_track_drag(
        &mut self,
        panel_id: MetricPanelId,
        event: &MouseDownEvent,
        _cx: &mut Context<Self>,
    ) {
        self.drag = Some(DragGesture::Detail {
            panel_id: panel_id.clone(),
            origin_x: f64::from(event.position.x),
            last_x: f64::from(event.position.x),
            moved: false,
        });
        self.track_hovers.remove(&panel_id);
    }

    fn move_detail_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(DragGesture::Detail {
            panel_id,
            origin_x,
            mut last_x,
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
        let Some(range) = self.navigation.brush().map(|brush| brush.selected()) else {
            self.drag = Some(DragGesture::Detail {
                panel_id,
                origin_x,
                last_x: current_x,
                moved,
            });
            return;
        };
        let delta = self.track_adapters.get(&panel_id).and_then(|adapter| {
            adapter
                .borrow()
                .detail_pan_delta(range, last_x, event.position)
        });
        if let Some(delta) = delta
            && let Some(brush) = self.navigation.brush_mut()
        {
            let _ = brush.pan_by(delta);
        }
        last_x = current_x;
        self.drag = Some(DragGesture::Detail {
            panel_id,
            origin_x,
            last_x,
            moved,
        });
        self.sync_track_charts(cx);
        cx.notify();
    }

    fn finish_track_click(
        &mut self,
        panel_id: &MetricPanelId,
        event: &gpui::ClickEvent,
        cx: &mut Context<Self>,
    ) {
        let click_position = match event {
            gpui::ClickEvent::Mouse(event) => {
                let delta = event.up.position - event.down.position;
                (f32::from(delta.x).abs() < 3. && f32::from(delta.y).abs() < 3.)
                    .then_some(event.up.position)
            }
            gpui::ClickEvent::Keyboard(_) => None,
        };
        if let Some(position) = click_position {
            self.drag = None;
            let range = self.navigation.brush().map(|brush| brush.selected());
            self.locked_cursor = range.and_then(|range| {
                self.track_adapters
                    .get(panel_id)
                    .and_then(|adapter| adapter.borrow().detail_axis_at(range, position))
            });
            self.show_metric_inspector(panel_id, cx);
        } else if matches!(event, gpui::ClickEvent::Keyboard(_)) {
            self.show_metric_inspector(panel_id, cx);
        }
    }

    fn finish_moved_track_drag(&mut self, cx: &mut Context<Self>) {
        if self
            .drag
            .as_ref()
            .is_some_and(|gesture| matches!(gesture, DragGesture::Detail { moved: true, .. }))
        {
            self.drag.take();
            self.request_detail(cx);
            cx.notify();
        }
    }

    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        let should_refresh = self
            .drag
            .take()
            .is_some_and(|gesture| !matches!(gesture, DragGesture::Detail { moved: false, .. }));
        if should_refresh {
            self.request_detail(cx);
            cx.notify();
        }
    }

    fn zoom_track(
        &mut self,
        panel_id: &MetricPanelId,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.navigation.brush().map(|brush| brush.selected()) else {
            return;
        };
        let Some(anchor) = self
            .track_adapters
            .get(panel_id)
            .and_then(|adapter| adapter.borrow().detail_axis_at(range, event.position))
        else {
            return;
        };
        let delta = f32::from(event.delta.pixel_delta(px(16.)).y);
        let factor = f64::from((-delta / 240.).exp().clamp(0.5, 2.));
        if self
            .navigation
            .brush_mut()
            .is_none_or(|brush| brush.zoom_at(anchor, factor).is_err())
        {
            return;
        }
        self.defer_metric_repaint(cx);
        self.schedule_detail_refresh(cx);
        cx.stop_propagation();
    }

    fn update_track_hover(
        &mut self,
        panel_id: &MetricPanelId,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        self.ruler_hover = None;
        self.track_pointer_hover = None;
        let Some(snapshot) = self
            .views
            .active_panel(panel_id)
            .and_then(|panel| panel.detail.clone())
        else {
            return;
        };
        let visible_runs = self.active_visible_runs();
        let Some(viewport) = renderer::detail_viewport(
            &snapshot,
            self.navigation.brush().map(|brush| brush.selected()),
            Some(&visible_runs),
        ) else {
            return;
        };
        let Some(adapter) = self.track_adapters.get(panel_id) else {
            return;
        };
        let adapter = adapter.borrow();
        let pointer_axis = adapter.detail_axis_at(viewport.x, event.position);
        let pointer_anchor = adapter.detail_pointer_anchor(event.position);
        let mut hover = adapter.hit_test(&snapshot, viewport, event.position, &visible_runs);
        if let (Some(hover), Some((x, align_left))) = (&mut hover, pointer_anchor) {
            hover.canvas_position.x = x;
            hover.align_left = align_left;
        }
        drop(adapter);
        self.track_pointer_hover = pointer_axis.map(|axis| (panel_id.clone(), axis));
        if let Some(hover) = hover {
            self.track_hovers.insert(panel_id.clone(), hover);
        } else {
            self.track_hovers.remove(panel_id);
        }
        cx.notify();
    }

    fn schedule_detail_refresh(&mut self, cx: &mut Context<Self>) {
        self.detail_refresh_token = self.detail_refresh_token.saturating_add(1);
        self.detail_refresh_pending = true;
        let token = self.detail_refresh_token;
        let timer = cx.background_executor().timer(Duration::from_millis(100));
        self.zoom_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.detail_refresh_token != token {
                    return;
                }
                this.detail_refresh_pending = false;
                this.request_detail(cx);
                cx.notify();
            });
        }));
    }

    fn cancel_detail_refresh(&mut self) {
        self.detail_refresh_token = self.detail_refresh_token.saturating_add(1);
        self.detail_refresh_pending = false;
        self.zoom_task = None;
    }

    fn workbench_document(&self) -> WorkbenchDocument {
        let source_path = |source_id: &DataSourceId| {
            self.sources.source(source_id).map_or_else(
                || PathBuf::from(source_id.as_str()),
                |source| source.root_path.clone(),
            )
        };
        let save_project = |project: &ProjectRef| SavedProjectRef {
            source_path: source_path(&project.source_id),
            project_id: project.project_id.clone(),
        };
        let save_run = |run: &RunRef| SavedRunRef {
            source_path: source_path(&run.source_id),
            project_id: run.project_id.clone(),
            run_id: run.run_id.clone(),
        };
        let active_index = self.views.active_index();
        let views = self
            .views
            .views()
            .iter()
            .enumerate()
            .map(|(index, view)| {
                let navigation = if index == active_index {
                    &self.navigation
                } else {
                    &view.navigation
                };
                SavedAnalysisView {
                    name: view.name.clone(),
                    runs: view.runs.iter().map(&save_run).collect(),
                    baseline: view.baseline.as_ref().map(&save_run),
                    pinned_runs: view.pinned_runs.iter().map(&save_run).collect(),
                    metrics: view
                        .panels
                        .iter()
                        .map(|panel| panel.metric_key.as_str().to_owned())
                        .collect(),
                    metric_heights: view
                        .panels
                        .iter()
                        .map(|panel| (panel.metric_key.as_str().to_owned(), panel.row_height))
                        .collect(),
                    selected_metric: view
                        .selected_panel_id
                        .as_ref()
                        .and_then(|panel_id| {
                            view.panels.iter().find(|panel| &panel.panel_id == panel_id)
                        })
                        .map(|panel| panel.metric_key.as_str().to_owned()),
                    axis: navigation.axis(),
                    track_density: view.track_density,
                    viewport: navigation.brush().map(|brush| brush.selected()),
                }
            })
            .collect();
        WorkbenchDocument {
            sources: self
                .sources
                .sources()
                .map(|source| source.root_path.clone())
                .collect(),
            pinned_projects: self
                .views
                .pinned_projects()
                .iter()
                .map(&save_project)
                .collect(),
            archived_projects: self
                .views
                .archived_projects()
                .iter()
                .map(&save_project)
                .collect(),
            removed_projects: self
                .views
                .removed_projects()
                .iter()
                .map(&save_project)
                .collect(),
            archived_runs: self.views.archived_runs().iter().map(&save_run).collect(),
            views,
            active_view: active_index,
            project_sidebar_visible: self.project_sidebar_visible,
            project_sidebar_width: f32::from(self.project_sidebar_width),
            metric_sidebar_compact: self.metric_sidebar_compact,
            bottom_inspector_visible: self.bottom_inspector_visible,
            bottom_inspector_height: f32::from(self.bottom_inspector_height),
        }
    }

    fn persist_workbench_if_changed(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.workbench_path.clone() else {
            return;
        };
        let document = self.workbench_document();
        let encoded = document.encode();
        if self.last_saved_workbench.as_deref() == Some(&encoded) {
            return;
        }
        self.last_saved_workbench = Some(encoded);
        let save = cx.background_spawn(async move { document.save(&path) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = save.await {
                let _ = this.update(cx, |this, cx| {
                    this.local_error = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn reconcile_canvas_widths(&mut self, scale_factor: f32, cx: &mut Context<Self>) {
        let Some((logical, physical)) = self.chart_adapter.borrow().overview_widths(scale_factor)
        else {
            return;
        };
        let changed = physical != self.overview_width;
        self.overview_logical_width = logical;
        self.overview_width = physical;
        if changed {
            self.request_overview(cx);
        }
    }
}

impl Render for ViewerApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.persist_workbench_if_changed(cx);
        self.theme = ViewerTheme::for_appearance(window.appearance());
        let theme = self.theme;
        self.reconcile_canvas_widths(window.scale_factor(), cx);
        let error = self.error().map(ToOwned::to_owned);
        let has_sources = self.sources.sources().next().is_some();

        div()
            .track_focus(&self.focus)
            .tab_group()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if event.dragging() {
                    this.move_metric_resize(event, cx);
                    this.move_inspector_resize(event, window, cx);
                    this.move_inspector_column_resize(event, cx);
                    this.move_sidebar_resize(event, window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.finish_metric_resize(cx);
                    this.finish_inspector_resize(cx);
                    this.finish_inspector_column_resize(cx);
                    this.finish_sidebar_resize(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.finish_metric_resize(cx);
                    this.finish_inspector_resize(cx);
                    this.finish_inspector_column_resize(cx);
                    this.finish_sidebar_resize(cx);
                }),
            )
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_reset))
            .on_action(cx.listener(Self::on_toggle_project_sidebar))
            .on_action(cx.listener(Self::on_toggle_metric_sidebar))
            .on_action(cx.listener(Self::on_toggle_bottom_inspector))
            .on_action(cx.listener(Self::on_show_metric_inspector))
            .on_action(cx.listener(Self::on_clear_locked_cursor))
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_step))
            .on_action(cx.listener(Self::on_elapsed))
            .flex()
            .size_full()
            .bg(theme.colors.window)
            .text_color(theme.colors.text)
            .child(
                div()
                    .flex()
                    .size_full()
                    .flex_1()
                    .overflow_hidden()
                    .children(
                        self.project_sidebar_visible
                            .then(|| self.render_project_sidebar(window, cx)),
                    )
                    .child(
                        div()
                            .id("analysis-workspace")
                            .debug_selector(|| "analysis-workspace".to_owned())
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(self.render_analysis_bar(cx))
                            .children(error.clone().map(|message| error_banner(message, theme)))
                            .child(if has_sources {
                                self.render_workspace(window, cx)
                            } else {
                                components::empty_state(theme)
                                    .child(
                                        components::status_badge(theme, StatusTone::Info).child(
                                            "Import a local PulseOn source to compare Runs.",
                                        ),
                                    )
                                    .child(
                                        components::toolbar_button(
                                            "open-project",
                                            theme,
                                            true,
                                            false,
                                        )
                                        .debug_selector(|| "open-project".to_owned())
                                        .key_context(SELECTABLE_CONTEXT)
                                        .tab_index(0)
                                        .cursor_pointer()
                                        .px_4()
                                        .py_2()
                                        .hover(|style| style.bg(theme.colors.accent_hover))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.open_picker(cx)),
                                        )
                                        .on_action(cx.listener(
                                            |this, _: &ActivateSelection, _, cx| {
                                                this.open_picker(cx)
                                            },
                                        ))
                                        .child("Import Source…"),
                                    )
                            }),
                    ),
            )
    }
}

fn axis_menu_item(
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

fn horizontal_resize_handle(
    id: SharedString,
    theme: ViewerTheme,
    active: bool,
    top_edge: bool,
) -> gpui::Stateful<gpui::Div> {
    let group = SharedString::from(format!("resize-boundary:{id}"));
    div()
        .id(id)
        .group(group.clone())
        .absolute()
        .left_0()
        .right_0()
        .h(px(5.))
        .when(top_edge, |handle| handle.top_0())
        .when(!top_edge, |handle| handle.bottom_0())
        .cursor(gpui::CursorStyle::ResizeUpDown)
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(1.))
                .when(top_edge, |line| line.top_0())
                .when(!top_edge, |line| line.bottom_0())
                .bg(if active {
                    theme.colors.focus
                } else {
                    theme.colors.transparent
                })
                .group_hover(group, |line| line.bg(theme.colors.focus)),
        )
}

fn vertical_resize_handle(
    id: SharedString,
    theme: ViewerTheme,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    let group = SharedString::from(format!("resize-boundary:{id}"));
    div()
        .id(id)
        .group(group.clone())
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(px(5.))
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(1.))
                .bg(if active {
                    theme.colors.focus
                } else {
                    theme.colors.transparent
                })
                .group_hover(group, |line| line.bg(theme.colors.focus)),
        )
}

fn inspector_header_cell(
    column: InspectorColumn,
    width: f32,
    direction: Option<InspectorSortDirection>,
    resizing: bool,
    theme: ViewerTheme,
    cx: &mut Context<ViewerApp>,
) -> gpui::Stateful<gpui::Div> {
    let indicator = direction.map(|direction| {
        let icon = components::icon(IconName::ChevronUp, theme).size(px(12.));
        match direction {
            InspectorSortDirection::Ascending => icon,
            InspectorSortDirection::Descending => {
                icon.with_transformation(Transformation::rotate(gpui::percentage(0.5)))
            }
        }
    });
    let resize_id = SharedString::from(format!("inspector-column-resize:{}", column.key()));
    div()
        .id(SharedString::from(format!(
            "inspector-header:{}",
            column.key()
        )))
        .debug_selector(move || format!("inspector-header:{}", column.key()))
        .w(px(width))
        .h(theme.spacing.control_height)
        .flex_none()
        .relative()
        .px_2()
        .when(column.has_left_border(), |cell| cell.border_l_1())
        .border_b_1()
        .border_color(theme.colors.border)
        .flex()
        .items_center()
        .gap_1()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.colors.text)
        .cursor_pointer()
        .hover(|style| style.bg(theme.colors.element_hover))
        .when(column.right_aligned(), |cell| cell.justify_end())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle_inspector_sort(column, cx);
        }))
        .child(column.label())
        .children(indicator)
        .children((column != InspectorColumn::Project).then(|| {
            vertical_resize_handle(resize_id, theme, resizing)
                .debug_selector(move || format!("inspector-column-resize:{}", column.key()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.begin_inspector_column_resize(column, event, cx);
                    }),
                )
                .on_click(|_, _, cx| cx.stop_propagation())
        }))
}

fn inspector_run_cell(
    row: &InspectorRow,
    width: f32,
    color: gpui::Rgba,
    theme: ViewerTheme,
) -> gpui::Div {
    div()
        .w(px(width))
        .h_full()
        .flex_none()
        .px_2()
        .overflow_hidden()
        .whitespace_nowrap()
        .flex()
        .items_center()
        .gap_1()
        .child(div().size(px(7.)).flex_none().rounded(px(3.5)).bg(color))
        .child(row.run_label.clone())
        .children(
            row.baseline
                .then(|| components::icon(IconName::Baseline, theme).size(px(12.))),
        )
        .children(
            (row.pinned && !row.baseline)
                .then(|| components::icon(IconName::Pin, theme).size(px(12.))),
        )
}

fn inspector_project_width(rows: &[InspectorRow]) -> f32 {
    rows.iter()
        .map(|row| row.project_label.chars().count() as f32 * 7. + 24.)
        .fold(InspectorColumn::Project.default_width(), f32::max)
}

impl InspectorColumn {
    const fn key(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::LastValue => "last-value",
            Self::Minimum => "min",
            Self::Maximum => "max",
            Self::Locked => "locked",
            Self::Hover => "hover",
            Self::Count => "count",
            Self::LastStep => "last-step",
            Self::Status => "status",
            Self::Evidence => "evidence",
            Self::Project => "project",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Run => "Run",
            Self::LastValue => "Last value",
            Self::Minimum => "Min",
            Self::Maximum => "Max",
            Self::Locked => "Locked",
            Self::Hover => "Hover",
            Self::Count => "Count",
            Self::LastStep => "Last step",
            Self::Status => "Status",
            Self::Evidence => "Evidence",
            Self::Project => "Project",
        }
    }

    const fn default_width(self) -> f32 {
        match self {
            Self::Run => 220.,
            Self::LastValue => 160.,
            Self::Minimum | Self::Maximum => 140.,
            Self::Locked | Self::Hover => 210.,
            Self::Count => 130.,
            Self::LastStep => 150.,
            Self::Status => 100.,
            Self::Evidence => 220.,
            Self::Project => 260.,
        }
    }

    const fn minimum_width(self) -> f32 {
        match self {
            Self::Run | Self::Project => 140.,
            Self::Evidence => 120.,
            Self::LastValue | Self::Minimum | Self::Maximum | Self::Locked | Self::Hover => 100.,
            Self::Count | Self::LastStep | Self::Status => 80.,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Run => 0,
            Self::LastValue => 1,
            Self::Minimum => 2,
            Self::Maximum => 3,
            Self::Locked => 4,
            Self::Hover => 5,
            Self::Count => 6,
            Self::LastStep => 7,
            Self::Status => 8,
            Self::Evidence => 9,
            Self::Project => 10,
        }
    }

    const fn right_aligned(self) -> bool {
        matches!(
            self,
            Self::LastValue
                | Self::Minimum
                | Self::Maximum
                | Self::Locked
                | Self::Hover
                | Self::Count
                | Self::LastStep
        )
    }

    const fn has_left_border(self) -> bool {
        !matches!(self, Self::Run | Self::LastValue)
    }
}

impl InspectorSortDirection {
    const fn apply(self, ordering: Ordering) -> Ordering {
        match self {
            Self::Ascending => ordering,
            Self::Descending => ordering.reverse(),
        }
    }
}

fn inspector_rows(
    panel: &MetricPanel,
    snapshot: &InspectorSnapshot,
    context: InspectorRowsContext<'_>,
) -> Vec<InspectorRow> {
    let mut rows = snapshot
        .runs
        .iter()
        .filter(|run| context.visible_runs.contains(&run.run_ref))
        .enumerate()
        .map(|(original_order, run)| {
            inspector_row(
                panel,
                run,
                context.baseline,
                context.pinned,
                context.locked_axis,
                context.hover_axis,
                original_order,
            )
        })
        .collect::<Vec<_>>();
    sort_inspector_rows(&mut rows, context.sort);
    rows
}

fn sort_inspector_rows(rows: &mut [InspectorRow], sort: Option<InspectorSort>) {
    rows.sort_by(|left, right| {
        inspector_row_group(left)
            .cmp(&inspector_row_group(right))
            .then_with(|| {
                sort.map_or_else(
                    || left.original_order.cmp(&right.original_order),
                    |sort| {
                        compare_inspector_rows(sort, left, right)
                            .then_with(|| left.original_order.cmp(&right.original_order))
                    },
                )
            })
    });
}

fn inspector_row(
    panel: &MetricPanel,
    run: &InspectorRunSnapshot,
    baseline: Option<&RunRef>,
    pinned: &[RunRef],
    locked_axis: Option<f64>,
    hover_axis: Option<f64>,
    original_order: usize,
) -> InspectorRow {
    let summary = run.summary.as_ref();
    InspectorRow {
        run_ref: run.run_ref.clone(),
        run_label: run.run.run_id.as_str().to_owned(),
        project_label: format!(
            "{} · {}",
            run.run_ref.project_id.as_str(),
            run.run_ref.source_id
        ),
        status: run.evidence.run_status,
        evidence: run.evidence.completeness,
        evidence_label: format!(
            "{:?}{}",
            run.evidence.completeness,
            reasons_label(&run.evidence.reasons)
        ),
        count: summary.map(|summary| summary.effective_count),
        last_step: summary.map(|summary| summary.last_step.value()),
        last_value: summary.map(|summary| summary.last_value_f64),
        minimum: summary.map(|summary| summary.min_value_f64),
        maximum: summary.map(|summary| summary.max_value_f64),
        locked: locked_axis.and_then(|axis| inspector_cursor_point(panel, &run.run_ref, axis)),
        hover: hover_axis.and_then(|axis| inspector_cursor_point(panel, &run.run_ref, axis)),
        baseline: baseline == Some(&run.run_ref),
        pinned: pinned.contains(&run.run_ref),
        original_order,
    }
}

fn inspector_cursor_point(panel: &MetricPanel, run_ref: &RunRef, axis: f64) -> Option<(i64, f64)> {
    panel
        .detail
        .as_ref()?
        .series
        .iter()
        .find(|series| &series.run_ref == run_ref)?
        .evidence
        .points
        .iter()
        .min_by(|left, right| {
            (left.axis_value as f64 - axis)
                .abs()
                .total_cmp(&(right.axis_value as f64 - axis).abs())
        })
        .map(|point| (point.axis_value, point.point.value_f64))
}

const fn inspector_row_group(row: &InspectorRow) -> u8 {
    if row.baseline {
        0
    } else if row.pinned {
        1
    } else {
        2
    }
}

fn compare_inspector_rows(
    sort: InspectorSort,
    left: &InspectorRow,
    right: &InspectorRow,
) -> Ordering {
    let direction = sort.direction;
    match sort.column {
        InspectorColumn::Run => direction.apply(left.run_label.cmp(&right.run_label)),
        InspectorColumn::LastValue => {
            compare_optional(left.last_value, right.last_value, direction, f64::total_cmp)
        }
        InspectorColumn::Minimum => {
            compare_optional(left.minimum, right.minimum, direction, f64::total_cmp)
        }
        InspectorColumn::Maximum => {
            compare_optional(left.maximum, right.maximum, direction, f64::total_cmp)
        }
        InspectorColumn::Locked => compare_optional(
            left.locked.map(|(_, value)| value),
            right.locked.map(|(_, value)| value),
            direction,
            f64::total_cmp,
        ),
        InspectorColumn::Hover => compare_optional(
            left.hover.map(|(_, value)| value),
            right.hover.map(|(_, value)| value),
            direction,
            f64::total_cmp,
        ),
        InspectorColumn::Count => compare_optional(left.count, right.count, direction, u64::cmp),
        InspectorColumn::LastStep => {
            compare_optional(left.last_step, right.last_step, direction, i64::cmp)
        }
        InspectorColumn::Status => direction
            .apply(inspector_status_order(left.status).cmp(&inspector_status_order(right.status))),
        InspectorColumn::Evidence => direction.apply(
            inspector_evidence_order(left.evidence).cmp(&inspector_evidence_order(right.evidence)),
        ),
        InspectorColumn::Project => direction.apply(left.project_label.cmp(&right.project_label)),
    }
}

fn compare_optional<T: Copy>(
    left: Option<T>,
    right: Option<T>,
    direction: InspectorSortDirection,
    compare: impl FnOnce(&T, &T) -> Ordering,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => direction.apply(compare(&left, &right)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

const fn inspector_status_order(status: RunStatus) -> u8 {
    match status {
        RunStatus::Running => 0,
        RunStatus::Finished => 1,
        RunStatus::Failed => 2,
    }
}

const fn inspector_evidence_order(evidence: EvidenceCompleteness) -> u8 {
    match evidence {
        EvidenceCompleteness::Complete => 0,
        EvidenceCompleteness::Partial => 1,
        EvidenceCompleteness::Unavailable => 2,
        EvidenceCompleteness::Invalid => 3,
    }
}

fn inspector_cell_text(
    column: InspectorColumn,
    row: &InspectorRow,
    baseline: Option<&InspectorRow>,
) -> String {
    match column {
        InspectorColumn::Run => row.run_label.clone(),
        InspectorColumn::LastValue => inspector_float(
            row.last_value,
            baseline.and_then(|baseline| baseline.last_value),
            row.baseline,
        ),
        InspectorColumn::Minimum => inspector_float(
            row.minimum,
            baseline.and_then(|baseline| baseline.minimum),
            row.baseline,
        ),
        InspectorColumn::Maximum => inspector_float(
            row.maximum,
            baseline.and_then(|baseline| baseline.maximum),
            row.baseline,
        ),
        InspectorColumn::Locked => inspector_cursor_cell(
            row.locked,
            baseline.and_then(|baseline| baseline.locked),
            row.baseline,
        ),
        InspectorColumn::Hover => inspector_cursor_cell(
            row.hover,
            baseline.and_then(|baseline| baseline.hover),
            row.baseline,
        ),
        InspectorColumn::Count => inspector_integer(row.count.map(i128::from), None, true),
        InspectorColumn::LastStep => inspector_integer(row.last_step.map(i128::from), None, true),
        InspectorColumn::Status => run_status(row.status).to_owned(),
        InspectorColumn::Evidence => row.evidence_label.clone(),
        InspectorColumn::Project => row.project_label.clone(),
    }
}

fn inspector_float(value: Option<f64>, baseline: Option<f64>, is_baseline: bool) -> String {
    value.map_or_else(String::new, |value| {
        let value_label = format!("{value:.6}");
        if is_baseline {
            value_label
        } else {
            baseline.map_or(value_label.clone(), |baseline| {
                format!(
                    "{value_label} ({})",
                    format_signed_delta(value - baseline, 6)
                )
            })
        }
    })
}

fn inspector_integer(value: Option<i128>, baseline: Option<i128>, is_baseline: bool) -> String {
    value.map_or_else(String::new, |value| {
        let value_label = format_grouped_integer(value);
        if is_baseline {
            value_label
        } else {
            baseline.map_or(value_label.clone(), |baseline| {
                format!(
                    "{value_label} ({})",
                    format_signed_integer(value - baseline)
                )
            })
        }
    })
}

fn inspector_cursor_cell(
    point: Option<(i64, f64)>,
    baseline: Option<(i64, f64)>,
    is_baseline: bool,
) -> String {
    point.map_or_else(String::new, |(_, value)| {
        inspector_float(
            Some(value),
            baseline.map(|(_, baseline)| baseline),
            is_baseline,
        )
    })
}

fn format_signed_integer(value: i128) -> String {
    if value.is_negative() {
        format!("−{}", format_grouped_integer(value.saturating_abs()))
    } else {
        format!("+{}", format_grouped_integer(value))
    }
}

fn format_grouped_integer(value: i128) -> String {
    let negative = value.is_negative();
    let digits = value.saturating_abs().to_string();
    let mut grouped =
        String::with_capacity(digits.len() + digits.len() / 3 + usize::from(negative));
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    if negative {
        grouped.insert(0, '−');
    }
    grouped
}

fn error_banner(message: String, theme: ViewerTheme) -> gpui::Div {
    components::status_badge(theme, StatusTone::Error)
        .mx_5()
        .mt_3()
        .px_4()
        .py_3()
        .child(message)
}

fn picked_directory(paths: Option<Vec<PathBuf>>) -> Option<PathBuf> {
    paths.and_then(|paths| paths.into_iter().next())
}

fn project_tree_label(name: &str, source_id: &DataSourceId, duplicate: bool) -> String {
    if duplicate {
        format!("{name} — {source_id}")
    } else {
        name.to_owned()
    }
}

const fn run_status(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Finished => "finished",
        RunStatus::Failed => "failed",
    }
}

fn format_tick(value: f64) -> String {
    if value.abs() >= 1_000_000. || (value != 0. && value.abs() < 0.001) {
        format!("{value:.2e}")
    } else if value.fract() == 0. {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}

fn format_axis_tick(axis: CurveAxis, value: f64) -> String {
    match axis {
        CurveAxis::Step => format_tick(value),
        CurveAxis::AbsoluteTime => format_utc_clock(value),
    }
}

fn format_ruler_coordinate(axis: CurveAxis, value: f64) -> String {
    match axis {
        CurveAxis::Step if value.is_finite() => format!("{value:.0}"),
        CurveAxis::Step => "—".to_owned(),
        CurveAxis::AbsoluteTime => format_utc_clock(value),
    }
}

fn format_utc_clock(value: f64) -> String {
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

fn hover_value_label(axis: CurveAxis, hover: &HoverPoint, delta: Option<f64>) -> String {
    let value = delta.map_or_else(
        || format!("{:.2}", hover.value),
        |delta| format!("{:.2} ({})", hover.value, format_signed_delta(delta, 2)),
    );
    let coordinate = match axis {
        CurveAxis::Step => hover.axis_value.to_string(),
        CurveAxis::AbsoluteTime => format_utc_clock(hover.axis_value as f64),
    };
    format!("{coordinate}: {value} {}", hover.run_ref.run_id.as_str())
}

fn track_tooltip_width<'a>(labels: impl IntoIterator<Item = &'a str>) -> f32 {
    let characters = labels
        .into_iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or_default();
    (characters as f32 * 6.5 + 28.).clamp(104., 248.)
}

fn format_signed_delta(delta: f64, precision: usize) -> String {
    let sign = if delta.is_sign_negative() { '−' } else { '+' };
    format!("{sign}{:.precision$}", delta.abs())
}

fn baseline_delta(panel: &MetricPanel, baseline: &RunRef, hover: &HoverPoint) -> Option<f64> {
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
        .evidence
        .points
        .iter()
        .min_by_key(|point| point.axis_value.abs_diff(hover.axis_value))?;
    Some(hover.value - point.point.value_f64)
}

fn reasons_label(reasons: &[EvidenceReason]) -> String {
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

#[cfg(test)]
mod tests {
    use pulseon_model::comparison::EvidenceCompleteness;
    use pulseon_model::run::RunId;

    use super::*;

    #[test]
    fn picker_cancellation_has_no_source_effect() {
        assert_eq!(picked_directory(None), None);
        assert_eq!(picked_directory(Some(Vec::new())), None);
    }

    #[test]
    fn picker_uses_the_single_selected_directory() {
        assert_eq!(
            picked_directory(Some(vec![PathBuf::from("project")])),
            Some(PathBuf::from("project"))
        );
    }

    #[test]
    fn text_cursor_moves_between_utf8_character_boundaries() {
        let text = "a界b";

        assert_eq!(previous_text_cursor(text, text.len()), "a界".len());
        assert_eq!(previous_text_cursor(text, "a界".len()), "a".len());
        assert_eq!(next_text_cursor(text, "a".len()), "a界".len());
        assert_eq!(next_text_cursor(text, text.len()), text.len());
    }

    #[test]
    fn duplicate_project_names_are_qualified_by_source_identity() {
        assert_eq!(
            project_tree_label("viewer", &DataSourceId::from_string("source-b"), true),
            "viewer — source-b"
        );
        assert_eq!(
            project_tree_label("viewer", &DataSourceId::from_string("source-b"), false),
            "viewer"
        );
    }

    #[test]
    fn hover_values_format_visible_and_contextual_evidence() {
        let hover = HoverPoint {
            run_ref: RunRef::new(
                DataSourceId::from_string("source"),
                ProjectId::from_string("project"),
                RunId::from_string("run"),
            ),
            axis_value: 2_904,
            value: 0.506,
            canvas_position: point(px(10.), px(20.)),
            align_left: false,
        };

        assert_eq!(
            hover_value_label(CurveAxis::Step, &hover, Some(0.55)),
            "2904: 0.51 (+0.55) run"
        );
        assert_eq!(
            hover_value_label(CurveAxis::Step, &hover, Some(-0.55)),
            "2904: 0.51 (−0.55) run"
        );
        assert_eq!(
            hover_value_label(CurveAxis::Step, &hover, None),
            "2904: 0.51 run"
        );
        assert_eq!(format_ruler_coordinate(CurveAxis::Step, 26_432.4), "26432");
        assert_eq!(track_tooltip_width(["0.28"]), 104.);
        assert_eq!(track_tooltip_width(["19433: 0.44 run-8"]), 138.5);
        let long_label = "x".repeat(40);
        assert_eq!(track_tooltip_width([long_label.as_str()]), 248.);
    }

    #[test]
    fn brush_handle_drag_stops_before_crossing_the_opposite_handle() {
        let home =
            pulseon_chart_core::AxisRange::new(0., 100.).expect("test brush range should be valid");
        let mut brush = BrushState::new(home).expect("test brush should initialize");
        brush.resize_start(20.).expect("start should resize");
        brush.resize_end(80.).expect("end should resize");

        let selected = brush.selected();
        update_brush_drag(&mut brush, &mut DragGesture::BrushStart, 90.);
        assert_eq!(brush.selected(), selected);
        update_brush_drag(&mut brush, &mut DragGesture::BrushEnd, 10.);
        assert_eq!(brush.selected(), selected);
    }

    #[test]
    fn inspector_sort_preserves_roles_and_cycles_numeric_order() {
        assert!(!InspectorColumn::Run.has_left_border());
        assert!(!InspectorColumn::LastValue.has_left_border());
        assert!(InspectorColumn::Minimum.has_left_border());

        let make_row =
            |run: &str, value: Option<f64>, baseline: bool, pinned: bool, order| InspectorRow {
                run_ref: RunRef::new(
                    DataSourceId::from_string("source"),
                    ProjectId::from_string("project"),
                    RunId::from_string(run),
                ),
                run_label: run.to_owned(),
                project_label: "project · source".to_owned(),
                status: RunStatus::Finished,
                evidence: EvidenceCompleteness::Complete,
                evidence_label: "Complete".to_owned(),
                count: Some(1),
                last_step: Some(1),
                last_value: value,
                minimum: value,
                maximum: value,
                locked: value.map(|value| (1, value)),
                hover: value.map(|value| (1, value)),
                baseline,
                pinned,
                original_order: order,
            };
        let mut long_project = make_row("path", Some(1.), false, false, 0);
        long_project.project_label =
            "viewer · /tmp/a/very/long/project/path/that/needs/content/sizing".to_owned();
        assert!(
            inspector_project_width(&[long_project]) > InspectorColumn::Project.default_width()
        );
        let mut rows = vec![
            make_row("run-3", Some(3.), false, false, 0),
            make_row("baseline", Some(2.), true, false, 1),
            make_row("pinned", Some(4.), false, true, 2),
            make_row("run-1", Some(1.), false, false, 3),
            make_row("missing", None, false, false, 4),
        ];

        sort_inspector_rows(
            &mut rows,
            Some(InspectorSort {
                column: InspectorColumn::Minimum,
                direction: InspectorSortDirection::Ascending,
            }),
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.run_label.as_str())
                .collect::<Vec<_>>(),
            ["baseline", "pinned", "run-1", "run-3", "missing"]
        );
        sort_inspector_rows(
            &mut rows,
            Some(InspectorSort {
                column: InspectorColumn::Minimum,
                direction: InspectorSortDirection::Descending,
            }),
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.run_label.as_str())
                .collect::<Vec<_>>(),
            ["baseline", "pinned", "run-3", "run-1", "missing"]
        );
        assert_eq!(inspector_float(Some(2.), Some(2.), true), "2.000000");
        assert_eq!(
            inspector_float(Some(3.), Some(2.), false),
            "3.000000 (+1.000000)"
        );
        assert_eq!(inspector_float(None, Some(2.), false), "");
        assert_eq!(
            inspector_integer(Some(20_512), Some(20_741), false),
            "20,512 (−229)"
        );
        assert_eq!(
            inspector_cursor_cell(Some((20_799, 0.62)), Some((20_800, 0.58)), false,),
            "0.620000 (+0.040000)"
        );
        assert_eq!(inspector_cursor_cell(None, Some((20_800, 0.58)), false), "");
        let baseline = rows.iter().find(|row| row.baseline).expect("baseline row");
        let candidate = rows
            .iter()
            .find(|row| !row.baseline)
            .expect("candidate row");
        assert_eq!(
            inspector_cell_text(InspectorColumn::Count, candidate, Some(baseline)),
            "1"
        );
        assert_eq!(
            inspector_cell_text(InspectorColumn::LastStep, candidate, Some(baseline)),
            "1"
        );
    }

    #[cfg(feature = "test-support")]
    mod gpui_tests {
        use std::sync::Arc;

        use gpui::{
            Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext, WindowHandle,
            point,
        };
        use pulseon_core::engine::client::NativeClient;
        use pulseon_viewer::workbench::TrackDensity;

        use super::*;

        fn fixture(metric_count: usize) -> (tempfile::TempDir, ProjectId, RunId) {
            fixture_with_runs(metric_count, 1)
        }

        fn fixture_with_metric(metric_key: &str) -> (tempfile::TempDir, ProjectId, RunId) {
            let root = tempfile::tempdir().expect("test directory should be created");
            let client = NativeClient::open(root.path()).expect("test client should open");
            let project = client
                .create_project("viewer", Some(ProjectId::from_string("project")))
                .expect("test project should be created");
            let run = client
                .create_run(&project.project_id, "run", Some(RunId::from_string("run")))
                .expect("test Run should be created");
            client
                .run_handle(run.clone())
                .log_metric_at_step(metric_key, 0, 1.)
                .expect("test metric should be logged");
            client.finish_run(&run.run_id).expect("Run should finish");
            client.shutdown(None).expect("test client should shut down");
            (root, project.project_id, run.run_id)
        }

        fn fixture_with_runs(
            metric_count: usize,
            run_count: usize,
        ) -> (tempfile::TempDir, ProjectId, RunId) {
            fixture_with_run_coverage(metric_count, run_count, false)
        }

        fn fixture_with_complete_runs(
            metric_count: usize,
            run_count: usize,
        ) -> (tempfile::TempDir, ProjectId, RunId) {
            fixture_with_run_coverage(metric_count, run_count, true)
        }

        fn fixture_with_run_coverage(
            metric_count: usize,
            run_count: usize,
            populate_all_runs: bool,
        ) -> (tempfile::TempDir, ProjectId, RunId) {
            let root = tempfile::tempdir().expect("test directory should be created");
            let client = NativeClient::open(root.path()).expect("test client should open");
            let project = client
                .create_project("viewer", Some(ProjectId::from_string("project")))
                .expect("test project should be created");
            let mut first_run_id = None;
            for run_index in 0..run_count {
                let run_name = format!("baseline {run_index}");
                let run = client
                    .create_run(
                        &project.project_id,
                        &run_name,
                        Some(RunId::from_string(format!(
                            "run-{run_index}-with-a-very-long-identifier-that-requires-horizontal-scrolling"
                        ))),
                    )
                    .expect("test Run should be created");
                if run_index == 0 || populate_all_runs {
                    let handle = client.run_handle(run.clone());
                    for index in 0..metric_count {
                        let metric_key = format!("metric-{index}");
                        handle
                            .log_metric_at_step(&metric_key, 0, (run_index + index) as f64)
                            .expect("test metric should be logged");
                        if populate_all_runs {
                            handle
                                .log_metric_at_step(
                                    &metric_key,
                                    100,
                                    (run_index + index + 1) as f64,
                                )
                                .expect("test metric extent should be logged");
                        }
                    }
                }
                client
                    .finish_run(&run.run_id)
                    .expect("test Run should finish");
                first_run_id.get_or_insert(run.run_id);
            }
            client.shutdown(None).expect("test client should shut down");
            (
                root,
                project.project_id,
                first_run_id.expect("fixture should contain at least one Run"),
            )
        }

        fn fixture_with_extent(end_step: i64) -> (tempfile::TempDir, ProjectId, RunId) {
            let root = tempfile::tempdir().expect("test directory should be created");
            let client = NativeClient::open(root.path()).expect("test client should open");
            let project = client
                .create_project("viewer", Some(ProjectId::from_string("project")))
                .expect("test project should be created");
            let run = client
                .create_run(
                    &project.project_id,
                    "baseline",
                    Some(RunId::from_string("run")),
                )
                .expect("test Run should be created");
            let handle = client.run_handle(run.clone());
            handle
                .log_metric_at_step("loss", 0, 1.)
                .expect("test metric should be logged");
            handle
                .log_metric_at_step("loss", end_step, 0.5)
                .expect("test metric should be logged");
            client
                .finish_run(&run.run_id)
                .expect("test Run should finish");
            client.shutdown(None).expect("test client should shut down");
            (root, project.project_id, run.run_id)
        }

        fn saved_workbench(
            source_path: PathBuf,
            project_id: ProjectId,
            runs: Vec<RunId>,
            metric: &str,
        ) -> WorkbenchDocument {
            WorkbenchDocument {
                sources: vec![source_path.clone()],
                pinned_projects: Vec::new(),
                archived_projects: Vec::new(),
                removed_projects: Vec::new(),
                archived_runs: Vec::new(),
                views: vec![SavedAnalysisView {
                    name: "Restored".to_owned(),
                    runs: runs
                        .into_iter()
                        .map(|run_id| SavedRunRef {
                            source_path: source_path.clone(),
                            project_id: project_id.clone(),
                            run_id,
                        })
                        .collect(),
                    baseline: None,
                    pinned_runs: Vec::new(),
                    metrics: vec![metric.to_owned()],
                    metric_heights: Vec::new(),
                    selected_metric: Some(metric.to_owned()),
                    axis: AlignmentAxis::Step,
                    track_density: TrackDensity::Compact,
                    viewport: Some(
                        pulseon_chart_core::AxisRange::new(0., 10.)
                            .expect("test viewport should be valid"),
                    ),
                }],
                active_view: 0,
                project_sidebar_visible: false,
                project_sidebar_width: 280.,
                metric_sidebar_compact: true,
                bottom_inspector_visible: true,
                bottom_inspector_height: 260.,
            }
        }

        fn open_viewer(
            cx: &mut TestAppContext,
            project_path: Option<PathBuf>,
        ) -> (WindowHandle<ViewerApp>, VisualTestContext) {
            let window = cx.update(|cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(800.), px(600.)),
                        ))),
                        ..WindowOptions::default()
                    },
                    move |window, cx| cx.new(|cx| ViewerApp::new(project_path, window, cx)),
                )
                .expect("test viewer window should open")
            });
            let visual = VisualTestContext::from_window(window.into(), cx);
            (window, visual)
        }

        #[track_caller]
        fn wait_for_viewer(
            window: WindowHandle<ViewerApp>,
            cx: &VisualTestContext,
            condition: impl Fn(&ViewerApp) -> bool,
        ) {
            for _ in 0..1_000 {
                cx.run_until_parked();
                let ready = window
                    .read_with(cx, |viewer, _| condition(viewer))
                    .expect("viewer should remain open");
                if ready {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("viewer state did not arrive before the test deadline");
        }

        fn first_panel_detail_is_settled(viewer: &ViewerApp) -> bool {
            let schedule = viewer.track_viewport.borrow();
            let Some(viewport) = viewer.navigation.selected_viewport() else {
                return false;
            };
            !schedule.overscan.is_empty()
                && schedule.physical_width > 0
                && viewer.views.active().panels.first().is_some_and(|panel| {
                    panel.detail.is_some()
                        && !panel.is_pending(ReadKind::Detail)
                        && panel.requested_detail_viewport == Some(viewport)
                        && panel.physical_width == schedule.physical_width
                })
        }

        fn source_catalog_loaded(viewer: &ViewerApp) -> bool {
            viewer
                .sources
                .sources()
                .any(|source| !source.catalog.projects.is_empty())
        }

        fn first_source_id(viewer: &ViewerApp) -> DataSourceId {
            viewer
                .sources
                .sources()
                .next()
                .expect("fixture source should be imported")
                .source_id
                .clone()
        }

        fn select_fixture_run(
            window: WindowHandle<ViewerApp>,
            cx: &mut VisualTestContext,
            project_id: ProjectId,
            run_id: RunId,
            metric_count: usize,
        ) {
            wait_for_viewer(window, cx, source_catalog_loaded);
            window
                .update(cx, |viewer, _, cx| {
                    let source_id = first_source_id(viewer);
                    viewer.toggle_run(RunRef::new(source_id, project_id, run_id), cx)
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, cx, |viewer| {
                viewer.available_metric_keys().len() == metric_count
            });
        }

        #[gpui::test]
        fn view_actions_dispatch_through_the_focused_root(cx: &mut TestAppContext) {
            let (window, mut cx) = open_viewer(cx, None);

            cx.dispatch_action(UseElapsed);

            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.navigation.axis())
                    .expect("viewer should remain open"),
                AlignmentAxis::ElapsedTime
            );
        }

        #[gpui::test]
        fn viewer_owned_workbench_state_is_saved_without_query_snapshots(cx: &mut TestAppContext) {
            let root = tempfile::tempdir().expect("test directory should be created");
            let path = root.path().join("workbench.state");
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, None);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.workbench_path = Some(path.clone());
                    viewer.last_saved_workbench = None;
                    viewer.project_sidebar_visible = false;
                    viewer.project_sidebar_width = px(288.);
                    viewer.metric_sidebar_compact = true;
                    viewer.bottom_inspector_height = px(260.);
                    let source_id = DataSourceId::from_path(root.path());
                    let project_id = ProjectId::from_string("project");
                    viewer
                        .views
                        .pin_project(ProjectRef::new(source_id.clone(), project_id.clone()));
                    viewer.views.archive_project(ProjectRef::new(
                        source_id.clone(),
                        ProjectId::from_string("archive"),
                    ));
                    viewer.views.remove_project(ProjectRef::new(
                        source_id.clone(),
                        ProjectId::from_string("removed"),
                    ));
                    viewer
                        .views
                        .set_active_baseline(Some(RunRef::new(
                            source_id.clone(),
                            project_id.clone(),
                            RunId::from_string("baseline"),
                        )))
                        .expect("baseline should fit the visible Run limit");
                    viewer
                        .views
                        .toggle_active_pinned_run(RunRef::new(
                            source_id.clone(),
                            project_id.clone(),
                            RunId::from_string("pinned"),
                        ))
                        .expect("pinned Run should fit the visible Run limit");
                    viewer.views.archive_run(RunRef::new(
                        source_id,
                        project_id,
                        RunId::from_string("archived"),
                    ));
                    cx.notify();
                })
                .expect("viewer should remain open");
            let mut loaded = None;
            for _ in 0..1_000 {
                cx.run_until_parked();
                loaded =
                    WorkbenchDocument::load(&path).expect("saved document should remain readable");
                if loaded.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let loaded = loaded.expect("workbench document should be saved");

            assert!(!loaded.project_sidebar_visible);
            assert_eq!(loaded.project_sidebar_width, 288.);
            assert!(loaded.metric_sidebar_compact);
            assert_eq!(loaded.bottom_inspector_height, 260.);
            assert_eq!(loaded.views.len(), 1);
            assert!(loaded.views[0].metrics.is_empty());
            assert_eq!(loaded.pinned_projects.len(), 1);
            assert_eq!(loaded.archived_projects.len(), 1);
            assert_eq!(loaded.removed_projects.len(), 1);
            assert_eq!(loaded.archived_runs.len(), 1);
            assert!(loaded.views[0].baseline.is_some());
            assert_eq!(loaded.views[0].pinned_runs.len(), 1);
        }

        #[gpui::test]
        fn restored_state_reconciles_removed_runs_and_unknown_metrics(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_complete_runs(2, 1);
            let document = saved_workbench(
                root.path().to_path_buf(),
                project_id,
                vec![run_id.clone(), RunId::from_string("removed")],
                "unknown-metric",
            );
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, None);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.restore_workbench(document, cx);
                    cx.notify();
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().runs.len() == 1
                    && viewer.views.active().runs[0].run_id == run_id
                    && viewer.views.active().panels[0]
                        .overview
                        .as_ref()
                        .is_some_and(|snapshot| {
                            snapshot.series.iter().all(|series| {
                                series.evidence.completeness == EvidenceCompleteness::Unavailable
                            })
                        })
            });

            assert!(root.path().exists());
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer
                        .local_error
                        .as_deref()
                        .is_some_and(|error| error.contains("no longer available")))
                    .expect("viewer should remain open")
            );
        }

        #[gpui::test]
        fn restored_missing_sources_remain_visible_without_creating_native_state(
            cx: &mut TestAppContext,
        ) {
            let root = tempfile::tempdir().expect("test directory should be created");
            let missing = root.path().join("moved-source");
            let document = saved_workbench(
                missing.clone(),
                ProjectId::from_string("project"),
                vec![RunId::from_string("run")],
                "loss",
            );
            let (window, mut cx) = open_viewer(cx, None);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.restore_workbench(document, cx);
                    cx.notify();
                })
                .expect("viewer should remain open");

            let status = window
                .read_with(&cx, |viewer, _| {
                    viewer
                        .sources
                        .sources()
                        .next()
                        .map(|source| (source.root_path.clone(), source.status.clone()))
                })
                .expect("viewer should remain open")
                .expect("missing source should remain listed");
            assert_eq!(status.0, missing);
            assert!(matches!(status.1, SourceStatus::Failed(_)));
            assert!(!missing.exists());
        }

        #[gpui::test]
        fn restored_organization_records_import_their_referenced_sources(cx: &mut TestAppContext) {
            let root = tempfile::tempdir().expect("test directory should be created");
            let missing = root.path().join("archived-source");
            let mut document = saved_workbench(
                missing.clone(),
                ProjectId::from_string("project"),
                Vec::new(),
                "loss",
            );
            document.sources.clear();
            document.views.clear();
            document.archived_projects.push(SavedProjectRef {
                source_path: missing.clone(),
                project_id: ProjectId::from_string("project"),
            });
            let (window, mut cx) = open_viewer(cx, None);

            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.restore_workbench(document, cx);
                    cx.notify();
                })
                .expect("viewer should remain open");

            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer
                        .sources
                        .sources()
                        .any(|source| source.root_path == missing))
                    .expect("viewer should remain open")
            );
            assert!(!missing.exists());
        }

        #[gpui::test]
        fn project_sidebar_toggle_expands_the_analysis_workspace(cx: &mut TestAppContext) {
            let (window, mut cx) = open_viewer(cx, None);
            let analysis_before = cx
                .debug_bounds("analysis-tab")
                .expect("Analysis workspace should render");
            assert!(cx.debug_bounds("project-run-tree").is_some());

            cx.dispatch_action(ToggleProjectSidebar);

            assert!(
                !window
                    .read_with(&cx, |viewer, _| viewer.project_sidebar_visible)
                    .expect("viewer should remain open")
            );
            assert!(cx.debug_bounds("project-run-tree").is_none());
            let left_controls = cx
                .debug_bounds("analysis-left-controls")
                .expect("sidebar reveal group should render");
            let show_sidebar = cx
                .debug_bounds("show-project-sidebar")
                .expect("sidebar reveal control should render");
            let tabs = cx
                .debug_bounds("analysis-view-tabs")
                .expect("View tabs should remain rendered");
            assert_eq!(show_sidebar.origin.x, left_controls.origin.x + px(4.));
            assert_eq!(tabs.origin.x, left_controls.right());
            let analysis_after = cx
                .debug_bounds("analysis-tab")
                .expect("Analysis workspace should remain rendered");
            assert!(analysis_after.origin.x < analysis_before.origin.x);
        }

        #[gpui::test]
        fn project_sidebar_width_resizes_from_its_boundary(cx: &mut TestAppContext) {
            let (window, mut cx) = open_viewer(cx, None);
            let sidebar_width = cx
                .debug_bounds("project-sidebar")
                .expect("Project sidebar should render")
                .size
                .width;
            let sidebar_resize = cx
                .debug_bounds("project-sidebar-resize")
                .expect("Project sidebar resize boundary should render");
            let resize_target = point(
                sidebar_resize.center().x + px(48.),
                sidebar_resize.center().y,
            );
            cx.simulate_mouse_down(
                sidebar_resize.center(),
                MouseButton::Left,
                Modifiers::default(),
            );
            cx.simulate_mouse_move(resize_target, Some(MouseButton::Left), Modifiers::default());
            cx.simulate_mouse_up(resize_target, MouseButton::Left, Modifiers::default());

            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.project_sidebar_width)
                    .expect("viewer should remain open"),
                sidebar_width + px(48.)
            );
        }

        #[gpui::test]
        fn project_filter_uses_placeholder_and_blinking_caret_states(cx: &mut TestAppContext) {
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, None);
            let placeholder = cx
                .debug_bounds("project-run-filter-placeholder")
                .expect("Project filter placeholder should render");

            let filter = cx
                .debug_bounds("project-run-filter")
                .expect("Project filter should render");
            assert!(placeholder.right() <= filter.right());
            cx.simulate_mouse_move(filter.center(), None, Modifiers::default());
            cx.simulate_click(filter.center(), Modifiers::default());
            assert!(
                window
                    .update(&mut cx, |viewer, window, _| viewer
                        .filter_focus
                        .is_focused(window))
                    .expect("viewer should remain open")
            );
            assert!(cx.debug_bounds("project-run-filter-caret").is_some());

            cx.executor().advance_clock(Duration::from_millis(500));
            cx.run_until_parked();
            assert!(
                !window
                    .read_with(&cx, |viewer, _| viewer.filter_cursor_visible)
                    .expect("viewer should remain open")
            );
            cx.simulate_keystrokes("viewer left left right x");
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.run_filter.clone())
                    .expect("viewer should remain open"),
                "viewexr"
            );
            let prefix = cx
                .debug_bounds("project-run-filter-value")
                .expect("filter value before the cursor should render");
            let caret = cx
                .debug_bounds("project-run-filter-caret")
                .expect("typing should reveal the filter caret");
            assert_eq!(caret.origin.x, prefix.right() + px(1.));
            assert!(cx.debug_bounds("project-run-filter-suffix").is_some());
        }

        #[gpui::test]
        fn run_markers_align_with_project_icons_and_run_labels(cx: &mut TestAppContext) {
            let (root, project_id, _) = fixture_with_runs(0, 9);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| {
                viewer
                    .sources
                    .sources()
                    .next()
                    .is_some_and(|source| source.catalog.runs.len() == 9)
            });
            window
                .update(&mut cx, |viewer, _, cx| {
                    let source = viewer.sources.sources().next().expect("source").clone();
                    let run_ref = |index: usize| {
                        RunRef::new(
                            source.source_id.clone(),
                            project_id.clone(),
                            source.catalog.runs[index].run_id.clone(),
                        )
                    };
                    viewer.set_run_baseline(run_ref(0), cx);
                    viewer.toggle_pinned_run(run_ref(1), cx);
                    for index in 2..8 {
                        viewer.archive_run(run_ref(index), cx);
                    }
                })
                .expect("viewer should remain open");

            let project_label = cx
                .debug_bounds("project-tree-label-0-0")
                .expect("Project label should render");
            let project_folder = cx
                .debug_bounds("project-folder-0-0")
                .expect("Project folder should render");
            let baseline_label = cx
                .debug_bounds("baseline-run-name-0")
                .expect("Baseline Run label should render");
            let pinned_label = cx
                .debug_bounds("pinned-run-name-0")
                .expect("Pinned Run label should render");
            let baseline_color = cx
                .debug_bounds("run-color-baseline-0")
                .expect("Baseline color marker should render");
            let pinned_color = cx
                .debug_bounds("run-color-pinned-0")
                .expect("Pinned color marker should render");
            let archived_label = cx
                .debug_bounds("archived-run-name-0")
                .expect("Archived Run label should render");
            let archived_color = cx
                .debug_bounds("run-color-archived-0")
                .expect("Archived color marker should render");
            let archived_more = cx
                .debug_bounds("show-more-archived")
                .expect("Archived pagination should render");
            assert_eq!(baseline_label.origin.x, pinned_label.origin.x);
            assert_eq!(baseline_label.origin.x, archived_label.origin.x);
            assert_eq!(archived_more.origin.x, archived_label.origin.x);
            for color in [baseline_color, pinned_color, archived_color] {
                assert_eq!(color.origin.x, project_folder.origin.x);
            }
            let sidebar = cx
                .debug_bounds("project-sidebar")
                .expect("Project sidebar should render");
            let archived_tree = cx
                .debug_bounds("archived-run-tree")
                .expect("Archived section should render");
            assert_eq!(archived_tree.bottom(), sidebar.bottom());
            let project_row = cx
                .debug_bounds("project-tree-row-0-0")
                .expect("Project row should render");
            cx.simulate_click(project_row.center(), Modifiers::default());
            let nested_run = cx
                .debug_bounds("project-run-name-0")
                .expect("nested Run label should render");
            let nested_eye = cx
                .debug_bounds("run-eye-0")
                .expect("nested Run eye should render");
            let nested_color = cx
                .debug_bounds("run-color-project-0")
                .expect("nested Run color marker should render");
            assert_eq!(nested_eye.origin.x, project_folder.origin.x);
            assert!(nested_color.origin.x > baseline_color.origin.x);
            assert!(nested_run.origin.x > baseline_label.origin.x);
            assert!(nested_run.origin.x > project_label.origin.x);
            assert!(nested_color.center().y >= nested_run.center().y);
            assert!(f32::from(nested_color.center().y - nested_run.center().y) <= 2.);

            window
                .update(&mut cx, |viewer, _, cx| {
                    let run_ref = {
                        let source = viewer.sources.sources().next().expect("source");
                        RunRef::new(
                            source.source_id.clone(),
                            project_id.clone(),
                            source.catalog.runs[8].run_id.clone(),
                        )
                    };
                    viewer.archive_run(run_ref, cx);
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            let no_runs = cx
                .debug_bounds("project-no-runs")
                .expect("Empty Project should render its placeholder");
            assert_eq!(no_runs.origin.x, project_label.origin.x);
        }

        #[gpui::test]
        fn application_shell_preserves_pinned_geometry_at_representative_sizes(
            cx: &mut TestAppContext,
        ) {
            let (root, _, _) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);

            for window_size in [(600., 520.), (800., 600.), (1_440., 900.)] {
                cx.simulate_resize(size(px(window_size.0), px(window_size.1)));
                cx.run_until_parked();

                let sidebar = cx
                    .debug_bounds("project-sidebar")
                    .expect("Project sidebar should render");
                let analysis = cx
                    .debug_bounds("analysis-workspace")
                    .expect("Analysis workspace should render");
                let tab_bar = cx
                    .debug_bounds("analysis-tab-bar")
                    .expect("Analysis tab bar should render");
                let sidebar_header = cx
                    .debug_bounds("project-sidebar-header")
                    .expect("Project sidebar header should render");
                let filter_row = cx
                    .debug_bounds("project-filter-row")
                    .expect("Project filter row should render");
                let filter = cx
                    .debug_bounds("project-run-filter")
                    .expect("Project filter should render");
                let baseline_group = cx
                    .debug_bounds("baseline-group-label")
                    .expect("Baseline group label should render");
                let brush_row = cx
                    .debug_bounds("brush-row")
                    .expect("Brush row should render");
                let brush_controls = cx
                    .debug_bounds("brush-controls")
                    .expect("Brush controls should render");
                let axis_picker = cx
                    .debug_bounds("axis-picker")
                    .expect("Axis picker should render");
                let add_metric = cx
                    .debug_bounds("add-metric")
                    .expect("Add Metric control should render");
                let tab = cx
                    .debug_bounds("analysis-tab")
                    .expect("active Analysis tab should render");
                let close = cx
                    .debug_bounds("close-active-view")
                    .expect("active View close control should render");
                let controls = cx
                    .debug_bounds("analysis-right-controls")
                    .expect("View toolbar controls should render");
                let new_view = cx
                    .debug_bounds("new-view")
                    .expect("View toolbar control should render");
                let inspector = cx
                    .debug_bounds("toggle-bottom-inspector")
                    .expect("bottom inspector control should render");
                let refresh = cx
                    .debug_bounds("refresh-view")
                    .expect("refresh control should render");

                assert_eq!(sidebar.origin.y, px(0.));
                assert_eq!(sidebar.size.height, px(window_size.1));
                assert_eq!(analysis.origin.x, sidebar.origin.x + sidebar.size.width);
                assert_eq!(tab_bar.origin.x, analysis.origin.x);
                assert_eq!(tab_bar.size.width, analysis.size.width);
                assert_eq!(tab_bar.size.height, px(32.));
                assert_eq!(sidebar_header.origin.y, tab_bar.origin.y);
                assert_eq!(sidebar_header.size.height, tab_bar.size.height);
                assert_eq!(filter_row.origin.y, brush_row.origin.y);
                assert_eq!(filter_row.size.height, brush_row.size.height);
                assert_eq!(filter_row.bottom(), brush_row.bottom());
                assert_eq!(baseline_group.origin.y - filter_row.bottom(), px(12.));
                assert_eq!(filter.origin.y, axis_picker.origin.y);
                assert_eq!(axis_picker.size.height, filter.size.height);
                assert_eq!(axis_picker.size.width, filter.size.height);
                assert_eq!(add_metric.size, axis_picker.size);
                assert_eq!(axis_picker.origin.x, brush_controls.origin.x + px(4.));
                assert_eq!(add_metric.right(), brush_controls.right() - px(5.));
                assert_eq!(tab.size.height, px(31.));
                assert_eq!(close.right(), tab.right() - px(5.));
                assert_eq!(new_view.origin.x, controls.origin.x + px(4.));
                assert_eq!(inspector.origin.x, new_view.right() + px(4.));
                assert_eq!(refresh.origin.x, inspector.right() + px(4.));
                assert_eq!(refresh.right(), controls.right() - px(4.));
                assert_eq!(new_view.size.height, px(20.));
            }

            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.hovered_project = Some(ProjectRef::new(
                        DataSourceId::from_path(root.path()),
                        ProjectId::from_string("project"),
                    ));
                    cx.notify();
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            let wide_project = cx
                .debug_bounds("project-tree-row-0-0")
                .expect("Project row should render");
            let wide_information = cx
                .debug_bounds("project-information-project")
                .expect("Project information should render on hover");
            assert!(wide_information.origin.x >= wide_project.right());
            assert!(
                f32::from(wide_information.origin.y - wide_project.origin.y).abs() <= 12.,
                "information {wide_information:?} should align with Project {wide_project:?}",
            );
            for selector in ["project-information-name", "project-information-source"] {
                let content = cx
                    .debug_bounds(selector)
                    .expect("Project information content should render");
                assert!(content.left() >= wide_information.left());
                assert!(content.right() <= wide_information.right());
            }
            cx.simulate_resize(size(px(420.), px(520.)));
            cx.run_until_parked();
            let project_menu = cx
                .debug_bounds("project-menu-project")
                .expect("Project menu control should render while hovered");
            let information = cx
                .debug_bounds("project-information-project")
                .expect("Project information should render while hovered");
            assert!(information.right() <= px(412.));
            assert!(information.bottom() <= px(512.));
            cx.simulate_click(project_menu.center(), Modifiers::default());
            let popover = cx
                .debug_bounds("project-popover-project")
                .expect("Project menu should open");
            let narrow_project = cx
                .debug_bounds("project-tree-row-0-0")
                .expect("Project row should remain rendered");
            assert!(popover.origin.x >= project_menu.origin.x);
            assert!(
                f32::from(popover.origin.y - narrow_project.origin.y).abs() <= 20.,
                "popover {popover:?} should anchor beside Project {narrow_project:?}",
            );
            assert!(popover.right() <= px(412.));
            assert!(popover.bottom() <= px(512.));
            assert!(cx.debug_bounds("project-menu-separator").is_some());
            assert!(cx.debug_bounds("reveal-project-source").is_none());
            assert!(cx.debug_bounds("refresh-project-source").is_none());
            assert!(cx.debug_bounds("remove-project-source").is_none());
            let pin = cx
                .debug_bounds("pin-project")
                .expect("normal Project menu should offer Pin project");
            cx.simulate_click(pin.center(), Modifiers::default());
            assert!(
                window
                    .read_with(&cx, |viewer, _| !viewer.views.pinned_projects().is_empty())
                    .expect("viewer should remain open")
            );
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.hovered_project = Some(ProjectRef::new(
                        DataSourceId::from_path(root.path()),
                        ProjectId::from_string("project"),
                    ));
                    cx.notify();
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            assert!(
                cx.debug_bounds("project-information-placement-icon")
                    .is_some()
            );
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.hovered_project = None;
                    viewer.project_menu = None;
                    cx.notify();
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
        }

        #[gpui::test]
        fn analysis_view_tabs_manage_the_active_view_lifecycle(cx: &mut TestAppContext) {
            let (window, mut cx) = open_viewer(cx, None);
            let original_view = window
                .update(&mut cx, |viewer, _, _| {
                    viewer.navigation.select_axis(AlignmentAxis::ElapsedTime);
                    viewer.views.active().view_id.clone()
                })
                .expect("viewer should remain open");

            let new_view = cx
                .debug_bounds("new-view")
                .expect("new View control should render");
            cx.simulate_mouse_move(new_view.center(), None, Modifiers::default());
            cx.executor().advance_clock(Duration::from_millis(500));
            cx.run_until_parked();
            assert!(cx.debug_bounds("label-tooltip").is_some());
            cx.simulate_click(new_view.center(), Modifiers::default());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.navigation.axis())
                    .expect("viewer should remain open"),
                AlignmentAxis::Step
            );
            let active_tab = cx
                .debug_bounds("analysis-tab")
                .expect("active View tab should render");
            cx.simulate_mouse_down(
                active_tab.center(),
                MouseButton::Right,
                Modifiers::default(),
            );
            let menu = cx
                .debug_bounds("view-menu")
                .expect("View menu should render");
            assert!(menu.top() >= active_tab.bottom());
            let duplicate = cx
                .debug_bounds("duplicate-view")
                .expect("duplicate View control should render");
            cx.simulate_click(duplicate.center(), Modifiers::default());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.views.views().len())
                    .expect("viewer should remain open"),
                3
            );

            let active_tab = cx
                .debug_bounds("analysis-tab")
                .expect("duplicated View tab should be active");
            cx.simulate_mouse_move(active_tab.center(), None, Modifiers::default());
            cx.simulate_mouse_down(
                active_tab.center(),
                MouseButton::Right,
                Modifiers::default(),
            );
            let rename = cx
                .debug_bounds("rename-view")
                .expect("View menu should expose Rename");
            cx.simulate_mouse_move(rename.center(), None, Modifiers::default());
            cx.simulate_click(rename.center(), Modifiers::default());
            assert!(cx.debug_bounds("rename-view-input").is_some());
            assert!(cx.debug_bounds("rename-view-selection").is_some());
            assert!(cx.debug_bounds("rename-view-caret").is_some());
            cx.executor().advance_clock(Duration::from_millis(500));
            cx.run_until_parked();
            assert!(cx.debug_bounds("rename-view-caret").is_none());
            cx.simulate_keystrokes("view left left right x");
            let prefix = cx
                .debug_bounds("rename-view-prefix")
                .expect("View name before the cursor should render");
            assert!(cx.debug_bounds("rename-view-suffix").is_some());
            let caret = cx
                .debug_bounds("rename-view-caret")
                .expect("typing should reveal the View name caret");
            assert_eq!(caret.origin.x, prefix.right() + px(1.));
            cx.simulate_keystrokes("enter");
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.views.active().name.clone())
                    .expect("viewer should remain open"),
                "viexw"
            );

            let active_tab = cx
                .debug_bounds("analysis-tab")
                .expect("renamed View tab should remain active");
            cx.simulate_mouse_down(
                active_tab.center(),
                MouseButton::Right,
                Modifiers::default(),
            );
            let rename = cx
                .debug_bounds("rename-view")
                .expect("View menu should expose Rename");
            cx.simulate_click(rename.center(), Modifiers::default());
            cx.simulate_keystrokes("outside");
            let controls = cx
                .debug_bounds("analysis-right-controls")
                .expect("View toolbar controls should render");
            cx.simulate_click(
                point(controls.origin.x + px(2.), controls.center().y),
                Modifiers::default(),
            );
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.renaming_view.is_none())
                    .expect("viewer should remain open")
            );
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.views.active().name.clone())
                    .expect("viewer should remain open"),
                "outside"
            );

            let close = cx
                .debug_bounds("close-active-view")
                .expect("active View close control should render");
            cx.simulate_click(close.center(), Modifiers::default());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.views.views().len())
                    .expect("viewer should remain open"),
                2
            );
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.activate_analysis_view(&original_view, cx);
                })
                .expect("viewer should remain open");
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.navigation.axis())
                    .expect("viewer should remain open"),
                AlignmentAxis::ElapsedTime
            );
        }

        #[gpui::test]
        fn bottom_inspector_can_close_after_switching_to_an_empty_view(cx: &mut TestAppContext) {
            let (window, mut cx) = open_viewer(cx, None);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                    viewer.bottom_inspector_visible = true;
                    viewer.create_analysis_view(cx);
                })
                .expect("viewer should remain open");
            window
                .read_with(&cx, |viewer, _| {
                    assert!(viewer.views.active().selected_panel_id.is_none());
                    assert!(viewer.bottom_inspector_visible);
                })
                .expect("viewer should remain open");

            let toggle = cx
                .debug_bounds("toggle-bottom-inspector")
                .expect("bottom inspector toggle should remain available");
            cx.simulate_click(toggle.center(), Modifiers::default());

            assert!(
                !window
                    .read_with(&cx, |viewer, _| viewer.bottom_inspector_visible)
                    .expect("viewer should remain open")
            );
        }

        #[gpui::test]
        fn worker_events_update_the_entity_without_render_polling(cx: &mut TestAppContext) {
            let (root, _, _) = fixture(1);
            cx.executor().allow_parking();
            let (window, cx) = open_viewer(cx, Some(root.path().to_path_buf()));

            wait_for_viewer(window, &cx, source_catalog_loaded);
        }

        #[gpui::test]
        fn project_tree_scrolls_to_runs_in_an_expanded_project(cx: &mut TestAppContext) {
            let (root, project_id, _) = fixture_with_runs(0, 12);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            let folder = cx
                .debug_bounds("project-folder-0-0")
                .expect("first Project folder should be rendered");
            cx.simulate_click(folder.center(), Modifiers::default());
            wait_for_viewer(window, &cx, |viewer| {
                viewer.sources.sources().next().is_some_and(|source| {
                    source
                        .catalog
                        .runs
                        .iter()
                        .any(|run| run.project_id == project_id)
                })
            });
            let tree = cx
                .debug_bounds("project-run-tree")
                .expect("Project tree should be rendered");
            let first_row = cx
                .debug_bounds("project-tree-run-0-0-0")
                .expect("first Run row should be rendered");
            let project_label = cx
                .debug_bounds("project-tree-label-0-0")
                .expect("Project label should render");
            let show_more = cx
                .debug_bounds("show-more-0-0")
                .expect("Project pagination should render");
            assert_eq!(show_more.origin.x, project_label.origin.x);
            let expected_row_height = window
                .read_with(&cx, |viewer, _| viewer.theme.spacing.tree_row_height)
                .expect("viewer should remain open");
            assert_eq!(first_row.size.height, expected_row_height);
            let eye_width = cx
                .debug_bounds("run-eye-0")
                .expect("Run visibility control should render")
                .size
                .width;
            assert!(cx.debug_bounds("run-status-0").is_some());
            assert!(cx.debug_bounds("run-actions-0").is_none());

            cx.simulate_mouse_move(
                point(first_row.origin.x + px(40.), first_row.center().y),
                None,
                Modifiers::default(),
            );

            assert!(cx.debug_bounds("run-status-0").is_none());
            assert!(cx.debug_bounds("run-actions-0").is_some());
            assert_eq!(
                cx.debug_bounds("run-eye-0")
                    .expect("Run visibility control should keep its width")
                    .size
                    .width,
                eye_width
            );
            let analysis_tab = cx
                .debug_bounds("analysis-tab")
                .expect("Analysis tab should render");
            cx.simulate_mouse_move(analysis_tab.center(), None, Modifiers::default());
            assert!(cx.debug_bounds("run-status-0").is_some());
            window
                .update(&mut cx, |viewer, window, cx| {
                    let source = viewer.sources.sources().next().expect("source");
                    let run = source.catalog.runs.first().expect("first Run");
                    let run_ref = RunRef::new(
                        source.source_id.clone(),
                        run.project_id.clone(),
                        run.run_id.clone(),
                    );
                    viewer
                        .run_focuses
                        .get(&run_ref)
                        .expect("rendered Run should own focus")
                        .focus(window);
                    cx.notify();
                })
                .expect("viewer should remain open");
            assert!(cx.debug_bounds("run-status-0").is_none());
            assert!(cx.debug_bounds("run-actions-0").is_some());
            assert_eq!(
                cx.debug_bounds("run-eye-0")
                    .expect("focused Run eye should keep its width")
                    .size
                    .width,
                eye_width
            );

            for _ in 0..2 {
                let show_more = cx
                    .debug_bounds("show-more-0-0")
                    .expect("Project pagination should expose more Runs");
                cx.simulate_click(show_more.center(), Modifiers::default());
            }

            cx.simulate_event(ScrollWheelEvent {
                position: tree.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-1_000.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            });

            assert!(cx.debug_bounds("project-tree-run-0-0-11").is_some());
        }

        #[gpui::test]
        fn project_filter_finds_runs_beyond_the_revealed_page(cx: &mut TestAppContext) {
            let (root, _, _) = fixture_with_runs(0, 12);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.run_filter = "baseline 11".to_owned();
                    cx.notify();
                })
                .expect("viewer should remain open");
            let folder = cx
                .debug_bounds("project-folder-0-0")
                .expect("matching Project should remain visible");

            cx.simulate_click(folder.center(), Modifiers::default());
            wait_for_viewer(window, &cx, |viewer| {
                viewer
                    .sources
                    .sources()
                    .next()
                    .is_some_and(|source| source.catalog.runs.len() == 12)
            });

            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.run_filter.clone())
                    .expect("viewer should remain open"),
                "baseline 11"
            );
            assert!(cx.debug_bounds("project-tree-run-0-0-0").is_some());
            assert!(cx.debug_bounds("project-tree-run-0-0-1").is_none());
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer
                        .sources
                        .sources()
                        .next()
                        .is_some_and(|source| source
                            .catalog
                            .runs
                            .iter()
                            .any(|run| run.name == "baseline 11")))
                    .expect("viewer should remain open")
            );
            assert!(cx.debug_bounds("show-more-0-0").is_none());

            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.run_filter = "viewer".to_owned();
                    cx.notify();
                })
                .expect("viewer should remain open");
            for selector in [
                "project-tree-run-0-0-0",
                "project-tree-run-0-0-1",
                "project-tree-run-0-0-2",
                "project-tree-run-0-0-3",
                "project-tree-run-0-0-4",
            ] {
                assert!(cx.debug_bounds(selector).is_some());
            }
            assert!(cx.debug_bounds("show-more-0-0").is_some());
        }

        #[gpui::test]
        fn project_visibility_keeps_baseline_and_pinned_runs_visible(cx: &mut TestAppContext) {
            let (root, project_id, _) = fixture_with_runs(0, 3);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| {
                viewer
                    .sources
                    .sources()
                    .next()
                    .is_some_and(|source| source.catalog.runs.len() == 3)
            });
            window
                .update(&mut cx, |viewer, _, cx| {
                    let source = viewer.sources.sources().next().expect("source").clone();
                    let project = SidebarProject {
                        source_index: 0,
                        project_index: 0,
                        project_ref: ProjectRef::new(source.source_id.clone(), project_id.clone()),
                        project: source.catalog.projects[0].clone(),
                        runs: source.catalog.runs.clone(),
                        source_label: "source".to_owned(),
                        placement: ProjectPlacement::Projects,
                    };
                    let run_ref = |index: usize| {
                        RunRef::new(
                            source.source_id.clone(),
                            project_id.clone(),
                            project.runs[index].run_id.clone(),
                        )
                    };
                    let baseline = run_ref(0);
                    let pinned = run_ref(1);
                    let ordinary = run_ref(2);
                    viewer.set_run_baseline(baseline.clone(), cx);
                    viewer.toggle_pinned_run(pinned.clone(), cx);
                    viewer.toggle_run(ordinary.clone(), cx);

                    viewer.toggle_project_runs(&project, cx);

                    assert!(viewer.views.active().runs.contains(&baseline));
                    assert!(viewer.views.active().runs.contains(&pinned));
                    assert!(!viewer.views.active().runs.contains(&ordinary));
                })
                .expect("viewer should remain open");
        }

        #[gpui::test]
        fn project_row_click_toggles_runs_without_changing_analysis(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_complete_runs(1, 1);
            cx.executor().allow_parking();
            cx.update(|cx| {
                cx.bind_keys([KeyBinding::new(
                    "enter",
                    ActivateSelection,
                    Some(SELECTABLE_CONTEXT),
                )]);
            });
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("metric-0"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            let before = window
                .read_with(&cx, |viewer, _| {
                    let panel = &viewer.views.active().panels[0];
                    (
                        viewer
                            .navigation
                            .brush()
                            .expect("timeline should be loaded"),
                        viewer.next_generation,
                        panel.overview.clone().expect("overview should be loaded"),
                        panel.detail.clone().expect("detail should be loaded"),
                        panel.overview_revision,
                        panel.detail_revision,
                    )
                })
                .expect("viewer should remain open");
            let project = cx
                .debug_bounds("project-tree-row-0-0")
                .expect("first Project row should be rendered");
            cx.simulate_click(project.center(), Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("project-tree-run-0-0-0").is_some());
            window
                .read_with(&cx, |viewer, _| {
                    let panel = &viewer.views.active().panels[0];
                    assert_eq!(viewer.navigation.brush(), Some(before.0));
                    assert_eq!(viewer.next_generation, before.1);
                    assert!(Arc::ptr_eq(
                        panel
                            .overview
                            .as_ref()
                            .expect("overview should remain loaded"),
                        &before.2,
                    ));
                    assert!(Arc::ptr_eq(
                        panel.detail.as_ref().expect("detail should remain loaded"),
                        &before.3,
                    ));
                    assert_eq!(panel.overview_revision, before.4);
                    assert_eq!(panel.detail_revision, before.5);
                })
                .expect("viewer should remain open");
            assert!(cx.debug_bounds("overview-chart").is_some());
            assert!(cx.debug_bounds("ruler-major-tick-0").is_some());
            assert!(cx.debug_bounds("project-information-project").is_none());
            cx.simulate_mouse_move(project.center(), None, Modifiers::default());
            assert!(cx.debug_bounds("project-information-project").is_some());
            assert!(cx.debug_bounds("project-menu-project").is_some());
            cx.simulate_click(project.center(), Modifiers::default());
            cx.run_until_parked();
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.expanded_projects.len())
                    .expect("viewer should remain open"),
                0,
            );
            window
                .read_with(&cx, |viewer, _| {
                    assert_eq!(viewer.navigation.brush(), Some(before.0));
                    assert_eq!(viewer.next_generation, before.1);
                })
                .expect("viewer should remain open");
        }

        #[gpui::test]
        fn project_sidebar_retains_multiple_imported_sources(cx: &mut TestAppContext) {
            let (first, _, _) = fixture_with_metric("loss");
            let (second, _, _) = fixture_with_metric("accuracy");
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(first.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| {
                viewer
                    .sources
                    .sources()
                    .next()
                    .is_some_and(|source| !source.catalog.projects.is_empty())
            });
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.open_source(second.path().to_path_buf(), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.sources.sources().len() == 2
                    && viewer
                        .sources
                        .sources()
                        .all(|source| !source.catalog.projects.is_empty())
            });

            assert!(cx.debug_bounds("project-tree-row-0-0").is_some());
            assert!(cx.debug_bounds("project-tree-row-1-0").is_some());
            assert!(cx.debug_bounds("analysis-tab").is_some());

            for (source_index, folder_selector, run_selector) in [
                (0, "project-folder-0-0", "project-tree-run-0-0-0"),
                (1, "project-folder-1-0", "project-tree-run-1-0-0"),
            ] {
                let folder = cx
                    .debug_bounds(folder_selector)
                    .expect("Project folder should be rendered");
                cx.simulate_click(folder.center(), Modifiers::default());
                wait_for_viewer(window, &cx, |viewer| {
                    viewer
                        .sources
                        .sources()
                        .nth(source_index)
                        .is_some_and(|source| !source.catalog.runs.is_empty())
                });
                let run = cx
                    .debug_bounds(run_selector)
                    .expect("Run row should be rendered");
                cx.simulate_click(run.center(), Modifiers::default());
            }
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.views.active().runs.len())
                    .expect("viewer should remain open"),
                2
            );
            wait_for_viewer(window, &cx, |viewer| {
                viewer.available_metric_keys().len() == 2
            });
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| {
                        viewer
                            .available_metric_keys()
                            .into_iter()
                            .map(|metric| metric.as_str().to_owned())
                            .collect::<Vec<_>>()
                    })
                    .expect("viewer should remain open"),
                ["accuracy", "loss"]
            );
        }

        #[gpui::test]
        fn top_refresh_requests_every_source_from_an_empty_view(cx: &mut TestAppContext) {
            let (first, _, _) = fixture_with_metric("loss");
            let (second, _, _) = fixture_with_metric("accuracy");
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(first.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.open_source(second.path().to_path_buf(), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| viewer.sources.sources().len() == 2);
            let before = window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.create_analysis_view(cx);
                    assert!(viewer.views.active().runs.is_empty());
                    viewer.next_generation
                })
                .expect("viewer should remain open");
            assert!(cx.debug_bounds("brush-controls").is_some());
            assert!(cx.debug_bounds("viewport-ruler").is_some());
            assert!(cx.debug_bounds("metric-track-scroll").is_some());
            assert!(cx.debug_bounds("open-project").is_none());

            let refresh = cx
                .debug_bounds("refresh-view")
                .expect("Refresh should remain available in an empty View");
            cx.simulate_click(refresh.center(), Modifiers::default());

            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.next_generation)
                    .expect("viewer should remain open")
                    >= before + 2
            );
        }

        #[gpui::test]
        fn shared_timeline_unions_extents_from_multiple_sources(cx: &mut TestAppContext) {
            let (first, first_project, first_run) = fixture_with_extent(10);
            let (second, second_project, second_run) = fixture_with_extent(20);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(first.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.open_source(second.path().to_path_buf(), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.sources.sources().len() == 2
                    && viewer.sources.sources().all(|source| {
                        source.status == SourceStatus::Ready && !source.catalog.projects.is_empty()
                    })
            });

            for (root, project_id, run_id) in [
                (first.path(), first_project, first_run),
                (second.path(), second_project, second_run),
            ] {
                let source_id = DataSourceId::from_path(root);
                window
                    .update(&mut cx, |viewer, _, cx| {
                        viewer.activate_tree_project(source_id.clone(), project_id.clone(), cx);
                    })
                    .expect("viewer should remain open");
                wait_for_viewer(window, &cx, |viewer| {
                    viewer.sources.source(&source_id).is_some_and(|source| {
                        source.catalog.runs.iter().any(|run| run.run_id == run_id)
                    })
                });
                window
                    .update(&mut cx, |viewer, _, cx| {
                        viewer.toggle_tree_run(
                            RunRef::new(source_id.clone(), project_id.clone(), run_id.clone()),
                            cx,
                        );
                    })
                    .expect("viewer should remain open");
            }
            wait_for_viewer(window, &cx, |viewer| {
                viewer.available_metric_keys().len() == 1
            });
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer
                    .views
                    .active()
                    .panels
                    .first()
                    .and_then(|panel| panel.overview.as_deref())
                    .is_some_and(|snapshot| snapshot.series.len() == 2)
            });

            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| {
                        viewer.navigation.brush().map(|brush| brush.home().end())
                    })
                    .expect("viewer should remain open"),
                Some(20.)
            );
        }

        #[gpui::test]
        fn switching_metric_tracks_preserves_the_shared_brush(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(2);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            cx.simulate_resize(size(px(1_000.), px(1_000.)));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 2);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("metric-0"), cx);
                    viewer.select_metric(MetricKey::from_string("metric-1"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().panels.len() == 2
                    && viewer
                        .views
                        .active()
                        .panels
                        .iter()
                        .all(|panel| panel.detail.is_some())
            });
            let brush = window
                .read_with(&cx, |viewer, _| viewer.navigation.brush())
                .expect("viewer should remain open")
                .expect("loaded metrics should create a shared brush");

            for metric in ["metric-0", "metric-1"] {
                let track = cx
                    .debug_bounds(if metric == "metric-0" {
                        "metric-sidebar-row:metric-0"
                    } else {
                        "metric-sidebar-row:metric-1"
                    })
                    .expect("Metric label should render");
                cx.simulate_click(track.center(), Modifiers::default());
                cx.run_until_parked();
                wait_for_viewer(window, &cx, |viewer| {
                    viewer
                        .views
                        .active()
                        .selected_panel_id
                        .as_ref()
                        .is_some_and(|panel_id| {
                            panel_id.as_str() == metric
                                && viewer
                                    .views
                                    .active_panel(panel_id)
                                    .is_some_and(|panel| !panel.is_pending(ReadKind::Inspector))
                        })
                });

                window
                    .read_with(&cx, |viewer, _| {
                        assert_eq!(viewer.navigation.brush(), Some(brush));
                        assert_eq!(
                            viewer
                                .views
                                .active()
                                .selected_panel_id
                                .as_ref()
                                .map(MetricPanelId::as_str),
                            Some(metric),
                        );
                    })
                    .expect("viewer should remain open");
                let controls = cx
                    .debug_bounds("brush-controls")
                    .expect("Brush controls should render");
                let row = cx
                    .debug_bounds("brush-row")
                    .expect("Brush row should render");
                let overview = cx
                    .debug_bounds("overview-chart")
                    .expect("Overview chart should render");
                assert_eq!(row.size.height, px(40.));
                assert_eq!(controls.size.height, overview.size.height);
                assert_eq!(controls.origin.y, overview.origin.y);
                assert_eq!(controls.bottom(), overview.bottom());
            }
        }

        #[gpui::test]
        fn run_organization_reuses_every_loaded_metric_snapshot(cx: &mut TestAppContext) {
            let (root, project_id, first_run_id) = fixture_with_complete_runs(2, 2);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 2);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("metric-0"), cx);
                    viewer.select_metric(MetricKey::from_string("metric-1"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().panels.iter().all(|panel| {
                    panel
                        .detail
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.series.len() == 1)
                })
            });
            let initial_revisions = window
                .read_with(&cx, |viewer, _| {
                    viewer
                        .views
                        .active()
                        .panels
                        .iter()
                        .map(|panel| panel.detail_revision)
                        .collect::<Vec<_>>()
                })
                .expect("viewer should remain open");
            let second_run = window
                .read_with(&cx, |viewer, _| {
                    RunRef::new(
                        first_source_id(viewer),
                        project_id.clone(),
                        RunId::from_string(
                            "run-1-with-a-very-long-identifier-that-requires-horizontal-scrolling",
                        ),
                    )
                })
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.toggle_run(second_run.clone(), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().panels.iter().all(|panel| {
                    panel
                        .detail
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.series.len() == 2)
                })
            });
            window
                .read_with(&cx, |viewer, _| {
                    assert_eq!(
                        viewer
                            .views
                            .active()
                            .panels
                            .iter()
                            .map(|panel| panel.detail_revision)
                            .collect::<Vec<_>>(),
                        initial_revisions,
                    );
                })
                .expect("viewer should remain open");
            let before = window
                .read_with(&cx, |viewer, _| {
                    viewer
                        .views
                        .active()
                        .panels
                        .iter()
                        .map(|panel| {
                            (
                                panel.detail.clone().expect("detail should be loaded"),
                                panel.detail_revision,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .expect("viewer should remain open");
            let timeline_before = window
                .read_with(&cx, |viewer, _| {
                    viewer
                        .navigation
                        .brush()
                        .expect("timeline should be loaded")
                })
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, _, cx| {
                    let hidden = viewer.views.active().runs[1].clone();
                    viewer.toggle_run(hidden, cx);
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            window
                .read_with(&cx, |viewer, _| {
                    assert_eq!(viewer.views.active().runs.len(), 1);
                    for (panel, (snapshot, revision)) in
                        viewer.views.active().panels.iter().zip(&before)
                    {
                        assert!(Arc::ptr_eq(
                            panel.detail.as_ref().expect("detail should remain loaded"),
                            snapshot,
                        ));
                        assert_eq!(panel.detail_revision, *revision);
                    }
                })
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, _, cx| {
                    let baseline = viewer.views.active().runs[0].clone();
                    viewer.set_run_baseline(baseline, cx);
                    viewer.toggle_pinned_run(second_run.clone(), cx);
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            window
                .read_with(&cx, |viewer, _| {
                    assert_eq!(viewer.navigation.brush(), Some(timeline_before));
                    for (panel, (snapshot, revision)) in
                        viewer.views.active().panels.iter().zip(&before)
                    {
                        assert!(Arc::ptr_eq(
                            panel.detail.as_ref().expect("detail should remain loaded"),
                            snapshot,
                        ));
                        assert_eq!(panel.detail_revision, *revision);
                    }
                })
                .expect("viewer should remain open");
            assert!(cx.debug_bounds("overview-chart").is_some());
            assert!(cx.debug_bounds("ruler-major-tick-0").is_some());
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.archive_run(second_run.clone(), cx);
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            window
                .read_with(&cx, |viewer, _| {
                    for (panel, (snapshot, revision)) in
                        viewer.views.active().panels.iter().zip(&before)
                    {
                        assert!(Arc::ptr_eq(
                            panel.detail.as_ref().expect("detail should remain loaded"),
                            snapshot,
                        ));
                        assert_eq!(panel.detail_revision, *revision);
                    }
                })
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.archive_run(second_run.clone(), cx);
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            window
                .read_with(&cx, |viewer, _| {
                    for (panel, (snapshot, revision)) in
                        viewer.views.active().panels.iter().zip(&before)
                    {
                        assert!(Arc::ptr_eq(
                            panel.detail.as_ref().expect("detail should remain loaded"),
                            snapshot,
                        ));
                        assert_eq!(panel.detail_revision, *revision);
                    }
                })
                .expect("viewer should remain open");
        }

        #[gpui::test]
        fn empty_view_keeps_the_converged_shell_and_opens_metric_picker(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(20);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 20);
            cx.simulate_resize(size(px(600.), px(520.)));
            cx.run_until_parked();
            assert!(cx.debug_bounds("brush-controls").is_some());
            assert!(cx.debug_bounds("viewport-ruler").is_some());
            assert!(cx.debug_bounds("metric-track-scroll").is_some());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.views.active().panels.len())
                    .expect("viewer should remain open"),
                0
            );
            let add = cx
                .debug_bounds("add-metric")
                .expect("empty View should retain Add Metric");
            cx.simulate_click(add.center(), Modifiers::default());
            cx.run_until_parked();
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.metric_picker_open)
                    .expect("viewer should remain open")
            );
        }

        #[gpui::test]
        fn metrics_appended_after_initial_layout_all_receive_detail(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_complete_runs(2, 1);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 2);

            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("metric-0"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);

            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("metric-1"), cx);
                    assert!(viewer.track_viewport.borrow().overscan.contains(&1));
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                let Some(viewport) = viewer.navigation.selected_viewport() else {
                    return false;
                };
                viewer.views.active().panels.len() == 2
                    && viewer.views.active().panels.iter().all(|panel| {
                        panel
                            .detail
                            .as_ref()
                            .is_some_and(|snapshot| snapshot.series.len() == 1)
                            && !panel.is_pending(ReadKind::Detail)
                            && panel.requested_detail_viewport == Some(viewport)
                    })
            });
        }

        #[gpui::test]
        fn popovers_close_after_clicking_outside_their_controls(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(2);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 2);

            let outside = cx
                .debug_bounds("new-view")
                .expect("New View should provide an outside click target")
                .center();
            let add_metric = cx
                .debug_bounds("add-metric")
                .expect("Add Metric control should render");
            cx.simulate_mouse_move(add_metric.center(), None, Modifiers::default());
            cx.simulate_click(add_metric.center(), Modifiers::default());
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.metric_picker_open)
                    .expect("viewer should remain open")
            );
            cx.simulate_click(add_metric.center(), Modifiers::default());
            assert!(
                !window
                    .read_with(&cx, |viewer, _| viewer.metric_picker_open)
                    .expect("viewer should remain open")
            );
            cx.simulate_click(add_metric.center(), Modifiers::default());
            let metric_picker = cx
                .debug_bounds("metric-picker")
                .expect("Metric picker should open");
            assert!(
                !metric_picker.contains(&outside),
                "outside target {outside:?} should not overlap picker {metric_picker:?}",
            );
            cx.simulate_mouse_move(outside, None, Modifiers::default());
            cx.simulate_mouse_down(outside, MouseButton::Left, Modifiers::default());
            assert!(
                !window
                    .read_with(&cx, |viewer, _| viewer.metric_picker_open)
                    .expect("viewer should remain open"),
                "outside mouse-down should clear the Metric picker state",
            );
            cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
            assert!(
                !window
                    .read_with(&cx, |viewer, _| viewer.metric_picker_open)
                    .expect("viewer should remain open")
            );

            let axis_picker = cx
                .debug_bounds("axis-picker")
                .expect("Axis picker control should render");
            cx.simulate_click(axis_picker.center(), Modifiers::default());
            assert!(cx.debug_bounds("axis-menu").is_some());
            cx.simulate_mouse_move(outside, None, Modifiers::default());
            cx.simulate_click(outside, Modifiers::default());
            assert!(
                !window
                    .read_with(&cx, |viewer, _| viewer.axis_picker_open)
                    .expect("viewer should remain open")
            );

            let active_tab = cx
                .debug_bounds("analysis-tab")
                .expect("active Analysis View should render");
            cx.simulate_mouse_down(
                active_tab.center(),
                MouseButton::Right,
                Modifiers::default(),
            );
            cx.simulate_mouse_up(
                active_tab.center(),
                MouseButton::Right,
                Modifiers::default(),
            );
            assert!(cx.debug_bounds("view-menu").is_some());
            cx.simulate_mouse_move(outside, None, Modifiers::default());
            cx.simulate_click(outside, Modifiers::default());
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.view_menu.is_none())
                    .expect("viewer should remain open")
            );
        }

        #[gpui::test]
        fn metric_picker_lists_only_metrics_not_already_in_the_view(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(20);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 20);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("metric-0"), cx);
                })
                .expect("viewer should remain open");
            let add = cx
                .debug_bounds("add-metric")
                .expect("Add Metric control should render beside the brush");
            cx.simulate_click(add.center(), Modifiers::default());
            let metric_picker = cx
                .debug_bounds("metric-picker")
                .expect("Metric picker should open below the Brush row");
            let candidates = cx
                .debug_bounds("metric-candidates")
                .expect("Metric candidates should use their own scroll region");
            let late_before = cx
                .debug_bounds("metric-candidate:metric-19")
                .expect("late Metric candidate should be laid out");
            assert_eq!(late_before.size.height, px(24.));
            assert!(late_before.origin.y >= candidates.bottom());
            cx.simulate_event(ScrollWheelEvent {
                position: candidates.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-1_000.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            });
            let late_after = cx
                .debug_bounds("metric-candidate:metric-19")
                .expect("late Metric candidate should remain laid out");
            assert!(late_after.origin.y < candidates.bottom());
            let axis = cx
                .debug_bounds("axis-picker")
                .expect("Axis picker should render beside Add Metric");
            let controls = cx
                .debug_bounds("brush-controls")
                .expect("Brush controls should own the fixed Metric label cell");
            let brush_row = cx
                .debug_bounds("brush-row")
                .expect("Brush row should render");
            assert_eq!(metric_picker.size.width, px(180.));
            assert_eq!(metric_picker.top(), brush_row.bottom() + px(4.));
            assert_eq!(metric_picker.left(), add.left());
            assert_eq!(axis.origin.x, controls.origin.x + px(4.));
            assert_eq!(add.right(), controls.right() - px(5.));
            cx.simulate_click(axis.center(), Modifiers::default());
            let axis_menu = cx
                .debug_bounds("axis-menu")
                .expect("Axis menu should open below the Brush row");
            assert_eq!(axis_menu.size.width, px(160.));
            assert_eq!(axis_menu.top(), brush_row.bottom() + px(4.));
            assert_eq!(axis_menu.left(), axis.left());
            window
                .read_with(&cx, |viewer, _| {
                    assert!(viewer.axis_picker_open);
                    assert!(!viewer.metric_picker_open);
                })
                .expect("viewer should remain open");
            cx.simulate_click(add.center(), Modifiers::default());
            window
                .read_with(&cx, |viewer, _| {
                    assert!(!viewer.axis_picker_open);
                    assert!(viewer.metric_picker_open);
                })
                .expect("viewer should remain open");
            assert!(cx.debug_bounds("metric-candidate:metric-0").is_none());
            let filter = cx
                .debug_bounds("metric-filter")
                .expect("Metric picker should expose search");
            cx.simulate_click(filter.center(), Modifiers::default());
            cx.simulate_keystrokes("2");
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.metric_filter.clone())
                    .expect("viewer should remain open"),
                "2"
            );
            let candidate = cx
                .debug_bounds("metric-candidate:metric-2")
                .expect("matching unadded Metric should be offered");

            cx.simulate_click(candidate.center(), Modifiers::default());
            cx.run_until_parked();

            assert!(
                window
                    .read_with(&cx, |viewer, _| {
                        !viewer.metric_picker_open
                            && viewer
                                .views
                                .active()
                                .panels
                                .iter()
                                .any(|panel| panel.metric_key.as_str() == "metric-2")
                    })
                    .expect("viewer should remain open")
            );
        }

        #[gpui::test]
        fn axis_picker_switches_the_view_to_observation_time(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(1);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("metric-0"), cx);
                })
                .expect("viewer should remain open");
            let picker = cx
                .debug_bounds("axis-picker")
                .expect("axis picker should render beside Add Metric");
            cx.simulate_click(picker.center(), Modifiers::default());
            let time = cx
                .debug_bounds("axis-time")
                .expect("axis menu should offer Absolute time");
            cx.simulate_click(time.center(), Modifiers::default());
            wait_for_viewer(window, &cx, |viewer| {
                viewer.navigation.axis() == AlignmentAxis::ElapsedTime
                    && viewer
                        .views
                        .active()
                        .panels
                        .first()
                        .and_then(|panel| panel.overview.as_ref())
                        .and_then(|snapshot| snapshot.real_range)
                        .is_some_and(|range| range.start() > 1_000_000_000_000)
            });
            assert!(cx.debug_bounds("viewport-ruler").is_some());
        }

        #[gpui::test]
        fn hover_and_locked_cursors_coexist_on_the_shared_ruler(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            let ruler = cx
                .debug_bounds("ruler-hit-area")
                .expect("shared ruler hit area should render");
            let first = point(ruler.origin.x + ruler.size.width * 0.25, ruler.center().y);
            cx.simulate_mouse_move(first, None, Modifiers::default());
            cx.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_up(first, MouseButton::Left, Modifiers::default());
            let locked = window
                .read_with(&cx, |viewer, _| viewer.locked_cursor)
                .expect("viewer should remain open")
                .expect("ruler click should lock a cursor");
            let second = point(ruler.origin.x + ruler.size.width * 0.75, ruler.center().y);
            cx.simulate_mouse_move(second, None, Modifiers::default());

            let capsule = cx
                .debug_bounds("ruler-hover-tooltip")
                .expect("hover coordinate capsule should render");
            assert!(f32::from(capsule.center().x - second.x).abs() < 40.);
            assert_eq!(capsule.size.height, px(18.));
            assert!(cx.debug_bounds("track-hover-callout").is_some());
            let track_scroll = cx
                .debug_bounds("metric-track-scroll")
                .expect("Metric track viewport should render");
            let track = cx
                .debug_bounds("metric-track:loss")
                .expect("Metric track should render");
            let cursor_overlay = cx
                .debug_bounds("metric-cursor-overlay")
                .expect("Shared Metric cursor overlay should render");
            assert_eq!(cursor_overlay.origin.x, track.origin.x);
            assert_eq!(cursor_overlay.origin.y, track_scroll.origin.y);
            assert_eq!(cursor_overlay.bottom(), track_scroll.bottom());
            window
                .read_with(&cx, |viewer, _| {
                    assert_eq!(viewer.locked_cursor, Some(locked));
                    assert!(viewer.ruler_hover.is_some_and(|hover| hover != locked));
                })
                .expect("viewer should remain open");

            cx.dispatch_action(ClearLockedCursor);

            window
                .read_with(&cx, |viewer, _| {
                    assert!(viewer.locked_cursor.is_none());
                    assert!(viewer.ruler_hover.is_some());
                })
                .expect("viewer should remain open");

            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.locked_cursor = Some(locked);
                    viewer.ruler_hover = None;
                    viewer.track_hovers.clear();
                    viewer.hovered_run = viewer.active_visible_runs().into_iter().next();
                    cx.notify();
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            let callout = cx
                .debug_bounds("track-hover-callout")
                .expect("Sidebar hover should show one locked-cursor tooltip");
            let track = cx
                .debug_bounds("metric-track:loss")
                .expect("Metric track should render");
            assert!(callout.size.height < track.size.height);
            assert!(callout.size.width < px(160.));
            assert!(cx.debug_bounds("track-tooltip-color").is_some());
            assert!(cx.debug_bounds("track-tooltip-label").is_some());
        }

        #[gpui::test]
        fn hover_frames_reuse_static_metric_chart_preparation(cx: &mut TestAppContext) {
            let (root, project_id, first_run_id) = fixture_with_complete_runs(3, 2);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 3);
            window
                .update(&mut cx, |viewer, _, cx| {
                    let source_id = first_source_id(viewer);
                    viewer.toggle_run(
                        RunRef::new(
                            source_id,
                            project_id,
                            RunId::from_string(
                                "run-1-with-a-very-long-identifier-that-requires-horizontal-scrolling",
                            ),
                        ),
                        cx,
                    );
                    for index in 0..3 {
                        viewer.select_metric(
                            MetricKey::from_string(format!("metric-{index}")),
                            cx,
                        );
                    }
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.track_adapters.len() == 3
                    && viewer.views.active().panels.iter().all(|panel| {
                        panel.detail.as_ref().is_some_and(|snapshot| {
                            snapshot.series.len() == 2 && !panel.is_pending(ReadKind::Detail)
                        })
                    })
            });
            cx.run_until_parked();
            let preparation_counts = |viewer: &ViewerApp| {
                viewer
                    .track_adapters
                    .iter()
                    .map(|(panel_id, adapter)| {
                        (
                            panel_id.as_str().to_owned(),
                            adapter.borrow().detail_prepare_count(),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            };
            let before = window
                .read_with(&cx, |viewer, _| preparation_counts(viewer))
                .expect("viewer should remain open");
            let ruler = cx
                .debug_bounds("ruler-hit-area")
                .expect("shared ruler should render");
            for index in 1..10 {
                let position = point(
                    ruler.origin.x + ruler.size.width * (index as f32 / 10.),
                    ruler.center().y,
                );
                cx.simulate_mouse_move(position, None, Modifiers::default());
            }
            let tooltip = cx
                .debug_bounds("track-hover-callout")
                .expect("Ruler hover should render one value per Metric");
            assert_eq!(tooltip.size.height, px(22.));
            let after = window
                .read_with(&cx, |viewer, _| preparation_counts(viewer))
                .expect("viewer should remain open");

            assert_eq!(after, before);
        }

        #[gpui::test]
        fn chart_pointer_drives_the_shared_hover_cursor_without_a_curve_hit(
            cx: &mut TestAppContext,
        ) {
            let (root, project_id, run_id) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                    viewer.views.active_mut().panels[0].row_height = 180.;
                    cx.notify();
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            let canvas = cx
                .debug_bounds("metric-canvas:loss")
                .expect("Metric chart should render");
            let pointer = point(
                canvas.origin.x + canvas.size.width * 0.25,
                canvas.bottom() - px(2.),
            );

            cx.simulate_mouse_move(pointer, None, Modifiers::default());

            assert!(cx.debug_bounds("ruler-hover-tooltip").is_some());
            window
                .read_with(&cx, |viewer, _| {
                    assert!(viewer.ruler_hover.is_none());
                    assert!(
                        !viewer
                            .track_hovers
                            .contains_key(&MetricPanelId::from_string("loss"))
                    );
                    assert!(
                        viewer.track_pointer_hover.as_ref().is_some_and(
                            |(panel_id, axis)| panel_id.as_str() == "loss" && axis.is_finite()
                        )
                    );
                })
                .expect("viewer should remain open");
        }

        #[gpui::test]
        fn zooming_multiple_metrics_repaints_without_pointer_motion(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_complete_runs(3, 1);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 3);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("metric-0"), cx);
                    viewer.select_metric(MetricKey::from_string("metric-1"), cx);
                    viewer.select_metric(MetricKey::from_string("metric-2"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                let Some(viewport) = viewer.navigation.selected_viewport() else {
                    return false;
                };
                viewer.views.active().panels.len() == 3
                    && viewer.views.active().panels.iter().all(|panel| {
                        panel.detail.is_some()
                            && !panel.is_pending(ReadKind::Detail)
                            && panel.requested_detail_viewport == Some(viewport)
                    })
                    && viewer.track_charts.len() == 3
            });
            window
                .update(&mut cx, |viewer, _, cx| {
                    let brush = viewer.navigation.brush_mut().expect("brush should exist");
                    let center = brush.home().start() + brush.home().span() / 2.;
                    brush.zoom_at(center, 2.).expect("brush should zoom in");
                    viewer.request_detail(cx);
                    cx.notify();
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                let Some(viewport) = viewer.navigation.selected_viewport() else {
                    return false;
                };
                viewer.views.active().panels.iter().all(|panel| {
                    panel.detail.is_some()
                        && !panel.is_pending(ReadKind::Detail)
                        && panel.requested_detail_viewport == Some(viewport)
                })
            });
            let charts = window
                .read_with(&cx, |viewer, _| {
                    ["metric-0", "metric-1", "metric-2"]
                        .into_iter()
                        .map(|metric| {
                            viewer
                                .track_charts
                                .get(&MetricPanelId::from_string(metric))
                                .cloned()
                                .expect("Metric chart should be cached")
                        })
                        .collect::<Vec<_>>()
                })
                .expect("viewer should remain open");
            let before = charts
                .iter()
                .map(|chart| chart.read_with(&cx, |chart, _| chart.viewport()))
                .collect::<Vec<_>>();
            let prepare_counts = |viewer: &ViewerApp| {
                ["metric-0", "metric-1", "metric-2"]
                    .into_iter()
                    .map(|metric| {
                        let panel_id = MetricPanelId::from_string(metric);
                        (
                            metric,
                            viewer
                                .track_adapters
                                .get(&panel_id)
                                .expect("Metric adapter should be cached")
                                .borrow()
                                .detail_prepare_count(),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            };
            let prepares_before = window
                .read_with(&cx, |viewer, _| prepare_counts(viewer))
                .expect("viewer should remain open");
            let narrow = window
                .read_with(&cx, |viewer, _| {
                    viewer
                        .navigation
                        .brush()
                        .expect("brush should exist")
                        .selected()
                })
                .expect("viewer should remain open");
            assert!(before.iter().all(|viewport| viewport.x == narrow));

            let canvas = cx
                .debug_bounds("metric-canvas:metric-0")
                .expect("Metric chart should render");
            cx.simulate_event(ScrollWheelEvent {
                position: canvas.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(1_000.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            });
            cx.run_until_parked();

            let selected = window
                .read_with(&cx, |viewer, _| {
                    viewer
                        .navigation
                        .brush()
                        .expect("brush should exist")
                        .selected()
                })
                .expect("viewer should remain open");
            let after = charts
                .iter()
                .map(|chart| chart.read_with(&cx, |chart, _| chart.viewport()))
                .collect::<Vec<_>>();
            assert_ne!(selected, narrow);
            for (before, after) in before.iter().zip(&after) {
                assert_ne!(after.x, before.x);
                assert_eq!(after.x, selected);
            }
            let prepares_after = window
                .read_with(&cx, |viewer, _| prepare_counts(viewer))
                .expect("viewer should remain open");
            for metric in ["metric-0", "metric-1", "metric-2"] {
                assert!(
                    prepares_after[metric] > prepares_before[metric],
                    "{metric} should prepare its expanded viewport immediately"
                );
            }

            cx.simulate_event(ScrollWheelEvent {
                position: canvas.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-100.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            });
            cx.run_until_parked();

            let contracted = window
                .read_with(&cx, |viewer, _| {
                    viewer
                        .navigation
                        .brush()
                        .expect("brush should exist")
                        .selected()
                })
                .expect("viewer should remain open");
            let after_zoom_in = charts
                .iter()
                .map(|chart| chart.read_with(&cx, |chart, _| chart.viewport()))
                .collect::<Vec<_>>();
            assert!(contracted.span() < selected.span());
            assert!(
                after_zoom_in
                    .iter()
                    .all(|viewport| viewport.x == contracted)
            );
            let prepares_after_zoom_in = window
                .read_with(&cx, |viewer, _| prepare_counts(viewer))
                .expect("viewer should remain open");
            for metric in ["metric-0", "metric-1", "metric-2"] {
                assert!(
                    prepares_after_zoom_in[metric] > prepares_after[metric],
                    "{metric} should prepare its contracted viewport immediately"
                );
            }

            cx.executor().advance_clock(Duration::from_millis(101));
            cx.run_until_parked();
            wait_for_viewer(window, &cx, |viewer| {
                let Some(detail_viewport) = viewer.navigation.selected_viewport() else {
                    return false;
                };
                !viewer.metric_repaint_pending
                    && viewer.views.active().panels.iter().all(|panel| {
                        panel.detail.is_some()
                            && !panel.is_pending(ReadKind::Detail)
                            && panel.requested_detail_viewport == Some(detail_viewport)
                    })
            });
        }

        #[gpui::test]
        fn ruler_drag_pans_the_shared_viewport_within_home(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| viewer.navigation.brush().is_some());
            window
                .update(&mut cx, |viewer, _, _| {
                    let brush = viewer.navigation.brush_mut().expect("brush should exist");
                    let center = brush.home().start() + brush.home().span() / 2.;
                    brush.zoom_at(center, 2.).expect("zoom should succeed");
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            let before = window
                .read_with(&cx, |viewer, _| {
                    viewer.navigation.brush().map(|brush| brush.selected())
                })
                .expect("viewer should remain open")
                .expect("selected viewport should exist");
            let ruler = cx
                .debug_bounds("ruler-hit-area")
                .expect("shared ruler hit area should render");
            let start = ruler.center();
            let end = point(ruler.origin.x + ruler.size.width * 0.75, ruler.center().y);

            cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
            cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());

            window
                .read_with(&cx, |viewer, _| {
                    let brush = viewer
                        .navigation
                        .brush()
                        .expect("brush should remain available");
                    assert_ne!(brush.selected(), before);
                    assert!(
                        (brush.selected().span() - before.span()).abs()
                            <= f64::EPSILON * before.span().abs().max(1.)
                    );
                    assert!(brush.selected().start() >= brush.home().start());
                    assert!(brush.selected().end() <= brush.home().end());
                    assert!(viewer.drag.is_none());
                })
                .expect("viewer should remain open");
        }

        #[gpui::test]
        fn ruler_scroll_pans_the_viewport_and_clamps_to_home(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| viewer.navigation.brush().is_some());
            window
                .update(&mut cx, |viewer, _, _| {
                    let brush = viewer.navigation.brush_mut().expect("brush should exist");
                    let center = brush.home().start() + brush.home().span() / 2.;
                    brush.zoom_at(center, 2.).expect("zoom should succeed");
                })
                .expect("viewer should remain open");
            let before = window
                .read_with(&cx, |viewer, _| {
                    viewer.navigation.brush().expect("brush").selected()
                })
                .expect("viewer should remain open");
            let ruler = cx
                .debug_bounds("ruler-hit-area")
                .expect("shared ruler hit area should render");
            let track = cx
                .debug_bounds("metric-track:loss")
                .expect("Metric track should render");
            let position = point(ruler.origin.x + px(10.), ruler.center().y);
            assert!(position.x < track.origin.x);

            for delta in [-80., -10_000.] {
                cx.simulate_event(ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                });
            }

            window
                .read_with(&cx, |viewer, _| {
                    let brush = viewer
                        .navigation
                        .brush()
                        .expect("brush should remain available");
                    assert!(
                        (brush.selected().span() - before.span()).abs()
                            <= f64::EPSILON * before.span().abs().max(1.)
                    );
                    assert!(brush.selected().start() > before.start());
                    assert_eq!(brush.selected().end(), brush.home().end());
                    assert!(viewer.detail_refresh_pending);
                })
                .expect("viewer should remain open");
        }

        #[gpui::test]
        fn modified_ruler_scroll_zooms_around_the_pointer(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| viewer.navigation.brush().is_some());
            window
                .update(&mut cx, |viewer, _, _| {
                    let brush = viewer.navigation.brush_mut().expect("brush should exist");
                    let center = brush.home().start() + brush.home().span() / 2.;
                    brush.zoom_at(center, 2.).expect("zoom should succeed");
                })
                .expect("viewer should remain open");
            let before = window
                .read_with(&cx, |viewer, _| {
                    viewer.navigation.brush().expect("brush").selected()
                })
                .expect("viewer should remain open");
            let plot = cx
                .debug_bounds("overview-chart")
                .expect("Overview plot should render");
            let ruler = cx
                .debug_bounds("ruler-hit-area")
                .expect("Shared ruler hit area should render");
            let anchor_ratio = 0.25;
            let position = point(
                plot.origin.x + plot.size.width * anchor_ratio,
                ruler.center().y,
            );
            let anchor = before.start() + before.span() * f64::from(anchor_ratio);
            let modifiers = Modifiers {
                platform: true,
                ..Modifiers::default()
            };

            cx.simulate_event(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
                modifiers,
                touch_phase: TouchPhase::Moved,
            });

            window
                .read_with(&cx, |viewer, _| {
                    let selected = viewer.navigation.brush().expect("brush").selected();
                    let anchored = selected.start() + selected.span() * f64::from(anchor_ratio);
                    assert!(selected.span() < before.span());
                    assert!((anchored - anchor).abs() < 1e-9);
                    assert!(viewer.detail_refresh_pending);
                })
                .expect("viewer should remain open");
        }

        #[gpui::test]
        fn metric_sidebar_rows_align_with_independent_chart_tracks(cx: &mut TestAppContext) {
            let (root, project_id, first_run_id) = fixture_with_complete_runs(2, 2);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 2);
            window
                .update(&mut cx, |viewer, _, cx| {
                    let source_id = first_source_id(viewer);
                    viewer.toggle_run(
                        RunRef::new(
                            source_id,
                            project_id,
                            RunId::from_string(
                                "run-1-with-a-very-long-identifier-that-requires-horizontal-scrolling",
                            ),
                        ),
                        cx,
                    );
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.available_metric_keys().len() == 2
            });
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("metric-0"), cx);
                    viewer.select_metric(MetricKey::from_string("metric-1"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().panels.len() == 2
                    && viewer
                        .views
                        .active()
                        .panels
                        .iter()
                        .all(|panel| panel.detail.is_some())
            });

            let workspace = cx
                .debug_bounds("analysis-workspace")
                .expect("Analysis workspace should render");
            let track_scroll = cx
                .debug_bounds("metric-track-scroll")
                .expect("Metric track viewport should render");
            assert_eq!(track_scroll.origin.x, workspace.origin.x);
            assert_eq!(track_scroll.size.width, workspace.size.width);
            let overview = cx
                .debug_bounds("overview-chart")
                .expect("Global overview should render");
            let ruler = cx
                .debug_bounds("viewport-ruler")
                .expect("Viewport ruler should render");
            assert_eq!(ruler.origin.x, workspace.origin.x);
            assert_eq!(ruler.size.width, workspace.size.width);
            assert!(overview.origin.x > ruler.origin.x);
            assert_eq!(overview.right(), ruler.right());
            let major_tick = cx
                .debug_bounds("ruler-major-mark-0")
                .expect("Ruler should render major tick marks");
            let minor_tick = cx
                .debug_bounds("ruler-minor-tick-0")
                .expect("Ruler should render minor tick marks");
            assert_eq!(major_tick.size, size(px(1.), px(8.)));
            assert_eq!(minor_tick.size, size(px(1.), px(5.)));
            assert_eq!(major_tick.origin.x, overview.origin.x);

            let gutter_width = f64::from(overview.origin.x - ruler.origin.x);
            let plot_width = f64::from(overview.size.width);
            window
                .update(&mut cx, |viewer, _, cx| {
                    let brush = viewer.navigation.brush_mut().expect("brush should exist");
                    let center = brush.home().start() + brush.home().span() / 2.;
                    brush.zoom_at(center, 2.).expect("zoom should succeed");
                    let selected = brush.selected();
                    let target_start =
                        brush.home().start() + selected.span() * gutter_width / plot_width;
                    brush
                        .pan_by(target_start - selected.start())
                        .expect("ruler viewport should pan");
                    cx.notify();
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            let home_tick = cx
                .debug_bounds("ruler-major-mark-0")
                .expect("Home tick should remain visible over the Metric label gutter");
            assert!(
                f32::from(home_tick.origin.x - ruler.origin.x).abs() < 0.5,
                "home tick {home_tick:?} should align with ruler {ruler:?}",
            );

            for metric in ["metric-0", "metric-1"] {
                let sidebar = cx
                    .debug_bounds(if metric == "metric-0" {
                        "metric-sidebar-row:metric-0"
                    } else {
                        "metric-sidebar-row:metric-1"
                    })
                    .expect("Metric sidebar row should render");
                let track = cx
                    .debug_bounds(if metric == "metric-0" {
                        "metric-track:metric-0"
                    } else {
                        "metric-track:metric-1"
                    })
                    .expect("Metric track should render");
                let canvas = cx
                    .debug_bounds(if metric == "metric-0" {
                        "metric-canvas:metric-0"
                    } else {
                        "metric-canvas:metric-1"
                    })
                    .expect("Metric canvas should render");
                let metadata = cx
                    .debug_bounds(if metric == "metric-0" {
                        "metric-metadata:metric-0"
                    } else {
                        "metric-metadata:metric-1"
                    })
                    .expect("Metric metadata should render on the second label line");
                assert_eq!(sidebar.origin.y, track.origin.y);
                assert_eq!(sidebar.size.height, track.size.height);
                assert_eq!(track.origin.x, sidebar.origin.x + sidebar.size.width);
                assert_eq!(
                    track.origin.x + track.size.width,
                    workspace.origin.x + workspace.size.width
                );
                assert!(track.origin.x > ruler.origin.x);
                assert_eq!(track.right(), ruler.right());
                assert_eq!(canvas.origin.x, track.origin.x);
                assert_eq!(canvas.size.width, track.size.width);
                assert!(canvas.size.width > px(0.));
                assert_eq!(
                    canvas.size.height,
                    track.size.height - px(METRIC_TRACK_VERTICAL_PADDING * 2.)
                );
                assert!(metadata.size.height > px(0.));
                assert!(
                    metadata.origin.y + metadata.size.height
                        <= sidebar.origin.y + sidebar.size.height,
                    "metadata {metadata:?} must remain inside sidebar {sidebar:?}",
                );
            }
            let (ranges, unavailable) = window
                .read_with(&cx, |viewer, _| {
                    let selected = viewer.navigation.brush().map(|brush| brush.selected());
                    let ranges = viewer
                        .views
                        .active()
                        .panels
                        .iter()
                        .map(|panel| {
                            renderer::detail_viewport(
                                panel.detail.as_deref().expect("detail should be loaded"),
                                selected,
                                None,
                            )
                            .expect("detail should be drawable")
                            .y
                        })
                        .collect::<Vec<_>>();
                    let unavailable = viewer.views.active().panels.iter().all(|panel| {
                        panel.detail.as_ref().is_some_and(|snapshot| {
                            snapshot.series.iter().any(|series| {
                                series.evidence.completeness == EvidenceCompleteness::Unavailable
                            })
                        })
                    });
                    (ranges, unavailable)
                })
                .expect("viewer should remain open");
            assert_ne!(ranges[0], ranges[1]);
            assert!(!unavailable);

            for selector in ["metric-resize:metric-0", "metric-resize:metric-1"] {
                let resize = cx
                    .debug_bounds(selector)
                    .expect("Every Metric row resize handle should render");
                let target = point(resize.center().x, resize.center().y + px(40.));
                cx.simulate_mouse_down(resize.center(), MouseButton::Left, Modifiers::default());
                cx.simulate_mouse_move(target, Some(MouseButton::Left), Modifiers::default());
                cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            }
            let heights = window
                .read_with(&cx, |viewer, _| {
                    viewer
                        .views
                        .active()
                        .panels
                        .iter()
                        .map(|panel| panel.row_height)
                        .collect::<Vec<_>>()
                })
                .expect("viewer should remain open");
            assert_eq!(heights, [92., 92.]);

            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.remove_metric_panel(&MetricPanelId::from_string("metric-0"), cx);
                })
                .expect("viewer should remain open");
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| {
                        viewer
                            .views
                            .active()
                            .panels
                            .iter()
                            .map(|panel| panel.metric_key.as_str().to_owned())
                            .collect::<Vec<_>>()
                    })
                    .expect("viewer should remain open"),
                ["metric-1".to_owned()]
            );
        }

        #[gpui::test]
        fn metric_click_opens_a_resizable_inspector_without_gesture_toggles(
            cx: &mut TestAppContext,
        ) {
            let (root, project_id, run_id) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            assert!(cx.debug_bounds("bottom-inspector").is_none());

            let metric_row = cx
                .debug_bounds("metric-sidebar-row:loss")
                .expect("Metric sidebar row should render");
            cx.simulate_click(metric_row.center(), Modifiers::default());
            let inspector = cx
                .debug_bounds("bottom-inspector")
                .expect("Bottom inspector should open");
            let scroll = cx
                .debug_bounds("bottom-inspector-scroll")
                .expect("Inspector content should own a scroll viewport");
            assert!(cx.debug_bounds("bottom-inspector-header").is_none());
            assert!(cx.debug_bounds("inspector-context").is_none());
            assert!(cx.debug_bounds("close-inspector").is_none());
            assert_eq!(scroll.origin.x, inspector.origin.x);
            assert_eq!(scroll.size.width, inspector.size.width);
            assert!(scroll.size.height < inspector.size.height);
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().panels.first().is_some_and(|panel| {
                    panel.inspector.is_some() && !panel.is_pending(ReadKind::Inspector)
                })
            });
            cx.simulate_resize(size(px(600.), px(520.)));
            cx.run_until_parked();
            let scroll = cx
                .debug_bounds("bottom-inspector-scroll")
                .expect("Inspector scroll viewport should remain rendered");
            let table = cx
                .debug_bounds("inspector-table")
                .expect("Inspector table should remain rendered");
            assert!(table.size.width > scroll.size.width);
            cx.simulate_resize(size(px(1_000.), px(1_000.)));
            cx.run_until_parked();
            window
                .read_with(&cx, |viewer, _| {
                    let run = viewer.views.active().panels[0]
                        .inspector
                        .as_ref()
                        .expect("exact inspector evidence should load")
                        .runs
                        .first()
                        .expect("fixture Run should be present");
                    let summary = run
                        .summary
                        .as_ref()
                        .expect("fixture should have a metric summary");
                    assert_eq!(summary.effective_count, 2);
                    assert_eq!(
                        (
                            summary.last_step.value(),
                            summary.last_value_f64,
                            summary.min_value_f64,
                            summary.max_value_f64,
                        ),
                        (100, 0.5, 0.5, 1.)
                    );
                    assert_eq!(run.evidence.last_step.map(|step| step.value()), Some(100));
                    assert_eq!(run.evidence.last_value_f64, Some(0.5));
                    assert_eq!(run.evidence.completeness, EvidenceCompleteness::Complete);
                })
                .expect("viewer should remain open");

            let inspector_before = window
                .read_with(&cx, |viewer, _| {
                    Arc::clone(
                        viewer.views.active().panels[0]
                            .inspector
                            .as_ref()
                            .expect("inspector snapshot should remain available"),
                    )
                })
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.zoom_from_keyboard(1.25, cx);
                })
                .expect("viewer should remain open");
            window
                .read_with(&cx, |viewer, _| {
                    let panel = &viewer.views.active().panels[0];
                    assert!(panel.inspector_generation.is_none());
                    assert!(Arc::ptr_eq(
                        &inspector_before,
                        panel
                            .inspector
                            .as_ref()
                            .expect("zoom should retain the whole-series inspector"),
                    ));
                })
                .expect("viewer should remain open");

            assert!(cx.debug_bounds("inspector-summary").is_none());
            assert!(cx.debug_bounds("inspector-ranking").is_none());
            assert!(cx.debug_bounds("inspector-evidence").is_none());
            for selector in [
                "inspector-header:run",
                "inspector-header:last-value",
                "inspector-header:min",
                "inspector-header:max",
                "inspector-header:locked",
                "inspector-header:hover",
                "inspector-header:count",
                "inspector-header:last-step",
                "inspector-header:status",
                "inspector-header:evidence",
                "inspector-header:project",
            ] {
                assert!(
                    cx.debug_bounds(selector).is_some(),
                    "every inspector header should be sortable: {selector}",
                );
            }
            assert!(cx.debug_bounds("inspector-column-resize:project").is_none());
            let resized_header = cx
                .debug_bounds("inspector-header:last-value")
                .expect("Last value header should render");
            let column_resize = cx
                .debug_bounds("inspector-column-resize:last-value")
                .expect("Last value column resize boundary should render");
            let resize_target = point(column_resize.center().x + px(36.), column_resize.center().y);
            cx.simulate_mouse_down(
                column_resize.center(),
                MouseButton::Left,
                Modifiers::default(),
            );
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| {
                        viewer.inspector_column_resize.map(|resize| resize.column)
                    })
                    .expect("viewer should remain open"),
                Some(InspectorColumn::LastValue)
            );
            cx.simulate_mouse_move(resize_target, Some(MouseButton::Left), Modifiers::default());
            cx.simulate_mouse_up(resize_target, MouseButton::Left, Modifiers::default());
            assert_eq!(
                cx.debug_bounds("inspector-header:last-value")
                    .expect("resized Last value header should remain rendered")
                    .size
                    .width,
                resized_header.size.width + px(36.)
            );
            let minimum = cx
                .debug_bounds("inspector-header:min")
                .expect("Min header should be sortable");
            cx.simulate_click(minimum.center(), Modifiers::default());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.inspector_sort)
                    .expect("viewer should remain open"),
                Some(InspectorSort {
                    column: InspectorColumn::Minimum,
                    direction: InspectorSortDirection::Ascending,
                })
            );
            let maximum = cx
                .debug_bounds("inspector-header:max")
                .expect("Max header should be sortable");
            cx.simulate_click(maximum.center(), Modifiers::default());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.inspector_sort)
                    .expect("viewer should remain open"),
                Some(InspectorSort {
                    column: InspectorColumn::Maximum,
                    direction: InspectorSortDirection::Ascending,
                })
            );
            cx.simulate_click(maximum.center(), Modifiers::default());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.inspector_sort)
                    .expect("viewer should remain open"),
                Some(InspectorSort {
                    column: InspectorColumn::Maximum,
                    direction: InspectorSortDirection::Descending,
                })
            );
            cx.simulate_click(maximum.center(), Modifiers::default());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.inspector_sort)
                    .expect("viewer should remain open"),
                None
            );

            let previous_height = window
                .read_with(&cx, |viewer, _| viewer.bottom_inspector_height)
                .expect("viewer should remain open");
            let inspector_resize = cx
                .debug_bounds("bottom-inspector-resize")
                .expect("Bottom inspector resize boundary should render");
            let resize_target = point(
                inspector_resize.center().x,
                inspector_resize.center().y + px(140.),
            );
            cx.simulate_mouse_down(
                inspector_resize.center(),
                MouseButton::Left,
                Modifiers::default(),
            );
            cx.simulate_mouse_move(resize_target, Some(MouseButton::Left), Modifiers::default());
            cx.simulate_mouse_up(resize_target, MouseButton::Left, Modifiers::default());
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.bottom_inspector_height)
                    .expect("viewer should remain open")
                    < previous_height
            );
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.bottom_inspector_height)
                    .expect("viewer should remain open")
                    < px(120.)
            );
            let retained_height = window
                .read_with(&cx, |viewer, _| viewer.bottom_inspector_height)
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, window, _| viewer.focus.focus(window))
                .expect("viewer should remain open");
            cx.dispatch_action(ToggleBottomInspector);
            assert!(
                !window
                    .read_with(&cx, |viewer, _| viewer.bottom_inspector_visible)
                    .expect("viewer should remain open")
            );
            cx.dispatch_action(ShowMetricInspector);
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.bottom_inspector_height)
                    .expect("viewer should remain open"),
                retained_height
            );
            let expanded_metric_width = cx
                .debug_bounds("metric-sidebar-row:loss")
                .expect("Metric sidebar should render")
                .size
                .width;
            cx.dispatch_action(ToggleMetricSidebar);
            let compact_metric_width = cx
                .debug_bounds("metric-sidebar-row:loss")
                .expect("compact Metric sidebar should render")
                .size
                .width;
            assert!(compact_metric_width < expanded_metric_width);
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.views.active().panels.len())
                    .expect("viewer should remain open"),
                1
            );

            let panel_id = MetricPanelId::from_string("loss");
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.bottom_inspector_visible = false;
                    cx.notify();
                })
                .expect("viewer should remain open");
            cx.run_until_parked();
            let track = cx
                .debug_bounds("metric-canvas:loss")
                .expect("Metric chart canvas should render");
            cx.simulate_mouse_down(track.center(), MouseButton::Left, Modifiers::default());
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.drag.is_some())
                    .expect("viewer should remain open")
            );
            cx.simulate_mouse_up(track.center(), MouseButton::Left, Modifiers::default());
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.drag.is_none())
                    .expect("viewer should remain open")
            );
            window
                .read_with(&cx, |viewer, _| {
                    assert!(viewer.bottom_inspector_visible);
                    assert!(viewer.locked_cursor.is_some());
                })
                .expect("viewer should remain open");

            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.bottom_inspector_visible = false;
                    viewer.drag = Some(DragGesture::Detail {
                        panel_id: panel_id.clone(),
                        origin_x: 100.,
                        last_x: 120.,
                        moved: true,
                    });
                    viewer.finish_moved_track_drag(cx);
                    viewer.drag = Some(DragGesture::BrushWindow { last_axis: 10. });
                    viewer.finish_drag(cx);
                    viewer.zoom_from_keyboard(1.25, cx);
                })
                .expect("viewer should remain open");
            assert!(
                !window
                    .read_with(&cx, |viewer, _| viewer.bottom_inspector_visible)
                    .expect("viewer should remain open")
            );
        }

        #[gpui::test]
        fn inspector_table_contains_only_visible_runs(cx: &mut TestAppContext) {
            let (root, project_id, first_run_id) = fixture_with_complete_runs(1, 4);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 1);
            let second = window
                .update(&mut cx, |viewer, _, cx| {
                    let source_id = first_source_id(viewer);
                    let additional = (1..4)
                        .map(|index| {
                            RunRef::new(
                                source_id.clone(),
                                project_id.clone(),
                                RunId::from_string(format!(
                                    "run-{index}-with-a-very-long-identifier-that-requires-horizontal-scrolling"
                                )),
                            )
                        })
                        .collect::<Vec<_>>();
                    for run in &additional {
                        viewer.toggle_run(run.clone(), cx);
                    }
                    viewer.select_metric(MetricKey::from_string("metric-0"), cx);
                    additional[0].clone()
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().panels[0]
                    .detail
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.series.len() == 4)
            });
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.bottom_inspector_height = px(120.);
                    viewer.show_metric_inspector(&MetricPanelId::from_string("metric-0"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().panels[0]
                    .inspector
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.runs.len() == 4)
            });
            let first_row = cx
                .debug_bounds("inspector-row:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling")
                .expect("first visible Run should have an inspector row");
            assert!(
                cx.debug_bounds("inspector-row:run-1-with-a-very-long-identifier-that-requires-horizontal-scrolling")
                    .is_some()
            );
            let scroll = cx
                .debug_bounds("bottom-inspector-scroll")
                .expect("inspector scroll viewport should render");
            cx.simulate_mouse_move(
                point(scroll.origin.x + px(40.), first_row.center().y),
                None,
                Modifiers::default(),
            );
            cx.run_until_parked();
            assert!(
                window
                    .read_with(&cx, |viewer, _| {
                        viewer.hovered_run.as_ref().is_some_and(|run| {
                            run.run_id.as_str()
                                == "run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling"
                        })
                    })
                    .expect("viewer should remain open")
            );
            let sticky_run = cx
                .debug_bounds("inspector-sticky-run:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling")
                .expect("Run cell should render in the fixed column");
            let sticky_x = sticky_run.origin.x;
            let sticky_y = sticky_run.origin.y;
            let row_y = first_row.origin.y;
            let header_y = cx
                .debug_bounds("inspector-table-header")
                .expect("fixed inspector header should render")
                .origin
                .y;
            let scroll_position = point(scroll.origin.x + px(300.), first_row.center().y);
            cx.simulate_event(ScrollWheelEvent {
                position: scroll_position,
                delta: ScrollDelta::Pixels(point(px(-500.), px(0.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            });
            cx.run_until_parked();
            assert!(
                window
                    .read_with(&cx, |viewer, _| {
                        viewer.inspector_horizontal_scroll.offset().x < px(0.)
                    })
                    .expect("viewer should remain open")
            );
            assert_eq!(
                cx.debug_bounds("inspector-sticky-run:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling")
                    .expect("fixed Run cell should remain rendered")
                    .origin
                    .x,
                sticky_x
            );
            cx.simulate_event(ScrollWheelEvent {
                position: scroll_position,
                delta: ScrollDelta::Pixels(point(px(0.), px(-500.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            });
            cx.run_until_parked();
            assert!(
                window
                    .read_with(&cx, |viewer, _| {
                        viewer.inspector_vertical_scroll.offset().y < px(0.)
                    })
                    .expect("viewer should remain open")
            );
            let scrolled_sticky = cx
                .debug_bounds("inspector-sticky-run:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling")
                .expect("Run cell should remain rendered after vertical scrolling");
            let scrolled_row = cx
                .debug_bounds("inspector-row:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling")
                .expect("data row should remain rendered after vertical scrolling");
            assert_eq!(
                scrolled_sticky.origin.y - sticky_y,
                scrolled_row.origin.y - row_y
            );
            assert_eq!(
                cx.debug_bounds("inspector-table-header")
                    .expect("header should remain rendered after vertical scrolling")
                    .origin
                    .y,
                header_y
            );

            window
                .update(&mut cx, |viewer, _, cx| viewer.toggle_run(second, cx))
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.active_visible_runs().len() == 3
                    && viewer.views.active().panels[0]
                        .inspector
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.runs.len() == 3)
            });

            assert!(
                cx.debug_bounds("inspector-row:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling")
                    .is_some()
            );
            window
                .read_with(&cx, |viewer, _| {
                    let visible = viewer.active_visible_runs();
                    let inspector = viewer.views.active().panels[0]
                        .inspector
                        .as_ref()
                        .expect("inspector should remain available");
                    assert!(
                        inspector
                            .runs
                            .iter()
                            .all(|run| visible.contains(&run.run_ref))
                    );
                })
                .expect("viewer should remain open");
        }

        #[gpui::test]
        fn track_scheduler_queries_and_prepares_only_visible_overscan(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(10);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            cx.simulate_resize(size(px(600.), px(420.)));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 10);
            window
                .update(&mut cx, |viewer, _, cx| {
                    for index in 0..10 {
                        viewer.select_metric(MetricKey::from_string(format!("metric-{index}")), cx);
                    }
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().panels.len() == 10
                    && viewer
                        .views
                        .active()
                        .panels
                        .iter()
                        .all(|panel| panel.overview.is_some())
            });
            wait_for_viewer(window, &cx, |viewer| {
                let schedule = viewer.track_viewport.borrow();
                !schedule.overscan.is_empty() && schedule.physical_width > 0
            });
            wait_for_viewer(window, &cx, |viewer| {
                let schedule = viewer.track_viewport.borrow();
                viewer.views.active().panels[schedule.overscan.clone()]
                    .iter()
                    .all(|panel| panel.detail.is_some())
                    && viewer.track_adapters.len() == schedule.overscan.len()
            });

            let (visible, overscan, detail_count, adapter_count) = window
                .read_with(&cx, |viewer, _| {
                    let schedule = viewer.track_viewport.borrow().clone();
                    (
                        schedule.visible,
                        schedule.overscan,
                        viewer
                            .views
                            .active()
                            .panels
                            .iter()
                            .filter(|panel| panel.detail.is_some())
                            .count(),
                        viewer.track_adapters.len(),
                    )
                })
                .expect("viewer should remain open");
            assert!(overscan.len() > visible.len());
            assert_eq!(detail_count, overscan.len());
            assert!(detail_count < 10);
            assert_eq!(adapter_count, overscan.len());
            window
                .read_with(&cx, |viewer, _| {
                    for panel in viewer
                        .views
                        .active()
                        .panels
                        .iter()
                        .filter(|panel| panel.detail.is_some())
                    {
                        let detail = panel.detail.as_ref().expect("detail should be present");
                        assert_eq!(
                            detail.point_budget,
                            panel.physical_width.saturating_mul(2).clamp(2_000, 10_000)
                        );
                    }
                })
                .expect("viewer should remain open");

            let tracks = cx
                .debug_bounds("metric-track-scroll")
                .expect("Metric track list should render");
            cx.simulate_event(ScrollWheelEvent {
                position: tracks.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-10_000.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            });
            wait_for_viewer(window, &cx, |viewer| {
                let schedule = viewer.track_viewport.borrow();
                schedule.visible.contains(&9)
                    && viewer
                        .views
                        .active()
                        .panels
                        .last()
                        .is_some_and(|panel| panel.detail.is_some())
            });
            assert!(cx.debug_bounds("metric-track:metric-9").is_some());

            let revisions_before_zoom = window
                .read_with(&cx, |viewer, _| {
                    let schedule = viewer.track_viewport.borrow();
                    viewer.views.active().panels[schedule.overscan.clone()]
                        .iter()
                        .map(|panel| (panel.panel_id.clone(), panel.detail_revision))
                        .collect::<HashMap<_, _>>()
                })
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, _, cx| {
                    let brush = viewer
                        .navigation
                        .brush_mut()
                        .expect("timeline brush should be available");
                    let anchor = brush.selected().start() + brush.selected().span() / 2.;
                    brush.zoom_at(anchor, 1.25).expect("zoom should succeed");
                    viewer.schedule_detail_refresh(cx);
                })
                .expect("viewer should remain open");
            cx.executor().advance_clock(Duration::from_millis(101));
            cx.run_until_parked();
            wait_for_viewer(window, &cx, |viewer| {
                let Some(viewport) = viewer.navigation.selected_viewport() else {
                    return false;
                };
                let schedule = viewer.track_viewport.borrow();
                viewer.views.active().panels[schedule.overscan.clone()]
                    .iter()
                    .all(|panel| {
                        panel.detail.is_some()
                            && !panel.is_pending(ReadKind::Detail)
                            && panel.requested_detail_viewport == Some(viewport)
                            && revisions_before_zoom
                                .get(&panel.panel_id)
                                .is_some_and(|revision| panel.detail_revision > *revision)
                    })
            });
        }

        #[gpui::test]
        #[ignore = "hardware-sensitive representative release workbench validation"]
        fn representative_workbench_stays_responsive_while_a_source_is_pending(
            cx: &mut TestAppContext,
        ) {
            assert!(
                std::hint::black_box(!cfg!(debug_assertions)),
                "workbench validation requires --release"
            );
            let (root, project_id, first_run_id) = fixture_with_complete_runs(6, 10);
            let (pending_root, _, _) = fixture_with_extent(10);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            cx.simulate_resize(size(px(2_560.), px(1_800.)));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 6);
            window
                .update(&mut cx, |viewer, _, cx| {
                    let source_id = first_source_id(viewer);
                    for run_index in 1..10 {
                        viewer.toggle_run(
                            RunRef::new(
                                source_id.clone(),
                                project_id.clone(),
                                RunId::from_string(format!(
                                    "run-{run_index}-with-a-very-long-identifier-that-requires-horizontal-scrolling"
                                )),
                            ),
                            cx,
                        );
                    }
                    viewer.views.active_mut().track_density = TrackDensity::Compact;
                    for metric_index in 0..6 {
                        viewer.select_metric(
                            MetricKey::from_string(format!("metric-{metric_index}")),
                            cx,
                        );
                    }
                    viewer.show_metric_inspector(
                        &MetricPanelId::from_string("metric-0"),
                        cx,
                    );
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                let schedule = viewer.track_viewport.borrow();
                viewer.views.active().runs.len() == 10
                    && viewer.views.active().panels.len() == 6
                    && schedule.overscan.len() >= 6
                    && viewer.views.active().panels.iter().all(|panel| {
                        panel.detail.as_ref().is_some_and(|detail| {
                            detail.series.len() == 10
                                && detail.point_budget
                                    == panel.physical_width.saturating_mul(2).clamp(2_000, 10_000)
                                && detail.series.iter().all(|series| {
                                    series.evidence.points.len() <= detail.point_budget as usize + 2
                                })
                        })
                    })
            });

            assert!(cx.debug_bounds("bottom-inspector").is_some());
            assert!(cx.debug_bounds("metric-track:metric-0").is_some());
            assert!(cx.debug_bounds("metric-track:metric-5").is_some());

            let before = window
                .read_with(&cx, |viewer, _| {
                    viewer.navigation.brush().map(|brush| brush.selected())
                })
                .expect("viewer should remain open")
                .expect("representative View should have a shared viewport");
            let pending_source_id = DataSourceId::from_path(pending_root.path());
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.open_source(pending_root.path().to_path_buf(), cx);
                    assert!(matches!(
                        viewer
                            .sources
                            .source(&pending_source_id)
                            .map(|source| &source.status),
                        Some(SourceStatus::Loading)
                    ));
                    viewer.zoom_from_keyboard(1.25, cx);
                })
                .expect("viewer should remain open");
            let (after, run_count, panel_count) = window
                .read_with(&cx, |viewer, _| {
                    (
                        viewer.navigation.brush().map(|brush| brush.selected()),
                        viewer.views.active().runs.len(),
                        viewer.views.active().panels.len(),
                    )
                })
                .expect("viewer should remain open");
            let after = after.expect("shared viewport should remain available");
            assert_ne!(after, before);
            assert_eq!((run_count, panel_count), (10, 6));
            assert!(cx.debug_bounds("metric-track:metric-0").is_some());
            wait_for_viewer(window, &cx, |viewer| {
                viewer
                    .sources
                    .source(&pending_source_id)
                    .is_some_and(|source| source.status == SourceStatus::Ready)
            });
        }

        #[gpui::test]
        fn zoom_debounce_commits_only_the_latest_viewport(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer
                    .views
                    .active()
                    .panels
                    .first()
                    .is_some_and(|panel| panel.detail.is_some())
            });
            window
                .update(&mut cx, |_, _, cx| cx.notify())
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            let (before_generation, before_revision, before_viewport) = window
                .read_with(&cx, |viewer, _| {
                    let panel = viewer
                        .views
                        .active()
                        .panels
                        .first()
                        .expect("metric panel should exist");
                    (
                        viewer.next_generation,
                        panel.detail_revision,
                        panel.requested_detail_viewport,
                    )
                })
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, _, cx| {
                    let brush = viewer
                        .navigation
                        .brush_mut()
                        .expect("timeline brush should exist");
                    let anchor = brush.selected().start() + brush.selected().span() / 2.;
                    brush.zoom_at(anchor, 1.25).expect("zoom should succeed");
                    viewer.schedule_detail_refresh(cx);
                    let brush = viewer
                        .navigation
                        .brush_mut()
                        .expect("timeline brush should exist");
                    let anchor = brush.selected().start() + brush.selected().span() / 2.;
                    brush.zoom_at(anchor, 1.25).expect("zoom should succeed");
                    viewer.schedule_detail_refresh(cx);
                    assert!(
                        viewer
                            .views
                            .active()
                            .panels
                            .first()
                            .is_some_and(|panel| !panel.is_pending(ReadKind::Detail))
                    );
                })
                .expect("viewer should remain open");
            let final_viewport = window
                .read_with(&cx, |viewer, _| viewer.navigation.selected_viewport())
                .expect("viewer should remain open");
            let immediate = window
                .read_with(&cx, |viewer, _| {
                    let panel = viewer
                        .views
                        .active()
                        .panels
                        .first()
                        .expect("metric panel should exist");
                    (panel.detail_revision, panel.requested_detail_viewport)
                })
                .expect("viewer should remain open");
            assert_eq!(immediate, (before_revision, before_viewport));
            assert_ne!(final_viewport, before_viewport);

            cx.executor().advance_clock(Duration::from_millis(101));
            cx.run_until_parked();
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            let after = window
                .read_with(&cx, |viewer, _| {
                    let panel = viewer
                        .views
                        .active()
                        .panels
                        .first()
                        .expect("metric panel should exist");
                    (
                        viewer.next_generation,
                        panel.detail_revision,
                        panel.requested_detail_viewport,
                    )
                })
                .expect("viewer should remain open");
            assert_eq!(after.0, before_generation + 1);
            assert!(after.1 > before_revision);
            assert_eq!(after.2, final_viewport);
        }

        #[gpui::test]
        fn keyboard_zoom_reprojects_immediately_and_debounces_detail(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, source_catalog_loaded);
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                viewer
                    .views
                    .active()
                    .panels
                    .first()
                    .is_some_and(|panel| panel.detail.is_some())
            });
            window
                .update(&mut cx, |_, _, cx| cx.notify())
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            let (before_generation, before_revision, before_viewport, before_span) = window
                .read_with(&cx, |viewer, _| {
                    let panel = viewer
                        .views
                        .active()
                        .panels
                        .first()
                        .expect("metric panel should exist");
                    (
                        viewer.next_generation,
                        panel.detail_revision,
                        panel.requested_detail_viewport,
                        viewer
                            .navigation
                            .brush()
                            .expect("timeline brush should exist")
                            .selected()
                            .span(),
                    )
                })
                .expect("viewer should remain open");

            cx.dispatch_action(ZoomIn);
            cx.dispatch_action(ZoomIn);
            let immediate = window
                .read_with(&cx, |viewer, _| {
                    let panel = viewer
                        .views
                        .active()
                        .panels
                        .first()
                        .expect("metric panel should exist");
                    (
                        viewer.next_generation,
                        panel.detail_revision,
                        panel.requested_detail_viewport,
                        viewer
                            .navigation
                            .brush()
                            .expect("timeline brush should exist")
                            .selected()
                            .span(),
                    )
                })
                .expect("viewer should remain open");
            assert_eq!(immediate.0, before_generation);
            assert_eq!(immediate.1, before_revision);
            assert_eq!(immediate.2, before_viewport);
            assert!(immediate.3 < before_span);
            let final_viewport = window
                .read_with(&cx, |viewer, _| viewer.navigation.selected_viewport())
                .expect("viewer should remain open");

            cx.executor().advance_clock(Duration::from_millis(101));
            cx.run_until_parked();
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            let after = window
                .read_with(&cx, |viewer, _| {
                    let panel = viewer
                        .views
                        .active()
                        .panels
                        .first()
                        .expect("metric panel should exist");
                    (
                        viewer.next_generation,
                        panel.detail_revision,
                        panel.requested_detail_viewport,
                    )
                })
                .expect("viewer should remain open");
            assert_eq!(after.0, before_generation + 1);
            assert!(after.1 > before_revision);
            assert_eq!(after.2, final_viewport);
        }
    }
}
