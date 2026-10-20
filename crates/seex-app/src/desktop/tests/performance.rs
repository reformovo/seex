use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{ListOffset, TestAppContext, VisualTestContext, WindowHandle, px, size};

use super::super::test_support::*;
use super::*;

fn benchmark_root() -> PathBuf {
    std::env::var_os("SEEX_VIEWER_BENCH_ROOT")
        .map(PathBuf::from)
        .expect("SEEX_VIEWER_BENCH_ROOT must name the prepared compact fixture")
}

fn benchmark_count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .map(|value| {
            let count = value
                .parse()
                .unwrap_or_else(|_| panic!("{name} must be a positive integer"));
            assert!(count > 0, "{name} must be a positive integer");
            count
        })
        .unwrap_or(default)
}

fn optional_benchmark_count(name: &str) -> usize {
    std::env::var(name).map_or(0, |value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a non-negative integer"))
    })
}

fn inactive_views_are_released(viewer: &ViewerApp, cx: &App) -> bool {
    let snapshot = viewer.session_snapshot(cx);
    let active = snapshot.views.active();
    snapshot.views.views().iter().all(|view| {
        view.view_id == active.view_id
            || (view.timeline_extents.is_empty()
                && view.panels.iter().all(|panel| {
                    panel.overview.is_none()
                        && panel.detail.is_none()
                        && panel.inspector.is_none()
                        && panel.overview_generation.is_none()
                        && panel.detail_generation.is_none()
                        && panel.inspector_generation.is_none()
                        && panel.requested_overview_budget.is_none()
                        && panel.requested_detail_viewport.is_none()
                        && panel.source_errors.is_empty()
                        && panel.inspector_errors.is_empty()
                }))
    })
}

fn active_previews_are_ready(viewer: &ViewerApp, cx: &App) -> bool {
    let snapshot = viewer.session_snapshot(cx);
    let active = snapshot.views.active();
    let visible = viewer
        .workspace
        .read(cx)
        .track_viewport
        .borrow()
        .visible
        .clone();
    active.panels.len() == benchmark_count("SEEX_VIEWER_BENCH_METRICS", 2)
        && !visible.is_empty()
        && active.panels[visible.clone()]
            .iter()
            .all(|panel| panel.overview.is_some())
        && active
            .panels
            .iter()
            .filter(|panel| panel.detail.is_some())
            .count()
            <= visible.len()
        && active
            .selected_panel_id
            .as_ref()
            .and_then(|panel_id| snapshot.views.active_panel(panel_id))
            .is_some_and(|panel| panel.overview.is_some())
        && active
            .panels
            .iter()
            .filter(|panel| panel.overview.is_some())
            .count()
            <= visible.len().saturating_add(3)
        && viewer.workspace.read(cx).track_charts.len() == visible.len()
        && inactive_views_are_released(viewer, cx)
}

fn active_overview_coverage_is_ready(viewer: &ViewerApp, cx: &App) -> bool {
    let snapshot = viewer.session_snapshot(cx);
    let active = snapshot.views.active();
    active_previews_are_ready(viewer, cx)
        && active
            .panels
            .iter()
            .filter(|panel| panel.overview.is_some())
            .all(|panel| {
                !panel.is_pending(ReadKind::Overview)
                    && panel.overview.as_ref().is_some_and(|overview| {
                        active
                            .runs
                            .iter()
                            .all(|run| overview.series.iter().any(|curve| &curve.run_ref == run))
                    })
            })
}

fn active_curves_are_settled(viewer: &ViewerApp, cx: &App) -> bool {
    active_overview_coverage_is_ready(viewer, cx)
        && viewer
            .session_snapshot(cx)
            .views
            .active()
            .panels
            .iter()
            .all(|panel| !panel.is_pending(ReadKind::Detail))
}

fn selected_detail_is_settled(viewer: &ViewerApp, cx: &App) -> bool {
    let snapshot = viewer.session_snapshot(cx);
    active_overview_coverage_is_ready(viewer, cx)
        && snapshot
            .views
            .active()
            .selected_panel_id
            .as_ref()
            .and_then(|panel_id| snapshot.views.active_panel(panel_id))
            .is_some_and(|panel| !panel.is_pending(ReadKind::Detail))
}

