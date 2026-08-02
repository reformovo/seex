use pyo3::prelude::*;

use crate::model::alignment::{AlignedMetricPoint, AlignedMetricResult};
use crate::model::comparison::{EvidenceCompleteness, EvidenceReason};

#[derive(Clone)]
#[pyclass(
    name = "AlignedMetricPoint",
    module = "seex._seex",
    skip_from_py_object
)]
pub struct PyAlignedMetricPoint {
    #[pyo3(get)]
    run_id: String,
    #[pyo3(get)]
    metric_key: String,
    #[pyo3(get)]
    step: i64,
    #[pyo3(get)]
    timestamp: String,
    #[pyo3(get)]
    value_f64: f64,
    #[pyo3(get)]
    ingested_at: String,
    #[pyo3(get)]
    axis_value: i64,
}

impl From<AlignedMetricPoint> for PyAlignedMetricPoint {
    fn from(aligned: AlignedMetricPoint) -> Self {
        let point = aligned.point;
        Self {
            run_id: point.run_id.as_str().to_owned(),
            metric_key: point.metric_key.as_str().to_owned(),
            step: point.step.value(),
            timestamp: point.timestamp.to_rfc3339(),
            value_f64: point.value_f64,
            ingested_at: point.ingested_at.to_rfc3339(),
            axis_value: aligned.axis_value,
        }
    }
}

#[pyclass(name = "AlignedMetricResult", module = "seex._seex")]
pub struct PyAlignedMetricResult {
    points: Vec<PyAlignedMetricPoint>,
    #[pyo3(get)]
    source_row_count: u64,
    #[pyo3(get)]
    downsampled: bool,
    #[pyo3(get)]
    completeness: String,
    #[pyo3(get)]
    reasons: Vec<String>,
}

#[pymethods]
impl PyAlignedMetricResult {
    #[getter]
    fn points(&self) -> Vec<PyAlignedMetricPoint> {
        self.points.clone()
    }
}

impl From<AlignedMetricResult> for PyAlignedMetricResult {
    fn from(result: AlignedMetricResult) -> Self {
        let downsampled = result.downsampled();
        Self {
            points: result
                .points
                .into_iter()
                .map(PyAlignedMetricPoint::from)
                .collect(),
            source_row_count: result.source_row_count,
            downsampled,
            completeness: completeness_value(result.completeness).to_owned(),
            reasons: result
                .reasons
                .into_iter()
                .map(|reason| reason_value(reason).to_owned())
                .collect(),
        }
    }
}

fn completeness_value(completeness: EvidenceCompleteness) -> &'static str {
    match completeness {
        EvidenceCompleteness::Complete => "complete",
        EvidenceCompleteness::Partial => "partial",
        EvidenceCompleteness::Unavailable => "unavailable",
        EvidenceCompleteness::Invalid => "invalid",
    }
}

fn reason_value(reason: EvidenceReason) -> &'static str {
    match reason {
        EvidenceReason::MissingMetric => "missing_metric",
        EvidenceReason::MissingRunStart => "missing_run_start",
        EvidenceReason::NegativeAxis => "negative_axis",
        EvidenceReason::DecreasingAxis => "decreasing_axis",
        EvidenceReason::NonFiniteValue => "non_finite_value",
        EvidenceReason::DiagnosticsUnavailable => "diagnostics_unavailable",
        EvidenceReason::RunRunning => "run_running",
        EvidenceReason::RunFailed => "run_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_unavailable_has_a_stable_python_value() {
        assert_eq!(
            reason_value(EvidenceReason::DiagnosticsUnavailable),
            "diagnostics_unavailable"
        );
    }
}
