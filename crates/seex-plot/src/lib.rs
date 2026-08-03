//! Renderer-agnostic models and geometry for Seex metric charts.

#![forbid(unsafe_code)]

mod interaction;

use std::error::Error;
use std::fmt;

pub use interaction::{
    BrushState, Hit, PathCache, PointHit, ScreenPoint, SelectionState, ZoomState, hit_test,
    hit_test_point,
};

/// Errors produced while constructing or transforming chart data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChartError {
    EmptySeriesId,
    InvalidPoint { index: usize },
    UnsortedSeries { index: usize },
    InvalidRange,
    InvalidOutputRange,
    InvalidCanvasSize,
    InvalidBrushRange,
    InvalidTransform,
}

impl fmt::Display for ChartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySeriesId => formatter.write_str("series id must not be empty"),
            Self::InvalidPoint { index } => {
                write!(formatter, "series point at index {index} is not finite")
            }
            Self::UnsortedSeries { index } => write!(
                formatter,
                "series x values must be nondecreasing (out of order at index {index})"
            ),
            Self::InvalidRange => formatter.write_str("axis range must be finite and increasing"),
            Self::InvalidOutputRange => {
                formatter.write_str("scale output range must be finite and non-empty")
            }
            Self::InvalidCanvasSize => {
                formatter.write_str("canvas dimensions must be finite and positive")
            }
            Self::InvalidBrushRange => {
                formatter.write_str("brush home range must span at least one axis unit")
            }
            Self::InvalidTransform => formatter.write_str("chart transform inputs are invalid"),
        }
    }
}

impl Error for ChartError {}

/// Stable identity for one chart series.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SeriesId(String);

impl SeriesId {
    /// Creates a non-empty series identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::EmptySeriesId`] when the value is blank.
    pub fn new(value: impl Into<String>) -> Result<Self, ChartError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ChartError::EmptySeriesId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SeriesId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One point in chart data coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DataPoint {
    pub x: f64,
    pub y: f64,
}

impl DataPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// An immutable, x-ordered chart series.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    id: SeriesId,
    points: Vec<DataPoint>,
}

impl Series {
    /// Creates a series whose finite points are ordered by x coordinate.
    ///
    /// # Errors
    ///
    /// Returns an error for non-finite coordinates or decreasing x values.
    pub fn new(id: SeriesId, points: Vec<DataPoint>) -> Result<Self, ChartError> {
        for (index, point) in points.iter().enumerate() {
            if !point.x.is_finite() || !point.y.is_finite() {
                return Err(ChartError::InvalidPoint { index });
            }
            if index > 0 && point.x < points[index - 1].x {
                return Err(ChartError::UnsortedSeries { index });
            }
        }
        Ok(Self { id, points })
    }

    pub fn id(&self) -> &SeriesId {
        &self.id
    }

    pub fn points(&self) -> &[DataPoint] {
        &self.points
    }

    /// Returns points intersecting an x range plus one neighbor on each side.
    pub fn visible_points(&self, x_range: AxisRange) -> &[DataPoint] {
        let first = self
            .points
            .partition_point(|point| point.x < x_range.start());
        let after = self
            .points
            .partition_point(|point| point.x <= x_range.end());
        if first == self.points.len() || after == 0 {
            return &[];
        }
        &self.points[first.saturating_sub(1)..after.saturating_add(1).min(self.points.len())]
    }
}

const Y_RANGE_PADDING_FRACTION: f64 = 0.05;
const MIN_CONSTANT_Y_PADDING: f64 = 1e-9;

/// Calculates a padded y range for points inside a closed x range.
///
/// Only real points satisfying `x_range.start() <= x <= x_range.end()` are
/// included, so path-connection neighbors do not affect the result. The range
/// receives 5% padding; constant values use
/// `max(abs(value) * 5%, 1e-9)` instead.
///
/// # Errors
///
/// Returns [`ChartError::InvalidRange`] when padding would produce a
/// non-finite range.
pub fn visible_y_range(
    series: &[Series],
    x_range: AxisRange,
) -> Result<Option<AxisRange>, ChartError> {
    visible_y_range_for(series, x_range)
}

