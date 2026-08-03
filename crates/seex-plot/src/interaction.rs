use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::{
    AxisRange, CanvasSize, ChartError, DataPoint, LinearScale, Series, SeriesId, Viewport,
    build_path,
};

/// One point in top-left-origin screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

impl ScreenPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PathCacheKey {
    revision: u64,
    viewport_bits: [u64; 4],
    canvas_bits: [u64; 2],
}

/// Reuses projected paths until their series, viewport, or canvas changes.
#[derive(Debug, Default)]
pub struct PathCache {
    entries: HashMap<SeriesId, (PathCacheKey, Arc<[ScreenPoint]>)>,
}

impl PathCache {
    /// Returns the cached path or projects it from the current series.
    ///
    /// Only the latest key for each series is retained, bounding stale paths
    /// created during continuous pan and zoom.
    ///
    /// # Errors
    ///
    /// Returns an error when path construction fails.
    pub fn path_for(
        &mut self,
        series: &Series,
        revision: u64,
        viewport: Viewport,
        canvas: CanvasSize,
    ) -> Result<Arc<[ScreenPoint]>, ChartError> {
        let key = PathCacheKey {
            revision,
            viewport_bits: [
                viewport.x.start().to_bits(),
                viewport.x.end().to_bits(),
                viewport.y.start().to_bits(),
                viewport.y.end().to_bits(),
            ],
            canvas_bits: [canvas.width().to_bits(), canvas.height().to_bits()],
        };
        if let Some((cached_key, path)) = self.entries.get(series.id())
            && cached_key == &key
        {
            return Ok(Arc::clone(path));
        }

        let path: Arc<[ScreenPoint]> = build_path(series, viewport, canvas)?.into();
        self.entries
            .insert(series.id().clone(), (key, Arc::clone(&path)));
        Ok(path)
    }

    pub fn invalidate_series(&mut self, series_id: &SeriesId) {
        self.entries.remove(series_id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Nearest location on one polyline segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub segment_index: usize,
    pub position: ScreenPoint,
    pub distance: f64,
}

/// Finds the closest polyline segment within a screen-space radius.
pub fn hit_test(path: &[ScreenPoint], cursor: ScreenPoint, radius: f64) -> Option<Hit> {
    if path.is_empty() || !radius.is_finite() || radius < 0.0 {
        return None;
    }
    if path.len() == 1 {
        let distance = distance(path[0], cursor);
        return (distance <= radius).then_some(Hit {
            segment_index: 0,
            position: path[0],
            distance,
        });
    }

    path.windows(2)
        .enumerate()
        .map(|(segment_index, segment)| {
            let position = closest_point(segment[0], segment[1], cursor);
            Hit {
                segment_index,
                position,
                distance: distance(position, cursor),
            }
        })
        .filter(|hit| hit.distance <= radius)
        .min_by(|left, right| left.distance.total_cmp(&right.distance))
}

/// Nearest rendered real point, identified by its index in the source series.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointHit {
    pub point_index: usize,
    pub position: ScreenPoint,
    pub distance: f64,
}

/// Finds the closest real series point within a screen-space radius.
///
/// Only points inside the viewport's closed x range are candidates. Equal
/// screen distances are resolved by the lower source-series point index.
///
/// # Errors
///
/// Returns an error when either projection scale is invalid.
pub fn hit_test_point(
    series: &Series,
    viewport: Viewport,
    canvas: CanvasSize,
    cursor: ScreenPoint,
    radius: f64,
) -> Result<Option<PointHit>, ChartError> {
    if !cursor.x.is_finite() || !cursor.y.is_finite() || !radius.is_finite() || radius < 0.0 {
        return Ok(None);
    }

    let points = series.points();
    let first = points.partition_point(|point| point.x < viewport.x.start());
    let after = points.partition_point(|point| point.x <= viewport.x.end());
    let x_scale = LinearScale::new(viewport.x, 0.0, canvas.width())?;
    let y_scale = LinearScale::new(viewport.y, canvas.height(), 0.0)?;

    Ok(points[first..after]
        .iter()
        .enumerate()
        .map(|(offset, point)| {
            let position = ScreenPoint::new(x_scale.map(point.x), y_scale.map(point.y));
            PointHit {
                point_index: first + offset,
                position,
                distance: distance(position, cursor),
            }
        })
        .filter(|hit| hit.distance <= radius)
        .min_by(|left, right| {
            left.distance
                .total_cmp(&right.distance)
                .then_with(|| left.point_index.cmp(&right.point_index))
        }))
}

fn closest_point(start: ScreenPoint, end: ScreenPoint, point: ScreenPoint) -> ScreenPoint {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return start;
    }
    let projection =
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0);
    ScreenPoint::new(start.x + projection * dx, start.y + projection * dy)
}

