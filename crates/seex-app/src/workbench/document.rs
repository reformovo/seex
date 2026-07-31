#![expect(
    dead_code,
    reason = "legacy parser helpers are removed in the next cleanup slice"
)]

use std::path::PathBuf;

use seex_chart_core::AxisRange;
use seex_model::alignment::AlignmentAxis;
use seex_model::run::RunId;
use seex_model::types::ProjectId;

const HEADER: &str = "seex-workbench 1";

#[derive(Clone, Debug, PartialEq)]
pub struct WorkbenchDocument {
    pub sources: Vec<PathBuf>,
    pub pinned_projects: Vec<SavedProjectRef>,
    pub archived_projects: Vec<SavedProjectRef>,
    pub removed_projects: Vec<SavedProjectRef>,
    pub archived_runs: Vec<SavedRunRef>,
    pub views: Vec<SavedAnalysisView>,
    pub active_view: usize,
    pub project_sidebar_visible: bool,
    pub project_sidebar_width: f32,
    pub metric_sidebar_compact: bool,
    pub bottom_inspector_visible: bool,
    pub bottom_inspector_height: f32,
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
    pub source_path: PathBuf,
    pub project_id: ProjectId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedRunRef {
    pub source_path: PathBuf,
    pub project_id: ProjectId,
    pub run_id: RunId,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkbenchDocumentError {
    #[error("failed to read workbench document at {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write workbench document at {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unsupported workbench document header {0:?}")]
    UnsupportedVersion(String),
    #[error("invalid workbench document at line {line}: {message}")]
    Invalid { line: usize, message: String },
}

fn decode_project_ref(
    source: &str,
    project: &str,
    line: usize,
) -> Result<SavedProjectRef, WorkbenchDocumentError> {
    Ok(SavedProjectRef {
        source_path: PathBuf::from(decode(source, line)?),
        project_id: ProjectId::from_string(decode(project, line)?),
    })
}

fn decode_run_ref(
    source: &str,
    project: &str,
    run: &str,
    line: usize,
) -> Result<SavedRunRef, WorkbenchDocumentError> {
    let project = decode_project_ref(source, project, line)?;
    Ok(SavedRunRef {
        source_path: project.source_path,
        project_id: project.project_id,
        run_id: RunId::from_string(decode(run, line)?),
    })
}

fn decode(value: &str, line: usize) -> Result<String, WorkbenchDocumentError> {
    if !value.len().is_multiple_of(2) {
        return Err(invalid_value(line, "hex value has an odd length"));
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid_value(line, "hex value contains a non-hex digit"))?;
    String::from_utf8(bytes).map_err(|_| invalid_value(line, "hex value is not UTF-8"))
}

fn parse<T: std::str::FromStr>(value: &str, line: usize) -> Result<T, WorkbenchDocumentError> {
    value
        .parse()
        .map_err(|_| invalid_value(line, "invalid numeric value"))
}

fn parse_bool(value: &str, line: usize) -> Result<bool, WorkbenchDocumentError> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(invalid_value(line, "invalid boolean value")),
    }
}

fn parse_axis(value: &str, line: usize) -> Result<AlignmentAxis, WorkbenchDocumentError> {
    match value {
        "step" => Ok(AlignmentAxis::Step),
        "elapsed" => Ok(AlignmentAxis::ElapsedTime),
        _ => Err(invalid_value(line, "invalid enum value")),
    }
}

fn invalid_value(line: usize, message: &str) -> WorkbenchDocumentError {
    WorkbenchDocumentError::Invalid {
        line,
        message: message.to_owned(),
    }
}
