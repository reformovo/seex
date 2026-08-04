use std::path::{Path, PathBuf};

use crate::config::{ConfiguredSource, SourceConfiguration, same_source_path};
use crate::data::SourcePreflight;
#[cfg(all(test, feature = "test-support"))]
use crate::data::registry::SourceStatus;
use crate::domain::{DataSourceId, RunRef, SourceAlias, suggest_source_alias};
use crate::workbench::ProjectRef;
use crate::workbench::import::{
    ImportSourceMapping, WorkbenchImportPlan, preflight_workbench_import,
    referenced_projects_by_alias,
};
use crate::workbench::toml_document::TomlWorkbenchDocument;
use gpui::{
    AnyWindowHandle, Context, FocusHandle, MouseButton, MouseMoveEvent, MouseUpEvent,
    PathPromptOptions, PromptLevel, Render, SharedString, Window, div, prelude::*,
};
use seex::ProjectId;

#[cfg(all(test, feature = "test-support"))]
use super::{ActivateSelection, SELECTABLE_CONTEXT};
use super::{ExportWorkbench, ImportWorkbench, OpenSources, Quit, ReloadSources};

#[path = "assets.rs"]
mod assets;
#[path = "chart/mod.rs"]
mod chart;
#[path = "command.rs"]
mod command;
#[path = "components/mod.rs"]
mod components;
#[path = "inspector/mod.rs"]
mod inspector;
#[path = "interaction.rs"]
mod interaction;
#[path = "project_sidebar/mod.rs"]
mod project_sidebar;
#[path = "session/mod.rs"]
mod session;
#[path = "source_management.rs"]
mod source_management;
#[path = "theme.rs"]
mod theme;
#[path = "view_bar.rs"]
mod view_bar;
#[path = "workspace/mod.rs"]
mod workspace;

#[cfg(all(test, feature = "test-support"))]
#[path = "test_support.rs"]
mod test_support;
#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;

pub(super) use assets::ViewerAssets;
use inspector::*;
use interaction::WorkbenchInteraction;
use project_sidebar::*;
use session::{
    SessionSnapshot, ViewerLayoutState, WorkbenchSession, WorkbenchSessionEvent,
    default_workbench_path,
};
use source_management::{
    ConfirmedSource, SourceBatchPlan, SourceManagement, SourceManagementEvent,
    SourcePreflightRequest,
};
use theme::ViewerTheme;
use view_bar::{AnalysisViewBar, AnalysisViewBarEvent};
use workspace::*;

pub(super) struct ViewerApp {
    theme: ViewerTheme,
    focus: FocusHandle,
    project_sidebar: gpui::Entity<ProjectSidebar>,
    interaction: gpui::Entity<WorkbenchInteraction>,
    analysis_view_bar: gpui::Entity<AnalysisViewBar>,
    bottom_inspector: gpui::Entity<BottomInspector>,
    session: gpui::Entity<WorkbenchSession>,
    workspace: gpui::Entity<AnalysisWorkspace>,
    source_management: gpui::Entity<SourceManagement>,
    source_configuration: Option<SourceConfiguration>,
    pending_workbench_import: Option<PendingWorkbenchImport>,
    pending_import_flush_revision: Option<u64>,
    pending_close_flush_revision: Option<u64>,
    pending_close_target: Option<CloseTarget>,
    source_reload_generation: u64,
    project_root: Option<PathBuf>,
    pending_commands: Vec<command::WorkbenchCommand>,
    command_dispatch_pending: bool,
    run_hover_revision: u64,
    hovered_run_region: Option<(RunRef, SharedString)>,
}

struct PendingWorkbenchImport {
    external_path: PathBuf,
    external_fingerprint: Vec<u8>,
    configuration: SourceConfiguration,
    plan: WorkbenchImportPlan,
}

#[derive(Clone, Copy)]
enum CloseTarget {
    Application,
    Window(AnyWindowHandle),
}

struct WorkbenchImportPreparation {
    external_path: PathBuf,
    external_fingerprint: Vec<u8>,
    configuration: SourceConfiguration,
    document: TomlWorkbenchDocument,
    local_sources: Vec<LocalSourceCandidate>,
    mappings: Vec<ImportSourceMapping>,
    unresolved: Vec<(SourceAlias, Vec<ProjectId>)>,
}

struct LocalSourceCandidate {
    source: ConfiguredSource,
    available_projects: Result<Vec<ProjectId>, String>,
}

impl ViewerApp {
    pub(super) fn new(
        project_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_home(project_path, None, window, cx)
    }

    #[cfg(all(test, feature = "test-support"))]
    fn new_for_test(
        project_path: Option<PathBuf>,
        home: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_home(project_path, Some(home), window, cx)
    }

