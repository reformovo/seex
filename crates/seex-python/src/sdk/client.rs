use pyo3::create_exception;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use seex::ClientDiagnostics;

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
