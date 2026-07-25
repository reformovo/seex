use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;
use std::{cell::RefCell, rc::Rc};

use gpui::{
    App, Application, Bounds, Context, Corner, FocusHandle, KeyBinding, KeyDownEvent,
    ListAlignment, ListState, Menu, MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathPromptOptions, Render, ScrollWheelEvent, SharedString, SystemMenuType, Task,
    Window, WindowBounds, WindowOptions, actions, anchored, deferred, div, list, point, prelude::*,
    px, relative, size,
};
use pulseon_chart_core::{BrushState, CanvasSize};
use pulseon_model::alignment::{AlignmentAxis, AlignmentViewport};
use pulseon_model::comparison::{EvidenceReason, ObjectiveDirection};
use pulseon_model::metric::MetricKey;
use pulseon_model::run::{Run, RunStatus};
use pulseon_model::types::{Project, ProjectId};
use pulseon_viewer::coordination::AnalysisViewId;
use pulseon_viewer::coordination::{
    MetricPanelId, PanelReadCoordinator, PanelReadOutcome, PanelReadRequest, PanelReadTag,
};
use pulseon_viewer::core::{
    ApplyOutcome, DataSourceId, MAX_SELECTED_RUNS, RunRef, ViewerCore, run_matches_filter,
};
use pulseon_viewer::model::{CatalogSnapshot, DiscoveryRequest};
use pulseon_viewer::query::{CurveAxis, InspectorSnapshot};
use pulseon_viewer::registry::{SourceRegistry, SourceStatus};
use pulseon_viewer::workbench::{AnalysisViews, InspectorTab, MetricPanel, ProjectRef};
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
    source_path: PathBuf,
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

#[derive(Default)]
struct RunListCache {
    runs: Rc<[Run]>,
}

