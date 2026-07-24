use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{cell::RefCell, rc::Rc};

use gpui::{
    AnyElement, App, Application, Bounds, Context, EntityId, FocusHandle, KeyBinding, KeyDownEvent,
    ListHorizontalSizingBehavior, Menu, MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathPromptOptions, Point, Render, ScrollWheelEvent, SharedString, SystemMenuType,
    Task, UniformListDecoration, UniformListScrollHandle, Window, WindowBounds, WindowOptions,
    actions, div, prelude::*, px, size, uniform_list,
};
use pulseon_chart_core::{BrushState, CanvasSize};
use pulseon_model::alignment::{AlignmentAxis, AlignmentViewport};
use pulseon_model::comparison::{EvidenceCompleteness, EvidenceReason, ObjectiveDirection};
use pulseon_model::metric::MetricKey;
use pulseon_model::run::{Run, RunStatus};
use pulseon_model::types::ProjectId;
use pulseon_viewer::coordination::AnalysisViewId;
use pulseon_viewer::coordination::{
    MetricPanelId, PanelReadCoordinator, PanelReadOutcome, PanelReadRequest, PanelReadTag,
};
use pulseon_viewer::core::{
    ApplyOutcome, DataSourceId, MAX_SELECTED_RUNS, RunRef, ViewerCore, run_matches_filter,
};
use pulseon_viewer::model::{CatalogSnapshot, DiscoveryRequest};
use pulseon_viewer::query::InspectorSnapshot;
use pulseon_viewer::registry::{SourceRegistry, SourceStatus};
use pulseon_viewer::workbench::{AnalysisViews, InspectorTab, MetricPanel, TrackDensity};
use pulseon_viewer::workbench_document::{SavedAnalysisView, SavedRunRef, WorkbenchDocument};
use pulseon_viewer::worker::{Generation, ReadEvent, ReadEventReceiver, ReadKind, ReadRequest};

mod assets;
mod components;
mod renderer;
mod theme;

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TrackViewport {
    visible: Range<usize>,
    overscan: Range<usize>,
    logical_width_bits: u32,
    physical_width: u32,
}

#[derive(Clone)]
struct TrackViewportObserver {
    state: Rc<RefCell<TrackViewport>>,
    viewer_id: EntityId,
    metric_sidebar_width: gpui::Pixels,
    plot_inset: gpui::Pixels,
}

impl UniformListDecoration for TrackViewportObserver {
    fn compute(
        &self,
        visible: Range<usize>,
        bounds: Bounds<gpui::Pixels>,
        _: Point<gpui::Pixels>,
        _: gpui::Pixels,
        item_count: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let viewport_items = visible.len().max(1);
        let overscan = visible.start.saturating_sub(viewport_items)
            ..visible.end.saturating_add(viewport_items).min(item_count);
        let plot_width = f32::from(
            (bounds.size.width - self.metric_sidebar_width - self.plot_inset).max(px(1.)),
        );
        let physical_width = (plot_width * window.scale_factor()).round().max(1.) as u32;
        let next = TrackViewport {
            visible,
            overscan,
            logical_width_bits: plot_width.to_bits(),
            physical_width,
        };
        let mut state = self.state.borrow_mut();
        if *state != next {
            *state = next;
            let viewer_id = self.viewer_id;
            cx.defer(move |cx| cx.notify(viewer_id));
            window.request_animation_frame();
        }
        div().into_any_element()
    }
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
        DragGesture::BrushStart | DragGesture::BrushEnd | DragGesture::Detail { .. } => {}
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

    fn shared(&self) -> Rc<[Run]> {
        Rc::clone(&self.runs)
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
                KeyBinding::new("cmd-+", ZoomIn, None),
                KeyBinding::new("cmd--", ZoomOut, None),
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
                MenuItem::action("Elapsed", UseElapsed),
            ],
        },
    ]
}

