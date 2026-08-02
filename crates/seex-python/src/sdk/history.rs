use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool};
use seex::{
    EvidenceCompleteness, EvidenceReason, MetricAxis, MetricQuery, MetricRange, MetricSeries, Step,
};

use crate::sdk::client::PyMetricPoint;

#[pyclass(name = "MetricSeries", module = "seex._seex")]
pub struct PyMetricSeries {
    pub(crate) series: MetricSeries,
}

#[pymethods]
impl PyMetricSeries {
    #[getter]
    fn axis(&self) -> &'static str {
        match self.series.axis() {
            MetricAxis::Step => "step",
            MetricAxis::RelativeTime => "relative_time",
            MetricAxis::Timestamp => "timestamp",
        }
    }

    #[getter]
    fn points(&self) -> Vec<PyMetricPoint> {
        self.series
            .samples()
            .iter()
            .map(|sample| PyMetricPoint::from(sample.point.clone()))
            .collect()
    }

    #[getter]
    fn source_count(&self) -> u64 {
        self.series.source_count()
    }

    #[getter]
    fn downsampled(&self) -> bool {
        self.series.downsampled()
    }

    #[getter]
    fn completeness(&self) -> &'static str {
        match self.series.completeness() {
            EvidenceCompleteness::Complete => "complete",
            EvidenceCompleteness::Partial => "partial",
            EvidenceCompleteness::Unavailable => "unavailable",
            EvidenceCompleteness::Invalid => "invalid",
        }
    }

    #[getter]
    fn reasons(&self) -> Vec<&'static str> {
        self.series.reasons().iter().map(reason_name).collect()
    }
}

pub fn metric_query(
    axis: &str,
    start: Option<&Bound<'_, PyAny>>,
    end: Option<&Bound<'_, PyAny>>,
    max_points: Option<&Bound<'_, PyAny>>,
) -> PyResult<MetricQuery> {
    if axis != "step" {
        return Err(PyValueError::new_err(
            "x_axis must be 'step', 'relative_time', or 'timestamp'",
        ));
    }
    let start = start.map(step_bound).transpose()?;
    let end = end.map(step_bound).transpose()?;
    let range = match (start, end) {
        (None, None) => MetricRange::All(MetricAxis::Step),
        (Some(start), None) => MetricRange::StepsFrom { start },
        (None, Some(end)) => MetricRange::StepsUntil { end },
        (Some(start), Some(end)) => MetricRange::Steps { start, end },
    };
    MetricQuery::new(range, point_limit(max_points)?)
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

fn step_bound(value: &Bound<'_, PyAny>) -> PyResult<Step> {
    if value.is_instance_of::<PyBool>() {
        return Err(PyTypeError::new_err(
            "step bounds must be integers, not bool",
        ));
    }
    value
        .extract::<i64>()
        .map(Step::new)
        .map_err(|_| PyTypeError::new_err("step bounds must be integers"))
}

fn point_limit(value: Option<&Bound<'_, PyAny>>) -> PyResult<Option<usize>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_instance_of::<PyBool>() {
        return Err(PyTypeError::new_err(
            "max_points must be an integer, not bool",
        ));
    }
    value
        .extract::<usize>()
        .map(Some)
        .map_err(|_| PyTypeError::new_err("max_points must be an integer or None"))
}

fn reason_name(reason: &EvidenceReason) -> &'static str {
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
