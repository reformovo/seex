use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use gpui::Styled;
use gpui::{
    AnyView, Bounds, ContentMask, Context, Entity, IntoElement, Path, PathBuilder, Pixels, Point,
    Render, Rgba, StyleRefinement, Window, WindowAppearance, canvas, fill, point, px, size,
};
use pulseon_chart_core::{
    AxisRange, BrushState, CanvasSize, LinearScale, PathCache, ScreenPoint, Viewport,
    hit_test_point, visible_y_range_for,
};
use pulseon_model::comparison::EvidenceCompleteness;
use pulseon_viewer::core::RunRef;
use pulseon_viewer::query::CurveSnapshot;

use super::theme::ViewerTheme;

#[derive(Clone, Debug)]
pub struct HoverPoint {
    pub run_ref: RunRef,
    pub run_name: String,
    pub metric_key: String,
    pub axis_value: i64,
    pub value: f64,
    pub canvas_position: Point<Pixels>,
    pub align_left: bool,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct GpuiPathKey {
    revision: u64,
    viewport: [u64; 4],
    bounds: [u32; 4],
    dark: bool,
    partial: bool,
    highlighted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrushDragTarget {
    Start,
    End,
    Window,
}

#[derive(Default)]
pub struct ChartAdapter {
    detail_projection_cache: PathCache,
    detail_gpui_paths: HashMap<String, (GpuiPathKey, Path<Pixels>)>,
    detail_bounds: Option<Bounds<Pixels>>,
    overview_bounds: Option<Bounds<Pixels>>,
    #[cfg(all(test, feature = "test-support"))]
    detail_prepare_count: usize,
}

struct PreparedChart {
    paths: Vec<(Path<Pixels>, Rgba)>,
    theme: ViewerTheme,
}

const RENDER_BUCKET_WIDTH: f64 = 2.;
const BRUSH_HANDLE_WIDTH: f32 = 2.;

pub struct DetailChart {
    adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
    snapshot: Arc<CurveSnapshot>,
    revision: u64,
    viewport: Viewport,
    baseline: Option<RunRef>,
    visible_runs: std::rc::Rc<[RunRef]>,
}

impl DetailChart {
    pub fn new(
        adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
        snapshot: Arc<CurveSnapshot>,
        revision: u64,
        viewport: Viewport,
        baseline: Option<RunRef>,
        visible_runs: std::rc::Rc<[RunRef]>,
    ) -> Self {
        Self {
            adapter,
            snapshot,
            revision,
            viewport,
            baseline,
            visible_runs,
        }
    }

    pub fn update(
        &mut self,
        snapshot: Arc<CurveSnapshot>,
        revision: u64,
        viewport: Viewport,
        baseline: Option<RunRef>,
        visible_runs: std::rc::Rc<[RunRef]>,
    ) -> bool {
        if Arc::ptr_eq(&self.snapshot, &snapshot)
            && self.revision == revision
            && self.viewport == viewport
            && self.baseline == baseline
            && self.visible_runs == visible_runs
        {
            return false;
        }
        self.snapshot = snapshot;
        self.revision = revision;
        self.viewport = viewport;
        self.baseline = baseline;
        self.visible_runs = visible_runs;
        true
    }
}

impl Render for DetailChart {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        detail_canvas(
            std::rc::Rc::clone(&self.adapter),
            Arc::clone(&self.snapshot),
            self.revision,
            self.viewport,
            self.baseline.clone(),
            std::rc::Rc::clone(&self.visible_runs),
        )
        .size_full()
    }
}

pub fn cached_detail_chart(chart: Entity<DetailChart>) -> AnyView {
    AnyView::from(chart).cached(StyleRefinement::default().size_full())
}

#[derive(Clone, Copy)]
struct RenderRuns<'a> {
    baseline: Option<&'a RunRef>,
    visible: Option<&'a [RunRef]>,
}

impl ChartAdapter {
    pub fn clear(&mut self) {
        self.detail_projection_cache.clear();
        self.detail_gpui_paths.clear();
        self.detail_bounds = None;
        self.overview_bounds = None;
    }

    pub fn warm_projection(
        &mut self,
        snapshot: &CurveSnapshot,
        revision: u64,
        viewport: Viewport,
        canvas: CanvasSize,
        visible_runs: &[RunRef],
    ) {
        for series in snapshot
            .series
            .iter()
            .filter(|curve| visible_runs.contains(&curve.run_ref))
            .filter_map(|curve| curve.chart_series.as_ref())
        {
            let _ = self
                .detail_projection_cache
                .path_for(series, revision, viewport, canvas);
        }
    }

