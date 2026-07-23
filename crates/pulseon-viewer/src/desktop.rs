use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::{cell::RefCell, rc::Rc};

use gpui::{
    App, Application, Bounds, Context, FocusHandle, KeyBinding, KeyDownEvent,
    ListHorizontalSizingBehavior, Menu, MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathPromptOptions, Render, ScrollWheelEvent, SharedString, SystemMenuType, Task,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size, uniform_list,
};
use pulseon_chart_core::BrushState;
use pulseon_model::alignment::AlignmentAxis;
use pulseon_model::comparison::{EvidenceCompleteness, EvidenceReason};
use pulseon_model::metric::MetricKey;
use pulseon_model::run::{Run, RunStatus};
use pulseon_model::types::ProjectId;
use pulseon_viewer::core::{
    ApplyOutcome, DataSourceId, MAX_SELECTED_RUNS, RunRef, ViewerCore, run_matches_filter,
};
use pulseon_viewer::model::{CatalogSnapshot, DiscoveryRequest};
use pulseon_viewer::query::{CurveSelection, DetailRequest, OverviewRequest};
use pulseon_viewer::registry::{SourceRegistry, SourceStatus};
use pulseon_viewer::worker::{Generation, ReadEvent, ReadEventReceiver, ReadKind, ReadRequest};

mod assets;
mod components;
mod renderer;
mod theme;

use assets::ViewerAssets;
use components::{IconName, StatusTone};
use renderer::{ChartAdapter, HoverPoint};
use theme::ViewerTheme;

#[derive(Clone, Copy, Debug)]
enum DragGesture {
    BrushStart,
    BrushEnd,
    BrushWindow { last_axis: f64 },
    Detail { last_x: f64 },
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
        UseStep,
        UseElapsed,
        ActivateSelection,
        Quit
    ]
);

const SELECTABLE_CONTEXT: &str = "ViewerSelectable";

