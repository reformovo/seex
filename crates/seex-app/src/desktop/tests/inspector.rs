use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, point, px, size};

use super::super::test_support::*;
use super::*;
use crate::desktop::{ShowMetricInspector, ToggleBottomInspector, ToggleMetricSidebar};

#[gpui::test]
fn bottom_inspector_can_close_after_switching_to_an_empty_view(cx: &mut TestAppContext) {
    let (window, mut cx) = open_viewer(cx, None);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
            viewer.bottom_inspector.update(cx, |inspector, _| {
                inspector.visible = true;
            });
            viewer.dispatch_workbench_command(WorkbenchCommand::CreateView, cx);
        })
        .expect("viewer should remain open");
    window
        .read_with(&cx, |viewer, cx| {
            assert!(
                viewer
                    .session_snapshot(cx)
                    .views
                    .active()
                    .selected_panel_id
                    .is_none()
            );
            assert!(viewer.inspector_visible(cx));
        })
        .expect("viewer should remain open");

    let toggle = cx
        .debug_bounds("toggle-bottom-inspector")
        .expect("bottom inspector toggle should remain available");
    cx.simulate_click(toggle.center(), Modifiers::default());

    assert!(
        !window
            .read_with(&cx, |viewer, cx| viewer.inspector_visible(cx))
            .expect("viewer should remain open")
    );
}

