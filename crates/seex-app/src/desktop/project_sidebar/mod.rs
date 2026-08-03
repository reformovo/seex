use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Context, EventEmitter, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Render, SharedString, Window, div, prelude::*, px,
};
use seex::ProjectId;
use seex::Run;

use crate::domain::{DataSourceId, RunRef};
use crate::workbench::ProjectRef;
use crate::workbench::panel_reads::AnalysisViewId;

use super::ViewerApp;
use super::command::WorkbenchCommand;
use super::components::{self, IconName, ResizeEdge, TextInput, resize_handle};
use super::interaction::InteractionSnapshot;
use super::session::SessionSnapshot;
use super::theme::ViewerTheme;

mod rows;

pub(super) use rows::*;

const RUN_PAGE_SIZE: usize = 5;
const RUN_COLOR_MARKER_SIZE: f32 = 7.;
const RUN_TEXT_OFFSET: f32 = 20.;
const PROJECT_TEXT_OFFSET: f32 = 33.;
const RUN_HOVER_GRACE: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RunHoverExitPolicy {
    EndOfEvent,
    OneFrameGrace,
}

pub(crate) struct ProjectSidebar {
    pub filter_focus: FocusHandle,
    pub filter: gpui::Entity<TextInput>,
    pub project_focuses: HashMap<ProjectRef, FocusHandle>,
    pub run_focuses: HashMap<RunRef, FocusHandle>,
    pub menu: Option<ProjectRef>,
    pub hovered_project: Option<ProjectRef>,
    pub project_run_limits: HashMap<ProjectRef, usize>,
    pub pinned_run_limits: HashMap<AnalysisViewId, usize>,
    pub archived_run_limit: usize,
    pub visible: bool,
    pub width: gpui::Pixels,
    pub resize: Option<SidebarResize>,
    pub archived_height: Option<gpui::Pixels>,
    pub archived_resize: Option<ArchivedResize>,
    snapshot: Option<Arc<SessionSnapshot>>,
    interaction: InteractionSnapshot,
}

#[derive(Clone, Debug)]
pub(crate) enum ProjectSidebarEvent {
    Command(WorkbenchCommand),
    ImportSource,
    ManageSource(DataSourceId),
    RemoveProject(ProjectRef),
    DismissOtherPopovers,
    HoveredRun {
        run: RunRef,
        region: SharedString,
        hovered: bool,
    },
}

impl EventEmitter<ProjectSidebarEvent> for ProjectSidebar {}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ArchivedResize {
    start_y: gpui::Pixels,
    start_height: gpui::Pixels,
}

impl ProjectSidebar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|_| TextInput::default());
        cx.observe(&filter, |_, _, cx| cx.notify()).detach();
        Self {
            filter_focus: cx.focus_handle().tab_stop(true),
            filter,
            project_focuses: HashMap::new(),
            run_focuses: HashMap::new(),
            menu: None,
            hovered_project: None,
            project_run_limits: HashMap::new(),
            pinned_run_limits: HashMap::new(),
            archived_run_limit: RUN_PAGE_SIZE,
            visible: true,
            width: px(190.),
            resize: None,
            archived_height: None,
            archived_resize: None,
            snapshot: None,
            interaction: InteractionSnapshot::default(),
        }
    }

    pub(crate) fn sync(
        &mut self,
        snapshot: Arc<SessionSnapshot>,
        interaction: InteractionSnapshot,
    ) {
        self.snapshot = Some(snapshot);
        self.interaction = interaction;
    }
}

impl ViewerApp {
    pub(super) fn sidebar_visible(&self, cx: &App) -> bool {
        self.project_sidebar.read(cx).visible
    }

    pub(super) fn sidebar_width(&self, cx: &App) -> gpui::Pixels {
        self.project_sidebar.read(cx).width
    }
}