    fn prepare(
        &mut self,
        snapshot: &CurveSnapshot,
        revision: u64,
        viewport: Viewport,
        bounds: Bounds<Pixels>,
        appearance: WindowAppearance,
        runs: RenderRuns<'_>,
    ) -> PreparedChart {
        #[cfg(all(test, feature = "test-support"))]
        {
            self.detail_prepare_count = self.detail_prepare_count.saturating_add(1);
        }
        let theme = ViewerTheme::for_appearance(appearance);
        self.detail_bounds = Some(bounds);
        let Ok(canvas) =
            CanvasSize::new(f64::from(bounds.size.width), f64::from(bounds.size.height))
        else {
            return PreparedChart {
                paths: Vec::new(),
                theme,
            };
        };
        let mut paths = Vec::new();
        for curve in &snapshot.series {
            if runs
                .visible
                .is_some_and(|visible| !visible.contains(&curve.run_ref))
            {
                continue;
            }
            let Some(series) = curve.chart_series.as_ref() else {
                continue;
            };
            let partial = curve.evidence.completeness == EvidenceCompleteness::Partial;
            let highlighted = runs.baseline == Some(&curve.run_ref);
            let key = GpuiPathKey {
                revision,
                viewport: [
                    viewport.x.start().to_bits(),
                    viewport.x.end().to_bits(),
                    viewport.y.start().to_bits(),
                    viewport.y.end().to_bits(),
                ],
                bounds: [
                    f32::from(bounds.origin.x).to_bits(),
                    f32::from(bounds.origin.y).to_bits(),
                    f32::from(bounds.size.width).to_bits(),
                    f32::from(bounds.size.height).to_bits(),
                ],
                dark: theme.dark,
                partial,
                highlighted,
            };
            let projection_cache = &mut self.detail_projection_cache;
            let gpui_paths = &mut self.detail_gpui_paths;
            let cache_id = series.id().as_str();
            let path = if let Some((cached_key, path)) = gpui_paths.get(cache_id)
                && cached_key == &key
            {
                // PERF: GPUI consumes Path during paint, so retaining cached commands
                // requires a clone but still avoids projection and PathBuilder work.
                path.clone()
            } else {
                let Ok(points) = projection_cache.path_for(series, revision, viewport, canvas)
                else {
                    continue;
                };
                let points = compact_render_points(&points, RENDER_BUCKET_WIDTH);
                let width = if highlighted { px(3.) } else { px(2.) };
                let path = if partial {
                    let mut builder = PathBuilder::stroke(width).dash_array(&[px(7.), px(4.)]);
                    for (point_index, projected) in points.iter().enumerate() {
                        let position = projected_point(bounds, *projected);
                        if point_index == 0 {
                            builder.move_to(position);
                        } else {
                            builder.line_to(position);
                        }
                    }
                    let Ok(path) = builder.build() else {
                        continue;
                    };
                    path
                } else {
                    let Some(path) = solid_polyline_path(&points, bounds, width) else {
                        continue;
                    };
                    path
                };
                gpui_paths.insert(cache_id.to_owned(), (key, path.clone()));
                path
            };
            paths.push((
                path,
                theme
                    .colors
                    .series_color(series_color_index(&curve.run_ref)),
            ));
        }
        PreparedChart { paths, theme }
    }

    pub fn hit_test(
        &self,
        snapshot: &CurveSnapshot,
        viewport: Viewport,
        cursor: Point<Pixels>,
        visible_runs: &[RunRef],
    ) -> Option<HoverPoint> {
        let bounds = self.detail_bounds?;
        let canvas =
            CanvasSize::new(f64::from(bounds.size.width), f64::from(bounds.size.height)).ok()?;
        let local = ScreenPoint::new(
            f64::from(cursor.x - bounds.origin.x),
            f64::from(cursor.y - bounds.origin.y),
        );
        let mut nearest = None;
        for curve in &snapshot.series {
            if !visible_runs.contains(&curve.run_ref) {
                continue;
            }
            let Some(series) = curve.chart_series.as_ref() else {
                continue;
            };
            let Some(hit) = hit_test_point(series, viewport, canvas, local, 8.).ok()? else {
                continue;
            };
            if nearest
                .as_ref()
                .is_some_and(|(distance, _): &(f64, HoverPoint)| *distance <= hit.distance)
            {
                continue;
            }
            let aligned = curve.evidence.points.get(hit.point_index)?;
            nearest = Some((
                hit.distance,
                HoverPoint {
                    run_ref: curve.run_ref.clone(),
                    run_name: curve.run.name.clone(),
                    metric_key: aligned.point.metric_key.as_str().to_owned(),
                    axis_value: aligned.axis_value,
                    value: aligned.point.value_f64,
                    canvas_position: point(px(hit.position.x as f32), px(hit.position.y as f32)),
                    align_left: hit.position.x > canvas.width() * 0.72,
                },
            ));
        }
        nearest.map(|(_, point)| point)
    }