fn first_visible_curve(viewer: &ViewerApp, cx: &App) -> bool {
    inactive_views_are_released(viewer, cx) && !viewer.workspace.read(cx).track_charts.is_empty()
}

fn duration_p95(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[(samples.len() * 95).div_ceil(100).saturating_sub(1)]
}

fn wait_for_benchmark(
    window: WindowHandle<ViewerApp>,
    cx: &VisualTestContext,
    condition: impl Fn(&ViewerApp, &App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        cx.run_until_parked();
        let ready = window
            .read_with(cx, |viewer, cx| condition(viewer, cx))
            .expect("viewer should remain open");
        if ready {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("viewer benchmark state did not arrive before the test deadline");
}

fn scroll_metric_tracks(
    window: WindowHandle<ViewerApp>,
    cx: &mut VisualTestContext,
    metric_count: usize,
    to_end: bool,
) {
    window
        .update(cx, |viewer, _, cx| {
            viewer.workspace.update(cx, |workspace, cx| {
                if to_end {
                    workspace
                        .metric_scroll
                        .scroll_to_reveal_item(metric_count - 1);
                } else {
                    workspace.metric_scroll.scroll_to(ListOffset::default());
                }
                cx.notify();
            });
        })
        .expect("Viewer should remain open");
    wait_for_benchmark(window, cx, active_curves_are_settled);
}

// TestPlatform state/readiness diagnostic, not a native frame/RSS measurement.
fn scroll_and_drain(
    window: WindowHandle<ViewerApp>,
    cx: &mut VisualTestContext,
    target: usize,
    tail: bool,
) {
    window
        .update(cx, |viewer, _, cx| {
            viewer.workspace.update(cx, |workspace, cx| {
                workspace.metric_scroll.scroll_to(ListOffset {
                    item_ix: target,
                    offset_in_item: px(0.),
                });
                cx.notify();
            });
        })
        .unwrap();
    let started = Instant::now();
    let mut peak = [0; 3];
    loop {
        cx.run_until_parked();
        let ready = window.read_with(cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            let active = snapshot.views.active();
            let workspace = viewer.workspace.read(cx);
            let viewport = workspace.track_viewport.borrow();
            let depths = viewer.session.read(cx).sources.queue_depths();
            for i in 0..3 { peak[i] = peak[i].max(depths[i]); }
            let arrived = viewport.visible.contains(&target)
                && (!tail || viewport.visible.end == active.panels.len());
            let mut counts = [0; 3];
            let mut covered = true;
            let mut pending = false;
            for (index, panel) in active.panels.iter().enumerate() {
                let retained = viewport.overview.contains(&index)
                    || active.selected_panel_id.as_ref() == Some(&panel.panel_id);
                covered &= !retained || panel.overview.as_ref().is_some_and(|s| {
                    active.runs.iter().all(|r| s.series.iter().any(|c| &c.run_ref == r))
                });
                pending |= panel.is_pending(ReadKind::Overview) || panel.is_pending(ReadKind::Detail);
                assert!(panel.source_errors.is_empty(), "panel read failed");
                for (snapshot, allowed, slot) in [(&panel.overview, retained, 0),
                    (&panel.detail, viewport.visible.contains(&index), 1)] {
                    if let Some(snapshot) = snapshot {
                        assert!(allowed, "offscreen payload retained at {index}");
                        counts[slot] += 1;
                        counts[2] += snapshot.series.iter().filter_map(|s| s.chart_series.as_ref())
                            .map(|s| s.points().len()).sum::<usize>();
                    }
                }
            }
            let limit = viewport.overview.len() + 1;
            assert!(counts[0] <= limit && counts[1] <= viewport.visible.len());
            assert!(counts[2] <= limit + viewport.visible.len(), "one-point fixture exceeded residency bound");
            let ready = arrived && covered && !pending && depths == [0; 3];
            assert!(ready || started.elapsed() < Duration::from_secs(15), "scroll/drain timeout target={target} visible={:?} arrived={arrived} covered={covered} pending={pending} depths={depths:?} counts={counts:?}", viewport.visible);
            if ready { println!("SEEX_SCROLL_DRAIN target={target} visible={:?} ms={} view_payloads_points={counts:?} sampled_peak={peak:?}", viewport.visible, started.elapsed().as_millis()); }
            ready
        }).unwrap();
        if ready {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "scroll/drain timeout target={target} peak={peak:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui::test]
#[ignore = "diagnostic overview scheduling profile"]
fn overview_schedule_diagnostic(cx: &mut TestAppContext) {
    assert!(!cfg!(debug_assertions), "requires --release");
    let metric_count = benchmark_count("SEEX_DESKTOP_PROFILE_METRICS", 100);
    let started = Instant::now();
    let (root, project_id, run_id) = fixture_with_runs(metric_count, 1);
    println!(
        "SEEX_STAGE fixture metrics={metric_count} ms={}",
        started.elapsed().as_millis()
    );
    cx.executor().allow_parking();
    let (window, mut visual) = open_viewer_with_configured_source(cx, root.path().to_owned());
    visual.simulate_resize(size(px(2_560.), px(1_800.)));
    wait_for_viewer(window, &visual, source_catalog_loaded);
    window
        .update(&mut visual, |viewer, _, cx| {
            let source_id = first_source_id(viewer, cx);
            viewer.toggle_run(RunRef::new(source_id, project_id, run_id), cx);
        })
        .expect("Viewer should remain open");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        visual.run_until_parked();
        let available = window
            .read_with(&visual, |viewer, cx| viewer.available_metric_keys(cx).len())
            .unwrap();
        if available == metric_count {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "discovery did not settle: expected={metric_count} available={available}; no panel selection performed"
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("SEEX_STAGE discovery_ready metrics={metric_count}");
    super::super::workspace::reset_schedule_diagnostics();
    let selected = Instant::now();
    window
        .update(&mut visual, |viewer, _, cx| {
            viewer.dispatch_workbench_command(
                WorkbenchCommand::SelectMetrics(viewer.available_metric_keys(cx)),
                cx,
            );
        })
        .expect("Viewer should remain open");
    println!(
        "SEEX_STAGE selection metrics={metric_count} ms={}",
        selected.elapsed().as_millis()
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        visual.run_until_parked();
        let state = window
            .read_with(&visual, |viewer, cx| {
                let snapshot = viewer.session_snapshot(cx);
                let active = snapshot.views.active();
                let visible = viewer
                    .workspace
                    .read(cx)
                    .track_viewport
                    .borrow()
                    .visible
                    .clone();
                (
                    active.panels.len(),
                    visible.len(),
                    active.panels[visible]
                        .iter()
                        .filter(|panel| panel.overview.is_some())
                        .count(),
                    active
                        .panels
                        .iter()
                        .filter(|panel| panel.is_pending(ReadKind::Overview))
                        .count(),
                    active
                        .panels
                        .iter()
                        .filter(|panel| !panel.source_errors.is_empty())
                        .count(),
                )
            })
            .unwrap();
        let diagnostics = super::super::workspace::schedule_diagnostics();
        if state.0 == metric_count && state.1 > 0 && state.1 == state.2 {
            println!(
                "SEEX_STAGE settled metrics={metric_count} since_selection_ms={} state={state:?} diagnostics={diagnostics:?}",
                selected.elapsed().as_millis()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Overview did not settle: state={state:?} diagnostics={diagnostics:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    for (target, tail) in [
        (metric_count - 1, true),
        (metric_count / 2, false),
        (0, false),
    ] {
        scroll_and_drain(window, &mut visual, target, tail);
    }
}

#[gpui::test]
#[ignore = "diagnostic cancelled Detail reversal integration profile"]
fn cancelled_detail_reversal_integration(cx: &mut TestAppContext) {
    assert!(!cfg!(debug_assertions), "requires --release");
    let metric_count = benchmark_count("SEEX_DETAIL_REVERSAL_METRICS", 128);
    assert!(
        metric_count >= 128,
        "needs separated head/middle/tail windows"
    );
    let root = tempfile::tempdir().unwrap();
    let client = seex::Client::builder(root.path()).open().unwrap();
    let run = client
        .start_run(seex::RunOptions::new("project").id("run"))
        .unwrap();
    for start in (0..metric_count).step_by(1024) {
        run.log_with(
            (start..(start + 1024).min(metric_count)).map(|i| (format!("metric-{i:04}"), 1.)),
            seex::LogOptions::new().step(0),
        )
        .unwrap();
    }
    for step in 1..4096 {
        run.log_with(
            [("metric-0000", (step as f64).sin())],
            seex::LogOptions::new().step(step),
        )
        .unwrap();
    }
    run.finish().unwrap();
    client.shutdown().unwrap();
    let project_id = seex::ProjectId::from_string("project");
    let run_id = run.run_id().clone();
    cx.executor().allow_parking();
    let (window, mut visual) = open_viewer_with_configured_source(cx, root.path().to_owned());
    visual.simulate_resize(size(px(2_560.), px(1_800.)));
    wait_for_viewer(window, &visual, source_catalog_loaded);
    window
        .update(&mut visual, |viewer, _, cx| {
            let source_id = first_source_id(viewer, cx);
            viewer.toggle_run(RunRef::new(source_id, project_id, run_id), cx);
        })
        .unwrap();
    wait_for_benchmark(window, &visual, |viewer, cx| {
        viewer.available_metric_keys(cx).len() == metric_count
    });
    window
        .update(&mut visual, |viewer, _, cx| {
            viewer.dispatch_workbench_command(
                WorkbenchCommand::SelectMetrics(viewer.available_metric_keys(cx)),
                cx,
            );
            viewer.select_metric(seex::MetricKey::from_string("metric-0000"), cx);
        })
        .unwrap();
    wait_for_benchmark(window, &visual, |viewer, cx| {
        let snapshot = viewer.session_snapshot(cx);
        let active = snapshot.views.active();
        let dense = active.panels[0].overview.as_ref();
        active.panels[..33]
            .iter()
            .all(|panel| panel.overview.is_some())
            && dense.is_some_and(|s| s.series[0].downsampled())
    });
    let pause = crate::data::worker::TestReadPause::arm();
    window
        .update(&mut visual, |viewer, _, cx| {
            zoom_session_navigation(viewer, 2., cx);
        })
        .unwrap();
    wait_for_viewer(window, &visual, |_, _| pause.entered() > 0);
    let old_generation = window
        .read_with(&visual, |viewer, cx| {
            viewer.session_snapshot(cx).views.active().panels[0]
                .detail_generation
                .unwrap()
        })
        .unwrap();
    for target in [metric_count - 1, metric_count / 2, 0, metric_count - 1, 0] {
        window
            .update(&mut visual, |viewer, _, cx| {
                viewer.workspace.update(cx, |workspace, cx| {
                    workspace.metric_scroll.scroll_to(ListOffset {
                        item_ix: target,
                        offset_in_item: px(0.),
                    });
                    cx.notify();
                });
            })
            .unwrap();
        visual.run_until_parked();
        window
            .read_with(&visual, |viewer, cx| {
                let workspace = viewer.workspace.read(cx);
                let viewport = workspace.track_viewport.borrow();
                assert!(viewport.visible.contains(&target));
                let snapshot = viewer.session_snapshot(cx);
                let active = snapshot.views.active();
                if target != 0 {
                    assert!(active.panels[0].detail_generation.is_none());
                    assert!(active.panels[0].detail.is_none());
                }
                let mut payloads = 0;
                let mut points = 0;
                for (index, panel) in active.panels.iter().enumerate() {
                    let retained = viewport.overview.contains(&index)
                        || active.selected_panel_id.as_ref() == Some(&panel.panel_id);
                    for (curve, allowed) in [(&panel.overview, retained),
                        (&panel.detail, viewport.visible.contains(&index))] {
                        if let Some(curve) = curve {
                            assert!(allowed, "offscreen View payload at {index}");
                            payloads += 1;
                            points += curve.series.iter().filter_map(|c| c.chart_series.as_ref())
                                .map(|s| s.points().len()).sum::<usize>();
                        }
                    }
                }
                let max_payloads = viewport.overview.len() + 1 + viewport.visible.len();
                assert!(payloads <= max_payloads);
                // Only the first metric is dense; at most Overview + Detail for it.
                assert!(points <= max_payloads + 2 * seex::MAX_CHART_POINTS);
                println!("SEEX_REVERSAL_WINDOW metrics={metric_count} target={target} view_payloads={payloads} view_points={points}");
            })
            .unwrap();
    }
    let entered = pause.entered();
    drop(pause);
    let started = Instant::now();
    wait_for_viewer(window, &visual, |viewer, cx| {
        let snapshot = viewer.session_snapshot(cx);
        let pending = snapshot
            .views
            .active()
            .panels
            .iter()
            .any(|p| p.is_pending(ReadKind::Detail) || p.is_pending(ReadKind::Overview));
        !pending && viewer.session.read(cx).sources.queue_depths() == [0; 3]
    });
    window
        .read_with(&visual, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            let active = snapshot.views.active();
            let panel = &active.panels[0];
            assert!(
                panel.detail_revision > old_generation.0,
                "cancelled generation was accepted"
            );
            let detail = panel
                .detail
                .as_ref()
                .expect("returned dense metric needs Detail");
            let series = detail.series[0].chart_series.as_ref().unwrap();
            assert!(!series.points().is_empty());
            let failures = active
                .panels
                .iter()
                .filter(|p| !p.source_errors.is_empty())
                .count();
            assert_eq!(failures, 0);
            let workspace = viewer.workspace.read(cx);
            let viewport = workspace.track_viewport.borrow();
            let mut payloads = 0;
            let mut points = 0;
            for (index, panel) in active.panels.iter().enumerate() {
                let retained = viewport.overview.contains(&index)
                    || active.selected_panel_id.as_ref() == Some(&panel.panel_id);
                for (curve, allowed) in [(&panel.overview, retained),
                    (&panel.detail, viewport.visible.contains(&index))] {
                    if let Some(curve) = curve {
                        assert!(allowed, "offscreen payload after drain at {index}");
                        payloads += 1;
                        points += curve.series.iter().filter_map(|c| c.chart_series.as_ref())
                            .map(|s| s.points().len()).sum::<usize>();
                    }
                }
            }
            let limit = viewport.overview.len() + 1 + viewport.visible.len();
            assert!(payloads <= limit && points <= limit + 2 * seex::MAX_CHART_POINTS);
            println!("SEEX_REVERSAL_DRAIN metrics={metric_count} view_payloads={payloads} view_points={points}");
        })
        .unwrap();
    println!(
        "SEEX_ACTIVE_DETAIL_REVERSAL metrics={metric_count} entered={} drain_ms={}",
        entered,
        started.elapsed().as_millis()
    );
}

#[gpui::test]
#[ignore = "hardware-sensitive compact dual-View RSS benchmark"]
fn compact_dual_view_peak_rss(cx: &mut TestAppContext) {
    assert!(
        std::hint::black_box(!cfg!(debug_assertions)),
        "Viewer RSS benchmark requires --release"
    );
    let root = benchmark_root();
    let reader = seex::Reader::builder(&root)
        .open()
        .expect("compact Viewer benchmark dataset should open");
    let project_id = reader
        .projects()
        .expect("benchmark dataset projects should load")[0]
        .project_id
        .clone();
    let runs = reader
        .runs(&project_id)
        .expect("benchmark dataset Runs should load");
    let metrics = reader
        .metrics(&runs[0])
        .expect("benchmark dataset Metrics should load");
    assert_eq!(
        (runs.len(), metrics.len()),
        (
            benchmark_count("SEEX_VIEWER_BENCH_RUNS", 4),
            benchmark_count("SEEX_VIEWER_BENCH_METRICS", 2)
        )
    );
    drop(reader);

    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, root);
    cx.simulate_resize(size(px(2_560.), px(1_800.)));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(
        window,
        &mut cx,
        project_id.clone(),
        runs[0].run_id.clone(),
        metrics.len(),
    );
    window
        .update(&mut cx, |viewer, _, cx| {
            let source_id = first_source_id(viewer, cx);
            for run in &runs[1..] {
                viewer.toggle_run(
                    RunRef::new(source_id.clone(), project_id.clone(), run.run_id.clone()),
                    cx,
                );
            }
            for metric in &metrics {
                viewer.select_metric(metric.metric_key.clone(), cx);
            }
        })
        .expect("Viewer should remain open");
    wait_for_benchmark(window, &cx, active_curves_are_settled);
    let original_view = window
        .read_with(&cx, |viewer, cx| {
            viewer.session_snapshot(cx).views.active().view_id.clone()
        })
        .expect("Viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.dispatch_workbench_command(WorkbenchCommand::DuplicateActiveView, cx);
        })
        .expect("Viewer should remain open");
    wait_for_benchmark(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.views().len() == 2
            && active_curves_are_settled(viewer, cx)
    });
    let duplicate_view = window
        .read_with(&cx, |viewer, cx| {
            viewer.session_snapshot(cx).views.active().view_id.clone()
        })
        .expect("Viewer should remain open");

    let mut first_curve = Vec::new();
    let mut all_first_runs = Vec::new();
    let mut full_coverage = Vec::new();
    let mut detail_settled = Vec::new();
    for view_id in [
        original_view.clone(),
        duplicate_view.clone(),
        original_view,
        duplicate_view,
    ] {
        let activated = Instant::now();
        window
            .update(&mut cx, |viewer, _, cx| {
                viewer.dispatch_workbench_command(WorkbenchCommand::ActivateView(view_id), cx);
            })
            .expect("Viewer should remain open");
        wait_for_benchmark(window, &cx, first_visible_curve);
        first_curve.push(activated.elapsed());
        wait_for_benchmark(window, &cx, active_previews_are_ready);
        let first_runs_elapsed = activated.elapsed();
        all_first_runs.push(first_runs_elapsed);
        wait_for_benchmark(window, &cx, active_overview_coverage_is_ready);
        let coverage_elapsed = activated.elapsed();
        full_coverage.push(coverage_elapsed);
        wait_for_benchmark(window, &cx, selected_detail_is_settled);
        let settled_elapsed = activated.elapsed();
        detail_settled.push(settled_elapsed);
        println!(
            "SEEX_ACTIVATION_SAMPLE first_curve_ms={} all_first_runs_ms={} full_coverage_ms={} detail_settled_ms={}",
            first_curve
                .last()
                .expect("first curve sample should exist")
                .as_millis(),
            first_runs_elapsed.as_millis(),
            coverage_elapsed.as_millis(),
            settled_elapsed.as_millis(),
        );
        wait_for_benchmark(window, &cx, active_curves_are_settled);
    }
    let first_curve_p95 = duration_p95(&mut first_curve);
    let all_first_runs_p95 = duration_p95(&mut all_first_runs);
    let full_coverage_p95 = duration_p95(&mut full_coverage);
    let detail_settled_p95 = duration_p95(&mut detail_settled);
    println!(
        "SEEX_ACTIVATION_FIRST_CURVE_P95_MS {}",
        first_curve_p95.as_millis()
    );
    println!(
        "SEEX_ACTIVATION_ALL_FIRST_RUNS_P95_MS {}",
        all_first_runs_p95.as_millis()
    );
    println!(
        "SEEX_ACTIVATION_FULL_COVERAGE_P95_MS {}",
        full_coverage_p95.as_millis()
    );
    println!(
        "SEEX_ACTIVATION_DETAIL_SETTLED_P95_MS {}",
        detail_settled_p95.as_millis()
    );
    assert!(first_curve_p95 <= Duration::from_secs(1));
    assert!(all_first_runs_p95 <= Duration::from_secs(1));
    assert!(full_coverage_p95 <= Duration::from_millis(3_500));
    assert!(detail_settled_p95 <= Duration::from_secs(5));

    println!("SEEX_RSS_PHASE warm");
    for round in 0..optional_benchmark_count("SEEX_VIEWER_BENCH_SCROLL_ROUND_TRIPS") {
        scroll_metric_tracks(window, &mut cx, metrics.len(), true);
        println!("SEEX_RSS_PHASE scroll_bottom_{round}");
        scroll_metric_tracks(window, &mut cx, metrics.len(), false);
        println!("SEEX_RSS_PHASE scroll_top_{round}");
    }
    for _ in 0..benchmark_count("SEEX_VIEWER_BENCH_ZOOM_ROUND_TRIPS", 30) {
        for factor in [1.25, 1. / 1.25] {
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.zoom_from_keyboard(factor, cx);
                })
                .expect("Viewer should remain open");
            cx.executor().advance_clock(Duration::from_millis(101));
            cx.run_until_parked();
            wait_for_benchmark(window, &cx, active_curves_are_settled);
        }
    }
    println!("SEEX_RSS_PHASE cycles_done");
    std::thread::sleep(Duration::from_millis(200));
    println!("SEEX_RSS_PHASE final");
    std::thread::sleep(Duration::from_millis(200));
}
