//! Schema-v1 TOML workbench model.

use seex_chart_core::AxisRange;
use seex_model::alignment::AlignmentAxis;
use seex_model::run::RunId;
use seex_model::types::ProjectId;

use crate::domain::SourceAlias;

pub const WORKBENCH_SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct TomlWorkbenchDocument {
    pub active_view: usize,
    pub layout: SavedLayout,
    pub expanded_projects: Vec<SavedProjectRef>,
    pub pinned_projects: Vec<SavedProjectRef>,
    pub archived_projects: Vec<SavedProjectRef>,
    pub archived_runs: Vec<SavedRunRef>,
    pub views: Vec<SavedAnalysisView>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedLayout {
    pub project_sidebar_visible: bool,
    pub project_sidebar_width: f32,
    pub metric_sidebar_compact: bool,
    pub bottom_inspector_visible: bool,
    pub bottom_inspector_height: f32,
}

impl Default for SavedLayout {
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

#[derive(Clone, Debug, PartialEq)]
pub struct SavedAnalysisView {
    pub name: String,
    pub runs: Vec<SavedRunRef>,
    pub baseline: Option<SavedRunRef>,
    pub pinned_runs: Vec<SavedRunRef>,
    pub metrics: Vec<String>,
    pub metric_heights: Vec<(String, f32)>,
    pub selected_metric: Option<String>,
    pub axis: AlignmentAxis,
    pub viewport: Option<AxisRange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedProjectRef {
    pub source_alias: SourceAlias,
    pub project_id: ProjectId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedRunRef {
    pub source_alias: SourceAlias,
    pub project_id: ProjectId,
    pub run_id: RunId,
}

impl SavedRunRef {
    pub fn project_ref(&self) -> SavedProjectRef {
        SavedProjectRef {
            source_alias: self.source_alias.clone(),
            project_id: self.project_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_reference_derives_its_alias_qualified_project() {
        let run = SavedRunRef {
            source_alias: SourceAlias::new("research").expect("test alias should be valid"),
            project_id: ProjectId::from_string("project-1"),
            run_id: RunId::from_string("run-1"),
        };

        assert_eq!(run.project_ref().source_alias.as_str(), "research");
        assert_eq!(run.project_ref().project_id.as_str(), "project-1");
    }
}
