use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use gpui::{Bounds, Path, Pixels, Point, point, px};
use seex_plot::{AxisRange, LinearScale, ScreenPoint, Viewport, visible_y_range_for};

use crate::data::query::CurveSnapshot;
use crate::domain::RunRef;

pub(super) fn projected_point(bounds: Bounds<Pixels>, projected: ScreenPoint) -> Point<Pixels> {
    point(
        bounds.origin.x + px(projected.x as f32),
        bounds.origin.y + px(projected.y as f32),
    )
}

pub(super) fn compact_render_points(points: &[ScreenPoint], bucket_width: f64) -> Vec<ScreenPoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let bucket_capacity = points.last().map_or(2, |point| {
        (point.x.max(0.) / bucket_width).ceil() as usize * 2 + 2
    });
    let mut compact = Vec::with_capacity(points.len().min(bucket_capacity));
    compact.push(points[0]);
    let mut start = 0;
    while start < points.len() {
        let bucket = (points[start].x / bucket_width).floor();
        let mut end = start + 1;
        while end < points.len() && (points[end].x / bucket_width).floor() == bucket {
            end += 1;
        }
        let mut min = start;
        let mut max = start;
        for index in start + 1..end {
            if points[index].y < points[min].y {
                min = index;
            }
            if points[index].y > points[max].y {
                max = index;
            }
        }
        for index in [min.min(max), min.max(max)] {
            if compact.last() != Some(&points[index]) {
                compact.push(points[index]);
            }
        }
        start = end;
    }
    if compact.last() != points.last() {
        compact.push(points[points.len() - 1]);
    }
    compact
}

pub(super) fn solid_polyline_path(
    points: &[ScreenPoint],
    bounds: Bounds<Pixels>,
    width: Pixels,
) -> Option<Path<Pixels>> {
    let half_width = f32::from(width) / 2.;
    let mut path = None;
    let mut previous_join = None;
    let texture = (point(0., 1.), point(0., 1.), point(0., 1.));
    for segment in points.windows(2) {
        let start = projected_point(bounds, segment[0]);
        let end = projected_point(bounds, segment[1]);
        let dx = f32::from(end.x - start.x);
        let dy = f32::from(end.y - start.y);
        let length = dx.hypot(dy);
        if length <= f32::EPSILON {
            continue;
        }
        let normal = point(px(-dy / length * half_width), px(dx / length * half_width));
        let start_positive = start + normal;
        let start_negative = start - normal;
        let end_positive = end + normal;
        let end_negative = end - normal;
        let path = path.get_or_insert_with(|| Path::new(start));
        if let Some((previous_positive, previous_negative)) = previous_join {
            path.push_triangle((previous_positive, start, start_positive), texture);
            path.push_triangle((previous_negative, start_negative, start), texture);
        }
        path.push_triangle((start_positive, start_negative, end_positive), texture);
        path.push_triangle((end_positive, start_negative, end_negative), texture);
        previous_join = Some((end_positive, end_negative));
    }
    path
}

pub(super) fn axis_at(
    bounds: Bounds<Pixels>,
    range: AxisRange,
    cursor: Point<Pixels>,
) -> Option<f64> {
    if cursor.x < bounds.origin.x || cursor.x > bounds.origin.x + bounds.size.width {
        return None;
    }
    LinearScale::new(
        range,
        f64::from(bounds.origin.x),
        f64::from(bounds.origin.x + bounds.size.width),
    )
    .ok()
    .map(|scale| scale.invert(f64::from(cursor.x)))
}

pub(in crate::desktop::app) fn detail_viewport(
    snapshot: &CurveSnapshot,
    selected: Option<AxisRange>,
    visible_runs: Option<&[RunRef]>,
) -> Option<Viewport> {
    let snapshot_x = AxisRange::new(
        snapshot.viewport.start() as f64,
        snapshot.viewport.end() as f64,
    )
    .ok()?;
    let x = selected
        .filter(|selected| {
            snapshot_x.start() <= selected.start() && snapshot_x.end() >= selected.end()
        })
        .unwrap_or(snapshot_x);
    let y_range = |range| {
        visible_y_range_for(
            snapshot
                .series
                .iter()
                .filter(|curve| visible_runs.is_none_or(|runs| runs.contains(&curve.run_ref)))
                .filter_map(|curve| curve.chart_series.as_ref()),
            range,
        )
        .ok()
        .flatten()
    };
    y_range(x)
        .or_else(|| (x != snapshot_x).then(|| y_range(snapshot_x)).flatten())
        .map(|y| Viewport::new(x, y))
}

pub(super) fn overview_viewport(
    snapshot: &CurveSnapshot,
    home: AxisRange,
    visible_runs: &[RunRef],
) -> Option<Viewport> {
    detail_viewport(snapshot, None, Some(visible_runs))
        .map(|viewport| Viewport::new(home, viewport.y))
}

pub(in crate::desktop::app) fn series_color_index(run_ref: &RunRef) -> usize {
    let mut hasher = DefaultHasher::new();
    run_ref.hash(&mut hasher);
    hasher.finish() as usize
}