#[gpui::test]
fn metric_click_opens_a_resizable_inspector_without_gesture_toggles(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_extent(100);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, root.path().to_path_buf());
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    assert!(cx.debug_bounds("bottom-inspector").is_none());

    let metric_row = cx
        .debug_bounds("metric-sidebar-row:loss")
        .expect("Metric sidebar row should render");
    cx.simulate_click(metric_row.center(), Modifiers::default());
    let inspector = cx
        .debug_bounds("bottom-inspector")
        .expect("Bottom inspector should open");
    let scroll = cx
        .debug_bounds("bottom-inspector-scroll")
        .expect("Inspector content should own a scroll viewport");
    assert!(cx.debug_bounds("bottom-inspector-header").is_none());
    assert!(cx.debug_bounds("inspector-context").is_none());
    assert!(cx.debug_bounds("close-inspector").is_none());
    assert_eq!(scroll.origin.x, inspector.origin.x);
    assert_eq!(scroll.size.width, inspector.size.width);
    assert!(scroll.size.height < inspector.size.height);
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .views
            .active()
            .panels
            .first()
            .is_some_and(|panel| {
                panel.inspector.is_some() && !panel.is_pending(ReadKind::Inspector)
            })
    });
    let table_header = cx
        .debug_bounds("inspector-table-header")
        .expect("Inspector table header should render after its data loads");
    assert_eq!(table_header.origin.x, inspector.origin.x);
    assert_eq!(table_header.size.width, inspector.size.width);
    cx.simulate_resize(size(px(600.), px(520.)));
    cx.run_until_parked();
    let scroll = cx
        .debug_bounds("bottom-inspector-scroll")
        .expect("Inspector scroll viewport should remain rendered");
    let table = cx
        .debug_bounds("inspector-table")
        .expect("Inspector table should remain rendered");
    assert!(table.size.width > scroll.size.width);
    cx.simulate_resize(size(px(1_000.), px(1_000.)));
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            let run = viewer.session_snapshot(cx).views.active().panels[0]
                .inspector
                .as_ref()
                .expect("exact inspector evidence should load")
                .runs
                .first()
                .cloned()
                .expect("fixture Run should be present");
            let summary = run
                .summary
                .as_ref()
                .expect("fixture should have a metric summary");
            assert_eq!(summary.effective_count, 2);
            assert_eq!(
                (
                    summary.last_step.value(),
                    summary.last_value_f64,
                    summary.min_value_f64,
                    summary.max_value_f64,
                ),
                (100, 0.5, 0.5, 1.)
            );
            assert_eq!(run.evidence.last_step.map(|step| step.value()), Some(100));
            assert_eq!(run.evidence.last_value_f64, Some(0.5));
            assert_eq!(run.evidence.completeness, EvidenceCompleteness::Complete);
        })
        .expect("viewer should remain open");

    let inspector_before = window
        .read_with(&cx, |viewer, cx| {
            Arc::clone(
                viewer.session_snapshot(cx).views.active().panels[0]
                    .inspector
                    .as_ref()
                    .expect("inspector snapshot should remain available"),
            )
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.zoom_from_keyboard(1.25, cx);
        })
        .expect("viewer should remain open");
    window
        .read_with(&cx, |viewer, cx| {
            let panel = viewer.session_snapshot(cx).views.active().panels[0].clone();
            assert!(panel.inspector_generation.is_none());
            assert!(Arc::ptr_eq(
                &inspector_before,
                panel
                    .inspector
                    .as_ref()
                    .expect("zoom should retain the whole-series inspector"),
            ));
        })
        .expect("viewer should remain open");

    assert!(cx.debug_bounds("inspector-summary").is_none());
    assert!(cx.debug_bounds("inspector-ranking").is_none());
    assert!(cx.debug_bounds("inspector-evidence").is_none());
    for selector in [
        "inspector-header:run",
        "inspector-header:last-value",
        "inspector-header:min",
        "inspector-header:max",
        "inspector-header:locked",
        "inspector-header:hover",
        "inspector-header:count",
        "inspector-header:last-step",
        "inspector-header:status",
        "inspector-header:evidence",
        "inspector-header:project",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "every inspector header should be sortable: {selector}",
        );
    }
    assert!(cx.debug_bounds("inspector-column-resize:project").is_none());
    let resized_header = cx
        .debug_bounds("inspector-header:last-value")
        .expect("Last value header should render");
    let column_resize = cx
        .debug_bounds("inspector-column-resize:last-value")
        .expect("Last value column resize boundary should render");
    let resize_midpoint = point(column_resize.center().x + px(18.), column_resize.center().y);
    let resize_target = point(column_resize.center().x + px(36.), column_resize.center().y);
    cx.simulate_mouse_down(
        column_resize.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    assert_eq!(
        window
            .read_with(&cx, |viewer, inner_cx| {
                viewer
                    .bottom_inspector
                    .read(inner_cx)
                    .column_resize
                    .map(|resize| resize.column)
            })
            .expect("viewer should remain open"),
        Some(InspectorColumn::LastValue)
    );
    cx.simulate_mouse_move(
        resize_midpoint,
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    assert_eq!(
        cx.debug_bounds("inspector-column-resize:last-value")
            .expect("column resize boundary should follow intermediate pointer movement")
            .center()
            .x,
        resize_midpoint.x,
    );
    assert_eq!(
        window
            .read_with(&cx, |viewer, inner_cx| viewer
                .bottom_inspector
                .read(inner_cx)
                .column_resize
                .map(|resize| resize.column))
            .expect("viewer should remain open"),
        Some(InspectorColumn::LastValue),
        "column resize should remain active across consecutive pointer movements",
    );
    cx.simulate_mouse_move(resize_target, Some(MouseButton::Left), Modifiers::default());
    let moved_resize = cx
        .debug_bounds("inspector-column-resize:last-value")
        .expect("column resize boundary should remain under the pointer while dragging");
    assert_eq!(moved_resize.center().x, resize_target.x);
    cx.simulate_mouse_up(resize_target, MouseButton::Left, Modifiers::default());
    assert!(
        window
            .read_with(&cx, |viewer, inner_cx| viewer
                .bottom_inspector
                .read(inner_cx)
                .column_resize
                .is_none())
            .expect("viewer should remain open"),
        "releasing a column boundary should clear its active resize state",
    );
    assert_eq!(
        cx.debug_bounds("inspector-header:last-value")
            .expect("resized Last value header should remain rendered")
            .size
            .width,
        resized_header.size.width + px(36.)
    );
    let resized_boundary = cx
        .debug_bounds("inspector-column-resize:last-value")
        .expect("resized column boundary should remain interactive");
    let outside_header = point(
        resized_boundary.center().x,
        resized_boundary.bottom() + px(16.),
    );
    cx.simulate_mouse_down(
        resized_boundary.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_up(outside_header, MouseButton::Left, Modifiers::default());
    assert!(
        window
            .read_with(&cx, |viewer, inner_cx| viewer
                .bottom_inspector
                .read(inner_cx)
                .column_resize
                .is_none())
            .expect("viewer should remain open"),
        "releasing outside the column boundary should clear its active resize state",
    );
    let minimum = cx
        .debug_bounds("inspector-header:min")
        .expect("Min header should be sortable");
    cx.simulate_click(minimum.center(), Modifiers::default());
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer.bottom_inspector.read(cx).sort)
            .expect("viewer should remain open"),
        Some(InspectorSort {
            column: InspectorColumn::Minimum,
            direction: InspectorSortDirection::Ascending,
        })
    );
    let maximum = cx
        .debug_bounds("inspector-header:max")
        .expect("Max header should be sortable");
    cx.simulate_click(maximum.center(), Modifiers::default());
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer.bottom_inspector.read(cx).sort)
            .expect("viewer should remain open"),
        Some(InspectorSort {
            column: InspectorColumn::Maximum,
            direction: InspectorSortDirection::Ascending,
        })
    );
    cx.simulate_click(maximum.center(), Modifiers::default());
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer.bottom_inspector.read(cx).sort)
            .expect("viewer should remain open"),
        Some(InspectorSort {
            column: InspectorColumn::Maximum,
            direction: InspectorSortDirection::Descending,
        })
    );
    cx.simulate_click(maximum.center(), Modifiers::default());
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer.bottom_inspector.read(cx).sort)
            .expect("viewer should remain open"),
        None
    );

    let previous_height = window
        .read_with(&cx, |viewer, cx| viewer.bottom_inspector.read(cx).height)
        .expect("viewer should remain open");
    let inspector_resize = cx
        .debug_bounds("bottom-inspector-resize")
        .expect("Bottom inspector resize boundary should render");
    let resize_target = point(
        inspector_resize.center().x,
        inspector_resize.center().y + px(140.),
    );
    cx.simulate_mouse_down(
        inspector_resize.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(resize_target, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(resize_target, MouseButton::Left, Modifiers::default());
    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer.bottom_inspector.read(cx).height)
            .expect("viewer should remain open")
            < previous_height
    );
    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer.bottom_inspector.read(cx).height)
            .expect("viewer should remain open")
            < px(120.)
    );
    let retained_height = window
        .read_with(&cx, |viewer, cx| viewer.bottom_inspector.read(cx).height)
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, window, _| viewer.focus.focus(window))
        .expect("viewer should remain open");
    cx.dispatch_action(ToggleBottomInspector);
    assert!(
        !window
            .read_with(&cx, |viewer, cx| viewer.inspector_visible(cx))
            .expect("viewer should remain open")
    );
    cx.dispatch_action(ShowMetricInspector);
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer.bottom_inspector.read(cx).height)
            .expect("viewer should remain open"),
        retained_height
    );
    let expanded_metric_width = cx
        .debug_bounds("metric-sidebar-row:loss")
        .expect("Metric sidebar should render")
        .size
        .width;
    cx.dispatch_action(ToggleMetricSidebar);
    let compact_metric_width = cx
        .debug_bounds("metric-sidebar-row:loss")
        .expect("compact Metric sidebar should render")
        .size
        .width;
    assert!(compact_metric_width < expanded_metric_width);
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .len())
            .expect("viewer should remain open"),
        1
    );

    let panel_id = MetricPanelId::from_string("loss");
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.bottom_inspector.update(cx, |inspector, _| {
                inspector.visible = false;
            });
            cx.notify();
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    let track = cx
        .debug_bounds("metric-canvas:loss")
        .expect("Metric chart canvas should render");
    cx.simulate_mouse_down(track.center(), MouseButton::Left, Modifiers::default());
    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer.workspace.read(cx).drag.is_some())
            .expect("viewer should remain open")
    );
    cx.simulate_mouse_up(track.center(), MouseButton::Left, Modifiers::default());
    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer.workspace.read(cx).drag.is_none())
            .expect("viewer should remain open")
    );
    window
        .read_with(&cx, |viewer, cx| {
            assert!(viewer.inspector_visible(cx));
            assert!(viewer.interaction_snapshot(cx).locked_cursor.is_some());
        })
        .expect("viewer should remain open");

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.bottom_inspector.update(cx, |inspector, _| {
                inspector.visible = false;
            });
            viewer.workspace.update(cx, |workspace, _| {
                workspace.drag = Some(DragGesture::Detail {
                    panel_id: panel_id.clone(),
                    origin_x: 100.,
                    last_x: 120.,
                    moved: true,
                });
            });
            viewer.finish_moved_track_drag(cx);
            viewer.workspace.update(cx, |workspace, _| {
                workspace.drag = Some(DragGesture::BrushWindow { last_axis: 10. });
            });
            viewer.finish_drag(cx);
            viewer.zoom_from_keyboard(1.25, cx);
        })
        .expect("viewer should remain open");
    assert!(
        !window
            .read_with(&cx, |viewer, cx| viewer.inspector_visible(cx))
            .expect("viewer should remain open")
    );
}

