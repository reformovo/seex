use std::collections::HashMap;
use std::sync::Arc;

use pulseon_model::alignment::AlignmentViewport;
use pulseon_model::comparison::ObjectiveDirection;
use pulseon_model::metric::MetricKey;
use pulseon_model::run::RunId;
use pulseon_model::types::ProjectId;

use crate::coordination::{AnalysisViewId, MetricPanelId, SourceReadFailure};
use crate::core::{DataSourceId, RunRef, SelectionError, ViewerCore, ViewerSelection};
use crate::query::{CurveSnapshot, InspectorSnapshot};
use crate::workbench_document::WorkbenchDocument;
use crate::worker::{Generation, ReadKind};

pub const DEFAULT_METRIC_ROW_HEIGHT: f32 = 52.;
const MAX_METRIC_ROW_HEIGHT: f32 = 180.;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrackDensity {
    Compact,
    #[default]
    Comfortable,
    Spacious,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InspectorTab {
    #[default]
    Summary,
    Ranking,
    Evidence,
}

/// Collision-free viewer-local identity for a Project in an imported source.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProjectRef {
    pub source_id: DataSourceId,
    pub project_id: ProjectId,
}

impl ProjectRef {
    pub const fn new(source_id: DataSourceId, project_id: ProjectId) -> Self {
        Self {
            source_id,
            project_id,
        }
    }
}

#[derive(Clone)]
pub struct MetricPanel {
    pub panel_id: MetricPanelId,
    pub metric_key: MetricKey,
    pub row_height: f32,
    pub overview: Option<Arc<CurveSnapshot>>,
    pub detail: Option<Arc<CurveSnapshot>>,
    pub source_errors: Vec<SourceReadFailure>,
    pub overview_generation: Option<Generation>,
    pub detail_generation: Option<Generation>,
    pub overview_revision: u64,
    pub detail_revision: u64,
    pub physical_width: u32,
    pub requested_detail_viewport: Option<AlignmentViewport>,
    pub inspector: Option<Arc<InspectorSnapshot>>,
    pub inspector_generation: Option<Generation>,
    pub inspector_errors: Vec<SourceReadFailure>,
}

impl MetricPanel {
    fn new(metric_key: MetricKey) -> Self {
        Self {
            panel_id: MetricPanelId::from_string(metric_key.as_str()),
            metric_key,
            row_height: DEFAULT_METRIC_ROW_HEIGHT,
            overview: None,
            detail: None,
            source_errors: Vec::new(),
            overview_generation: None,
            detail_generation: None,
            overview_revision: 0,
            detail_revision: 0,
            physical_width: 1_000,
            requested_detail_viewport: None,
            inspector: None,
            inspector_generation: None,
            inspector_errors: Vec::new(),
        }
    }

    pub fn is_pending(&self, kind: ReadKind) -> bool {
        match kind {
            ReadKind::Overview => self.overview_generation.is_some(),
            ReadKind::Detail => self.detail_generation.is_some(),
            ReadKind::Inspector => self.inspector_generation.is_some(),
            ReadKind::Catalog => false,
        }
    }

    pub fn needs_detail(&self, viewport: AlignmentViewport, physical_width: u32) -> bool {
        !self.is_pending(ReadKind::Detail)
            && (self.detail.is_none()
                || self.requested_detail_viewport != Some(viewport)
                || self.physical_width != physical_width)
    }
}

#[derive(Clone)]
pub struct AnalysisView {
    pub view_id: AnalysisViewId,
    pub name: String,
    pub runs: Vec<RunRef>,
    pub baseline: Option<RunRef>,
    pub pinned_runs: Vec<RunRef>,
    pub panels: Vec<MetricPanel>,
    pub selected_panel_id: Option<MetricPanelId>,
    pub inspector_tab: InspectorTab,
    pub ranking_direction: Option<ObjectiveDirection>,
    pub track_density: TrackDensity,
    pub core: ViewerCore,
    pub local_error: Option<String>,
    pub overview_revision: u64,
    pub detail_revision: u64,
    pub timeline_extents: HashMap<MetricKey, AlignmentViewport>,
}

pub struct AnalysisViews {
    views: Vec<AnalysisView>,
    pinned_projects: Vec<ProjectRef>,
    archived_projects: Vec<ProjectRef>,
    archived_runs: Vec<RunRef>,
    active_view_id: AnalysisViewId,
    next_id: u64,
}