fn distance(left: ScreenPoint, right: ScreenPoint) -> f64 {
    (left.x - right.x).hypot(left.y - right.y)
}

/// Renderer-independent hover and multi-series selection state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectionState {
    selected: BTreeSet<SeriesId>,
    hovered: Option<SeriesId>,
}

impl SelectionState {
    pub fn selected(&self) -> &BTreeSet<SeriesId> {
        &self.selected
    }

    pub fn hovered(&self) -> Option<&SeriesId> {
        self.hovered.as_ref()
    }

    pub fn select_only(&mut self, series_id: SeriesId) {
        self.selected.clear();
        self.selected.insert(series_id);
    }

    pub fn toggle(&mut self, series_id: SeriesId) -> bool {
        if self.selected.remove(&series_id) {
            false
        } else {
            self.selected.insert(series_id);
            true
        }
    }

    pub fn set_hovered(&mut self, series_id: Option<SeriesId>) {
        self.hovered = series_id;
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.hovered = None;
    }
}

const MIN_BRUSH_SPAN: f64 = 1.0;

fn maximum_start_for_minimum_span(end: f64) -> f64 {
    let start = end - MIN_BRUSH_SPAN;
    if end - start >= MIN_BRUSH_SPAN {
        start
    } else {
        end.next_down()
    }
}

fn minimum_end_for_minimum_span(start: f64) -> f64 {
    let end = start + MIN_BRUSH_SPAN;
    if end - start >= MIN_BRUSH_SPAN {
        end
    } else {
        start.next_up()
    }
}

/// Canonical horizontal viewport constrained to a fixed home range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushState {
    home: AxisRange,
    selected: AxisRange,
}

impl BrushState {
    /// Creates a brush selecting its complete home range.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidBrushRange`] when the home range is
    /// narrower than one axis unit.
    pub fn new(home: AxisRange) -> Result<Self, ChartError> {
        if home.span() < MIN_BRUSH_SPAN {
            return Err(ChartError::InvalidBrushRange);
        }
        Ok(Self {
            home,
            selected: home,
        })
    }

    pub const fn home(self) -> AxisRange {
        self.home
    }

    pub const fn selected(self) -> AxisRange {
        self.selected
    }

    /// Moves the selection's left handle, clamped before its right handle.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidTransform`] for a non-finite position.
    pub fn resize_start(&mut self, position: f64) -> Result<(), ChartError> {
        if !position.is_finite() {
            return Err(ChartError::InvalidTransform);
        }
        let start = position.clamp(
            self.home.start(),
            maximum_start_for_minimum_span(self.selected.end()),
        );
        self.selected = AxisRange::new(start, self.selected.end())?;
        Ok(())
    }

    /// Moves the selection's right handle, clamped after its left handle.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidTransform`] for a non-finite position.
    pub fn resize_end(&mut self, position: f64) -> Result<(), ChartError> {
        if !position.is_finite() {
            return Err(ChartError::InvalidTransform);
        }
        let end = position.clamp(
            minimum_end_for_minimum_span(self.selected.start()),
            self.home.end(),
        );
        self.selected = AxisRange::new(self.selected.start(), end)?;
        Ok(())
    }

    /// Pans the selected range without changing its width.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidTransform`] for a non-finite delta.
    pub fn pan_by(&mut self, delta: f64) -> Result<(), ChartError> {
        if !delta.is_finite() {
            return Err(ChartError::InvalidTransform);
        }
        let minimum_delta = self.home.start() - self.selected.start();
        let maximum_delta = self.home.end() - self.selected.end();
        let delta = delta.clamp(minimum_delta, maximum_delta);
        let span = self.selected.span();
        let (mut start, mut end) = if delta == minimum_delta {
            (self.home.start(), self.home.start() + span)
        } else if delta == maximum_delta {
            (self.home.end() - span, self.home.end())
        } else {
            (self.selected.start() + delta, self.selected.end() + delta)
        };
        if end - start < MIN_BRUSH_SPAN {
            if delta.is_sign_negative() {
                end = minimum_end_for_minimum_span(start).min(self.home.end());
            } else {
                start = maximum_start_for_minimum_span(end).max(self.home.start());
            }
        }
        self.selected = AxisRange::new(start, end)?;
        Ok(())
    }

