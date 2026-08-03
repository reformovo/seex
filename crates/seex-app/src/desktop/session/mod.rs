use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, EventEmitter, Task};

use crate::data::registry::SourceRegistry;
use crate::data::worker::{Generation, ReadEvent, ReadEventReceiver, ReadKind, ReadRequest};
use crate::domain::{DataSourceId, MAX_SELECTED_RUNS, RunRef};
use crate::workbench::AnalysisViews;
use crate::workbench::panel_reads::{PanelReadCoordinator, PanelReadOutcome};

mod persistence;
mod reads;

pub(super) use persistence::{ViewerLayoutState, default_workbench_path};

#[derive(Clone)]
pub(crate) struct SessionSnapshot {
    pub revision: u64,
    pub views: AnalysisViews,
    pub sources: Arc<[crate::data::registry::ImportedSource]>,
}

#[derive(Clone)]
pub(crate) struct SemanticWorkbenchSnapshot {
    pub revision: u64,
    pub document: crate::workbench::toml_document::TomlWorkbenchDocument,
}

impl SessionSnapshot {
    pub(crate) fn contains_catalog_project(
        &self,
        project_ref: &crate::workbench::ProjectRef,
    ) -> bool {
        self.sources.iter().any(|source| {
            source.source_id == project_ref.source_id
                && source
                    .catalog
                    .projects
                    .iter()
                    .any(|project| project.project_id == project_ref.project_id)
        })
    }

    pub(crate) fn contains_catalog_run(&self, run_ref: &RunRef) -> bool {
        self.sources.iter().any(|source| {
            source.source_id == run_ref.source_id
                && source
                    .catalog
                    .runs
                    .iter()
                    .any(|run| run.project_id == run_ref.project_id && run.run_id == run_ref.run_id)
        })
    }

    pub(crate) fn active_catalog_run_count(&self) -> usize {
        self.catalog_visible_run_count(self.views.active())
    }

    pub(crate) fn unavailable_references(
        &self,
    ) -> (Vec<crate::workbench::ProjectRef>, Vec<RunRef>) {
        let mut runs = HashSet::new();
        for view in self.views.views() {
            runs.extend(view.runs.iter().cloned());
            runs.extend(view.baseline.iter().cloned());
            runs.extend(view.pinned_runs.iter().cloned());
        }
        runs.extend(self.views.archived_runs().iter().cloned());
        let unavailable_runs = runs
            .iter()
            .filter(|run| !self.contains_catalog_run(run))
            .cloned()
            .collect::<Vec<_>>();
        let mut projects = self
            .views
            .pinned_projects()
            .iter()
            .chain(self.views.archived_projects())
            .chain(self.views.expanded_projects())
            .cloned()
            .collect::<HashSet<_>>();
        projects.extend(
            runs.into_iter()
                .map(|run| crate::workbench::ProjectRef::new(run.source_id, run.project_id)),
        );
        let unavailable_projects = projects
            .into_iter()
            .filter(|project| !self.contains_catalog_project(project))
            .collect();
        (unavailable_projects, unavailable_runs)
    }

    pub(crate) fn active_selection_has_capacity(&self) -> bool {
        self.active_catalog_run_count() < MAX_SELECTED_RUNS
    }

    pub(crate) fn has_capacity_for_runs(&self, runs: &[RunRef]) -> bool {
        let active_view_id = &self.views.active().view_id;
        let archived = self.views.archived_runs();
        let candidates = runs
            .iter()
            .filter(|run| self.contains_catalog_run(run))
            .fold(Vec::new(), |mut unique, run| {
                if !unique.contains(run) {
                    unique.push((*run).clone());
                }
                unique
            });
        self.views.views().iter().all(|view| {
            let mut visible = view
                .runs
                .iter()
                .filter(|run| self.contains_catalog_run(run) && !archived.contains(run))
                .cloned()
                .collect::<Vec<_>>();
            for run in &candidates {
                let becomes_visible = &view.view_id == active_view_id || view.runs.contains(run);
                if becomes_visible && !visible.contains(run) {
                    visible.push(run.clone());
                }
            }
            visible.len() <= MAX_SELECTED_RUNS
        })
    }

    fn catalog_visible_run_count(&self, view: &crate::workbench::AnalysisView) -> usize {
        let archived = self.views.archived_runs();
        view.runs
            .iter()
            .filter(|run| self.contains_catalog_run(run) && !archived.contains(run))
            .fold(Vec::new(), |mut unique, run| {
                if !unique.contains(run) {
                    unique.push((*run).clone());
                }
                unique
            })
            .len()
    }
}