    fn new_with_home(
        project_path: Option<PathBuf>,
        home: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let viewer = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let target = CloseTarget::Window(window.window_handle());
            viewer
                .update(cx, |viewer, cx| {
                    viewer.request_close(target, cx);
                    false
                })
                .unwrap_or(true)
        });
        let focus = cx.focus_handle();
        focus.focus(window);
        let session =
            cx.new(|_| WorkbenchSession::new(default_workbench_path(project_path.as_deref())));
        let mut app = Self {
            theme: ViewerTheme::for_appearance(window.appearance()),
            focus,
            project_sidebar: cx.new(ProjectSidebar::new),
            interaction: cx.new(|_| WorkbenchInteraction::default()),
            analysis_view_bar: cx.new(AnalysisViewBar::new),
            bottom_inspector: cx.new(|_| BottomInspector::new()),
            session,
            workspace: cx.new(AnalysisWorkspace::new),
            source_management: cx.new(SourceManagement::new),
            source_configuration: None,
            pending_workbench_import: None,
            pending_import_flush_revision: None,
            pending_close_flush_revision: None,
            pending_close_target: None,
            source_reload_generation: 0,
            project_root: project_path.clone(),
            pending_commands: Vec::new(),
            command_dispatch_pending: false,
            run_hover_revision: 0,
            hovered_run_region: None,
        };
        let (filter_focus, run_filter) = app.project_sidebar.read_with(cx, |sidebar, _| {
            (sidebar.filter_focus.clone(), sidebar.filter.clone())
        });
        let focused_filter = run_filter.clone();
        cx.on_focus(&filter_focus, window, move |_, _, cx| {
            focused_filter.update(cx, |input, cx| input.start_blink(cx));
            cx.notify();
        })
        .detach();
        let blurred_filter = run_filter.clone();
        cx.on_blur(&filter_focus, window, move |_, _, cx| {
            blurred_filter.update(cx, |input, cx| input.stop_blink(cx));
            cx.notify();
        })
        .detach();
        let view_name_focus = app.analysis_view_bar.read(cx).rename_focus.clone();
        let view_bar = app.analysis_view_bar.clone();
        cx.on_blur(&view_name_focus, window, move |_, _, cx| {
            let view_bar = view_bar.clone();
            cx.defer(move |cx| {
                view_bar.update(cx, |bar, cx| {
                    bar.finish_rename_analysis_view(true, cx);
                });
            });
        })
        .detach();
        cx.observe_in(&app.project_sidebar, window, |this, _, window, cx| {
            this.sync_child_snapshots(cx);
            this.sync_workspace_width(window, cx);
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &app.project_sidebar,
            window,
            |this, _, event: &ProjectSidebarEvent, window, cx| {
                this.handle_project_sidebar_event(event, window, cx);
            },
        )
        .detach();
        cx.observe_in(&app.workspace, window, |this, _, window, cx| {
            this.sync_child_snapshots(cx);
            this.sync_workspace_width(window, cx);
            cx.notify();
        })
        .detach();
        cx.subscribe(
            &app.workspace,
            |this, _, event: &AnalysisWorkspaceEvent, cx| {
                this.handle_workspace_event(event, cx);
            },
        )
        .detach();
        let metric_filter = app.workspace.read(cx).metric_filter.clone();
        cx.observe(&metric_filter, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(
            &app.analysis_view_bar,
            window,
            |this, _, event: &AnalysisViewBarEvent, window, cx| {
                this.handle_view_bar_event(event, window, cx);
            },
        )
        .detach();
        cx.subscribe(
            &app.bottom_inspector,
            |this, _, event: &BottomInspectorEvent, cx| {
                this.handle_bottom_inspector_event(event, cx);
            },
        )
        .detach();
        cx.observe(&app.bottom_inspector, |this, _, cx| {
            this.sync_child_snapshots(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&app.interaction, |this, _, cx| {
            this.sync_child_snapshots(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&app.source_management, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe_in(
            &app.source_management,
            window,
            |this, _, event: &SourceManagementEvent, window, cx| {
                this.handle_source_management_event(event, window, cx);
            },
        )
        .detach();
        cx.observe(&app.session, |this, _, cx| {
            this.sync_child_snapshots(cx);
            cx.notify();
        })
        .detach();
        cx.observe_window_bounds(window, |this, window, cx| {
            this.sync_workspace_width(window, cx);
        })
        .detach();
        cx.subscribe(&app.session, |_, _, event: &WorkbenchSessionEvent, cx| {
            let event = event.clone();
            let viewer = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = viewer.update(cx, |viewer, cx| {
                    viewer.handle_import_autosave_event(&event, cx);
                    viewer.handle_close_autosave_event(&event, cx);
                    viewer.handle_session_event(event, cx);
                });
            });
        })
        .detach();
        if let Some(path) = app.session.read(cx).workbench_path.clone() {
            match TomlWorkbenchDocument::load(&path) {
                Ok(Some(document)) => app.restore_toml_workbench(document, cx),
                Ok(None) => {}
                Err(error) => {
                    app.session.update(cx, |session, cx| {
                        session.transient_error = Some(error.to_string());
                        session.autosave_blocked = true;
                        session.publish_snapshot();
                        cx.notify();
                    });
                }
            }
        }
        let configuration = match home {
            Some(home) => {
                SourceConfiguration::load_for_scope_at(project_path.as_deref(), Some(home))
            }
            None => SourceConfiguration::load_for_scope(project_path.as_deref()),
        };
        match configuration {
            Ok(configuration) => {
                let sources = configuration.configured_sources();
                app.source_configuration = Some(configuration);
                app.open_configured_sources(sources, cx);
            }
            Err(error) => {
                app.session.update(cx, |session, cx| {
                    session.transient_error = Some(error.to_string());
                    session.publish_snapshot();
                    cx.notify();
                });
            }
        }
        app.sync_child_snapshots(cx);
        app.sync_workspace_width(window, cx);
        app
    }

    pub(crate) fn session_snapshot(&self, cx: &gpui::App) -> std::sync::Arc<SessionSnapshot> {
        self.session.read(cx).snapshot()
    }

    pub(crate) fn sync_child_snapshots(&mut self, cx: &mut Context<Self>) {
        let session = self.session_snapshot(cx);
        let sidebar_visible = self.sidebar_visible(cx);
        let sidebar_width = self.sidebar_width(cx);
        let inspector_visible = self.inspector_visible(cx);
        let interaction = self.interaction_snapshot(cx);
        let visible_runs = self.active_visible_runs(cx);
        self.analysis_view_bar.update(cx, |bar, _| {
            bar.sync(session.clone(), sidebar_visible, inspector_visible);
        });
        self.project_sidebar.update(cx, |sidebar, _| {
            sidebar.sync(session.clone(), interaction.clone());
        });
        self.bottom_inspector.update(cx, |inspector, _| {
            inspector.sync(session.clone(), interaction.clone(), visible_runs.clone());
        });
        let available_metrics = self.available_metric_keys(cx);
        let scheduled = self.workspace.update(cx, |workspace, cx| {
            workspace.sync(
                session,
                interaction,
                visible_runs,
                available_metrics,
                sidebar_visible,
                sidebar_width,
            );
            workspace.sync_track_charts(cx);
            workspace.reconcile_track_schedule(cx)
        });
        for request in scheduled {
            self.request_panel_detail(
                &request.panel_id,
                request.viewport,
                request.logical_width,
                cx,
            );
        }
        let layout = ViewerLayoutState {
            project_sidebar_visible: sidebar_visible,
            project_sidebar_width: f32::from(sidebar_width),
            metric_sidebar_compact: self.workspace.read(cx).metric_sidebar_compact,
            bottom_inspector_visible: inspector_visible,
            bottom_inspector_height: f32::from(self.bottom_inspector.read(cx).height),
        };
        if self.session.read(cx).needs_layout_sync_or_persist(layout) {
            self.session.update(cx, |session, cx| {
                session.sync_layout_and_persist(layout, cx);
            });
        }
    }

    fn sync_workspace_width(&mut self, window: &Window, cx: &mut Context<Self>) {
        let theme = ViewerTheme::for_appearance(window.appearance());
        let project_width = if self.sidebar_visible(cx) {
            self.sidebar_width(cx)
        } else {
            gpui::px(0.)
        };
        let metric_width = self.workspace.read(cx).metric_sidebar_width(theme);
        let logical_width = f32::from(
            (window.viewport_size().width - project_width - metric_width).max(gpui::px(1.)),
        );
        let physical_width = (logical_width * window.scale_factor()).ceil().max(1.) as u32;
        self.workspace.update(cx, |workspace, cx| {
            workspace.update_overview_widths(logical_width, physical_width, cx);
        });
    }

    fn open_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let existing_sources = self
            .source_configuration
            .as_ref()
            .map(SourceConfiguration::configured_sources)
            .unwrap_or_default();
        let existing_aliases = self
            .source_configuration
            .as_ref()
            .map(|configuration| {
                configuration
                    .sources
                    .iter()
                    .map(|source| source.configured.alias.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let requests = self.source_management.update(cx, |management, cx| {
            management.begin_sources(existing_sources, existing_aliases, window, cx)
        });
        self.spawn_source_preflights(requests, window, cx);
    }

    fn choose_source_directories(
        &mut self,
        replace: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: replace.is_none(),
            prompt: Some("Add Sources".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match prompt.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    let _ = this.update_in(cx, |viewer, _, cx| {
                        viewer.source_management.update(cx, |management, cx| {
                            management.report_source_selection_error(error.to_string(), cx);
                        });
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update_in(cx, |viewer, _, cx| {
                        viewer.source_management.update(cx, |management, cx| {
                            management.report_source_selection_error(error.to_string(), cx);
                        });
                    });
                    return;
                }
            };
            if paths.is_empty() {
                return;
            }
            let _ = this.update_in(cx, |viewer, window, cx| {
                let requests = viewer.source_management.update(cx, |management, cx| {
                    if let Some(draft_id) = replace {
                        paths
                            .into_iter()
                            .next()
                            .and_then(|path| management.replace_source_path(draft_id, path, cx))
                            .into_iter()
                            .collect()
                    } else {
                        management.queue_source_paths(paths, cx)
                    }
                });
                viewer.spawn_source_preflights(requests, window, cx);
            });
        })
        .detach();
    }

    fn spawn_source_preflights(
        &mut self,
        requests: Vec<SourcePreflightRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for request in requests {
            let path = request.root_path.clone();
            let preflight = cx.background_spawn(async move { SourcePreflight::load(&path) });
            cx.spawn_in(window, async move |this, cx| {
                let result = preflight.await.map_err(|error| error.to_string());
                let _ = this.update_in(cx, |viewer, window, cx| {
                    viewer.source_management.update(cx, |management, cx| {
                        management.finish_preflight(request, result, window, cx);
                    });
                });
            })
            .detach();
        }
    }

    #[cfg(all(test, feature = "test-support"))]
    fn open_source_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_sources(window, cx);
    }

    fn on_open_sources(&mut self, _: &OpenSources, window: &mut Window, cx: &mut Context<Self>) {
        self.open_sources(window, cx);
    }

    fn on_reload_sources(
        &mut self,
        _: &ReloadSources,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reload_sources(cx);
    }

    fn on_export_workbench(
        &mut self,
        _: &ExportWorkbench,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let directory = self
            .project_root
            .clone()
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let prompt = cx.prompt_for_new_path(&directory, Some("workbench.toml"));
        let snapshot = self.session.read(cx).semantic_snapshot();
        cx.spawn_in(window, async move |this, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    let _ = this.update_in(cx, |viewer, _, cx| {
                        viewer.report_source_error(error.to_string(), cx);
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update_in(cx, |viewer, _, cx| {
                        viewer.report_source_error(error.to_string(), cx);
                    });
                    return;
                }
            };
            let save = cx.background_spawn(async move { snapshot.document.save(&path) });
            if let Err(error) = save.await {
                let _ = this.update_in(cx, |viewer, _, cx| {
                    viewer.report_source_error(error.to_string(), cx);
                });
            }
        })
        .detach();
    }

    fn on_import_workbench(
        &mut self,
        _: &ImportWorkbench,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import Workbench".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match prompt.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) => return,
                Ok(Err(error)) => {
                    let _ = this.update_in(cx, |viewer, _, cx| {
                        viewer.report_source_error(error.to_string(), cx);
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update_in(cx, |viewer, _, cx| {
                        viewer.report_source_error(error.to_string(), cx);
                    });
                    return;
                }
            };
            if let Some(path) = paths.into_iter().next() {
                let _ = this.update_in(cx, |viewer, window, cx| {
                    viewer.preflight_workbench_path(path, window, cx);
                });
            }
        })
        .detach();
    }

    fn preflight_workbench_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(configuration) = self.source_configuration.clone() else {
            self.report_source_error("Viewer configuration is unavailable".to_owned(), cx);
            return;
        };
        let preparation =
            cx.background_spawn(async move { prepare_workbench_import(path, configuration) });
        cx.spawn_in(window, async move |this, cx| {
            let result = async {
                let preparation = preparation.await?;
                let mut selections = Vec::with_capacity(preparation.unresolved.len());
                for (external_alias, _) in &preparation.unresolved {
                    let prompt = cx
                        .update(|_, cx| {
                            cx.prompt_for_paths(PathPromptOptions {
                                files: false,
                                directories: true,
                                multiple: false,
                                prompt: Some(format!("Map Source {external_alias}").into()),
                            })
                        })
                        .map_err(|error| error.to_string())?;
                    let paths = prompt
                        .await
                        .map_err(|error| error.to_string())?
                        .map_err(|error| error.to_string())?;
                    let Some(path) = paths.and_then(|paths| paths.into_iter().next()) else {
                        return Ok(None);
                    };
                    let preflight = cx
                        .background_spawn(async move { SourcePreflight::load(&path) })
                        .await
                        .map_err(|error| error.to_string())?;
                    selections.push((external_alias.clone(), preflight));
                }
                finish_workbench_import(preparation, selections).map(Some)
            }
            .await;
            match result {
                Ok(Some(pending)) => {
                    let _ = this.update_in(cx, |viewer, _, cx| {
                        viewer.source_management.update(cx, |management, cx| {
                            management.begin_workbench(pending.plan.clone(), cx);
                        });
                        viewer.pending_workbench_import = Some(pending);
                        cx.notify();
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = this.update_in(cx, |viewer, _, cx| {
                        viewer.report_source_error(error, cx);
                    });
                }
            }
        })
        .detach();
    }

    fn reload_sources(&mut self, cx: &mut Context<Self>) {
        let generation = self.begin_source_reload();
        let project_root = self.project_root.clone();
        let load = cx.background_spawn(async move { load_and_preflight_sources(project_root) });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |viewer, cx| {
                viewer.finish_source_reload(generation, result, cx);
            });
        })
        .detach();
    }

    fn begin_source_reload(&mut self) -> u64 {
        self.source_reload_generation = self.source_reload_generation.saturating_add(1);
        self.source_reload_generation
    }

    fn finish_source_reload(
        &mut self,
        generation: u64,
        result: Result<(SourceConfiguration, Vec<ConfiguredSource>), String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.source_reload_generation {
            return;
        }
        match result {
            Ok((configuration, sources)) => {
                self.source_configuration = Some(configuration);
                let visible_runs = self.active_visible_runs(cx);
                self.session.update(cx, |session, session_cx| {
                    session.replace_sources(sources, &visible_runs, session_cx);
                });
                cx.notify();
            }
            Err(error) => {
                self.report_source_error(error, cx);
            }
        }
    }

    #[cfg(all(test, feature = "test-support"))]
    fn begin_source_reload_for_test(&mut self) -> u64 {
        self.begin_source_reload()
    }

    #[cfg(all(test, feature = "test-support"))]
    fn finish_source_reload_for_test(
        &mut self,
        generation: u64,
        configuration: SourceConfiguration,
        cx: &mut Context<Self>,
    ) {
        self.finish_source_reload(generation, Ok((configuration, Vec::new())), cx);
    }

    fn handle_source_management_event(
        &mut self,
        event: &SourceManagementEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SourceManagementEvent::ChooseSources(replace) => {
                self.choose_source_directories(*replace, window, cx);
            }
            SourceManagementEvent::Confirmed(plan) => {
                self.confirm_source_batch(plan.clone(), window, cx);
            }
            SourceManagementEvent::ConfirmedWorkbench => self.confirm_workbench_import(cx),
            SourceManagementEvent::Cancelled => {
                self.pending_workbench_import = None;
                self.pending_import_flush_revision = None;
            }
        }
    }

    fn confirm_source_batch(
        &mut self,
        plan: SourceBatchPlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut candidate) = self.source_configuration.clone() else {
            self.source_management.update(cx, |management, cx| {
                management.finish_save(Some("Viewer configuration is unavailable".to_owned()), cx);
            });
            return;
        };
        let removed_aliases = candidate
            .sources
            .iter()
            .filter(|source| {
                plan.removed_roots
                    .iter()
                    .any(|root| same_source_path(root, &source.configured.root_path))
            })
            .map(|source| source.configured.alias.clone())
            .collect::<Vec<_>>();
        let removed_sources = removed_aliases
            .iter()
            .map(DataSourceId::from_alias)
            .collect::<Vec<_>>();
        let mut removed_projects = Vec::<ProjectRef>::new();
        for source in &candidate.sources {
            let replacement = plan
                .updates
                .iter()
                .find(|update| update.alias == source.configured.alias);
            let projects = if removed_aliases.contains(&source.configured.alias) {
                source.configured.projects.iter().collect::<Vec<_>>()
            } else if let Some(replacement) = replacement {
                source
                    .configured
                    .projects
                    .iter()
                    .filter(|project| !replacement.projects.contains(project))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            for project in projects {
                let project = ProjectRef::new(
                    DataSourceId::from_alias(&source.configured.alias),
                    project.clone(),
                );
                if !removed_projects.contains(&project) {
                    removed_projects.push(project);
                }
            }
        }
        let update = (|| {
            for alias in &removed_aliases {
                candidate.remove_source(alias)?;
            }
            for source in &plan.updates {
                candidate.set_source(&source.alias, &source.root_path, &source.projects)?;
            }
            Ok::<(), crate::config::ConfigEditError>(())
        })();
        if let Err(error) = update {
            self.source_management.update(cx, |management, cx| {
                management.finish_save(Some(error.to_string()), cx);
            });
            return;
        }
        if removed_projects.is_empty() {
            self.save_source_batch(candidate, removed_sources, removed_projects, cx);
            return;
        }
        let message = format!(
            "This unimports {} Project{} and removes all of their Workbench references.",
            removed_projects.len(),
            if removed_projects.len() == 1 { "" } else { "s" }
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            "Remove Sources or Projects?",
            Some(&message),
            &["Remove", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update_in(cx, |viewer, _, cx| {
                    viewer.save_source_batch(candidate, removed_sources, removed_projects, cx);
                });
            } else {
                let _ = this.update_in(cx, |viewer, _, cx| {
                    viewer.source_management.update(cx, |management, cx| {
                        management.finish_save(None, cx);
                    });
                });
            }
        })
        .detach();
    }

    fn save_source_batch(
        &mut self,
        mut candidate: SourceConfiguration,
        removed_sources: Vec<DataSourceId>,
        removed_projects: Vec<ProjectRef>,
        cx: &mut Context<Self>,
    ) {
        let save = cx.background_spawn(async move { candidate.save().map(|()| candidate) });
        cx.spawn(async move |this, cx| match save.await {
            Ok(configuration) => {
                let _ = this.update(cx, |viewer, cx| {
                    let sources = configuration.configured_sources();
                    let visible_runs = viewer.active_visible_runs(cx);
                    viewer.source_configuration = Some(configuration);
                    viewer.session.update(cx, |session, session_cx| {
                        for source_id in &removed_sources {
                            session.views.remove_source(source_id);
                        }
                        for project in &removed_projects {
                            session.views.remove_project(project.clone());
                        }
                        if !removed_sources.is_empty() || !removed_projects.is_empty() {
                            session.persistence_dirty = true;
                        }
                        session.replace_sources(sources, &visible_runs, session_cx);
                        if !removed_sources.is_empty() || !removed_projects.is_empty() {
                            session.publish_semantic_snapshot();
                        }
                    });
                    viewer.source_management.update(cx, |management, cx| {
                        management.complete_save(cx);
                    });
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = this.update(cx, |viewer, cx| {
                    viewer.source_management.update(cx, |management, cx| {
                        management.finish_save(Some(error.to_string()), cx);
                    });
                });
            }
        })
        .detach();
    }

    fn confirm_workbench_import(&mut self, cx: &mut Context<Self>) {
        if self.pending_workbench_import.is_none() {
            let error = "Workbench import plan is unavailable".to_owned();
            self.source_management.update(cx, |management, cx| {
                management.finish_workbench_save(error.clone(), cx);
            });
            self.report_source_error(error, cx);
            return;
        }
        let flush_revision = self
            .session
            .update(cx, |session, cx| session.flush_pending_revision(cx));
        self.pending_import_flush_revision = flush_revision;
        if flush_revision.is_none() {
            self.persist_workbench_import(cx);
        }
    }

    fn handle_import_autosave_event(
        &mut self,
        event: &WorkbenchSessionEvent,
        cx: &mut Context<Self>,
    ) {
        let WorkbenchSessionEvent::AutosaveFinished {
            revision,
            succeeded,
        } = event
        else {
            return;
        };
        let Some(target) = self.pending_import_flush_revision else {
            return;
        };
        if !succeeded {
            self.pending_import_flush_revision = None;
            let error = "Could not flush the current Workbench".to_owned();
            self.source_management.update(cx, |management, cx| {
                management.finish_workbench_save(error.clone(), cx);
            });
            self.report_source_error(error, cx);
            return;
        }
        if *revision < target {
            return;
        }
        let next_revision = self
            .session
            .update(cx, |session, cx| session.flush_pending_revision(cx));
        if let Some(revision) = next_revision {
            self.pending_import_flush_revision = Some(revision);
        } else {
            self.pending_import_flush_revision = None;
            self.persist_workbench_import(cx);
        }
    }

    fn on_quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        self.request_close(CloseTarget::Application, cx);
    }

    fn request_close(&mut self, target: CloseTarget, cx: &mut Context<Self>) {
        if self.pending_close_flush_revision.is_some() {
            if matches!(target, CloseTarget::Application) {
                self.pending_close_target = Some(CloseTarget::Application);
            }
            return;
        }
        let flush_revision = self
            .session
            .update(cx, |session, cx| session.flush_pending_revision(cx));
        if let Some(revision) = flush_revision {
            self.pending_close_flush_revision = Some(revision);
            self.pending_close_target = Some(target);
        } else {
            Self::finish_close(target, cx);
        }
    }

    fn handle_close_autosave_event(
        &mut self,
        event: &WorkbenchSessionEvent,
        cx: &mut Context<Self>,
    ) {
        let WorkbenchSessionEvent::AutosaveFinished {
            revision,
            succeeded,
        } = event
        else {
            return;
        };
        let Some(target_revision) = self.pending_close_flush_revision else {
            return;
        };
        if !succeeded {
            self.pending_close_flush_revision = None;
            self.pending_close_target = None;
            self.report_source_error(
                "Could not flush the Workbench before closing".to_owned(),
                cx,
            );
            return;
        }
        if *revision < target_revision {
            return;
        }
        let next_revision = self
            .session
            .update(cx, |session, cx| session.flush_pending_revision(cx));
        if let Some(revision) = next_revision {
            self.pending_close_flush_revision = Some(revision);
        } else {
            self.pending_close_flush_revision = None;
            if let Some(target) = self.pending_close_target.take() {
                Self::finish_close(target, cx);
            }
        }
    }

    fn finish_close(target: CloseTarget, cx: &mut Context<Self>) {
        match target {
            CloseTarget::Application => cx.quit(),
            CloseTarget::Window(window_handle) => cx.defer(move |cx| {
                let _ = window_handle.update(cx, |_, window, _| window.remove_window());
            }),
        }
    }

    fn persist_workbench_import(&mut self, cx: &mut Context<Self>) {
        let Some(workbench_path) = self.session.read(cx).workbench_path.clone() else {
            let error = "Current Workbench path is unavailable".to_owned();
            self.source_management.update(cx, |management, cx| {
                management.finish_workbench_save(error.clone(), cx);
            });
            self.report_source_error(error, cx);
            return;
        };
        let Some(mut pending) = self.pending_workbench_import.take() else {
            return;
        };
        let write = cx.background_spawn(async move {
            let result = (|| {
                if std::fs::read(&pending.external_path).map_err(|error| error.to_string())?
                    != pending.external_fingerprint
                {
                    return Err("Imported Workbench changed after preflight".to_owned());
                }
                let original = match std::fs::read(&workbench_path) {
                    Ok(bytes) => Some(bytes),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.to_string()),
                };
                pending
                    .configuration
                    .save_with_workbench(&workbench_path, original, &pending.plan.document)
                    .map_err(|error| error.to_string())
            })();
            match result {
                Ok(()) => Ok(pending),
                Err(error) => Err((pending, error)),
            }
        });
        cx.spawn(async move |this, cx| match write.await {
            Ok(pending) => {
                let _ = this.update(cx, |viewer, cx| {
                    let sources = pending.configuration.configured_sources();
                    viewer.source_configuration = Some(pending.configuration);
                    viewer.session.update(cx, |session, session_cx| {
                        session.autosave_blocked = false;
                        session.replace_sources(sources, &[], session_cx);
                    });
                    viewer.restore_toml_workbench(pending.plan.document, cx);
                    viewer.source_management.update(cx, |management, cx| {
                        management.complete_workbench_save(cx);
                    });
                    cx.notify();
                });
            }
            Err((pending, error)) => {
                let _ = this.update(cx, |viewer, cx| {
                    viewer.pending_workbench_import = Some(pending);
                    viewer.source_management.update(cx, |management, cx| {
                        management.finish_workbench_save(error.clone(), cx);
                    });
                    viewer.report_source_error(error, cx);
                });
            }
        })
        .detach();
    }

    fn save_confirmed_source(&mut self, source: ConfirmedSource, cx: &mut Context<Self>) {
        let Some(mut candidate) = self.source_configuration.clone() else {
            self.report_source_error("Viewer configuration is unavailable".to_owned(), cx);
            return;
        };
        let existing = candidate
            .sources
            .iter()
            .find(|configured| configured.configured.alias == source.alias)
            .map(|configured| configured.configured.clone());
        if !source.manage && existing.is_some() {
            self.report_source_error(
                format!("Source alias {} is already configured", source.alias),
                cx,
            );
            return;
        }
        let update = if source.projects.is_empty() {
            candidate.remove_source(&source.alias)
        } else {
            candidate.set_source(&source.alias, &source.root_path, &source.projects)
        };
        if let Err(error) = update {
            self.report_source_error(error.to_string(), cx);
            return;
        }
        let removed_projects = existing
            .as_ref()
            .map(|existing| {
                existing
                    .projects
                    .iter()
                    .filter(|project| !source.projects.contains(project))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let source_id = DataSourceId::from_alias(&source.alias);
        let configured = (!source.projects.is_empty()).then_some(ConfiguredSource {
            alias: source.alias,
            root_path: source.root_path,
            projects: source.projects,
        });
        let save = cx.background_spawn(async move { candidate.save().map(|()| candidate) });
        cx.spawn(async move |this, cx| match save.await {
            Ok(configuration) => {
                let _ = this.update(cx, |viewer, cx| {
                    viewer.source_configuration = Some(configuration);
                    viewer.session.update(cx, |session, session_cx| {
                        for project_id in &removed_projects {
                            session.views.remove_project(ProjectRef::new(
                                source_id.clone(),
                                project_id.clone(),
                            ));
                        }
                        if !removed_projects.is_empty() {
                            session.persistence_dirty = true;
                        }
                        if configured.is_none() {
                            session.sources.remove(&source_id);
                            session.event_tasks.remove(&source_id);
                        }
                        if removed_projects.is_empty() {
                            session.publish_snapshot();
                        } else {
                            session.publish_semantic_snapshot();
                        }
                        session_cx.notify();
                    });
                    if let Some(configured) = configured {
                        viewer.open_configured_sources(vec![configured], cx);
                    }
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = this.update(cx, |viewer, cx| {
                    viewer.report_source_error(error.to_string(), cx);
                });
            }
        })
        .detach();
    }

    #[cfg(all(test, feature = "test-support"))]
    fn manage_source_projects(
        &mut self,
        _source_id: DataSourceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_sources(window, cx);
    }

    fn confirm_remove_project(
        &mut self,
        project: ProjectRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            "Remove Project?",
            Some("This unimports the Project and removes its Workbench references."),
            &["Remove", "Cancel"],
            cx,
        );
        let configured = self
            .source_configuration
            .as_ref()
            .and_then(|configuration| {
                configuration
                    .sources
                    .iter()
                    .find(|source| source.configured.alias == *project.source_id.alias())
                    .map(|source| source.configured.clone())
            });
        cx.spawn_in(window, async move |this, cx| {
            if !matches!(answer.await, Ok(0)) {
                return;
            }
            let _ = this.update_in(cx, |viewer, _, cx| {
                let Some(configured) = configured else {
                    viewer
                        .report_source_error("Source configuration is unavailable".to_owned(), cx);
                    return;
                };
                let projects = configured
                    .projects
                    .into_iter()
                    .filter(|project_id| project_id != &project.project_id)
                    .collect();
                viewer.save_confirmed_source(
                    ConfirmedSource {
                        manage: true,
                        alias: configured.alias,
                        root_path: configured.root_path,
                        projects,
                    },
                    cx,
                );
            });
        })
        .detach();
    }

    fn report_source_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| {
            session.transient_error = Some(message);
            session.publish_snapshot();
            cx.notify();
        });
    }
}

