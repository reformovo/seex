use std::collections::HashMap;

use crate::data::query::CurveSnapshot;
use crate::domain::RunRef;
use gpui::{Bounds, Path, PathBuilder, Pixels, Point, Rgba, WindowAppearance, point, px};
use seex_chart_core::{
    AxisRange, BrushState, CanvasSize, LinearScale, PathCache, ScreenPoint, Viewport,
    hit_test_point,
};
use seex_model::comparison::EvidenceCompleteness;

use super::theme::ViewerTheme;

mod canvas;
mod detail;
mod projection;

pub(in crate::desktop::app) use canvas::{
    callout_shell, cursor_canvas, detail_canvas, timeline_canvas,
};
pub(in crate::desktop::app) use detail::{
    BrushDragTarget, DetailChart, HoverPoint, OverviewChart, cached_detail_chart,
};
use projection::{
    axis_at, compact_render_points, overview_viewport, projected_point, solid_polyline_path,
};
pub(in crate::desktop::app) use projection::{detail_viewport, series_color_index};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct GpuiPathKey {
    revision: u64,
    viewport: [u64; 4],
    bounds: [u32; 4],
    dark: bool,
    partial: bool,
    highlighted: bool,
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
const CURVE_STROKE_WIDTH: f32 = 1.;
const HIGHLIGHTED_CURVE_STROKE_WIDTH: f32 = 1.5;

#[derive(Clone, Copy)]
struct RenderRuns<'a> {
    baseline: Option<&'a RunRef>,
    emphasized: Option<&'a RunRef>,
    visible: Option<&'a [RunRef]>,
}

impl ChartAdapter {
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
            let baseline = runs.baseline == Some(&curve.run_ref);
            let emphasized = runs.emphasized == Some(&curve.run_ref);
            let highlighted = baseline || emphasized;
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
                let width = px(if highlighted {
                    HIGHLIGHTED_CURVE_STROKE_WIDTH
                } else {
                    CURVE_STROKE_WIDTH
                });
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
            let mut color = theme
                .colors
                .series_color(series_color_index(&curve.run_ref));
            color.a *= if runs.emphasized.is_none() || emphasized {
                1.
            } else if baseline {
                0.5
            } else {
                0.18
            };
            paths.push((path, color));
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
}

#[cfg(test)]
mod tests;
