use std::collections::HashMap;

use seex_storage::{ProjectConnection, ProjectMetricReader};

use crate::engine::EngineError;
use crate::model::alignment::{
    AlignedMetricResult, AlignmentQuery, AlignmentQueryResult, AlignmentReason,
};
use crate::model::comparison::{
    EvidenceCompleteness, EvidenceReason, ObjectiveEvidence, ObjectiveMetric,
};
use crate::model::metric::{
    MetricAggregate, MetricKey, MetricPoint, MetricQuery, ReductionPolicy, Step,
};
use crate::model::run::{Run, RunId, RunStatus};

pub struct NativeQueryStore<'connection> {
    source: QuerySource<'connection>,
}

enum QuerySource<'connection> {
    Project(&'connection ProjectConnection),
    #[cfg(test)]
    DuckDb(&'connection duckdb::Connection),
}

pub type MetricQueryResult = seex_model::metric::MetricQueryResult;

impl<'connection> NativeQueryStore<'connection> {
    pub const fn new(connection: &'connection ProjectConnection) -> Self {
        Self {
            source: QuerySource::Project(connection),
        }
    }

    #[cfg(test)]
    pub const fn from_duckdb(connection: &'connection duckdb::Connection) -> Self {
        Self {
            source: QuerySource::DuckDb(connection),
        }
    }

    pub fn query_metric_effective(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
    ) -> Result<Vec<MetricPoint>, EngineError> {
        self.query_metric(run_id, metric_key, None, None, None)
    }

    pub fn query_metric(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        start_step: Option<Step>,
        end_step: Option<Step>,
        max_points: Option<usize>,
    ) -> Result<Vec<MetricPoint>, EngineError> {
        self.query_metric_with_metadata(run_id, metric_key, start_step, end_step, max_points)
            .map(|result| result.points)
    }

    pub fn query_metric_with_metadata(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        start_step: Option<Step>,
        end_step: Option<Step>,
        max_points: Option<usize>,
    ) -> Result<MetricQueryResult, EngineError> {
        let reduction = match max_points {
            None => ReductionPolicy::Full,
            Some(max_points) => ReductionPolicy::lttb(max_points)
                .map_err(|_| EngineError::MetricQueryMaxPointsTooSmall { max_points })?,
        };
        // Preserve the shipped Python behavior for empty or reversed ranges.
        let query = MetricQuery {
            run_id: run_id.clone(),
            metric_key: metric_key.clone(),
            start_step,
            end_step,
            reduction,
        };
        Ok(self.reader().query_metric(&query)?)
    }

    pub fn query_aligned_metric(
        &self,
        query: &AlignmentQuery,
        run_status: RunStatus,
    ) -> Result<AlignedMetricResult, EngineError> {
        let result = self.reader().query_aligned_metric(query)?;
        Ok(aligned_metric_result(result, run_status))
    }

    pub fn objective_evidence(
        &self,
        run_id: &RunId,
        run_status: RunStatus,
        objective: &ObjectiveMetric,
    ) -> Result<ObjectiveEvidence, EngineError> {
        let aggregate = self
            .query_metric_summaries(std::slice::from_ref(run_id), &objective.metric_key)?
            .into_iter()
            .next();
        Ok(objective_evidence(run_id, run_status, aggregate))
    }

    pub fn objective_evidence_for_runs(
        &self,
        runs: &[Run],
        objective: &ObjectiveMetric,
    ) -> Result<Vec<ObjectiveEvidence>, EngineError> {
        let run_ids = runs
            .iter()
            .map(|run| run.run_id.clone())
            .collect::<Vec<_>>();
        let mut aggregates = self
            .query_metric_summaries(&run_ids, &objective.metric_key)?
            .into_iter()
            .map(|aggregate| (aggregate.run_id.clone(), aggregate))
            .collect::<HashMap<_, _>>();
        Ok(runs
            .iter()
            .map(|run| objective_evidence(&run.run_id, run.status, aggregates.remove(&run.run_id)))
            .collect())
    }

    pub fn metric_aggregate(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
    ) -> Result<MetricAggregate, EngineError> {
        Ok(self.reader().metric_aggregate(run_id, metric_key)?)
    }

    pub fn query_metric_summaries(
        &self,
        run_ids: &[RunId],
        metric_key: &MetricKey,
    ) -> Result<Vec<MetricAggregate>, EngineError> {
        Ok(self.reader().query_metric_summaries(run_ids, metric_key)?)
    }

    pub fn list_metrics(
        &self,
        run_id: &RunId,
        run_status: RunStatus,
    ) -> Result<Vec<MetricAggregate>, EngineError> {
        Ok(self.reader().list_metrics(run_id, run_status)?)
    }

    fn reader(&self) -> ProjectMetricReader<'_> {
        match self.source {
            QuerySource::Project(connection) => ProjectMetricReader::new(connection),
            #[cfg(test)]
            QuerySource::DuckDb(connection) => ProjectMetricReader::new(connection),
        }
    }
}

