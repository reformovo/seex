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
    let (root, project_id, first_run_id) = fixture_with_complete_runs(6, 20);
    let (pending_root, _, _) = fixture_with_extent(10);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    cx.simulate_resize(size(px(2_560.), px(1_800.)));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 6);
    window
        .update(&mut cx, |viewer, _, cx| {
            let source_id = first_source_id(viewer, cx);
            for run_index in 1..20 {
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
        viewer.session_snapshot(cx).views.active().runs.len() == 20
            && viewer.session_snapshot(cx).views.active().panels.len() == 6
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .all(|panel| {
                    panel.detail.as_ref().is_some_and(|detail| {
                        detail.series.len() == 20
                            && detail.point_budget
                                == panel.physical_width.saturating_mul(2).clamp(2_000, 10_000)
                            && detail.series.iter().all(|series| {
                                series.evidence.points.len() <= detail.point_budget as usize + 2
                            })
                    })
                })
    });

    assert!(cx.debug_bounds("bottom-inspector").is_some());
    assert!(cx.debug_bounds("metric-track:metric-0").is_some());
    assert!(cx.debug_bounds("metric-track:metric-5").is_some());

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
    assert_eq!((run_count, panel_count), (20, 6));
    assert!(cx.debug_bounds("metric-track:metric-0").is_some());
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .find(|source| source.source_id == pending_source_id)
            .is_some_and(|source| source.status == SourceStatus::Ready)
    });
}