    pub fn points_at_axis(
        &self,
        snapshot: &CurveSnapshot,
        viewport: Viewport,
        axis: f64,
        visible_runs: &[RunRef],
    ) -> Vec<HoverPoint> {
        let Some(bounds) = self.detail_bounds else {
            return Vec::new();
        };
        let Ok(x_scale) = LinearScale::new(viewport.x, 0., f64::from(bounds.size.width)) else {
            return Vec::new();
        };
        let Ok(y_scale) = LinearScale::new(viewport.y, f64::from(bounds.size.height), 0.) else {
            return Vec::new();
        };
        if axis < viewport.x.start() || axis > viewport.x.end() {
            return Vec::new();
        }
        snapshot
            .series
            .iter()
            .filter(|curve| visible_runs.contains(&curve.run_ref))
            .filter_map(|curve| {
                let series = curve.chart_series.as_ref()?;
                let points = series.points();
                let after = points.partition_point(|point| point.x < axis);
                let start = after.saturating_sub(1);
                let end = after.saturating_add(1).min(points.len());
                let point_index = (start..end).min_by(|left, right| {
                    (points[*left].x - axis)
                        .abs()
                        .total_cmp(&(points[*right].x - axis).abs())
                })?;
                let aligned = curve.evidence.points.get(point_index)?;
                Some(HoverPoint {
                    run_ref: curve.run_ref.clone(),
                    run_name: curve.run.name.clone(),
                    metric_key: aligned.point.metric_key.as_str().to_owned(),
                    axis_value: aligned.axis_value,
                    value: aligned.point.value_f64,
                    canvas_position: point(
                        px(x_scale.map(axis) as f32),
                        px(y_scale.map(aligned.point.value_f64) as f32),
                    ),
                    align_left: x_scale.map(axis) > f64::from(bounds.size.width) * 0.72,
                })
            })
            .collect()
    }

    pub fn spread_callouts(&self, callouts: &mut [HoverPoint]) {
        let Some(bounds) = self.detail_bounds else {
            return;
        };
        spread_callouts(callouts, bounds.size.height);
    }

    pub fn detail_axis_at(&self, range: AxisRange, cursor: Point<Pixels>) -> Option<f64> {
        axis_at(self.detail_bounds?, range, cursor)
    }

    #[cfg(all(test, feature = "test-support"))]
    pub const fn detail_prepare_count(&self) -> usize {
        self.detail_prepare_count
    }

    pub fn detail_pointer_anchor(&self, cursor: Point<Pixels>) -> Option<(Pixels, bool)> {
        let bounds = self.detail_bounds?;
        if cursor.x < bounds.origin.x || cursor.x > bounds.right() {
            return None;
        }
        let x = cursor.x - bounds.origin.x;
        Some((x, x > bounds.size.width * 0.72))
    }

    pub fn detail_pan_delta(
        &self,
        range: AxisRange,
        previous_x: f64,
        cursor: Point<Pixels>,
    ) -> Option<f64> {
        let bounds = self.detail_bounds?;
        let current = axis_at(bounds, range, cursor)?;
        let scale = LinearScale::new(
            range,
            f64::from(bounds.origin.x),
            f64::from(bounds.origin.x + bounds.size.width),
        )
        .ok()?;
        Some(scale.invert(previous_x) - current)
    }

    pub fn overview_axis_at(&self, brush: BrushState, cursor: Point<Pixels>) -> Option<f64> {
        axis_at(self.overview_bounds?, brush.home(), cursor)
    }

    pub fn brush_target(
        &self,
        brush: BrushState,
        cursor: Point<Pixels>,
    ) -> Option<BrushDragTarget> {
        let bounds = self.overview_bounds?;
        let scale = LinearScale::new(
            brush.home(),
            f64::from(bounds.origin.x),
            f64::from(bounds.origin.x + bounds.size.width),
        )
        .ok()?;
        let x = f64::from(cursor.x);
        let start = scale.map(brush.selected().start());
        let end = scale.map(brush.selected().end());
        let start_distance = (x - start).abs();
        let end_distance = (x - end).abs();
        if start_distance <= 8. || end_distance <= 8. {
            Some(if start_distance <= end_distance {
                BrushDragTarget::Start
            } else {
                BrushDragTarget::End
            })
        } else if x > start && x < end {
            Some(BrushDragTarget::Window)
        } else {
            None
        }
    }

    pub fn overview_widths(&self, scale_factor: f32) -> Option<(f32, u32)> {
        let bounds = self.overview_bounds?;
        Some((
            f32::from(bounds.size.width),
            physical_width(Some(bounds), scale_factor)?,
        ))
    }
}

fn spread_callouts(callouts: &mut [HoverPoint], height: Pixels) {
    if callouts.len() < 2 {
        return;
    }
    let height = f32::from(height);
    let half_height = 12_f32.min(height / 2.);
    let available = (height - half_height * 2.).max(0.);
    let spacing = 24_f32.min(available / (callouts.len() - 1) as f32);
    let mut order = (0..callouts.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        f32::from(callouts[*left].canvas_position.y)
            .total_cmp(&f32::from(callouts[*right].canvas_position.y))
    });
    let mut positions = order
        .iter()
        .map(|index| {
            f32::from(callouts[*index].canvas_position.y).clamp(half_height, height - half_height)
        })
        .collect::<Vec<_>>();
    for index in 1..positions.len() {
        positions[index] = positions[index].max(positions[index - 1] + spacing);
    }
    if let Some(last) = positions.last_mut() {
        *last = (*last).min(height - half_height);
    }
    for index in (0..positions.len() - 1).rev() {
        positions[index] = positions[index].min(positions[index + 1] - spacing);
    }
    for (index, y) in order.into_iter().zip(positions) {
        callouts[index].canvas_position.y = px(y);
    }
}