#[gpui::test]
fn inspector_table_contains_only_visible_runs(cx: &mut TestAppContext) {
    let (root, project_id, first_run_id) = fixture_with_complete_runs(1, 4);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, root.path().to_path_buf());
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 1);
    let second = window
        .update(&mut cx, |viewer, _, cx| {
            let source_id = first_source_id(viewer, cx);
            let additional = (1..4)
                .map(|index| {
                    RunRef::new(
                        source_id.clone(),
                        project_id.clone(),
                        RunId::from_string(format!(
                            "run-{index}-with-a-very-long-identifier-that-requires-horizontal-scrolling"
                        )),
                    )
                })
                .collect::<Vec<_>>();
            for run in &additional {
                viewer.toggle_run(run.clone(), cx);
            }
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
            additional[0].clone()
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.active().panels[0]
            .detail
            .as_ref()
            .is_some_and(|snapshot| snapshot.series.len() == 4)
    });
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.bottom_inspector.update(cx, |inspector, _| {
                inspector.height = px(120.);
            });
            viewer.show_metric_inspector(&MetricPanelId::from_string("metric-0"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.active().panels[0]
            .inspector
            .as_ref()
            .is_some_and(|snapshot| snapshot.runs.len() == 4)
    });
    let first_row = cx
        .debug_bounds(
            "inspector-row:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling",
        )
        .expect("first visible Run should have an inspector row");
    let second_row = cx
        .debug_bounds(
            "inspector-row:run-1-with-a-very-long-identifier-that-requires-horizontal-scrolling",
        )
        .expect("second visible Run should have an inspector row");
    let scroll = cx
        .debug_bounds("bottom-inspector-scroll")
        .expect("inspector scroll viewport should render");
    let prepare_count = |viewer: &ViewerApp, cx: &App| {
        viewer
            .workspace
            .read(cx)
            .track_charts
            .get(&MetricPanelId::from_string("metric-0"))
            .expect("Metric chart should be cached")
            .read(cx)
            .prepare_count()
    };
    cx.simulate_mouse_move(
        point(scroll.origin.x + px(40.), first_row.center().y),
        None,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .interaction_snapshot(cx)
                    .emphasized_run
                    .as_ref()
                    .is_some_and(|run| {
                    run.run_id.as_str()
                        == "run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling"
                })
            })
            .expect("viewer should remain open")
    );
    let first_run = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .interaction_snapshot(cx)
                .emphasized_run
                .expect("first Run should be emphasized")
        })
        .expect("viewer should remain open");
    let first_prepare = window
        .read_with(&cx, prepare_count)
        .expect("viewer should remain open");
    let sticky_run = cx
        .debug_bounds("inspector-sticky-run:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling")
        .expect("Run cell should render in the fixed column");
    let sticky_region =
        SharedString::from(format!("inspector-sticky-run:{}", first_run.cache_key()));
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.handle_bottom_inspector_event(
                &BottomInspectorEvent::HoveredRun {
                    run: first_run.clone(),
                    region: sticky_region,
                    hovered: false,
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    cx.executor().advance_clock(Duration::from_millis(8));
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .interaction_snapshot(cx)
                .emphasized_run)
            .expect("viewer should remain open"),
        Some(first_run.clone()),
        "a sub-frame gap should retain the current Inspector emphasis",
    );
    assert_eq!(
        window
            .read_with(&cx, prepare_count)
            .expect("viewer should remain open"),
        first_prepare,
        "the grace interval should not prepare an intermediate un-emphasized chart",
    );

    let visible_data_x = sticky_run.right() + px(40.);
    cx.simulate_mouse_move(
        point(visible_data_x, second_row.center().y),
        None,
        Modifiers::default(),
    );
    cx.run_until_parked();
    let second_run = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .interaction_snapshot(cx)
                .emphasized_run
                .expect("second Run should replace the first emphasis")
        })
        .expect("viewer should remain open");
    assert_ne!(second_run, first_run);
    let second_prepare = window
        .read_with(&cx, prepare_count)
        .expect("viewer should remain open");
    assert_eq!(second_prepare, first_prepare + 1);
    cx.executor().advance_clock(Duration::from_millis(16));
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .interaction_snapshot(cx)
                .emphasized_run)
            .expect("viewer should remain open"),
        Some(second_run),
        "an expired leave from the previous row must not clear the new emphasis",
    );
    assert_eq!(
        window
            .read_with(&cx, prepare_count)
            .expect("viewer should remain open"),
        second_prepare,
    );

    cx.simulate_mouse_move(
        point(visible_data_x, first_row.center().y),
        None,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .interaction_snapshot(cx)
                .emphasized_run)
            .expect("viewer should remain open"),
        Some(first_run.clone()),
        "Inspector emphasis should hand off in the opposite vertical direction",
    );
    let upward_prepare = window
        .read_with(&cx, prepare_count)
        .expect("viewer should remain open");
    assert_eq!(upward_prepare, second_prepare + 1);

    cx.simulate_mouse_move(sticky_run.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, prepare_count)
            .expect("viewer should remain open"),
        upward_prepare,
        "crossing between Inspector regions for one Run should not prepare the chart again",
    );
    let analysis_tab = cx
        .debug_bounds("analysis-tab")
        .expect("analysis workspace should render");
    cx.simulate_mouse_move(analysis_tab.center(), None, Modifiers::default());
    cx.executor().advance_clock(Duration::from_millis(15));
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .interaction_snapshot(cx)
                .emphasized_run)
            .expect("viewer should remain open"),
        Some(first_run),
    );
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .interaction_snapshot(cx)
                .emphasized_run
                .is_none())
            .expect("viewer should remain open"),
        "leaving the Inspector should clear emphasis after one frame",
    );
    assert_eq!(
        window
            .read_with(&cx, prepare_count)
            .expect("viewer should remain open"),
        upward_prepare + 1,
    );
    let sticky_x = sticky_run.origin.x;
    let sticky_y = sticky_run.origin.y;
    let row_y = first_row.origin.y;
    let header_y = cx
        .debug_bounds("inspector-table-header")
        .expect("fixed inspector header should render")
        .origin
        .y;
    let scroll_position = point(scroll.origin.x + px(300.), first_row.center().y);
    cx.simulate_event(ScrollWheelEvent {
        position: scroll_position,
        delta: ScrollDelta::Pixels(point(px(-500.), px(0.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
    assert!(
        window
            .read_with(&cx, |viewer, inner_cx| {
                viewer
                    .bottom_inspector
                    .read(inner_cx)
                    .horizontal_scroll
                    .offset()
                    .x
                    < px(0.)
            })
            .expect("viewer should remain open")
    );
    assert_eq!(
        cx.debug_bounds("inspector-sticky-run:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling")
            .expect("fixed Run cell should remain rendered")
            .origin
            .x,
        sticky_x
    );
    cx.simulate_event(ScrollWheelEvent {
        position: scroll_position,
        delta: ScrollDelta::Pixels(point(px(0.), px(-500.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
    assert!(
        window
            .read_with(&cx, |viewer, inner_cx| {
                viewer
                    .bottom_inspector
                    .read(inner_cx)
                    .vertical_scroll
                    .offset()
                    .y
                    < px(0.)
            })
            .expect("viewer should remain open")
    );
    let scrolled_sticky = cx
        .debug_bounds("inspector-sticky-run:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling")
        .expect("Run cell should remain rendered after vertical scrolling");
    let scrolled_row = cx
        .debug_bounds(
            "inspector-row:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling",
        )
        .expect("data row should remain rendered after vertical scrolling");
    assert_eq!(
        scrolled_sticky.origin.y - sticky_y,
        scrolled_row.origin.y - row_y
    );
    assert_eq!(
        cx.debug_bounds("inspector-table-header")
            .expect("header should remain rendered after vertical scrolling")
            .origin
            .y,
        header_y
    );

    window
        .update(&mut cx, |viewer, _, cx| viewer.toggle_run(second, cx))
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.active_visible_runs(cx).len() == 3
            && viewer.session_snapshot(cx).views.active().panels[0]
                .inspector
                .as_ref()
                .is_some_and(|snapshot| snapshot.runs.len() == 3)
    });

    assert!(
        cx.debug_bounds(
            "inspector-row:run-0-with-a-very-long-identifier-that-requires-horizontal-scrolling"
        )
        .is_some()
    );
    window
        .read_with(&cx, |viewer, cx| {
            let visible = viewer.active_visible_runs(cx);
            let inspector = viewer.session_snapshot(cx).views.active().panels[0]
                .inspector
                .clone()
                .expect("inspector should remain available");
            assert!(
                inspector
                    .runs
                    .iter()
                    .all(|run| visible.contains(&run.run_ref))
            );
        })
        .expect("viewer should remain open");
}
