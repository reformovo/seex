use std::collections::{HashMap, HashSet};
use std::fmt;

use seex_model::alignment::AlignmentViewport;
use seex_model::metric::MetricKey;

use crate::data::query::{
    CurveAxis, CurveSelection, CurveSeriesSnapshot, CurveSnapshot, DetailRequest, InspectorRequest,
    InspectorRunSnapshot, InspectorSnapshot, OverviewRequest,
};
use crate::data::worker::{Generation, ReadEvent, ReadKind, ReadRequest, ReadSnapshot};
use crate::domain::{DataSourceId, RunRef};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AnalysisViewId(String);

impl AnalysisViewId {
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
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

    pub fn as_str(&self) -> &str {
        &self.0
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
    pub mode: PanelReadMode,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PanelReadMode {
    #[default]
    Replace,
    Merge,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PanelReadRequest {
    Overview {
        runs: Vec<RunRef>,
        metric_key: MetricKey,
        axis: CurveAxis,
        logical_width: u32,
    },
    Detail {
        runs: Vec<RunRef>,
        metric_key: MetricKey,
        axis: CurveAxis,
        viewport: AlignmentViewport,
        logical_width: u32,
    },
    Inspector {
        runs: Vec<RunRef>,
        metric_key: MetricKey,
    },
}

impl PanelReadRequest {
    const fn kind(&self) -> ReadKind {
        match self {
            Self::Overview { .. } => ReadKind::Overview,
            Self::Detail { .. } => ReadKind::Detail,
            Self::Inspector { .. } => ReadKind::Inspector,
        }
    }

    fn runs(&self) -> &[RunRef] {
        match self {
            Self::Overview { runs, .. }
            | Self::Detail { runs, .. }
            | Self::Inspector { runs, .. } => runs,
        }
    }

    fn for_source(&self, source_id: DataSourceId, runs: Vec<RunRef>) -> ReadRequest {
        match self {
            Self::Overview {
                metric_key,
                axis,
                logical_width,
                ..
            } => ReadRequest::Overview(OverviewRequest {
                selection: CurveSelection {
                    source_id,
                    runs,
                    metric_key: metric_key.clone(),
                    axis: *axis,
                },
                logical_width: *logical_width,
            }),
            Self::Detail {
                metric_key,
                axis,
                viewport,
                logical_width,
                ..
            } => ReadRequest::Detail(DetailRequest {
                selection: CurveSelection {
                    source_id,
                    runs,
                    metric_key: metric_key.clone(),
                    axis: *axis,
                },
                viewport: *viewport,
                logical_width: *logical_width,
            }),
            Self::Inspector { metric_key, .. } => ReadRequest::Inspector(InspectorRequest {
                source_id,
                runs,
                metric_key: metric_key.clone(),
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
    pub inspector: Option<InspectorSnapshot>,
    pub source_errors: Vec<SourceReadFailure>,
}

#[cfg(feature = "test-support")]
impl PanelReadSnapshot {
    pub fn resource_snapshot(
        &self,
        read_kind: ReadKind,
    ) -> Option<crate::performance::PanelResourceSnapshot> {
        Some(crate::performance::PanelResourceSnapshot {
            view_id: self.tag.view_id.as_str().to_owned(),
            panel_id: self.tag.panel_id.as_str().to_owned(),
            generation: self.tag.generation.0,
            read_kind: format!("{read_kind:?}").to_lowercase(),
            curves: self.curves.as_ref()?.resource_snapshot(),
            concurrent_reads: 0,
            peak_concurrent_reads: 0,
            superseded_reads: 0,
            stale_reads: 0,
            stale_retained_snapshots: 0,
        })
    }
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
    responses: HashMap<DataSourceId, Result<SourcePanelSnapshot, String>>,
}

enum SourcePanelSnapshot {
    Curves(CurveSnapshot),
    Inspector(InspectorSnapshot),
}

#[derive(Default)]
pub struct PanelReadCoordinator {
    pending: HashMap<PanelReadKey, PendingPanelRead>,
    inflight: HashMap<(Generation, ReadKind), PanelReadKey>,
    #[cfg(feature = "test-support")]
    stale_reads: u64,
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
            #[cfg(feature = "test-support")]
            {
                self.stale_reads += 1;
            }
            return PanelReadOutcome::IgnoredStale;
        };
        let Some(pending) = self.pending.get_mut(&key) else {
            #[cfg(feature = "test-support")]
            {
                self.stale_reads += 1;
            }
            return PanelReadOutcome::IgnoredStale;
        };
        if !pending.expected.contains(&event.source_id)
            || pending.responses.contains_key(&event.source_id)
        {
            #[cfg(feature = "test-support")]
            {
                self.stale_reads += 1;
            }
            return PanelReadOutcome::IgnoredStale;
        }
        let response = match event.result {
            Ok(ReadSnapshot::Overview(snapshot) | ReadSnapshot::Detail(snapshot)) => {
                Ok(SourcePanelSnapshot::Curves(snapshot))
            }
            Ok(ReadSnapshot::Inspector(snapshot)) => Ok(SourcePanelSnapshot::Inspector(snapshot)),
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

    #[cfg(feature = "test-support")]
    pub fn resource_snapshot(&self) -> crate::performance::ReadSchedulingSnapshot {
        crate::performance::ReadSchedulingSnapshot {
            stale_reads: self.stale_reads,
            stale_retained_snapshots: 0,
            ..crate::performance::ReadSchedulingSnapshot::default()
        }
    }
}

fn merge_panel_read(mut pending: PendingPanelRead) -> PanelReadSnapshot {
    let mut series = HashMap::<RunRef, CurveSeriesSnapshot>::new();
    let mut inspector_runs = HashMap::<RunRef, InspectorRunSnapshot>::new();
    let mut shape = None;
    let mut has_inspector = false;
    let mut real_range: Option<AlignmentViewport> = None;
    let mut source_errors = Vec::new();
    for source_id in &pending.source_order {
        match pending.responses.remove(source_id) {
            Some(Ok(SourcePanelSnapshot::Curves(snapshot))) => {
                shape.get_or_insert((snapshot.viewport, snapshot.point_budget));
                real_range = union_range(real_range, snapshot.real_range);
                series.extend(
                    snapshot
                        .series
                        .into_iter()
                        .map(|curve| (curve.run_ref.clone(), curve)),
                );
            }
            Some(Ok(SourcePanelSnapshot::Inspector(snapshot))) => {
                has_inspector = true;
                inspector_runs.extend(
                    snapshot
                        .runs
                        .into_iter()
                        .map(|run| (run.run_ref.clone(), run)),
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
    let inspector = has_inspector.then(|| InspectorSnapshot {
        runs: pending
            .run_order
            .iter()
            .filter_map(|run_ref| inspector_runs.remove(run_ref))
            .collect(),
    });
    PanelReadSnapshot {
        tag: pending.tag,
        curves,
        inspector,
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
    use seex_chart_core::{DataPoint, Series, SeriesId};
    use seex_model::comparison::EvidenceCompleteness;
    use seex_model::run::{Run, RunId, RunStatus};
    use seex_model::types::ProjectId;

    use crate::SourceError;
    use crate::data::worker::WorkerError;

    use super::{
        AlignmentViewport, AnalysisViewId, CurveAxis, CurveSeriesSnapshot, CurveSnapshot,
        DataSourceId, Generation, MetricKey, MetricPanelId, PanelReadCoordinator, PanelReadMode,
        PanelReadOutcome, PanelReadRequest, PanelReadTag, ReadEvent, ReadKind, ReadRequest,
        ReadSnapshot, RunRef,
    };

    fn run_ref(source: &str, run: &str) -> RunRef {
        RunRef::new(
            DataSourceId::new(source).expect("test alias should be valid"),
            ProjectId::from_string("project"),
            RunId::from_string(run),
        )
    }

    fn tag(generation: u64) -> PanelReadTag {
        PanelReadTag {
            view_id: AnalysisViewId::from_string("view"),
            panel_id: MetricPanelId::from_string("panel"),
            generation: Generation(generation),
            mode: PanelReadMode::Replace,
        }
    }

    fn request(runs: Vec<RunRef>) -> PanelReadRequest {
        PanelReadRequest::Detail {
            runs,
            metric_key: MetricKey::from_string("loss"),
            axis: CurveAxis::Step,
            viewport: AlignmentViewport::new(0, 10).expect("test viewport should be valid"),
            logical_width: 1_000,
        }
    }

    fn snapshot(run_ref: RunRef) -> CurveSnapshot {
        let timestamp = "2026-01-01T00:00:00Z"
            .parse()
            .expect("fixed timestamp should parse");
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
                completeness: EvidenceCompleteness::Complete,
                reasons: Vec::new(),
                source_row_count: 1,
                returned_point_count: 1,
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
        #[cfg(feature = "test-support")]
        assert_eq!(
            coordinator.resource_snapshot(),
            crate::performance::ReadSchedulingSnapshot {
                stale_reads: 2,
                stale_retained_snapshots: 0,
                ..crate::performance::ReadSchedulingSnapshot::default()
            }
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn resource_snapshot_keeps_panel_generation_identity() {
        let run = run_ref("source", "run");
        let mut coordinator = PanelReadCoordinator::default();
        coordinator
            .begin(tag(7), request(vec![run.clone()]))
            .expect("read should begin");
        let PanelReadOutcome::Completed(completed) =
            coordinator.apply(detail_event(run.source_id.clone(), 7, Ok(snapshot(run))))
        else {
            panic!("single-source read should complete");
        };

        let resources = completed
            .resource_snapshot(ReadKind::Detail)
            .expect("curve result should expose resources");
        assert_eq!(
            (
                resources.view_id.as_str(),
                resources.panel_id.as_str(),
                resources.generation
            ),
            ("view", "panel", 7)
        );
        assert_eq!(resources.read_kind, "detail");
    }
}
