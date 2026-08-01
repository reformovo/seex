//! Unified Rust SDK facade for Seex.
//!
//! During U1 this unpublished facade introduces the stable read surface over
//! the existing workspace crates. Storage engines, their errors, and native
//! connection types remain implementation details.
//!
//! ```compile_fail
//! use seex::ProjectConnection;
//! ```
//!
//! ```compile_fail
//! use seex::ProjectMetricReader;
//! ```
//!
//! ```compile_fail
//! use seex::StorageError;
//! ```
//!
//! ```compile_fail
//! use seex::NativeQueryStore;
//! ```

#![forbid(unsafe_code)]

mod error;
mod reader;

pub use error::{Error, Result};
pub use reader::{
    MetricAxis, MetricCoordinate, MetricQuery, MetricQueryError, MetricRange, MetricSample,
    MetricSeries, MetricSeriesError, Reader, ReaderBuilder, RelativeTime, Timestamp,
};
pub use seex_model::comparison::{
    ComparisonOutcome, ComparisonPreference, ComparisonReport, ComparisonResult,
    EvidenceCompleteness, EvidenceReason, MetricComparisonResult, ObjectiveDirection,
    ObjectiveEvidence, ObjectiveMetric, RankingEntry, RankingResult,
};
pub use seex_model::metric::{MetricAggregate, MetricKey, MetricPoint, Step};
pub use seex_model::run::{Run, RunId, RunStatus};
pub use seex_model::types::{Project, ProjectId};
