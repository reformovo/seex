use std::fmt;
use std::path::Path;
use std::sync::Arc;

use pulseon_chart_core::{AxisRange, BrushState};
use pulseon_model::alignment::{AlignmentAxis, AlignmentViewport};
use pulseon_model::metric::MetricKey;
use pulseon_model::run::{Run, RunId, RunStatus};
use pulseon_model::types::ProjectId;

use crate::model::CatalogSnapshot;
use crate::query::CurveSnapshot;
use crate::worker::{Generation, ReadEvent, ReadKind, ReadRequest, ReadSnapshot};

pub const MAX_SELECTED_RUNS: usize = 10;

/// Stable viewer-local identity for one imported native source.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DataSourceId(String);

impl DataSourceId {
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn from_path(path: &Path) -> Self {
        Self(path.to_string_lossy().into_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DataSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Collision-free identity for one Run selected from an imported source.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RunRef {
    pub source_id: DataSourceId,
    pub project_id: ProjectId,
    pub run_id: RunId,
}

impl RunRef {
    pub const fn new(source_id: DataSourceId, project_id: ProjectId, run_id: RunId) -> Self {
        Self {
            source_id,
            project_id,
            run_id,
        }
    }

    pub fn cache_key(&self) -> String {
        format!(
            "{}:{}{}:{}{}:{}",
            self.source_id.as_str().len(),
            self.source_id.as_str(),
            self.project_id.as_str().len(),
            self.project_id.as_str(),
            self.run_id.as_str().len(),
            self.run_id.as_str(),
        )
    }
}

/// Matches a Run by name, identifier, or lifecycle status.
pub fn run_matches_filter(run: &Run, query: &str) -> bool {
    run_fields_match_filter(
        &run.name,
        run.run_id.as_str(),
        match run.status {
            RunStatus::Running => "running",
            RunStatus::Finished => "finished",
            RunStatus::Failed => "failed",
        },
        query,
    )
}

fn run_fields_match_filter(name: &str, run_id: &str, status: &str, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    name.to_lowercase().contains(&query)
        || run_id.to_lowercase().contains(&query)
        || status.contains(&query)
}

/// Stable identities selected by the viewer independently of rendered widgets.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ViewerSelection {
    pub source_id: Option<DataSourceId>,
    pub project_id: Option<ProjectId>,
    pub runs: Vec<RunRef>,
    pub metric_key: Option<MetricKey>,
}

/// Whether a worker event changed the current viewer state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    Applied,
    IgnoredStale,
    IgnoredMismatched,
}

/// Invalid user selection transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SelectionError {
    #[error("at most {MAX_SELECTED_RUNS} Runs can be selected")]
    RunLimit,
}

/// GPUI-independent state reconciler for native read snapshots.
#[derive(Clone)]
pub struct ViewerCore {
    selection: ViewerSelection,
    axis: AlignmentAxis,
    brush: Option<BrushState>,
    catalog: Option<CatalogSnapshot>,
    overview: Option<Arc<CurveSnapshot>>,
    detail: Option<Arc<CurveSnapshot>>,
    expected: [Option<ExpectedRequest>; 4],
    last_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpectedRequest {
    generation: Generation,
    source_id: DataSourceId,
}

impl Default for ViewerCore {
    fn default() -> Self {
        Self {
            selection: ViewerSelection::default(),
            axis: AlignmentAxis::Step,
            brush: None,
            catalog: None,
            overview: None,
            detail: None,
            expected: [const { None }; 4],
            last_error: None,
        }
    }
}

impl ViewerCore {
    pub fn new(selection: ViewerSelection) -> Self {
        Self {
            selection,
            ..Self::default()
        }
    }

    pub const fn selection(&self) -> &ViewerSelection {
        &self.selection
    }

    pub const fn axis(&self) -> AlignmentAxis {
        self.axis
    }

    pub const fn brush(&self) -> Option<BrushState> {
        self.brush
    }