impl Default for AnalysisViews {
    fn default() -> Self {
        let view = AnalysisView {
            view_id: AnalysisViewId::from_string("view-1"),
            name: "View 1".to_owned(),
            runs: Vec::new(),
            baseline: None,
            pinned_runs: Vec::new(),
            panels: Vec::new(),
            selected_panel_id: None,
            inspector_tab: InspectorTab::default(),
            ranking_direction: None,
            track_density: TrackDensity::default(),
            core: ViewerCore::default(),
            local_error: None,
            overview_revision: 0,
            detail_revision: 0,
            timeline_extents: HashMap::new(),
        };
        Self {
            active_view_id: view.view_id.clone(),
            views: vec![view],
            pinned_projects: Vec::new(),
            archived_projects: Vec::new(),
            archived_runs: Vec::new(),
            next_id: 2,
        }
    }
}

impl AnalysisViews {
    pub fn restore(document: &WorkbenchDocument) -> (Self, Vec<String>) {
        let mut issues = Vec::new();
        let mut views = Vec::new();
        for (index, saved) in document.views.iter().enumerate() {
            let mut runs = Vec::new();
            for saved_run in &saved.runs {
                let run = RunRef::new(
                    DataSourceId::from_path(&saved_run.source_path),
                    saved_run.project_id.clone(),
                    saved_run.run_id.clone(),
                );
                if runs.contains(&run) {
                    issues.push(format!(
                        "View {:?} contains duplicate Run {}",
                        saved.name,
                        saved_run.run_id.as_str()
                    ));
                } else if runs.len() == crate::core::MAX_SELECTED_RUNS {
                    issues.push(format!(
                        "View {:?} exceeds the 10 Run selection limit",
                        saved.name
                    ));
                } else {
                    runs.push(run);
                }
            }
            let baseline = saved.baseline.as_ref().map(saved_run_ref);
            let pinned_runs = saved.pinned_runs.iter().map(saved_run_ref).collect();
            for organized in baseline.iter().chain(&pinned_runs) {
                if runs.contains(organized) {
                    continue;
                }
                if runs.len() == crate::core::MAX_SELECTED_RUNS {
                    issues.push(format!(
                        "View {:?} cannot make every organized Run visible",
                        saved.name
                    ));
                    break;
                }
                runs.push(organized.clone());
            }
            let mut panels = Vec::new();
            for metric in &saved.metrics {
                let metric_key = MetricKey::from_string(metric);
                if panels
                    .iter()
                    .any(|panel: &MetricPanel| panel.metric_key == metric_key)
                {
                    issues.push(format!(
                        "View {:?} contains duplicate Metric {metric:?}",
                        saved.name
                    ));
                } else {
                    let mut panel = MetricPanel::new(metric_key);
                    if let Some((_, height)) =
                        saved.metric_heights.iter().find(|(key, _)| key == metric)
                    {
                        panel.row_height = metric_row_height(*height);
                    }
                    panels.push(panel);
                }
            }
            let selected_panel_id = saved.selected_metric.as_ref().and_then(|selected| {
                panels
                    .iter()
                    .find(|panel| panel.metric_key.as_str() == selected)
                    .map(|panel| panel.panel_id.clone())
            });
            if saved.selected_metric.is_some() && selected_panel_id.is_none() {
                issues.push(format!(
                    "View {:?} selected an unknown persisted Metric",
                    saved.name
                ));
            }
            let archived_runs = document
                .archived_runs
                .iter()
                .map(saved_run_ref)
                .collect::<Vec<_>>();
            let mut core = ViewerCore::new(ViewerSelection {
                source_id: runs.first().map(|run| run.source_id.clone()),
                project_id: runs.first().map(|run| run.project_id.clone()),
                runs: runs
                    .iter()
                    .filter(|run| !archived_runs.contains(run))
                    .cloned()
                    .collect(),
                metric_key: selected_panel_id.as_ref().and_then(|panel_id| {
                    panels
                        .iter()
                        .find(|panel| &panel.panel_id == panel_id)
                        .map(|panel| panel.metric_key.clone())
                }),
            });
            core.select_axis(saved.axis);
            if let Some(viewport) = saved.viewport
                && let Ok(viewport) = AlignmentViewport::new(
                    viewport.start().floor() as i64,
                    viewport.end().ceil() as i64,
                )
            {
                core.set_timeline_home(viewport);
            }
            views.push(AnalysisView {
                view_id: AnalysisViewId::from_string(format!("view-{}", index + 1)),
                name: saved.name.clone(),
                runs,
                baseline,
                pinned_runs,
                panels,
                selected_panel_id,
                inspector_tab: saved.inspector_tab,
                ranking_direction: saved.ranking_direction,
                track_density: saved.track_density,
                core,
                local_error: None,
                overview_revision: 0,
                detail_revision: 0,
                timeline_extents: HashMap::new(),
            });
        }
        if views.is_empty() {
            views = Self::default().views;
        }
        let active = document.active_view.min(views.len() - 1);
        if active != document.active_view {
            issues.push("persisted active View index is unavailable".to_owned());
        }
        let active_view_id = views[active].view_id.clone();
        let next_id = views.len() as u64 + 1;
        (
            Self {
                views,
                pinned_projects: document
                    .pinned_projects
                    .iter()
                    .map(|project| {
                        ProjectRef::new(
                            DataSourceId::from_path(&project.source_path),
                            project.project_id.clone(),
                        )
                    })
                    .collect(),
                archived_projects: document
                    .archived_projects
                    .iter()
                    .map(|project| {
                        ProjectRef::new(
                            DataSourceId::from_path(&project.source_path),
                            project.project_id.clone(),
                        )
                    })
                    .collect(),
                archived_runs: document.archived_runs.iter().map(saved_run_ref).collect(),
                active_view_id,
                next_id,
            },
            issues,
        )
    }

