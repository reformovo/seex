use std::fs;
use std::path::{Path, PathBuf};

use pulseon_chart_core::AxisRange;
use pulseon_model::alignment::AlignmentAxis;
use pulseon_model::comparison::ObjectiveDirection;
use pulseon_model::run::RunId;
use pulseon_model::types::ProjectId;

use crate::workbench::{InspectorTab, TrackDensity};

const HEADER: &str = "pulseon-workbench 1";

#[derive(Clone, Debug, PartialEq)]
pub struct WorkbenchDocument {
    pub sources: Vec<PathBuf>,
    pub pinned_projects: Vec<SavedProjectRef>,
    pub archived_projects: Vec<SavedProjectRef>,
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
    pub selected_metric: Option<String>,
    pub inspector_tab: InspectorTab,
    pub ranking_direction: Option<ObjectiveDirection>,
    pub axis: AlignmentAxis,
    pub track_density: TrackDensity,
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
    pub fn save(&self, path: &Path) -> Result<(), WorkbenchDocumentError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| WorkbenchDocumentError::Write {
                path: path.to_owned(),
                source,
            })?;
        }
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, self.encode()).map_err(|source| WorkbenchDocumentError::Write {
            path: temporary.clone(),
            source,
        })?;
        fs::rename(&temporary, path).map_err(|source| WorkbenchDocumentError::Write {
            path: path.to_owned(),
            source,
        })
    }

    pub fn load(path: &Path) -> Result<Option<Self>, WorkbenchDocumentError> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(WorkbenchDocumentError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        Self::decode(&raw).map(Some)
    }

    pub fn encode(&self) -> String {
        let mut output = format!("{HEADER}\n");
        output.push_str(&format!(
            "dock {} {} {} {} {}\nactive {}\n",
            u8::from(self.project_sidebar_visible),
            self.project_sidebar_width.to_bits(),
            u8::from(self.metric_sidebar_compact),
            u8::from(self.bottom_inspector_visible),
            self.bottom_inspector_height.to_bits(),
            self.active_view
        ));
        for source in &self.sources {
            output.push_str(&format!(
                "source {}\n",
                encode(source.to_string_lossy().as_ref())
            ));
        }
        for project in &self.pinned_projects {
            encode_project_record(&mut output, "pinned-project", project);
        }
        for project in &self.archived_projects {
            encode_project_record(&mut output, "archived-project", project);
        }
        for run in &self.archived_runs {
            encode_run_record(&mut output, "archived-run", run);
        }
        for view in &self.views {
            let viewport = view.viewport.map_or_else(
                || "- -".to_owned(),
                |range| format!("{} {}", range.start().to_bits(), range.end().to_bits()),
            );
            output.push_str(&format!(
                "view {} {} {} {} {} {} {viewport}\n",
                encode(&view.name),
                axis_name(view.axis),
                density_name(view.track_density),
                tab_name(view.inspector_tab),
                direction_name(view.ranking_direction),
                view.selected_metric
                    .as_deref()
                    .map_or_else(|| "-".to_owned(), encode),
            ));
            for run in &view.runs {
                encode_run_record(&mut output, "run", run);
            }
            if let Some(baseline) = &view.baseline {
                encode_run_record(&mut output, "baseline-run", baseline);
            }
            for run in &view.pinned_runs {
                encode_run_record(&mut output, "pinned-run", run);
            }
            for metric in &view.metrics {
                output.push_str(&format!("metric {}\n", encode(metric)));
            }
            output.push_str("end\n");
        }
        output
    }

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
            archived_runs: Vec::new(),
            views: Vec::new(),
            active_view: 0,
            project_sidebar_visible: true,
            project_sidebar_width: 320.,
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
                [
                    "view",
                    name,
                    axis,
                    density,
                    tab,
                    direction,
                    selected,
                    start,
                    end,
                ] => {
                    if let Some(view) = current.take() {
                        document.views.push(view);
                    }
                    current = Some(SavedAnalysisView {
                        name: decode(name, line_number)?,
                        runs: Vec::new(),
                        baseline: None,
                        pinned_runs: Vec::new(),
                        metrics: Vec::new(),
                        selected_metric: (*selected != "-")
                            .then(|| decode(selected, line_number))
                            .transpose()?,
                        inspector_tab: parse_tab(tab, line_number)?,
                        ranking_direction: parse_direction(direction, line_number)?,
                        axis: parse_axis(axis, line_number)?,
                        track_density: parse_density(density, line_number)?,
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
                ["metric", metric] => current
                    .as_mut()
                    .ok_or_else(|| invalid("metric appears outside a view"))?
                    .metrics
                    .push(decode(metric, line_number)?),
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

fn encode_project_record(output: &mut String, kind: &str, project: &SavedProjectRef) {
    output.push_str(&format!(
        "{kind} {} {}\n",
        encode(project.source_path.to_string_lossy().as_ref()),
        encode(project.project_id.as_str()),
    ));
}

fn encode_run_record(output: &mut String, kind: &str, run: &SavedRunRef) {
    output.push_str(&format!(
        "{kind} {} {} {}\n",
        encode(run.source_path.to_string_lossy().as_ref()),
        encode(run.project_id.as_str()),
        encode(run.run_id.as_str()),
    ));
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

fn encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

macro_rules! named_enum {
    ($name:ident, $parse_name:ident, $type:ty, {$($text:literal => $value:path),+ $(,)?}) => {
        fn $name(value: $type) -> &'static str { match value { $($value => $text),+ } }
        fn $parse_name(value: &str, line: usize) -> Result<$type, WorkbenchDocumentError> {
            match value { $($text => Ok($value)),+, _ => Err(invalid_value(line, "invalid enum value")) }
        }
    };
}

named_enum!(axis_name, parse_axis, AlignmentAxis, {"step" => AlignmentAxis::Step, "elapsed" => AlignmentAxis::ElapsedTime});
named_enum!(density_name, parse_density, TrackDensity, {"compact" => TrackDensity::Compact, "comfortable" => TrackDensity::Comfortable, "spacious" => TrackDensity::Spacious});
named_enum!(tab_name, parse_tab, InspectorTab, {"summary" => InspectorTab::Summary, "ranking" => InspectorTab::Ranking, "evidence" => InspectorTab::Evidence});

fn direction_name(direction: Option<ObjectiveDirection>) -> &'static str {
    match direction {
        None => "none",
        Some(ObjectiveDirection::Minimize) => "minimize",
        Some(ObjectiveDirection::Maximize) => "maximize",
    }
}

fn parse_direction(
    value: &str,
    line: usize,
) -> Result<Option<ObjectiveDirection>, WorkbenchDocumentError> {
    match value {
        "none" => Ok(None),
        "minimize" => Ok(Some(ObjectiveDirection::Minimize)),
        "maximize" => Ok(Some(ObjectiveDirection::Maximize)),
        _ => Err(invalid_value(line, "invalid ranking direction")),
    }
}

fn invalid_value(line: usize, message: &str) -> WorkbenchDocumentError {
    WorkbenchDocumentError::Invalid {
        line,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> WorkbenchDocument {
        let source_path = PathBuf::from("/tmp/project with spaces");
        let project_id = ProjectId::from_string("project");
        let run = |run_id: &str| SavedRunRef {
            source_path: source_path.clone(),
            project_id: project_id.clone(),
            run_id: RunId::from_string(run_id),
        };
        WorkbenchDocument {
            sources: vec![source_path.clone()],
            pinned_projects: vec![SavedProjectRef {
                source_path: source_path.clone(),
                project_id: project_id.clone(),
            }],
            archived_projects: vec![SavedProjectRef {
                source_path: PathBuf::from("/tmp/archive"),
                project_id: ProjectId::from_string("archived-project"),
            }],
            archived_runs: vec![run("archived-run")],
            views: vec![SavedAnalysisView {
                name: "Loss / Accuracy".to_owned(),
                runs: vec![run("run")],
                baseline: Some(run("baseline")),
                pinned_runs: vec![run("pinned")],
                metrics: vec!["loss".to_owned()],
                selected_metric: Some("loss".to_owned()),
                inspector_tab: InspectorTab::Evidence,
                ranking_direction: Some(ObjectiveDirection::Minimize),
                axis: AlignmentAxis::ElapsedTime,
                track_density: TrackDensity::Compact,
                viewport: Some(AxisRange::new(10., 20.).expect("test viewport should be valid")),
            }],
            active_view: 0,
            project_sidebar_visible: false,
            project_sidebar_width: 280.,
            metric_sidebar_compact: true,
            bottom_inspector_visible: true,
            bottom_inspector_height: 240.,
        }
    }

    #[test]
    fn versioned_document_round_trips_all_viewer_owned_state() {
        let document = document();
        assert_eq!(
            WorkbenchDocument::decode(&document.encode()).ok(),
            Some(document)
        );
    }

    #[test]
    fn atomic_save_replaces_the_previous_document() {
        let root = tempfile::tempdir().expect("test directory should be created");
        let path = root.path().join("workbench.state");
        let document = document();
        document.save(&path).expect("document should save");
        assert_eq!(
            WorkbenchDocument::load(&path).ok().flatten(),
            Some(document)
        );
    }

    #[test]
    fn unsupported_versions_are_explicit() {
        let error = WorkbenchDocument::decode("pulseon-workbench 99\n").unwrap_err();
        assert!(matches!(
            error,
            WorkbenchDocumentError::UnsupportedVersion(_)
        ));
    }

    #[test]
    fn original_v1_records_default_new_organization_state() {
        let raw = "pulseon-workbench 1\n\
                   dock 1 1134559232 0 0 1130102784\n\
                   active 0\n\
                   view 566965772031 step comfortable summary none - - -\n\
                   end\n";

        let document = WorkbenchDocument::decode(raw).expect("original v1 document should load");

        assert!(document.pinned_projects.is_empty());
        assert!(document.archived_projects.is_empty());
        assert!(document.archived_runs.is_empty());
        assert!(document.views[0].baseline.is_none());
        assert!(document.views[0].pinned_runs.is_empty());
    }
}