impl ProjectSidebar {
    fn sidebar_projects(&self) -> Vec<SidebarProject> {
        let Some(session) = self.snapshot.as_ref() else {
            return Vec::new();
        };
        let mut projects = Vec::new();
        for (source_index, source) in session.sources.iter().enumerate() {
            let source_label = source
                .root_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(source.source_id.as_str())
                .to_owned();
            for (project_index, project) in source.catalog.projects.iter().enumerate() {
                let project_ref =
                    ProjectRef::new(source.source_id.clone(), project.project_id.clone());
                let placement = if session.views.archived_projects().contains(&project_ref) {
                    ProjectPlacement::Archived
                } else if session.views.pinned_projects().contains(&project_ref) {
                    ProjectPlacement::Pinned
                } else {
                    ProjectPlacement::Projects
                };
                projects.push(SidebarProject {
                    source_index,
                    project_index,
                    project_ref,
                    project: project.clone(),
                    runs: source
                        .catalog
                        .runs
                        .iter()
                        .filter(|run| run.project_id == project.project_id)
                        .cloned()
                        .collect(),
                    source_label: source_label.clone(),
                    placement,
                });
            }
        }
        projects
    }

    fn sidebar_run(&self, run_ref: &RunRef) -> Option<Run> {
        let session = self.snapshot.as_ref()?;
        let source = session
            .sources
            .iter()
            .find(|source| source.source_id == run_ref.source_id)?;
        source
            .catalog
            .runs
            .iter()
            .find(|run| run.project_id == run_ref.project_id && run.run_id == run_ref.run_id)
            .cloned()
    }