    pub fn brush_mut(&mut self) -> Option<&mut BrushState> {
        self.brush.as_mut()
    }

    pub fn selected_viewport(&self) -> Option<AlignmentViewport> {
        let selected = self.brush?.selected();
        AlignmentViewport::new(
            selected.start().floor() as i64,
            selected.end().ceil() as i64,
        )
        .ok()
    }

    pub const fn catalog(&self) -> Option<&CatalogSnapshot> {
        self.catalog.as_ref()
    }

    pub fn overview(&self) -> Option<&CurveSnapshot> {
        self.overview.as_deref()
    }

    pub fn detail(&self) -> Option<&CurveSnapshot> {
        self.detail.as_deref()
    }

    pub fn overview_shared(&self) -> Option<Arc<CurveSnapshot>> {
        self.overview.as_ref().map(Arc::clone)
    }

    pub fn detail_shared(&self) -> Option<Arc<CurveSnapshot>> {
        self.detail.as_ref().map(Arc::clone)
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Clears all state associated with the currently open source.
    pub fn reset_source(&mut self, source_id: DataSourceId) {
        *self = Self::default();
        self.selection.source_id = Some(source_id);
    }

    pub fn select_project(&mut self, project_id: Option<ProjectId>) {
        if self.selection.project_id == project_id {
            return;
        }
        self.selection = ViewerSelection {
            source_id: self.selection.source_id.clone(),
            project_id,
            ..ViewerSelection::default()
        };
        if let Some(catalog) = self.catalog.as_mut() {
            catalog.runs.clear();
            catalog.metric_keys.clear();
        }
        self.clear_curves();
    }

    /// Toggles one Run while enforcing the product's comparison limit.
    ///
    /// # Errors
    ///
    /// Returns [`SelectionError::RunLimit`] when adding an eleventh Run.
    pub fn toggle_run(&mut self, run: RunRef) -> Result<bool, SelectionError> {
        let selected = toggle_run_selection(&mut self.selection.runs, run)?;
        self.clear_curves();
        Ok(selected)
    }

    pub fn select_metric(&mut self, metric_key: Option<MetricKey>) {
        if self.selection.metric_key != metric_key {
            self.selection.metric_key = metric_key;
            self.clear_curves();
        }
    }

    pub fn select_axis(&mut self, axis: AlignmentAxis) {
        if self.axis != axis {
            self.axis = axis;
            self.clear_curves();
        }
    }

    pub fn reset_view(&mut self) -> bool {
        let Some(brush) = self.brush.as_mut() else {
            return false;
        };
        brush.reset();
        true
    }

    pub fn set_timeline_home(&mut self, range: AlignmentViewport) {
        let Ok(home) = AxisRange::new(range.start() as f64, range.end() as f64) else {
            return;
        };
        let previous = self.brush.map(BrushState::selected);
        let Ok(mut brush) = BrushState::new(home) else {
            return;
        };
        if let Some(previous) = previous {
            let start = previous.start().clamp(home.start(), home.end());
            let end = previous.end().clamp(home.start(), home.end());
            if end - start >= 1. {
                let _ = brush.resize_start(start);
                let _ = brush.resize_end(end);
            }
        }
        self.brush = Some(brush);
    }

    pub fn install_overview(&mut self, snapshot: CurveSnapshot) {
        self.apply_overview(snapshot);
    }

    pub fn clear_timeline(&mut self) {
        self.brush = None;
    }

    /// Marks one request stream pending without clearing its current snapshot.
    pub fn begin(
        &mut self,
        generation: Generation,
        source_id: DataSourceId,
        request: &ReadRequest,
    ) {
        self.expected[kind_index(request.kind())] = Some(ExpectedRequest {
            generation,
            source_id,
        });
        self.last_error = None;
    }

    pub fn is_pending(&self, kind: ReadKind) -> bool {
        self.expected[kind_index(kind)].is_some()
    }

    /// Cancels ownership of in-flight results without clearing snapshots.
    pub fn cancel_pending(&mut self) {
        self.expected.fill(None);
    }

    /// Applies only the result currently expected for its independent stream.
    pub fn apply(&mut self, event: ReadEvent) -> ApplyOutcome {
        let index = kind_index(event.kind);
        let Some(expected) = self.expected[index].as_ref() else {
            return ApplyOutcome::IgnoredStale;
        };
        if expected.generation != event.generation || expected.source_id != event.source_id {
            return ApplyOutcome::IgnoredStale;
        }
        if event
            .result
            .as_ref()
            .is_ok_and(|snapshot| snapshot.kind() != event.kind)
        {
            return ApplyOutcome::IgnoredMismatched;
        }
        self.expected[index] = None;
        match event.result {
            Ok(ReadSnapshot::Catalog(snapshot)) => self.apply_catalog(snapshot),
            Ok(ReadSnapshot::Overview(snapshot)) => self.apply_overview(snapshot),
            Ok(ReadSnapshot::Detail(snapshot)) => self.detail = Some(Arc::new(snapshot)),
            Ok(ReadSnapshot::Inspector(_)) => {}
            Err(error) => self.last_error = Some(error.to_string()),
        }
        ApplyOutcome::Applied
    }

    fn apply_catalog(&mut self, snapshot: CatalogSnapshot) {
        let project_exists = self
            .selection
            .project_id
            .as_ref()
            .is_some_and(|project_id| {
                snapshot
                    .projects
                    .iter()
                    .any(|project| &project.project_id == project_id)
            });
        let previous = self.selection.clone();
        if !project_exists {
            self.selection = ViewerSelection {
                source_id: self.selection.source_id.clone(),
                ..ViewerSelection::default()
            };
        } else {
            self.selection.runs.retain(|selected| {
                snapshot.runs.iter().any(|run| {
                    run.project_id == selected.project_id && run.run_id == selected.run_id
                })
            });
            if self
                .selection
                .metric_key
                .as_ref()
                .is_some_and(|metric_key| !snapshot.metric_keys.iter().any(|key| key == metric_key))
            {
                self.selection.metric_key = None;
            }
        }
        if self.selection != previous {
            self.overview = None;
            self.detail = None;
            self.expected[kind_index(ReadKind::Overview)] = None;
            self.expected[kind_index(ReadKind::Detail)] = None;
        }
        self.catalog = Some(snapshot);
    }

    fn apply_overview(&mut self, snapshot: CurveSnapshot) {
        self.brush = snapshot.real_range.and_then(|range| {
            let home = AxisRange::new(range.start() as f64, range.end() as f64).ok()?;
            let previous = self.brush.map(BrushState::selected);
            let mut brush = BrushState::new(home).ok()?;
            if let Some(selected) = previous {
                brush.resize_start(selected.start()).ok()?;
                brush.resize_end(selected.end()).ok()?;
            }
            Some(brush)
        });
        if self.brush.is_none() {
            self.detail = None;
            self.expected[kind_index(ReadKind::Detail)] = None;
        }
        self.overview = Some(Arc::new(snapshot));
    }

    fn clear_curves(&mut self) {
        self.brush = None;
        self.overview = None;
        self.detail = None;
        self.expected[kind_index(ReadKind::Overview)] = None;
        self.expected[kind_index(ReadKind::Detail)] = None;
        self.last_error = None;
    }
}

/// Toggles one composite Run identity in a View-owned ordered selection.
///
/// # Errors
///
/// Returns [`SelectionError::RunLimit`] when adding an eleventh Run.
pub fn toggle_run_selection(runs: &mut Vec<RunRef>, run: RunRef) -> Result<bool, SelectionError> {
    if let Some(index) = runs.iter().position(|selected| selected == &run) {
        runs.remove(index);
        return Ok(false);
    }
    if runs.len() == MAX_SELECTED_RUNS {
        return Err(SelectionError::RunLimit);
    }
    runs.push(run);
    Ok(true)
}

const fn kind_index(kind: ReadKind) -> usize {
    match kind {
        ReadKind::Catalog => 0,
        ReadKind::Overview => 1,
        ReadKind::Detail => 2,
        ReadKind::Inspector => 3,
    }
}

#[cfg(test)]
mod tests {
    use pulseon_model::alignment::AlignmentViewport;