fn aligned_metric_result(
    result: AlignmentQueryResult,
    run_status: RunStatus,
) -> AlignedMetricResult {
    let has_non_finite_value = result
        .points
        .iter()
        .any(|point| !point.point.value_f64.is_finite());
    let mut reasons = result
        .reasons
        .into_iter()
        .map(|reason| match reason {
            AlignmentReason::MissingRunStart => EvidenceReason::MissingRunStart,
            AlignmentReason::NegativeAxis => EvidenceReason::NegativeAxis,
            AlignmentReason::DecreasingAxis => EvidenceReason::DecreasingAxis,
        })
        .collect::<Vec<_>>();
    if has_non_finite_value {
        reasons.push(EvidenceReason::NonFiniteValue);
    }
    let mut completeness = if reasons.iter().any(|reason| {
        matches!(
            reason,
            EvidenceReason::NegativeAxis
                | EvidenceReason::DecreasingAxis
                | EvidenceReason::NonFiniteValue
        )
    }) {
        EvidenceCompleteness::Invalid
    } else if reasons.contains(&EvidenceReason::MissingRunStart) || result.points.is_empty() {
        if reasons.is_empty() {
            reasons.push(EvidenceReason::MissingMetric);
        }
        EvidenceCompleteness::Unavailable
    } else {
        EvidenceCompleteness::Complete
    };
    qualify_lifecycle(run_status, &mut completeness, &mut reasons);
    AlignedMetricResult {
        points: result.points,
        source_row_count: result.source_row_count,
        completeness,
        reasons,
    }
}

fn objective_evidence(
    run_id: &RunId,
    run_status: RunStatus,
    aggregate: Option<MetricAggregate>,
) -> ObjectiveEvidence {
    let (last_step, last_value_f64, mut completeness, mut reasons) = match aggregate {
        None => (
            None,
            None,
            EvidenceCompleteness::Unavailable,
            vec![EvidenceReason::MissingMetric],
        ),
        Some(aggregate) if !aggregate.last_value_f64.is_finite() => (
            Some(aggregate.last_step),
            Some(aggregate.last_value_f64),
            EvidenceCompleteness::Invalid,
            vec![EvidenceReason::NonFiniteValue],
        ),
        Some(aggregate) => (
            Some(aggregate.last_step),
            Some(aggregate.last_value_f64),
            EvidenceCompleteness::Complete,
            Vec::new(),
        ),
    };
    qualify_lifecycle(run_status, &mut completeness, &mut reasons);
    ObjectiveEvidence {
        run_id: run_id.clone(),
        run_status,
        last_step,
        last_value_f64,
        completeness,
        reasons,
    }
}

fn qualify_lifecycle(
    run_status: RunStatus,
    completeness: &mut EvidenceCompleteness,
    reasons: &mut Vec<EvidenceReason>,
) {
    let reason = match run_status {
        RunStatus::Running => Some(EvidenceReason::RunRunning),
        RunStatus::Failed => Some(EvidenceReason::RunFailed),
        RunStatus::Finished => None,
    };
    if let Some(reason) = reason {
        *completeness = (*completeness).max(EvidenceCompleteness::Partial);
        reasons.push(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aligned_point(value_f64: f64) -> crate::model::alignment::AlignedMetricPoint {
        let timestamp = chrono::Utc::now();
        crate::model::alignment::AlignedMetricPoint {
            point: MetricPoint {
                run_id: RunId::from_string("run-1"),
                metric_key: MetricKey::from_string("loss"),
                step: Step::new(0),
                timestamp,
                value_f64,
                ingested_at: timestamp,
            },
            axis_value: 0,
        }
    }

    #[test]
    fn objective_evidence_retains_invalid_value_and_failed_reason() {
        let run_id = RunId::from_string("run-1");
        let evidence = objective_evidence(
            &run_id,
            RunStatus::Failed,
            Some(MetricAggregate {
                run_id: run_id.clone(),
                metric_key: MetricKey::from_string("loss"),
                effective_count: 1,
                last_step: Step::new(4),
                last_value_f64: f64::NAN,
                min_value_f64: f64::NAN,
                max_value_f64: f64::NAN,
            }),
        );

        assert_eq!(
            (evidence.completeness, evidence.reasons),
            (
                EvidenceCompleteness::Invalid,
                vec![EvidenceReason::NonFiniteValue, EvidenceReason::RunFailed]
            )
        );
    }

    #[test]
    fn aligned_empty_running_series_is_unavailable_with_both_reasons() {
        let result = aligned_metric_result(
            AlignmentQueryResult {
                points: Vec::new(),
                source_row_count: 0,
                reasons: Vec::new(),
            },
            RunStatus::Running,
        );

        assert_eq!(
            (result.completeness, result.reasons),
            (
                EvidenceCompleteness::Unavailable,
                vec![EvidenceReason::MissingMetric, EvidenceReason::RunRunning]
            )
        );
    }

    #[test]
    fn aligned_non_finite_finished_series_is_invalid_without_repair() {
        let result = aligned_metric_result(
            AlignmentQueryResult {
                points: vec![aligned_point(f64::NAN)],
                source_row_count: 1,
                reasons: Vec::new(),
            },
            RunStatus::Finished,
        );

        assert_eq!(result.completeness, EvidenceCompleteness::Invalid);
        assert_eq!(result.reasons, vec![EvidenceReason::NonFiniteValue]);
        assert!(result.points[0].point.value_f64.is_nan());
    }
}