    /// Zooms around an anchor inside the selected range.
    ///
    /// A factor above one zooms in; a factor below one zooms out. The resulting
    /// range is clamped between one axis unit and the complete home range.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidTransform`] for an invalid factor or an
    /// anchor outside the selected range.
    pub fn zoom_at(&mut self, anchor: f64, factor: f64) -> Result<(), ChartError> {
        if !anchor.is_finite()
            || !factor.is_finite()
            || factor <= 0.0
            || anchor < self.selected.start()
            || anchor > self.selected.end()
        {
            return Err(ChartError::InvalidTransform);
        }
        if factor == 1.0 {
            return Ok(());
        }

        let span = (self.selected.span() / factor).clamp(MIN_BRUSH_SPAN, self.home.span());
        if span == self.home.span() {
            self.selected = self.home;
            return Ok(());
        }
        let anchor_ratio = (anchor - self.selected.start()) / self.selected.span();
        let latest_start =
            (self.home.end() - span).min(maximum_start_for_minimum_span(self.home.end()));
        let start = (anchor - anchor_ratio * span).clamp(self.home.start(), latest_start);
        let end = (start + span)
            .max(minimum_end_for_minimum_span(start))
            .min(self.home.end());
        self.selected = AxisRange::new(start, end)?;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.selected = self.home;
    }
}

/// Current viewport with pan, anchor zoom, and reset behavior.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomState {
    home: Viewport,
    current: Viewport,
}

impl ZoomState {
    pub const fn new(home: Viewport) -> Self {
        Self {
            home,
            current: home,
        }
    }

    pub const fn viewport(self) -> Viewport {
        self.current
    }

    /// Pans the current viewport in data coordinates.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidTransform`] for non-finite deltas.
    pub fn pan_by(&mut self, x_delta: f64, y_delta: f64) -> Result<(), ChartError> {
        if !x_delta.is_finite() || !y_delta.is_finite() {
            return Err(ChartError::InvalidTransform);
        }
        self.current = Viewport::new(
            AxisRange::new(
                self.current.x.start() + x_delta,
                self.current.x.end() + x_delta,
            )?,
            AxisRange::new(
                self.current.y.start() + y_delta,
                self.current.y.end() + y_delta,
            )?,
        );
        Ok(())
    }

    /// Zooms both axes around a data-coordinate anchor.
    ///
    /// A factor above one zooms in; a factor below one zooms out.
    ///
    /// # Errors
    ///
    /// Returns [`ChartError::InvalidTransform`] for invalid inputs.
    pub fn zoom_at(&mut self, anchor: DataPoint, factor: f64) -> Result<(), ChartError> {
        if !anchor.x.is_finite() || !anchor.y.is_finite() || !factor.is_finite() || factor <= 0.0 {
            return Err(ChartError::InvalidTransform);
        }
        self.current = Viewport::new(
            zoom_range(self.current.x, anchor.x, factor)?,
            zoom_range(self.current.y, anchor.y, factor)?,
        );
        Ok(())
    }

    pub fn reset(&mut self) {
        self.current = self.home;
    }
}

