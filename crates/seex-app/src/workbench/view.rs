use std::collections::HashMap;
use std::sync::Arc;

use seex::AlignmentViewport;
use seex::MetricKey;
use seex::ProjectId;
use seex::RunId;

use crate::data::query::{CurveSnapshot, InspectorSnapshot};
use crate::data::worker::{Generation, ReadKind};
use crate::domain::{DataSourceId, RunRef, SelectionError, ViewNavigation};
use crate::workbench::panel_reads::{
    AnalysisViewId, MetricPanelId, PanelReadMode, SourceReadFailure,
};
use crate::workbench::toml_document::{SavedRunRef, TomlWorkbenchDocument};

pub const DEFAULT_METRIC_ROW_HEIGHT: f32 = 52.;
const MAX_METRIC_ROW_HEIGHT: f32 = 180.;

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

    fn contains_run(&self, run: &RunRef) -> bool {
        self.source_id == run.source_id && self.project_id == run.project_id
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
    pub logical_width: u32,
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
            logical_width: 1_000,
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

    pub fn needs_detail(&self, viewport: AlignmentViewport, logical_width: u32) -> bool {
        !self.is_pending(ReadKind::Detail)
            && (self.requested_detail_viewport != Some(viewport)
                || self.logical_width != logical_width)
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
    pub navigation: ViewNavigation,
    pub timeline_extents: HashMap<MetricKey, AlignmentViewport>,
}

#[derive(Clone)]
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
            navigation: ViewNavigation::default(),
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
    pub fn restore(document: &TomlWorkbenchDocument) -> (Self, Vec<String>) {
        let mut issues = Vec::new();
        let mut views = Vec::new();
        for (index, saved) in document.views.iter().enumerate() {
            let mut runs = Vec::new();
            for saved_run in &saved.runs {
                let run = RunRef::new(
                    DataSourceId::from_alias(&saved_run.source_alias),
                    saved_run.project_id.clone(),
                    saved_run.run_id.clone(),
                );
                if runs.contains(&run) {
                    issues.push(format!(
                        "View {:?} contains duplicate Run {}",
                        saved.name,
                        saved_run.run_id.as_str()
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
            let mut navigation = ViewNavigation::default();
            navigation.select_axis(saved.axis);
            if let Some(viewport) = saved.viewport
                && let Ok(viewport) = AlignmentViewport::new(
                    viewport.start().floor() as i64,
                    viewport.end().ceil() as i64,
                )
            {
                navigation.set_timeline_home(viewport);
            }
            views.push(AnalysisView {
                view_id: AnalysisViewId::from_string(format!("view-{}", index + 1)),
                name: saved.name.clone(),
                runs,
                baseline,
                pinned_runs,
                panels,
                selected_panel_id,
                navigation,
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
        let restored = Self {
            views,
            pinned_projects: document
                .pinned_projects
                .iter()
                .map(|project| {
                    ProjectRef::new(
                        DataSourceId::from_alias(&project.source_alias),
                        project.project_id.clone(),
                    )
                })
                .collect(),
            archived_projects: document
                .archived_projects
                .iter()
                .map(|project| {
                    ProjectRef::new(
                        DataSourceId::from_alias(&project.source_alias),
                        project.project_id.clone(),
                    )
                })
                .collect(),
            archived_runs: document.archived_runs.iter().map(saved_run_ref).collect(),
            active_view_id,
            next_id,
        };
        (restored, issues)
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

    pub fn active_visible_runs(&self) -> impl Iterator<Item = &RunRef> {
        self.active()
            .runs
            .iter()
            .filter(|run| !self.archived_runs.contains(run))
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
            navigation: ViewNavigation::default(),
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
            navigation: active.navigation,
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

    pub fn toggle_active_run(
        &mut self,
        run: RunRef,
        selection_has_capacity: bool,
    ) -> Result<bool, SelectionError> {
        let view = self.active_mut();
        let selected = if let Some(index) = view.runs.iter().position(|item| item == &run) {
            view.runs.remove(index);
            false
        } else {
            if !selection_has_capacity {
                return Err(SelectionError::RunLimit);
            }
            view.runs.push(run);
            true
        };
        Ok(selected)
    }

    pub fn set_active_baseline(
        &mut self,
        baseline: Option<RunRef>,
        selection_has_capacity: bool,
    ) -> Result<(), SelectionError> {
        if let Some(run) = &baseline
            && !self.active().runs.contains(run)
        {
            self.toggle_active_run(run.clone(), selection_has_capacity)?;
        }
        if let Some(run) = &baseline {
            self.active_mut().pinned_runs.retain(|item| item != run);
        }
        self.active_mut().baseline = baseline;
        Ok(())
    }

    pub fn toggle_active_pinned_run(
        &mut self,
        run: RunRef,
        selection_has_capacity: bool,
    ) -> Result<bool, SelectionError> {
        let adding = !self.active().pinned_runs.contains(&run);
        let was_baseline = self.active().baseline.as_ref() == Some(&run);
        if adding && !self.active().runs.contains(&run) {
            self.toggle_active_run(run.clone(), selection_has_capacity)?;
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

    pub fn remove_project(&mut self, project: ProjectRef) {
        self.pinned_projects.retain(|item| item != &project);
        self.archived_projects.retain(|item| item != &project);
        self.archived_runs.retain(|run| !project.contains_run(run));
        for view in &mut self.views {
            let removed = view
                .runs
                .iter()
                .filter(|run| project.contains_run(run))
                .cloned()
                .collect::<Vec<_>>();
            view.runs.retain(|run| !project.contains_run(run));
            view.pinned_runs.retain(|run| !project.contains_run(run));
            if view
                .baseline
                .as_ref()
                .is_some_and(|run| project.contains_run(run))
            {
                view.baseline = None;
            }
            if !removed.is_empty() {
                invalidate_view_panels(view);
            }
        }
    }

    pub fn archive_run(&mut self, run: RunRef) {
        for view in &mut self.views {
            view.pinned_runs.retain(|item| item != &run);
            if view.baseline.as_ref() == Some(&run) {
                view.baseline = None;
            }
        }
        if !self.archived_runs.contains(&run) {
            self.archived_runs.push(run);
        }
    }

    pub fn restore_run(&mut self, run: &RunRef) {
        self.archived_runs.retain(|item| item != run);
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

    pub fn cancel_active_panel_reads(&mut self) {
        for panel in &mut self.active_mut().panels {
            panel.overview_generation = None;
            panel.detail_generation = None;
            panel.inspector_generation = None;
            panel.requested_detail_viewport = None;
        }
    }

    pub fn reconcile_source_runs(
        &mut self,
        source_id: &DataSourceId,
        available_runs: &[(ProjectId, RunId)],
    ) -> Vec<RunRef> {
        let mut removed = Vec::new();
        for view in &mut self.views {
            let stale = view
                .runs
                .iter()
                .filter(|run| {
                    &run.source_id == source_id
                        && !available_runs.iter().any(|(project_id, run_id)| {
                            project_id == &run.project_id && run_id == &run.run_id
                        })
                })
                .cloned()
                .collect::<Vec<_>>();
            view.runs.retain(|run| !stale.contains(run));
            view.pinned_runs.retain(|run| !stale.contains(run));
            if view
                .baseline
                .as_ref()
                .is_some_and(|run| stale.contains(run))
            {
                view.baseline = None;
            }
            if !stale.is_empty() {
                invalidate_view_panels(view);
            }
            removed.extend(stale);
        }
        self.archived_runs.retain(|run| {
            &run.source_id != source_id
                || available_runs.iter().any(|(project_id, run_id)| {
                    project_id == &run.project_id && run_id == &run.run_id
                })
        });
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
        logical_width: u32,
    ) {
        let Some(panel) = self.active_panel_mut(panel_id) else {
            return;
        };
        panel.detail_generation = Some(generation);
        panel.requested_detail_viewport = Some(viewport);
        panel.logical_width = logical_width;
    }

    pub fn complete_active_panel_read(
        &mut self,
        panel_id: &MetricPanelId,
        kind: ReadKind,
        generation: Generation,
        mode: PanelReadMode,
        snapshot: Option<CurveSnapshot>,
        source_errors: Vec<SourceReadFailure>,
    ) -> bool {
        let run_order = self.active().runs.clone();
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
                    let replacing = mode == PanelReadMode::Replace || panel.overview.is_none();
                    if mode == PanelReadMode::Merge {
                        merge_curve_snapshot(&mut panel.overview, snapshot, &run_order);
                    } else {
                        panel.overview = Some(Arc::new(snapshot));
                    }
                    if replacing {
                        panel.overview_revision = generation.0;
                    }
                }
                ReadKind::Detail => {
                    let replacing = mode == PanelReadMode::Replace || panel.detail.is_none();
                    if mode == PanelReadMode::Merge {
                        merge_curve_snapshot(&mut panel.detail, snapshot, &run_order);
                    } else {
                        panel.detail = Some(Arc::new(snapshot));
                    }
                    if replacing {
                        panel.detail_revision = generation.0;
                    }
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

fn merge_curve_snapshot(
    current: &mut Option<Arc<CurveSnapshot>>,
    incoming: CurveSnapshot,
    run_order: &[RunRef],
) {
    let Some(current_snapshot) = current.as_mut() else {
        *current = Some(Arc::new(incoming));
        return;
    };
    if current_snapshot.viewport != incoming.viewport
        || current_snapshot.point_budget != incoming.point_budget
    {
        *current = Some(Arc::new(incoming));
        return;
    }
    let snapshot = Arc::make_mut(current_snapshot);
    for curve in incoming.series {
        snapshot
            .series
            .retain(|existing| existing.run_ref != curve.run_ref);
        snapshot.series.push(curve);
    }
    snapshot.series.sort_by_key(|curve| {
        run_order
            .iter()
            .position(|run| run == &curve.run_ref)
            .unwrap_or(usize::MAX)
    });
    snapshot.real_range = match (snapshot.real_range, incoming.real_range) {
        (Some(current), Some(incoming)) => AlignmentViewport::new(
            current.start().min(incoming.start()),
            current.end().max(incoming.end()),
        )
        .ok(),
        (current @ Some(_), None) => current,
        (None, incoming) => incoming,
    };
}

fn invalidate_view_panels(view: &mut AnalysisView) {
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

fn metric_row_height(height: f32) -> f32 {
    height.clamp(DEFAULT_METRIC_ROW_HEIGHT, MAX_METRIC_ROW_HEIGHT)
}

fn saved_run_ref(saved: &SavedRunRef) -> RunRef {
    RunRef::new(
        DataSourceId::from_alias(&saved.source_alias),
        saved.project_id.clone(),
        saved.run_id.clone(),
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