/// Calculates a padded y range without requiring ownership of the series list.
///
/// # Errors
///
/// Returns [`ChartError::InvalidRange`] when padding would produce a
/// non-finite range.
pub fn visible_y_range_for<'a>(
    series: impl IntoIterator<Item = &'a Series>,
    x_range: AxisRange,
) -> Result<Option<AxisRange>, ChartError> {
    let mut bounds: Option<(f64, f64)> = None;
    for series in series {
        let points = series.points();
        let first = points.partition_point(|point| point.x < x_range.start());
        let after = points.partition_point(|point| point.x <= x_range.end());
        for point in &points[first..after] {
            bounds = Some(match bounds {
                Some((minimum, maximum)) => (minimum.min(point.y), maximum.max(point.y)),
                None => (point.y, point.y),
            });
        }
    }

    let Some((minimum, maximum)) = bounds else {
        return Ok(None);
    };
    let padding = if minimum == maximum {
        (minimum.abs() * Y_RANGE_PADDING_FRACTION).max(MIN_CONSTANT_Y_PADDING)
    } else {
        (maximum - minimum) * Y_RANGE_PADDING_FRACTION
    };
    let padded_minimum = minimum - padding;
    let padded_maximum = maximum + padding;
    let padded_minimum = if padded_minimum < minimum {
        padded_minimum
    } else {
        minimum.next_down()
    };
    let padded_maximum = if padded_maximum > maximum {
        padded_maximum
    } else {
        maximum.next_up()
    };
    AxisRange::new(padded_minimum, padded_maximum).map(Some)
}

/// A finite, increasing range on one data axis.
///
/// The fields are private so callers must use [`AxisRange::new`].
///
/// ```compile_fail
/// use seex_chart_core::AxisRange;
/// let _invalid = AxisRange { start: 1.0, end: 1.0 };
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisRange {
    start: f64,
    end: f64,
}

impl AxisRange {
    /// Creates a finite range where `start < end` and the span is finite.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidRange`] for an empty range, non-finite
    /// endpoint, or overflowing span.
    pub fn new(start: f64, end: f64) -> Result<Self, ChartError> {
        if !start.is_finite() || !end.is_finite() || start >= end || !(end - start).is_finite() {
            return Err(ChartError::InvalidRange);
        }
        Ok(Self { start, end })
    }

    pub const fn start(self) -> f64 {
        self.start
    }

    pub const fn end(self) -> f64 {
        self.end
    }

    pub const fn span(self) -> f64 {
        self.end - self.start
    }
}

/// The data-coordinate rectangle currently visible to a renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: AxisRange,
    pub y: AxisRange,
}

impl Viewport {
    pub const fn new(x: AxisRange, y: AxisRange) -> Self {
        Self { x, y }
    }
}

/// Positive pixel dimensions for a chart surface.
///
/// The fields are private so callers must use [`CanvasSize::new`].
///
/// ```compile_fail
/// use seex_chart_core::CanvasSize;
/// let _invalid = CanvasSize { width: 0.0, height: 100.0 };
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasSize {
    width: f64,
    height: f64,
}

impl CanvasSize {
    /// Creates finite, positive canvas dimensions.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidCanvasSize`] for invalid dimensions.
    pub fn new(width: f64, height: f64) -> Result<Self, ChartError> {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Err(ChartError::InvalidCanvasSize);
        }
        Ok(Self { width, height })
    }

    pub const fn width(self) -> f64 {
        self.width
    }

    pub const fn height(self) -> f64 {
        self.height
    }
}

/// Maps one data range onto an arbitrary output interval.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearScale {
    domain: AxisRange,
    output_start: f64,
    output_end: f64,
}

impl LinearScale {
    /// Creates a scale from a validated domain to a non-empty output interval.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidOutputRange`] for a non-finite, empty, or
    /// overflowing output interval.
    pub fn new(domain: AxisRange, output_start: f64, output_end: f64) -> Result<Self, ChartError> {
        if !output_start.is_finite()
            || !output_end.is_finite()
            || output_start == output_end
            || !(output_end - output_start).is_finite()
        {
            return Err(ChartError::InvalidOutputRange);
        }
        Ok(Self {
            domain,
            output_start,
            output_end,
        })
    }

