use gpui::{TestAppContext, px, size};

use super::super::test_support::*;
use super::*;

#[gpui::test]
#[ignore = "hardware-sensitive representative release workbench validation"]
fn representative_workbench_stays_responsive_while_a_source_is_pending(cx: &mut TestAppContext) {
    assert!(
        std::hint::black_box(!cfg!(debug_assertions)),
        "workbench validation requires --release"
    );
    let (root, project_id, first_run_id) = fixture_with_complete_runs(6, 10);
    let (pending_root, _, _) = fixture_with_extent(10);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    cx.simulate_resize(size(px(2_560.), px(1_800.)));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 6);
    window
        .update(&mut cx, |viewer, _, cx| {
            let source_id = first_source_id(viewer, cx);
            for run_index in 1..10 {
                viewer.toggle_run(
                    RunRef::new(
                        source_id.clone(),
                        project_id.clone(),
                        RunId::from_string(format!(
                            "run-{run_index}-with-a-very-long-identifier-that-requires-horizontal-scrolling"
                        )),
                    ),
                    cx,
                );
            }
            for metric_index in 0..6 {
                viewer.select_metric(
                    MetricKey::from_string(format!("metric-{metric_index}")),
                    cx,
                );
            }
            viewer.show_metric_inspector(
                &MetricPanelId::from_string("metric-0"),
                cx,
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.active().runs.len() == 10
            && viewer.session_snapshot(cx).views.active().panels.len() == 6
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .all(|panel| {
                    panel.detail.as_ref().is_some_and(|detail| {
                        detail.series.len() == 10
                            && detail.point_budget
                                == panel.logical_width.saturating_mul(2).clamp(512, 5_000)
                            && detail.series.iter().all(|series| {
                                series.returned_point_count <= u64::from(detail.point_budget) + 2
                            })
                    })
                })
    });

    assert!(cx.debug_bounds("bottom-inspector").is_some());
    assert!(cx.debug_bounds("metric-track:metric-0").is_some());
    assert!(cx.debug_bounds("metric-track:metric-5").is_some());

    for scale in [1_u32, 2, 3] {
        window
            .update(&mut cx, |viewer, _, cx| {
                viewer.workspace.update(cx, |workspace, cx| {
                    workspace.update_overview_widths(400., 400 * scale, cx);
                    assert_eq!(
                        workspace.track_viewport.borrow().physical_width,
                        400 * scale
                    );
                });
            })
            .expect("viewer should remain open");
    }

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.dispatch_workbench_command(WorkbenchCommand::DuplicateActiveView, cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .views
                .views()
                .len())
            .expect("viewer should remain open"),
        2
    );

    let before = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .active_navigation(cx)
                .brush()
                .map(|brush| brush.selected())
        })
        .expect("viewer should remain open")
        .expect("representative View should have a shared viewport");
    let pending_source_id = DataSourceId::from_path(pending_root.path());
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.open_source(pending_root.path().to_path_buf(), cx);
            assert!(matches!(
                viewer
                    .session_snapshot(cx)
                    .sources
                    .iter()
                    .find(|source| source.source_id == pending_source_id)
                    .map(|source| &source.status),
                Some(SourceStatus::Loading)
            ));
            viewer.zoom_from_keyboard(1.25, cx);
        })
        .expect("viewer should remain open");
    let (after, run_count, panel_count) = window
        .read_with(&cx, |viewer, cx| {
            (
                viewer
                    .active_navigation(cx)
                    .brush()
                    .map(|brush| brush.selected()),
                viewer.session_snapshot(cx).views.active().runs.len(),
                viewer.session_snapshot(cx).views.active().panels.len(),
            )
        })
        .expect("viewer should remain open");
    let after = after.expect("shared viewport should remain available");
    assert_ne!(after, before);
    assert_eq!((run_count, panel_count), (10, 6));
    let (source_resources, panel_resources) = window
        .read_with(&cx, |viewer, cx| {
            let session = viewer.session.read(cx);
            (
                session.sources.resource_snapshot(),
                session.panel_reads.resource_snapshot(),
            )
        })
        .expect("viewer should remain open");
    let resources_pass = source_resources.peak_concurrent_reads <= 4
        && panel_resources.stale_retained_snapshots == 0;
    println!(
        "SEEX_PERF {{\"schema_version\":2,\"record_type\":\"check\",\
         \"domain\":\"viewer\",\"check\":\"workload_matrix\",\
         \"passed\":{resources_pass},\"detail\":\"runs=10,metrics=6,views=2,\
         peak_concurrency={},superseded={},stale={},retained={}\"}}",
        source_resources.peak_concurrent_reads,
        source_resources.superseded_reads,
        panel_resources.stale_reads,
        panel_resources.stale_retained_snapshots,
    );
    assert!(resources_pass);
    assert!(cx.debug_bounds("metric-track:metric-0").is_some());
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .find(|source| source.source_id == pending_source_id)
            .is_some_and(|source| source.status == SourceStatus::Ready)
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(101));
    cx.run_until_parked();
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.zoom_from_keyboard(1.25, cx);
            viewer.zoom_from_keyboard(1. / 1.25, cx);
        })
        .expect("viewer should remain open");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(101));
    cx.run_until_parked();
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    std::thread::sleep(std::time::Duration::from_millis(200));

    println!("SEEX_RSS_PHASE warm");
    for _ in 0..30 {
        window
            .update(&mut cx, |viewer, _, cx| {
                viewer.zoom_from_keyboard(1.25, cx);
                viewer.zoom_from_keyboard(1. / 1.25, cx);
            })
            .expect("viewer should remain open");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    println!("SEEX_RSS_PHASE cycles_done");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(101));
    cx.run_until_parked();
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    let (runs, panels) = window
        .read_with(&cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            let active = snapshot.views.active();
            (active.runs.len(), active.panels.len())
        })
        .expect("viewer should remain open");
    assert_eq!((runs, panels), (10, 6));
    println!("SEEX_RSS_PHASE final");
    std::thread::sleep(std::time::Duration::from_millis(200));
}
