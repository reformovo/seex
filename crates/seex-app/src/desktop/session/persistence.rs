use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{AppContext, Context};

use crate::domain::{DataSourceId, RunRef};
use crate::workbench::ProjectRef;
use crate::workbench::toml_document::{
    SavedAnalysisView as TomlAnalysisView, SavedLayout, SavedProjectRef as TomlProjectRef,
    SavedRunRef as TomlRunRef, TomlWorkbenchDocument,
};

use super::WorkbenchSession;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ViewerLayoutState {
    pub project_sidebar_visible: bool,
    pub project_sidebar_width: f32,
    pub metric_sidebar_compact: bool,
    pub bottom_inspector_visible: bool,
    pub bottom_inspector_height: f32,
}

impl Default for ViewerLayoutState {
    fn default() -> Self {
        Self {
            project_sidebar_visible: true,
            project_sidebar_width: 190.,
            metric_sidebar_compact: false,
            bottom_inspector_visible: false,
            bottom_inspector_height: 220.,
        }
    }
}

pub(crate) struct RestoredWorkbench {
    pub layout: ViewerLayoutState,
    pub available_source_ids: Vec<DataSourceId>,
}

impl WorkbenchSession {
    pub(crate) fn needs_layout_sync_or_persist(&self, layout: ViewerLayoutState) -> bool {
        self.persistence_dirty || self.layout != layout
    }

    pub(crate) fn restore_toml_document(
        &mut self,
        document: crate::workbench::toml_document::TomlWorkbenchDocument,
        cx: &mut Context<Self>,
    ) -> RestoredWorkbench {
        let (views, issues) = crate::workbench::AnalysisViews::restore(&document);
        self.views = views;
        self.last_saved_workbench = Some(document.encode());
        self.persistence_dirty = false;
        self.layout = ViewerLayoutState {
            project_sidebar_visible: document.layout.project_sidebar_visible,
            project_sidebar_width: document.layout.project_sidebar_width.clamp(160., 600.),
            metric_sidebar_compact: document.layout.metric_sidebar_compact,
            bottom_inspector_visible: document.layout.bottom_inspector_visible,
            bottom_inspector_height: document.layout.bottom_inspector_height.clamp(56., 2_000.),
        };
        if !issues.is_empty() {
            self.transient_error = Some(issues.join("; "));
        }
        let available_source_ids = self
            .sources
            .sources()
            .filter(|source| source.root_path.is_dir())
            .map(|source| source.source_id.clone())
            .collect();
        self.publish_semantic_snapshot();
        cx.notify();
        RestoredWorkbench {
            layout: self.layout,
            available_source_ids,
        }
    }

    pub(crate) fn sync_layout_and_persist(
        &mut self,
        layout: ViewerLayoutState,
        cx: &mut Context<Self>,
    ) {
        if self.layout != layout {
            self.layout = layout;
            self.persistence_dirty = true;
            self.publish_semantic_snapshot();
        }
        self.schedule_autosave(Duration::from_millis(250), cx);
    }

    pub(crate) fn flush_now(&mut self, cx: &mut Context<Self>) {
        self.flush_requested = true;
        self.schedule_autosave(Duration::ZERO, cx);
    }

    pub(crate) fn flush_pending_revision(&mut self, cx: &mut Context<Self>) -> Option<u64> {
        if self.autosave_blocked || !self.persistence_dirty || self.workbench_path.is_none() {
            return None;
        }
        let revision = self.semantic_snapshot().revision;
        self.flush_now(cx);
        self.persistence_dirty.then_some(revision)
    }