fn projected_point(bounds: Bounds<Pixels>, projected: ScreenPoint) -> Point<Pixels> {
    point(
        bounds.origin.x + px(projected.x as f32),
        bounds.origin.y + px(projected.y as f32),
    )
}

fn compact_render_points(points: &[ScreenPoint], bucket_width: f64) -> Vec<ScreenPoint> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut compact = Vec::with_capacity(points.len());
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

fn solid_polyline_path(
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

fn axis_at(bounds: Bounds<Pixels>, range: AxisRange, cursor: Point<Pixels>) -> Option<f64> {
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

fn physical_width(bounds: Option<Bounds<Pixels>>, scale_factor: f32) -> Option<u32> {
    let width = f32::from(bounds?.size.width) * scale_factor;
    (width.is_finite() && width >= 1.).then(|| width.round() as u32)
}

pub fn detail_viewport(
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

pub fn detail_canvas(
    adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
    snapshot: Arc<CurveSnapshot>,
    revision: u64,
    viewport: Viewport,
    baseline: Option<RunRef>,
    visible_runs: std::rc::Rc<[RunRef]>,
) -> impl gpui::Styled + gpui::IntoElement {
    canvas(
        move |bounds, window, _| {
            let resized = adapter.borrow().detail_bounds != Some(bounds);
            let prepared = adapter.borrow_mut().prepare(
                &snapshot,
                revision,
                viewport,
                bounds,
                window.appearance(),
                RenderRuns {
                    baseline: baseline.as_ref(),
                    visible: Some(&visible_runs),
                },
            );
            if resized {
                window.request_animation_frame();
            }
            prepared
        },
        move |bounds, prepared, window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let grid = prepared.theme.colors.chart_grid;
                for index in 0..=5 {
                    let ratio = index as f32 / 5.;
                    let x = bounds.origin.x + bounds.size.width * ratio;
                    let y = bounds.origin.y + bounds.size.height * ratio;
                    window.paint_quad(fill(
                        Bounds::new(point(x, bounds.origin.y), size(px(1.), bounds.size.height)),
                        grid,
                    ));
                    window.paint_quad(fill(
                        Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, px(1.))),
                        grid,
                    ));
                }
                for (path, color) in prepared.paths {
                    window.paint_path(path, color);
                }
            });
        },
    )
}

pub fn cursor_canvas(
    adapter: Option<std::rc::Rc<std::cell::RefCell<ChartAdapter>>>,
    range: AxisRange,
    hover: Option<f64>,
    locked: Option<f64>,
) -> impl gpui::Styled + gpui::IntoElement {
    canvas(
        move |bounds, window, _| {
            if let Some(adapter) = &adapter {
                adapter.borrow_mut().detail_bounds = Some(bounds);
            }
            ViewerTheme::for_appearance(window.appearance())
        },
        move |bounds, theme, window, _| {
            let x_for = |axis: f64| {
                bounds.origin.x
                    + bounds.size.width
                        * ((axis - range.start()) / range.span()).clamp(0., 1.) as f32
            };
            if let Some(axis) = locked.filter(|axis| *axis >= range.start() && *axis <= range.end())
            {
                let x = x_for(axis);
                window.paint_quad(fill(
                    Bounds::new(point(x, bounds.origin.y), size(px(1.), bounds.size.height)),
                    theme.colors.accent,
                ));
                let mut triangle = PathBuilder::fill();
                triangle.move_to(point(x - px(5.), bounds.origin.y));
                triangle.line_to(point(x + px(5.), bounds.origin.y));
                triangle.line_to(point(x, bounds.origin.y + px(7.)));
                triangle.close();
                if let Ok(path) = triangle.build() {
                    window.paint_path(path, theme.colors.accent);
                }
            }
            if let Some(axis) = hover.filter(|axis| *axis >= range.start() && *axis <= range.end())
            {
                let x = x_for(axis);
                let mut y = bounds.origin.y;
                while y < bounds.bottom() {
                    window.paint_quad(fill(
                        Bounds::new(point(x, y), size(px(1.), px(5.))),
                        theme.colors.accent,
                    ));
                    y += px(9.);
                }
            }
        },
    )
}

