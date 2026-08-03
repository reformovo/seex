//! Schema-v1 TOML workbench model.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use seex::AlignmentAxis;
use seex::ProjectId;
use seex::RunId;
use seex_plot::AxisRange;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

use crate::domain::SourceAlias;

pub const WORKBENCH_SCHEMA_VERSION: i64 = 1;
static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

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
    /// Loads a workbench document when it exists.
    ///
    /// # Errors
    ///
    /// Returns [`TomlWorkbenchError`] for I/O, TOML, schema, or validation
    /// failures.
    pub fn load(path: &Path) -> Result<Option<Self>, TomlWorkbenchError> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(TomlWorkbenchError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        Self::decode(&raw).map(Some)
    }

    /// Decodes a schema-v1 TOML workbench document.
    ///
    /// # Errors
    ///
    /// Returns [`TomlWorkbenchError`] when the TOML, schema version, or a typed
    /// workbench field is invalid.
    pub fn decode(raw: &str) -> Result<Self, TomlWorkbenchError> {
        let document = raw.parse::<DocumentMut>()?;
        if document["schema_version"].as_integer() != Some(WORKBENCH_SCHEMA_VERSION) {
            return Err(TomlWorkbenchError::UnsupportedSchema);
        }
        for field in ["sources", "projects", "removed_projects", "path", "s3"] {
            if document.as_table().contains_key(field) {
                return Err(TomlWorkbenchError::InvalidField(field));
            }
        }
        let workbench = Self {
            active_view: document["active_view"]
                .as_integer()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(TomlWorkbenchError::InvalidField("active_view"))?,
            layout: layout(document.get("layout"))?,
            expanded_projects: table_array(
                document.get("expanded_projects"),
                "expanded_projects",
                project_ref,
            )?,
            pinned_projects: table_array(
                document.get("pinned_projects"),
                "pinned_projects",
                project_ref,
            )?,
            archived_projects: table_array(
                document.get("archived_projects"),
                "archived_projects",
                project_ref,
            )?,
            archived_runs: table_array(document.get("archived_runs"), "archived_runs", run_ref)?,
            views: table_array(document.get("views"), "views", view)?,
        };
        workbench.validate()?;
        Ok(workbench)
    }

    /// Validates semantic schema-v1 workbench invariants.
    ///
    /// # Errors
    ///
    /// Returns [`TomlWorkbenchError::InvalidField`] for inconsistent state.
    pub fn validate(&self) -> Result<(), TomlWorkbenchError> {
        if (self.views.is_empty() && self.active_view != 0)
            || (!self.views.is_empty() && self.active_view >= self.views.len())
        {
            return Err(TomlWorkbenchError::InvalidField("active_view"));
        }
        if !positive(self.layout.project_sidebar_width)
            || !positive(self.layout.bottom_inspector_height)
        {
            return Err(TomlWorkbenchError::InvalidField("layout"));
        }
        validate_projects(&self.expanded_projects, "expanded_projects")?;
        validate_projects(&self.pinned_projects, "pinned_projects")?;
        validate_projects(&self.archived_projects, "archived_projects")?;
        validate_runs(&self.archived_runs, "archived_runs")?;
        self.views.iter().try_for_each(validate_view)
    }

    /// Atomically saves a validated workbench through a same-directory file.
    ///
    /// # Errors
    ///
    /// Returns [`TomlWorkbenchError`] for invalid state or I/O failures.
    pub fn save(&self, path: &Path) -> Result<(), TomlWorkbenchError> {
        self.validate()?;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| TomlWorkbenchError::Write {
                path: path.to_owned(),
                source,
            })?;
        }
        let temporary = temporary_path(path);
        let write = || -> Result<(), io::Error> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(self.encode().as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, path)
        };
        if let Err(source) = write() {
            let _ = fs::remove_file(&temporary);
            return Err(TomlWorkbenchError::Write {
                path: path.to_owned(),
                source,
            });
        }
        Ok(())
    }

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

