//! Native engine over DuckLake.

pub mod bootstrap {
    pub use seex_storage::bootstrap::*;
}
pub mod client;
pub mod comparison;
pub mod query;
pub mod ranking;
pub mod reporting;
mod time {
    pub use seex_storage::time::*;
}
pub mod write {
    pub use seex_storage::write::*;
}

// Unified error type for engine operations.
// Merged from engine/error.rs per oracle review (simplify: extract to
// error.rs when this grows beyond a handful of variants).
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("native connection lock was poisoned")]
    ConnectionLockPoisoned,
    #[error("project already exists: {project_id}")]
    ProjectAlreadyExists { project_id: String },
    #[error("project not found: {project_id}")]
    ProjectNotFound { project_id: String },
    #[error("run already exists: {run_id}")]
    RunAlreadyExists { run_id: String },
    #[error("run already has an active writer: {run_id}")]
    RunAlreadyActive { run_id: String },
    #[error("run not found: {run_id}")]
    RunNotFound { run_id: String },
    #[error("duplicate run identity in request: {run_id}")]
    DuplicateRunIdentity { run_id: String },
    #[error("run is closed for metric reporting: {run_id}")]
    RunClosed { run_id: String },
    #[error("invalid run transition for {run_id}: {from} -> {to}")]
    InvalidRunTransition {
        run_id: String,
        from: &'static str,
        to: &'static str,
    },
    #[error("metric query max_points must be at least 2, got {max_points}")]
    MetricQueryMaxPointsTooSmall { max_points: usize },
    #[error("metric query max_points is too large for DuckDB LTTB: {max_points}")]
    MetricQueryMaxPointsTooLarge { max_points: usize },
    #[error("metric queue is full")]
    MetricQueueFull,
    #[error("metric writer failed: {message}")]
    MetricWriterFailed { message: String },
    #[error("metric drain timed out")]
    MetricDrainTimeout,
    #[error("metric flush failed: {message}")]
    MetricFlush { message: String },
    #[error("metric flush timed out")]
    MetricFlushTimeout,
    #[error("client is closed")]
    ClientClosed,
    #[error("catalog not found: {name}")]
    CatalogNotFound { name: String },
    #[error("storage operation failed while {operation}: {name}")]
    Storage {
        operation: &'static str,
        name: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    StorageFailure(seex_storage::StorageError),
    #[error("storage operation failed: {message}")]
    StorageLayer { message: String },
    #[error("invalid stored run status: {status}")]
    InvalidRunStatus { status: String },
    #[error("invalid stored timestamp for {field}: {millis}")]
    InvalidTimestamp { field: &'static str, millis: i64 },
}

impl From<seex_storage::StorageError> for EngineError {
    fn from(error: seex_storage::StorageError) -> Self {
        use seex_storage::StorageError;

        match error {
            error @ StorageError::DuckDb(_) => Self::StorageFailure(error),
            StorageError::Io(source) => Self::Io(source),
            StorageError::RunAlreadyExists { run_id } => Self::RunAlreadyExists { run_id },
            StorageError::RunNotFound { run_id } => Self::RunNotFound { run_id },
            StorageError::RunAlreadyActive { run_id } => Self::RunAlreadyActive { run_id },
            StorageError::CatalogNotFound { name } => Self::CatalogNotFound { name },
            StorageError::Storage {
                operation,
                name,
                source,
            } => Self::Storage {
                operation,
                name,
                source,
            },
            error @ StorageError::StorageDuckDb { .. } => Self::StorageFailure(error),
            StorageError::QueryMaxPointsTooLarge { max_points } => {
                Self::MetricQueryMaxPointsTooLarge { max_points }
            }
            error @ StorageError::LttbExtensionUnavailable { .. } => Self::StorageFailure(error),
            StorageError::InvalidRunStatus { status } => Self::InvalidRunStatus { status },
            StorageError::InvalidTimestamp { field, millis } => {
                Self::InvalidTimestamp { field, millis }
            }
            other => Self::StorageLayer {
                message: other.to_string(),
            },
        }
    }
}