pub fn run(project_path: Option<PathBuf>) {
    Application::new()
        .with_assets(ViewerAssets)
        .run(move |cx: &mut App| {
            cx.bind_keys([
                KeyBinding::new("cmd-o", OpenProject, None),
                KeyBinding::new("cmd-r", Refresh, None),
                KeyBinding::new("cmd-0", ResetView, None),
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
                MenuItem::action("Open Project…", OpenProject),
                MenuItem::action("Refresh", Refresh),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Reset View", ResetView),
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
    run_filter: String,
    run_list: RunListCache,
    expanded_projects: HashSet<(DataSourceId, ProjectId)>,
    source_path: Option<PathBuf>,
    sources: SourceRegistry,
    event_tasks: HashMap<DataSourceId, Task<()>>,
    core: ViewerCore,
    next_generation: u64,
    local_error: Option<String>,
    chart_adapter: Rc<RefCell<ChartAdapter>>,
    overview_revision: u64,
    detail_revision: u64,
    overview_width: u32,
    detail_width: u32,
    hover: Option<HoverPoint>,
    drag: Option<DragGesture>,
    zoom_task: Option<Task<()>>,
}

impl ViewerApp {
    fn new(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let mut app = Self {
            theme: ViewerTheme::for_appearance(window.appearance()),
            focus,
            filter_focus: cx.focus_handle().tab_stop(true),
            run_filter: String::new(),
            run_list: RunListCache::default(),
            expanded_projects: HashSet::new(),
            source_path: None,
            sources: SourceRegistry::default(),
            event_tasks: HashMap::new(),
            core: ViewerCore::default(),
            next_generation: 1,
            local_error: None,
            chart_adapter: Rc::new(RefCell::new(ChartAdapter::default())),
            overview_revision: 0,
            detail_revision: 0,
            overview_width: 1_000,
            detail_width: 1_000,
            hover: None,
            drag: None,
            zoom_task: None,
        };
        if let Some(path) = project_path {
            app.open_source(path, cx);
        }
        app
    }

    fn open_source(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let source_id = self.sources.import(path.clone());
        self.core.reset_source(source_id);
        self.run_list = RunListCache::default();
        self.chart_adapter.borrow_mut().clear();
        self.hover = None;
        self.drag = None;
        self.zoom_task = None;
        self.local_error = None;
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
        match self.sources.activate(&source_id) {
            Ok(Some(events)) => self.listen_for_events(source_id.clone(), events, cx),
            Ok(None) => {}
            Err(error) => {
                self.local_error = Some(error.to_string());
                return;
            }
        }
        let generation = Generation(self.next_generation);
        self.next_generation = self.next_generation.saturating_add(1);
        match self.sources.submit(&source_id, generation, request.clone()) {
            Ok(()) => self.core.begin(generation, source_id, &request),
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
        self.sources.apply_event(&event);
        let kind = event.kind;
        let revision = event.generation.0;
        let succeeded = event.result.is_ok();
        if self.core.apply(event) != ApplyOutcome::Applied || !succeeded {
            return;
        }
        if kind == ReadKind::Catalog {
            self.run_list.rebuild(self.core.catalog(), &self.run_filter);
        }
        match kind {
            ReadKind::Catalog if self.curve_selection().is_some() => self.request_overview(cx),
            ReadKind::Overview => {
                self.overview_revision = revision;
                self.request_detail(cx);
            }
            ReadKind::Detail => self.detail_revision = revision,
            ReadKind::Catalog => {}
        }
    }

    fn curve_selection(&self) -> Option<CurveSelection> {
        let selection = self.core.selection();
        if selection.runs.is_empty() {
            return None;
        }
        Some(CurveSelection {
            source_id: selection.source_id.clone()?,
            runs: selection.runs.clone(),
            metric_key: selection.metric_key.clone()?,
            axis: self.core.axis(),
        })
    }

    fn request_overview(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.curve_selection() else {
            return;
        };
        self.submit(
            ReadRequest::Overview(OverviewRequest {
                selection,
                physical_width: self.overview_width,
            }),
            cx,
        );
    }

    fn request_detail(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.curve_selection() else {
            return;
        };
        let Some(viewport) = self.core.selected_viewport() else {
            return;
        };
        self.submit(
            ReadRequest::Detail(DetailRequest {
                selection,
                viewport,
                physical_width: self.detail_width,
            }),
            cx,
        );
    }

    fn open_picker(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from("Open Project")),
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
            return "Open a local PulseOn project to compare Runs.".into();
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

    fn on_reset(&mut self, _: &ResetView, _: &mut Window, cx: &mut Context<Self>) {
        if self.core.reset_view() {
            self.request_detail(cx);
        }
        cx.notify();
    }

    fn on_step(&mut self, _: &UseStep, _: &mut Window, cx: &mut Context<Self>) {
        self.core.select_axis(AlignmentAxis::Step);
        self.request_overview(cx);
        cx.notify();
    }

    fn on_elapsed(&mut self, _: &UseElapsed, _: &mut Window, cx: &mut Context<Self>) {
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
        match self.core.toggle_run(run) {
            Ok(_) => {
                self.local_error = None;
                self.refresh_catalog(cx);
            }
            Err(error) => self.local_error = Some(error.to_string()),
        }
        cx.notify();
    }

    fn select_metric(&mut self, metric_key: MetricKey, cx: &mut Context<Self>) {
        self.core.select_metric(Some(metric_key));
        self.request_overview(cx);
        cx.notify();
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

    fn render_project_sidebar(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let sources = self.sources.sources().cloned().collect::<Vec<_>>();
        let selected_runs = self.core.selection().runs.clone();
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
            .w(theme.spacing.sidebar_width)
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .p(theme.spacing.panel_padding)
            .bg(theme.colors.panel)
            .border_r_1()
            .border_color(theme.colors.border)
            .child(section_label("Projects", theme))
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
                                ),
                            )
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
        let selected_count = selection.runs.len();
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
                    .w(theme.spacing.sidebar_width)
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
        let Some(snapshot) = self.core.overview_shared() else {
            return div().h(px(96.));
        };
        let Some(brush) = self.core.brush() else {
            return div().h(px(96.));
        };
        let Some(viewport) = renderer::overview_viewport(&snapshot, brush) else {
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
                    .focusable()
                    .h(px(96.))
                    .w_full()
                    .relative()
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme.colors.border)
                    .bg(theme.colors.surface)
                    .child(
                        renderer::overview_canvas(
                            adapter,
                            snapshot,
                            self.overview_revision,
                            viewport,
                            brush,
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
        let Some(mut gesture) = self.drag else {
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
                last_x: f64::from(event.position.x),
            });
        self.hover = None;
        cx.notify();
    }

    fn move_detail_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(DragGesture::Detail { mut last_x }) = self.drag else {
            return;
        };
        let Some(range) = self.core.brush().map(|brush| brush.selected()) else {
            return;
        };
        let Some(delta) =
            self.chart_adapter
                .borrow()
                .detail_pan_delta(range, last_x, event.position)
        else {
            return;
        };
        if let Some(brush) = self.core.brush_mut() {
            let _ = brush.pan_by(delta);
        }
        last_x = f64::from(event.position.x);
        self.drag = Some(DragGesture::Detail { last_x });
        cx.notify();
    }

    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
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
        let timer = cx.background_executor().timer(Duration::from_millis(100));
        self.zoom_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                this.request_detail(cx);
                cx.notify();
            });
        }));
        cx.stop_propagation();
        cx.notify();
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

        div()
            .track_focus(&self.focus)
            .tab_group()
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_reset))
            .on_action(cx.listener(Self::on_step))
            .on_action(cx.listener(Self::on_elapsed))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.colors.window)
            .text_color(theme.colors.text)
            .child(
                div()
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
                    .child(self.render_project_sidebar(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(
                                components::tab_bar(theme)
                                    .child(
                                        components::analysis_tab("analysis-tab", theme, true)
                                            .debug_selector(|| "analysis-tab".to_owned())
                                            .tab_index(0)
                                            .child("Analysis"),
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
                                        .child("Open Project…"),
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

    #[cfg(feature = "test-support")]
    mod gpui_tests {
        use gpui::{
            Modifiers, ScrollDelta, TestAppContext, TouchPhase, VisualTestContext, WindowHandle,
            point,
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
                if run_index == 0 {
                    let handle = client.run_handle(run.clone());
                    for index in 0..metric_count {
                        let metric_key = format!("metric-{index}");
                        handle
                            .log_metric_at_step(&metric_key, 0, index as f64)
                            .expect("test metric should be logged");
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
            for _ in 0..200 {
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
        fn zoom_debounce_commits_only_the_latest_wheel_event(cx: &mut TestAppContext) {
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
            wait_for_viewer(window, &cx, |viewer| viewer.core.detail().is_some());
            let chart = cx
                .debug_bounds("detail-chart")
                .expect("detail chart should be rendered");
            window
                .update(&mut cx, |_, _, cx| cx.notify())
                .expect("viewer should remain open");
            wait_for_viewer(window, &cx, |viewer| {
                !viewer.core.is_pending(ReadKind::Detail)
            });
            let before = window
                .read_with(&cx, |viewer, _| viewer.next_generation)
                .expect("viewer should remain open");
            let wheel = ScrollWheelEvent {
                position: chart.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-24.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            };

            cx.simulate_event(wheel.clone());
            cx.simulate_event(wheel);
            cx.executor().advance_clock(Duration::from_millis(101));
            cx.run_until_parked();

            assert_eq!(
                window
                    .read_with(&cx, |viewer, _| viewer.next_generation)
                    .expect("viewer should remain open"),
                before + 1
            );
        }
    }
}