#[derive(Debug, thiserror::Error)]
pub enum TomlWorkbenchError {
    #[error("failed to read workbench document at {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write workbench document at {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("workbench schema_version must be 1")]
    UnsupportedSchema,
    #[error("invalid workbench field {0}")]
    InvalidField(&'static str),
    #[error("invalid workbench TOML: {0}")]
    Parse(#[from] toml_edit::TomlError),
}

fn temporary_path(path: &Path) -> PathBuf {
    let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(".{name}.{}.{id}.tmp", std::process::id()))
}

fn layout(item: Option<&Item>) -> Result<SavedLayout, TomlWorkbenchError> {
    let table = item
        .and_then(Item::as_table)
        .ok_or(TomlWorkbenchError::InvalidField("layout"))?;
    Ok(SavedLayout {
        project_sidebar_visible: bool_field(table, "project_sidebar_visible")?,
        project_sidebar_width: float_field(table, "project_sidebar_width")?,
        metric_sidebar_compact: bool_field(table, "metric_sidebar_compact")?,
        bottom_inspector_visible: bool_field(table, "bottom_inspector_visible")?,
        bottom_inspector_height: float_field(table, "bottom_inspector_height")?,
    })
}

fn table_array<T>(
    item: Option<&Item>,
    field: &'static str,
    parse: fn(&Table) -> Result<T, TomlWorkbenchError>,
) -> Result<Vec<T>, TomlWorkbenchError> {
    match item {
        None => Ok(Vec::new()),
        Some(item) => item
            .as_array_of_tables()
            .ok_or(TomlWorkbenchError::InvalidField(field))?
            .iter()
            .map(parse)
            .collect(),
    }
}

fn view(table: &Table) -> Result<SavedAnalysisView, TomlWorkbenchError> {
    let metrics = strings(table.get("metrics"), "metrics")?;
    Ok(SavedAnalysisView {
        name: string(table, "name")?.to_owned(),
        runs: table_array(table.get("runs"), "runs", run_ref)?,
        baseline: optional(table.get("baseline"), "baseline", Item::as_table)?
            .map(run_ref)
            .transpose()?,
        pinned_runs: table_array(table.get("pinned_runs"), "pinned_runs", run_ref)?,
        metrics,
        metric_heights: metric_heights(table.get("metric_heights"))?,
        selected_metric: optional(table.get("selected_metric"), "selected_metric", |item| {
            item.as_str().map(str::to_owned)
        })?,
        axis: match string(table, "axis")? {
            "step" => AlignmentAxis::Step,
            "timestamp" => AlignmentAxis::ElapsedTime,
            _ => return Err(TomlWorkbenchError::InvalidField("axis")),
        },
        viewport: viewport(table.get("viewport"))?,
    })
}

fn project_ref(table: &Table) -> Result<SavedProjectRef, TomlWorkbenchError> {
    if table.contains_key("path") || table.contains_key("projects") {
        return Err(TomlWorkbenchError::InvalidField("source"));
    }
    Ok(SavedProjectRef {
        source_alias: SourceAlias::new(string(table, "source")?)
            .map_err(|_| TomlWorkbenchError::InvalidField("source"))?,
        project_id: ProjectId::from_string(string(table, "project_id")?),
    })
}

fn run_ref(table: &Table) -> Result<SavedRunRef, TomlWorkbenchError> {
    let project = project_ref(table)?;
    Ok(SavedRunRef {
        source_alias: project.source_alias,
        project_id: project.project_id,
        run_id: RunId::from_string(string(table, "run_id")?),
    })
}

fn validate_view(view: &SavedAnalysisView) -> Result<(), TomlWorkbenchError> {
    if view.name.is_empty() {
        return Err(TomlWorkbenchError::InvalidField("name"));
    }
    validate_runs(&view.runs, "runs")?;
    validate_runs(&view.pinned_runs, "pinned_runs")?;
    if let Some(baseline) = &view.baseline {
        validate_run(baseline, "baseline")?;
    }
    let mut metrics = HashSet::with_capacity(view.metrics.len());
    if !view
        .metrics
        .iter()
        .all(|metric| !metric.is_empty() && metrics.insert(metric.as_str()))
    {
        return Err(TomlWorkbenchError::InvalidField("metrics"));
    }
    if view
        .selected_metric
        .as_ref()
        .is_some_and(|metric| !metrics.contains(metric.as_str()))
    {
        return Err(TomlWorkbenchError::InvalidField("selected_metric"));
    }
    if !view
        .metric_heights
        .iter()
        .all(|(metric, height)| metrics.contains(metric.as_str()) && positive(*height))
    {
        return Err(TomlWorkbenchError::InvalidField("metric_heights"));
    }
    Ok(())
}

fn validate_projects(
    projects: &[SavedProjectRef],
    field: &'static str,
) -> Result<(), TomlWorkbenchError> {
    let mut seen = HashSet::with_capacity(projects.len());
    if projects.iter().all(|project| {
        !project.project_id.as_str().is_empty()
            && seen.insert((project.source_alias.as_str(), project.project_id.as_str()))
    }) {
        Ok(())
    } else {
        Err(TomlWorkbenchError::InvalidField(field))
    }
}

fn validate_runs(runs: &[SavedRunRef], field: &'static str) -> Result<(), TomlWorkbenchError> {
    let mut seen = HashSet::with_capacity(runs.len());
    if runs.iter().all(|run| {
        validate_run(run, field).is_ok()
            && seen.insert((
                run.source_alias.as_str(),
                run.project_id.as_str(),
                run.run_id.as_str(),
            ))
    }) {
        Ok(())
    } else {
        Err(TomlWorkbenchError::InvalidField(field))
    }
}

fn validate_run(run: &SavedRunRef, field: &'static str) -> Result<(), TomlWorkbenchError> {
    if run.project_id.as_str().is_empty() || run.run_id.as_str().is_empty() {
        Err(TomlWorkbenchError::InvalidField(field))
    } else {
        Ok(())
    }
}

fn positive(value: f32) -> bool {
    value.is_finite() && value > 0.
}

fn strings(item: Option<&Item>, field: &'static str) -> Result<Vec<String>, TomlWorkbenchError> {
    item.and_then(Item::as_array)
        .ok_or(TomlWorkbenchError::InvalidField(field))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or(TomlWorkbenchError::InvalidField(field))
        })
        .collect()
}

