use std::collections::HashMap;
use std::sync::Arc;

use pulseon_model::alignment::AlignmentViewport;
use pulseon_model::comparison::ObjectiveDirection;
use pulseon_model::metric::MetricKey;

use crate::coordination::{AnalysisViewId, MetricPanelId, SourceReadFailure};
use crate::core::{DataSourceId, RunRef, SelectionError, ViewerCore, toggle_run_selection};
use crate::query::{CurveSnapshot, InspectorSnapshot};
use crate::worker::{Generation, ReadKind};

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

#[derive(Clone)]
pub struct MetricPanel {
    pub panel_id: MetricPanelId,
    pub metric_key: MetricKey,
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
    active_view_id: AnalysisViewId,
    next_id: u64,
}

impl Default for AnalysisViews {
    fn default() -> Self {
        let view = AnalysisView {
            view_id: AnalysisViewId::from_string("view-1"),
            name: "View 1".to_owned(),
            runs: Vec::new(),
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
        let selected = toggle_run_selection(&mut self.active_mut().runs, run)?;
        self.invalidate_active_panels();
        Ok(selected)
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

    pub fn set_active_inspector_tab(&mut self, tab: InspectorTab) {
        self.active_mut().inspector_tab = tab;
    }

    pub fn set_active_ranking_direction(&mut self, direction: ObjectiveDirection) {
        self.active_mut().ranking_direction = Some(direction);
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
            if view.runs.len() != previous {
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
        views.active_mut().panels.clear();
        views
            .active_mut()
            .core
            .select_axis(pulseon_model::alignment::AlignmentAxis::Step);
        assert!(views.activate(&AnalysisViewId::from_string("view-1")));

        assert_eq!(views.active().runs.len(), 1);
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