    pub fn views(&self) -> &[AnalysisView] {
        &self.views
    }

    pub fn pinned_projects(&self) -> &[ProjectRef] {
        &self.pinned_projects
    }

    pub fn archived_projects(&self) -> &[ProjectRef] {
        &self.archived_projects
    }

    pub fn archived_runs(&self) -> &[RunRef] {
        &self.archived_runs
    }

    pub fn active_index(&self) -> usize {
        self.views
            .iter()
            .position(|view| view.view_id == self.active_view_id)
            .expect("active Analysis View must remain in the collection")
    }

    pub fn active(&self) -> &AnalysisView {
        self.views
            .iter()
            .find(|view| view.view_id == self.active_view_id)
            .expect("active Analysis View must remain in the collection")
    }

    pub fn active_mut(&mut self) -> &mut AnalysisView {
        self.views
            .iter_mut()
            .find(|view| view.view_id == self.active_view_id)
            .expect("active Analysis View must remain in the collection")
    }

    pub fn create_empty(&mut self) -> AnalysisViewId {
        let view_id = self.next_view_id();
        self.views.push(AnalysisView {
            view_id: view_id.clone(),
            name: format!("View {}", self.views.len() + 1),
            runs: Vec::new(),
            baseline: None,
            pinned_runs: Vec::new(),
            panels: Vec::new(),
            selected_panel_id: None,
            inspector_tab: InspectorTab::default(),
            ranking_direction: None,
            track_density: TrackDensity::default(),
            core: ViewerCore::default(),
            local_error: None,
            overview_revision: 0,
            detail_revision: 0,
            timeline_extents: HashMap::new(),
        });
        self.active_view_id = view_id.clone();
        view_id
    }

    pub fn duplicate_active(&mut self) -> AnalysisViewId {
        let active = self.active().clone();
        let view_id = self.next_view_id();
        self.views.push(AnalysisView {
            view_id: view_id.clone(),
            name: format!("{} Copy", active.name),
            runs: active.runs,
            baseline: active.baseline,
            pinned_runs: active.pinned_runs,
            panels: active.panels,
            selected_panel_id: active.selected_panel_id,
            inspector_tab: active.inspector_tab,
            ranking_direction: active.ranking_direction,
            track_density: active.track_density,
            core: active.core,
            local_error: active.local_error,
            overview_revision: active.overview_revision,
            detail_revision: active.detail_revision,
            timeline_extents: active.timeline_extents,
        });
        self.active_view_id = view_id.clone();
        view_id
    }

    pub fn activate(&mut self, view_id: &AnalysisViewId) -> bool {
        if self.views.iter().any(|view| &view.view_id == view_id) {
            self.active_view_id.clone_from(view_id);
            true
        } else {
            false
        }
    }

    pub fn rename(&mut self, view_id: &AnalysisViewId, name: &str) -> bool {
        let name = name.trim();
        let Some(view) = self.views.iter_mut().find(|view| &view.view_id == view_id) else {
            return false;
        };
        if name.is_empty() {
            return false;
        }
        view.name = name.to_owned();
        true
    }

    pub fn close(&mut self, view_id: &AnalysisViewId) -> bool {
        let Some(index) = self.views.iter().position(|view| &view.view_id == view_id) else {
            return false;
        };
        let was_active = self.active_view_id == *view_id;
        self.views.remove(index);
        if self.views.is_empty() {
            self.create_empty();
        } else if was_active {
            let replacement = index.min(self.views.len() - 1);
            self.active_view_id = self.views[replacement].view_id.clone();
        }
        true
    }

