use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use gpui::{
    Bounds, ContentMask, Path, PathBuilder, Pixels, Point, Rgba, WindowAppearance, canvas, fill,
    point, px, size,
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
    pub step: i64,
    pub value: f64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct GpuiPathKey {
    revision: u64,
    viewport: [u64; 4],
    bounds: [u32; 4],
    dark: bool,
    partial: bool,
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
}

struct PreparedChart {
    paths: Vec<(Path<Pixels>, Rgba)>,
    theme: ViewerTheme,
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
    ) {
        for series in snapshot
            .series
            .iter()
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
    ) -> PreparedChart {
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
            let Some(series) = curve.chart_series.as_ref() else {
                continue;
            };
            let partial = curve.evidence.completeness == EvidenceCompleteness::Partial;
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
                let mut builder = PathBuilder::stroke(px(2.));
                if partial {
                    builder = builder.dash_array(&[px(7.), px(4.)]);
                }
                for (point_index, projected) in points.iter().enumerate() {
                    let position = point(
                        bounds.origin.x + px(projected.x as f32),
                        bounds.origin.y + px(projected.y as f32),
                    );
                    if point_index == 0 {
                        builder.move_to(position);
                    } else {
                        builder.line_to(position);
                    }
                }
                let Ok(path) = builder.build() else {
                    continue;
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
                    step: aligned.point.step.value(),
                    value: aligned.point.value_f64,
                },
            ));
        }
        nearest.map(|(_, point)| point)
    }

    pub fn detail_axis_at(&self, range: AxisRange, cursor: Point<Pixels>) -> Option<f64> {
        axis_at(self.detail_bounds?, range, cursor)
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

    pub fn physical_widths(&self, scale_factor: f32) -> (Option<u32>, Option<u32>) {
        (
            physical_width(self.overview_bounds, scale_factor),
            physical_width(self.detail_bounds, scale_factor),
        )
    }
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

pub fn detail_viewport(snapshot: &CurveSnapshot, selected: Option<AxisRange>) -> Option<Viewport> {
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

pub fn selection_covered(snapshot: &CurveSnapshot, selected: AxisRange) -> bool {
    snapshot.viewport.start() as f64 <= selected.start()
        && snapshot.viewport.end() as f64 >= selected.end()
}

pub fn detail_canvas(
    adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
    snapshot: Arc<CurveSnapshot>,
    revision: u64,
    viewport: Viewport,
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

pub fn timeline_canvas(
    adapter: std::rc::Rc<std::cell::RefCell<ChartAdapter>>,
    brush: BrushState,
) -> impl gpui::Styled + gpui::IntoElement {
    canvas(
        move |bounds, window, _| {
            let resized = adapter.borrow().overview_bounds != Some(bounds);
            adapter.borrow_mut().overview_bounds = Some(bounds);
            if resized {
                window.request_animation_frame();
            }
            ViewerTheme::for_appearance(window.appearance())
        },
        move |bounds, theme, window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for index in 0..=6 {
                    let ratio = index as f32 / 6.;
                    let x = bounds.origin.x + bounds.size.width * ratio;
                    window.paint_quad(fill(
                        Bounds::new(point(x, bounds.origin.y), size(px(1.), bounds.size.height)),
                        theme.colors.chart_grid,
                    ));
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
                    theme.colors.brush_selection,
                ));
                for x in [start, end] {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x - px(4.), bounds.origin.y),
                            size(px(8.), bounds.size.height),
                        ),
                        theme.colors.accent,
                    ));
                }
            });
        },
    )
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
    fn detail_snapshot_coverage_distinguishes_transient_ranges() {
        let snapshot = CurveSnapshot {
            viewport: AlignmentViewport::new(10, 20).expect("test viewport should be valid"),
            point_budget: 2_000,
            real_range: None,
            series: Vec::new(),
        };

        assert!(selection_covered(&snapshot, range(12., 18.)));
        assert!(!selection_covered(&snapshot, range(5., 15.)));
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
    fn physical_width_uses_display_scale() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(400.), px(40.)));

        assert_eq!(physical_width(Some(bounds), 2.), Some(800));
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
        let viewport = detail_viewport(&snapshot, None).ok_or("detail should be drawable")?;
        let adapter = ChartAdapter {
            detail_bounds: Some(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)))),
            ..ChartAdapter::default()
        };

        let hover = adapter
            .hit_test(&snapshot, viewport, point(px(0.), px(50.)))
            .ok_or("stored point should be hit")?;

        assert_eq!(
            (
                hover.run_name.as_str(),
                hover.metric_key.as_str(),
                hover.axis_value,
                hover.step,
                hover.value,
                hover.run_ref,
            ),
            ("baseline", "loss", 7, 7, 1.25, run_ref)
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

        let viewport = detail_viewport(&snapshot, Some(selected))
            .expect("neighbor points should keep transient zoom drawable");

        assert_eq!(viewport.x, selected);
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
        let mut adapter = ChartAdapter::default();
        black_box(adapter.prepare(&snapshot, 1, viewport, bounds, WindowAppearance::Light));
        measure_cpu_budget("cached path preparation", 20, 200, || {
            black_box(adapter.prepare(&snapshot, 1, viewport, bounds, WindowAppearance::Light));
        });
        let mut revision = 2;
        measure_cpu_budget("uncached path preparation", 20, 200, || {
            black_box(adapter.prepare(
                &snapshot,
                revision,
                viewport,
                bounds,
                WindowAppearance::Light,
            ));
            revision += 1;
        });
        measure_cpu_budget("hit testing", 20, 200, || {
            black_box(adapter.hit_test(&snapshot, viewport, point(px(1_250.), px(400.))));
        });
    }
}