struct ViewerApp {
    theme: ViewerTheme,
    focus: FocusHandle,
    filter_focus: FocusHandle,
    view_name_focus: FocusHandle,
    run_filter: String,
    run_list: RunListCache,
    expanded_projects: HashSet<(DataSourceId, ProjectId)>,
    views: AnalysisViews,
    panel_reads: PanelReadCoordinator,
    renaming_view: Option<AnalysisViewId>,
    view_name_draft: String,
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
    metric_scroll: UniformListScrollHandle,
    track_viewport: Rc<RefCell<TrackViewport>>,
    overview_revision: u64,
    detail_revision: u64,
    overview_width: u32,
    detail_width: u32,
    hover: Option<HoverPoint>,
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
            view_name_focus: cx.focus_handle().tab_stop(true),
            run_filter: String::new(),
            run_list: RunListCache::default(),
            expanded_projects: HashSet::new(),
            views: AnalysisViews::default(),
            panel_reads: PanelReadCoordinator::default(),
            renaming_view: None,
            view_name_draft: String::new(),
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
            metric_scroll: UniformListScrollHandle::new(),
            track_viewport: Rc::new(RefCell::new(TrackViewport::default())),
            overview_revision: 0,
            detail_revision: 0,
            overview_width: 1_000,
            detail_width: 1_000,
            hover: None,
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
        for path in document
            .views
            .iter()
            .flat_map(|view| view.runs.iter().map(|run| &run.source_path))
        {
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
        self.metric_scroll = UniformListScrollHandle::new();
        *self.track_viewport.borrow_mut() = TrackViewport::default();
        self.hover = None;
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
                if !self.views.active().runs.is_empty()
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

    fn request_panel_overview(&mut self, panel_id: &MetricPanelId, cx: &mut Context<Self>) {
        let runs = self.views.active().runs.clone();
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
                axis: self.core.axis(),
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
        let runs = self.views.active().runs.clone();
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
        let runs = self.views.active().runs.clone();
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
                axis: self.core.axis(),
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
        self.metric_scroll = UniformListScrollHandle::new();
        *self.track_viewport.borrow_mut() = TrackViewport::default();
        self.hover = None;
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
        self.views.remove_source(source_id);
        self.source_menu = None;
        if active {
            self.core = ViewerCore::default();
            self.source_path = None;
            self.run_list = RunListCache::default();
            self.chart_adapter.borrow_mut().clear();
            self.hover = None;
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
        self.views.clear_active_timeline_extents();
        self.core.select_axis(AlignmentAxis::Step);
        self.request_overview(cx);
        cx.notify();
    }

    fn on_elapsed(&mut self, _: &UseElapsed, _: &mut Window, cx: &mut Context<Self>) {
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
            self.hover = None;
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

    fn select_metric(&mut self, metric_key: MetricKey, cx: &mut Context<Self>) {
        let panel_id = self.views.select_active_metric(metric_key.clone());
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

    fn remove_metric_panel(&mut self, panel_id: &MetricPanelId, cx: &mut Context<Self>) {
        if self.views.remove_active_panel(panel_id) {
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

    fn render_project_sidebar(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let sources = self.sources.sources().cloned().collect::<Vec<_>>();
        let selected_runs = self.views.active().runs.clone();
        let expanded = self.expanded_projects.clone();
        let query = self.run_filter.trim().to_lowercase();
        let filter_focus = self.filter_focus.clone();
        let mut name_counts = HashMap::<String, usize>::new();
        for project in sources
            .iter()
            .flat_map(|source| source.catalog.projects.iter())
        {
            *name_counts.entry(project.name.to_lowercase()).or_default() += 1;
        }

        div()
            .id("project-sidebar")
            .debug_selector(|| "project-sidebar".to_owned())
            .w(self.project_sidebar_width)
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .p(theme.spacing.panel_padding)
            .bg(theme.colors.panel)
            .border_r_1()
            .border_color(theme.colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_label("Projects", theme))
                    .child(
                        components::icon_button("import-source", theme, false, false)
                            .debug_selector(|| "import-source".to_owned())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.open_picker(cx)))
                            .child(components::icon(IconName::Plus, theme)),
                    ),
            )
            .child(
                div()
                    .id("project-run-filter")
                    .track_focus(&filter_focus)
                    .cursor_text()
                    .px_3()
                    .h(theme.spacing.control_height)
                    .flex()
                    .items_center()
                    .rounded(theme.spacing.corner_radius)
                    .border_1()
                    .border_color(theme.colors.border)
                    .focus(|style| style.border_color(theme.colors.focus))
                    .on_key_down(cx.listener(Self::on_filter_key))
                    .on_click(move |_, window, _| filter_focus.focus(window))
                    .child(if self.run_filter.is_empty() {
                        "Filter Projects and Runs".to_owned()
                    } else {
                        self.run_filter.clone()
                    }),
            )
            .child(
                div()
                    .id("project-run-tree")
                    .debug_selector(|| "project-run-tree".to_owned())
                    .flex_1()
                    .overflow_y_scroll()
                    .children(sources.into_iter().enumerate().map(|(source_index, source)| {
                        let source_label = source
                            .root_path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or(source.source_id.as_str())
                            .to_owned();
                        let status = match &source.status {
                            SourceStatus::Dormant => "Dormant",
                            SourceStatus::Loading => "Loading…",
                            SourceStatus::Ready => "Ready",
                            SourceStatus::Failed(_) => "Unavailable · click to retry",
                        };
                        let retry_path = source.root_path.clone();
                        let source_failed = matches!(source.status, SourceStatus::Failed(_));
                        let menu_open = self.source_menu.as_ref() == Some(&source.source_id);
                        let menu_source_id = source.source_id.clone();
                        let reveal_source_id = source.source_id.clone();
                        let refresh_source_id = source.source_id.clone();
                        let remove_source_id = source.source_id.clone();
                        div()
                            .mb_2()
                            .child(
                                components::sidebar_tree_row(
                                    SharedString::from(format!(
                                        "source:{}",
                                        source.source_id.as_str()
                                    )),
                                    theme,
                                    false,
                                    false,
                                )
                                .when(source_failed, |row| {
                                    row.cursor_pointer().on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.open_source(retry_path.clone(), cx);
                                        },
                                    ))
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(source_label),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(if source_failed {
                                            theme.colors.error_text
                                        } else {
                                            theme.colors.text_muted
                                        })
                                        .child(status),
                                )
                                .child(
                                    components::icon_button(
                                        SharedString::from(format!(
                                            "source-menu:{}",
                                            source.source_id
                                        )),
                                        theme,
                                        menu_open,
                                        false,
                                    )
                                    .debug_selector(move || {
                                        format!("source-menu-{source_index}")
                                    })
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.source_menu.as_ref() == Some(&menu_source_id) {
                                            this.source_menu = None;
                                        } else {
                                            this.source_menu = Some(menu_source_id.clone());
                                        }
                                        cx.stop_propagation();
                                        cx.notify();
                                    }))
                                    .child(components::icon(IconName::Ellipsis, theme)),
                                ),
                            )
                            .children(menu_open.then(|| {
                                components::popover(theme)
                                    .id(SharedString::from(format!(
                                        "source-popover:{source_index}"
                                    )))
                                    .debug_selector(move || {
                                        format!("source-popover-{source_index}")
                                    })
                                    .ml_2()
                                    .mb_2()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        components::toolbar_button(
                                            SharedString::from(format!(
                                                "reveal-source:{}",
                                                reveal_source_id
                                            )),
                                            theme,
                                            false,
                                            false,
                                        )
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.reveal_source(&reveal_source_id, cx);
                                        }))
                                        .child("Reveal in Finder"),
                                    )
                                    .child(
                                        components::toolbar_button(
                                            SharedString::from(format!(
                                                "refresh-source:{}",
                                                refresh_source_id
                                            )),
                                            theme,
                                            false,
                                            false,
                                        )
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.refresh_source(refresh_source_id.clone(), cx);
                                        }))
                                        .child("Refresh"),
                                    )
                                    .child(
                                        components::toolbar_button(
                                            SharedString::from(format!(
                                                "remove-source:{}",
                                                remove_source_id
                                            )),
                                            theme,
                                            false,
                                            false,
                                        )
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_source(&remove_source_id, cx);
                                        }))
                                        .child("Remove from Workbench"),
                                    )
                            }))
                            .children(source.catalog.projects.into_iter().enumerate().filter_map(|(project_index, project)| {
                                let project_runs = source
                                    .catalog
                                    .runs
                                    .iter()
                                    .filter(|run| run.project_id == project.project_id)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                let matches = query.is_empty()
                                    || project.name.to_lowercase().contains(&query)
                                    || project.project_id.as_str().to_lowercase().contains(&query)
                                    || project_runs.iter().any(|run| {
                                        run.name.to_lowercase().contains(&query)
                                            || run.run_id.as_str().to_lowercase().contains(&query)
                                    });
                                if !matches {
                                    return None;
                                }
                                let project_key =
                                    (source.source_id.clone(), project.project_id.clone());
                                let is_expanded = expanded.contains(&project_key);
                                let source_id = source.source_id.clone();
                                let source_path = source.root_path.clone();
                                let project_id = project.project_id.clone();
                                let action_source_id = source_id.clone();
                                let action_source_path = source_path.clone();
                                let action_project_id = project_id.clone();
                                let duplicate = name_counts
                                    .get(&project.name.to_lowercase())
                                    .is_some_and(|count| *count > 1);
                                let label = project_tree_label(
                                    &project.name,
                                    &source.source_id,
                                    duplicate,
                                );
                                Some(
                                    div()
                                        .ml_2()
                                        .child(
                                            components::sidebar_tree_row(
                                                SharedString::from(format!(
                                                    "project:{}:{}",
                                                    source.source_id,
                                                    project.project_id.as_str()
                                                )),
                                                theme,
                                                self.core.selection().source_id.as_ref()
                                                    == Some(&source.source_id)
                                                    && self.core.selection().project_id.as_ref()
                                                        == Some(&project.project_id),
                                                false,
                                            )
                                            .debug_selector(move || {
                                                format!(
                                                    "project-tree-row-{source_index}-{project_index}"
                                                )
                                            })
                                            .key_context(SELECTABLE_CONTEXT)
                                            .tab_index(0)
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.activate_tree_project(
                                                    source_id.clone(),
                                                    source_path.clone(),
                                                    project_id.clone(),
                                                    cx,
                                                );
                                            }))
                                            .on_action(cx.listener(
                                                move |this, _: &ActivateSelection, _, cx| {
                                                    this.activate_tree_project(
                                                        action_source_id.clone(),
                                                        action_source_path.clone(),
                                                        action_project_id.clone(),
                                                        cx,
                                                    );
                                                },
                                            ))
                                            .child(components::icon(
                                                if is_expanded {
                                                    IconName::ChevronDown
                                                } else {
                                                    IconName::ChevronRight
                                                },
                                                theme,
                                            ))
                                            .child(label),
                                        )
                                        .when(is_expanded, |tree| {
                                            tree.children(project_runs.into_iter().enumerate().map(|(run_index, run)| {
                                                let run_ref = RunRef::new(
                                                    source.source_id.clone(),
                                                    run.project_id.clone(),
                                                    run.run_id.clone(),
                                                );
                                                let selected = selected_runs.contains(&run_ref);
                                                let action_run = run_ref.clone();
                                                let action_path = source.root_path.clone();
                                                components::sidebar_tree_row(
                                                    SharedString::from(format!(
                                                        "run:{}",
                                                        run_ref.cache_key()
                                                    )),
                                                    theme,
                                                    selected,
                                                    !selected
                                                        && selected_runs.len() >= MAX_SELECTED_RUNS,
                                                )
                                                .debug_selector(move || {
                                                    format!(
                                                        "project-tree-run-{source_index}-{project_index}-{run_index}"
                                                    )
                                                })
                                                .ml_5()
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.toggle_tree_run(
                                                        action_run.clone(),
                                                        action_path.clone(),
                                                        cx,
                                                    );
                                                }))
                                                .child(if selected { "✓" } else { "○" })
                                                .child(run.name)
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.colors.text_muted)
                                                        .child(run_status(run.status)),
                                                )
                                            }))
                                        }),
                                )
                            }))
                    })),
            )
    }

    fn render_workspace(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        if !self.views.active().panels.is_empty() {
            return self.render_metric_workspace(cx);
        }
        let theme = self.theme;
        let catalog = self
            .core
            .catalog()
            .expect("catalog checked before rendering");
        let projects = catalog.projects.clone();
        let runs = self.run_list.shared();
        let metrics = catalog.metric_keys.clone();
        let selection = self.core.selection().clone();
        let selected_project_id = selection.project_id.clone();
        let selected_metric_key = selection.metric_key.clone();
        let has_project = selected_project_id.is_some();
        let filter_focus = self.filter_focus.clone();
        let selected_count = self.views.active().runs.len();
        let source_id = selection
            .source_id
            .clone()
            .expect("catalog workspace requires a source identity");
        let main = self.render_detail(cx);

        div()
            .flex()
            .flex_1()
            .overflow_hidden()
            .child(
                div()
                    .w(self.metric_sidebar_width())
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.content_gap)
                    .p(theme.spacing.panel_padding)
                    .bg(theme.colors.panel)
                    .border_r_1()
                    .border_color(theme.colors.border)
                    .child(section_label("Project", theme).hidden())
                    .child(
                        uniform_list(
                            "projects",
                            projects.len(),
                            cx.processor(move |_this, range: Range<usize>, _, cx| {
                                range
                                    .filter_map(|index| {
                                        projects.get(index).map(|project| (index, project))
                                    })
                                    .map(|(index, project)| {
                                        let project_id = project.project_id.clone();
                                        let action_project_id = project_id.clone();
                                        let selected = selected_project_id.as_ref()
                                            == Some(&project.project_id);
                                        components::sidebar_tree_row(
                                            ("project", index),
                                            theme,
                                            selected,
                                            false,
                                        )
                                        .debug_selector(move || format!("project-row-{index}"))
                                        .key_context(SELECTABLE_CONTEXT)
                                        .tab_index(0)
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_project(project_id.clone(), cx);
                                        }))
                                        .on_action(cx.listener(
                                            move |this, _: &ActivateSelection, _, cx| {
                                                this.select_project(action_project_id.clone(), cx);
                                            },
                                        ))
                                        .child(project.name.clone())
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .debug_selector(|| "projects-list".to_owned())
                        .h(px(120.))
                        .hidden(),
                    )
                    .when(has_project, |sidebar| {
                        sidebar
                            .child(section_label(
                                &format!("Runs ({selected_count}/{MAX_SELECTED_RUNS})"),
                                theme,
                            ).hidden())
                            .child(
                                div()
                                    .id("run-filter")
                                    .track_focus(&filter_focus)
                                    .cursor_text()
                                    .px_3()
                                    .py_2()
                                    .h(theme.spacing.control_height)
                                    .rounded(theme.spacing.corner_radius)
                                    .border_1()
                                    .border_color(theme.colors.border)
                                    .debug_selector(|| "run-filter".to_owned())
                                    .focus(|style| style.border_color(theme.colors.focus))
                                    .on_key_down(cx.listener(Self::on_filter_key))
                                    .on_click(move |_, window, _| filter_focus.focus(window))
                                    .child(if self.run_filter.is_empty() {
                                        "Filter by name, id, or status".to_owned()
                                    } else {
                                        self.run_filter.clone()
                                    })
                                    .hidden(),
                            )
                            .child(
                                uniform_list(
                                    "runs",
                                    runs.len(),
                                    cx.processor(move |this, range: Range<usize>, _, cx| {
                                        range
                                            .filter_map(|index| {
                                                runs.get(index).map(|run| (index, run))
                                            })
                                            .map(|(index, run)| {
                                                let run_ref = RunRef::new(
                                                    source_id.clone(),
                                                    run.project_id.clone(),
                                                    run.run_id.clone(),
                                                );
                                                let selected = this
                                                    .core
                                                    .selection()
                                                    .runs
                                                    .contains(&run_ref);
                                                let can_toggle = selected
                                                    || this.core.selection().runs.len()
                                                        < MAX_SELECTED_RUNS;
                                                components::sidebar_tree_row(
                                                    ("run", index),
                                                    theme,
                                                    selected,
                                                    !can_toggle,
                                                )
                                                .debug_selector(move || format!("run-row-{index}"))
                                                .whitespace_nowrap()
                                                .border_b_1()
                                                .border_color(theme.colors.border)
                                                .when(can_toggle, |row| {
                                                        let action_run = run_ref.clone();
                                                    row.key_context(SELECTABLE_CONTEXT)
                                                            .tab_index(0)
                                                            .cursor_pointer()
                                                            .on_click(cx.listener(
                                                                move |this, _, _, cx| {
                                                                    this.toggle_run(run_ref.clone(), cx);
                                                                },
                                                            ))
                                                            .on_action(cx.listener(
                                                                move |this,
                                                                      _: &ActivateSelection,
                                                                      _,
                                                                      cx| {
                                                                    this.toggle_run(action_run.clone(), cx);
                                                                },
                                                            ))
                                                })
                                                .child(
                                                    div().flex_shrink_0().child(run.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .debug_selector(move || {
                                                            format!("run-meta-{index}")
                                                        })
                                                        .flex_shrink_0()
                                                        .text_xs()
                                                        .text_color(theme.colors.text_muted)
                                                        .child(format!(
                                                            "{} · {}",
                                                            run.run_id.as_str(),
                                                            run_status(run.status)
                                                        )),
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .with_horizontal_sizing_behavior(
                                    ListHorizontalSizingBehavior::Unconstrained,
                                )
                                .debug_selector(|| "runs-list".to_owned())
                                .h(px(300.))
                                .hidden(),
                            )
                            .child(section_label("Metric", theme))
                            .child(
                                uniform_list(
                                    "metrics",
                                    metrics.len(),
                                    cx.processor(move |_this, range: Range<usize>, _, cx| {
                                        range
                                            .filter_map(|index| {
                                                metrics.get(index).map(|metric| (index, metric))
                                            })
                                            .map(|(index, metric)| {
                                                let selected =
                                                    selected_metric_key.as_ref() == Some(metric);
                                                let selected_metric = metric.clone();
                                                let action_metric = selected_metric.clone();
                                                components::sidebar_tree_row(
                                                    ("metric", index),
                                                    theme,
                                                    selected,
                                                    false,
                                                )
                                                .debug_selector(move || {
                                                    format!("metric-row-{index}")
                                                })
                                                .key_context(SELECTABLE_CONTEXT)
                                                .tab_index(0)
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.select_metric(selected_metric.clone(), cx);
                                                }))
                                                .on_action(cx.listener(
                                                    move |this, _: &ActivateSelection, _, cx| {
                                                        this.select_metric(
                                                            action_metric.clone(),
                                                            cx,
                                                        );
                                                    },
                                                ))
                                                .child(metric.as_str().to_owned())
                                            })
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .debug_selector(|| "metrics-list".to_owned())
                                .flex_1()
                                .min_h(px(80.)),
                            )
                    }),
            )
            .child(div().flex_1().h_full().overflow_hidden().child(main))
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
        let row_height = match self.views.active().track_density {
            TrackDensity::Compact => px(180.),
            TrackDensity::Comfortable => px(240.),
            TrackDensity::Spacious => px(320.),
        };
        let metric_sidebar_width = self.metric_sidebar_width();
        let timeline = self.render_overview(cx);
        let list_panels = Rc::clone(&panels);
        let panel_count = panels.len();
        let observer = TrackViewportObserver {
            state: Rc::clone(&self.track_viewport),
            viewer_id: cx.entity().entity_id(),
            metric_sidebar_width,
            plot_inset: theme.spacing.content_padding * 2.,
        };
        let scroll = self.metric_scroll.clone();
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
                            .w(metric_sidebar_width)
                            .flex_shrink_0()
                            .p(theme.spacing.panel_padding)
                            .bg(theme.colors.panel)
                            .border_r_1()
                            .border_color(theme.colors.border)
                            .child(section_label("Metrics", theme))
                            .children(
                                available
                                    .into_iter()
                                    .filter(|metric| !selected.contains(metric))
                                    .map(|metric| {
                                        let action_metric = metric.clone();
                                        components::toolbar_button(
                                            SharedString::from(format!(
                                                "add-metric:{}",
                                                metric.as_str()
                                            )),
                                            theme,
                                            false,
                                            false,
                                        )
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_metric(action_metric.clone(), cx);
                                        }))
                                        .child(format!("+ {}", metric.as_str()))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .px(theme.spacing.content_padding)
                            .child(timeline),
                    ),
            )
            .child(
                uniform_list(
                    "metric-track-scroll",
                    panel_count,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        range
                            .filter_map(|index| list_panels.get(index).cloned())
                            .map(|panel| this.render_metric_row(panel, row_height, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(scroll)
                .with_decoration(observer)
                .debug_selector(|| "metric-track-scroll".to_owned())
                .flex_1(),
            )
            .children(inspector)
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
        let snapshot = panel.as_ref().and_then(|panel| panel.inspector.as_deref());
        let body = if panel
            .as_ref()
            .is_some_and(|panel| panel.is_pending(ReadKind::Inspector))
        {
            div().child("Loading metric summaries and objective evidence…")
        } else {
            match active_tab {
                InspectorTab::Summary => {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .children(snapshot.into_iter().flat_map(|snapshot| {
                            snapshot.runs.iter().map(|run| match &run.summary {
                            Some(stats) => format!(
                                "{} · count {} · last step {} · last {:.6} · min {:.6} · max {:.6}",
                                run.run.name,
                                stats.effective_count,
                                stats.last_step.value(),
                                stats.last_value_f64,
                                stats.min_value_f64,
                                stats.max_value_f64,
                            ),
                            None => format!("{} · no metric summary", run.run.name),
                        })
                        }))
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
                        .children(direction.into_iter().flat_map(|_| {
                            snapshot
                                .into_iter()
                                .flat_map(ranking_lines)
                        }))
                }
                InspectorTab::Evidence => {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .children(snapshot.into_iter().flat_map(|snapshot| {
                            snapshot.runs.iter().map(|run| {
                                format!(
                                    "{} · status {} · last step {} · last value {} · {:?}{} · Project {} · Source {}",
                                    run.run.name,
                                    run_status(run.evidence.run_status),
                                    run.evidence.last_step.map_or_else(
                                        || "—".to_owned(),
                                        |step| step.value().to_string(),
                                    ),
                                    run.evidence.last_value_f64.map_or_else(
                                        || "—".to_owned(),
                                        |value| format!("{value:.6}"),
                                    ),
                                    run.evidence.completeness,
                                    reasons_label(&run.evidence.reasons),
                                    run.run_ref.project_id.as_str(),
                                    run.run_ref.source_id
                                )
                            })
                        }))
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
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(theme.spacing.tab_height)
                    .flex_shrink_0()
                    .px(theme.spacing.panel_padding)
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .mr_3()
                            .child(metric_name),
                    )
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
                    ),
            )
            .child(
                body.flex_1()
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
        let row_height = match self.views.active().track_density {
            TrackDensity::Compact => 180.,
            TrackDensity::Comfortable => 240.,
            TrackDensity::Spacious => 320.,
        } - f64::from(self.theme.spacing.content_padding * 2.);
        let canvas = CanvasSize::new(logical_width, row_height.max(1.)).ok();
        for panel in panels {
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
        let selected = self.views.active().selected_panel_id.as_ref() == Some(&panel_id);
        let error_count = panel.source_errors.len();
        let track = self.render_metric_track(&panel, cx);
        div()
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
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(panel.metric_key.as_str().to_owned())
                            .children((error_count > 0).then(|| {
                                div()
                                    .text_xs()
                                    .text_color(theme.colors.error_text)
                                    .child(format!("{error_count} source error(s)"))
                            })),
                    )
                    .child(
                        components::icon_button(
                            SharedString::from(format!("remove-metric:{}", panel_id.as_str())),
                            theme,
                            false,
                            false,
                        )
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
                        )
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
            .children(self.track_hovers.get(&panel_id).map(|hover| {
                components::tooltip(theme)
                    .absolute()
                    .top_2()
                    .left_2()
                    .child(format!("{} · {}", hover.run_name, hover.metric_key))
                    .child(hover_value_line(self.core.axis(), hover))
            }))
    }

    fn render_detail(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let Some(snapshot) = self.core.detail_shared() else {
            let overview = self.core.overview_shared();
            let message =
                empty_detail_message(overview.is_some(), self.core.is_pending(ReadKind::Detail));
            return div()
                .size_full()
                .flex()
                .flex_col()
                .p(theme.spacing.content_padding)
                .gap(theme.spacing.content_gap)
                .children(
                    overview
                        .as_ref()
                        .map(|snapshot| self.render_legend(snapshot)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(message),
                );
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
        let x_ticks = pulseon_chart_core::linear_ticks(viewport.x, 6);
        let y_ticks = pulseon_chart_core::linear_ticks(viewport.y, 6);
        let adapter = Rc::clone(&self.chart_adapter);
        let hit_snapshot = Arc::clone(&snapshot);
        let hit_adapter = Rc::clone(&self.chart_adapter);
        let axis = self.core.axis();
        let interaction_range = self.core.brush().map(|brush| brush.selected());
        let pending = self.core.is_pending(ReadKind::Detail)
            || interaction_range
                .is_some_and(|selected| !renderer::selection_covered(&snapshot, selected));

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .p(theme.spacing.content_padding)
            .gap(theme.spacing.content_gap)
            .child(self.render_legend(&snapshot))
            .child(
                div()
                    .flex_1()
                    .min_h(px(240.))
                    .flex()
                    .child(
                        div()
                            .w(px(72.))
                            .h_full()
                            .flex()
                            .flex_col()
                            .justify_between()
                            .text_xs()
                            .text_color(theme.colors.text_muted)
                            .children(y_ticks.iter().rev().map(|value| format_tick(*value))),
                    )
                    .child(
                        div()
                            .id("detail-chart")
                            .debug_selector(|| "detail-chart".to_owned())
                            .focusable()
                            .relative()
                            .flex_1()
                            .h_full()
                            .border_1()
                            .border_color(theme.colors.border)
                            .bg(theme.colors.surface)
                            .child(
                                renderer::detail_canvas(
                                    adapter,
                                    Arc::clone(&snapshot),
                                    self.detail_revision,
                                    viewport,
                                )
                                .size_full(),
                            )
                            .on_mouse_move(cx.listener(
                                move |this, event: &MouseMoveEvent, _, cx| {
                                    if event.dragging() {
                                        this.move_detail_drag(event, cx);
                                    } else {
                                        this.hover = hit_adapter.borrow().hit_test(
                                            &hit_snapshot,
                                            viewport,
                                            event.position,
                                        );
                                    }
                                    cx.notify();
                                },
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.begin_detail_drag(event, cx);
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    this.finish_drag(cx);
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                    this.finish_drag(cx);
                                }),
                            )
                            .on_scroll_wheel(cx.listener(
                                |this, event: &ScrollWheelEvent, _, cx| {
                                    this.zoom_detail(event, cx);
                                },
                            ))
                            .on_hover(cx.listener(|this, hovered, _, cx| {
                                if !hovered {
                                    this.hover = None;
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .ml(px(72.))
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.colors.text_muted)
                    .children(x_ticks.into_iter().map(format_tick)),
            )
            .child(
                div()
                    .ml(px(72.))
                    .text_center()
                    .text_sm()
                    .child(axis_label(axis)),
            )
            .child(self.render_overview(cx))
            .children(pending.then(|| {
                components::status_badge(theme, StatusTone::Warning)
                    .absolute()
                    .top(px(84.))
                    .right(px(28.))
                    .child("Updating viewport…")
            }))
            .children(self.hover.as_ref().map(|hover| {
                components::tooltip(theme)
                    .id(SharedString::from(format!(
                        "hover-tooltip:{}",
                        hover.run_ref.cache_key()
                    )))
                    .absolute()
                    .top(px(84.))
                    .left(px(100.))
                    .child(format!("{} · {}", hover.run_name, hover.metric_key))
                    .child(hover_value_line(axis, hover))
            }))
    }

    fn render_legend(&self, snapshot: &pulseon_viewer::query::CurveSnapshot) -> gpui::Div {
        let colors = self.theme.colors;
        div()
            .flex()
            .flex_wrap()
            .gap_3()
            .children(snapshot.series.iter().map(|curve| {
                let drawable = matches!(
                    curve.evidence.completeness,
                    EvidenceCompleteness::Complete | EvidenceCompleteness::Partial
                );
                let color = if drawable {
                    colors.series_color(renderer::series_color_index(&curve.run_ref))
                } else {
                    colors.disabled
                };
                let evidence = format!(
                    "{:?}{}",
                    curve.evidence.completeness,
                    reasons_label(&curve.evidence.reasons)
                );
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(10.)).rounded_full().bg(color))
                    .child(curve.run.name.clone())
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.text_muted)
                            .child(evidence),
                    )
            }))
    }

    fn render_overview(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let Some(brush) = self.core.brush() else {
            return div().h(px(96.));
        };
        let adapter = Rc::clone(&self.chart_adapter);
        let selected = brush.selected();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .id("overview-chart")
                    .debug_selector(|| "overview-chart".to_owned())
                    .focusable()
                    .h(px(96.))
                    .w_full()
                    .relative()
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme.colors.border)
                    .bg(theme.colors.surface)
                    .child(renderer::timeline_canvas(adapter, brush).size_full())
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
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.colors.text_muted)
                    .child(format_tick(selected.start()))
                    .child(format_tick(selected.end())),
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
        self.hover = None;
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

    fn begin_detail_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(range) = self.core.brush().map(|brush| brush.selected()) else {
            return;
        };
        self.drag = self
            .chart_adapter
            .borrow()
            .detail_axis_at(range, event.position)
            .map(|_| DragGesture::Detail {
                panel_id: MetricPanelId::from_string("legacy-detail"),
                origin_x: f64::from(event.position.x),
                last_x: f64::from(event.position.x),
                moved: false,
            });
        self.hover = None;
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
        let adapter = if panel_id.as_str() == "legacy-detail" {
            Some(Rc::clone(&self.chart_adapter))
        } else {
            self.track_adapters.get(&panel_id).cloned()
        };
        let delta = adapter.and_then(|adapter| {
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
        let is_click = match event {
            gpui::ClickEvent::Mouse(event) => {
                let delta = event.up.position - event.down.position;
                f32::from(delta.x).abs() < 3. && f32::from(delta.y).abs() < 3.
            }
            gpui::ClickEvent::Keyboard(_) => true,
        };
        if is_click {
            self.drag = None;
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

    fn zoom_detail(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Some(range) = self.core.brush().map(|brush| brush.selected()) else {
            return;
        };
        let Some(anchor) = self
            .chart_adapter
            .borrow()
            .detail_axis_at(range, event.position)
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
                    runs: view
                        .runs
                        .iter()
                        .map(|run| SavedRunRef {
                            source_path: self.sources.source(&run.source_id).map_or_else(
                                || PathBuf::from(run.source_id.as_str()),
                                |source| source.root_path.clone(),
                            ),
                            project_id: run.project_id.clone(),
                            run_id: run.run_id.clone(),
                        })
                        .collect(),
                    metrics: view
                        .panels
                        .iter()
                        .map(|panel| panel.metric_key.as_str().to_owned())
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
        let source = self.source_path.as_ref().map_or_else(
            || "No project open".to_owned(),
            |path| path.display().to_string(),
        );
        let error = self.error().map(ToOwned::to_owned);
        let has_catalog = self
            .core
            .catalog()
            .is_some_and(|catalog| !catalog.projects.is_empty());
        let can_refresh = self.source_path.is_some();
        let views = self.views.views().to_vec();
        let active_view_id = self.views.active().view_id.clone();
        let renaming_view = self.renaming_view.clone();
        let view_name_focus = self.view_name_focus.clone();
        let view_name_draft = self.view_name_draft.clone();

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
            .on_action(cx.listener(Self::on_zoom_in))
            .on_action(cx.listener(Self::on_zoom_out))
            .on_action(cx.listener(Self::on_step))
            .on_action(cx.listener(Self::on_elapsed))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.colors.window)
            .text_color(theme.colors.text)
            .child(
                div()
                    .id("application-header")
                    .debug_selector(|| "application-header".to_owned())
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_5()
                    .py_3()
                    .bg(theme.colors.panel)
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("PulseOn Viewer"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.text_muted)
                            .child(source),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .children(
                        self.project_sidebar_visible
                            .then(|| self.render_project_sidebar(cx)),
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
                            .child(
                                components::tab_bar(theme)
                                    .id("analysis-tab-bar")
                                    .debug_selector(|| "analysis-tab-bar".to_owned())
                                    .children(views.into_iter().enumerate().map(|(index, view)| {
                                        let selected = view.view_id == active_view_id;
                                        let activate_id = view.view_id.clone();
                                        let close_id = view.view_id.clone();
                                        let editing = renaming_view.as_ref() == Some(&view.view_id);
                                        let focus = view_name_focus.clone();
                                        components::analysis_tab(
                                            SharedString::from(format!(
                                                "analysis-tab:{}",
                                                view.view_id
                                            )),
                                            theme,
                                            selected,
                                        )
                                        .debug_selector(move || {
                                            if selected {
                                                "analysis-tab".to_owned()
                                            } else {
                                                format!("analysis-tab-{index}")
                                            }
                                        })
                                        .tab_index(0)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.activate_analysis_view(&activate_id, cx);
                                        }))
                                        .child(if editing {
                                            div()
                                                .id(SharedString::from(format!(
                                                    "rename-view:{}",
                                                    view.view_id
                                                )))
                                                .track_focus(&focus)
                                                .on_key_down(cx.listener(Self::on_view_name_key))
                                                .child(view_name_draft.clone())
                                        } else {
                                            div()
                                                .id(SharedString::from(format!(
                                                    "view-name:{}",
                                                    view.view_id
                                                )))
                                                .child(view.name)
                                        })
                                        .child(
                                            components::icon_button(
                                                SharedString::from(format!(
                                                    "close-view:{}",
                                                    close_id
                                                )),
                                                theme,
                                                false,
                                                false,
                                            )
                                            .debug_selector(move || {
                                                if selected {
                                                    "close-active-view".to_owned()
                                                } else {
                                                    format!("close-view-{index}")
                                                }
                                            })
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.close_analysis_view(&close_id, cx);
                                                cx.stop_propagation();
                                            }))
                                            .child(components::icon(IconName::Close, theme)),
                                        )
                                    }))
                                    .child(
                                        components::toolbar_button(
                                            "duplicate-view",
                                            theme,
                                            false,
                                            false,
                                        )
                                        .debug_selector(|| "duplicate-view".to_owned())
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.duplicate_analysis_view(cx);
                                        }))
                                        .child("Duplicate"),
                                    )
                                    .child(
                                        components::toolbar_button(
                                            "rename-view",
                                            theme,
                                            false,
                                            false,
                                        )
                                        .debug_selector(|| "rename-view".to_owned())
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let view_id = this.views.active().view_id.clone();
                                            this.begin_rename_analysis_view(view_id, window, cx);
                                        }))
                                        .child("Rename"),
                                    )
                                    .child(
                                        components::icon_button("new-view", theme, false, false)
                                            .debug_selector(|| "new-view".to_owned())
                                            .cursor_pointer()
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.create_analysis_view(cx);
                                            }))
                                            .child(components::icon(IconName::Plus, theme)),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        components::icon_button(
                                            "refresh-view",
                                            theme,
                                            false,
                                            !can_refresh,
                                        )
                                        .debug_selector(|| "refresh-view".to_owned())
                                        .when(can_refresh, |button| {
                                            button.cursor_pointer().on_click(cx.listener(
                                                |this, _, _, cx| {
                                                    this.local_error = None;
                                                    this.refresh_catalog(cx);
                                                    cx.notify();
                                                },
                                            ))
                                        })
                                        .child(components::icon(IconName::Refresh, theme)),
                                    ),
                            )
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

