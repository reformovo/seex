use std::path::PathBuf;

use crate::config::{ConfiguredSource, SourceConfiguration};
use crate::data::SourcePreflight;
#[cfg(all(test, feature = "test-support"))]
use crate::data::registry::SourceStatus;
use crate::domain::{DataSourceId, RunRef, suggest_source_alias};
use crate::workbench::ProjectRef;
use crate::workbench::toml_document::TomlWorkbenchDocument;
use gpui::{
    Context, FocusHandle, MouseButton, MouseMoveEvent, MouseUpEvent, PathPromptOptions,
    PromptLevel, Render, SharedString, Window, div, prelude::*,
};

use super::ImportSource;
#[cfg(all(test, feature = "test-support"))]
use super::{ActivateSelection, SELECTABLE_CONTEXT};

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
use session::{SessionSnapshot, ViewerLayoutState, WorkbenchSession, default_workbench_path};
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
    pending_commands: Vec<command::WorkbenchCommand>,
    command_dispatch_pending: bool,
    run_hover_revision: u64,
    hovered_run_region: Option<(RunRef, SharedString)>,
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
        cx.subscribe(
            &app.source_management,
            |this, _, event: &SourceManagementEvent, cx| {
                this.handle_source_management_event(event, cx);
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
        cx.subscribe(&app.session, |_, _, event, cx| {
            let event = event.clone();
            let viewer = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = viewer.update(cx, |viewer, cx| {
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
                        session.workbench_path = None;
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

    fn handle_source_management_event(
        &mut self,
        event: &SourceManagementEvent,
        cx: &mut Context<Self>,
    ) {
        if let SourceManagementEvent::Confirmed(source) = event {
            self.save_confirmed_source(source.clone(), cx);
        }
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
        let configured = (!source.projects.is_empty()).then(|| ConfiguredSource {
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
                        session.publish_snapshot();
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