    fn schedule_autosave(&mut self, delay: Duration, cx: &mut Context<Self>) {
        if self.autosave_blocked || self.autosave_in_flight || !self.persistence_dirty {
            return;
        }
        let Some(path) = self.workbench_path.clone() else {
            return;
        };
        let snapshot = self.semantic_snapshot();
        let encoded = snapshot.document.encode();
        if self.last_saved_workbench.as_deref() == Some(&encoded) {
            self.persistence_dirty = false;
            self.flush_requested = false;
            return;
        }
        let timer = cx.background_executor().timer(delay);
        self.autosave_task = Some(cx.spawn(async move |this, cx| {
            timer.await;
            let request = this
                .update(cx, |session, _| {
                    if session.autosave_blocked || session.autosave_in_flight {
                        return None;
                    }
                    session.autosave_in_flight = true;
                    Some((snapshot, path, encoded))
                })
                .ok()
                .flatten();
            let Some((snapshot, path, encoded)) = request else {
                return;
            };
            let revision = snapshot.revision;
            let save = cx.background_spawn(async move { snapshot.document.save(&path) });
            let result = save.await;
            let _ = this.update(cx, |session, cx| {
                session.autosave_in_flight = false;
                let succeeded = result.is_ok();
                if let Err(error) = result {
                    session.transient_error = Some(error.to_string());
                    session.persistence_dirty = true;
                } else {
                    session.last_saved_workbench = Some(encoded);
                    session.persistence_dirty = session.semantic_snapshot.revision != revision;
                }
                cx.emit(super::WorkbenchSessionEvent::AutosaveFinished {
                    revision,
                    succeeded,
                });
                session.publish_snapshot();
                cx.notify();
                if succeeded && session.persistence_dirty {
                    let delay = if session.flush_requested {
                        Duration::ZERO
                    } else {
                        Duration::from_millis(250)
                    };
                    let weak = cx.entity().downgrade();
                    cx.defer(move |cx| {
                        let _ = weak.update(cx, |session, cx| {
                            session.schedule_autosave(delay, cx);
                        });
                    });
                } else if succeeded {
                    session.flush_requested = false;
                }
            });
        }));
    }
}

pub(super) fn workbench_document(
    views_state: &crate::workbench::AnalysisViews,
    layout: ViewerLayoutState,
) -> TomlWorkbenchDocument {
    let save_project = |project: &ProjectRef| TomlProjectRef {
        source_alias: project.source_id.alias().clone(),
        project_id: project.project_id.clone(),
    };
    let save_run = |run: &RunRef| TomlRunRef {
        source_alias: run.source_id.alias().clone(),
        project_id: run.project_id.clone(),
        run_id: run.run_id.clone(),
    };
    let views = views_state
        .views()
        .iter()
        .map(|view| TomlAnalysisView {
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
                .and_then(|panel_id| view.panels.iter().find(|panel| &panel.panel_id == panel_id))
                .map(|panel| panel.metric_key.as_str().to_owned()),
            axis: view.navigation.axis(),
            viewport: view.navigation.brush().map(|brush| brush.selected()),
        })
        .collect();
    TomlWorkbenchDocument {
        active_view: views_state.active_index(),
        layout: SavedLayout {
            project_sidebar_visible: layout.project_sidebar_visible,
            project_sidebar_width: layout.project_sidebar_width,
            metric_sidebar_compact: layout.metric_sidebar_compact,
            bottom_inspector_visible: layout.bottom_inspector_visible,
            bottom_inspector_height: layout.bottom_inspector_height,
        },
        expanded_projects: views_state
            .expanded_projects()
            .iter()
            .map(&save_project)
            .collect(),
        pinned_projects: views_state
            .pinned_projects()
            .iter()
            .map(&save_project)
            .collect(),
        archived_projects: views_state
            .archived_projects()
            .iter()
            .map(&save_project)
            .collect(),
        archived_runs: views_state.archived_runs().iter().map(&save_run).collect(),
        views,
    }
}

pub(crate) fn default_workbench_path(_project_root: Option<&Path>) -> Option<PathBuf> {
    #[cfg(test)]
    return None;

    #[cfg(not(test))]
    _project_root
        .map(Path::to_owned)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .map(|root| root.join(".seex/workbench.toml"))
}