fn section_label(label: &str, theme: ViewerTheme) -> gpui::Div {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.colors.text_muted)
        .child(label.to_owned())
}

fn ranking_lines(snapshot: &InspectorSnapshot) -> Vec<String> {
    let mut projects = Vec::<((DataSourceId, ProjectId), Vec<_>)>::new();
    for run in &snapshot.runs {
        let key = (
            run.run_ref.source_id.clone(),
            run.run_ref.project_id.clone(),
        );
        if let Some((_, runs)) = projects.iter_mut().find(|(candidate, _)| candidate == &key) {
            runs.push(run);
        } else {
            projects.push((key, vec![run]));
        }
    }
    let mut lines = Vec::new();
    for ((source_id, project_id), mut runs) in projects {
        lines.push(format!(
            "Project {} · Source {}",
            project_id.as_str(),
            source_id
        ));
        runs.sort_by_key(|run| {
            run.ranking
                .map(|ranking| ranking.order)
                .unwrap_or(usize::MAX)
        });
        for run in runs {
            let rank = run
                .ranking
                .and_then(|ranking| ranking.rank)
                .map_or_else(|| "—".to_owned(), |rank| format!("#{rank}"));
            let step = run
                .evidence
                .last_step
                .map_or_else(|| "—".to_owned(), |step| step.value().to_string());
            let value = run
                .evidence
                .last_value_f64
                .map_or_else(|| "—".to_owned(), |value| format!("{value:.6}"));
            lines.push(format!(
                "{rank} · {} · status {} · step {step} · value {value} · {:?}{}",
                run.run.name,
                run_status(run.evidence.run_status),
                run.evidence.completeness,
                reasons_label(&run.evidence.reasons),
            ));
        }
    }
    lines
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

const fn axis_label(axis: AlignmentAxis) -> &'static str {
    match axis {
        AlignmentAxis::Step => "Step",
        AlignmentAxis::ElapsedTime => "Elapsed (ms)",
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

fn hover_value_line(axis: AlignmentAxis, hover: &HoverPoint) -> String {
    match axis {
        AlignmentAxis::Step => format!("step={} · value={}", hover.axis_value, hover.value),
        AlignmentAxis::ElapsedTime => format!(
            "{}={} · step={} · value={}",
            renderer::axis_value_label(axis),
            hover.axis_value,
            hover.step,
            hover.value
        ),
    }
}

const fn empty_detail_message(has_overview: bool, pending: bool) -> &'static str {
    if pending {
        "Loading curves…"
    } else if has_overview {
        "No drawable evidence is available for the selected Runs."
    } else {
        "Select Runs and one metric to draw curves."
    }
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
    fn empty_detail_message_distinguishes_evidence_from_missing_selection() {
        assert_eq!(
            empty_detail_message(true, false),
            "No drawable evidence is available for the selected Runs."
        );
        assert_eq!(
            empty_detail_message(false, false),
            "Select Runs and one metric to draw curves."
        );
        assert_eq!(empty_detail_message(true, true), "Loading curves…");
    }

    #[test]
    fn hover_value_line_avoids_duplicate_step_but_retains_step_for_elapsed_axis() {
        let hover = HoverPoint {
            run_ref: RunRef::new(
                DataSourceId::from_string("source"),
                ProjectId::from_string("project"),
                RunId::from_string("run"),
            ),
            run_name: "run".to_owned(),
            metric_key: "loss".to_owned(),
            axis_value: 2_904,
            step: 497,
            value: 0.506,
        };

        assert_eq!(
            hover_value_line(AlignmentAxis::Step, &hover),
            "step=2904 · value=0.506"
        );
        assert_eq!(
            hover_value_line(AlignmentAxis::ElapsedTime, &hover),
            "elapsed_ms=2904 · step=497 · value=0.506"
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
    fn run_list_cache_shares_filtered_runs_between_renders() {
        let cache = RunListCache::default();

        let first = cache.shared();
        let second = cache.shared();

        assert!(Rc::ptr_eq(&first, &second));
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

        let lines = ranking_lines(&snapshot);

        assert_eq!(
            lines,
            [
                "Project alpha · Source source",
                "#1 · Run 1 · status finished · step 1 · value 1.000000 · Complete",
                "#2 · Run 2 · status finished · step 1 · value 2.000000 · Complete",
                "Project beta · Source source",
                "#1 · Run 3 · status finished · step 1 · value 3.000000 · Complete",
            ]
        );
    }

    #[cfg(feature = "test-support")]
    mod gpui_tests {
        use gpui::{
            Keystroke, Modifiers, ScrollDelta, ScrollStrategy, TestAppContext, TouchPhase,
            VisualTestContext, WindowHandle, point,
        };
        use pulseon_core::engine::client::NativeClient;

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
                    metrics: vec![metric.to_owned()],
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

                let header = cx
                    .debug_bounds("application-header")
                    .expect("application header should render");
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
                    .debug_bounds("duplicate-view")
                    .expect("View toolbar control should render");

                assert_eq!(header.size.width, px(window_size.0));
                assert_eq!(sidebar.origin.y, header.origin.y + header.size.height);
                assert_eq!(analysis.origin.x, sidebar.origin.x + sidebar.size.width);
                assert_eq!(tab_bar.origin.x, analysis.origin.x);
                assert_eq!(tab_bar.size.width, analysis.size.width);
                assert_eq!(tab_bar.size.height, px(32.));
                assert_eq!(tab.size.height, px(31.));
                assert_eq!(control.size.height, px(28.));
            }

            let source_menu = cx
                .debug_bounds("source-menu-0")
                .expect("source menu control should render");
            cx.simulate_click(source_menu.center(), Modifiers::default());
            assert!(cx.debug_bounds("source-popover-0").is_some());
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.source_menu = None;
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
            cx.simulate_click(new_view.center(), Modifiers::default());
            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.core.axis())
                    .expect("viewer should remain open"),
                AlignmentAxis::Step
            );
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

            let rename = cx
                .debug_bounds("rename-view")
                .expect("rename View control should render");
            assert!(rename.size.width > px(0.));
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
            let project = cx
                .debug_bounds("project-tree-row-0-0")
                .expect("first Project row should be rendered");
            cx.simulate_click(project.center(), Modifiers::default());
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

            for (source_index, project_selector, run_selector) in [
                (0, "project-tree-row-0-0", "project-tree-run-0-0-0"),
                (1, "project-tree-row-1-0", "project-tree-run-1-0-0"),
            ] {
                let project = cx
                    .debug_bounds(project_selector)
                    .expect("Project row should be rendered");
                cx.simulate_click(project.center(), Modifiers::default());
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
                        "metric-track:metric-0"
                    } else {
                        "metric-track:metric-1"
                    })
                    .expect("Metric track should render");
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
                assert!(cx.debug_bounds("overview-chart").is_some());
            }
        }

        #[gpui::test]
        fn metric_list_scrolls_to_late_rows_in_a_small_window(cx: &mut TestAppContext) {
            let (root, project_id, run_id) = fixture(20);
            cx.executor().allow_parking();
            let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
            wait_for_viewer(window, &cx, |viewer| viewer.core.catalog().is_some());
            select_fixture_run(window, &mut cx, project_id, run_id, 20);
            cx.simulate_resize(size(px(600.), px(520.)));
            cx.run_until_parked();
            let metrics = cx
                .debug_bounds("metrics-list")
                .expect("Metric list should be rendered");
            assert!(cx.debug_bounds("metric-row-19").is_none());

            cx.simulate_event(ScrollWheelEvent {
                position: metrics.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-1_000.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            });

            assert!(cx.debug_bounds("metric-row-19").is_some());
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
                assert_eq!(sidebar.origin.y, track.origin.y);
                assert_eq!(sidebar.size.height, track.size.height);
                assert_eq!(track.origin.x, sidebar.origin.x + sidebar.size.width);
                assert_eq!(
                    track.origin.x + track.size.width,
                    workspace.origin.x + workspace.size.width
                );
                assert!(canvas.size.width > px(0.));
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
            assert!(cx.debug_bounds("bottom-inspector").is_some());
            wait_for_viewer(window, &cx, |viewer| {
                viewer.views.active().panels.first().is_some_and(|panel| {
                    panel.inspector.is_some() && !panel.is_pending(ReadKind::Inspector)
                })
            });
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
            assert!(
                window
                    .read_with(&cx, |viewer, _| viewer.bottom_inspector_visible)
                    .expect("viewer should remain open")
            );

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

            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer
                        .metric_scroll
                        .scroll_to_item_strict(9, ScrollStrategy::Bottom);
                    cx.notify();
                })
                .expect("viewer should remain open");
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
                    && schedule.visible.len() >= 6
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