    use super::*;

    fn source_id(value: &str) -> DataSourceId {
        DataSourceId::from_string(value)
    }

    fn run_ref(source: &str, project: &str, run: &str) -> RunRef {
        RunRef::new(
            source_id(source),
            ProjectId::from_string(project),
            RunId::from_string(run),
        )
    }

    fn curves() -> CurveSnapshot {
        CurveSnapshot {
            viewport: AlignmentViewport::new(0, 1).expect("test viewport should be valid"),
            point_budget: 2_000,
            real_range: None,
            series: Vec::new(),
        }
    }

    fn detail_event(source: &str, generation: u64) -> ReadEvent {
        ReadEvent {
            source_id: source_id(source),
            generation: Generation(generation),
            kind: ReadKind::Detail,
            result: Ok(ReadSnapshot::Detail(curves())),
        }
    }

    #[test]
    fn detail_stays_visible_while_pending_and_stale_results_are_ignored() {
        let mut core = ViewerCore::default();
        let request = ReadRequest::Detail(crate::query::DetailRequest {
            selection: crate::query::CurveSelection {
                source_id: source_id("source-a"),
                runs: Vec::new(),
                metric_key: MetricKey::from_string("loss"),
                axis: pulseon_model::alignment::AlignmentAxis::Step,
            },
            viewport: AlignmentViewport::new(0, 1).expect("test viewport should be valid"),
            physical_width: 1_000,
        });
        core.begin(Generation(1), source_id("source-a"), &request);
        assert_eq!(
            core.apply(detail_event("source-a", 1)),
            ApplyOutcome::Applied
        );
        core.begin(Generation(2), source_id("source-a"), &request);

        assert!(core.detail().is_some() && core.is_pending(ReadKind::Detail));
        assert_eq!(
            core.apply(detail_event("source-b", 2)),
            ApplyOutcome::IgnoredStale
        );
        assert_eq!(
            core.apply(detail_event("source-a", 1)),
            ApplyOutcome::IgnoredStale
        );
        assert!(core.detail().is_some() && core.is_pending(ReadKind::Detail));
    }

