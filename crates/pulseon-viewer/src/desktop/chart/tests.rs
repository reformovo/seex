use std::hint::black_box;
use std::time::Duration;

use crate::data::query::{
    CurveAxis, CurveSelection, CurveSeriesSnapshot, CurveSnapshot, DetailRequest,
};
use crate::data::worker::{Generation, ReadRequest, ReadSnapshot, ReadWorker, recv_event_for_test};
use crate::domain::DataSourceId;
use pulseon_chart_core::{DataPoint, Series, SeriesId};
use pulseon_core::engine::client::NativeClient;
use pulseon_model::alignment::{AlignedMetricPoint, AlignedMetricResult, AlignmentViewport};
use pulseon_model::comparison::EvidenceCompleteness;
use pulseon_model::metric::{MetricKey, MetricPoint, Step};
use pulseon_model::run::{Run, RunId, RunStatus};
use pulseon_model::types::ProjectId;

use super::*;
use gpui::size;

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
fn emphasized_runs_dim_other_curves_without_hiding_the_baseline() {
    let snapshot = synthetic_snapshot(3, 10);
    let visible = snapshot
        .series
        .iter()
        .map(|curve| curve.run_ref.clone())
        .collect::<Vec<_>>();
    let viewport = detail_viewport(&snapshot, None, Some(&visible)).expect("viewport");
    let prepared = ChartAdapter::default().prepare(
        &snapshot,
        1,
        viewport,
        Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.))),
        WindowAppearance::Dark,
        RenderRuns {
            baseline: Some(&visible[1]),
            emphasized: Some(&visible[0]),
            visible: Some(&visible),
        },
    );

    assert_eq!(prepared.paths.len(), 3);
    for (actual, expected) in prepared
        .paths
        .iter()
        .map(|(_, color)| color.a)
        .zip([1., 0.5, 0.18])
    {
        assert!((actual - expected).abs() < f32::EPSILON);
    }
}

#[test]
fn hidden_runs_are_excluded_from_hover_evidence() {
    let snapshot = synthetic_snapshot(2, 10);
    let visible = vec![snapshot.series[1].run_ref.clone()];
    let viewport =
        detail_viewport(&snapshot, None, Some(&visible)).expect("visible Run should be drawable");
    let adapter = ChartAdapter {
        detail_bounds: Some(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)))),
        ..ChartAdapter::default()
    };

    let points = adapter.points_at_axis(&snapshot, viewport, 5., &visible);

    assert_eq!(points.len(), 1);
    assert_eq!(points[0].run_ref, visible[0]);
}

#[test]
fn hover_maps_a_rendered_point_back_to_stored_evidence() -> Result<(), Box<dyn std::error::Error>> {
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
    let mut worker = ReadWorker::spawn(root.path())?;
    let events = worker
        .take_event_receiver()
        .ok_or("worker event receiver should be available")?;
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
    let event = recv_event_for_test(&events, Duration::from_secs(10))
        .ok_or("worker event did not arrive before the test deadline")?;
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
        (hover.axis_value, hover.value, hover.run_ref),
        (7, 1.25, run_ref)
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
                SeriesId::new(run_ref.cache_key()).expect("Run reference should make a series id"),
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
            AlignmentViewport::new(0, point_count - 1).expect("generated range should be valid"),
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

fn measure_cpu_budget(label: &str, warmups: usize, samples: usize, mut operation: impl FnMut()) {
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
            emphasized: None,
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
                emphasized: None,
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
                emphasized: None,
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
