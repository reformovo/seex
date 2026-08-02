//! Public SDK errors without storage-engine details.

use std::fmt;

/// A result produced by the public Seex SDK.
pub type Result<T> = std::result::Result<T, Error>;

/// Matchable public failure categories.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    Configuration,
    InvalidRunOptions {
        field: &'static str,
    },
    RunAlreadyExists {
        run_id: String,
    },
    RunAlreadyActive {
        run_id: String,
    },
    RunNotFound {
        run_id: String,
    },
    DuplicateRunIdentity {
        run_id: String,
    },
    InvalidRunState {
        run_id: String,
    },
    RunProjectMismatch {
        run_id: String,
    },
    InvalidMetricMapping,
    MetricMappingTooLarge {
        count: usize,
        maximum: usize,
    },
    MetricQueueFull,
    RunClosed {
        run_id: String,
    },
    StepRegression {
        cursor: i64,
        attempted: i64,
    },
    StepOverflow {
        step: i64,
    },
    TerminalOutcomeConflict {
        selected: crate::RunStatus,
        requested: crate::RunStatus,
    },
    MetricWriterFailed,
    MetricDrainTimeout,
    MetricFlushFailed,
    MetricFlushTimeout,
    ClientClosed,
    UnsupportedQuery,
    Storage,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration => formatter.write_str("Seex configuration is invalid"),
            Self::InvalidRunOptions { field } => {
                write!(formatter, "Seex Run option `{field}` is invalid")
            }
            Self::RunAlreadyExists { run_id } => write!(formatter, "Run already exists: {run_id}"),
            Self::RunAlreadyActive { run_id } => {
                write!(formatter, "Run already has an active writer: {run_id}")
            }
            Self::RunNotFound { run_id } => write!(formatter, "Run not found: {run_id}"),
            Self::DuplicateRunIdentity { run_id } => {
                write!(formatter, "duplicate Run identity in request: {run_id}")
            }
            Self::InvalidRunState { run_id } => {
                write!(
                    formatter,
                    "Run state does not allow this operation: {run_id}"
                )
            }
            Self::RunProjectMismatch { run_id } => {
                write!(formatter, "Run belongs to a different Project: {run_id}")
            }
            Self::InvalidMetricMapping => formatter.write_str("metric Mapping is invalid"),
            Self::MetricMappingTooLarge { count, maximum } => {
                write!(
                    formatter,
                    "metric Mapping has {count} points; maximum is {maximum}"
                )
            }
            Self::MetricQueueFull => formatter.write_str("Seex metric queue is full"),
            Self::RunClosed { run_id } => write!(formatter, "Run is closed: {run_id}"),
            Self::StepRegression { cursor, attempted } => write!(
                formatter,
                "committed step {attempted} regresses from cursor {cursor}"
            ),
            Self::StepOverflow { step } => {
                write!(formatter, "step {step} has no representable successor")
            }
            Self::TerminalOutcomeConflict {
                selected,
                requested,
            } => write!(
                formatter,
                "Run terminal outcome conflicts: selected {selected:?}, requested {requested:?}"
            ),
            Self::MetricWriterFailed => formatter.write_str("Seex metric writer failed"),
            Self::MetricDrainTimeout => formatter.write_str("Seex metric drain timed out"),
            Self::MetricFlushFailed => formatter.write_str("Seex metric flush failed"),
            Self::MetricFlushTimeout => formatter.write_str("Seex metric flush timed out"),
            Self::ClientClosed => formatter.write_str("Seex client is closed"),
            Self::UnsupportedQuery => formatter.write_str("Reader query is not yet supported"),
            Self::Storage => formatter.write_str("Seex storage operation failed"),
        }
    }
}

impl std::error::Error for Error {}

impl From<crate::engine::EngineError> for Error {
    fn from(error: crate::engine::EngineError) -> Self {
        use crate::engine::EngineError;

        match error {
            EngineError::RunAlreadyExists { run_id } => Self::RunAlreadyExists { run_id },
            EngineError::RunAlreadyActive { run_id } => Self::RunAlreadyActive { run_id },
            EngineError::RunNotFound { run_id } => Self::RunNotFound { run_id },
            EngineError::DuplicateRunIdentity { run_id } => Self::DuplicateRunIdentity { run_id },
            EngineError::InvalidRunTransition { run_id, .. } => Self::InvalidRunState { run_id },
            EngineError::MetricQueueFull => Self::MetricQueueFull,
            EngineError::InvalidMetricBatch { count } => Self::MetricMappingTooLarge {
                count,
                maximum: 8_192,
            },
            EngineError::RunClosed { run_id } => Self::RunClosed { run_id },
            EngineError::StepRegression { cursor, attempted } => {
                Self::StepRegression { cursor, attempted }
            }
            EngineError::StepOverflow { step } => Self::StepOverflow { step },
            EngineError::MetricWriterFailed { .. } => Self::MetricWriterFailed,
            EngineError::MetricDrainTimeout => Self::MetricDrainTimeout,
            EngineError::MetricFlush { .. } => Self::MetricFlushFailed,
            EngineError::MetricFlushTimeout => Self::MetricFlushTimeout,
            EngineError::ClientClosed => Self::ClientClosed,
            _ => Self::Storage,
        }
    }
}
