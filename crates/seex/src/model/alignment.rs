use crate::model::comparison::{EvidenceCompleteness, EvidenceReason};
use crate::model::metric::{MetricKey, MetricPoint};
use crate::model::run::RunId;

const EXTREMA_PER_BUCKET: usize = 4;
const MAX_SCREEN_QUERY_POINTS: usize = 1_000_000;

/// Horizontal coordinate used to align an effective metric series.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlignmentAxis {
    Step,
    ElapsedTime,
}

/// Closed integer viewport used by aligned metric queries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AlignmentViewport {
    start: i64,
    end: i64,
}

impl AlignmentViewport {
    /// Creates a non-decreasing closed viewport `[start, end]`.
    ///
    /// # Errors
    ///
    /// Returns [`AlignmentQueryError::InvalidViewport`] when `start > end`.
    pub const fn new(start: i64, end: i64) -> Result<Self, AlignmentQueryError> {
        if start > end {
            return Err(AlignmentQueryError::InvalidViewport);
        }
        Ok(Self { start, end })
    }

    pub const fn start(self) -> i64 {
        self.start
    }

    pub const fn end(self) -> i64 {
        self.end
    }
}

/// Reduction applied after alignment and closed-viewport selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlignmentReduction {
    Full,
    ScreenBudget(AlignmentScreenBudget),
}

/// Validated positive inputs for screen-budgeted extrema reduction.
///
/// ```compile_fail
/// use seex::model::alignment::AlignmentScreenBudget;
/// let _invalid = AlignmentScreenBudget { pixel_width: 0, points_per_pixel: 1 };
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AlignmentScreenBudget {
    pixel_width: u32,
    points_per_pixel: u16,
}

impl AlignmentReduction {
    /// Creates a positive screen-derived extrema budget.
    ///
    /// # Errors
    ///
    /// Returns [`AlignmentQueryError::InvalidScreenBudget`] for a zero input.
    pub const fn screen_budget(
        pixel_width: u32,
        points_per_pixel: u16,
    ) -> Result<Self, AlignmentQueryError> {
        if pixel_width == 0 || points_per_pixel == 0 {
            return Err(AlignmentQueryError::InvalidScreenBudget);
        }
        Ok(Self::ScreenBudget(AlignmentScreenBudget {
            pixel_width,
            points_per_pixel,
        }))
    }

    pub fn max_points(self) -> Option<usize> {
        match self {
            Self::Full => None,
            Self::ScreenBudget(budget) => Some(
                (budget.pixel_width as usize)
                    .saturating_mul(budget.points_per_pixel as usize)
                    .clamp(EXTREMA_PER_BUCKET, MAX_SCREEN_QUERY_POINTS),
            ),
        }
    }
}

/// Renderer-independent aligned metric query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlignmentQuery {
    pub run_id: RunId,
    pub metric_key: MetricKey,
    pub axis: AlignmentAxis,
    pub viewport: AlignmentViewport,
    pub reduction: AlignmentReduction,
}

/// One effective metric point and its derived horizontal coordinate.
#[derive(Clone, Debug, PartialEq)]
pub struct AlignedMetricPoint {
    pub point: MetricPoint,
    pub axis_value: i64,
}

/// Structured reason attached to unavailable or invalid aligned evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlignmentReason {
    MissingRunStart,
    NegativeAxis,
    DecreasingAxis,
}

/// Result of an aligned storage query.
#[derive(Clone, Debug, PartialEq)]
pub struct AlignmentQueryResult {
    pub points: Vec<AlignedMetricPoint>,
    /// Visible points plus strict viewport neighbors before reduction.
    pub source_row_count: u64,
    pub reasons: Vec<AlignmentReason>,
}

impl AlignmentQueryResult {
    pub fn downsampled(&self) -> bool {
        self.source_row_count > self.points.len() as u64
    }
}

/// Product-level aligned metric evidence returned by Core.
#[derive(Clone, Debug, PartialEq)]
pub struct AlignedMetricResult {
    pub points: Vec<AlignedMetricPoint>,
    pub source_row_count: u64,
    pub completeness: EvidenceCompleteness,
    pub reasons: Vec<EvidenceReason>,
}

impl AlignedMetricResult {
    pub fn downsampled(&self) -> bool {
        self.source_row_count > self.points.len() as u64
    }
}

/// Invalid aligned metric query inputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AlignmentQueryError {
    #[error("alignment viewport must be a non-decreasing closed range")]
    InvalidViewport,
    #[error("screen query pixel width and point density must be positive")]
    InvalidScreenBudget,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_accepts_a_single_closed_coordinate() {
        let viewport = AlignmentViewport::new(4, 4).expect("single coordinate should be valid");

        assert_eq!((viewport.start(), viewport.end()), (4, 4));
    }

    #[test]
    fn viewport_rejects_decreasing_bounds() {
        let error = AlignmentViewport::new(5, 4).expect_err("decreasing viewport should fail");

        assert_eq!(error, AlignmentQueryError::InvalidViewport);
    }

    #[test]
    fn screen_budget_is_positive_and_bounded() {
        assert_eq!(
            AlignmentReduction::screen_budget(0, 1),
            Err(AlignmentQueryError::InvalidScreenBudget)
        );
        let reduction = AlignmentReduction::screen_budget(u32::MAX, u16::MAX)
            .expect("positive budget should be valid");
        assert_eq!(reduction.max_points(), Some(MAX_SCREEN_QUERY_POINTS));
    }
}
