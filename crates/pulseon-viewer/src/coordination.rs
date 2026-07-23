use std::collections::{HashMap, HashSet};
use std::fmt;

use pulseon_model::alignment::{AlignmentAxis, AlignmentViewport};
use pulseon_model::metric::MetricKey;

use crate::core::{DataSourceId, RunRef};
use crate::query::{
    CurveSelection, CurveSeriesSnapshot, CurveSnapshot, DetailRequest, OverviewRequest,
};
use crate::worker::{Generation, ReadEvent, ReadKind, ReadRequest, ReadSnapshot};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AnalysisViewId(String);

impl AnalysisViewId {
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for AnalysisViewId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MetricPanelId(String);

impl MetricPanelId {
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for MetricPanelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanelReadTag {
    pub view_id: AnalysisViewId,
    pub panel_id: MetricPanelId,
    pub generation: Generation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PanelReadRequest {
    Overview {
        runs: Vec<RunRef>,
        metric_key: MetricKey,
        axis: AlignmentAxis,
        physical_width: u32,
    },
    Detail {
        runs: Vec<RunRef>,
        metric_key: MetricKey,
        axis: AlignmentAxis,
        viewport: AlignmentViewport,
        physical_width: u32,
    },
}

impl PanelReadRequest {
    const fn kind(&self) -> ReadKind {
        match self {
            Self::Overview { .. } => ReadKind::Overview,
            Self::Detail { .. } => ReadKind::Detail,
        }
    }

    fn runs(&self) -> &[RunRef] {
        match self {
            Self::Overview { runs, .. } | Self::Detail { runs, .. } => runs,
        }
    }

    fn for_source(&self, source_id: DataSourceId, runs: Vec<RunRef>) -> ReadRequest {
        match self {
            Self::Overview {
                metric_key,
                axis,
                physical_width,
                ..
            } => ReadRequest::Overview(OverviewRequest {
                selection: CurveSelection {
                    source_id,
                    runs,
                    metric_key: metric_key.clone(),
                    axis: *axis,
                },
                physical_width: *physical_width,
            }),
            Self::Detail {
                metric_key,
                axis,
                viewport,
                physical_width,
                ..
            } => ReadRequest::Detail(DetailRequest {
                selection: CurveSelection {
                    source_id,
                    runs,
                    metric_key: metric_key.clone(),
                    axis: *axis,
                },
                viewport: *viewport,
                physical_width: *physical_width,
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedSourceRead {
    pub tag: PanelReadTag,
    pub source_id: DataSourceId,
    pub generation: Generation,
    pub request: ReadRequest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReadFailure {
    pub source_id: DataSourceId,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelReadSnapshot {
    pub tag: PanelReadTag,
    pub curves: Option<CurveSnapshot>,
    pub source_errors: Vec<SourceReadFailure>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PanelReadOutcome {
    AcceptedPending,
    Completed(PanelReadSnapshot),
    IgnoredStale,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PanelReadKey {
    view_id: AnalysisViewId,
    panel_id: MetricPanelId,
    kind: ReadKind,
}

struct PendingPanelRead {
    tag: PanelReadTag,
    run_order: Vec<RunRef>,
    source_order: Vec<DataSourceId>,
    expected: HashSet<DataSourceId>,
    responses: HashMap<DataSourceId, Result<CurveSnapshot, String>>,
}

#[derive(Default)]
pub struct PanelReadCoordinator {
    pending: HashMap<PanelReadKey, PendingPanelRead>,
    inflight: HashMap<(Generation, ReadKind), PanelReadKey>,
}

impl PanelReadCoordinator {
    /// Replaces pending work for the same View, panel, and request kind.
    ///
    /// # Errors
    ///
    /// Returns [`PanelCoordinationError`] for an empty selection or a reused
    /// in-flight generation.
    pub fn begin(
        &mut self,
        tag: PanelReadTag,
        request: PanelReadRequest,
    ) -> Result<Vec<PlannedSourceRead>, PanelCoordinationError> {
        if request.runs().is_empty() {
            return Err(PanelCoordinationError::EmptySelection);
        }
        let kind = request.kind();
        if self.inflight.contains_key(&(tag.generation, kind)) {
            return Err(PanelCoordinationError::DuplicateGeneration(tag.generation));
        }
        let key = PanelReadKey {
            view_id: tag.view_id.clone(),
            panel_id: tag.panel_id.clone(),
            kind,
        };
        if let Some(previous) = self.pending.remove(&key) {
            self.inflight.remove(&(previous.tag.generation, kind));
        }
        let mut groups: Vec<(DataSourceId, Vec<RunRef>)> = Vec::new();
        for run in request.runs() {
            if let Some((_, runs)) = groups
                .iter_mut()
                .find(|(source_id, _)| source_id == &run.source_id)
            {
                runs.push(run.clone());
            } else {
                groups.push((run.source_id.clone(), vec![run.clone()]));
            }
        }
        let planned = groups
            .iter()
            .map(|(source_id, runs)| PlannedSourceRead {
                tag: tag.clone(),
                source_id: source_id.clone(),
                generation: tag.generation,
                request: request.for_source(source_id.clone(), runs.clone()),
            })
            .collect::<Vec<_>>();
        let source_order = groups
            .into_iter()
            .map(|(source_id, _)| source_id)
            .collect::<Vec<_>>();
        self.inflight.insert((tag.generation, kind), key.clone());
        self.pending.insert(
            key,
            PendingPanelRead {
                tag,
                run_order: request.runs().to_vec(),
                expected: source_order.iter().cloned().collect(),
                source_order,
                responses: HashMap::new(),
            },
        );
        Ok(planned)
    }

    pub fn apply(&mut self, event: ReadEvent) -> PanelReadOutcome {
        let lookup = (event.generation, event.kind);
        let Some(key) = self.inflight.get(&lookup).cloned() else {
            return PanelReadOutcome::IgnoredStale;
        };
        let Some(pending) = self.pending.get_mut(&key) else {
            return PanelReadOutcome::IgnoredStale;
        };
        if !pending.expected.contains(&event.source_id)
            || pending.responses.contains_key(&event.source_id)
        {
            return PanelReadOutcome::IgnoredStale;
        }
        let response = match event.result {
            Ok(ReadSnapshot::Overview(snapshot) | ReadSnapshot::Detail(snapshot)) => Ok(snapshot),
            Ok(ReadSnapshot::Catalog(_)) => {
                Err("panel read returned a catalog snapshot".to_owned())
            }
            Err(error) => Err(error.to_string()),
        };
        pending.responses.insert(event.source_id, response);
        if pending.responses.len() != pending.expected.len() {
            return PanelReadOutcome::AcceptedPending;
        }
        self.inflight.remove(&lookup);
        let pending = self
            .pending
            .remove(&key)
            .expect("completed panel read must remain registered");
        PanelReadOutcome::Completed(merge_panel_read(pending))
    }

    pub fn deactivate_view(&mut self, view_id: &AnalysisViewId) {
        let mut removed = Vec::new();
        self.pending.retain(|key, pending| {
            if &key.view_id == view_id {
                removed.push((pending.tag.generation, key.kind));
                false
            } else {
                true
            }
        });
        for (generation, kind) in removed {
            self.inflight.remove(&(generation, kind));
        }
    }
}

fn merge_panel_read(mut pending: PendingPanelRead) -> PanelReadSnapshot {
    let mut series = HashMap::<RunRef, CurveSeriesSnapshot>::new();
    let mut shape = None;
    let mut real_range: Option<AlignmentViewport> = None;
    let mut source_errors = Vec::new();
    for source_id in &pending.source_order {
        match pending.responses.remove(source_id) {
            Some(Ok(snapshot)) => {
                shape.get_or_insert((snapshot.viewport, snapshot.point_budget));
                real_range = union_range(real_range, snapshot.real_range);
                series.extend(
                    snapshot
                        .series
                        .into_iter()
                        .map(|curve| (curve.run_ref.clone(), curve)),
                );
            }
            Some(Err(message)) => source_errors.push(SourceReadFailure {
                source_id: source_id.clone(),
                message,
            }),
            None => {}
        }
    }
    let curves = shape.map(|(viewport, point_budget)| CurveSnapshot {
        viewport,
        point_budget,
        real_range,
        series: pending
            .run_order
            .iter()
            .filter_map(|run_ref| series.remove(run_ref))
            .collect(),
    });
    PanelReadSnapshot {
        tag: pending.tag,
        curves,
        source_errors,
    }
}

fn union_range(
    current: Option<AlignmentViewport>,
    next: Option<AlignmentViewport>,
) -> Option<AlignmentViewport> {
    match (current, next) {
        (Some(current), Some(next)) => AlignmentViewport::new(
            current.start().min(next.start()),
            current.end().max(next.end()),
        )
        .ok(),
        (current @ Some(_), None) => current,
        (None, next) => next,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PanelCoordinationError {
    #[error("panel read selection is empty")]
    EmptySelection,
    #[error("panel read generation {0:?} is already in flight")]
    DuplicateGeneration(Generation),
}

#[cfg(test)]
mod tests {
    use pulseon_chart_core::{DataPoint, Series, SeriesId};
    use pulseon_model::alignment::{AlignedMetricPoint, AlignedMetricResult};
    use pulseon_model::comparison::EvidenceCompleteness;
    use pulseon_model::metric::{MetricPoint, Step};
    use pulseon_model::run::{Run, RunId, RunStatus};
    use pulseon_model::types::ProjectId;

    use crate::SourceError;
    use crate::worker::WorkerError;

    use super::*;

    fn run_ref(source: &str, run: &str) -> RunRef {
        RunRef::new(
            DataSourceId::from_string(source),
            ProjectId::from_string("project"),
            RunId::from_string(run),
        )
    }

    fn tag(generation: u64) -> PanelReadTag {
        PanelReadTag {
            view_id: AnalysisViewId::from_string("view"),
            panel_id: MetricPanelId::from_string("panel"),
            generation: Generation(generation),
        }
    }

    fn request(runs: Vec<RunRef>) -> PanelReadRequest {
        PanelReadRequest::Detail {
            runs,
            metric_key: MetricKey::from_string("loss"),
            axis: AlignmentAxis::Step,
            viewport: AlignmentViewport::new(0, 10).expect("test viewport should be valid"),
            physical_width: 1_000,
        }
    }

    fn snapshot(run_ref: RunRef) -> CurveSnapshot {
        let timestamp = "2026-01-01T00:00:00Z"
            .parse()
            .expect("fixed timestamp should parse");
        let metric_key = MetricKey::from_string("loss");
        let point = AlignedMetricPoint {
            point: MetricPoint {
                run_id: run_ref.run_id.clone(),
                metric_key: metric_key.clone(),
                step: Step::new(1),
                timestamp,
                value_f64: 0.5,
                ingested_at: timestamp,
            },
            axis_value: 1,
        };
        let chart_series = Series::new(
            SeriesId::new(run_ref.cache_key()).expect("Run reference should make a series id"),
            vec![DataPoint::new(1., 0.5)],
        )
        .expect("test series should be valid");
        CurveSnapshot {
            viewport: AlignmentViewport::new(0, 10).expect("test viewport should be valid"),
            point_budget: 2_000,
            real_range: Some(
                AlignmentViewport::new(1, 2).expect("test real range should be valid"),
            ),
            series: vec![CurveSeriesSnapshot {
                run: Run {
                    run_id: run_ref.run_id.clone(),
                    project_id: run_ref.project_id.clone(),
                    name: run_ref.run_id.as_str().to_owned(),
                    status: RunStatus::Finished,
                    created_at: timestamp,
                    started_at: timestamp,
                    finished_at: Some(timestamp),
                },
                run_ref,
                evidence: AlignedMetricResult {
                    source_row_count: 1,
                    points: vec![point],
                    completeness: EvidenceCompleteness::Complete,
                    reasons: Vec::new(),
                },
                chart_series: Some(chart_series),
            }],
        }
    }

    fn detail_event(
        source_id: DataSourceId,
        generation: u64,
        result: Result<CurveSnapshot, WorkerError>,
    ) -> ReadEvent {
        ReadEvent {
            source_id,
            generation: Generation(generation),
            kind: ReadKind::Detail,
            result: result.map(ReadSnapshot::Detail),
        }
    }

    #[test]
    fn panel_requests_are_partitioned_by_source_with_full_run_references() {
        let first = run_ref("source-a", "run-1");
        let second = run_ref("source-b", "run-1");
        let third = run_ref("source-a", "run-2");
        let mut coordinator = PanelReadCoordinator::default();

        let planned = coordinator
            .begin(
                tag(1),
                request(vec![first.clone(), second.clone(), third.clone()]),
            )
            .expect("panel request should plan");

        assert_eq!(planned.len(), 2);
        let ReadRequest::Detail(first_source) = &planned[0].request else {
            panic!("expected a detail request");
        };
        assert_eq!(first_source.selection.runs, [first, third]);
        let ReadRequest::Detail(second_source) = &planned[1].request else {
            panic!("expected a detail request");
        };
        assert_eq!(second_source.selection.runs, [second]);
    }

    #[test]
    fn source_failures_do_not_erase_other_sources_drawable_series() {
        let drawable = run_ref("source-a", "run-1");
        let failed = run_ref("source-b", "run-1");
        let mut coordinator = PanelReadCoordinator::default();
        coordinator
            .begin(tag(1), request(vec![drawable.clone(), failed.clone()]))
            .expect("panel request should plan");

        assert_eq!(
            coordinator.apply(detail_event(
                drawable.source_id.clone(),
                1,
                Ok(snapshot(drawable.clone())),
            )),
            PanelReadOutcome::AcceptedPending
        );
        let completed = coordinator.apply(detail_event(
            failed.source_id.clone(),
            1,
            Err(WorkerError::Source(SourceError::UnsupportedS3)),
        ));
        let PanelReadOutcome::Completed(completed) = completed else {
            panic!("all source responses should complete the panel read");
        };

        assert_eq!(
            completed
                .curves
                .expect("successful source should remain drawable")
                .series[0]
                .run_ref,
            drawable
        );
        assert_eq!(completed.source_errors[0].source_id, failed.source_id);
    }

    #[test]
    fn superseded_and_inactive_view_results_are_ignored() {
        let run = run_ref("source", "run");
        let mut coordinator = PanelReadCoordinator::default();
        coordinator
            .begin(tag(1), request(vec![run.clone()]))
            .expect("first request should plan");
        coordinator
            .begin(tag(2), request(vec![run.clone()]))
            .expect("replacement request should plan");

        assert_eq!(
            coordinator.apply(detail_event(
                run.source_id.clone(),
                1,
                Ok(snapshot(run.clone())),
            )),
            PanelReadOutcome::IgnoredStale
        );
        coordinator.deactivate_view(&AnalysisViewId::from_string("view"));
        assert_eq!(
            coordinator.apply(detail_event(run.source_id.clone(), 2, Ok(snapshot(run)))),
            PanelReadOutcome::IgnoredStale
        );
    }
}
