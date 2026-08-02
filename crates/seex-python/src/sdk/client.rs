use pyo3::create_exception;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use seex::{ClientDiagnostics, MetricAggregate, MetricPoint, Project};

create_exception!(
    seex._seex,
    SeexError,
    PyRuntimeError,
    "Base class for Seex SDK errors."
);

macro_rules! sdk_exception {
    ($name:ident, $message:literal) => {
        create_exception!(seex._seex, $name, SeexError, $message);
    };
}

sdk_exception!(MetricQueueFullError, "The metric queue is full.");
sdk_exception!(MetricWriterFailedError, "The metric writer failed.");
sdk_exception!(MetricDrainTimeoutError, "Metric drain timed out.");
sdk_exception!(MetricFlushError, "Metric flush failed.");
sdk_exception!(MetricFlushTimeoutError, "Metric flush timed out.");
sdk_exception!(RunClosedError, "The run is closed for metric reporting.");
sdk_exception!(
    InvalidRunStateError,
    "The run state does not allow this operation."
);
sdk_exception!(
    RunAlreadyExistsError,
    "A run with the requested run_id already exists."
);
sdk_exception!(
    RunAlreadyActiveError,
    "The requested run already has an active writer."
);
sdk_exception!(InvalidConfigurationError, "Seex configuration is invalid.");
sdk_exception!(StorageError, "A storage operation failed.");
sdk_exception!(ApiClosedError, "The read-only API is closed.");

#[pyclass(name = "Project", module = "seex._seex")]
pub struct PyProject {
    #[pyo3(get)]
    project_id: String,
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    created_at: String,
}

impl From<Project> for PyProject {
    fn from(project: Project) -> Self {
        Self {
            project_id: project.project_id.as_str().to_owned(),
            name: project.name,
            created_at: project.created_at.to_rfc3339(),
        }
    }
}

#[pyclass(name = "Diagnostics", module = "seex._seex")]
pub struct PyDiagnostics {
    #[pyo3(get)]
    pending_reports: u64,
    #[pyo3(get)]
    queue_full_errors: u64,
    #[pyo3(get)]
    persisted_reports: u64,
    #[pyo3(get)]
    writer_state: &'static str,
    #[pyo3(get)]
    last_write_error: Option<String>,
    #[pyo3(get)]
    last_flush_run_id: Option<String>,
    #[pyo3(get)]
    last_flush_status: &'static str,
    #[pyo3(get)]
    last_flush_error: Option<String>,
}

impl From<ClientDiagnostics> for PyDiagnostics {
    fn from(diagnostics: ClientDiagnostics) -> Self {
        Self {
            pending_reports: diagnostics.pending_reports,
            queue_full_errors: diagnostics.queue_full_errors,
            persisted_reports: diagnostics.persisted_reports,
            writer_state: match diagnostics.writer_state {
                seex::WriterState::Running => "running",
                seex::WriterState::Retrying => "retrying",
                seex::WriterState::Failed => "failed",
                seex::WriterState::Drained => "drained",
                seex::WriterState::Closed => "closed",
            },
            last_write_error: diagnostics.last_write_error,
            last_flush_run_id: diagnostics.last_flush_run_id,
            last_flush_status: match diagnostics.last_flush_state {
                seex::FlushState::None => "none",
                seex::FlushState::Running => "running",
                seex::FlushState::Succeeded => "succeeded",
                seex::FlushState::Failed => "failed",
                seex::FlushState::TimedOut => "timed_out",
            },
            last_flush_error: diagnostics.last_flush_error,
        }
    }
}

#[pyclass(name = "MetricPoint", module = "seex._seex")]
pub struct PyMetricPoint {
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
}

impl From<MetricPoint> for PyMetricPoint {
    fn from(point: MetricPoint) -> Self {
        Self {
            run_id: point.run_id.as_str().to_owned(),
            metric_key: point.metric_key.as_str().to_owned(),
            step: point.step.value(),
            timestamp: point.timestamp.to_rfc3339(),
            value_f64: point.value_f64,
            ingested_at: point.ingested_at.to_rfc3339(),
        }
    }
}

#[pyclass(name = "MetricSummary", module = "seex._seex")]
pub struct PyMetricSummary {
    #[pyo3(get)]
    run_id: String,
    #[pyo3(get)]
    metric_key: String,
    #[pyo3(get)]
    effective_count: u64,
    #[pyo3(get)]
    last_step: i64,
    #[pyo3(get)]
    last_value_f64: f64,
    #[pyo3(get)]
    min_value_f64: f64,
    #[pyo3(get)]
    max_value_f64: f64,
}

impl From<MetricAggregate> for PyMetricSummary {
    fn from(summary: MetricAggregate) -> Self {
        Self {
            run_id: summary.run_id.as_str().to_owned(),
            metric_key: summary.metric_key.as_str().to_owned(),
            effective_count: summary.effective_count,
            last_step: summary.last_step.value(),
            last_value_f64: summary.last_value_f64,
            min_value_f64: summary.min_value_f64,
            max_value_f64: summary.max_value_f64,
        }
    }
}
