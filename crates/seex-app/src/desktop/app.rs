use std::path::PathBuf;

use crate::config::{ConfiguredSource, SourceConfiguration};
use crate::data::SourcePreflight;
#[cfg(all(test, feature = "test-support"))]
use crate::data::registry::SourceStatus;
use crate::domain::{DataSourceId, RunRef, suggest_source_alias};
use crate::workbench::ProjectRef;
use crate::workbench::import::{
    ImportSourceMapping, WorkbenchImportPlan, preflight_workbench_import,
    referenced_projects_by_alias,
};
use crate::workbench::toml_document::TomlWorkbenchDocument;
use gpui::{
    Context, FocusHandle, MouseButton, MouseMoveEvent, MouseUpEvent, PathPromptOptions,
    PromptLevel, Render, SharedString, Window, div, prelude::*,
};

#[cfg(all(test, feature = "test-support"))]
use super::{ActivateSelection, SELECTABLE_CONTEXT};
use super::{ExportWorkbench, ImportSource, ImportWorkbench, ReloadSources};

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
use source_management::{ConfirmedSource, SourceManagement, SourceManagementEvent};
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

impl ViewerApp {
    pub(super) fn new(
        project_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
        match SourceConfiguration::load_for_scope(project_path.as_deref()) {
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

    fn choose_source_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let existing = self
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
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Import Source".into()),
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
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let preflight = cx.background_spawn(async move { SourcePreflight::load(&path) });
            let result = preflight.await;
            let _ = this.update_in(cx, |viewer, window, cx| match result {
                Ok(preflight) => {
                    let alias = suggest_source_alias(&preflight.root_path, &existing);
                    viewer.source_management.update(cx, |management, cx| {
                        management.begin(preflight, alias, window, cx);
                    });
                }
                Err(error) => viewer.report_source_error(error.to_string(), cx),
            });
        })
        .detach();
    }

    fn on_import_source(&mut self, _: &ImportSource, window: &mut Window, cx: &mut Context<Self>) {
        self.choose_source_directory(window, cx);
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
                let _ = this.update_in(cx, |viewer, _, cx| {
                    viewer.preflight_workbench_path(path, cx);
                });
            }
        })
        .detach();
    }

    fn preflight_workbench_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(configuration) = self.source_configuration.clone() else {
            self.report_source_error("Viewer configuration is unavailable".to_owned(), cx);
            return;
        };
        let preflight =
            cx.background_spawn(async move { prepare_workbench_import(path, configuration) });
        cx.spawn(async move |this, cx| match preflight.await {
            Ok(pending) => {
                let _ = this.update(cx, |viewer, cx| {
                    viewer.source_management.update(cx, |management, cx| {
                        management.begin_workbench(pending.plan.clone(), cx);
                    });
                    viewer.pending_workbench_import = Some(pending);
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = this.update(cx, |viewer, cx| viewer.report_source_error(error, cx));
            }
        })
        .detach();
    }

    fn reload_sources(&mut self, cx: &mut Context<Self>) {
        let project_root = self.project_root.clone();
        let load = cx.background_spawn(async move { load_and_preflight_sources(project_root) });
        cx.spawn(async move |this, cx| match load.await {
            Ok((configuration, sources)) => {
                let _ = this.update(cx, |viewer, cx| {
                    viewer.source_configuration = Some(configuration);
                    let visible_runs = viewer.active_visible_runs(cx);
                    viewer.session.update(cx, |session, session_cx| {
                        session.replace_sources(sources, &visible_runs, session_cx);
                    });
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = this.update(cx, |viewer, cx| viewer.report_source_error(error, cx));
            }
        })
        .detach();
    }

    fn handle_source_management_event(
        &mut self,
        event: &SourceManagementEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SourceManagementEvent::Confirmed(source) => {
                self.confirm_managed_source(source.clone(), window, cx);
            }
            SourceManagementEvent::ConfirmedWorkbench => self.confirm_workbench_import(cx),
            SourceManagementEvent::Cancelled => {
                self.pending_workbench_import = None;
            }
        }
    }

    fn confirm_managed_source(
        &mut self,
        source: ConfirmedSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let removes_projects = source.manage
            && self
                .source_configuration
                .as_ref()
                .is_some_and(|configuration| {
                    configuration.sources.iter().any(|configured| {
                        configured.configured.alias == source.alias
                            && configured
                                .configured
                                .projects
                                .iter()
                                .any(|project| !source.projects.contains(project))
                    })
                });
        if !removes_projects {
            self.save_confirmed_source(source, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Remove Projects?",
            Some("This unimports the deselected Projects and removes their Workbench references."),
            &["Remove", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if matches!(answer.await, Ok(0)) {
                let _ = this.update_in(cx, |viewer, _, cx| {
                    viewer.save_confirmed_source(source, cx);
                });
            }
        })
        .detach();
    }

    fn confirm_workbench_import(&mut self, cx: &mut Context<Self>) {
        if self.pending_workbench_import.is_none() {
            self.report_source_error("Workbench import plan is unavailable".to_owned(), cx);
            return;
        }
        let flush_revision = self.session.update(cx, |session, cx| {
            if session.autosave_blocked || !session.persistence_dirty {
                None
            } else {
                let revision = session.semantic_snapshot().revision;
                session.flush_now(cx);
                Some(revision)
            }
        });
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
            self.pending_workbench_import = None;
            self.report_source_error("Could not flush the current Workbench".to_owned(), cx);
            return;
        }
        if *revision < target {
            return;
        }
        let next_revision = self.session.update(cx, |session, cx| {
            session.persistence_dirty.then(|| {
                let revision = session.semantic_snapshot().revision;
                session.flush_now(cx);
                revision
            })
        });
        if let Some(revision) = next_revision {
            self.pending_import_flush_revision = Some(revision);
        } else {
            self.pending_import_flush_revision = None;
            self.persist_workbench_import(cx);
        }
    }

    fn persist_workbench_import(&mut self, cx: &mut Context<Self>) {
        let Some(mut pending) = self.pending_workbench_import.take() else {
            return;
        };
        let Some(workbench_path) = self.session.read(cx).workbench_path.clone() else {
            self.report_source_error("Current Workbench path is unavailable".to_owned(), cx);
            return;
        };
        let write = cx.background_spawn(async move {
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
                .map_err(|error| error.to_string())?;
            Ok(pending)
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
                    cx.notify();
                });
            }
            Err(error) => {
                let _ = this.update(cx, |viewer, cx| viewer.report_source_error(error, cx));
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

    fn manage_source_projects(
        &mut self,
        source_id: DataSourceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .session_snapshot(cx)
            .sources
            .iter()
            .find(|source| source.source_id == source_id)
            .cloned()
        else {
            self.report_source_error(format!("Source {source_id} is unavailable"), cx);
            return;
        };
        let root_path = source.root_path.clone();
        let preflight = cx.background_spawn(async move { SourcePreflight::load(&root_path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = preflight.await;
            let _ = this.update_in(cx, |viewer, window, cx| match result {
                Ok(preflight) => {
                    viewer.source_management.update(cx, |management, cx| {
                        management.begin_manage(
                            preflight,
                            source.source_id.alias().clone(),
                            &source.project_allowlist,
                            window,
                            cx,
                        );
                    });
                }
                Err(error) => viewer.report_source_error(error.to_string(), cx),
            });
        })
        .detach();
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
    mut configuration: SourceConfiguration,
) -> Result<PendingWorkbenchImport, String> {
    let external_fingerprint = std::fs::read(&external_path).map_err(|error| error.to_string())?;
    let raw = std::str::from_utf8(&external_fingerprint)
        .map_err(|_| "Imported Workbench is not UTF-8".to_owned())?;
    let document = TomlWorkbenchDocument::decode(raw).map_err(|error| error.to_string())?;
    let mut referenced = referenced_projects_by_alias(&document)
        .into_iter()
        .collect::<Vec<_>>();
    referenced.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));

    let mut local_sources = Vec::new();
    for source in configuration.configured_sources() {
        let preflight =
            SourcePreflight::load(&source.root_path).map_err(|error| error.to_string())?;
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
        local_sources.push((source, available));
    }

    let mut mappings = Vec::new();
    for (external_alias, required_projects) in referenced {
        let candidates = local_sources
            .iter()
            .filter(|(_, available)| {
                required_projects
                    .iter()
                    .all(|project| available.contains(project))
            })
            .collect::<Vec<_>>();
        let selected = candidates
            .iter()
            .copied()
            .find(|(source, _)| source.alias == external_alias)
            .or_else(|| (candidates.len() == 1).then(|| candidates[0]))
            .ok_or_else(|| {
                format!(
                    "Source {} cannot be mapped uniquely to a local Source",
                    external_alias
                )
            })?;
        mappings.push(ImportSourceMapping {
            external_alias,
            local_alias: selected.0.alias.clone(),
            available_projects: selected.1.clone(),
            imported_projects: selected.0.projects.clone(),
        });
    }
    let plan =
        preflight_workbench_import(document, &mappings).map_err(|error| error.to_string())?;
    for (alias, additions) in &plan.allowlist_additions {
        let source = configuration
            .sources
            .iter()
            .find(|source| &source.configured.alias == alias)
            .map(|source| source.configured.clone())
            .ok_or_else(|| format!("Source {alias} is unavailable"))?;
        let mut projects = source.projects;
        projects.extend(additions.iter().cloned());
        configuration
            .set_source(alias, &source.root_path, &projects)
            .map_err(|error| error.to_string())?;
    }
    Ok(PendingWorkbenchImport {
        external_path,
        external_fingerprint,
        configuration,
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
            .on_action(cx.listener(Self::on_import_source))
            .on_action(cx.listener(Self::on_reload_sources))
            .on_action(cx.listener(Self::on_import_workbench))
            .on_action(cx.listener(Self::on_export_workbench))
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
