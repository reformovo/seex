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

impl WorkbenchDocument {
    pub fn decode(raw: &str) -> Result<Self, WorkbenchDocumentError> {
        let mut lines = raw.lines().enumerate();
        let header = lines.next().map(|(_, line)| line).unwrap_or_default();
        if header != HEADER {
            return Err(WorkbenchDocumentError::UnsupportedVersion(
                header.to_owned(),
            ));
        }
        let mut document = Self {
            sources: Vec::new(),
            pinned_projects: Vec::new(),
            archived_projects: Vec::new(),
            removed_projects: Vec::new(),
            archived_runs: Vec::new(),
            views: Vec::new(),
            active_view: 0,
            project_sidebar_visible: true,
            project_sidebar_width: 190.,
            metric_sidebar_compact: false,
            bottom_inspector_visible: false,
            bottom_inspector_height: 220.,
        };
        let mut current: Option<SavedAnalysisView> = None;
        for (index, line) in lines {
            let line_number = index + 1;
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let invalid = |message: &str| WorkbenchDocumentError::Invalid {
                line: line_number,
                message: message.to_owned(),
            };
            match fields.as_slice() {
                [
                    "dock",
                    project,
                    project_width,
                    metric,
                    inspector,
                    inspector_height,
                ] => {
                    document.project_sidebar_visible = parse_bool(project, line_number)?;
                    document.project_sidebar_width =
                        f32::from_bits(parse(project_width, line_number)?);
                    document.metric_sidebar_compact = parse_bool(metric, line_number)?;
                    document.bottom_inspector_visible = parse_bool(inspector, line_number)?;
                    document.bottom_inspector_height =
                        f32::from_bits(parse(inspector_height, line_number)?);
                }
                ["active", active] => document.active_view = parse(active, line_number)?,
                ["source", path] => document
                    .sources
                    .push(PathBuf::from(decode(path, line_number)?)),
                ["view", name, axis, selected, start, end] => {
                    if let Some(view) = current.take() {
                        document.views.push(view);
                    }
                    current = Some(SavedAnalysisView {
                        name: decode(name, line_number)?,
                        runs: Vec::new(),
                        baseline: None,
                        pinned_runs: Vec::new(),
                        metrics: Vec::new(),
                        metric_heights: Vec::new(),
                        selected_metric: (*selected != "-")
                            .then(|| decode(selected, line_number))
                            .transpose()?,
                        axis: parse_axis(axis, line_number)?,
                        viewport: if *start == "-" && *end == "-" {
                            None
                        } else {
                            Some(
                                AxisRange::new(
                                    f64::from_bits(parse(start, line_number)?),
                                    f64::from_bits(parse(end, line_number)?),
                                )
                                .map_err(|error| invalid(&error.to_string()))?,
                            )
                        },
                    });
                }
                ["run", source, project, run] => current
                    .as_mut()
                    .ok_or_else(|| invalid("run appears outside a view"))?
                    .runs
                    .push(SavedRunRef {
                        source_path: PathBuf::from(decode(source, line_number)?),
                        project_id: ProjectId::from_string(decode(project, line_number)?),
                        run_id: RunId::from_string(decode(run, line_number)?),
                    }),
                ["baseline-run", source, project, run] => {
                    let baseline = decode_run_ref(source, project, run, line_number)?;
                    current
                        .as_mut()
                        .ok_or_else(|| invalid("baseline-run appears outside a view"))?
                        .baseline = Some(baseline);
                }
                ["pinned-run", source, project, run] => current
                    .as_mut()
                    .ok_or_else(|| invalid("pinned-run appears outside a view"))?
                    .pinned_runs
                    .push(decode_run_ref(source, project, run, line_number)?),
                ["archived-run", source, project, run] => document
                    .archived_runs
                    .push(decode_run_ref(source, project, run, line_number)?),
                ["pinned-project", source, project] => document
                    .pinned_projects
                    .push(decode_project_ref(source, project, line_number)?),
                ["archived-project", source, project] => document
                    .archived_projects
                    .push(decode_project_ref(source, project, line_number)?),
                ["removed-project", source, project] => document
                    .removed_projects
                    .push(decode_project_ref(source, project, line_number)?),
                ["metric", metric] => current
                    .as_mut()
                    .ok_or_else(|| invalid("metric appears outside a view"))?
                    .metrics
                    .push(decode(metric, line_number)?),
                ["metric-height", metric, height] => current
                    .as_mut()
                    .ok_or_else(|| invalid("metric-height appears outside a view"))?
                    .metric_heights
                    .push((
                        decode(metric, line_number)?,
                        f32::from_bits(parse(height, line_number)?),
                    )),
                ["end"] => {
                    let view = current
                        .take()
                        .ok_or_else(|| invalid("end appears outside a view"))?;
                    document.views.push(view);
                }
                [] => {}
                _ => return Err(invalid("unknown or malformed record")),
            }
        }
        if let Some(view) = current {
            document.views.push(view);
        }
        Ok(document)
    }
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
