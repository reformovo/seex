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
}