fn load_and_preflight_sources(
    project_root: Option<PathBuf>,
) -> Result<(SourceConfiguration, Vec<ConfiguredSource>), String> {
    let configuration = SourceConfiguration::load_for_scope(project_root.as_deref())
        .map_err(|error| error.to_string())?;
    let sources = configuration.configured_sources();
    for source in &sources {
        let preflight =
            SourcePreflight::load(&source.root_path).map_err(|error| error.to_string())?;
        if let Some(project_id) = source.projects.iter().find(|project_id| {
            !preflight
                .projects
                .iter()
                .any(|project| &project.project_id == *project_id)
        }) {
            return Err(format!(
                "Source {} does not contain Project {}",
                source.alias,
                project_id.as_str()
            ));
        }
    }
    Ok((configuration, sources))
}

fn prepare_workbench_import(
    external_path: PathBuf,
    configuration: SourceConfiguration,
) -> Result<WorkbenchImportPreparation, String> {
    let external_fingerprint = std::fs::read(&external_path).map_err(|error| error.to_string())?;
    let raw = std::str::from_utf8(&external_fingerprint)
        .map_err(|_| "Imported Workbench is not UTF-8".to_owned())?;
    let document = TomlWorkbenchDocument::decode(raw).map_err(|error| error.to_string())?;
    let mut referenced = referenced_projects_by_alias(&document)
        .into_iter()
        .collect::<Vec<_>>();
    referenced.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));

    let local_sources = configuration
        .configured_sources()
        .into_iter()
        .map(|source| LocalSourceCandidate {
            available_projects: preflight_import_candidate(&source),
            source,
        })
        .collect::<Vec<_>>();

    let mut mappings = Vec::new();
    let mut unresolved = Vec::new();
    for (external_alias, required_projects) in referenced {
        let candidates = local_sources
            .iter()
            .filter_map(|candidate| {
                candidate
                    .available_projects
                    .as_ref()
                    .ok()
                    .filter(|available| {
                        required_projects
                            .iter()
                            .all(|project| available.contains(project))
                    })
                    .map(|available| (candidate, available))
            })
            .collect::<Vec<_>>();
        let selected = candidates
            .iter()
            .copied()
            .find(|(candidate, _)| candidate.source.alias == external_alias)
            .or_else(|| (candidates.len() == 1).then(|| candidates[0]));
        if let Some((candidate, available_projects)) = selected {
            mappings.push(ImportSourceMapping {
                external_alias,
                local_alias: candidate.source.alias.clone(),
                available_projects: available_projects.clone(),
                imported_projects: candidate.source.projects.clone(),
            });
        } else {
            unresolved.push((external_alias, required_projects));
        }
    }
    Ok(WorkbenchImportPreparation {
        external_path,
        external_fingerprint,
        configuration,
        document,
        local_sources,
        mappings,
        unresolved,
    })
}