pub(crate) struct WorkbenchSession {
    pub views: AnalysisViews,
    pub panel_reads: PanelReadCoordinator,
    pub sources: SourceRegistry,
    pub event_tasks: HashMap<DataSourceId, Task<()>>,
    pub next_generation: u64,
    pub transient_error: Option<String>,
    pub workbench_path: Option<PathBuf>,
    pub last_saved_workbench: Option<String>,
    pub layout: ViewerLayoutState,
    pub persistence_dirty: bool,
    pub autosave_blocked: bool,
    autosave_in_flight: bool,
    flush_requested: bool,
    autosave_task: Option<Task<()>>,
    snapshot: Arc<SessionSnapshot>,
    semantic_snapshot: Arc<SemanticWorkbenchSnapshot>,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionReadEffect {
    pub kind: ReadKind,
    pub accepted: bool,
    pub succeeded: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum WorkbenchSessionEvent {
    ReadApplied(SessionReadEffect),
    AutosaveFinished { revision: u64, succeeded: bool },
}

impl EventEmitter<WorkbenchSessionEvent> for WorkbenchSession {}

impl WorkbenchSession {
    pub fn new(workbench_path: Option<PathBuf>) -> Self {
        let views = AnalysisViews::default();
        let sources = SourceRegistry::default();
        let snapshot = Arc::new(SessionSnapshot {
            revision: 0,
            views: views.clone(),
            sources: sources.snapshot(),
        });
        let layout = ViewerLayoutState::default();
        let semantic_snapshot = Arc::new(SemanticWorkbenchSnapshot {
            revision: 0,
            document: persistence::workbench_document(&views, layout),
        });
        Self {
            views,
            panel_reads: PanelReadCoordinator::default(),
            sources,
            event_tasks: HashMap::new(),
            next_generation: 1,
            transient_error: None,
            workbench_path,
            last_saved_workbench: None,
            layout,
            persistence_dirty: false,
            autosave_blocked: false,
            autosave_in_flight: false,
            flush_requested: false,
            autosave_task: None,
            snapshot,
            semantic_snapshot,
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<SessionSnapshot> {
        Arc::clone(&self.snapshot)
    }

    pub(crate) fn semantic_snapshot(&self) -> Arc<SemanticWorkbenchSnapshot> {
        Arc::clone(&self.semantic_snapshot)
    }

    pub(crate) fn publish_semantic_snapshot(&mut self) {
        self.semantic_snapshot = Arc::new(SemanticWorkbenchSnapshot {
            revision: self.semantic_snapshot.revision.saturating_add(1),
            document: persistence::workbench_document(&self.views, self.layout),
        });
        self.publish_snapshot();
    }

    pub(crate) fn publish_snapshot(&mut self) {
        self.snapshot = Arc::new(SessionSnapshot {
            revision: self.snapshot.revision.saturating_add(1),
            views: self.views.clone(),
            sources: self.sources.snapshot(),
        });
    }

    fn allocate_generation(&mut self) -> Generation {
        let generation = Generation(self.next_generation);
        self.next_generation = self.next_generation.saturating_add(1);
        generation
    }

    fn submit(
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
                self.transient_error = Some(error.to_string());
                self.publish_snapshot();
                cx.notify();
                return;
            }
        }
        if let Err(error) = self.sources.submit(&source_id, generation, request) {
            self.transient_error = Some(error.to_string());
        }
        self.publish_snapshot();
        cx.notify();
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
                    .update(cx, |session, cx| {
                        let effect = session.apply_read_event(event);
                        session.publish_snapshot();
                        cx.emit(WorkbenchSessionEvent::ReadApplied(effect));
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

    fn apply_read_event(&mut self, event: ReadEvent) -> SessionReadEffect {
        let kind = event.kind;
        let succeeded = event.result.is_ok();
        self.sources.apply_event(&event);
        if !matches!(
            kind,
            ReadKind::Overview | ReadKind::Detail | ReadKind::Inspector
        ) {
            return SessionReadEffect {
                kind,
                accepted: succeeded,
                succeeded,
            };
        }
        let PanelReadOutcome::Completed(completed) = self.panel_reads.apply(event) else {
            return SessionReadEffect {
                kind,
                accepted: false,
                succeeded,
            };
        };
        if completed.tag.view_id != self.views.active().view_id {
            return SessionReadEffect {
                kind,
                accepted: false,
                succeeded,
            };
        }
        let panel_id = completed.tag.panel_id.clone();
        let metric_key = self
            .views
            .active_panel(&panel_id)
            .map(|panel| panel.metric_key.clone());
        if !completed.source_errors.is_empty() {
            self.transient_error = Some(
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
                && let Some(home) = self.views.record_active_metric_extent(metric_key, extent)
            {
                self.views.active_mut().navigation.set_timeline_home(home);
            }
        }
        SessionReadEffect {
            kind,
            accepted,
            succeeded,
        }
    }
}
