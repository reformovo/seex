use std::collections::HashMap;

use crate::data::source::ReadSession;
use crate::domain::{DataSourceId, RunRef};
use seex::{MetricAxis, MetricCoordinate, MetricQuery, MetricQueryError, MetricRange, Timestamp};
use seex_chart_core::{DataPoint, Series, SeriesId};
use seex_core::engine::EngineError;
use seex_core::engine::query::NativeQueryStore;
use seex_model::alignment::{AlignmentQueryError, AlignmentViewport};
use seex_model::comparison::{
    EvidenceCompleteness, EvidenceReason, ObjectiveDirection, ObjectiveEvidence, ObjectiveMetric,
};
use seex_model::metric::{MetricAggregate, MetricKey};
use seex_model::run::Run;
use seex_storage::StorageError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveAxis {
    Step,
    AbsoluteTime,
}

/// Shared series selection for overview and detail queries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurveSelection {
    pub source_id: DataSourceId,
    pub runs: Vec<RunRef>,
    pub metric_key: MetricKey,
    pub axis: CurveAxis,
}

/// Full non-negative-axis overview query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OverviewRequest {
    pub selection: CurveSelection,
    pub logical_width: u32,
}

/// Closed-viewport detail query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetailRequest {
    pub selection: CurveSelection,
    pub viewport: AlignmentViewport,
    pub logical_width: u32,
}

/// Whole-series product summaries and objective evidence for the inspector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectorRequest {
    pub source_id: DataSourceId,
    pub runs: Vec<RunRef>,
    pub metric_key: MetricKey,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InspectorRunSnapshot {
    pub run_ref: RunRef,
    pub run: Run,
    pub summary: Option<MetricAggregate>,
    pub evidence: ObjectiveEvidence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InspectorSnapshot {
    pub runs: Vec<InspectorRunSnapshot>,
}

/// One Run's evidence and optional drawable chart series.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveSeriesSnapshot {
    pub run_ref: RunRef,
    pub run: Run,
    pub completeness: EvidenceCompleteness,
    pub reasons: Vec<EvidenceReason>,
    pub source_row_count: u64,
    pub returned_point_count: u64,
    pub chart_series: Option<Series>,
}

impl CurveSeriesSnapshot {
    pub fn downsampled(&self) -> bool {
        self.source_row_count > self.returned_point_count
    }
}

/// Immutable reduced curves returned for one requested viewport.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveSnapshot {
    pub viewport: AlignmentViewport,
    pub point_budget: u32,
    /// Data-derived range, expanded to one rendered axis unit for one coordinate.
    pub real_range: Option<AlignmentViewport>,
    pub series: Vec<CurveSeriesSnapshot>,
}

#[cfg(feature = "test-support")]
impl CurveSnapshot {
    /// Returns shallow point ownership without instrumenting the production path.
    pub fn resource_snapshot(&self) -> crate::performance::CurveResourceSnapshot {
        let returned_points = self
            .series
            .iter()
            .map(|curve| curve.returned_point_count)
            .sum::<u64>();
        let chart_points = self
            .series
            .iter()
            .filter_map(|curve| curve.chart_series.as_ref())
            .map(|series| series.points().len() as u64)
            .sum::<u64>();
        crate::performance::CurveResourceSnapshot {
            requested_budget: u64::from(self.point_budget),
            source_points: self.series.iter().map(|curve| curve.source_row_count).sum(),
            returned_points,
            snapshot_points: chart_points,
            snapshot_bytes: chart_points * std::mem::size_of::<DataPoint>() as u64,
            ..crate::performance::CurveResourceSnapshot::default()
        }
    }
}

