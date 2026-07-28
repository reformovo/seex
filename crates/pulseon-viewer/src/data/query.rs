use std::collections::HashMap;

use crate::data::source::ReadSession;
use crate::domain::{DataSourceId, RunRef};
use pulseon_chart_core::{DataPoint, Series, SeriesId};
use pulseon_core::engine::EngineError;
use pulseon_core::engine::query::NativeQueryStore;
use pulseon_model::alignment::{
    AlignedMetricResult, AlignmentAxis, AlignmentQuery, AlignmentQueryError, AlignmentReduction,
    AlignmentViewport,
};
use pulseon_model::comparison::{
    EvidenceCompleteness, ObjectiveDirection, ObjectiveEvidence, ObjectiveMetric,
};
use pulseon_model::metric::{MetricAggregate, MetricKey};
use pulseon_model::run::Run;
use pulseon_storage::StorageError;

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
    pub physical_width: u32,
}

/// Closed-viewport detail query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetailRequest {
    pub selection: CurveSelection,
    pub viewport: AlignmentViewport,
    pub physical_width: u32,
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
    pub evidence: AlignedMetricResult,
    pub chart_series: Option<Series>,
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

/// Failures while planning or executing a viewer curve query.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error(transparent)]
    Alignment(#[from] AlignmentQueryError),
    #[error(transparent)]
    Chart(#[from] pulseon_chart_core::ChartError),
    #[error(transparent)]
    Core(#[from] EngineError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl ReadSession {
    /// Queries the full non-negative axis at one point per physical pixel.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError`] when the request cannot be planned or executed.
    pub fn query_overview(&self, request: &OverviewRequest) -> Result<CurveSnapshot, QueryError> {
        let viewport = match request.selection.axis {
            CurveAxis::Step => AlignmentViewport::new(0, i64::MAX)?,
            CurveAxis::AbsoluteTime => AlignmentViewport::new(i64::MIN, i64::MAX)?,
        };
        query_curves(
            self,
            &request.selection,
            viewport,
            overview_budget(request.physical_width),
        )
    }

    /// Queries a closed detail viewport at two points per physical pixel.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError`] when the request cannot be planned or executed.
    pub fn query_detail(&self, request: &DetailRequest) -> Result<CurveSnapshot, QueryError> {
        query_curves(
            self,
            &request.selection,
            request.viewport,
            detail_budget(request.physical_width),
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

fn overview_budget(physical_width: u32) -> u32 {
    physical_width.clamp(500, 2_000)
}

fn detail_budget(physical_width: u32) -> u32 {
    physical_width.saturating_mul(2).clamp(2_000, 10_000)
}

fn query_curves(
    session: &ReadSession,
    selection: &CurveSelection,
    viewport: AlignmentViewport,
    point_budget: u32,
) -> Result<CurveSnapshot, QueryError> {
    let run_ids = selection
        .runs
        .iter()
        .map(|run| run.run_id.clone())
        .collect::<Vec<_>>();
    let runs = session.connection().get_runs(&run_ids)?;
    let store = NativeQueryStore::new(session.connection());
    let reduction = AlignmentReduction::screen_budget(point_budget, 1)?;
    let mut real_bounds: Option<(i64, i64)> = None;
    let mut series = Vec::with_capacity(runs.len());
    for run in runs {
        let Some(run_ref) = selection.runs.iter().find(|selected| {
            selected.source_id == selection.source_id
                && selected.project_id == run.project_id
                && selected.run_id == run.run_id
        }) else {
            continue;
        };
        let storage_axis = match selection.axis {
            CurveAxis::Step => AlignmentAxis::Step,
            CurveAxis::AbsoluteTime => AlignmentAxis::ElapsedTime,
        };
        let storage_viewport = match selection.axis {
            CurveAxis::Step => viewport,
            CurveAxis::AbsoluteTime => {
                let started_at = run.started_at.timestamp_millis();
                AlignmentViewport::new(
                    viewport.start().saturating_sub(started_at).max(0),
                    viewport.end().saturating_sub(started_at).max(0),
                )?
            }
        };
        let mut evidence = store.query_aligned_metric(
            &AlignmentQuery {
                run_id: run.run_id.clone(),
                metric_key: selection.metric_key.clone(),
                axis: storage_axis,
                viewport: storage_viewport,
                reduction,
            },
            run.status,
        )?;
        if selection.axis == CurveAxis::AbsoluteTime {
            for point in &mut evidence.points {
                point.axis_value = point.point.timestamp.timestamp_millis();
            }
        }
        let drawable = matches!(
            evidence.completeness,
            EvidenceCompleteness::Complete | EvidenceCompleteness::Partial
        );
        let chart_series = drawable
            .then(|| {
                Series::new(
                    SeriesId::new(run_ref.cache_key())?,
                    evidence
                        .points
                        .iter()
                        .map(|point| DataPoint::new(point.axis_value as f64, point.point.value_f64))
                        .collect(),
                )
            })
            .transpose()?;
        if drawable {
            for axis_value in evidence
                .points
                .iter()
                .map(|point| point.axis_value)
                .filter(|value| *value >= viewport.start() && *value <= viewport.end())
            {
                real_bounds = Some(match real_bounds {
                    Some((start, end)) => (start.min(axis_value), end.max(axis_value)),
                    None => (axis_value, axis_value),
                });
            }
        }
        series.push(CurveSeriesSnapshot {
            run_ref: run_ref.clone(),
            run,
            evidence,
            chart_series,
        });
    }
    let real_range = real_bounds.map(brushable_range).transpose()?;
    Ok(CurveSnapshot {
        viewport,
        point_budget,
        real_range,
        series,
    })
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
    use pulseon_chart_core::{AxisRange, BrushState};
    use pulseon_core::engine::client::NativeClient;
    use pulseon_model::run::{RunId, RunStatus};
    use pulseon_model::types::ProjectId;

    use super::{
        AlignmentViewport, CurveAxis, CurveSelection, DataSourceId, DetailRequest, MetricKey,
        OverviewRequest, ReadSession, RunRef, brushable_range, detail_budget, overview_budget,
    };

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
            [500, 900, 2_000, 2_000, 5_000, 10_000]
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
        let source_id = DataSourceId::from_path(root.path());
        let selection = CurveSelection {
            source_id: source_id.clone(),
            runs: vec![RunRef::new(source_id, project.project_id, run.run_id)],
            metric_key: MetricKey::from_string("loss"),
            axis: CurveAxis::AbsoluteTime,
        };

        let overview = session.query_overview(&OverviewRequest {
            selection: selection.clone(),
            physical_width: 500,
        })?;
        let evidence = &overview.series[0].evidence;

        assert_eq!(overview.series[0].run.status, RunStatus::Finished);
        assert!(evidence.points.iter().all(|point| {
            point.axis_value == point.point.timestamp.timestamp_millis()
                && point.axis_value > 1_000_000_000_000
        }));
        let first = evidence
            .points
            .first()
            .expect("fixture should have points")
            .axis_value;
        let last = evidence
            .points
            .last()
            .expect("fixture should have points")
            .axis_value;
        let detail = session.query_detail(&DetailRequest {
            selection,
            viewport: AlignmentViewport::new(first, last)?,
            physical_width: 500,
        })?;
        assert!(
            detail.series[0]
                .evidence
                .points
                .iter()
                .all(|point| point.axis_value >= first && point.axis_value <= last)
        );
        Ok(())
    }
}
