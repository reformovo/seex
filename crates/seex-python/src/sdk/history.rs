use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyCapsule;
use pyo3::types::{PyAny, PyBool, PyDateTime, PyDelta};
use seex::{
    EvidenceCompleteness, EvidenceReason, MetricAxis, MetricQuery, MetricRange, MetricSeries,
    RelativeTime, Step, Timestamp,
};

use crate::sdk::arrow::metric_series_stream;
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

    #[pyo3(signature = (requested_schema=None))]
    fn __arrow_c_stream__<'py>(
        &self,
        py: Python<'py>,
        requested_schema: Option<&Bound<'py, PyCapsule>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        let _ = requested_schema;
        metric_series_stream(py, &self.series)
    }
}

pub fn metric_query(
    axis: &str,
    start: Option<&Bound<'_, PyAny>>,
    end: Option<&Bound<'_, PyAny>>,
    max_points: Option<&Bound<'_, PyAny>>,
) -> PyResult<MetricQuery> {
    let range = match axis {
        "step" => step_range(start, end)?,
        "relative_time" => relative_time_range(start, end)?,
        "timestamp" => timestamp_range(start, end)?,
        _ => {
            return Err(PyValueError::new_err(
                "x_axis must be 'step', 'relative_time', or 'timestamp'",
            ));
        }
    };
    MetricQuery::new(range, point_limit(max_points)?)
        .map_err(|error| PyValueError::new_err(error.to_string()))
}

fn step_range(
    start: Option<&Bound<'_, PyAny>>,
    end: Option<&Bound<'_, PyAny>>,
) -> PyResult<MetricRange> {
    Ok(
        match (
            start.map(step_bound).transpose()?,
            end.map(step_bound).transpose()?,
        ) {
            (None, None) => MetricRange::All(MetricAxis::Step),
            (Some(start), None) => MetricRange::StepsFrom { start },
            (None, Some(end)) => MetricRange::StepsUntil { end },
            (Some(start), Some(end)) => MetricRange::Steps { start, end },
        },
    )
}

fn relative_time_range(
    start: Option<&Bound<'_, PyAny>>,
    end: Option<&Bound<'_, PyAny>>,
) -> PyResult<MetricRange> {
    Ok(
        match (
            start.map(relative_time_bound).transpose()?,
            end.map(relative_time_bound).transpose()?,
        ) {
            (None, None) => MetricRange::All(MetricAxis::RelativeTime),
            (Some(start), None) => MetricRange::RelativeTimeFrom { start },
            (None, Some(end)) => MetricRange::RelativeTimeUntil { end },
            (Some(start), Some(end)) => MetricRange::RelativeTime { start, end },
        },
    )
}

fn timestamp_range(
    start: Option<&Bound<'_, PyAny>>,
    end: Option<&Bound<'_, PyAny>>,
) -> PyResult<MetricRange> {
    Ok(
        match (
            start.map(timestamp_bound).transpose()?,
            end.map(timestamp_bound).transpose()?,
        ) {
            (None, None) => MetricRange::All(MetricAxis::Timestamp),
            (Some(start), None) => MetricRange::TimestampsFrom { start },
            (None, Some(end)) => MetricRange::TimestampsUntil { end },
            (Some(start), Some(end)) => MetricRange::Timestamps { start, end },
        },
    )
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

fn relative_time_bound(value: &Bound<'_, PyAny>) -> PyResult<RelativeTime> {
    let delta = value
        .cast::<PyDelta>()
        .map_err(|_| PyTypeError::new_err("relative_time bounds must be timedelta values"))?;
    let days = delta.getattr("days")?.extract::<i64>()?;
    let seconds = delta.getattr("seconds")?.extract::<i64>()?;
    let microseconds = delta.getattr("microseconds")?.extract::<i64>()?;
    if microseconds % 1_000 != 0 {
        return Err(PyValueError::new_err(
            "relative_time bounds must have millisecond precision",
        ));
    }
    let millis = days
        .checked_mul(86_400_000)
        .and_then(|value| value.checked_add(seconds * 1_000))
        .and_then(|value| value.checked_add(microseconds / 1_000))
        .ok_or_else(|| PyValueError::new_err("relative_time bound is out of range"))?;
    Ok(RelativeTime::from_millis(millis))
}

fn timestamp_bound(value: &Bound<'_, PyAny>) -> PyResult<Timestamp> {
    let datetime = value
        .cast::<PyDateTime>()
        .map_err(|_| PyTypeError::new_err("timestamp bounds must be datetime values"))?;
    if datetime.getattr("tzinfo")?.is_none() || datetime.call_method0("utcoffset")?.is_none() {
        return Err(PyValueError::new_err(
            "timestamp bounds must be timezone-aware datetime values",
        ));
    }
    if datetime.getattr("microsecond")?.extract::<u32>()? % 1_000 != 0 {
        return Err(PyValueError::new_err(
            "timestamp bounds must have millisecond precision",
        ));
    }
    let millis = datetime.call_method0("timestamp")?.extract::<f64>()? * 1_000.0;
    if !millis.is_finite()
        || millis.fract() != 0.0
        || millis < i64::MIN as f64
        || millis > i64::MAX as f64
    {
        return Err(PyValueError::new_err(
            "timestamp bound cannot be represented as UTC milliseconds",
        ));
    }
    Ok(Timestamp::from_millis(millis as i64))
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
