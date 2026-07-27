use std::fmt;
use std::path::Path;

use pulseon_chart_core::{AxisRange, BrushState};
use pulseon_model::alignment::{AlignmentAxis, AlignmentViewport};
use pulseon_model::run::{Run, RunId, RunStatus};
use pulseon_model::types::ProjectId;

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

/// Invalid user selection transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SelectionError {
    #[error("at most {MAX_SELECTED_RUNS} Runs can be selected")]
    RunLimit,
}

/// Per-View navigation state shared by the brush, ruler, and Metric tracks.
#[derive(Clone)]
pub struct ViewNavigation {
    axis: AlignmentAxis,
    brush: Option<BrushState>,
}

impl Default for ViewNavigation {
    fn default() -> Self {
        Self {
            axis: AlignmentAxis::Step,
            brush: None,
        }
    }
}

impl ViewNavigation {
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

    pub fn select_axis(&mut self, axis: AlignmentAxis) {
        if self.axis != axis {
            self.axis = axis;
            self.brush = None;
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

    pub fn clear_timeline(&mut self) {
        self.brush = None;
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

#[cfg(test)]
mod tests {
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
    fn view_commands_switch_axis_and_reset_the_brush() {
        let mut navigation = ViewNavigation::default();
        navigation.set_timeline_home(
            AlignmentViewport::new(0, 10).expect("test viewport should be valid"),
        );
        navigation
            .brush_mut()
            .expect("timeline should initialize brush")
            .resize_start(4.)
            .expect("test brush should resize");

        navigation.select_axis(AlignmentAxis::ElapsedTime);
        assert_eq!(navigation.axis(), AlignmentAxis::ElapsedTime);
        assert!(navigation.brush().is_none());

        navigation.set_timeline_home(
            AlignmentViewport::new(0, 10).expect("test viewport should be valid"),
        );
        navigation
            .brush_mut()
            .expect("timeline should initialize brush")
            .resize_start(4.)
            .expect("test brush should resize");
        assert!(navigation.reset_view());
        let brush = navigation.brush().expect("reset should retain brush");
        assert_eq!(brush.selected(), brush.home());
    }
}