    fn render_converged_project_sidebar(
        &mut self,
        window: &Window,
        theme: ViewerTheme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let projects = self.sidebar_projects();
        let Some(session) = self.snapshot.clone() else {
            return div().id("project-sidebar");
        };
        let baseline = session
            .views
            .active()
            .baseline
            .clone()
            .filter(|run| session.contains_catalog_run(run));
        let pinned_runs = session
            .views
            .active()
            .pinned_runs
            .iter()
            .filter(|run| session.contains_catalog_run(run))
            .cloned()
            .collect::<Vec<_>>();
        let archived_runs = session
            .views
            .archived_runs()
            .iter()
            .filter(|run| session.contains_catalog_run(run))
            .cloned()
            .collect::<Vec<_>>();
        let active_view = session.views.active().view_id.clone();
        let pinned_limit = self
            .pinned_run_limits
            .get(&active_view)
            .copied()
            .unwrap_or(RUN_PAGE_SIZE);
        let archived_limit = self.archived_run_limit;
        let filter_input = self.filter.clone();
        let filter_focus = self.filter_focus.clone();
        let sidebar_width = self.width;
        let sidebar_resizing = self.resize.is_some();
        let archived_height = self.archived_height;
        let archived_resizing = self.archived_resize.is_some();
        let filter = filter_input.read(cx);
        let filter_text = filter.text().to_owned();
        let filter_cursor = filter.cursor();
        let filter_cursor_visible = filter.cursor_visible();
        let query = filter_text.trim().to_lowercase();
        let (unavailable_projects, unavailable_runs) = session.unavailable_references();
        let (filter_prefix, filter_suffix) = filter_text.split_at(filter_cursor);
        let filter_prefix = filter_prefix.to_owned();
        let filter_suffix = filter_suffix.to_owned();
        let filter_focused = filter_focus.is_focused(window);

        let mut resources = div()
            .id("project-run-tree")
            .debug_selector(|| "project-run-tree".to_owned())
            .flex_1()
            .overflow_y_scroll()
            .px(theme.spacing.panel_padding)
            .pt_2()
            .pb_3();

        resources = resources.child(
            sidebar_group_label("Baseline", theme)
                .mt_1()
                .debug_selector(|| "baseline-group-label".to_owned()),
        );
        if let Some(run) = baseline {
            resources = resources.child(self.render_sidebar_run(
                run,
                RunPlacement::Baseline,
                0,
                window,
                cx,
            ));
        }

        resources = resources.child(sidebar_group_label("Pinned", theme));
        for project in projects
            .iter()
            .filter(|project| project.placement == ProjectPlacement::Pinned)
            .cloned()
        {
            resources = resources.child(self.render_sidebar_project(project, &query, window, cx));
        }
        if !unavailable_projects.is_empty() || !unavailable_runs.is_empty() {
            resources = resources.child(sidebar_group_label("Unavailable", theme));
        }
        for project in unavailable_projects {
            let identity = format!(
                "{}/{}",
                project.source_id.as_str(),
                project.project_id.as_str()
            );
            resources = resources.child(unavailable_reference_row(
                format!("unavailable-project:{identity}"),
                format!("{identity} — unavailable"),
                theme,
            ));
        }
        for run in unavailable_runs {
            let identity = format!(
                "{}/{}/{}",
                run.source_id.as_str(),
                run.project_id.as_str(),
                run.run_id.as_str()
            );
            resources = resources.child(unavailable_reference_row(
                format!("unavailable-run:{identity}"),
                format!("{identity} — unavailable"),
                theme,
            ));
        }
        for (index, run) in pinned_runs.iter().take(pinned_limit).cloned().enumerate() {
            resources = resources.child(self.render_sidebar_run(
                run,
                RunPlacement::Pinned,
                index,
                window,
                cx,
            ));
        }
        if pinned_runs.len() > pinned_limit {
            let view_id = active_view.clone();
            resources = resources.child(
                sidebar_text_button("show-more-pinned", "Show more", theme)
                    .debug_selector(|| "show-more-pinned".to_owned())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        *this
                            .pinned_run_limits
                            .entry(view_id.clone())
                            .or_insert(RUN_PAGE_SIZE) += RUN_PAGE_SIZE;
                        cx.notify();
                    })),
            );
        }

        resources = resources.child(sidebar_group_label("Projects", theme));
        for project in projects
            .iter()
            .filter(|project| project.placement == ProjectPlacement::Projects)
            .cloned()
        {
            resources = resources.child(self.render_sidebar_project(project, &query, window, cx));
        }

        let mut archived_resources = div()
            .id("archived-run-tree")
            .debug_selector(|| "archived-run-tree".to_owned())
            .flex_none()
            .relative()
            .overflow_y_scroll()
            .border_t_1()
            .border_color(theme.colors.transparent)
            .px(theme.spacing.panel_padding)
            .pb_3()
            .child(sidebar_group_label("Archived", theme));
        archived_resources = if let Some(height) = archived_height {
            archived_resources.h(height)
        } else {
            archived_resources.max_h(px(220.))
        };
        for project in projects
            .into_iter()
            .filter(|project| project.placement == ProjectPlacement::Archived)
        {
            archived_resources =
                archived_resources.child(self.render_sidebar_project(project, &query, window, cx));
        }
        for (index, run) in archived_runs
            .iter()
            .take(archived_limit)
            .cloned()
            .enumerate()
        {
            archived_resources = archived_resources.child(self.render_sidebar_run(
                run,
                RunPlacement::Archived,
                index,
                window,
                cx,
            ));
        }
        if archived_runs.len() > archived_limit {
            archived_resources = archived_resources.child(
                sidebar_text_button("show-more-archived", "Show more", theme)
                    .debug_selector(|| "show-more-archived".to_owned())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.archived_run_limit += RUN_PAGE_SIZE;
                        cx.notify();
                    })),
            );
        }
        archived_resources = archived_resources.child(
            resize_handle(
                SharedString::from("archived-sidebar-resize"),
                theme,
                archived_resizing,
                ResizeEdge::Top,
            )
            .debug_selector(|| "archived-sidebar-resize".to_owned())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.begin_archived_resize(event, window, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.finish_archived_resize(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.finish_archived_resize(cx);
                }),
            ),
        );

        div()
            .id("project-sidebar")
            .debug_selector(|| "project-sidebar".to_owned())
            .w(sidebar_width)
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .relative()
            .text_xs()
            .bg(theme.colors.panel)
            .border_r_1()
            .border_color(theme.colors.transparent)
            .child(
                div()
                    .id("project-sidebar-header")
                    .debug_selector(|| "project-sidebar-header".to_owned())
                    .h(theme.spacing.tab_height)
                    .flex_none()
                    .px(theme.spacing.panel_padding)
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Seex"),
                    )
                    .child(
                        div()
                            .id("project-header-controls")
                            .debug_selector(|| "project-header-controls".to_owned())
                            .h_full()
                            .flex_none()
                            .pl_1()
                            .gap_1()
                            .flex()
                            .items_center()
                            .child(
                                components::top_bar_icon_button(
                                    "import-source",
                                    theme,
                                    false,
                                    false,
                                )
                                .debug_selector(|| "import-source".to_owned())
                                .tooltip(components::label_tooltip("Import Source", theme))
                                .on_click(cx.listener(|_this, _, _, cx| {
                                    cx.emit(ProjectSidebarEvent::ImportSource);
                                    cx.notify();
                                }))
                                .child(components::icon(IconName::Plus, theme)),
                            )
                            .child(
                                components::top_bar_icon_button(
                                    "hide-project-sidebar",
                                    theme,
                                    false,
                                    false,
                                )
                                .tooltip(components::label_tooltip("Hide Projects", theme))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.visible = false;
                                    this.menu = None;
                                    cx.notify();
                                }))
                                .child(components::icon(IconName::PanelLeft, theme)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("project-filter-row")
                    .debug_selector(|| "project-filter-row".to_owned())
                    .h(px(40.))
                    .flex_none()
                    .px(theme.spacing.panel_padding)
                    .py(px(6.))
                    .flex()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .id("project-run-filter")
                            .debug_selector(|| "project-run-filter".to_owned())
                            .track_focus(&filter_focus)
                            .cursor_text()
                            .relative()
                            .px_3()
                            .h(theme.spacing.control_height)
                            .w_full()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .text_xs()
                            .rounded(theme.spacing.corner_radius)
                            .border_1()
                            .border_color(theme.colors.border)
                            .on_key_down(cx.listener(Self::on_filter_key))
                            .children((!filter_focused && filter_text.is_empty()).then(|| {
                                div()
                                    .id("project-run-filter-placeholder")
                                    .debug_selector(|| "project-run-filter-placeholder".to_owned())
                                    .flex_1()
                                    .truncate()
                                    .text_color(theme.colors.disabled)
                                    .child("Filter Projects and Runs")
                            }))
                            .children((!filter_text.is_empty()).then(|| {
                                div()
                                    .id("project-run-filter-prefix")
                                    .debug_selector(|| "project-run-filter-value".to_owned())
                                    .text_color(theme.colors.text)
                                    .child(filter_prefix)
                            }))
                            .children((filter_focused && filter_cursor_visible).then(|| {
                                div()
                                    .id("project-run-filter-caret")
                                    .debug_selector(|| "project-run-filter-caret".to_owned())
                                    .ml(px(1.))
                                    .w(px(1.))
                                    .h(px(14.))
                                    .bg(theme.colors.text)
                            }))
                            .children((!filter_text.is_empty()).then(|| {
                                div()
                                    .id("project-run-filter-suffix")
                                    .debug_selector(|| "project-run-filter-suffix".to_owned())
                                    .text_color(theme.colors.text)
                                    .child(filter_suffix)
                            }))
                            .child(TextInput::cursor_target(
                                filter_input,
                                filter_focus,
                                px(12.),
                            )),
                    ),
            )
            .child(resources)
            .child(archived_resources)
            .child(
                resize_handle(
                    SharedString::from("project-sidebar-resize"),
                    theme,
                    sidebar_resizing,
                    ResizeEdge::Right,
                )
                .debug_selector(|| "project-sidebar-resize".to_owned())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.begin_sidebar_resize(event, cx);
                    }),
                ),
            )
    }
}