    pub fn toggle_active_run(&mut self, run: RunRef) -> Result<bool, SelectionError> {
        let archived = self.archived_runs.clone();
        let view = self.active_mut();
        let selected = if let Some(index) = view.runs.iter().position(|item| item == &run) {
            view.runs.remove(index);
            false
        } else {
            let visible_count = view
                .runs
                .iter()
                .filter(|item| !archived.contains(item))
                .count();
            if visible_count == crate::core::MAX_SELECTED_RUNS {
                return Err(SelectionError::RunLimit);
            }
            view.runs.push(run);
            true
        };
        self.invalidate_active_panels();
        Ok(selected)
    }

    pub fn set_active_baseline(&mut self, baseline: Option<RunRef>) -> Result<(), SelectionError> {
        if let Some(run) = &baseline
            && !self.active().runs.contains(run)
        {
            self.toggle_active_run(run.clone())?;
        }
        if let Some(run) = &baseline {
            self.active_mut().pinned_runs.retain(|item| item != run);
        }
        self.active_mut().baseline = baseline;
        self.invalidate_active_panels();
        Ok(())
    }

    pub fn toggle_active_pinned_run(&mut self, run: RunRef) -> Result<bool, SelectionError> {
        let adding = !self.active().pinned_runs.contains(&run);
        let was_baseline = self.active().baseline.as_ref() == Some(&run);
        if adding && !self.active().runs.contains(&run) {
            self.toggle_active_run(run.clone())?;
        }
        let pinned = &mut self.active_mut().pinned_runs;
        let added = if let Some(index) = pinned.iter().position(|candidate| candidate == &run) {
            pinned.remove(index);
            false
        } else {
            pinned.push(run);
            true
        };
        if added && was_baseline {
            self.active_mut().baseline = None;
        }
        self.invalidate_active_panels();
        Ok(added)
    }

    pub fn pin_project(&mut self, project: ProjectRef) {
        self.archived_projects.retain(|item| item != &project);
        if !self.pinned_projects.contains(&project) {
            self.pinned_projects.push(project);
        }
    }

    pub fn unpin_project(&mut self, project: &ProjectRef) {
        self.pinned_projects.retain(|item| item != project);
    }

    pub fn archive_project(&mut self, project: ProjectRef) {
        self.pinned_projects.retain(|item| item != &project);
        if !self.archived_projects.contains(&project) {
            self.archived_projects.push(project);
        }
    }

    pub fn restore_project(&mut self, project: &ProjectRef) {
        self.archived_projects.retain(|item| item != project);
    }

    pub fn archive_run(&mut self, run: RunRef) {
        for view in &mut self.views {
            view.pinned_runs.retain(|item| item != &run);
            if view.baseline.as_ref() == Some(&run) {
                view.baseline = None;
            }
            if view.core.selection().runs.contains(&run) {
                let _ = view.core.toggle_run(run.clone());
            }
        }
        if !self.archived_runs.contains(&run) {
            self.archived_runs.push(run);
        }
    }

    pub fn restore_run(&mut self, run: &RunRef) {
        self.archived_runs.retain(|item| item != run);
        for view in &mut self.views {
            if view.runs.contains(run) && !view.core.selection().runs.contains(run) {
                let _ = view.core.toggle_run(run.clone());
            }
        }
    }

    pub fn select_active_metric(&mut self, metric_key: MetricKey) -> MetricPanelId {
        if let Some(panel) = self
            .active()
            .panels
            .iter()
            .find(|panel| panel.metric_key == metric_key)
        {
            let panel_id = panel.panel_id.clone();
            self.active_mut().selected_panel_id = Some(panel_id.clone());
            return panel_id;
        }
        let panel = MetricPanel::new(metric_key);
        let panel_id = panel.panel_id.clone();
        self.active_mut().panels.push(panel);
        self.active_mut().selected_panel_id = Some(panel_id.clone());
        panel_id
    }

    pub fn remove_active_panel(&mut self, panel_id: &MetricPanelId) -> bool {
        let view = self.active_mut();
        let Some(index) = view
            .panels
            .iter()
            .position(|panel| &panel.panel_id == panel_id)
        else {
            return false;
        };
        let removed = view.panels.remove(index);
        view.timeline_extents.remove(&removed.metric_key);
        if view.selected_panel_id.as_ref() == Some(panel_id) {
            view.selected_panel_id = view
                .panels
                .get(index.min(view.panels.len().saturating_sub(1)))
                .map(|panel| panel.panel_id.clone());
        }
        true
    }

    pub fn active_panel(&self, panel_id: &MetricPanelId) -> Option<&MetricPanel> {
        self.active()
            .panels
            .iter()
            .find(|panel| &panel.panel_id == panel_id)
    }

