use chrono::{DateTime, Utc};

use crate::run::RunId;

const MAX_SCREEN_QUERY_POINTS: usize = 1_000_000;
const EXTREMA_PER_BUCKET: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct MetricKey(String);

impl MetricKey {
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Step(i64);

impl Step {
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> i64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricPoint {
    pub run_id: RunId,
    pub metric_key: MetricKey,
    pub step: Step,
    pub timestamp: DateTime<Utc>,
    pub value_f64: f64,
    pub ingested_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricAggregate {
    pub run_id: RunId,
    pub metric_key: MetricKey,
    pub effective_count: u64,
    pub last_step: Step,
    pub last_value_f64: f64,
    pub min_value_f64: f64,
    pub max_value_f64: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricQuery {
    pub run_id: RunId,
    pub metric_key: MetricKey,
    pub start_step: Option<Step>,
    pub end_step: Option<Step>,
    pub reduction: ReductionPolicy,
}

impl MetricQuery {
    /// Creates a query over a half-open step range.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidStepRange`] for non-increasing bounds.
    pub fn new(
        run_id: RunId,
        metric_key: MetricKey,
        start_step: Option<Step>,
        end_step: Option<Step>,
        reduction: ReductionPolicy,
    ) -> Result<Self, QueryError> {
        if let (Some(start), Some(end)) = (start_step, end_step)
            && start >= end
        {
            return Err(QueryError::InvalidStepRange);
        }
        Ok(Self {
            run_id,
            metric_key,
            start_step,
            end_step,
            reduction,
        })
    }
}

/// Point reduction applied after effective-series and range filtering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReductionPolicy {
    Full,
    Lttb {
        max_points: usize,
    },
    ScreenBudget {
        pixel_width: u32,
        points_per_pixel: u16,
    },
}

impl ReductionPolicy {
    /// Creates an LTTB limit compatible with the Python query surface.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::LttbMaxPointsTooSmall`] below two points.
    pub const fn lttb(max_points: usize) -> Result<Self, QueryError> {
        if max_points < 2 {
            return Err(QueryError::LttbMaxPointsTooSmall { max_points });
        }
        Ok(Self::Lttb { max_points })
    }

    /// Creates a positive screen-derived point budget.
    ///
    /// # Errors
    ///
    /// Returns [`QueryError::InvalidScreenBudget`] when either input is zero.
    pub const fn screen_budget(
        pixel_width: u32,
        points_per_pixel: u16,
    ) -> Result<Self, QueryError> {
        if pixel_width == 0 || points_per_pixel == 0 {
            return Err(QueryError::InvalidScreenBudget);
        }
        Ok(Self::ScreenBudget {
            pixel_width,
            points_per_pixel,
        })
    }

    pub fn max_points(self) -> Option<usize> {
        match self {
            Self::Full => None,
            Self::Lttb { max_points } => Some(max_points),
            Self::ScreenBudget {
                pixel_width,
                points_per_pixel,
            } => Some(
                (pixel_width as usize)
                    .saturating_mul(points_per_pixel as usize)
                    .clamp(EXTREMA_PER_BUCKET, MAX_SCREEN_QUERY_POINTS),
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricQueryResult {
    pub points: Vec<MetricPoint>,
    pub source_row_count: u64,
}

impl MetricQueryResult {
    pub fn downsampled(&self) -> bool {
        self.source_row_count > self.points.len() as u64
    }
}

/// Invalid metric query inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum QueryError {
    #[error("step range must form a non-empty half-open range")]
    InvalidStepRange,
    #[error("LTTB max_points must be at least 2, got {max_points}")]
    LttbMaxPointsTooSmall { max_points: usize },
    #[error("screen query pixel width and point density must be positive")]
    InvalidScreenBudget,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_rejects_non_increasing_step_range() {
        let error = MetricQuery::new(
            RunId::from_string("run-1"),
            MetricKey::from_string("loss"),
            Some(Step::new(4)),
            Some(Step::new(4)),
            ReductionPolicy::Full,
        )
        .expect_err("empty ranges should fail");

        assert_eq!(error, QueryError::InvalidStepRange);
    }

    #[test]
    fn screen_budget_is_bounded_for_query_planning() {
        let reduction = ReductionPolicy::screen_budget(u32::MAX, u16::MAX)
            .expect("positive budget should be valid");

        assert_eq!(reduction.max_points(), Some(MAX_SCREEN_QUERY_POINTS));
    }
}
