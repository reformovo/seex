//! Temporary adapters preserving the shipped Python query surface during U1.

use pyo3::prelude::*;
use seex::{
    EvidenceCompleteness, EvidenceReason, MetricAxis, MetricCoordinate, MetricKey, MetricSample,
    MetricSeries, RunId, Step,
};

use crate::engine::client::NativeClient;
use crate::engine::query::MetricQueryResult;
use crate::sdk::client::{SeexError, runtime_error};

pub fn query_step_metric(
    client: &NativeClient,
    run_id: &RunId,
    metric_key: &MetricKey,
    start_step: Option<Step>,
    end_step: Option<Step>,
    max_points: Option<usize>,
) -> PyResult<MetricSeries> {
    let result = client
        .query_metric_with_metadata(run_id, metric_key, start_step, end_step, max_points)
        .map_err(runtime_error)?;
    metric_series(result)
}

fn metric_series(result: MetricQueryResult) -> PyResult<MetricSeries> {
    let samples = result
        .points
        .into_iter()
        .map(|point| MetricSample {
            coordinate: MetricCoordinate::Step(point.step),
            point,
        })
        .collect::<Vec<_>>();
    let (completeness, reasons) = if samples.is_empty() {
        (
            EvidenceCompleteness::Unavailable,
            vec![EvidenceReason::MissingMetric],
        )
    } else {
        (EvidenceCompleteness::Complete, Vec::new())
    };
    MetricSeries::from_samples(
        MetricAxis::Step,
        samples,
        result.source_row_count,
        completeness,
        reasons,
    )
    .map_err(|error| SeexError::new_err(format!("invalid compatibility metric series: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_legacy_results_become_unavailable_step_series() -> PyResult<()> {
        let series = metric_series(MetricQueryResult {
            points: Vec::new(),
            source_row_count: 0,
        })?;

        assert_eq!(series.axis(), MetricAxis::Step);
        assert!(series.samples().is_empty());
        assert_eq!(series.completeness(), EvidenceCompleteness::Unavailable);
        assert_eq!(series.reasons(), [EvidenceReason::MissingMetric]);
        Ok(())
    }
}