fn preflight_import_candidate(source: &ConfiguredSource) -> Result<Vec<ProjectId>, String> {
    let preflight = SourcePreflight::load(&source.root_path).map_err(|error| error.to_string())?;
    let available = preflight
        .projects
        .into_iter()
        .map(|project| project.project_id)
        .collect::<Vec<_>>();
    if let Some(project_id) = source
        .projects
        .iter()
        .find(|project_id| !available.contains(project_id))
    {
        return Err(format!(
            "Source {} does not contain Project {}",
            source.alias,
            project_id.as_str()
        ));
    }
    Ok(available)
}

fn finish_workbench_import(
    mut preparation: WorkbenchImportPreparation,
    selections: Vec<(SourceAlias, SourcePreflight)>,
) -> Result<PendingWorkbenchImport, String> {
    let mut new_sources = Vec::<(SourceAlias, PathBuf)>::new();
    let mut used_aliases = preparation
        .configuration
        .sources
        .iter()
        .map(|source| source.configured.alias.clone())
        .collect::<Vec<_>>();
    for (external_alias, required_projects) in &preparation.unresolved {
        let selected = selections
            .iter()
            .find(|(alias, _)| alias == external_alias)
            .map(|(_, preflight)| preflight)
            .ok_or_else(|| format!("Source {external_alias} is not mapped"))?;
        let available_projects = selected
            .projects
            .iter()
            .map(|project| project.project_id.clone())
            .collect::<Vec<_>>();
        if let Some(project_id) = required_projects
            .iter()
            .find(|project| !available_projects.contains(project))
        {
            return Err(format!(
                "Source {} does not contain Project {}",
                external_alias,
                project_id.as_str()
            ));
        }
        let existing = preparation
            .local_sources
            .iter()
            .find(|candidate| same_source_path(&candidate.source.root_path, &selected.root_path));
        let (local_alias, imported_projects) = if let Some(candidate) = existing {
            (
                candidate.source.alias.clone(),
                candidate.source.projects.clone(),
            )
        } else if let Some((alias, _)) = new_sources
            .iter()
            .find(|(_, root_path)| same_source_path(root_path, &selected.root_path))
        {
            (alias.clone(), Vec::new())
        } else {
            let alias = if used_aliases.contains(external_alias) {
                suggest_source_alias(&selected.root_path, &used_aliases)
            } else {
                external_alias.clone()
            };
            used_aliases.push(alias.clone());
            new_sources.push((alias.clone(), selected.root_path.clone()));
            (alias, Vec::new())
        };
        preparation.mappings.push(ImportSourceMapping {
            external_alias: external_alias.clone(),
            local_alias,
            available_projects,
            imported_projects,
        });
    }
    preparation.mappings.sort_by(|left, right| {
        left.external_alias
            .as_str()
            .cmp(right.external_alias.as_str())
    });
    let plan = preflight_workbench_import(preparation.document, &preparation.mappings)
        .map_err(|error| error.to_string())?;
    for (alias, additions) in &plan.allowlist_additions {
        let existing = preparation
            .configuration
            .sources
            .iter()
            .find(|source| &source.configured.alias == alias)
            .map(|source| source.configured.clone());
        let (root_path, mut projects) = if let Some(source) = existing {
            (source.root_path, source.projects)
        } else {
            let root_path = new_sources
                .iter()
                .find(|(new_alias, _)| new_alias == alias)
                .map(|(_, root_path)| root_path.clone())
                .ok_or_else(|| format!("Source {alias} is unavailable"))?;
            (root_path, Vec::new())
        };
        projects.extend(additions.iter().cloned());
        preparation
            .configuration
            .set_source(alias, &root_path, &projects)
            .map_err(|error| error.to_string())?;
    }
    Ok(PendingWorkbenchImport {
        external_path: preparation.external_path,
        external_fingerprint: preparation.external_fingerprint,
        configuration: preparation.configuration,
        plan,
    })
}