impl RunListCache {
    fn rebuild(&mut self, catalog: Option<&CatalogSnapshot>, filter: &str) {
        self.runs = catalog.map_or_else(Rc::default, |catalog| {
            catalog
                .runs
                .iter()
                .filter(|run| run_matches_filter(run, filter))
                .cloned()
                .collect::<Vec<_>>()
                .into()
        });
    }
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
    metric_filter: String,
    run_list: RunListCache,
    expanded_projects: HashSet<(DataSourceId, ProjectId)>,
    removed_projects: HashSet<ProjectRef>,
    project_focuses: HashMap<ProjectRef, FocusHandle>,
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
    metric_picker_open: bool,
    axis_picker_open: bool,
    project_sidebar_visible: bool,
    project_sidebar_width: gpui::Pixels,
    metric_sidebar_compact: bool,
    bottom_inspector_visible: bool,
    bottom_inspector_height: gpui::Pixels,
    inspector_resize: Option<InspectorResize>,
    source_menu: Option<DataSourceId>,
    source_path: Option<PathBuf>,
    sources: SourceRegistry,
    event_tasks: HashMap<DataSourceId, Task<()>>,
    core: ViewerCore,
    next_generation: u64,
    local_error: Option<String>,
    chart_adapter: Rc<RefCell<ChartAdapter>>,
    track_adapters: HashMap<MetricPanelId, Rc<RefCell<ChartAdapter>>>,
    track_hovers: HashMap<MetricPanelId, HoverPoint>,
    metric_scroll: ListState,
    metric_resize: Option<MetricResize>,
    track_viewport: Rc<RefCell<TrackViewport>>,
    overview_revision: u64,
    detail_revision: u64,
    overview_width: u32,
    detail_width: u32,
    ruler_hover: Option<f64>,
    locked_cursor: Option<f64>,
    drag: Option<DragGesture>,
    zoom_task: Option<Task<()>>,
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
            metric_filter: String::new(),
            run_list: RunListCache::default(),
            expanded_projects: HashSet::new(),
            removed_projects: HashSet::new(),
            project_focuses: HashMap::new(),
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
            metric_picker_open: false,
            axis_picker_open: false,
            project_sidebar_visible: true,
            project_sidebar_width: px(320.),
            metric_sidebar_compact: false,
            bottom_inspector_visible: false,
            bottom_inspector_height: px(220.),
            inspector_resize: None,
            source_menu: None,
            source_path: None,
            sources: SourceRegistry::default(),
            event_tasks: HashMap::new(),
            core: ViewerCore::default(),
            next_generation: 1,
            local_error: None,
            chart_adapter: Rc::new(RefCell::new(ChartAdapter::default())),
            track_adapters: HashMap::new(),
            track_hovers: HashMap::new(),
            metric_scroll: ListState::new(0, ListAlignment::Top, px(480.)),
            metric_resize: None,
            track_viewport: Rc::new(RefCell::new(TrackViewport::default())),
            overview_revision: 0,
            detail_revision: 0,
            overview_width: 1_000,
            detail_width: 1_000,
            ruler_hover: None,
            locked_cursor: None,
            drag: None,
            zoom_task: None,
            detail_refresh_token: 0,
            detail_refresh_pending: false,
            workbench_path: default_workbench_path(),
            last_saved_workbench: None,
        };
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
        self.project_sidebar_width = px(document.project_sidebar_width.clamp(180., 600.));
        self.metric_sidebar_compact = document.metric_sidebar_compact;
        self.bottom_inspector_height = px(document.bottom_inspector_height.clamp(120., 600.));
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
            let track_in_core = self.core.selection().source_id.as_ref() == Some(&source_id);
            self.submit_to_source(
                source_id,
                ReadRequest::Discover(DiscoveryRequest {
                    project_id: track_in_core
                        .then(|| self.core.selection().project_id.clone())
                        .flatten(),
                    selected_run_ids: if track_in_core {
                        self.core
                            .selection()
                            .runs
                            .iter()
                            .filter(|run| {
                                self.core.selection().source_id.as_ref() == Some(&run.source_id)
                            })
                            .map(|run| run.run_id.clone())
                            .collect()
                    } else {
                        Vec::new()
                    },
                }),
                track_in_core,
                cx,
            );
        }
    }

    fn open_source(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let source_id = self.sources.import(path.clone());
        self.local_error = None;
        if self.core.selection().source_id.is_some() {
            self.submit_to_source(
                source_id,
                ReadRequest::Discover(DiscoveryRequest {
                    project_id: None,
                    selected_run_ids: Vec::new(),
                }),
                false,
                cx,
            );
            return;
        }
        self.core.reset_source(source_id);
        self.run_list = RunListCache::default();
        self.chart_adapter.borrow_mut().clear();
        self.track_adapters.clear();
        self.track_hovers.clear();
        self.metric_scroll = ListState::new(0, ListAlignment::Top, px(480.));
        *self.track_viewport.borrow_mut() = TrackViewport::default();
        self.ruler_hover = None;
        self.locked_cursor = None;
        self.drag = None;
        self.cancel_detail_refresh();
        self.source_path = Some(path);
        self.refresh_catalog(cx);
    }

    fn refresh_catalog(&mut self, cx: &mut Context<Self>) {
        let selection = self.core.selection();
        self.submit(
            ReadRequest::Discover(DiscoveryRequest {
                project_id: selection.project_id.clone(),
                selected_run_ids: selection
                    .runs
                    .iter()
                    .map(|run| run.run_id.clone())
                    .collect(),
            }),
            cx,
        );
    }

    fn submit(&mut self, request: ReadRequest, cx: &mut Context<Self>) {
        let Some(source_id) = self.core.selection().source_id.clone() else {
            return;
        };
        self.submit_to_source(source_id, request, true, cx);
    }

    fn submit_to_source(
        &mut self,
        source_id: DataSourceId,
        request: ReadRequest,
        track_in_core: bool,
        cx: &mut Context<Self>,
    ) {
        let generation = Generation(self.next_generation);
        self.next_generation = self.next_generation.saturating_add(1);
        self.submit_source_generation(source_id, generation, request, track_in_core, cx);
    }

    fn submit_source_generation(
        &mut self,
        source_id: DataSourceId,
        generation: Generation,
        request: ReadRequest,
        track_in_core: bool,
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
        match self.sources.submit(&source_id, generation, request.clone()) {
            Ok(()) if track_in_core => self.core.begin(generation, source_id, &request),
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
        let catalog_project = self.sources.apply_event(&event);
        if let (Some(project_id), Ok(pulseon_viewer::worker::ReadSnapshot::Catalog(snapshot))) =
            (catalog_project, &event.result)
        {
            let available_runs = snapshot
                .runs
                .iter()
                .filter(|run| run.project_id == project_id)
                .map(|run| run.run_id.clone())
                .collect::<Vec<_>>();
            let removed =
                self.views
                    .reconcile_source_project(&event.source_id, &project_id, &available_runs);
            for run in &removed {
                if self.core.selection().runs.contains(run) {
                    let _ = self.core.toggle_run(run.clone());
                }
            }
            if !removed.is_empty() {
                self.local_error = Some(format!(
                    "{} persisted Run selection(s) are no longer available",
                    removed.len()
                ));
            }
        }
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
                let extent = completed
                    .curves
                    .as_ref()
                    .and_then(|snapshot| snapshot.real_range);
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
                        completed.curves,
                        completed.source_errors,
                    )
                };
                if accepted && kind == ReadKind::Overview {
                    if let Some(metric_key) = metric_key
                        && let Some(home) =
                            self.views.record_active_metric_extent(metric_key, extent)
                    {
                        self.core.set_timeline_home(home);
                    }
                    if let Some(viewport) = self.core.selected_viewport()
                        && self.panel_is_scheduled(&panel_id)
                    {
                        let physical_width = self.track_viewport.borrow().physical_width.max(1);
                        if self.should_schedule_panel_detail(&panel_id, viewport, physical_width) {
                            self.request_panel_detail(&panel_id, viewport, physical_width, cx);
                        }
                    }
                }
            }
            return;
        }
        let succeeded = event.result.is_ok();
        if self.core.apply(event) != ApplyOutcome::Applied || !succeeded {
            return;
        }
        if kind == ReadKind::Catalog {
            self.run_list.rebuild(self.core.catalog(), &self.run_filter);
        }
        match kind {
            ReadKind::Catalog
                if !self.active_visible_runs().is_empty()
                    && !self.views.active().panels.is_empty() =>
            {
                self.request_overview(cx);
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
        match self.core.axis() {
            AlignmentAxis::Step => CurveAxis::Step,
            AlignmentAxis::ElapsedTime => CurveAxis::AbsoluteTime,
        }
    }

    fn request_panel_overview(&mut self, panel_id: &MetricPanelId, cx: &mut Context<Self>) {
        let runs = self.active_visible_runs();
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
            self.submit_source_generation(read.source_id, read.generation, read.request, false, cx);
        }
    }

    fn request_detail(&mut self, cx: &mut Context<Self>) {
        let Some(viewport) = self.core.selected_viewport() else {
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
        };
        let planned = match self.panel_reads.begin(
            tag,
            PanelReadRequest::Inspector {
                runs,
                metric_key,
                ranking_direction: self.views.active().ranking_direction,
            },
        ) {
            Ok(planned) => planned,
            Err(error) => {
                self.local_error = Some(error.to_string());
                return;
            }
        };
        self.views
            .begin_active_panel_read(&panel_id, ReadKind::Inspector, generation);
        for read in planned {
            self.submit_source_generation(read.source_id, read.generation, read.request, false, cx);
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
            self.submit_source_generation(read.source_id, read.generation, read.request, false, cx);
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
        self.local_error
            .as_deref()
            .or_else(|| self.core.last_error())
    }

    fn status(&self) -> SharedString {
        if self.source_path.is_none() {
            return "Import a local PulseOn source to compare Runs.".into();
        }
        if self.core.catalog().is_none() && self.core.is_pending(ReadKind::Catalog) {
            return "Loading Projects…".into();
        }
        let Some(catalog) = self.core.catalog() else {
            return "No catalog is available.".into();
        };
        if catalog.projects.is_empty() {
            "This store does not contain any Projects.".into()
        } else {
            "Select a Project to begin.".into()
        }
    }

    fn on_open(&mut self, _: &OpenProject, _: &mut Window, cx: &mut Context<Self>) {
        self.open_picker(cx);
    }

    fn on_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.local_error = None;
        self.refresh_catalog(cx);
        cx.notify();
    }

    fn on_toggle_project_sidebar(
        &mut self,
        _: &ToggleProjectSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.project_sidebar_visible = !self.project_sidebar_visible;
        self.source_menu = None;
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
        if self.views.active().selected_panel_id.is_some() {
            self.bottom_inspector_visible = !self.bottom_inspector_visible;
            if self.bottom_inspector_visible {
                self.request_inspector(cx);
            }
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
        self.renaming_view = None;
        cx.notify();
    }

    fn duplicate_analysis_view(&mut self, cx: &mut Context<Self>) {
        self.store_active_view_state();
        self.views.duplicate_active();
        self.restore_active_view_state();
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
            }
            self.renaming_view = None;
            cx.notify();
        }
    }

    fn store_active_view_state(&mut self) {
        let view_id = self.views.active().view_id.clone();
        self.panel_reads.deactivate_view(&view_id);
        self.core.cancel_pending();
        let view = self.views.active_mut();
        view.core = std::mem::take(&mut self.core);
        view.local_error = self.local_error.take();
        view.overview_revision = self.overview_revision;
        view.detail_revision = self.detail_revision;
    }

    fn restore_active_view_state(&mut self) {
        let view = self.views.active_mut();
        self.core = std::mem::take(&mut view.core);
        self.local_error = view.local_error.take();
        self.overview_revision = view.overview_revision;
        self.detail_revision = view.detail_revision;
        self.source_path = self
            .core
            .selection()
            .source_id
            .as_ref()
            .and_then(|source_id| self.sources.source(source_id))
            .map(|source| source.root_path.clone());
        self.run_list.rebuild(self.core.catalog(), &self.run_filter);
        self.chart_adapter.borrow_mut().clear();
        self.track_adapters.clear();
        self.track_hovers.clear();
        self.metric_scroll = ListState::new(
            self.views.active().panels.len(),
            ListAlignment::Top,
            px(480.),
        );
        *self.track_viewport.borrow_mut() = TrackViewport::default();
        self.ruler_hover = None;
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
        self.renaming_view = Some(view_id);
        self.view_name_focus.focus(window);
        cx.notify();
    }

    fn on_view_name_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => {
                if let Some(view_id) = self.renaming_view.take() {
                    self.views.rename(&view_id, &self.view_name_draft);
                }
            }
            "escape" => self.renaming_view = None,
            "backspace" => {
                self.view_name_draft.pop();
            }
            _ if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.chars().any(char::is_control)
                {
                    self.view_name_draft.push_str(text);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn refresh_source(&mut self, source_id: DataSourceId, cx: &mut Context<Self>) {
        let active = self.core.selection().source_id.as_ref() == Some(&source_id);
        let project_id = active
            .then(|| self.core.selection().project_id.clone())
            .flatten();
        self.submit_to_source(
            source_id,
            ReadRequest::Discover(DiscoveryRequest {
                project_id,
                selected_run_ids: if active {
                    self.core
                        .selection()
                        .runs
                        .iter()
                        .map(|run| run.run_id.clone())
                        .collect()
                } else {
                    Vec::new()
                },
            }),
            active,
            cx,
        );
        self.source_menu = None;
        cx.notify();
    }

    fn reveal_source(&mut self, source_id: &DataSourceId, cx: &mut Context<Self>) {
        let Some(source) = self.sources.source(source_id) else {
            return;
        };
        #[cfg(target_os = "macos")]
        if let Err(error) = std::process::Command::new("open")
            .arg("-R")
            .arg(&source.root_path)
            .spawn()
        {
            self.local_error = Some(format!("failed to reveal source: {error}"));
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.local_error = Some(format!(
                "Reveal is unavailable on this platform: {}",
                source.root_path.display()
            ));
        }
        self.source_menu = None;
        cx.notify();
    }

    fn remove_source(&mut self, source_id: &DataSourceId, cx: &mut Context<Self>) {
        let active = self.core.selection().source_id.as_ref() == Some(source_id);
        self.sources.remove(source_id);
        self.event_tasks.remove(source_id);
        self.expanded_projects
            .retain(|(selected_source, _)| selected_source != source_id);
        self.project_focuses
            .retain(|project, _| &project.source_id != source_id);
        self.views.remove_source(source_id);
        self.source_menu = None;
        if active {
            self.core = ViewerCore::default();
            self.source_path = None;
            self.run_list = RunListCache::default();
            self.chart_adapter.borrow_mut().clear();
        }
        cx.notify();
    }

    fn on_reset(&mut self, _: &ResetView, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel_detail_refresh();
        if self.core.reset_view() {
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
        let Some(selected) = self.core.brush().map(|brush| brush.selected()) else {
            return;
        };
        let anchor = selected.start() + selected.span() / 2.;
        if self
            .core
            .brush_mut()
            .is_none_or(|brush| brush.zoom_at(anchor, factor).is_err())
        {
            return;
        }
        self.schedule_detail_refresh(cx);
        cx.notify();
    }

    fn on_step(&mut self, _: &UseStep, _: &mut Window, cx: &mut Context<Self>) {
        self.axis_picker_open = false;
        self.views.clear_active_timeline_extents();
        self.core.select_axis(AlignmentAxis::Step);
        self.request_overview(cx);
        cx.notify();
    }

    fn on_elapsed(&mut self, _: &UseElapsed, _: &mut Window, cx: &mut Context<Self>) {
        self.axis_picker_open = false;
        self.views.clear_active_timeline_extents();
        self.core.select_axis(AlignmentAxis::ElapsedTime);
        self.request_overview(cx);
        cx.notify();
    }

    fn select_project(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        self.core.select_project(Some(project_id));
        self.run_filter.clear();
        self.run_list.rebuild(self.core.catalog(), &self.run_filter);
        self.refresh_catalog(cx);
        cx.notify();
    }

    fn activate_tree_project(
        &mut self,
        source_id: DataSourceId,
        source_path: PathBuf,
        project_id: ProjectId,
        cx: &mut Context<Self>,
    ) {
        let key = (source_id.clone(), project_id.clone());
        if !self.expanded_projects.insert(key.clone()) {
            self.expanded_projects.remove(&key);
        }
        if self.core.selection().source_id.as_ref() != Some(&source_id) {
            self.core.reset_source(source_id);
            self.source_path = Some(source_path);
            self.run_list = RunListCache::default();
            self.chart_adapter.borrow_mut().clear();
        }
        self.select_project(project_id, cx);
    }

    fn toggle_tree_run(&mut self, run_ref: RunRef, source_path: PathBuf, cx: &mut Context<Self>) {
        if self.core.selection().source_id.as_ref() != Some(&run_ref.source_id) {
            self.core.reset_source(run_ref.source_id.clone());
            self.source_path = Some(source_path);
        }
        if self.core.selection().project_id.as_ref() != Some(&run_ref.project_id) {
            self.core.select_project(Some(run_ref.project_id.clone()));
        }
        self.toggle_run(run_ref, cx);
    }

    fn toggle_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        match self.views.toggle_active_run(run.clone()) {
            Ok(selected) => {
                self.local_error = None;
                let core_selected = self.core.selection().runs.contains(&run);
                if selected != core_selected
                    && let Err(error) = self.core.toggle_run(run)
                {
                    self.local_error = Some(error.to_string());
                    cx.notify();
                    return;
                }
                self.refresh_catalog(cx);
            }
            Err(error) => self.local_error = Some(error.to_string()),
        }
        cx.notify();
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

    fn sync_core_runs(&mut self) {
        let desired = self.active_visible_runs();
        let current = self.core.selection().runs.clone();
        for run in current {
            if !desired.contains(&run) {
                let _ = self.core.toggle_run(run);
            }
        }
        for run in desired {
            if !self.core.selection().runs.contains(&run) {
                let _ = self.core.toggle_run(run);
            }
        }
    }

    fn set_run_baseline(&mut self, run: RunRef, cx: &mut Context<Self>) {
        if self.views.archived_runs().contains(&run) {
            self.views.restore_run(&run);
        }
        let baseline = (self.views.active().baseline.as_ref() != Some(&run)).then_some(run);
        if let Err(error) = self.views.set_active_baseline(baseline) {
            self.local_error = Some(error.to_string());
        } else {
            self.sync_core_runs();
            self.request_overview(cx);
            self.request_inspector(cx);
        }
        cx.notify();
    }

    fn toggle_pinned_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        if self.views.archived_runs().contains(&run) {
            self.views.restore_run(&run);
        }
        if let Err(error) = self.views.toggle_active_pinned_run(run) {
            self.local_error = Some(error.to_string());
        } else {
            self.sync_core_runs();
            self.request_overview(cx);
        }
        cx.notify();
    }

    fn archive_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        if self.views.archived_runs().contains(&run) {
            self.views.restore_run(&run);
        } else {
            self.views.archive_run(run);
        }
        self.sync_core_runs();
        self.request_overview(cx);
        self.request_inspector(cx);
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
        self.removed_projects.insert(project.clone());
        self.project_focuses.remove(&project);
        self.views.unpin_project(&project);
        self.views.restore_project(&project);
        self.project_menu = None;
        cx.notify();
    }

    fn toggle_project_runs(&mut self, project: &SidebarProject, cx: &mut Context<Self>) {
        let archived = self.views.archived_runs().to_vec();
        let movable = project
            .runs
            .iter()
            .map(|run| {
                RunRef::new(
                    project.project_ref.source_id.clone(),
                    run.project_id.clone(),
                    run.run_id.clone(),
                )
            })
            .filter(|run| !archived.contains(run))
            .collect::<Vec<_>>();
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
        self.sync_core_runs();
        self.project_menu = None;
        self.request_overview(cx);
        cx.notify();
    }

    fn select_metric(&mut self, metric_key: MetricKey, cx: &mut Context<Self>) {
        self.metric_picker_open = false;
        self.metric_filter.clear();
        let panel_id = self.views.select_active_metric(metric_key.clone());
        self.metric_scroll.reset(self.views.active().panels.len());
        self.core.select_metric(Some(metric_key));
        self.request_panel_overview(&panel_id, cx);
        if self.bottom_inspector_visible {
            self.request_inspector(cx);
        }
        cx.notify();
    }

    fn show_metric_inspector(&mut self, panel_id: &MetricPanelId, cx: &mut Context<Self>) {
        if !self.views.select_active_panel(panel_id) {
            return;
        }
        let metric_key = self
            .views
            .active_panel(panel_id)
            .map(|panel| panel.metric_key.clone());
        self.core.select_metric(metric_key);
        self.bottom_inspector_visible = true;
        self.request_inspector(cx);
        cx.notify();
    }

    fn select_inspector_tab(&mut self, tab: InspectorTab, cx: &mut Context<Self>) {
        self.views.set_active_inspector_tab(tab);
        let inspector_missing = self
            .views
            .active()
            .selected_panel_id
            .as_ref()
            .and_then(|panel_id| self.views.active_panel(panel_id))
            .is_none_or(|panel| panel.inspector.is_none());
        if inspector_missing
            || (tab == InspectorTab::Ranking && self.views.active().ranking_direction.is_some())
        {
            self.request_inspector(cx);
        }
        cx.notify();
    }

    fn select_ranking_direction(&mut self, direction: ObjectiveDirection, cx: &mut Context<Self>) {
        self.views.set_active_ranking_direction(direction);
        self.request_inspector(cx);
        cx.notify();
    }

    fn begin_inspector_resize(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.inspector_resize = Some(InspectorResize {
            start_y: event.position.y,
            start_height: self.bottom_inspector_height,
        });
        cx.notify();
    }

    fn move_inspector_resize(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(resize) = self.inspector_resize else {
            return;
        };
        self.bottom_inspector_height = (resize.start_height + resize.start_y - event.position.y)
            .max(px(120.))
            .min(px(600.));
        cx.notify();
    }

    fn finish_inspector_resize(&mut self, cx: &mut Context<Self>) {
        if self.inspector_resize.take().is_some() {
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
            self.metric_scroll.reset(self.views.active().panels.len());
            self.track_adapters.remove(panel_id);
            self.track_hovers.remove(panel_id);
            let selected_metric = self
                .views
                .active()
                .selected_panel_id
                .as_ref()
                .and_then(|selected| self.views.active_panel(selected))
                .map(|panel| panel.metric_key.clone());
            self.core.select_metric(selected_metric);
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
                self.core.set_timeline_home(home);
            } else {
                self.core.clear_timeline();
            }
            cx.notify();
        }
    }

    fn on_filter_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "backspace" => {
                self.run_filter.pop();
            }
            "escape" => self.run_filter.clear(),
            _ if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.chars().any(char::is_control)
                {
                    self.run_filter.push_str(text);
                }
            }
            _ => return,
        }
        self.run_list.rebuild(self.core.catalog(), &self.run_filter);
        cx.stop_propagation();
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

    fn render_workspace(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        self.render_metric_workspace(cx)
    }
    fn render_metric_workspace(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        self.reconcile_track_schedule(cx);
        let theme = self.theme;
        let panels: Rc<[MetricPanel]> = self.views.active().panels.clone().into();
        let selected = panels
            .iter()
            .map(|panel| panel.metric_key.clone())
            .collect::<HashSet<_>>();
        let available = self
            .core
            .catalog()
            .map(|catalog| catalog.metric_keys.clone())
            .unwrap_or_default();
        let metric_picker = self.render_metric_picker(available, &selected, cx);
        let axis_picker = self.render_axis_picker(cx);
        let metric_sidebar_width = self.metric_sidebar_width();
        let timeline = self.render_overview(cx);
        let ruler = self.render_ruler(cx);
        let list_panels = Rc::clone(&panels);
        let panel_count = panels.len();
        let scroll = self.metric_scroll.clone();
        if scroll.item_count() != panel_count {
            scroll.reset(panel_count);
        }
        let physical_width = self.overview_width.max(1);
        let logical_width = physical_width as f32;
        if self.track_viewport.borrow().overscan.is_empty() && panel_count > 0 {
            let visible = 0..panel_count.min(4);
            *self.track_viewport.borrow_mut() = TrackViewport {
                overscan: 0..panel_count.min(8),
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
                    .flex()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .id("brush-controls")
                            .debug_selector(|| "brush-controls".to_owned())
                            .w(metric_sidebar_width)
                            .flex_shrink_0()
                            .px(theme.spacing.panel_padding)
                            .py(px(6.))
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
                    .child(
                        div()
                            .flex_1()
                            .px(theme.spacing.content_padding)
                            .child(timeline),
                    ),
            )
            .child(
                div()
                    .h(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .w(metric_sidebar_width)
                            .flex_shrink_0()
                            .bg(theme.colors.panel)
                            .border_r_1()
                            .border_color(theme.colors.border),
                    )
                    .child(ruler),
            )
            .child(
                div()
                    .id("metric-track-scroll")
                    .debug_selector(|| "metric-track-scroll".to_owned())
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
                    ),
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
        let mut picker = div().relative().child(
            components::icon_button("add-metric", theme, self.metric_picker_open, false)
                .debug_selector(|| "add-metric".to_owned())
                .tooltip(components::label_tooltip("Add Metric", theme))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    let opening = !this.metric_picker_open;
                    this.metric_picker_open = opening;
                    this.axis_picker_open = false;
                    if opening {
                        this.metric_filter.clear();
                        this.metric_filter_focus.focus(window);
                    }
                    cx.notify();
                }))
                .child(components::icon(IconName::Plus, theme)),
        );
        if self.metric_picker_open {
            picker = picker.child(deferred(
                anchored()
                    .anchor(Corner::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .offset(point(px(0.), theme.spacing.control_height + px(4.)))
                    .child(
                        components::popover(theme)
                            .id("metric-picker")
                            .debug_selector(|| "metric-picker".to_owned())
                            .w(px(260.))
                            .max_h(px(320.))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Add Metric"),
                            )
                            .child(
                                div()
                                    .id("metric-filter")
                                    .debug_selector(|| "metric-filter".to_owned())
                                    .track_focus(&filter_focus)
                                    .h(theme.spacing.control_height)
                                    .px_3()
                                    .border_1()
                                    .border_color(theme.colors.border)
                                    .rounded(theme.spacing.corner_radius)
                                    .flex()
                                    .items_center()
                                    .cursor_text()
                                    .focus(|style| style.border_color(theme.colors.focus))
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
                                    .max_h(px(240.))
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
                                                    format!("metric-candidate:{}", metric.as_str())
                                                }
                                            })
                                            .h(theme.spacing.control_height)
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
                                            .child(metric.as_str().to_owned())
                                    }))
                                    .children(candidates.is_empty().then(|| {
                                        div()
                                            .h(theme.spacing.control_height)
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .text_color(theme.colors.text_muted)
                                            .child("No matching metrics")
                                    })),
                            ),
                    ),
            ));
        }
        picker
    }

    fn render_axis_picker(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let absolute = self.curve_axis() == CurveAxis::AbsoluteTime;
        let mut picker = div().relative().child(
            components::icon_button("axis-picker", theme, self.axis_picker_open, false)
                .debug_selector(|| "axis-picker".to_owned())
                .tooltip(components::label_tooltip(
                    if absolute { "Absolute time" } else { "Step" },
                    theme,
                ))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.axis_picker_open = !this.axis_picker_open;
                    this.metric_picker_open = false;
                    this.metric_filter.clear();
                    cx.notify();
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
            picker = picker.child(deferred(
                anchored()
                    .anchor(Corner::TopLeft)
                    .offset(point(px(0.), theme.spacing.control_height + px(4.)))
                    .child(
                        components::popover(theme)
                            .id("axis-menu")
                            .debug_selector(|| "axis-menu".to_owned())
                            .w(px(180.))
                            .flex()
                            .flex_col()
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
                                        this.core.select_axis(AlignmentAxis::Step);
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
                                        this.core.select_axis(AlignmentAxis::ElapsedTime);
                                        this.request_overview(cx);
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            ));
        }
        picker
    }

    fn render_ruler(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let selected = self.core.brush().map(|brush| brush.selected());
        let ticks = selected
            .map(|range| pulseon_chart_core::linear_ticks(range, 6))
            .unwrap_or_default();
        let axis = self.curve_axis();
        let hover_axis = self.ruler_hover;
        let locked_cursor = self.locked_cursor;
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
                    .size_full()
                    .px(theme.spacing.content_padding)
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.colors.text_muted)
                    .children(
                        ticks
                            .into_iter()
                            .map(move |tick| format_axis_tick(axis, tick)),
                    ),
            )
            .children(selected.map(|range| {
                div()
                    .absolute()
                    .size_full()
                    .child(
                        renderer::cursor_canvas(None, range, hover_axis, locked_cursor)
                            .absolute()
                            .size_full(),
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
                let offset = if ratio < 0.08 {
                    px(0.)
                } else if ratio > 0.92 {
                    px(-64.)
                } else {
                    px(-32.)
                };
                components::tooltip(theme)
                    .id("ruler-hover-tooltip")
                    .debug_selector(|| "ruler-hover-tooltip".to_owned())
                    .absolute()
                    .top(px(2.))
                    .left(relative(ratio))
                    .ml(offset)
                    .min_w(px(64.))
                    .flex()
                    .justify_center()
                    .py_0()
                    .child(format_cursor_coordinate(axis, value))
            }))
    }

    fn ruler_axis_at(&self, position: gpui::Point<gpui::Pixels>, window: &Window) -> Option<f64> {
        let range = self.core.brush()?.selected();
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
        let Some(before) = self.core.brush().map(|brush| brush.selected()) else {
            return;
        };
        let transformed = if event.modifiers.platform || event.modifiers.control {
            self.ruler_axis_at(event.position, window)
                .zip(self.core.brush_mut())
                .is_some_and(|(anchor, brush)| {
                    let factor = f64::from((-delta * 0.002).exp().clamp(0.5, 2.));
                    brush.zoom_at(anchor, factor).is_ok()
                })
        } else {
            let (_, width) = self.ruler_plot_geometry(window);
            let axis_delta = -f64::from(delta) * before.span() / f64::from(width);
            self.core
                .brush_mut()
                .is_some_and(|brush| brush.pan_by(axis_delta).is_ok())
        };
        if transformed
            && self
                .core
                .brush()
                .is_some_and(|brush| brush.selected() != before)
        {
            self.schedule_detail_refresh(cx);
            cx.stop_propagation();
            cx.notify();
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
            && let Some(brush) = self.core.brush_mut()
        {
            let _ = brush.pan_by(delta);
        }
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
        let active_tab = self.views.active().inspector_tab;
        let panel = self
            .views
            .active()
            .selected_panel_id
            .as_ref()
            .and_then(|panel_id| self.views.active_panel(panel_id))
            .cloned();
        let metric_name = panel.as_ref().map_or_else(
            || "No Metric selected".to_owned(),
            |panel| panel.metric_key.as_str().to_owned(),
        );
        let cursor_coordinate = self.locked_cursor.or(self.ruler_hover).map_or_else(
            || "—".to_owned(),
            |value| format_cursor_coordinate(self.curve_axis(), value),
        );
        let inspector_context = format!("Selected: {metric_name} · cursor {cursor_coordinate}");
        let snapshot = panel.as_ref().and_then(|panel| panel.inspector.as_deref());
        let baseline = self.views.active().baseline.clone();
        let body = if panel
            .as_ref()
            .is_some_and(|panel| panel.is_pending(ReadKind::Inspector))
        {
            div().child("Loading metric summaries and objective evidence…")
        } else {
            match active_tab {
                InspectorTab::Summary => {
                    let rows = snapshot
                        .into_iter()
                        .flat_map(|snapshot| {
                            snapshot.runs.iter().map(|run| {
                                let Some(stats) = &run.summary else {
                                    return vec![
                                        run.run.name.clone(),
                                        "—".to_owned(),
                                        "—".to_owned(),
                                        "—".to_owned(),
                                        "—".to_owned(),
                                        "—".to_owned(),
                                    ];
                                };
                                vec![
                                    run.run.name.clone(),
                                    stats.effective_count.to_string(),
                                    stats.last_step.value().to_string(),
                                    format!("{:.6}", stats.last_value_f64),
                                    format!("{:.6}", stats.min_value_f64),
                                    format!("{:.6}", stats.max_value_f64),
                                ]
                            })
                        })
                        .collect();
                    inspector_table(
                        &[
                            ("Run", 180., false),
                            ("Count", 90., true),
                            ("Last step", 100., true),
                            ("Last value", 150., true),
                            ("Min", 110., true),
                            ("Max", 110., true),
                        ],
                        rows,
                        theme,
                    )
                }
                InspectorTab::Ranking => {
                    let direction = self.views.active().ranking_direction;
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .child(
                                    components::toolbar_button(
                                        "ranking-minimize",
                                        theme,
                                        direction == Some(ObjectiveDirection::Minimize),
                                        false,
                                    )
                                    .debug_selector(|| "ranking-minimize".to_owned())
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.select_ranking_direction(
                                            ObjectiveDirection::Minimize,
                                            cx,
                                        );
                                    }))
                                    .child("Minimize"),
                                )
                                .debug_selector(|| "axis-time".to_owned())
                                .child(
                                    components::toolbar_button(
                                        "ranking-maximize",
                                        theme,
                                        direction == Some(ObjectiveDirection::Maximize),
                                        false,
                                    )
                                    .debug_selector(|| "ranking-maximize".to_owned())
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.select_ranking_direction(
                                            ObjectiveDirection::Maximize,
                                            cx,
                                        );
                                    }))
                                    .child("Maximize"),
                                ),
                        )
                        .child(inspector_table(
                            &[
                                ("Rank", 60., true),
                                ("Run", 180., false),
                                ("Role", 90., false),
                                ("Status", 90., false),
                                ("Objective", 120., true),
                                ("Δ vs baseline", 130., true),
                                ("Evidence", 140., false),
                                ("Project · Source", 240., false),
                            ],
                            direction
                                .zip(snapshot)
                                .map_or_else(Vec::new, |(_, snapshot)| {
                                    ranking_table_rows(snapshot, baseline.as_ref())
                                }),
                            theme,
                        ))
                }
                InspectorTab::Evidence => {
                    let rows = snapshot
                        .into_iter()
                        .flat_map(|snapshot| {
                            snapshot.runs.iter().map(|run| {
                                vec![
                                    run.run.name.clone(),
                                    run_status(run.evidence.run_status).to_owned(),
                                    run.evidence.last_step.map_or_else(
                                        || "—".to_owned(),
                                        |step| step.value().to_string(),
                                    ),
                                    run.evidence.last_value_f64.map_or_else(
                                        || "—".to_owned(),
                                        |value| format!("{value:.6}"),
                                    ),
                                    format!(
                                        "{:?}{}",
                                        run.evidence.completeness,
                                        reasons_label(&run.evidence.reasons)
                                    ),
                                    format!(
                                        "{} · {}",
                                        run.run_ref.project_id.as_str(),
                                        run.run_ref.source_id
                                    ),
                                ]
                            })
                        })
                        .collect();
                    inspector_table(
                        &[
                            ("Run", 180., false),
                            ("Status", 90., false),
                            ("Last step", 100., true),
                            ("Last value", 150., true),
                            ("Completeness", 180., false),
                            ("Project · Source", 260., false),
                        ],
                        rows,
                        theme,
                    )
                }
            }
        };

        div()
            .id("bottom-inspector")
            .debug_selector(|| "bottom-inspector".to_owned())
            .h(self.bottom_inspector_height)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(theme.colors.surface)
            .border_t_1()
            .border_color(theme.colors.border)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.dragging() {
                    this.move_inspector_resize(event, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.finish_inspector_resize(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.finish_inspector_resize(cx);
                }),
            )
            .child(
                div()
                    .id("bottom-inspector-resize")
                    .debug_selector(|| "bottom-inspector-resize".to_owned())
                    .h(px(5.))
                    .flex_shrink_0()
                    .cursor(gpui::CursorStyle::ResizeUpDown)
                    .hover(|style| style.bg(theme.colors.focus))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.begin_inspector_resize(event, cx);
                        }),
                    ),
            )
            .child(
                div()
                    .id("bottom-inspector-header")
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(theme.spacing.tab_height)
                    .flex_shrink_0()
                    .overflow_hidden()
                    .px(theme.spacing.panel_padding)
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        components::toolbar_button(
                            "inspector-summary",
                            theme,
                            active_tab == InspectorTab::Summary,
                            false,
                        )
                        .debug_selector(|| "inspector-summary".to_owned())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.select_inspector_tab(InspectorTab::Summary, cx);
                        }))
                        .child("Summary"),
                    )
                    .child(
                        components::toolbar_button(
                            "inspector-ranking",
                            theme,
                            active_tab == InspectorTab::Ranking,
                            false,
                        )
                        .debug_selector(|| "inspector-ranking".to_owned())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.select_inspector_tab(InspectorTab::Ranking, cx);
                        }))
                        .child("Ranking"),
                    )
                    .child(
                        components::toolbar_button(
                            "inspector-evidence",
                            theme,
                            active_tab == InspectorTab::Evidence,
                            false,
                        )
                        .debug_selector(|| "inspector-evidence".to_owned())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.select_inspector_tab(InspectorTab::Evidence, cx);
                        }))
                        .child("Evidence"),
                    )
                    .child(
                        div()
                            .id("inspector-context")
                            .debug_selector(|| "inspector-context".to_owned())
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_right()
                            .text_xs()
                            .text_color(theme.colors.text_muted)
                            .child(inspector_context),
                    )
                    .child(
                        components::icon_button("close-inspector", theme, false, false)
                            .debug_selector(|| "close-inspector".to_owned())
                            .tooltip(components::label_tooltip("Hide inspector", theme))
                            .flex_none()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.bottom_inspector_visible = false;
                                cx.stop_propagation();
                                cx.notify();
                            }))
                            .child(components::icon(IconName::Close, theme)),
                    ),
            )
            .child(
                body.id("bottom-inspector-scroll")
                    .debug_selector(|| "bottom-inspector-scroll".to_owned())
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_x_scroll()
                    .overflow_y_scroll()
                    .whitespace_nowrap()
                    .p(theme.spacing.content_padding)
                    .text_sm()
                    .text_color(theme.colors.text_muted),
            )
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
        let Some(detail_viewport) = self.core.selected_viewport() else {
            return;
        };
        let panel_count = self.views.active().panels.len();
        let scheduled = state.overscan.start.min(panel_count)..state.overscan.end.min(panel_count);
        let panels = self.views.active().panels[scheduled].to_vec();
        let scheduled_ids = panels
            .iter()
            .map(|panel| panel.panel_id.clone())
            .collect::<HashSet<_>>();
        self.track_adapters
            .retain(|panel_id, _| scheduled_ids.contains(panel_id));
        self.track_hovers
            .retain(|panel_id, _| scheduled_ids.contains(panel_id));
        let logical_width = f32::from_bits(state.logical_width_bits) as f64;
        for panel in panels {
            let canvas_height =
                f64::from(panel.row_height) - f64::from(self.theme.spacing.content_padding * 2.);
            let canvas = CanvasSize::new(logical_width, canvas_height.max(1.)).ok();
            if let Some(snapshot) = panel.detail.as_deref()
                && let Some(viewport) = renderer::detail_viewport(
                    snapshot,
                    self.core.brush().map(|brush| brush.selected()),
                )
                && let Some(canvas) = canvas
            {
                self.track_adapters
                    .entry(panel.panel_id.clone())
                    .or_insert_with(|| Rc::new(RefCell::new(ChartAdapter::default())))
                    .borrow_mut()
                    .warm_projection(snapshot, panel.detail_revision, viewport, canvas);
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
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.dragging() && this.metric_resize.is_some() {
                    this.move_metric_resize(event, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.finish_metric_resize(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.finish_metric_resize(cx);
                }),
            )
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
                    .p(theme.spacing.panel_padding)
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
                            .gap_1()
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
                        components::icon_button(
                            SharedString::from(format!("remove-metric:{}", panel_id.as_str())),
                            theme,
                            false,
                            false,
                        )
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
                    .bg(theme.colors.brush_selection)
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
                div()
                    .id(SharedString::from(format!(
                        "metric-resize:{}",
                        resize_id.as_str()
                    )))
                    .debug_selector({
                        let resize_id = resize_id.clone();
                        move || format!("metric-resize:{}", resize_id.as_str())
                    })
                    .absolute()
                    .bottom(px(-2.))
                    .left_0()
                    .w_full()
                    .h(px(5.))
                    .cursor(gpui::CursorStyle::ResizeUpDown)
                    .hover(|style| style.bg(theme.colors.focus))
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
        let Some(snapshot) = panel.detail.clone() else {
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
        };
        let selected = self.core.brush().map(|brush| brush.selected());
        let Some(viewport) = renderer::detail_viewport(&snapshot, selected) else {
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
        let hover_axis = self
            .track_hovers
            .get(&panel_id)
            .map(|hover| hover.axis_value as f64)
            .or(self.ruler_hover);
        let locked_cursor = self.locked_cursor;
        let baseline = self.views.active().baseline.clone();
        let hover = self.track_hovers.get(&panel_id).cloned();
        let mut callouts = hover.map_or_else(
            || {
                self.ruler_hover.map_or_else(Vec::new, |axis| {
                    adapter.borrow().points_at_axis(&snapshot, viewport, axis)
                })
            },
            |hover| vec![hover],
        );
        adapter.borrow().spread_callouts(&mut callouts);
        let callouts = callouts
            .into_iter()
            .map(|hover| {
                let delta = baseline
                    .as_ref()
                    .and_then(|baseline| baseline_delta(panel, baseline, &hover));
                (hover, delta)
            })
            .collect::<Vec<_>>();
        let callout_panel = panel_id.clone();
        let callout_axis = self.curve_axis();
        div()
            .relative()
            .size_full()
            .p(theme.spacing.content_padding)
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
                    .child(
                        renderer::detail_canvas(
                            paint_adapter,
                            snapshot,
                            panel.detail_revision,
                            viewport,
                            baseline.clone(),
                        )
                        .size_full(),
                    )
                    .child(
                        renderer::cursor_canvas(None, viewport.x, hover_axis, locked_cursor)
                            .absolute()
                            .size_full(),
                    )
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
                            cx.notify();
                        }
                    })),
            )
            .children(callouts.into_iter().map(move |(hover, delta)| {
                let x = theme.spacing.content_padding + hover.canvas_position.x;
                let y = theme.spacing.content_padding + hover.canvas_position.y;
                let left = if hover.align_left {
                    (x - px(112.)).max(px(0.))
                } else {
                    x + px(8.)
                };
                let description = hover_value_description(callout_axis, &hover, delta);
                components::tooltip(theme)
                    .id(SharedString::from(format!(
                        "track-hover-callout:{}:{}",
                        callout_panel.as_str(),
                        hover.run_ref.cache_key()
                    )))
                    .debug_selector(|| "track-hover-callout".to_owned())
                    .tooltip(components::label_tooltip(description, theme))
                    .absolute()
                    .left(left)
                    .top((y - px(12.)).max(px(0.)))
                    .w(px(104.))
                    .px_2()
                    .py_1()
                    .whitespace_nowrap()
                    .child(
                        renderer::callout_pointer(hover.align_left)
                            .absolute()
                            .top(px(6.))
                            .when(hover.align_left, |pointer| pointer.right(px(-8.)))
                            .when(!hover.align_left, |pointer| pointer.left(px(-8.))),
                    )
                    .child(hover_value_label(&hover, delta))
            }))
    }

    fn render_overview(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let Some(brush) = self.core.brush() else {
            return div().h(px(40.));
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
        div().h(px(40.)).child(
            div()
                .id("overview-chart")
                .debug_selector(|| "overview-chart".to_owned())
                .focusable()
                .h_full()
                .w_full()
                .relative()
                .cursor_pointer()
                .border_1()
                .border_color(theme.colors.border)
                .bg(theme.colors.surface)
                .child(renderer::timeline_canvas(adapter, brush, snapshot, revision).size_full())
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
        let Some(brush) = self.core.brush() else {
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
        let Some(brush) = self.core.brush() else {
            return;
        };
        let Some(axis) = self
            .chart_adapter
            .borrow()
            .overview_axis_at(brush, event.position)
        else {
            return;
        };
        if let Some(brush) = self.core.brush_mut() {
            update_brush_drag(brush, &mut gesture, axis);
        }
        self.drag = Some(gesture);
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
        let Some(range) = self.core.brush().map(|brush| brush.selected()) else {
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
            && let Some(brush) = self.core.brush_mut()
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
            let range = self.core.brush().map(|brush| brush.selected());
            self.locked_cursor = range.and_then(|range| {
                self.track_adapters
                    .get(panel_id)
                    .and_then(|adapter| adapter.borrow().detail_axis_at(range, position))
            });
            cx.notify();
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
        let Some(range) = self.core.brush().map(|brush| brush.selected()) else {
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
            .core
            .brush_mut()
            .is_none_or(|brush| brush.zoom_at(anchor, factor).is_err())
        {
            return;
        }
        self.schedule_detail_refresh(cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn update_track_hover(
        &mut self,
        panel_id: &MetricPanelId,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.views.active_panel(panel_id) else {
            return;
        };
        let Some(snapshot) = panel.detail.as_ref() else {
            return;
        };
        let Some(viewport) =
            renderer::detail_viewport(snapshot, self.core.brush().map(|brush| brush.selected()))
        else {
            return;
        };
        let hover = self.track_adapters.get(panel_id).and_then(|adapter| {
            adapter
                .borrow()
                .hit_test(snapshot, viewport, event.position)
        });
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
                let core = if index == active_index {
                    &self.core
                } else {
                    &view.core
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
                    inspector_tab: view.inspector_tab,
                    ranking_direction: view.ranking_direction,
                    axis: core.axis(),
                    track_density: view.track_density,
                    viewport: core.brush().map(|brush| brush.selected()),
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
        let (overview, detail) = self.chart_adapter.borrow().physical_widths(scale_factor);
        let overview_changed = overview.is_some_and(|width| width != self.overview_width);
        let detail_changed = detail.is_some_and(|width| width != self.detail_width);
        if let Some(width) = overview {
            self.overview_width = width;
        }
        if let Some(width) = detail {
            self.detail_width = width;
        }
        if overview_changed {
            self.request_overview(cx);
        }
        if detail_changed {
            self.request_detail(cx);
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
        let has_catalog = self
            .core
            .catalog()
            .is_some_and(|catalog| !catalog.projects.is_empty());

        div()
            .track_focus(&self.focus)
            .tab_group()
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
                            .child(if has_catalog {
                                self.render_workspace(cx)
                            } else {
                                components::empty_state(theme)
                                    .child(
                                        components::status_badge(theme, StatusTone::Info)
                                            .child(self.status()),
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
    div()
        .id(id)
        .h(theme.spacing.control_height)
        .px_2()
        .gap_2()
        .rounded(theme.spacing.corner_radius)
        .flex()
        .items_center()
        .cursor_pointer()
        .when(selected, |item| item.bg(theme.colors.element_active))
        .when(!selected, |item| {
            item.hover(|style| style.bg(theme.colors.element_hover))
        })
        .child(
            div()
                .w(px(20.))
                .flex()
                .justify_center()
                .child(components::icon(icon, theme)),
        )
        .child(label.to_owned())
}

fn inspector_table(
    columns: &[(&str, f32, bool)],
    rows: Vec<Vec<String>>,
    theme: ViewerTheme,
) -> gpui::Div {
    let width = columns.iter().map(|(_, width, _)| width).sum::<f32>();
    div().child(
        div()
            .id("inspector-table")
            .debug_selector(|| "inspector-table".to_owned())
            .min_w(px(width))
            .flex()
            .flex_col()
            .child(inspector_table_row(
                columns,
                columns
                    .iter()
                    .map(|(label, _, _)| (*label).to_owned())
                    .collect(),
                theme,
                true,
            ))
            .children(
                rows.into_iter()
                    .map(|row| inspector_table_row(columns, row, theme, false)),
            ),
    )
}

fn inspector_table_row(
    columns: &[(&str, f32, bool)],
    cells: Vec<String>,
    theme: ViewerTheme,
    header: bool,
) -> gpui::Div {
    div()
        .h(theme.spacing.control_height)
        .flex_none()
        .flex()
        .items_center()
        .when(header, |row| {
            row.border_b_1()
                .border_color(theme.colors.border)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.colors.text)
        })
        .children(
            columns
                .iter()
                .zip(cells)
                .map(|((_, width, right_aligned), value)| {
                    div()
                        .w(px(*width))
                        .flex_none()
                        .px_2()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .flex()
                        .items_center()
                        .when(*right_aligned, |cell| cell.justify_end())
                        .child(value)
                }),
        )
}

fn ranking_table_rows(snapshot: &InspectorSnapshot, baseline: Option<&RunRef>) -> Vec<Vec<String>> {
    let baseline_value = baseline_evidence_value(Some(snapshot), baseline);
    let mut runs = snapshot.runs.iter().collect::<Vec<_>>();
    runs.sort_by(|left, right| {
        left.run_ref
            .source_id
            .as_str()
            .cmp(right.run_ref.source_id.as_str())
            .then_with(|| {
                left.run_ref
                    .project_id
                    .as_str()
                    .cmp(right.run_ref.project_id.as_str())
            })
            .then_with(|| {
                left.ranking
                    .map(|ranking| ranking.order)
                    .unwrap_or(usize::MAX)
                    .cmp(
                        &right
                            .ranking
                            .map(|ranking| ranking.order)
                            .unwrap_or(usize::MAX),
                    )
            })
    });
    runs.into_iter()
        .map(|run| {
            let value = run.evidence.last_value_f64;
            vec![
                run.ranking
                    .and_then(|ranking| ranking.rank)
                    .map_or_else(|| "—".to_owned(), |rank| format!("#{rank}")),
                run.run.name.clone(),
                if baseline == Some(&run.run_ref) {
                    "Baseline".to_owned()
                } else {
                    "Candidate".to_owned()
                },
                run_status(run.evidence.run_status).to_owned(),
                value.map_or_else(|| "—".to_owned(), |value| format!("{value:.6}")),
                inspector_delta(value, &run.run_ref, baseline, baseline_value),
                format!(
                    "{:?}{}",
                    run.evidence.completeness,
                    reasons_label(&run.evidence.reasons)
                ),
                format!(
                    "{} · {}",
                    run.run_ref.project_id.as_str(),
                    run.run_ref.source_id
                ),
            ]
        })
        .collect()
}

fn baseline_evidence_value(
    snapshot: Option<&InspectorSnapshot>,
    baseline: Option<&RunRef>,
) -> Option<f64> {
    let baseline = baseline?;
    snapshot?
        .runs
        .iter()
        .find(|run| &run.run_ref == baseline)?
        .evidence
        .last_value_f64
}

fn inspector_delta(
    value: Option<f64>,
    run_ref: &RunRef,
    baseline: Option<&RunRef>,
    baseline_value: Option<f64>,
) -> String {
    if baseline == Some(run_ref) {
        return "—".to_owned();
    }
    let Some(baseline) = baseline else {
        return "—".to_owned();
    };
    if run_ref.source_id != baseline.source_id || run_ref.project_id != baseline.project_id {
        return "—".to_owned();
    }
    value.zip(baseline_value).map_or_else(
        || "—".to_owned(),
        |(value, baseline)| format_signed_delta(value - baseline, 6),
    )
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

fn format_cursor_coordinate(axis: CurveAxis, value: f64) -> String {
    match axis {
        CurveAxis::Step if value.abs() >= 1_000_000. => format!("{:.1}M", value / 1_000_000.),
        CurveAxis::Step if value.abs() >= 1_000. => format!("{:.0}k", value / 1_000.),
        CurveAxis::Step => format_tick(value),
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

fn hover_value_label(hover: &HoverPoint, delta: Option<f64>) -> String {
    delta.map_or_else(
        || format!("{:.2}", hover.value),
        |delta| format!("{:.2}({})", hover.value, format_signed_delta(delta, 2)),
    )
}

fn hover_value_description(axis: CurveAxis, hover: &HoverPoint, delta: Option<f64>) -> String {
    let coordinate = match axis {
        CurveAxis::Step => format!("Step {}", hover.axis_value),
        CurveAxis::AbsoluteTime => format!(
            "Time {} UTC",
            format_axis_tick(CurveAxis::AbsoluteTime, hover.axis_value as f64)
        ),
    };
    let delta = delta.map_or_else(String::new, |delta| {
        format!("; {} from baseline", format_signed_delta(delta, 2))
    });
    format!(
        "{} ({}) · {} · {coordinate} · {:.2}{delta}",
        hover.run_name,
        hover.run_ref.run_id.as_str(),
        hover.metric_key,
        hover.value,
    )
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
    use std::sync::Arc;

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
            run_name: "run".to_owned(),
            metric_key: "loss".to_owned(),
            axis_value: 2_904,
            value: 0.506,
            canvas_position: point(px(10.), px(20.)),
            align_left: false,
        };

        assert_eq!(hover_value_label(&hover, Some(0.55)), "0.51(+0.55)");
        assert_eq!(hover_value_label(&hover, Some(-0.55)), "0.51(−0.55)");
        assert_eq!(
            hover_value_description(CurveAxis::Step, &hover, Some(-0.55)),
            "run (run) · loss · Step 2904 · 0.51; −0.55 from baseline"
        );
        assert_eq!(format_cursor_coordinate(CurveAxis::Step, 496_000.), "496k");
        assert_eq!(
            format_cursor_coordinate(CurveAxis::AbsoluteTime, 34_920_000.),
            "09:42"
        );
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
    fn rankings_restart_within_each_project() {
        let timestamp = "2026-01-01T00:00:00Z"
            .parse()
            .expect("fixed timestamp should parse");
        let make_run = |project: &str, run_id: &str, value: f64| {
            let project_id = ProjectId::from_string(project);
            let run_id = RunId::from_string(run_id);
            pulseon_viewer::query::InspectorRunSnapshot {
                run_ref: RunRef::new(
                    DataSourceId::from_string("source"),
                    project_id.clone(),
                    run_id.clone(),
                ),
                run: Run {
                    run_id: run_id.clone(),
                    project_id,
                    name: format!("Run {value}"),
                    status: RunStatus::Finished,
                    created_at: timestamp,
                    started_at: timestamp,
                    finished_at: Some(timestamp),
                },
                summary: Some(pulseon_model::metric::MetricAggregate {
                    run_id: run_id.clone(),
                    metric_key: MetricKey::from_string("loss"),
                    effective_count: 1,
                    last_step: pulseon_model::metric::Step::new(1),
                    last_value_f64: value,
                    min_value_f64: value,
                    max_value_f64: value,
                }),
                evidence: pulseon_model::comparison::ObjectiveEvidence {
                    run_id,
                    run_status: RunStatus::Finished,
                    last_step: Some(pulseon_model::metric::Step::new(1)),
                    last_value_f64: Some(value),
                    completeness: EvidenceCompleteness::Complete,
                    reasons: Vec::new(),
                },
                ranking: Some(pulseon_viewer::query::InspectorRanking {
                    rank: Some(if project == "alpha" && value == 2. {
                        2
                    } else {
                        1
                    }),
                    order: usize::from(project == "alpha" && value == 2.),
                }),
            }
        };
        let snapshot = InspectorSnapshot {
            ranking_direction: Some(ObjectiveDirection::Minimize),
            runs: vec![
                make_run("alpha", "a-slow", 2.),
                make_run("alpha", "a-fast", 1.),
                make_run("beta", "b-only", 3.),
            ],
        };

        let rows = ranking_table_rows(&snapshot, None);
        assert_eq!(
            rows.iter()
                .map(|row| [row[0].as_str(), row[1].as_str()])
                .collect::<Vec<_>>(),
            [["#1", "Run 1"], ["#2", "Run 2"], ["#1", "Run 3"],]
        );

        let baseline = snapshot.runs[0].run_ref.clone();
        let rows = ranking_table_rows(&snapshot, Some(&baseline));
        assert_eq!(
            rows.iter()
                .map(|row| [
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_str(),
                    row[5].as_str()
                ])
                .collect::<Vec<_>>(),
            [
                ["#1", "Run 1", "Candidate", "−1.000000"],
                ["#2", "Run 2", "Baseline", "—"],
                ["#1", "Run 3", "Candidate", "—"],
            ]
        );
    }

    #[cfg(feature = "test-support")]
    mod gpui_tests {
        use gpui::{
            Keystroke, Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext,
            WindowHandle, point,
        };
        use pulseon_core::engine::client::NativeClient;
        use pulseon_viewer::workbench::TrackDensity;

        use super::*;

        fn fixture(metric_count: usize) -> (tempfile::TempDir, ProjectId, RunId) {
            fixture_with_runs(metric_count, 1)
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
                    inspector_tab: InspectorTab::Evidence,
                    ranking_direction: Some(ObjectiveDirection::Minimize),
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
            let Some(viewport) = viewer.core.selected_viewport() else {
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

        fn select_fixture_run(
            window: WindowHandle<ViewerApp>,
            cx: &mut VisualTestContext,
            project_id: ProjectId,
            run_id: RunId,
            metric_count: usize,
        ) {
            window
                .update(cx, |viewer, _, cx| {
                    viewer.select_project(project_id.clone(), cx)
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, cx, |viewer| {
                viewer
                    .core
                    .catalog()
                    .is_some_and(|catalog| !catalog.runs.is_empty())
            });
            window
                .update(cx, |viewer, _, cx| {
                    let source_id = viewer
                        .core
                        .selection()
                        .source_id
                        .clone()
                        .expect("fixture source should be selected");
                    viewer.toggle_run(RunRef::new(source_id, project_id, run_id), cx)
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, cx, |viewer| {
                viewer
                    .core
                    .catalog()
                    .is_some_and(|catalog| catalog.metric_keys.len() == metric_count)
            });
        }

        #[gpui::test]
        fn view_actions_dispatch_through_the_focused_root(cx: &mut TestAppContext) {
            let (window, mut cx) = open_viewer(cx, None);

            cx.dispatch_action(UseElapsed);

            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.core.axis())
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
            assert_eq!(loaded.archived_runs.len(), 1);
            assert!(loaded.views[0].baseline.is_some());
            assert_eq!(loaded.views[0].pinned_runs.len(), 1);
        }

        #[gpui::test]
        fn restored_state_reconciles_removed_runs_and_unknown_metrics(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_extent(100);
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
            assert!(cx.debug_bounds("show-project-sidebar").is_some());
            let analysis_after = cx
                .debug_bounds("analysis-tab")
                .expect("Analysis workspace should remain rendered");
            assert!(analysis_after.origin.x < analysis_before.origin.x);
        }

        #[gpui::test]
        fn application_shell_preserves_pinned_geometry_at_representative_sizes(
            cx: &mut TestAppContext,
        ) {
            let (root, _, _) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());

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
                let tab = cx
                    .debug_bounds("analysis-tab")
                    .expect("active Analysis tab should render");
                let control = cx
                    .debug_bounds("new-view")
                    .expect("View toolbar control should render");

                assert_eq!(sidebar.origin.y, px(0.));
                assert_eq!(sidebar.size.height, px(window_size.1));
                assert_eq!(analysis.origin.x, sidebar.origin.x + sidebar.size.width);
                assert_eq!(tab_bar.origin.x, analysis.origin.x);
                assert_eq!(tab_bar.size.width, analysis.size.width);
                assert_eq!(tab_bar.size.height, px(32.));
                assert_eq!(tab.size.height, px(31.));
                assert_eq!(control.size.height, px(28.));
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
            let project_menu = cx
                .debug_bounds("project-menu-project")
                .expect("Project menu control should render while hovered");
            cx.simulate_click(project_menu.center(), Modifiers::default());
            let popover = cx
                .debug_bounds("project-popover-project")
                .expect("Project menu should open");
            assert!(popover.origin.x >= project_menu.origin.x);
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
                    viewer.core.select_axis(AlignmentAxis::ElapsedTime);
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
                    .read_with(&cx, |viewer, _| viewer.core.axis())
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
            assert!(cx.debug_bounds("view-menu").is_some());
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

            window
                .update(&mut cx, |viewer, window, cx| {
                    let view_id = viewer.views.active().view_id.clone();
                    viewer.begin_rename_analysis_view(view_id, window, cx);
                    viewer.on_view_name_key(
                        &KeyDownEvent {
                            keystroke: Keystroke {
                                key: "x".to_owned(),
                                key_char: Some("x".to_owned()),
                                ..Keystroke::default()
                            },
                            is_held: false,
                        },
                        window,
                        cx,
                    );
                    viewer.on_view_name_key(
                        &KeyDownEvent {
                            keystroke: Keystroke {
                                key: "enter".to_owned(),
                                ..Keystroke::default()
                            },
                            is_held: false,
                        },
                        window,
                        cx,
                    );
                })
                .expect("viewer should remain open");
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.views.active().name.ends_with('x'))
                    .expect("viewer should remain open")
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
                    .read_with(&cx, |viewer, _| viewer.core.axis())
                    .expect("viewer should remain open"),
                AlignmentAxis::ElapsedTime
            );
        }

        #[gpui::test]
        fn worker_events_update_the_entity_without_render_polling(cx: &mut TestAppContext) {
            let (root, _, _) = fixture(1);
            cx.executor().allow_parking();
            let (window, cx) = open_viewer(cx, Some(root.path().to_path_buf()));

            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
        }

        #[gpui::test]
        fn project_tree_scrolls_to_runs_in_an_expanded_project(cx: &mut TestAppContext) {
            let (root, project_id, _) = fixture_with_runs(0, 12);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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

            cx.simulate_mouse_move(first_row.center(), None, Modifiers::default());

            assert!(cx.debug_bounds("run-status-0").is_none());
            assert!(cx.debug_bounds("run-actions-0").is_some());
            assert_eq!(
                cx.debug_bounds("run-eye-0")
                    .expect("Run visibility control should keep its width")
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
        fn focused_project_activates_from_the_keyboard(cx: &mut TestAppContext) {
            let (root, _, _) = fixture(1);
            cx.executor().allow_parking();
            cx.update(|cx| {
                cx.bind_keys([KeyBinding::new(
                    "enter",
                    ActivateSelection,
                    Some(SELECTABLE_CONTEXT),
                )]);
            });
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
            let project = cx
                .debug_bounds("project-tree-row-0-0")
                .expect("first Project row should be rendered");
            cx.simulate_click(project.center(), Modifiers::default());
            cx.run_until_parked();
            assert!(cx.debug_bounds("project-information-project").is_some());
            assert!(cx.debug_bounds("project-menu-project").is_some());
            let before = window
                .read_with(&cx, |viewer, _| viewer.next_generation)
                .expect("viewer should remain open");

            cx.simulate_keystrokes("enter");

            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.next_generation)
                    .expect("viewer should remain open")
                    > before
            );
        }

        #[gpui::test]
        fn project_sidebar_retains_multiple_imported_sources(cx: &mut TestAppContext) {
            let (first, _, _) = fixture(0);
            let (second, _, _) = fixture(0);
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
        }

        #[gpui::test]
        fn shared_timeline_unions_extents_from_multiple_sources(cx: &mut TestAppContext) {
            let (first, first_project, first_run) = fixture_with_extent(10);
            let (second, second_project, second_run) = fixture_with_extent(20);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(first.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
                        viewer.activate_tree_project(
                            source_id.clone(),
                            root.to_path_buf(),
                            project_id.clone(),
                            cx,
                        );
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
                            root.to_path_buf(),
                            cx,
                        );
                    })
                    .expect("viewer should remain open");
            }
            wait_for_viewer(window, &cx, |viewer| {
                viewer
                    .core
                    .catalog()
                    .is_some_and(|catalog| catalog.metric_keys.len() == 1)
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
                        viewer.core.brush().map(|brush| brush.home().end())
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
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
                .read_with(&cx, |viewer, _| viewer.core.brush())
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
                        assert_eq!(viewer.core.brush(), Some(brush));
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
                let overview = cx
                    .debug_bounds("overview-chart")
                    .expect("Overview chart should render");
                assert_eq!(controls.size.height, px(40.));
                assert_eq!(overview.size.height, px(40.));
                assert_eq!(controls.origin.y, overview.origin.y);
                assert_eq!(controls.bottom(), overview.bottom());
            }
        }

        #[gpui::test]
        fn empty_view_keeps_the_converged_shell_and_opens_metric_picker(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(20);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
        fn metric_picker_lists_only_metrics_not_already_in_the_view(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(20);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
            let candidates = cx
                .debug_bounds("metric-candidates")
                .expect("Metric candidates should use their own scroll region");
            let late_before = cx
                .debug_bounds("metric-candidate:metric-19")
                .expect("late Metric candidate should be laid out");
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
            let padding = window
                .read_with(&cx, |viewer, _| viewer.theme.spacing.panel_padding)
                .expect("viewer should remain open");
            assert_eq!(axis.origin.x, controls.origin.x + padding);
            assert!(f32::from(add.right() - (controls.right() - padding)).abs() <= 1.);
            cx.simulate_click(axis.center(), Modifiers::default());
            assert!(cx.debug_bounds("axis-menu").is_some());
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
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
                viewer.core.axis() == AlignmentAxis::ElapsedTime
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
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
            assert!(cx.debug_bounds("track-hover-callout").is_some());
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
        }

        #[gpui::test]
        fn ruler_drag_pans_the_shared_viewport_within_home(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture_with_extent(100);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| viewer.core.brush().is_some());
            window
                .update(&mut cx, |viewer, _, _| {
                    let brush = viewer.core.brush_mut().expect("brush should exist");
                    let center = brush.home().start() + brush.home().span() / 2.;
                    brush.zoom_at(center, 2.).expect("zoom should succeed");
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, first_panel_detail_is_settled);
            let before = window
                .read_with(&cx, |viewer, _| {
                    viewer.core.brush().map(|brush| brush.selected())
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
                    let brush = viewer.core.brush().expect("brush should remain available");
                    assert_ne!(brush.selected(), before);
                    assert_eq!(brush.selected().span(), before.span());
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
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| viewer.core.brush().is_some());
            window
                .update(&mut cx, |viewer, _, _| {
                    let brush = viewer.core.brush_mut().expect("brush should exist");
                    let center = brush.home().start() + brush.home().span() / 2.;
                    brush.zoom_at(center, 2.).expect("zoom should succeed");
                })
                .expect("viewer should remain open");
            let before = window
                .read_with(&cx, |viewer, _| {
                    viewer.core.brush().expect("brush").selected()
                })
                .expect("viewer should remain open");
            let ruler = cx
                .debug_bounds("ruler-hit-area")
                .expect("shared ruler hit area should render");

            for delta in [-80., -10_000.] {
                cx.simulate_event(ScrollWheelEvent {
                    position: ruler.center(),
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                });
            }

            window
                .read_with(&cx, |viewer, _| {
                    let brush = viewer.core.brush().expect("brush should remain available");
                    assert_eq!(brush.selected().span(), before.span());
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
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
            select_fixture_run(window, &mut cx, project_id, run_id, 1);
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.select_metric(MetricKey::from_string("loss"), cx);
                })
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| viewer.core.brush().is_some());
            window
                .update(&mut cx, |viewer, _, _| {
                    let brush = viewer.core.brush_mut().expect("brush should exist");
                    let center = brush.home().start() + brush.home().span() / 2.;
                    brush.zoom_at(center, 2.).expect("zoom should succeed");
                })
                .expect("viewer should remain open");
            let before = window
                .read_with(&cx, |viewer, _| {
                    viewer.core.brush().expect("brush").selected()
                })
                .expect("viewer should remain open");
            let ruler = cx
                .debug_bounds("ruler-hit-area")
                .expect("shared ruler hit area should render");
            let anchor_ratio = 0.25;
            let position = point(
                ruler.origin.x + ruler.size.width * anchor_ratio,
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
                    let selected = viewer.core.brush().expect("brush").selected();
                    let anchored = selected.start() + selected.span() * f64::from(anchor_ratio);
                    assert!(selected.span() < before.span());
                    assert!((anchored - anchor).abs() < 1e-9);
                    assert!(viewer.detail_refresh_pending);
                })
                .expect("viewer should remain open");
        }

        #[gpui::test]
        fn metric_sidebar_rows_align_with_independent_chart_tracks(cx: &mut TestAppContext) {
            let (root, project_id, first_run_id) = fixture_with_runs(2, 2);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
            select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 2);
            window
                .update(&mut cx, |viewer, _, cx| {
                    let source_id = viewer
                        .core
                        .selection()
                        .source_id
                        .clone()
                        .expect("fixture source should be selected");
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
                viewer
                    .core
                    .catalog()
                    .is_some_and(|catalog| catalog.metric_keys.len() == 2)
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
                assert!(canvas.size.width > px(0.));
                assert!(metadata.size.height > px(0.));
            }
            let (ranges, unavailable) = window
                .read_with(&cx, |viewer, _| {
                    let selected = viewer.core.brush().map(|brush| brush.selected());
                    let ranges = viewer
                        .views
                        .active()
                        .panels
                        .iter()
                        .map(|panel| {
                            renderer::detail_viewport(
                                panel.detail.as_deref().expect("detail should be loaded"),
                                selected,
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
            assert!(unavailable);

            let resize = cx
                .debug_bounds("metric-resize:metric-0")
                .expect("Metric row resize handle should render");
            cx.simulate_mouse_down(resize.center(), MouseButton::Left, Modifiers::default());
            cx.simulate_mouse_move(
                point(resize.center().x, resize.center().y + px(40.)),
                Some(MouseButton::Left),
                Modifiers::default(),
            );
            cx.simulate_mouse_up(
                point(resize.center().x, resize.center().y + px(40.)),
                MouseButton::Left,
                Modifiers::default(),
            );
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
            assert_eq!(heights, [92., 52.]);

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
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
            assert!(cx.debug_bounds("inspector-context").is_some());
            assert!(cx.debug_bounds("close-inspector").is_some());
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

            let ranking = cx
                .debug_bounds("inspector-ranking")
                .expect("Ranking inspector tab should render");
            cx.simulate_click(ranking.center(), Modifiers::default());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.views.active().inspector_tab)
                    .expect("viewer should remain open"),
                InspectorTab::Ranking
            );
            let maximize = cx
                .debug_bounds("ranking-maximize")
                .expect("Ranking direction should require an explicit choice");
            cx.simulate_click(maximize.center(), Modifiers::default());
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().ranking_direction == Some(ObjectiveDirection::Maximize)
                    && viewer.views.active().panels[0]
                        .inspector_generation
                        .is_none()
            });

            let previous_height = window
                .read_with(&cx, |viewer, _| viewer.bottom_inspector_height)
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.begin_inspector_resize(
                        &MouseDownEvent {
                            position: point(px(0.), px(300.)),
                            modifiers: Modifiers::default(),
                            button: MouseButton::Left,
                            click_count: 1,
                            first_mouse: false,
                        },
                        cx,
                    );
                    viewer.move_inspector_resize(
                        &MouseMoveEvent {
                            position: point(px(0.), px(340.)),
                            modifiers: Modifiers::default(),
                            pressed_button: Some(MouseButton::Left),
                        },
                        cx,
                    );
                    viewer.finish_inspector_resize(cx);
                })
                .expect("viewer should remain open");
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.bottom_inspector_height)
                    .expect("viewer should remain open")
                    < previous_height
            );
            let retained_height = window
                .read_with(&cx, |viewer, _| viewer.bottom_inspector_height)
                .expect("viewer should remain open");
            window
                .update(&mut cx, |viewer, window, _| viewer.focus.focus(window))
                .expect("viewer should remain open");
            let close = cx
                .debug_bounds("close-inspector")
                .expect("Inspector header should expose a close control");
            cx.simulate_click(close.center(), Modifiers::default());
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
                    assert!(!viewer.bottom_inspector_visible);
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
        fn track_scheduler_queries_and_prepares_only_visible_overscan(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(10);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            cx.simulate_resize(size(px(600.), px(420.)));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
            select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 6);
            window
                .update(&mut cx, |viewer, _, cx| {
                    let source_id = viewer
                        .core
                        .selection()
                        .source_id
                        .clone()
                        .expect("fixture source should be selected");
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
                    viewer.core.brush().map(|brush| brush.selected())
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
                        viewer.core.brush().map(|brush| brush.selected()),
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
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
                        .core
                        .brush_mut()
                        .expect("timeline brush should exist");
                    let anchor = brush.selected().start() + brush.selected().span() / 2.;
                    brush.zoom_at(anchor, 1.25).expect("zoom should succeed");
                    viewer.schedule_detail_refresh(cx);
                    let brush = viewer
                        .core
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
                .read_with(&cx, |viewer, _| viewer.core.selected_viewport())
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
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
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
                            .core
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
                            .core
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
                .read_with(&cx, |viewer, _| viewer.core.selected_viewport())
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