/// Failures while planning or executing a viewer curve query.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error(transparent)]
    Alignment(#[from] AlignmentQueryError),
    #[error(transparent)]
    Chart(#[from] seex_chart_core::ChartError),
    #[error(transparent)]
    Core(#[from] EngineError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Sdk(#[from] seex::Error),
    #[error(transparent)]
    MetricQuery(#[from] MetricQueryError),
    #[error("Reader returned a coordinate on the wrong axis")]
    ReaderAxisMismatch,
}

impl ReadSession {
    /// Queries the full non-negative axis at one point per physical pixel.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError`] when the request cannot be planned or executed.
    pub fn query_overview(&self, request: &OverviewRequest) -> Result<CurveSnapshot, QueryError> {
        self.query_overview_until(request, &mut || false)
            .map(|snapshot| snapshot.expect("non-cancellable query should return a snapshot"))
    }

    pub(crate) fn query_overview_until(
        &self,
        request: &OverviewRequest,
        is_superseded: &mut dyn FnMut() -> bool,
    ) -> Result<Option<CurveSnapshot>, QueryError> {
        let viewport = match request.selection.axis {
            CurveAxis::Step => AlignmentViewport::new(0, i64::MAX)?,
            CurveAxis::AbsoluteTime => AlignmentViewport::new(i64::MIN, i64::MAX)?,
        };
        query_curves(
            self,
            &request.selection,
            viewport,
            overview_budget(request.logical_width),
            true,
            is_superseded,
        )
    }

    /// Queries a closed detail viewport at two points per physical pixel.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError`] when the request cannot be planned or executed.
    pub fn query_detail(&self, request: &DetailRequest) -> Result<CurveSnapshot, QueryError> {
        self.query_detail_until(request, &mut || false)
            .map(|snapshot| snapshot.expect("non-cancellable query should return a snapshot"))
    }

    pub(crate) fn query_detail_until(
        &self,
        request: &DetailRequest,
        is_superseded: &mut dyn FnMut() -> bool,
    ) -> Result<Option<CurveSnapshot>, QueryError> {
        query_curves(
            self,
            &request.selection,
            request.viewport,
            detail_budget(request.logical_width),
            false,
            is_superseded,
        )
    }

    /// Queries whole-series summaries and objective evidence for the inspector.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError`] when native storage cannot execute the query.
    pub fn query_inspector(
        &self,
        request: &InspectorRequest,
    ) -> Result<InspectorSnapshot, QueryError> {
        let run_ids = request
            .runs
            .iter()
            .map(|run| run.run_id.clone())
            .collect::<Vec<_>>();
        let runs = self.connection().get_runs(&run_ids)?;
        let store = NativeQueryStore::new(self.connection());
        let mut summaries = store
            .query_metric_summaries(&run_ids, &request.metric_key)?
            .into_iter()
            .map(|summary| (summary.run_id.clone(), summary))
            .collect::<HashMap<_, _>>();
        let objective = ObjectiveMetric {
            metric_key: request.metric_key.clone(),
            direction: ObjectiveDirection::Minimize,
        };
        let evidence = store.objective_evidence_for_runs(&runs, &objective)?;
        let snapshots = runs
            .into_iter()
            .zip(evidence)
            .filter_map(|(run, evidence)| {
                let run_ref = request.runs.iter().find(|selected| {
                    selected.source_id == request.source_id
                        && selected.project_id == run.project_id
                        && selected.run_id == run.run_id
                })?;
                let summary = summaries.remove(&run.run_id);
                Some(InspectorRunSnapshot {
                    run_ref: run_ref.clone(),
                    run,
                    summary,
                    evidence,
                })
            })
            .collect::<Vec<_>>();
        Ok(InspectorSnapshot { runs: snapshots })
    }
}

fn overview_budget(logical_width: u32) -> u32 {
    logical_width.clamp(256, 2_000)
}

fn detail_budget(logical_width: u32) -> u32 {
    logical_width.saturating_mul(2).clamp(512, 5_000)
}

fn query_curves(
    session: &ReadSession,
    selection: &CurveSelection,
    viewport: AlignmentViewport,
    point_budget: u32,
    force_full_step: bool,
    is_superseded: &mut dyn FnMut() -> bool,
) -> Result<Option<CurveSnapshot>, QueryError> {
    let run_ids = selection
        .runs
        .iter()
        .filter(|run| run.source_id == selection.source_id)
        .map(|run| run.run_id.clone())
        .collect::<Vec<_>>();
    let runs = session.reader().runs_for_desktop(&run_ids)?;
    let mut real_bounds: Option<(i64, i64)> = None;
    let mut series = Vec::with_capacity(selection.runs.len());
    for run in runs {
        if is_superseded() {
            return Ok(None);
        }
        let Some(run_ref) = selection.runs.iter().find(|selected| {
            selected.source_id == selection.source_id
                && selected.project_id == run.project_id
                && selected.run_id == run.run_id
        }) else {
            continue;
        };
        let query = MetricQuery::new(
            desktop_metric_range(selection.axis, viewport),
            Some(point_budget as usize),
        )?;
        let evidence = if force_full_step {
            session.reader().query_metric_overview_for_desktop(
                &run.run_id,
                &selection.metric_key,
                &query,
            )?
        } else {
            session
                .reader()
                .query_metric_for_desktop(&run.run_id, &selection.metric_key, &query)?
        };
        let drawable = matches!(
            evidence.completeness(),
            EvidenceCompleteness::Complete | EvidenceCompleteness::Partial
        );
        let returned_point_count = evidence.samples().len() as u64;
        let source_row_count = evidence.source_count();
        let completeness = evidence.completeness();
        let reasons = evidence.reasons().to_vec();
        let chart_points = if drawable {
            let mut points = Vec::with_capacity(evidence.samples().len());
            for sample in evidence.samples() {
                let axis_value = desktop_axis_value(selection.axis, sample.coordinate)?;
                if axis_value >= viewport.start() && axis_value <= viewport.end() {
                    real_bounds = Some(match real_bounds {
                        Some((start, end)) => (start.min(axis_value), end.max(axis_value)),
                        None => (axis_value, axis_value),
                    });
                }
                points.push(DataPoint::new(axis_value as f64, sample.point.value_f64));
            }
            Some(points)
        } else {
            None
        };
        let chart_series = chart_points
            .map(|points| Series::new(SeriesId::new(run_ref.cache_key())?, points))
            .transpose()?;
        series.push(CurveSeriesSnapshot {
            run_ref: run_ref.clone(),
            run,
            completeness,
            reasons,
            source_row_count,
            returned_point_count,
            chart_series,
        });
    }
    let real_range = real_bounds.map(brushable_range).transpose()?;
    if is_superseded() {
        return Ok(None);
    }
    Ok(Some(CurveSnapshot {
        viewport,
        point_budget,
        real_range,
        series,
    }))
}

fn desktop_metric_range(axis: CurveAxis, viewport: AlignmentViewport) -> MetricRange {
    let end = viewport.end().saturating_add(1);
    match axis {
        CurveAxis::Step => MetricRange::Steps {
            start: seex::Step::new(viewport.start()),
            end: seex::Step::new(end),
        },
        CurveAxis::AbsoluteTime if viewport.start() == i64::MIN && viewport.end() == i64::MAX => {
            MetricRange::All(MetricAxis::Timestamp)
        }
        CurveAxis::AbsoluteTime => MetricRange::Timestamps {
            start: Timestamp::from_millis(viewport.start()),
            end: Timestamp::from_millis(end),
        },
    }
}

fn desktop_axis_value(axis: CurveAxis, coordinate: MetricCoordinate) -> Result<i64, QueryError> {
    match (axis, coordinate) {
        (CurveAxis::Step, MetricCoordinate::Step(value)) => Ok(value.value()),
        (CurveAxis::AbsoluteTime, MetricCoordinate::Timestamp(value)) => Ok(value.as_millis()),
        _ => Err(QueryError::ReaderAxisMismatch),
    }
}

fn brushable_range(
    (mut start, mut end): (i64, i64),
) -> Result<AlignmentViewport, AlignmentQueryError> {
    let rendered_start = start as f64;
    if (end as f64) - rendered_start < 1.0 {
        if let Some(next) = end
            .checked_add(1)
            .filter(|next| (*next as f64) - rendered_start >= 1.0)
        {
            end = next;
        } else {
            let next = rendered_start.next_up() as i64;
            if next > end {
                end = next;
            } else {
                start = (end as f64).next_down() as i64;
            }
        }
    }
    AlignmentViewport::new(start, end)
}

#[cfg(test)]
mod tests {
    use super::{
        AlignmentViewport, CurveAxis, CurveSelection, DataSourceId, DetailRequest, MetricKey,
        OverviewRequest, ReadSession, RunRef, brushable_range, detail_budget, overview_budget,
    };
    use seex_chart_core::{AxisRange, BrushState};
    use seex_core::engine::client::NativeClient;
    use seex_model::run::{RunId, RunStatus};
    use seex_model::types::ProjectId;

    #[test]
    fn screen_budgets_clamp_density_and_overflow() {
        assert_eq!(
            [
                overview_budget(1),
                overview_budget(900),
                overview_budget(9_000),
                detail_budget(1),
                detail_budget(2_500),
                detail_budget(u32::MAX)
            ],
            [256, 900, 2_000, 512, 5_000, 5_000]
        );
    }

    #[test]
    fn data_ranges_have_a_brushable_rendered_span() {
        let ordinary = brushable_range((7, 7)).expect("ordinary range should be valid");
        let maximum = brushable_range((i64::MAX, i64::MAX))
            .expect("maximum coordinate range should be valid");
        let rounded_maximum = brushable_range((i64::MAX - 1, i64::MAX))
            .expect("rounded maximum range should be valid");

        assert_eq!((ordinary.start(), ordinary.end()), (7, 8));
        for range in [ordinary, maximum, rounded_maximum] {
            let axis = AxisRange::new(range.start() as f64, range.end() as f64)
                .expect("real range should initialize a chart range");
            BrushState::new(axis).expect("real range should initialize a brush");
        }
    }

    #[test]
    fn absolute_time_uses_observation_timestamps_without_extending_alignment_axis()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = NativeClient::open(root.path())?;
        let project = client.create_project("viewer", Some(ProjectId::from_string("project")))?;
        let run = client.create_run(
            &project.project_id,
            "candidate",
            Some(RunId::from_string("run")),
        )?;
        let handle = client.run_handle(run.clone());
        handle.log_metric_at_step("loss", 0, 1.)?;
        handle.log_metric_at_step("loss", 1, 0.5)?;
        client.finish_run(&run.run_id)?;
        client.shutdown(None)?;
        let session = ReadSession::open_existing(root.path())?;
        let source_id = DataSourceId::new("source").expect("test alias should be valid");
        let selection = CurveSelection {
            source_id: source_id.clone(),
            runs: vec![RunRef::new(source_id, project.project_id, run.run_id)],
            metric_key: MetricKey::from_string("loss"),
            axis: CurveAxis::AbsoluteTime,
        };

        let overview = session.query_overview(&OverviewRequest {
            selection: selection.clone(),
            logical_width: 500,
        })?;
        let curve = &overview.series[0];

        assert_eq!(overview.series[0].run.status, RunStatus::Finished);
        let points = curve
            .chart_series
            .as_ref()
            .expect("complete evidence should draw")
            .points();
        assert!(points.iter().all(|point| point.x > 1_000_000_000_000.));
        let first = points.first().expect("fixture should have points").x as i64;
        let last = points.last().expect("fixture should have points").x as i64;
        let detail = session.query_detail(&DetailRequest {
            selection,
            viewport: AlignmentViewport::new(first, last)?,
            logical_width: 500,
        })?;
        assert!(
            detail.series[0]
                .chart_series
                .as_ref()
                .expect("complete evidence should draw")
                .points()
                .iter()
                .all(|point| point.x >= first as f64 && point.x <= last as f64)
        );
        Ok(())
    }
}