impl Render for ViewerApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = ViewerTheme::for_appearance(window.appearance());
        let theme = self.theme;
        let session = self.session_snapshot(cx);
        let has_sources = !session.sources.is_empty();
        let _session_revision = session.revision;
        let _active_view = session.views.active();
        let inspector_visible = self.inspector_visible(cx);
        div()
            .track_focus(&self.focus)
            .tab_group()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if event.dragging() {
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.move_metric_resize(event, cx);
                    });
                    this.bottom_inspector.update(cx, |inspector, cx| {
                        inspector.move_inspector_resize(event, window, cx);
                        inspector.move_inspector_column_resize(event, cx);
                    });
                    this.project_sidebar.update(cx, |sidebar, cx| {
                        sidebar.move_sidebar_resize(event, window, cx);
                        sidebar.move_archived_resize(event, window, cx);
                    });
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.finish_metric_resize(cx));
                    this.bottom_inspector.update(cx, |inspector, cx| {
                        inspector.finish_inspector_resize(cx);
                        inspector.finish_inspector_column_resize(cx);
                    });
                    this.project_sidebar.update(cx, |sidebar, cx| {
                        sidebar.finish_sidebar_resize(cx);
                        sidebar.finish_archived_resize(cx);
                    });
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| workspace.finish_metric_resize(cx));
                    this.bottom_inspector.update(cx, |inspector, cx| {
                        inspector.finish_inspector_resize(cx);
                        inspector.finish_inspector_column_resize(cx);
                    });
                    this.project_sidebar.update(cx, |sidebar, cx| {
                        sidebar.finish_sidebar_resize(cx);
                        sidebar.finish_archived_resize(cx);
                    });
                }),
            )
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_open_sources))
            .on_action(cx.listener(Self::on_reload_sources))
            .on_action(cx.listener(Self::on_import_workbench))
            .on_action(cx.listener(Self::on_export_workbench))
            .on_action(cx.listener(Self::on_quit))
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
                    .child(self.project_sidebar.clone())
                    .child(
                        div()
                            .id("analysis-workspace")
                            .debug_selector(|| "analysis-workspace".to_owned())
                            .flex()
                            .flex_col()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(self.analysis_view_bar.clone())
                            .child(if has_sources {
                                self.workspace.clone().into_any_element()
                            } else {
                                div().flex_1().into_any_element()
                            })
                            .children(
                                (has_sources && inspector_visible)
                                    .then(|| self.bottom_inspector.clone()),
                            ),
                    ),
            )
            .child(self.source_management.clone())
    }
}