pub fn callout_pointer(points_right: bool) -> impl gpui::Styled + gpui::IntoElement {
    canvas(
        move |_, window, _| ViewerTheme::for_appearance(window.appearance()),
        move |bounds, theme, window, _| {
            let mut triangle = PathBuilder::fill();
            let middle = bounds.origin.y + bounds.size.height / 2.;
            if points_right {
                triangle.move_to(bounds.origin);
                triangle.line_to(point(bounds.origin.x, bounds.bottom()));
                triangle.line_to(point(bounds.right(), middle));
            } else {
                triangle.move_to(point(bounds.right(), bounds.origin.y));
                triangle.line_to(point(bounds.right(), bounds.bottom()));
                triangle.line_to(point(bounds.origin.x, middle));
            }
            triangle.close();
            if let Ok(path) = triangle.build() {
                window.paint_path(path, theme.colors.tooltip_background);
            }
        },
    )
    .w(px(8.))
    .h(px(12.))
}

pub fn timeline_canvas(
    adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
    brush: BrushState,
    snapshot: Option<Arc<CurveSnapshot>>,
    revision: u64,
    visible_runs: std::rc::Rc<[RunRef]>,
) -> impl gpui::Styled + gpui::IntoElement {
    canvas(
        move |bounds, window, _| {
            let mut adapter = adapter.borrow_mut();
            let resized = adapter.overview_bounds != Some(bounds);
            adapter.overview_bounds = Some(bounds);
            if resized {
                window.request_animation_frame();
            }
            let Some(snapshot) = snapshot.as_deref() else {
                return PreparedChart {
                    paths: Vec::new(),
                    theme: ViewerTheme::for_appearance(window.appearance()),
                };
            };
            let Some(viewport) = overview_viewport(snapshot, brush.home(), &visible_runs) else {
                return PreparedChart {
                    paths: Vec::new(),
                    theme: ViewerTheme::for_appearance(window.appearance()),
                };
            };
            let detail_bounds = adapter.detail_bounds;
            let prepared = adapter.prepare(
                snapshot,
                revision,
                viewport,
                bounds,
                window.appearance(),
                RenderRuns {
                    baseline: None,
                    visible: Some(&visible_runs),
                },
            );
            adapter.detail_bounds = detail_bounds;
            prepared
        },
        move |bounds, prepared, window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for index in 0..=6 {
                    let ratio = index as f32 / 6.;
                    let x = bounds.origin.x + bounds.size.width * ratio;
                    window.paint_quad(fill(
                        Bounds::new(point(x, bounds.origin.y), size(px(1.), bounds.size.height)),
                        prepared.theme.colors.chart_grid,
                    ));
                }
                for (path, color) in prepared.paths {
                    window.paint_path(path, color);
                }
                let start_ratio = ((brush.selected().start() - brush.home().start())
                    / brush.home().span()) as f32;
                let end_ratio =
                    ((brush.selected().end() - brush.home().start()) / brush.home().span()) as f32;
                let start = bounds.origin.x + bounds.size.width * start_ratio;
                let end = bounds.origin.x + bounds.size.width * end_ratio;
                window.paint_quad(fill(
                    Bounds::new(
                        point(start, bounds.origin.y),
                        size(end - start, bounds.size.height),
                    ),
                    prepared.theme.colors.brush_selection,
                ));
                for x in [start, end] {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x - px(BRUSH_HANDLE_WIDTH / 2.), bounds.origin.y),
                            size(px(BRUSH_HANDLE_WIDTH), bounds.size.height),
                        ),
                        prepared.theme.colors.accent,
                    ));
                }
            });
        },
    )
}

fn overview_viewport(
    snapshot: &CurveSnapshot,
    home: AxisRange,
    visible_runs: &[RunRef],
) -> Option<Viewport> {
    detail_viewport(snapshot, None, Some(visible_runs))
        .map(|viewport| Viewport::new(home, viewport.y))
}

