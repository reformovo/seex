//! Schema-v1 TOML workbench model.

use seex_chart_core::AxisRange;
use seex_model::alignment::AlignmentAxis;
use seex_model::run::RunId;
use seex_model::types::ProjectId;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

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

impl TomlWorkbenchDocument {
    pub fn encode(&self) -> String {
        let mut document = DocumentMut::new();
        document["schema_version"] = value(WORKBENCH_SCHEMA_VERSION);
        document["active_view"] = value(self.active_view as i64);
        document["layout"] = Item::Table(layout_table(self.layout));
        document["expanded_projects"] =
            Item::ArrayOfTables(project_tables(&self.expanded_projects));
        document["pinned_projects"] = Item::ArrayOfTables(project_tables(&self.pinned_projects));
        document["archived_projects"] =
            Item::ArrayOfTables(project_tables(&self.archived_projects));
        document["archived_runs"] = Item::ArrayOfTables(run_tables(&self.archived_runs));
        let mut views = ArrayOfTables::new();
        for view in &self.views {
            views.push(view_table(view));
        }
        document["views"] = Item::ArrayOfTables(views);
        document.to_string()
    }
}

fn layout_table(layout: SavedLayout) -> Table {
    let mut table = Table::new();
    table["project_sidebar_visible"] = value(layout.project_sidebar_visible);
    table["project_sidebar_width"] = value(f64::from(layout.project_sidebar_width));
    table["metric_sidebar_compact"] = value(layout.metric_sidebar_compact);
    table["bottom_inspector_visible"] = value(layout.bottom_inspector_visible);
    table["bottom_inspector_height"] = value(f64::from(layout.bottom_inspector_height));
    table
}

fn project_tables(projects: &[SavedProjectRef]) -> ArrayOfTables {
    let mut tables = ArrayOfTables::new();
    for project in projects {
        tables.push(project_table(project));
    }
    tables
}

fn project_table(project: &SavedProjectRef) -> Table {
    let mut table = Table::new();
    table["source"] = value(project.source_alias.as_str());
    table["project_id"] = value(project.project_id.as_str());
    table
}

fn run_tables(runs: &[SavedRunRef]) -> ArrayOfTables {
    let mut tables = ArrayOfTables::new();
    for run in runs {
        tables.push(run_table(run));
    }
    tables
}

fn run_table(run: &SavedRunRef) -> Table {
    let mut table = Table::new();
    table["source"] = value(run.source_alias.as_str());
    table["project_id"] = value(run.project_id.as_str());
    table["run_id"] = value(run.run_id.as_str());
    table
}

fn view_table(view: &SavedAnalysisView) -> Table {
    let mut table = Table::new();
    table["name"] = value(&view.name);
    table["axis"] = value(match view.axis {
        AlignmentAxis::Step => "step",
        AlignmentAxis::ElapsedTime => "timestamp",
    });
    if let Some(selected_metric) = &view.selected_metric {
        table["selected_metric"] = value(selected_metric);
    }
    table["metrics"] = value(string_array(&view.metrics));
    if let Some(viewport) = view.viewport {
        let mut range = Array::new();
        range.extend([viewport.start(), viewport.end()]);
        table["viewport"] = value(range);
    }
    table["runs"] = Item::ArrayOfTables(run_tables(&view.runs));
    if let Some(baseline) = &view.baseline {
        table["baseline"] = Item::Table(run_table(baseline));
    }
    table["pinned_runs"] = Item::ArrayOfTables(run_tables(&view.pinned_runs));
    let mut heights = Table::new();
    for (metric, height) in &view.metric_heights {
        heights[metric] = value(f64::from(*height));
    }
    table["metric_heights"] = Item::Table(heights);
    table
}

fn string_array(values: &[String]) -> Array {
    let mut array = Array::new();
    array.extend(values.iter().map(String::as_str));
    array
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

    #[test]
    fn schema_v1_encoding_contains_aliases_without_machine_paths() {
        let source_alias = SourceAlias::new("research").expect("test alias should be valid");
        let project = SavedProjectRef {
            source_alias: source_alias.clone(),
            project_id: ProjectId::from_string("project-1"),
        };
        let run = SavedRunRef {
            source_alias,
            project_id: project.project_id.clone(),
            run_id: RunId::from_string("run-1"),
        };
        let document = TomlWorkbenchDocument {
            active_view: 0,
            layout: SavedLayout::default(),
            expanded_projects: vec![project.clone()],
            pinned_projects: vec![project],
            archived_projects: Vec::new(),
            archived_runs: Vec::new(),
            views: vec![SavedAnalysisView {
                name: "Training".to_owned(),
                runs: vec![run],
                baseline: None,
                pinned_runs: Vec::new(),
                metrics: vec!["loss".to_owned()],
                metric_heights: vec![("loss".to_owned(), 160.)],
                selected_metric: Some("loss".to_owned()),
                axis: AlignmentAxis::Step,
                viewport: Some(AxisRange::new(1., 5.).expect("test range should be valid")),
            }],
        };

        let encoded = document.encode();
        let parsed = encoded
            .parse::<DocumentMut>()
            .expect("encoded workbench should be valid TOML");

        assert_eq!(parsed["schema_version"].as_integer(), Some(1));
        assert_eq!(
            parsed["views"][0]["runs"][0]["source"].as_str(),
            Some("research")
        );
        assert!(!encoded.contains("/tmp"));
        assert!(
            !encoded
                .lines()
                .any(|line| line.trim_start().starts_with("projects ="))
        );
        assert!(
            !encoded
                .lines()
                .any(|line| line.trim_start().starts_with("path ="))
        );
    }
}
