use std::path::{Path, PathBuf};

use gpui::{AppContext, Context};

use crate::domain::{DataSourceId, RunRef, SourceAlias};
use crate::workbench::ProjectRef;
use crate::workbench::document::WorkbenchDocument;
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

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "retained until legacy Viewer tests migrate")
    )]
    pub(crate) fn restore_document(
        &mut self,
        document: WorkbenchDocument,
        cx: &mut Context<Self>,
    ) -> RestoredWorkbench {
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
        let (views, issues) = crate::workbench::AnalysisViews::restore(&document);
        self.views = views;
        self.last_saved_workbench = Some(document.encode());
        self.persistence_dirty = false;
        self.layout = ViewerLayoutState {
            project_sidebar_visible: document.project_sidebar_visible,
            project_sidebar_width: document.project_sidebar_width.clamp(160., 600.),
            metric_sidebar_compact: document.metric_sidebar_compact,
            bottom_inspector_visible: document.bottom_inspector_visible,
            bottom_inspector_height: document.bottom_inspector_height.clamp(56., 2_000.),
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
        self.publish_snapshot();
        cx.notify();
        RestoredWorkbench {
            layout: self.layout,
            available_source_ids,
        }
    }

    pub(crate) fn restore_toml_document(
        &mut self,
        document: crate::workbench::toml_document::TomlWorkbenchDocument,
        cx: &mut Context<Self>,
    ) -> RestoredWorkbench {
        let (views, issues) = crate::workbench::AnalysisViews::restore_toml(&document);
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
        self.publish_snapshot();
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
        self.layout = layout;
        let Some(path) = self.workbench_path.clone() else {
            return;
        };
        let document = self.workbench_document(layout);
        let encoded = document.encode();
        if self.last_saved_workbench.as_deref() == Some(&encoded) {
            self.persistence_dirty = false;
            return;
        }
        self.last_saved_workbench = Some(encoded);
        self.persistence_dirty = false;
        let save = cx.background_spawn(async move { document.save(&path) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = save.await {
                let _ = this.update(cx, |session, cx| {
                    session.transient_error = Some(error.to_string());
                    session.persistence_dirty = true;
                    session.publish_snapshot();
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn workbench_document(&self, layout: ViewerLayoutState) -> TomlWorkbenchDocument {
        let save_project = |project: &ProjectRef| {
            SourceAlias::new(project.source_id.as_str())
                .ok()
                .map(|source_alias| TomlProjectRef {
                    source_alias,
                    project_id: project.project_id.clone(),
                })
        };
        let save_run = |run: &RunRef| {
            SourceAlias::new(run.source_id.as_str())
                .ok()
                .map(|source_alias| TomlRunRef {
                    source_alias,
                    project_id: run.project_id.clone(),
                    run_id: run.run_id.clone(),
                })
        };
        let views = self
            .views
            .views()
            .iter()
            .map(|view| TomlAnalysisView {
                name: view.name.clone(),
                runs: view.runs.iter().filter_map(&save_run).collect(),
                baseline: view.baseline.as_ref().and_then(&save_run),
                pinned_runs: view.pinned_runs.iter().filter_map(&save_run).collect(),
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
                axis: view.navigation.axis(),
                viewport: view.navigation.brush().map(|brush| brush.selected()),
            })
            .collect();
        TomlWorkbenchDocument {
            active_view: self.views.active_index(),
            layout: SavedLayout {
                project_sidebar_visible: layout.project_sidebar_visible,
                project_sidebar_width: layout.project_sidebar_width,
                metric_sidebar_compact: layout.metric_sidebar_compact,
                bottom_inspector_visible: layout.bottom_inspector_visible,
                bottom_inspector_height: layout.bottom_inspector_height,
            },
            expanded_projects: Vec::new(),
            pinned_projects: self
                .views
                .pinned_projects()
                .iter()
                .filter_map(&save_project)
                .collect(),
            archived_projects: self
                .views
                .archived_projects()
                .iter()
                .filter_map(&save_project)
                .collect(),
            archived_runs: self
                .views
                .archived_runs()
                .iter()
                .filter_map(&save_run)
                .collect(),
            views,
        }
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