pub fn series_color_index(run_ref: &RunRef) -> usize {
    let mut hasher = DefaultHasher::new();
    run_ref.hash(&mut hasher);
    hasher.finish() as usize
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Duration;

    use pulseon_chart_core::{DataPoint, Series, SeriesId};
    use pulseon_core::engine::client::NativeClient;
    use pulseon_model::alignment::{AlignedMetricPoint, AlignedMetricResult, AlignmentViewport};
    use pulseon_model::comparison::EvidenceCompleteness;
    use pulseon_model::metric::{MetricKey, MetricPoint, Step};
    use pulseon_model::run::{Run, RunId, RunStatus};
    use pulseon_model::types::ProjectId;
    use pulseon_viewer::core::DataSourceId;
    use pulseon_viewer::query::{
        CurveAxis, CurveSelection, CurveSeriesSnapshot, CurveSnapshot, DetailRequest,
    };
    use pulseon_viewer::worker::{Generation, ReadRequest, ReadSnapshot, ReadWorker};

    use super::*;

    fn range(start: f64, end: f64) -> AxisRange {
        AxisRange::new(start, end).expect("test range should be valid")
    }

    #[test]
    fn series_colors_derive_from_composite_run_identity() {
        let first = RunRef::new(
            DataSourceId::from_string("duckdb-source"),
            ProjectId::from_string("project"),
            RunId::from_string("run"),
        );
        let second = RunRef::new(
            DataSourceId::from_string("sqlite-source"),
            ProjectId::from_string("project"),
            RunId::from_string("run"),
        );

        assert_ne!(
            series_color_index(&first) % 10,
            series_color_index(&second) % 10
        );
    }

    #[test]
    fn overview_curves_project_across_the_global_brush_home() {
        let snapshot = synthetic_snapshot(2, 10);
        let home = range(-5., 20.);

        let visible = snapshot
            .series
            .iter()
            .map(|curve| curve.run_ref.clone())
            .collect::<Vec<_>>();
        let viewport =
            overview_viewport(&snapshot, home, &visible).expect("overview should be drawable");

        assert_eq!(viewport.x, home);
    }

    #[test]
    fn solid_polyline_requires_one_nonzero_segment() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)));
        let repeated = [ScreenPoint::new(1., 1.), ScreenPoint::new(1., 1.)];
        let line = [ScreenPoint::new(1., 1.), ScreenPoint::new(2., 2.)];

        assert!(solid_polyline_path(&repeated, bounds, px(2.)).is_none());
        assert!(solid_polyline_path(&line, bounds, px(2.)).is_some());
    }

    #[test]
    fn render_compaction_preserves_endpoints_and_bucket_extrema() {
        let points = [
            ScreenPoint::new(0., 5.),
            ScreenPoint::new(0.5, 1.),
            ScreenPoint::new(1., 9.),
            ScreenPoint::new(1.5, 4.),
            ScreenPoint::new(2.2, 6.),
        ];

        let compact = compact_render_points(&points, RENDER_BUCKET_WIDTH);

        assert_eq!(compact, [points[0], points[1], points[2], points[4]]);
    }

    #[test]
    fn render_compaction_caps_dense_paths_by_logical_width() {
        let points = (0..10_002)
            .map(|index| ScreenPoint::new(index as f64 * 2_500. / 10_001., (index % 17) as f64))
            .collect::<Vec<_>>();

        let compact = compact_render_points(&points, RENDER_BUCKET_WIDTH);

        assert!(compact.len() <= 2_504, "{} points remained", compact.len());
    }

    #[test]
    fn brush_hit_testing_distinguishes_handles_and_window() {
        let mut adapter = ChartAdapter {
            overview_bounds: Some(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(40.)))),
            ..ChartAdapter::default()
        };
        let mut brush = BrushState::new(range(0., 100.)).expect("brush should be valid");
        brush.resize_start(20.).expect("start should resize");
        brush.resize_end(80.).expect("end should resize");

        assert_eq!(
            [20., 50., 80.].map(|x| adapter.brush_target(brush, point(px(x), px(10.)))),
            [
                Some(BrushDragTarget::Start),
                Some(BrushDragTarget::Window),
                Some(BrushDragTarget::End),
            ]
        );
        brush.resize_start(48.).expect("start should resize");
        brush.resize_end(52.).expect("end should resize");
        assert_eq!(
            [48., 52.].map(|x| adapter.brush_target(brush, point(px(x), px(10.)))),
            [Some(BrushDragTarget::Start), Some(BrushDragTarget::End),]
        );
        adapter.overview_bounds = None;
        assert_eq!(adapter.brush_target(brush, point(px(50.), px(10.))), None);
    }

    #[test]
    fn overview_widths_separate_logical_layout_from_physical_budget() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(400.), px(40.)));
        let adapter = ChartAdapter {
            overview_bounds: Some(bounds),
            ..ChartAdapter::default()
        };

        assert_eq!(adapter.overview_widths(2.), Some((400., 800)));
    }

    #[test]
    fn ruler_callouts_separate_close_values_within_the_track() {
        let source_id = DataSourceId::from_string("source");
        let project_id = ProjectId::from_string("project");
        let mut callouts = (0..3)
            .map(|index| HoverPoint {
                run_ref: RunRef::new(
                    source_id.clone(),
                    project_id.clone(),
                    RunId::from_string(format!("run-{index}")),
                ),
                run_name: format!("Run {index}"),
                metric_key: "loss".to_owned(),
                axis_value: 10,
                value: f64::from(index),
                canvas_position: point(px(50.), px(40. + index as f32 * 2.)),
                align_left: false,
            })
            .collect::<Vec<_>>();

        spread_callouts(&mut callouts, px(100.));

        let positions = callouts
            .iter()
            .map(|callout| f32::from(callout.canvas_position.y))
            .collect::<Vec<_>>();
        assert!(positions.windows(2).all(|pair| pair[1] - pair[0] >= 24.));
        assert!(
            positions
                .iter()
                .all(|position| (12. ..=88.).contains(position))
        );
    }

    #[test]
    fn hidden_runs_are_excluded_from_hover_evidence() {
        let snapshot = synthetic_snapshot(2, 10);
        let visible = vec![snapshot.series[1].run_ref.clone()];
        let viewport = detail_viewport(&snapshot, None, Some(&visible))
            .expect("visible Run should be drawable");
        let adapter = ChartAdapter {
            detail_bounds: Some(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)))),
            ..ChartAdapter::default()
        };

        let points = adapter.points_at_axis(&snapshot, viewport, 5., &visible);

        assert_eq!(points.len(), 1);
        assert_eq!(points[0].run_ref, visible[0]);
    }

    #[test]
    fn hover_maps_a_rendered_point_back_to_stored_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = NativeClient::open(root.path())?;
        let project = client.create_project("viewer", Some(ProjectId::from_string("project")))?;
        let run = client.create_run(
            &project.project_id,
            "baseline",
            Some(RunId::from_string("run")),
        )?;
        client
            .run_handle(run.clone())
            .log_metric_at_step("loss", 7, 1.25)?;
        client.finish_run(&run.run_id)?;
        client.shutdown(None)?;
        let worker = ReadWorker::spawn(root.path())?;
        let source_id = DataSourceId::from_path(root.path());
        let run_ref = RunRef::new(
            source_id.clone(),
            run.project_id.clone(),
            run.run_id.clone(),
        );
        worker.submit(
            source_id.clone(),
            Generation(1),
            ReadRequest::Detail(DetailRequest {
                selection: CurveSelection {
                    source_id,
                    runs: vec![run_ref.clone()],
                    metric_key: MetricKey::from_string("loss"),
                    axis: CurveAxis::Step,
                },
                viewport: AlignmentViewport::new(7, 8)?,
                physical_width: 100,
            }),
        )?;
        let event = worker.recv_timeout(Duration::from_secs(10))?;
        let ReadSnapshot::Detail(snapshot) = event.result? else {
            return Err("worker returned the wrong snapshot kind".into());
        };
        let visible = snapshot
            .series
            .iter()
            .map(|curve| curve.run_ref.clone())
            .collect::<Vec<_>>();
        let viewport = detail_viewport(&snapshot, None, None).ok_or("detail should be drawable")?;
        let adapter = ChartAdapter {
            detail_bounds: Some(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)))),
            ..ChartAdapter::default()
        };

        let hover = adapter
            .hit_test(&snapshot, viewport, point(px(0.), px(50.)), &visible)
            .ok_or("stored point should be hit")?;

        assert_eq!(
            (
                hover.run_name.as_str(),
                hover.metric_key.as_str(),
                hover.axis_value,
                hover.value,
                hover.run_ref,
            ),
            ("baseline", "loss", 7, 1.25, run_ref)
        );
        Ok(())
    }

    fn synthetic_snapshot(series_count: usize, point_count: i64) -> CurveSnapshot {
        let timestamp = "2026-01-01T00:00:00Z"
            .parse()
            .expect("fixed timestamp should parse");
        let source_id = DataSourceId::from_string("synthetic-source");
        let project_id = ProjectId::from_string("viewer-scale");
        let metric_key = MetricKey::from_string("loss");
        let series = (0..series_count)
            .map(|run_index| {
                let run_id = RunId::from_string(format!("run-{run_index}"));
                let run_ref = RunRef::new(source_id.clone(), project_id.clone(), run_id.clone());
                let points = (0..point_count)
                    .map(|index| AlignedMetricPoint {
                        point: MetricPoint {
                            run_id: run_id.clone(),
                            metric_key: metric_key.clone(),
                            step: Step::new(index),
                            timestamp,
                            value_f64: run_index as f64 + (index % 1_000) as f64 / 1_000.,
                            ingested_at: timestamp,
                        },
                        axis_value: index,
                    })
                    .collect::<Vec<_>>();
                let chart_series = Series::new(
                    SeriesId::new(run_ref.cache_key())
                        .expect("Run reference should make a series id"),
                    points
                        .iter()
                        .map(|point| DataPoint::new(point.axis_value as f64, point.point.value_f64))
                        .collect(),
                )
                .expect("generated series should be valid");
                CurveSeriesSnapshot {
                    run_ref,
                    run: Run {
                        run_id,
                        project_id: project_id.clone(),
                        name: format!("Run {run_index}"),
                        status: RunStatus::Finished,
                        created_at: timestamp,
                        started_at: timestamp,
                        finished_at: Some(timestamp),
                    },
                    evidence: AlignedMetricResult {
                        source_row_count: points.len() as u64,
                        points,
                        completeness: EvidenceCompleteness::Complete,
                        reasons: Vec::new(),
                    },
                    chart_series: Some(chart_series),
                }
            })
            .collect();
        CurveSnapshot {
            viewport: AlignmentViewport::new(0, point_count - 1)
                .expect("generated viewport should be valid"),
            point_budget: 10_000,
            real_range: Some(
                AlignmentViewport::new(0, point_count - 1)
                    .expect("generated range should be valid"),
            ),
            series,
        }
    }

    #[test]
    fn detail_viewport_reuses_snapshot_y_range_between_reduced_points() {
        let snapshot = synthetic_snapshot(1, 2);
        let selected = range(0.25, 0.75);

        let viewport = detail_viewport(&snapshot, Some(selected), None)
            .expect("neighbor points should keep transient zoom drawable");

        assert_eq!(viewport.x, selected);
    }

    #[test]
    fn ruler_hover_maps_each_curve_to_nearest_stored_evidence() {
        let snapshot = synthetic_snapshot(2, 10);
        let visible = snapshot
            .series
            .iter()
            .map(|curve| curve.run_ref.clone())
            .collect::<Vec<_>>();
        let viewport = detail_viewport(&snapshot, None, None).expect("snapshot should be drawable");
        let adapter = ChartAdapter {
            detail_bounds: Some(Bounds::new(point(px(0.), px(0.)), size(px(90.), px(100.)))),
            ..ChartAdapter::default()
        };

        let points = adapter.points_at_axis(&snapshot, viewport, 5.2, &visible);

        assert_eq!(points.len(), 2);
        assert!(points.iter().all(|point| point.axis_value == 5));
        assert!(
            points
                .iter()
                .all(|point| point.canvas_position.x == px(52.))
        );
        assert_ne!(points[0].canvas_position.y, points[1].canvas_position.y);
    }

    fn measure_cpu_budget(
        label: &str,
        warmups: usize,
        samples: usize,
        mut operation: impl FnMut(),
    ) {
        for _ in 0..warmups {
            operation();
        }
        let mut elapsed = Vec::with_capacity(samples);
        for _ in 0..samples {
            let started = std::time::Instant::now();
            operation();
            elapsed.push(started.elapsed());
        }
        elapsed.sort_unstable();
        let percentile = |percent: usize| elapsed[(samples * percent).div_ceil(100) - 1];
        let p50 = percentile(50);
        let p95 = percentile(95);
        let maximum = elapsed[samples - 1];
        println!(
            "{label}: samples={samples}, p50={:.3} ms, p95={:.3} ms, max={:.3} ms",
            p50.as_secs_f64() * 1_000.,
            p95.as_secs_f64() * 1_000.,
            maximum.as_secs_f64() * 1_000.,
        );
        assert!(
            p95 <= Duration::from_micros(8_330),
            "{label} p95 was {p95:?}"
        );
        assert!(
            maximum <= Duration::from_micros(16_700),
            "{label} maximum was {maximum:?}"
        );
    }

    #[test]
    #[ignore = "hardware-sensitive release performance validation"]
    fn interactive_chart_cpu_budget() {
        assert!(
            black_box(!cfg!(debug_assertions)),
            "CPU validation requires --release"
        );
        let home = range(0., 10_001.);
        let mut brush = BrushState::new(home).expect("brush should initialize");
        brush.resize_start(2_000.).expect("brush should resize");
        brush.resize_end(8_000.).expect("brush should resize");
        measure_cpu_budget("brush resize", 100, 1_000, || {
            brush.resize_start(2_001.).expect("brush should resize");
            brush.resize_start(2_000.).expect("brush should resize");
            black_box(brush);
        });
        measure_cpu_budget("brush pan", 100, 1_000, || {
            brush.pan_by(1.).expect("brush should pan");
            brush.pan_by(-1.).expect("brush should pan");
            black_box(brush);
        });
        measure_cpu_budget("brush zoom", 100, 1_000, || {
            brush.zoom_at(5_000., 1.01).expect("brush should zoom");
            brush.zoom_at(5_000., 1. / 1.01).expect("brush should zoom");
            black_box(brush);
        });

        let snapshot = synthetic_snapshot(10, 10_002);
        let viewport = Viewport::new(home, range(0., 11.));
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(2_500.), px(800.)));
        let visible = snapshot
            .series
            .iter()
            .map(|curve| curve.run_ref.clone())
            .collect::<Vec<_>>();
        let mut adapter = ChartAdapter::default();
        black_box(adapter.prepare(
            &snapshot,
            1,
            viewport,
            bounds,
            WindowAppearance::Light,
            RenderRuns {
                baseline: None,
                visible: None,
            },
        ));
        measure_cpu_budget("cached path preparation", 20, 200, || {
            black_box(adapter.prepare(
                &snapshot,
                1,
                viewport,
                bounds,
                WindowAppearance::Light,
                RenderRuns {
                    baseline: None,
                    visible: None,
                },
            ));
        });
        let mut revision = 2;
        measure_cpu_budget("uncached path preparation", 20, 200, || {
            black_box(adapter.prepare(
                &snapshot,
                revision,
                viewport,
                bounds,
                WindowAppearance::Light,
                RenderRuns {
                    baseline: None,
                    visible: None,
                },
            ));
            revision += 1;
        });
        measure_cpu_budget("hit testing", 20, 200, || {
            black_box(adapter.hit_test(&snapshot, viewport, point(px(1_250.), px(400.)), &visible));
        });
        measure_cpu_budget("ruler hover evidence", 100, 1_000, || {
            black_box(adapter.points_at_axis(&snapshot, viewport, 5_000.5, &visible));
        });
    }
}
