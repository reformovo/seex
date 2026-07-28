//! Native project and standalone Parquet storage for Seex.

#![forbid(unsafe_code)]

mod alignment_query;
pub mod bootstrap;
pub mod config;
mod lock;
mod metric_query;
mod project;
mod query;
pub mod rows;
mod schema;
mod sql;
pub mod time;
pub mod write;

use seex_model::alignment::{AlignmentQuery, AlignmentQueryResult};
use seex_model::metric::{MetricQuery, MetricQueryResult};

pub use lock::RunWriterGuard;
pub use metric_query::{ProjectMetricReader, percent_encode_metric_key};
pub use project::{MetricWrite, ProjectConnection};
pub use query::{ParquetMetricReader, ParquetSource};
pub use schema::{ColumnSchema, SchemaReport, validate_metric_point_schema};

/// Common metric-series query interface for current storage inputs.
pub trait MetricReader {
    fn query_metric(&self, query: &MetricQuery) -> Result<MetricQueryResult, StorageError>;
    fn query_aligned_metric(
        &self,
        query: &AlignmentQuery,
    ) -> Result<AlignmentQueryResult, StorageError>;
}

/// Failures returned by Seex storage operations.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("run already exists: {run_id}")]
    RunAlreadyExists { run_id: String },
    #[error("run not found: {run_id}")]
    RunNotFound { run_id: String },
    #[error("run already has an active writer: {run_id}")]
    RunAlreadyActive { run_id: String },
    #[error("catalog not found: {name}")]
    CatalogNotFound { name: String },
    #[error("storage operation failed while {operation}: {name}")]
    Storage {
        operation: &'static str,
        name: String,
        #[source]
        source: std::io::Error,
    },
    #[error("storage operation failed while {operation}: {name}")]
    StorageDuckDb {
        operation: &'static str,
        name: String,
        #[source]
        source: duckdb::Error,
    },
    #[error("Parquet source must not be empty")]
    EmptySource,
    #[error("run id and metric key must not be empty")]
    InvalidIdentity,
    #[error("metric query max_points is too large for DuckDB: {max_points}")]
    QueryMaxPointsTooLarge { max_points: usize },
    #[error("DuckDB LTTB extension is unavailable: {message}")]
    LttbExtensionUnavailable { message: String },
    #[error("invalid stored run status: {status}")]
    InvalidRunStatus { status: String },
    #[error("invalid stored timestamp for {field}: {millis}")]
    InvalidTimestamp { field: &'static str, millis: i64 },
    #[error("metric_points is missing required column `{name}`")]
    MissingColumn { name: &'static str },
    #[error("metric_points contains duplicate column `{name}`")]
    DuplicateColumn { name: String },
    #[error("metric_points column `{name}` must be {expected}, found {actual}")]
    IncompatibleColumnType {
        name: &'static str,
        expected: &'static str,
        actual: String,
    },
    #[error("additive metric_points column `{name}` must be nullable")]
    IncompatibleAdditiveColumn { name: String },
    #[error("DuckDB storage operation failed: {0}")]
    DuckDb(#[from] duckdb::Error),
}
