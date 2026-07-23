use crate::coordination::AnalysisViewId;
use crate::core::{DataSourceId, RunRef, SelectionError, ViewerCore, toggle_run_selection};
use pulseon_model::metric::MetricKey;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrackDensity {
    Compact,
    #[default]
    Comfortable,
    Spacious,
}

#[derive(Clone)]
pub struct AnalysisView {
    pub view_id: AnalysisViewId,
    pub name: String,
    pub runs: Vec<RunRef>,
    pub metrics: Vec<MetricKey>,
    pub track_density: TrackDensity,
    pub core: ViewerCore,
    pub local_error: Option<String>,
    pub overview_revision: u64,
    pub detail_revision: u64,
}

pub struct AnalysisViews {
    views: Vec<AnalysisView>,
    active_view_id: AnalysisViewId,
    next_id: u64,
}

impl Default for AnalysisViews {
    fn default() -> Self {
        let view = AnalysisView {
            view_id: AnalysisViewId::from_string("view-1"),
            name: "View 1".to_owned(),
            runs: Vec::new(),
            metrics: Vec::new(),
            track_density: TrackDensity::default(),
            core: ViewerCore::default(),
            local_error: None,
            overview_revision: 0,
            detail_revision: 0,
        };
        Self {
            active_view_id: view.view_id.clone(),
            views: vec![view],
            next_id: 2,
        }
    }
}

impl AnalysisViews {
    pub fn views(&self) -> &[AnalysisView] {
        &self.views
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
            metrics: Vec::new(),
            track_density: TrackDensity::default(),
            core: ViewerCore::default(),
            local_error: None,
            overview_revision: 0,
            detail_revision: 0,
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
            metrics: active.metrics,
            track_density: active.track_density,
            core: active.core,
            local_error: active.local_error,
            overview_revision: active.overview_revision,
            detail_revision: active.detail_revision,
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
        toggle_run_selection(&mut self.active_mut().runs, run)
    }

    pub fn select_active_metric(&mut self, metric_key: MetricKey) {
        if !self.active().metrics.contains(&metric_key) {
            self.active_mut().metrics.push(metric_key);
        }
    }

    pub fn remove_source(&mut self, source_id: &DataSourceId) {
        for view in &mut self.views {
            view.runs.retain(|run| &run.source_id != source_id);
        }
    }

    fn next_view_id(&mut self) -> AnalysisViewId {
        let view_id = AnalysisViewId::from_string(format!("view-{}", self.next_id));
        self.next_id = self.next_id.saturating_add(1);
        view_id
    }
}

#[cfg(test)]
mod tests {
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
        views.select_active_metric(MetricKey::from_string("loss"));
        views
            .active_mut()
            .core
            .select_axis(pulseon_model::alignment::AlignmentAxis::ElapsedTime);

        let duplicate = views.duplicate_active();
        views.active_mut().runs.clear();
        views.active_mut().metrics.clear();
        views
            .active_mut()
            .core
            .select_axis(pulseon_model::alignment::AlignmentAxis::Step);
        assert!(views.activate(&AnalysisViewId::from_string("view-1")));

        assert_eq!(views.active().runs.len(), 1);
        assert_eq!(views.active().metrics, [MetricKey::from_string("loss")]);
        assert_eq!(
            views.active().core.axis(),
            pulseon_model::alignment::AlignmentAxis::ElapsedTime
        );
        assert!(views.activate(&duplicate));
        assert!(views.active().runs.is_empty());
        assert!(views.active().metrics.is_empty());
        assert_eq!(
            views.active().core.axis(),
            pulseon_model::alignment::AlignmentAxis::Step
        );
    }
}
