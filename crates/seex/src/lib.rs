//! Unified Rust SDK facade for Seex.
//!
//! This facade exposes the stable read surface while keeping storage engines,
//! their errors, and native connection types as implementation details.
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

#[doc(hidden)]
pub mod engine;
mod error;
#[doc(hidden)]
pub mod model;
mod reader;
#[doc(hidden)]
pub mod storage;

#[cfg(test)]
mod ducklake_test_support;
#[cfg(test)]
mod native_engine_behavior;

pub use crate::model::comparison::{
    ComparisonOutcome, ComparisonPreference, ComparisonReport, ComparisonResult,
    EvidenceCompleteness, EvidenceReason, MetricComparisonResult, ObjectiveDirection,
    ObjectiveEvidence, ObjectiveMetric, RankingEntry, RankingResult,
};
pub use crate::model::metric::{MetricAggregate, MetricKey, MetricPoint, Step};
pub use crate::model::run::{Run, RunId, RunStatus};
pub use crate::model::types::{Project, ProjectId};
pub use error::{Error, Result};
pub use reader::{
    MetricAxis, MetricCoordinate, MetricQuery, MetricQueryError, MetricRange, MetricSample,
    MetricSeries, MetricSeriesError, Reader, ReaderBuilder, RelativeTime, Timestamp,
};