fn metric_heights(item: Option<&Item>) -> Result<Vec<(String, f32)>, TomlWorkbenchError> {
    let Some(table) = optional(item, "metric_heights", Item::as_table)? else {
        return Ok(Vec::new());
    };
    table
        .iter()
        .map(|(metric, item)| {
            item.as_float()
                .map(|height| (metric.to_owned(), height as f32))
                .ok_or(TomlWorkbenchError::InvalidField("metric_heights"))
        })
        .collect()
}

fn viewport(item: Option<&Item>) -> Result<Option<AxisRange>, TomlWorkbenchError> {
    let Some(item) = item else {
        return Ok(None);
    };
    let array = item
        .as_array()
        .ok_or(TomlWorkbenchError::InvalidField("viewport"))?;
    let mut values = array.iter();
    let (Some(start), Some(end), None) = (values.next(), values.next(), values.next()) else {
        return Err(TomlWorkbenchError::InvalidField("viewport"));
    };
    AxisRange::new(
        start
            .as_float()
            .ok_or(TomlWorkbenchError::InvalidField("viewport"))?,
        end.as_float()
            .ok_or(TomlWorkbenchError::InvalidField("viewport"))?,
    )
    .map(Some)
    .map_err(|_| TomlWorkbenchError::InvalidField("viewport"))
}