    pub fn map(self, value: f64) -> f64 {
        let ratio = (value - self.domain.start()) / self.domain.span();
        self.output_start + ratio * (self.output_end - self.output_start)
    }

    pub fn invert(self, value: f64) -> f64 {
        let ratio = (value - self.output_start) / (self.output_end - self.output_start);
        self.domain.start() + ratio * self.domain.span()
    }
}

/// Generates human-scale linear tick values across an axis range.
pub fn linear_ticks(range: AxisRange, target_count: usize) -> Vec<f64> {
    if target_count == 0 {
        return Vec::new();
    }
    let target_count = target_count.min(1_024);
    let raw_step = range.span() / target_count as f64;
    let magnitude = 10_f64.powf(raw_step.log10().floor());
    let normalized = raw_step / magnitude;
    let factor = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    let step = factor * magnitude;
    if !step.is_finite() || step <= 0.0 {
        return vec![range.start(), range.end()];
    }

    let mut value = (range.start() / step).ceil() * step;
    let mut ticks = Vec::new();
    while value <= range.end() + step * 1e-10 && ticks.len() <= target_count + 2 {
        ticks.push(if value == -0.0 { 0.0 } else { value });
        value += step;
    }
    ticks
}

/// Projects the visible series path into top-left-origin screen coordinates.
///
/// # Errors
///
/// Returns an error when either scale is invalid.
pub fn build_path(
    series: &Series,
    viewport: Viewport,
    canvas: CanvasSize,
) -> Result<Vec<ScreenPoint>, ChartError> {
    let visible = series.visible_points(viewport.x);
    let x_scale = LinearScale::new(viewport.x, 0.0, canvas.width())?;
    let y_scale = LinearScale::new(viewport.y, canvas.height(), 0.0)?;
    Ok(visible
        .iter()
        .map(|point| ScreenPoint::new(x_scale.map(point.x), y_scale.map(point.y)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: f64, end: f64) -> AxisRange {
        AxisRange::new(start, end).expect("test range should be valid")
    }

    #[test]
    fn series_rejects_non_finite_and_unsorted_points() {
        let id = SeriesId::new("loss").expect("test id should be valid");
        let invalid = Series::new(id.clone(), vec![DataPoint::new(0.0, f64::NAN)]);
        let unsorted = Series::new(id, vec![DataPoint::new(2.0, 1.0), DataPoint::new(1.0, 2.0)]);

        assert_eq!(invalid, Err(ChartError::InvalidPoint { index: 0 }));
        assert_eq!(unsorted, Err(ChartError::UnsortedSeries { index: 1 }));
    }

    #[test]
    fn visible_points_keeps_boundary_neighbors_for_connected_paths() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            (0..=5)
                .map(|value| DataPoint::new(f64::from(value), f64::from(value)))
                .collect(),
        )
        .expect("test series should be valid");

        assert_eq!(
            series.visible_points(range(2.2, 3.2)),
            &series.points()[2..=4]
        );
    }

    #[test]
    fn visible_y_range_combines_series_and_excludes_neighbors() {
        let loss = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![
                DataPoint::new(0.0, -100.0),
                DataPoint::new(1.0, 0.0),
                DataPoint::new(2.0, 10.0),
                DataPoint::new(3.0, 100.0),
            ],
        )
        .expect("test series should be valid");
        let accuracy = Series::new(
            SeriesId::new("accuracy").expect("test id should be valid"),
            vec![DataPoint::new(1.0, -10.0), DataPoint::new(2.0, 5.0)],
        )
        .expect("test series should be valid");

        assert_eq!(
            visible_y_range(&[loss, accuracy], range(1.0, 2.0)),
            Ok(Some(range(-11.0, 11.0)))
        );
    }

    #[test]
    fn visible_y_range_pads_constant_values_by_magnitude() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![DataPoint::new(1.0, -20.0), DataPoint::new(2.0, -20.0)],
        )
        .expect("test series should be valid");

        assert_eq!(
            visible_y_range(&[series], range(1.0, 2.0)),
            Ok(Some(range(-21.0, -19.0)))
        );
    }

    #[test]
    fn visible_y_range_uses_a_floor_when_constant_value_is_zero() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![DataPoint::new(1.0, 0.0)],
        )
        .expect("test series should be valid");

        assert_eq!(
            visible_y_range(&[series], range(1.0, 2.0)),
            Ok(Some(range(-1e-9, 1e-9)))
        );
    }

    #[test]
    fn visible_y_range_returns_none_without_points_in_range() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![DataPoint::new(0.0, 1.0)],
        )
        .expect("test series should be valid");

        assert_eq!(visible_y_range(&[], range(1.0, 2.0)), Ok(None));
        assert_eq!(visible_y_range(&[series], range(1.0, 2.0)), Ok(None));
    }

    #[test]
    fn visible_y_range_rejects_non_finite_padded_bounds() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![DataPoint::new(1.0, f64::MAX)],
        )
        .expect("test series should be valid");

        assert_eq!(
            visible_y_range(&[series], range(1.0, 2.0)),
            Err(ChartError::InvalidRange)
        );
    }

    #[test]
    fn visible_y_range_rounds_small_padding_outward_at_large_offsets() {
        let minimum = 1e16;
        let maximum = minimum + 2.0;
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![DataPoint::new(1.0, minimum), DataPoint::new(2.0, maximum)],
        )
        .expect("test series should be valid");

        assert_eq!(
            visible_y_range(&[series], range(1.0, 2.0)),
            Ok(Some(range(minimum.next_down(), maximum.next_up())))
        );
    }

    #[test]
    fn linear_scale_maps_and_inverts_reversed_output() {
        let scale =
            LinearScale::new(range(0.0, 10.0), 100.0, 0.0).expect("test scale should be valid");

        assert_eq!(scale.map(2.5), 75.0);
        assert_eq!(scale.invert(75.0), 2.5);
    }

    #[test]
    fn axis_range_rejects_overflowing_span() {
        assert_eq!(
            AxisRange::new(-f64::MAX, f64::MAX),
            Err(ChartError::InvalidRange)
        );
    }

    #[test]
    fn linear_scale_rejects_overflowing_output_span() {
        assert_eq!(
            LinearScale::new(range(0.0, 1.0), -f64::MAX, f64::MAX),
            Err(ChartError::InvalidOutputRange)
        );
    }

    #[test]
    fn linear_scale_maps_accepted_extreme_endpoints_to_finite_values() {
        let start = -f64::MAX / 2.0;
        let end = f64::MAX / 2.0;
        let scale = LinearScale::new(range(start, end), start, end)
            .expect("finite spans should produce a valid scale");

        for value in [
            scale.map(start),
            scale.map(end),
            scale.invert(start),
            scale.invert(end),
        ] {
            assert!(value.is_finite(), "expected a finite endpoint, got {value}");
        }
    }

    #[test]
    fn linear_ticks_uses_nice_steps() {
        assert_eq!(linear_ticks(range(0.3, 9.1), 5), vec![2.0, 4.0, 6.0, 8.0]);
    }

    #[test]
    fn invariant_types_reject_invalid_dimensions() {
        assert_eq!(AxisRange::new(1.0, 1.0), Err(ChartError::InvalidRange));
        assert_eq!(
            CanvasSize::new(0.0, 100.0),
            Err(ChartError::InvalidCanvasSize)
        );
    }

    #[test]
    fn build_path_inverts_the_y_axis() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![DataPoint::new(0.0, 0.0), DataPoint::new(10.0, 10.0)],
        )
        .expect("test series should be valid");
        let viewport = Viewport::new(range(0.0, 10.0), range(0.0, 10.0));
        let canvas = CanvasSize::new(200.0, 100.0).expect("test canvas should be valid");

        assert_eq!(
            build_path(&series, viewport, canvas).expect("path should build"),
            vec![ScreenPoint::new(0.0, 100.0), ScreenPoint::new(200.0, 0.0)]
        );
    }

    #[test]
    fn build_path_projects_every_visible_point() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            (0..100)
                .map(|value| DataPoint::new(f64::from(value), f64::from(value)))
                .collect(),
        )
        .expect("test series should be valid");
        let viewport = Viewport::new(range(0.0, 99.0), range(0.0, 99.0));
        let canvas = CanvasSize::new(200.0, 100.0).expect("test canvas should be valid");

        let path = build_path(&series, viewport, canvas).expect("path should build");

        assert_eq!(path.len(), series.points().len());
    }
}