fn zoom_range(range: AxisRange, anchor: f64, factor: f64) -> Result<AxisRange, ChartError> {
    AxisRange::new(
        anchor - (anchor - range.start()) / factor,
        anchor + (range.end() - anchor) / factor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: f64, end: f64) -> AxisRange {
        AxisRange::new(start, end).expect("test range should be valid")
    }

    fn series() -> Series {
        Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![DataPoint::new(0.0, 0.0), DataPoint::new(10.0, 10.0)],
        )
        .expect("test series should be valid")
    }

    #[test]
    fn path_cache_reuses_matching_paths_and_invalidates_by_series() {
        let series = series();
        let viewport = Viewport::new(range(0.0, 10.0), range(0.0, 10.0));
        let canvas = CanvasSize::new(100.0, 100.0).expect("test canvas should be valid");
        let mut cache = PathCache::default();

        let first = cache
            .path_for(&series, 1, viewport, canvas)
            .expect("path should build");
        let second = cache
            .path_for(&series, 1, viewport, canvas)
            .expect("path should build");

        assert!(Arc::ptr_eq(&first, &second));
        let replacement = cache
            .path_for(&series, 2, viewport, canvas)
            .expect("path should build");
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(cache.len(), 1);
        cache.invalidate_series(series.id());
        assert!(cache.is_empty());
    }

    #[test]
    fn hit_test_projects_cursor_onto_nearest_segment() {
        let path = [ScreenPoint::new(0.0, 0.0), ScreenPoint::new(10.0, 0.0)];

        assert_eq!(
            hit_test(&path, ScreenPoint::new(4.0, 3.0), 3.0),
            Some(Hit {
                segment_index: 0,
                position: ScreenPoint::new(4.0, 0.0),
                distance: 3.0,
            })
        );
    }

    #[test]
    fn point_hit_returns_a_real_point_and_resolves_distance_ties_by_index() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![DataPoint::new(0.0, 0.0), DataPoint::new(10.0, 0.0)],
        )
        .expect("test series should be valid");
        let viewport = Viewport::new(range(0.0, 10.0), range(-1.0, 1.0));
        let canvas = CanvasSize::new(10.0, 10.0).expect("test canvas should be valid");

        assert_eq!(
            hit_test_point(&series, viewport, canvas, ScreenPoint::new(5.0, 5.0), 5.0)
                .expect("hit test should succeed"),
            Some(PointHit {
                point_index: 0,
                position: ScreenPoint::new(0.0, 5.0),
                distance: 5.0,
            })
        );
    }

    #[test]
    fn point_hit_preserves_source_indexes_for_repeated_x_values() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![
                DataPoint::new(0.0, 0.0),
                DataPoint::new(1.0, 0.0),
                DataPoint::new(1.0, 0.0),
                DataPoint::new(2.0, 0.0),
            ],
        )
        .expect("test series should be valid");
        let viewport = Viewport::new(range(1.0, 2.0), range(-1.0, 1.0));
        let canvas = CanvasSize::new(100.0, 10.0).expect("test canvas should be valid");

        let hit = hit_test_point(&series, viewport, canvas, ScreenPoint::new(0.0, 5.0), 0.0)
            .expect("hit test should succeed")
            .expect("point should be hit");

        assert_eq!(hit.point_index, 1);
    }

    #[test]
    fn point_hit_excludes_viewport_neighbors() {
        let series = Series::new(
            SeriesId::new("loss").expect("test id should be valid"),
            vec![DataPoint::new(0.99, 0.0), DataPoint::new(1.0, 0.0)],
        )
        .expect("test series should be valid");
        let viewport = Viewport::new(range(1.0, 2.0), range(-1.0, 1.0));
        let canvas = CanvasSize::new(100.0, 10.0).expect("test canvas should be valid");

        assert_eq!(
            hit_test_point(&series, viewport, canvas, ScreenPoint::new(-1.0, 5.0), 0.1),
            Ok(None)
        );
    }

    #[test]
    fn point_hit_ignores_invalid_inputs() {
        let series = series();
        let viewport = Viewport::new(range(0.0, 10.0), range(0.0, 10.0));
        let canvas = CanvasSize::new(100.0, 10.0).expect("test canvas should be valid");

        assert_eq!(
            hit_test_point(
                &series,
                viewport,
                canvas,
                ScreenPoint::new(f64::NAN, 5.0),
                8.0,
            ),
            Ok(None)
        );
        assert_eq!(
            hit_test_point(&series, viewport, canvas, ScreenPoint::new(0.0, 5.0), -1.0),
            Ok(None)
        );
    }

    #[test]
    fn point_hit_returns_none_for_empty_input() {
        let empty = Series::new(
            SeriesId::new("empty").expect("test id should be valid"),
            Vec::new(),
        )
        .expect("test series should be valid");
        let viewport = Viewport::new(range(0.0, 10.0), range(0.0, 10.0));
        let canvas = CanvasSize::new(100.0, 10.0).expect("test canvas should be valid");

        assert_eq!(
            hit_test_point(&empty, viewport, canvas, ScreenPoint::new(0.0, 5.0), 8.0),
            Ok(None)
        );
    }

    #[test]
    fn selection_state_toggles_independent_series() {
        let mut selection = SelectionState::default();
        let loss = SeriesId::new("loss").expect("test id should be valid");
        let accuracy = SeriesId::new("accuracy").expect("test id should be valid");

        assert!(selection.toggle(loss.clone()));
        assert!(selection.toggle(accuracy));
        assert!(!selection.toggle(loss));
        assert_eq!(selection.selected().len(), 1);
    }

    #[test]
    fn brush_rejects_a_home_range_narrower_than_one_axis_unit() {
        assert_eq!(
            BrushState::new(range(0.0, 0.5)),
            Err(ChartError::InvalidBrushRange)
        );
        assert!(BrushState::new(range(0.0, 1.0)).is_ok());
    }

    #[test]
    fn brush_resizes_handles_without_crossing_or_leaving_home() {
        let mut brush = BrushState::new(range(0.0, 10.0)).expect("brush should be valid");

        brush.resize_start(4.0).expect("resize should succeed");
        brush.resize_end(8.0).expect("resize should succeed");
        brush.resize_start(20.0).expect("resize should clamp");
        assert_eq!(brush.selected(), range(7.0, 8.0));
        brush.resize_start(-20.0).expect("resize should clamp");
        brush.resize_end(20.0).expect("resize should clamp");
        assert_eq!(brush.selected(), brush.home());
    }

    #[test]
    fn brush_pan_preserves_width_and_clamps_to_home() {
        let mut brush = BrushState::new(range(0.0, 10.0)).expect("brush should be valid");
        brush.resize_start(2.0).expect("resize should succeed");
        brush.resize_end(6.0).expect("resize should succeed");

        brush.pan_by(20.0).expect("pan should clamp");
        assert_eq!(brush.selected(), range(6.0, 10.0));
        brush.pan_by(-20.0).expect("pan should clamp");
        assert_eq!(brush.selected(), range(0.0, 4.0));

        let mut fractional = BrushState::new(range(0.0, 50.0)).expect("brush should be valid");
        fractional.resize_end(31.7).expect("resize should succeed");
        for _ in 0..1_000 {
            fractional.pan_by(0.1).expect("pan should succeed");
        }
        assert_eq!(fractional.selected().end(), fractional.home().end());
    }

    #[test]
    fn brush_zooms_around_anchor_clamps_and_resets() {
        let mut brush = BrushState::new(range(0.0, 10.0)).expect("brush should be valid");

        brush.zoom_at(2.0, 2.0).expect("zoom should succeed");
        assert_eq!(brush.selected(), range(1.0, 6.0));
        brush.zoom_at(1.0, f64::MAX).expect("zoom should clamp");
        assert_eq!(brush.selected(), range(1.0, 2.0));
        brush
            .zoom_at(1.5, f64::MIN_POSITIVE)
            .expect("zoom should clamp");
        assert_eq!(brush.selected(), brush.home());
        brush.resize_start(4.0).expect("resize should succeed");
        brush.reset();
        assert_eq!(brush.selected(), brush.home());
    }

    #[test]
    fn brush_rejects_invalid_transforms_without_mutation() {
        let mut brush = BrushState::new(range(0.0, 10.0)).expect("brush should be valid");
        let initial = brush;

        assert_eq!(
            brush.resize_start(f64::NAN),
            Err(ChartError::InvalidTransform)
        );
        assert_eq!(
            brush.pan_by(f64::INFINITY),
            Err(ChartError::InvalidTransform)
        );
        assert_eq!(brush.zoom_at(20.0, 2.0), Err(ChartError::InvalidTransform));
        assert_eq!(brush.zoom_at(5.0, 0.0), Err(ChartError::InvalidTransform));
        assert_eq!(brush, initial);
    }

    #[test]
    fn brush_keeps_minimum_span_representable_at_large_coordinates() {
        let start = 2_f64.powi(53);
        let home = range(start, start + 4.0);
        let mut brush = BrushState::new(home).expect("brush should be valid");

        brush.resize_start(home.end()).expect("resize should clamp");
        assert_eq!(brush.selected(), range(start + 2.0, start + 4.0));
        brush.reset();
        brush
            .zoom_at(home.start(), f64::MAX)
            .expect("zoom should clamp");
        assert_eq!(brush.selected(), range(start, start + 2.0));
    }

    #[test]
    fn brush_pan_does_not_collapse_a_narrow_range_during_rounding() {
        let end = 2_f64.powi(53);
        let mut brush = BrushState::new(range(end - 4.0, end)).expect("brush should be valid");
        brush
            .resize_start(end - 3.0)
            .expect("resize should succeed");
        brush.resize_end(end - 2.0).expect("resize should succeed");

        brush.pan_by(0.5).expect("pan should remain valid");

        assert!(brush.selected().span() >= MIN_BRUSH_SPAN);
    }

    #[test]
    fn zoom_state_pans_zooms_around_anchor_and_resets() {
        let home = Viewport::new(range(0.0, 10.0), range(0.0, 20.0));
        let mut zoom = ZoomState::new(home);

        zoom.zoom_at(DataPoint::new(5.0, 10.0), 2.0)
            .expect("zoom should be valid");
        assert_eq!(zoom.viewport().x, range(2.5, 7.5));
        zoom.pan_by(1.0, -2.0).expect("pan should be valid");
        assert_eq!(zoom.viewport().y, range(3.0, 13.0));
        zoom.reset();
        assert_eq!(zoom.viewport(), home);
    }
}