    pub fn active_panel_mut(&mut self, panel_id: &MetricPanelId) -> Option<&mut MetricPanel> {
        self.active_mut()
            .panels
            .iter_mut()
            .find(|panel| &panel.panel_id == panel_id)
    }

    pub fn select_active_panel(&mut self, panel_id: &MetricPanelId) -> bool {
        if self.active_panel(panel_id).is_none() {
            return false;
        }
        self.active_mut().selected_panel_id = Some(panel_id.clone());
        true
    }

    pub fn set_active_panel_height(&mut self, panel_id: &MetricPanelId, height: f32) -> bool {
        let Some(panel) = self.active_panel_mut(panel_id) else {
            return false;
        };
        panel.row_height = metric_row_height(height);
        true
    }

    pub fn set_active_inspector_tab(&mut self, tab: InspectorTab) {
        self.active_mut().inspector_tab = tab;
    }

    pub fn set_active_ranking_direction(&mut self, direction: ObjectiveDirection) {
        self.active_mut().ranking_direction = Some(direction);
    }

    pub fn reconcile_source_project(
        &mut self,
        source_id: &DataSourceId,
        project_id: &ProjectId,
        available_runs: &[RunId],
    ) -> Vec<RunRef> {
        let mut removed = Vec::new();
        for view in &mut self.views {
            let stale = view
                .runs
                .iter()
                .filter(|run| {
                    &run.source_id == source_id
                        && &run.project_id == project_id
                        && !available_runs.contains(&run.run_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            for run in &stale {
                if view.core.selection().runs.contains(run) {
                    let _ = view.core.toggle_run(run.clone());
                }
            }
            view.runs.retain(|run| !stale.contains(run));
            removed.extend(stale);
        }
        removed
    }

    pub fn begin_active_panel_read(
        &mut self,
        panel_id: &MetricPanelId,
        kind: ReadKind,
        generation: Generation,
    ) {
        let Some(panel) = self.active_panel_mut(panel_id) else {
            return;
        };
        match kind {
            ReadKind::Overview => panel.overview_generation = Some(generation),
            ReadKind::Detail => panel.detail_generation = Some(generation),
            ReadKind::Inspector => panel.inspector_generation = Some(generation),
            ReadKind::Catalog => {}
        }
    }

    pub fn begin_active_panel_detail(
        &mut self,
        panel_id: &MetricPanelId,
        generation: Generation,
        viewport: AlignmentViewport,
        physical_width: u32,
    ) {
        let Some(panel) = self.active_panel_mut(panel_id) else {
            return;
        };
        panel.detail_generation = Some(generation);
        panel.requested_detail_viewport = Some(viewport);
        panel.physical_width = physical_width;
    }

    pub fn complete_active_panel_read(
        &mut self,
        panel_id: &MetricPanelId,
        kind: ReadKind,
        generation: Generation,
        snapshot: Option<CurveSnapshot>,
        source_errors: Vec<SourceReadFailure>,
    ) -> bool {
        let Some(panel) = self.active_panel_mut(panel_id) else {
            return false;
        };
        let expected = match kind {
            ReadKind::Overview => &mut panel.overview_generation,
            ReadKind::Detail => &mut panel.detail_generation,
            ReadKind::Inspector => return false,
            ReadKind::Catalog => return false,
        };
        if *expected != Some(generation) {
            return false;
        }
        *expected = None;
        panel.source_errors = source_errors;
        if let Some(snapshot) = snapshot {
            match kind {
                ReadKind::Overview => {
                    panel.overview_revision = generation.0;
                    panel.overview = Some(Arc::new(snapshot));
                }
                ReadKind::Detail => {
                    panel.detail_revision = generation.0;
                    panel.detail = Some(Arc::new(snapshot));
                }
                ReadKind::Inspector => {}
                ReadKind::Catalog => {}
            }
        }
        true
    }

    pub fn complete_active_inspector_read(
        &mut self,
        panel_id: &MetricPanelId,
        generation: Generation,
        snapshot: Option<InspectorSnapshot>,
        source_errors: Vec<SourceReadFailure>,
    ) -> bool {
        let Some(panel) = self.active_panel_mut(panel_id) else {
            return false;
        };
        if panel.inspector_generation != Some(generation) {
            return false;
        }
        panel.inspector_generation = None;
        panel.inspector_errors = source_errors;
        if let Some(snapshot) = snapshot {
            panel.inspector = Some(Arc::new(snapshot));
        }
        true
    }

    pub fn record_active_metric_extent(
        &mut self,
        metric_key: MetricKey,
        extent: Option<AlignmentViewport>,
    ) -> Option<AlignmentViewport> {
        if let Some(extent) = extent {
            self.active_mut()
                .timeline_extents
                .insert(metric_key, extent);
        } else {
            self.active_mut().timeline_extents.remove(&metric_key);
        }
        self.active()
            .timeline_extents
            .values()
            .copied()
            .reduce(|left, right| {
                AlignmentViewport::new(left.start().min(right.start()), left.end().max(right.end()))
                    .expect("valid timeline extents must have a valid union")
            })
    }

    pub fn clear_active_timeline_extents(&mut self) {
        self.invalidate_active_panels();
    }

    pub fn remove_source(&mut self, source_id: &DataSourceId) {
        for view in &mut self.views {
            let previous = view.runs.len();
            view.runs.retain(|run| &run.source_id != source_id);
            let previous_pinned = view.pinned_runs.len();
            view.pinned_runs.retain(|run| &run.source_id != source_id);
            let removed_baseline = view
                .baseline
                .as_ref()
                .is_some_and(|run| &run.source_id == source_id);
            if removed_baseline {
                view.baseline = None;
            }
            if view.runs.len() != previous
                || view.pinned_runs.len() != previous_pinned
                || removed_baseline
            {
                view.timeline_extents.clear();
                for panel in &mut view.panels {
                    panel.overview = None;
                    panel.detail = None;
                    panel.source_errors.clear();
                    panel.overview_generation = None;
                    panel.detail_generation = None;
                    panel.requested_detail_viewport = None;
                    panel.inspector = None;
                    panel.inspector_generation = None;
                    panel.inspector_errors.clear();
                }
            }
        }
        self.pinned_projects
            .retain(|project| &project.source_id != source_id);
        self.archived_projects
            .retain(|project| &project.source_id != source_id);
        self.archived_runs.retain(|run| &run.source_id != source_id);
    }

    fn invalidate_active_panels(&mut self) {
        let view = self.active_mut();
        view.timeline_extents.clear();
        for panel in &mut view.panels {
            panel.overview = None;
            panel.detail = None;
            panel.source_errors.clear();
            panel.overview_generation = None;
            panel.detail_generation = None;
            panel.requested_detail_viewport = None;
            panel.inspector = None;
            panel.inspector_generation = None;
            panel.inspector_errors.clear();
        }
    }

    fn next_view_id(&mut self) -> AnalysisViewId {
        let view_id = AnalysisViewId::from_string(format!("view-{}", self.next_id));
        self.next_id = self.next_id.saturating_add(1);
        view_id
    }
}

fn metric_row_height(height: f32) -> f32 {
    height.clamp(DEFAULT_METRIC_ROW_HEIGHT, MAX_METRIC_ROW_HEIGHT)
}

fn saved_run_ref(saved: &crate::workbench_document::SavedRunRef) -> RunRef {
    RunRef::new(
        DataSourceId::from_path(&saved.source_path),
        saved.project_id.clone(),
        saved.run_id.clone(),
    )
}

#[cfg(test)]
mod tests {
    use pulseon_chart_core::AxisRange;
    use pulseon_model::alignment::AlignmentAxis;

    use crate::workbench_document::{SavedAnalysisView, SavedProjectRef, SavedRunRef};

    use super::*;

    #[test]
    fn closing_the_last_view_creates_a_new_empty_view() {
        let mut views = AnalysisViews::default();
        let original = views.active().view_id.clone();

        assert!(views.close(&original));
        assert_eq!(views.views().len(), 1);
        assert_ne!(views.active().view_id, original);
        assert!(views.active().runs.is_empty());
    }

    #[test]
    fn duplicated_views_copy_selection_without_sharing_mutation() {
        let mut views = AnalysisViews::default();
        let run = RunRef::new(
            DataSourceId::from_string("source"),
            pulseon_model::types::ProjectId::from_string("project"),
            pulseon_model::run::RunId::from_string("run"),
        );
        views
            .toggle_active_run(run)
            .expect("first Run should be selected");
        let baseline = RunRef::new(
            DataSourceId::from_string("source"),
            ProjectId::from_string("project"),
            RunId::from_string("baseline"),
        );
        views
            .set_active_baseline(Some(baseline.clone()))
            .expect("baseline should fit the visible Run limit");
        let pinned = RunRef::new(
            DataSourceId::from_string("source"),
            ProjectId::from_string("project"),
            RunId::from_string("pinned"),
        );
        assert!(
            views
                .toggle_active_pinned_run(pinned)
                .expect("pinned Run should fit the visible Run limit")
        );
        views.select_active_metric(MetricKey::from_string("loss"));
        views
            .active_mut()
            .core
            .select_axis(pulseon_model::alignment::AlignmentAxis::ElapsedTime);

        let duplicate = views.duplicate_active();
        views.active_mut().runs.clear();
        views.active_mut().baseline = None;
        views.active_mut().pinned_runs.clear();
        views.active_mut().panels.clear();
        views
            .active_mut()
            .core
            .select_axis(pulseon_model::alignment::AlignmentAxis::Step);
        assert!(views.activate(&AnalysisViewId::from_string("view-1")));

        assert_eq!(views.active().runs.len(), 3);
        assert!(views.active().baseline.is_some());
        assert_eq!(views.active().pinned_runs.len(), 1);
        assert_eq!(
            views
                .active()
                .panels
                .iter()
                .map(|panel| panel.metric_key.clone())
                .collect::<Vec<_>>(),
            [MetricKey::from_string("loss")]
        );
        assert_eq!(
            views.active().core.axis(),
            pulseon_model::alignment::AlignmentAxis::ElapsedTime
        );
        assert!(views.activate(&duplicate));
        assert!(views.active().runs.is_empty());
        assert!(views.active().baseline.is_none());
        assert!(views.active().pinned_runs.is_empty());
        assert!(views.active().panels.is_empty());
        assert_eq!(
            views.active().core.axis(),
            pulseon_model::alignment::AlignmentAxis::Step
        );
    }

    #[test]
    fn metric_selection_and_inspector_tab_are_isolated_per_view() {
        let mut views = AnalysisViews::default();
        let first_view = views.active().view_id.clone();
        let loss = views.select_active_metric(MetricKey::from_string("loss"));
        views.set_active_inspector_tab(InspectorTab::Evidence);

        let second_view = views.create_empty();
        let accuracy = views.select_active_metric(MetricKey::from_string("accuracy"));
        views.set_active_inspector_tab(InspectorTab::Ranking);
        assert!(views.select_active_panel(&accuracy));
        assert!(views.activate(&first_view));

        assert_eq!(views.active().selected_panel_id.as_ref(), Some(&loss));
        assert_eq!(views.active().inspector_tab, InspectorTab::Evidence);
        assert!(views.activate(&second_view));
        assert_eq!(views.active().selected_panel_id.as_ref(), Some(&accuracy));
        assert_eq!(views.active().inspector_tab, InspectorTab::Ranking);
    }

    #[test]
    fn metric_panel_heights_use_the_compact_bounded_range() {
        let mut views = AnalysisViews::default();
        let panel_id = views.select_active_metric(MetricKey::from_string("loss"));

        assert_eq!(views.active().panels[0].row_height, 52.);
        assert!(views.set_active_panel_height(&panel_id, 1.));
        assert_eq!(views.active().panels[0].row_height, 52.);
        assert!(views.set_active_panel_height(&panel_id, 1_000.));
        assert_eq!(views.active().panels[0].row_height, 180.);
    }

    #[test]
    fn restore_reports_duplicate_identities_and_unknown_selection() {
        let saved_run = SavedRunRef {
            source_path: "/tmp/source".into(),
            project_id: ProjectId::from_string("project"),
            run_id: RunId::from_string("run"),
        };
        let document = WorkbenchDocument {
            sources: vec![saved_run.source_path.clone()],
            pinned_projects: vec![SavedProjectRef {
                source_path: saved_run.source_path.clone(),
                project_id: ProjectId::from_string("project"),
            }],
            archived_projects: Vec::new(),
            archived_runs: Vec::new(),
            views: vec![SavedAnalysisView {
                name: "Duplicates".to_owned(),
                runs: vec![saved_run.clone(), saved_run],
                baseline: None,
                pinned_runs: Vec::new(),
                metrics: vec!["loss".to_owned(), "loss".to_owned()],
                metric_heights: Vec::new(),
                selected_metric: Some("unknown".to_owned()),
                inspector_tab: InspectorTab::Summary,
                ranking_direction: None,
                axis: AlignmentAxis::Step,
                track_density: TrackDensity::Comfortable,
                viewport: Some(AxisRange::new(0., 1.).expect("viewport should be valid")),
            }],
            active_view: 0,
            project_sidebar_visible: true,
            project_sidebar_width: 320.,
            metric_sidebar_compact: false,
            bottom_inspector_visible: false,
            bottom_inspector_height: 220.,
        };

        let (views, issues) = AnalysisViews::restore(&document);

        assert_eq!(views.active().runs.len(), 1);
        assert_eq!(views.active().panels.len(), 1);
        assert_eq!(views.pinned_projects().len(), 1);
        assert!(views.active().selected_panel_id.is_none());
        assert_eq!(issues.len(), 3);
    }

    #[test]
    fn restored_organization_uses_composite_source_identities() {
        let project_id = ProjectId::from_string("shared-project");
        let saved_project = |source: &str| SavedProjectRef {
            source_path: source.into(),
            project_id: project_id.clone(),
        };
        let document = WorkbenchDocument {
            sources: vec!["/tmp/source-a".into(), "/tmp/source-b".into()],
            pinned_projects: vec![saved_project("/tmp/source-a")],
            archived_projects: vec![saved_project("/tmp/source-b")],
            archived_runs: Vec::new(),
            views: Vec::new(),
            active_view: 0,
            project_sidebar_visible: true,
            project_sidebar_width: 320.,
            metric_sidebar_compact: false,
            bottom_inspector_visible: false,
            bottom_inspector_height: 220.,
        };

        let (views, issues) = AnalysisViews::restore(&document);

        assert!(issues.is_empty());
        assert_ne!(views.pinned_projects()[0], views.archived_projects()[0]);
        assert_eq!(views.pinned_projects()[0].project_id, project_id);
    }

    #[test]
    fn view_organization_is_local_while_archived_runs_are_shared() {
        let mut views = AnalysisViews::default();
        let run = |name: &str| {
            RunRef::new(
                DataSourceId::from_string("source"),
                ProjectId::from_string("project"),
                RunId::from_string(name),
            )
        };
        views
            .set_active_baseline(Some(run("baseline")))
            .expect("baseline should fit the visible Run limit");
        assert!(
            views
                .toggle_active_pinned_run(run("pinned"))
                .expect("pinned Run should fit the visible Run limit")
        );
        let first = views.active().view_id.clone();
        let second = views.create_empty();

        assert!(views.active().baseline.is_none());
        assert!(views.active().pinned_runs.is_empty());
        let archived = run("archived");
        views
            .toggle_active_run(archived.clone())
            .expect("archived candidate should fit the visible Run limit");
        views.archive_run(archived.clone());
        assert!(views.active().runs.contains(&archived));
        assert!(views.activate(&first));
        assert_eq!(views.active().baseline.as_ref(), Some(&run("baseline")));
        assert_eq!(views.active().pinned_runs, [run("pinned")]);
        assert_eq!(views.archived_runs(), std::slice::from_ref(&archived));
        assert!(views.activate(&second));
        assert_eq!(views.archived_runs(), std::slice::from_ref(&archived));
        views.restore_run(&archived);
        assert!(views.active().runs.contains(&archived));
    }

    #[test]
    fn timeline_home_is_the_union_of_loaded_metric_extents() {
        let mut views = AnalysisViews::default();
        views.record_active_metric_extent(
            MetricKey::from_string("loss"),
            Some(AlignmentViewport::new(10, 20).expect("test extent should be valid")),
        );

        let home = views
            .record_active_metric_extent(
                MetricKey::from_string("accuracy"),
                Some(AlignmentViewport::new(5, 15).expect("test extent should be valid")),
            )
            .expect("metric extents should produce a timeline home");

        assert_eq!((home.start(), home.end()), (5, 20));
    }

    #[test]
    fn metric_panels_keep_independent_generations_and_source_errors() {
        let mut views = AnalysisViews::default();
        let first = views.select_active_metric(MetricKey::from_string("loss"));
        let second = views.select_active_metric(MetricKey::from_string("accuracy"));
        views.begin_active_panel_read(&first, ReadKind::Detail, Generation(1));
        views.begin_active_panel_read(&second, ReadKind::Detail, Generation(2));

        assert!(!views.complete_active_panel_read(
            &first,
            ReadKind::Detail,
            Generation(2),
            None,
            Vec::new(),
        ));
        assert!(views.complete_active_panel_read(
            &second,
            ReadKind::Detail,
            Generation(2),
            None,
            vec![SourceReadFailure {
                source_id: DataSourceId::from_string("source-b"),
                message: "unavailable".to_owned(),
            }],
        ));

        assert!(
            views
                .active_panel(&first)
                .expect("first panel should exist")
                .is_pending(ReadKind::Detail)
        );
        assert_eq!(
            views
                .active_panel(&second)
                .expect("second panel should exist")
                .source_errors
                .len(),
            1
        );
        views.begin_active_panel_read(&first, ReadKind::Inspector, Generation(3));
        assert!(!views.complete_active_inspector_read(&first, Generation(2), None, Vec::new(),));
        assert!(views.complete_active_inspector_read(&first, Generation(3), None, Vec::new(),));
    }
}
