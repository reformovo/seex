use std::path::PathBuf;
use std::time::Duration;

use gpui::{TestAppContext, px, size};

use super::super::test_support::*;
use super::*;

fn benchmark_root() -> PathBuf {
    std::env::var_os("SEEX_VIEWER_BENCH_ROOT")
        .map(PathBuf::from)
        .expect("SEEX_VIEWER_BENCH_ROOT must name the prepared compact fixture")
}

fn active_details_are_settled(viewer: &ViewerApp, cx: &App) -> bool {
    let Some(viewport) = viewer.active_navigation(cx).selected_viewport() else {
        return false;
    };
    let snapshot = viewer.session_snapshot(cx);
    let active = snapshot.views.active();
    active.panels.len() == 2
        && active.panels.iter().all(|panel| {
            panel.detail.is_some()
                && !panel.is_pending(ReadKind::Detail)
                && panel.requested_detail_viewport == Some(viewport)
        })
}

#[test]
fn compact_fixture_refuses_unrelated_data() {
    let root = tempfile::tempdir().expect("test directory should be created");
    std::fs::write(root.path().join("keep"), "user data").expect("sentinel should be written");

    let error =
        prepare_viewer_performance_fixture(root.path()).expect_err("fixture must not replace data");

    assert!(error.to_string().contains("non-empty"));
}

#[test]
#[ignore = "creates the retained 4 Run x 2 Metric x 100k Viewer fixture"]
fn prepare_compact_viewer_fixture() -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        std::hint::black_box(!cfg!(debug_assertions)),
        "fixture preparation requires --release"
    );
    prepare_viewer_performance_fixture(&benchmark_root())
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
        .expect("compact Viewer fixture should open");
    let project_id = reader.projects().expect("fixture projects should load")[0]
        .project_id
        .clone();
    let runs = reader.runs(&project_id).expect("fixture Runs should load");
    let metrics = reader
        .metrics(&runs[0])
        .expect("fixture Metrics should load");
    assert_eq!((runs.len(), metrics.len()), (4, 2));

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
    wait_for_viewer(window, &cx, active_details_are_settled);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.dispatch_workbench_command(WorkbenchCommand::DuplicateActiveView, cx);
        })
        .expect("Viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.views().len() == 2
            && active_details_are_settled(viewer, cx)
    });

    println!("SEEX_RSS_PHASE warm");
    for _ in 0..3 {
        for factor in [1.25, 1. / 1.25] {
            window
                .update(&mut cx, |viewer, _, cx| {
                    viewer.zoom_from_keyboard(factor, cx);
                })
                .expect("Viewer should remain open");
            cx.executor().advance_clock(Duration::from_millis(101));
            cx.run_until_parked();
            wait_for_viewer(window, &cx, active_details_are_settled);
        }
    }
    println!("SEEX_RSS_PHASE cycles_done");
    std::thread::sleep(Duration::from_millis(200));
    println!("SEEX_RSS_PHASE final");
    std::thread::sleep(Duration::from_millis(200));
}