fn sidebar_group_label(label: &str, theme: ViewerTheme) -> gpui::Div {
    div()
        .mt_3()
        .mb_1()
        .px_2()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.colors.text_muted)
        .child(label.to_owned())
}

fn sidebar_text_button(
    id: impl Into<gpui::ElementId>,
    label: &str,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .ml(px(RUN_TEXT_OFFSET))
        .h(theme.spacing.tree_row_height)
        .flex()
        .items_center()
        .cursor_pointer()
        .text_xs()
        .text_color(theme.colors.text_muted)
        .hover(|style| style.text_color(theme.colors.text))
        .child(label.to_owned())
}

fn unavailable_reference_row(
    id: String,
    label: String,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .h(theme.spacing.tree_row_height)
        .px_2()
        .flex()
        .items_center()
        .truncate()
        .text_xs()
        .text_color(theme.colors.disabled)
        .cursor_default()
        .child(label)
}

fn sidebar_menu_item(
    id: impl Into<gpui::ElementId>,
    label: &str,
    icon: IconName,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    components::popover_menu_item(id, label, Some(icon), theme)
}

fn project_information_card(
    project: &SidebarProject,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!(
            "project-information:{}",
            project.project_ref.project_id.as_str()
        )))
        .debug_selector({
            let project_ref = project.project_ref.clone();
            move || format!("project-information-{}", project_ref.project_id.as_str())
        })
        .w(px(300.))
        .p_2()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .flex()
        .flex_col()
        .gap_2()
        .text_xs()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .child(components::icon(IconName::Folder, theme)),
                )
                .child(
                    div()
                        .id("project-information-name")
                        .debug_selector(|| "project-information-name".to_owned())
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(project.project.name.clone()),
                )
                .children((project.placement != ProjectPlacement::Projects).then(|| {
                    div()
                        .id("project-information-placement-icon")
                        .debug_selector(|| "project-information-placement-icon".to_owned())
                        .flex_none()
                        .child(components::icon(
                            match project.placement {
                                ProjectPlacement::Pinned => IconName::Pin,
                                ProjectPlacement::Archived => IconName::Archive,
                                ProjectPlacement::Projects => IconName::Folder,
                            },
                            theme,
                        ))
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(components::icon(IconName::CirclePlay, theme))
                .child(format!("{} Runs", project.runs.len())),
        )
        .child(
            div()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(theme.colors.text_muted)
                .child(
                    div()
                        .flex_none()
                        .child(components::icon(IconName::Folder, theme)),
                )
                .child(
                    div()
                        .id("project-information-source")
                        .debug_selector(|| "project-information-source".to_owned())
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(project.source_label.clone()),
                ),
        )
}

impl ProjectSidebar {
    fn activate_tree_project(
        &mut self,
        source_id: DataSourceId,
        project_id: ProjectId,
        cx: &mut Context<Self>,
    ) {
        cx.emit(ProjectSidebarEvent::Command(
            WorkbenchCommand::ToggleProjectExpanded(ProjectRef::new(source_id, project_id)),
        ));
        cx.notify();
    }

    fn toggle_tree_run(&mut self, run_ref: RunRef, cx: &mut Context<Self>) {
        self.toggle_run(run_ref, cx);
    }

    fn toggle_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        cx.emit(ProjectSidebarEvent::Command(WorkbenchCommand::ToggleRun(
            run,
        )));
        cx.notify();
    }

    fn set_run_baseline(&mut self, run: RunRef, cx: &mut Context<Self>) {
        let is_baseline = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.views.active().baseline.as_ref() == Some(&run));
        let baseline = if is_baseline { None } else { Some(run) };
        cx.emit(ProjectSidebarEvent::Command(WorkbenchCommand::SetBaseline(
            baseline,
        )));
        cx.notify();
    }

    fn toggle_pinned_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        cx.emit(ProjectSidebarEvent::Command(
            WorkbenchCommand::TogglePinnedRun(run),
        ));
        cx.notify();
    }

    fn archive_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        let restoring = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.views.archived_runs().contains(&run));
        cx.emit(ProjectSidebarEvent::Command(
            WorkbenchCommand::SetRunArchived {
                run,
                archived: !restoring,
            },
        ));
        cx.notify();
    }

    fn set_project_placement(
        &mut self,
        project: ProjectRef,
        placement: ProjectPlacement,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        cx.emit(ProjectSidebarEvent::Command(
            WorkbenchCommand::SetProjectPlacement { project, placement },
        ));
        cx.notify();
    }

    fn remove_project(&mut self, project: ProjectRef, cx: &mut Context<Self>) {
        self.menu = None;
        cx.emit(ProjectSidebarEvent::RemoveProject(project));
        cx.notify();
    }

    fn manage_source(&mut self, source_id: DataSourceId, cx: &mut Context<Self>) {
        self.menu = None;
        cx.emit(ProjectSidebarEvent::ManageSource(source_id));
        cx.notify();
    }

    fn project_listing_runs(&self, project: &SidebarProject) -> Vec<RunRef> {
        let Some(session) = self.snapshot.as_ref() else {
            return Vec::new();
        };
        let baseline = session.views.active().baseline.as_ref();
        let pinned = &session.views.active().pinned_runs;
        let archived = session.views.archived_runs();
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
        let Some(session) = self.snapshot.as_ref() else {
            return;
        };
        let hide = !session.active_selection_has_capacity()
            || (movable
                .iter()
                .all(|run| session.views.active().runs.contains(run))
                && !movable.is_empty());
        self.menu = None;
        cx.emit(ProjectSidebarEvent::Command(
            WorkbenchCommand::SetProjectRuns {
                runs: movable,
                selected: !hide,
            },
        ));
        cx.notify();
    }

    fn begin_sidebar_resize(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.resize = Some(SidebarResize {
            start_x: event.position.x,
            start_width: self.width,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn move_sidebar_resize(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.resize else {
            return;
        };
        let maximum = (window.viewport_size().width - px(320.)).max(px(160.));
        self.width = (resize.start_width + event.position.x - resize.start_x)
            .clamp(px(160.), maximum.min(px(600.)));
        cx.notify();
    }

    pub(crate) fn finish_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if self.resize.take().is_some() {
            cx.notify();
        }
    }

    fn begin_archived_resize(
        &mut self,
        event: &MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let start_height = self
            .archived_height
            .unwrap_or(window.viewport_size().height - event.position.y);
        self.archived_height = Some(start_height);
        self.archived_resize = Some(ArchivedResize {
            start_y: event.position.y,
            start_height,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn move_archived_resize(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.archived_resize else {
            return;
        };
        let maximum = (window.viewport_size().height - px(120.)).max(px(56.));
        self.archived_height =
            Some((resize.start_height + resize.start_y - event.position.y).clamp(px(56.), maximum));
        cx.notify();
    }

    pub(crate) fn finish_archived_resize(&mut self, cx: &mut Context<Self>) {
        if self.archived_resize.take().is_some() {
            cx.notify();
        }
    }

    fn on_filter_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let filter = self.filter.clone();
        let handled = filter.update(cx, |input, cx| {
            let handled = if event.keystroke.key == "escape" {
                input.clear();
                true
            } else {
                input.edit(event).handled
            };
            if handled {
                input.start_blink(cx);
            }
            handled
        });
        if !handled {
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn set_hovered_project(&mut self, project: &ProjectRef, hovered: bool, cx: &mut Context<Self>) {
        let next = hovered.then(|| project.clone());
        if !hovered && self.hovered_project.as_ref() != Some(project) {
            return;
        }
        if self.hovered_project != next {
            self.hovered_project = next;
            cx.notify();
        }
    }
}

impl Render for ProjectSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().id("project-sidebar-hidden").w_0().h_full();
        }
        let theme = ViewerTheme::for_appearance(window.appearance());
        self.render_converged_project_sidebar(window, theme, cx)
    }
}

impl ViewerApp {
    pub(super) fn active_visible_runs(&self, cx: &App) -> Vec<RunRef> {
        let session = self.session_snapshot(cx);
        session.views.active_visible_runs().cloned().collect()
    }

    pub(super) fn set_hovered_run(
        &mut self,
        run: &RunRef,
        region: &SharedString,
        hovered: bool,
        exit_policy: RunHoverExitPolicy,
        cx: &mut Context<Self>,
    ) {
        self.run_hover_revision = self.run_hover_revision.wrapping_add(1);
        let revision = self.run_hover_revision;
        let current = self.interaction_snapshot(cx).emphasized_run;
        if hovered {
            self.hovered_run_region = Some((run.clone(), region.clone()));
            if current.as_ref() == Some(run) {
                return;
            }
            let run = run.clone();
            self.interaction.update(cx, |interaction, cx| {
                if interaction.set_emphasized_run(Some(run)) {
                    cx.notify();
                }
            });
            cx.notify();
            return;
        }
        if !self
            .hovered_run_region
            .as_ref()
            .is_some_and(|(hovered_run, hovered_region)| {
                hovered_run == run && hovered_region == region
            })
        {
            return;
        }
        self.hovered_run_region = None;
        if current.as_ref() != Some(run) {
            return;
        }
        let run = run.clone();
        match exit_policy {
            RunHoverExitPolicy::EndOfEvent => {
                let this = cx.entity().downgrade();
                cx.defer(move |cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.clear_hovered_run_after_exit(&run, revision, cx);
                    });
                });
            }
            RunHoverExitPolicy::OneFrameGrace => {
                let timer = cx.background_executor().timer(RUN_HOVER_GRACE);
                cx.spawn(async move |this, cx| {
                    timer.await;
                    let _ = this.update(cx, |this, cx| {
                        this.clear_hovered_run_after_exit(&run, revision, cx);
                    });
                })
                .detach();
            }
        }
    }

    fn clear_hovered_run_after_exit(
        &mut self,
        run: &RunRef,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        if self.run_hover_revision != revision
            || self.hovered_run_region.is_some()
            || self.interaction_snapshot(cx).emphasized_run.as_ref() != Some(run)
        {
            return;
        }
        self.interaction.update(cx, |interaction, cx| {
            if interaction.set_emphasized_run(None) {
                cx.notify();
            }
        });
        cx.notify();
    }

    pub(super) fn handle_project_sidebar_event(
        &mut self,
        event: &ProjectSidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ProjectSidebarEvent::Command(command) => {
                self.dispatch_workbench_command(command.clone(), cx);
            }
            ProjectSidebarEvent::ImportSource => self.open_source_import(window, cx),
            ProjectSidebarEvent::ManageSource(source_id) => {
                self.manage_source_projects(source_id.clone(), window, cx);
            }
            ProjectSidebarEvent::RemoveProject(project) => {
                self.confirm_remove_project(project.clone(), window, cx);
            }
            ProjectSidebarEvent::DismissOtherPopovers => {
                self.analysis_view_bar.update(cx, |bar, cx| {
                    if bar.menu.take().is_some() {
                        cx.notify();
                    }
                });
                self.workspace.update(cx, |workspace, cx| {
                    let changed = workspace.metric_picker_open || workspace.axis_picker_open;
                    workspace.metric_picker_open = false;
                    workspace.axis_picker_open = false;
                    if changed {
                        cx.notify();
                    }
                });
            }
            ProjectSidebarEvent::HoveredRun {
                run,
                region,
                hovered,
            } => {
                self.set_hovered_run(run, region, *hovered, RunHoverExitPolicy::EndOfEvent, cx);
            }
        }
    }
}
