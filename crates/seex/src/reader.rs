//! Public read-query contracts.

use std::fmt;

use seex_model::metric::Step;

/// Horizontal coordinate requested for a metric series.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricAxis {
    Step,
    RelativeTime,
    Timestamp,
}

/// Milliseconds relative to a Run's start time.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RelativeTime(i64);

impl RelativeTime {
    pub const fn from_millis(value: i64) -> Self {
        Self(value)
    }

    pub const fn as_millis(self) -> i64 {
        self.0
    }
}

/// Milliseconds from the Unix epoch in UTC.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_millis(value: i64) -> Self {
        Self(value)
    }

    pub const fn as_millis(self) -> i64 {
        self.0
    }
}

/// A full axis or a typed half-open metric range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetricRange {
    All(MetricAxis),
    Steps {
        start: Step,
        end: Step,
    },
    RelativeTime {
        start: RelativeTime,
        end: RelativeTime,
    },
    Timestamps {
        start: Timestamp,
        end: Timestamp,
    },
}

impl MetricRange {
    pub const fn axis(&self) -> MetricAxis {
        match self {
            Self::All(axis) => *axis,
            Self::Steps { .. } => MetricAxis::Step,
            Self::RelativeTime { .. } => MetricAxis::RelativeTime,
            Self::Timestamps { .. } => MetricAxis::Timestamp,
        }
    }

    const fn is_empty(&self) -> bool {
        match self {
            Self::All(_) => false,
            Self::Steps { start, end } => start.value() >= end.value(),
            Self::RelativeTime { start, end } => start.as_millis() >= end.as_millis(),
            Self::Timestamps { start, end } => start.as_millis() >= end.as_millis(),
        }
    }
}

/// Validated options for one public metric-series query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricQuery {
    range: MetricRange,
    max_points: Option<usize>,
}

impl MetricQuery {
    /// Creates a typed query without deriving limits from display geometry.
    ///
    /// # Errors
    ///
    /// Returns [`MetricQueryError`] for an empty range or a point limit below
    /// two.
    pub const fn new(
        range: MetricRange,
        max_points: Option<usize>,
    ) -> Result<Self, MetricQueryError> {
        if range.is_empty() {
            return Err(MetricQueryError::EmptyRange);
        }
        if let Some(max_points) = max_points
            && max_points < 2
        {
            return Err(MetricQueryError::MaxPointsTooSmall { max_points });
        }
        Ok(Self { range, max_points })
    }

    pub const fn range(&self) -> &MetricRange {
        &self.range
    }

    pub const fn axis(&self) -> MetricAxis {
        self.range.axis()
    }

    pub const fn max_points(&self) -> Option<usize> {
        self.max_points
    }
}

/// Invalid public Reader query options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricQueryError {
    EmptyRange,
    MaxPointsTooSmall { max_points: usize },
}

impl fmt::Display for MetricQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRange => formatter.write_str("metric range must be non-empty"),
            Self::MaxPointsTooSmall { max_points } => {
                write!(formatter, "max_points must be at least 2, got {max_points}")
            }
        }
    }
}

impl std::error::Error for MetricQueryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_ranges_select_their_axis_and_reject_empty_bounds() {
        let query = MetricQuery::new(
            MetricRange::RelativeTime {
                start: RelativeTime::from_millis(10),
                end: RelativeTime::from_millis(20),
            },
            Some(500),
        )
        .expect("typed range should be valid");

        assert_eq!(query.axis(), MetricAxis::RelativeTime);
        assert_eq!(query.max_points(), Some(500));
        assert_eq!(
            MetricQuery::new(
                MetricRange::Steps {
                    start: Step::new(4),
                    end: Step::new(4),
                },
                None,
            ),
            Err(MetricQueryError::EmptyRange)
        );
    }

    #[test]
    fn public_point_limit_is_strictly_validated_without_pixels() {
        assert_eq!(
            MetricQuery::new(MetricRange::All(MetricAxis::Timestamp), Some(1)),
            Err(MetricQueryError::MaxPointsTooSmall { max_points: 1 })
        );
        assert!(MetricQuery::new(MetricRange::All(MetricAxis::Step), Some(2)).is_ok());
    }
}