fn string<'a>(table: &'a Table, field: &'static str) -> Result<&'a str, TomlWorkbenchError> {
    table
        .get(field)
        .and_then(Item::as_str)
        .ok_or(TomlWorkbenchError::InvalidField(field))
}

fn bool_field(table: &Table, field: &'static str) -> Result<bool, TomlWorkbenchError> {
    table
        .get(field)
        .and_then(Item::as_bool)
        .ok_or(TomlWorkbenchError::InvalidField(field))
}

fn float_field(table: &Table, field: &'static str) -> Result<f32, TomlWorkbenchError> {
    table
        .get(field)
        .and_then(Item::as_float)
        .map(|value| value as f32)
        .ok_or(TomlWorkbenchError::InvalidField(field))
}

fn optional<'a, T>(
    item: Option<&'a Item>,
    field: &'static str,
    parse: impl FnOnce(&'a Item) -> Option<T>,
) -> Result<Option<T>, TomlWorkbenchError> {
    item.map(|item| parse(item).ok_or(TomlWorkbenchError::InvalidField(field)))
        .transpose()
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

    fn document() -> TomlWorkbenchDocument {
        let source_alias = SourceAlias::new("research").expect("test alias should be valid");
        let project = SavedProjectRef {
            source_alias: source_alias.clone(),
            project_id: ProjectId::from_string("project-1"),
        };
        TomlWorkbenchDocument {
            active_view: 0,
            layout: SavedLayout::default(),
            expanded_projects: vec![project.clone()],
            pinned_projects: vec![project.clone()],
            archived_projects: Vec::new(),
            archived_runs: Vec::new(),
            views: vec![SavedAnalysisView {
                name: "Training".to_owned(),
                runs: vec![SavedRunRef {
                    source_alias,
                    project_id: project.project_id,
                    run_id: RunId::from_string("run-1"),
                }],
                baseline: None,
                pinned_runs: Vec::new(),
                metrics: vec!["loss".to_owned()],
                metric_heights: vec![("loss".to_owned(), 160.)],
                selected_metric: Some("loss".to_owned()),
                axis: AlignmentAxis::Step,
                viewport: Some(AxisRange::new(1., 5.).expect("test range should be valid")),
            }],
        }
    }

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
        let document = document();

        let encoded = document.encode();
        let decoded =
            TomlWorkbenchDocument::decode(&encoded).expect("encoded workbench should decode");
        let parsed = encoded
            .parse::<DocumentMut>()
            .expect("encoded workbench should be valid TOML");

        assert_eq!(decoded, document);
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

    #[test]
    fn duplicate_run_reference_is_rejected() {
        let mut document = document();
        let duplicate = document.views[0].runs[0].clone();
        document.views[0].runs.push(duplicate);

        assert!(matches!(
            document.validate(),
            Err(TomlWorkbenchError::InvalidField("runs"))
        ));
    }

    #[test]
    fn schema_v1_file_round_trips_without_temporary_residue()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join(".seex/workbench.toml");

        assert_eq!(TomlWorkbenchDocument::load(&path)?, None);
        document().save(&path)?;

        assert_eq!(TomlWorkbenchDocument::load(&path)?, Some(document()));
        assert_eq!(
            fs::read_dir(path.parent().ok_or("missing parent")?)?.count(),
            1
        );
        Ok(())
    }

    #[test]
    fn invalid_state_does_not_overwrite_existing_file() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("workbench.toml");
        fs::write(&path, "existing invalid document")?;
        let mut invalid = document();
        invalid.active_view = 1;

        let error = invalid
            .save(&path)
            .expect_err("invalid workbench should not be saved");

        assert!(matches!(
            error,
            TomlWorkbenchError::InvalidField("active_view")
        ));
        assert_eq!(fs::read_to_string(path)?, "existing invalid document");
        Ok(())
    }
}