    #[test]
    fn shared_detail_handles_reuse_the_snapshot_allocation() {
        let core = ViewerCore {
            detail: Some(Arc::new(curves())),
            ..ViewerCore::default()
        };

        let first = core
            .detail_shared()
            .expect("detail snapshot should be available");
        let second = core
            .detail_shared()
            .expect("detail snapshot should be available");

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn refresh_removes_missing_selection_and_curve_snapshots() {
        let source_id = source_id("source-a");
        let mut core = ViewerCore::new(ViewerSelection {
            source_id: Some(source_id.clone()),
            project_id: Some(ProjectId::from_string("removed")),
            runs: vec![run_ref("source-a", "removed", "run-1")],
            metric_key: Some(MetricKey::from_string("loss")),
        });
        core.detail = Some(Arc::new(curves()));
        let request = ReadRequest::Discover(crate::model::DiscoveryRequest::default());
        core.begin(Generation(1), source_id.clone(), &request);
        let event = ReadEvent {
            source_id: source_id.clone(),
            generation: Generation(1),
            kind: ReadKind::Catalog,
            result: Ok(ReadSnapshot::Catalog(CatalogSnapshot {
                projects: Vec::new(),
                runs: Vec::new(),
                metric_keys: Vec::new(),
            })),
        };

        assert_eq!(core.apply(event), ApplyOutcome::Applied);
        assert_eq!(
            core.selection(),
            &ViewerSelection {
                source_id: Some(source_id),
                ..ViewerSelection::default()
            }
        );
        assert!(core.detail().is_none());
    }

    #[test]
    fn selection_transitions_clear_only_dependent_state() {
        let mut core = ViewerCore::default();
        core.select_project(Some(ProjectId::from_string("project-1")));
        assert!(
            core.toggle_run(run_ref("source-a", "project-1", "run-1"))
                .expect("first Run should be selectable")
        );
        core.select_metric(Some(MetricKey::from_string("loss")));

        core.select_axis(AlignmentAxis::ElapsedTime);

        assert_eq!(core.axis(), AlignmentAxis::ElapsedTime);
        assert_eq!(core.selection().runs.len(), 1);
        assert_eq!(
            core.selection().metric_key.as_ref().map(MetricKey::as_str),
            Some("loss")
        );
        assert!(core.overview().is_none() && core.detail().is_none());
    }

    #[test]
    fn run_selection_enforces_the_ten_run_limit() {
        let mut core = ViewerCore::default();
        for index in 0..MAX_SELECTED_RUNS {
            core.toggle_run(run_ref("source-a", "project-1", &format!("run-{index}")))
                .expect("first ten Runs should be selectable");
        }

        assert_eq!(
            core.toggle_run(run_ref("source-a", "project-1", "run-10")),
            Err(SelectionError::RunLimit)
        );
        assert_eq!(core.selection().runs.len(), MAX_SELECTED_RUNS);
    }

    #[test]
    fn view_selection_limit_counts_runs_across_sources() {
        let mut runs = (0..MAX_SELECTED_RUNS)
            .map(|index| run_ref(&format!("source-{index}"), "project", "run"))
            .collect::<Vec<_>>();

        assert_eq!(
            toggle_run_selection(
                &mut runs,
                run_ref("another-source", "project", "another-run")
            ),
            Err(SelectionError::RunLimit)
        );
    }

    #[test]
    fn run_identity_distinguishes_identical_native_ids_from_different_sources() {
        let first = run_ref("source-a", "project-1", "run-1");
        let second = run_ref("source-b", "project-1", "run-1");

        assert_ne!(first, second);
        assert_ne!(first.cache_key(), second.cache_key());
    }

    #[test]
    fn run_filter_matches_name_id_and_status_without_case() {
        assert!(
            ["loss", "run-42", "FAILED", ""]
                .into_iter()
                .all(|query| run_fields_match_filter("Loss Baseline", "RUN-42", "failed", query))
        );
        assert!(!run_fields_match_filter(
            "Loss Baseline",
            "RUN-42",
            "failed",
            "running"
        ));
    }

    #[test]
    fn overview_without_a_real_range_removes_stale_detail() {
        let mut core = ViewerCore {
            detail: Some(Arc::new(curves())),
            ..ViewerCore::default()
        };

        core.apply_overview(curves());

        assert!(core.brush().is_none());
        assert!(core.detail().is_none());
    }

    #[test]
    fn view_commands_switch_axis_and_reset_the_brush() {
        let mut core = ViewerCore::default();
        let mut overview = curves();
        overview.real_range =
            Some(AlignmentViewport::new(0, 10).expect("test overview range should be valid"));
        core.apply_overview(overview);
        core.brush_mut()
            .expect("overview should initialize brush")
            .resize_start(4.)
            .expect("test brush should resize");

        core.select_axis(AlignmentAxis::ElapsedTime);
        assert_eq!(core.axis(), AlignmentAxis::ElapsedTime);
        assert!(core.brush().is_none());

        core.apply_overview({
            let mut snapshot = curves();
            snapshot.real_range =
                Some(AlignmentViewport::new(0, 10).expect("test overview range should be valid"));
            snapshot
        });
        core.brush_mut()
            .expect("overview should initialize brush")
            .resize_start(4.)
            .expect("test brush should resize");
        assert!(core.reset_view());
        let brush = core.brush().expect("reset should retain brush");
        assert_eq!(brush.selected(), brush.home());
    }
}
