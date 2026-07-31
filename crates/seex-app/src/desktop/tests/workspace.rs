use gpui::{Modifiers, ScrollDelta, TestAppContext, TouchPhase, point, px, size};

use super::super::test_support::*;
use super::*;
use std::collections::BTreeMap;

use crate::desktop::{ClearLockedCursor, ZoomIn};

#[test]
fn metric_metadata_reports_run_and_drawable_counts() {
    assert_eq!(metric_metadata(9, 0, false, false), "9 Runs");
    assert_eq!(metric_metadata(9, 0, false, true), "9 Runs · loading");
    assert_eq!(metric_metadata(9, 8, true, false), "9 Runs · 8 drawable");
}

#[gpui::test]
fn metric_rows_ignore_unavailable_selections(cx: &mut TestAppContext) {
    let (root, _, _) = fixture_with_complete_runs(1, 9);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);

    let source_id = DataSourceId::from_path(root.path());
    let ghost = RunRef::new(
        DataSourceId::from_string("missing-source"),
        ProjectId::from_string("missing-project"),
        RunId::from_string("missing-run"),
    );
    window
        .update(&mut cx, |viewer, _, cx| {
            let mut selected_runs = viewer
                .session_snapshot(cx)
                .sources
                .iter()
                .find(|source| source.source_id == source_id)
                .expect("fixture source should remain available")
                .catalog
                .runs
                .iter()
                .map(|run| {
                    RunRef::new(
                        source_id.clone(),
                        run.project_id.clone(),
                        run.run_id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(selected_runs.len(), 9);
            selected_runs.push(ghost.clone());
            viewer.session.update(cx, |session, session_cx| {
                session.views.active_mut().runs = selected_runs;
                session.publish_snapshot();
                session_cx.notify();
            });
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);

    let panel_id = MetricPanelId::from_string("metric-0");
    window
        .read_with(&cx, |viewer, cx| {
            assert_eq!(viewer.session_snapshot(cx).views.active().runs.len(), 10);
            assert_eq!(
                viewer.workspace.read(cx).metric_row_counts(&panel_id),
                Some((9, 9)),
            );
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn metric_without_drawable_evidence_renders_an_empty_chart(cx: &mut TestAppContext) {
    let (root, project_id, first_run_id) = fixture_with_runs(1, 2);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id.clone(), first_run_id.clone(), 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);

    let second = RunRef::new(
        DataSourceId::from_path(root.path()),
        project_id,
        RunId::from_string("run-1-with-a-very-long-identifier-that-requires-horizontal-scrolling"),
    );
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.toggle_run(second.clone(), cx);
            viewer.toggle_run(
                RunRef::new(
                    DataSourceId::from_path(root.path()),
                    second.project_id.clone(),
                    first_run_id,
                ),
                cx,
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.active_visible_runs(cx) == [second.clone()]
    });
    cx.run_until_parked();

    let panel_id = MetricPanelId::from_string("metric-0");
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .workspace
                .read(cx)
                .metric_row_counts(&panel_id))
            .expect("viewer should remain open"),
        Some((1, 0)),
    );
    assert!(cx.debug_bounds("empty-metric-chart:metric-0").is_some());
}

#[gpui::test]
fn shared_timeline_unions_extents_from_multiple_sources(cx: &mut TestAppContext) {
    let (first, first_project, first_run) = fixture_with_extent(10);
    let (second, second_project, second_run) = fixture_with_extent(20);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(first.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.open_source(second.path().to_path_buf(), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).sources.iter().len() == 2
            && viewer.session_snapshot(cx).sources.iter().all(|source| {
                source.status == SourceStatus::Ready && !source.catalog.projects.is_empty()
            })
    });

    for (root, project_id, run_id) in [
        (first.path(), first_project, first_run),
        (second.path(), second_project, second_run),
    ] {
        let source_id = DataSourceId::from_path(root);
        window
            .update(&mut cx, |viewer, _, cx| {
                viewer.activate_tree_project(source_id.clone(), project_id.clone(), cx);
            })
            .expect("viewer should remain open");
        wait_for_viewer(window, &cx, |viewer, cx| {
            viewer
                .session_snapshot(cx)
                .sources
                .iter()
                .find(|source| source.source_id == source_id)
                .is_some_and(|source| source.catalog.runs.iter().any(|run| run.run_id == run_id))
        });
        window
            .update(&mut cx, |viewer, _, cx| {
                viewer.toggle_tree_run(
                    RunRef::new(source_id.clone(), project_id.clone(), run_id.clone()),
                    cx,
                );
            })
            .expect("viewer should remain open");
    }
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.available_metric_keys(cx).len() == 1
    });
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .views
            .active()
            .panels
            .first()
            .and_then(|panel| panel.overview.as_deref())
            .is_some_and(|snapshot| snapshot.series.len() == 2)
    });

    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .active_navigation(cx)
                    .brush()
                    .map(|brush| brush.home().end())
            })
            .expect("viewer should remain open"),
        Some(20.)
    );
}

#[gpui::test]
fn metrics_appended_after_initial_layout_all_receive_detail(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_complete_runs(2, 1);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 2);

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("metric-1"), cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            assert!(
                viewer
                    .workspace
                    .read(cx)
                    .track_viewport
                    .borrow()
                    .overscan
                    .contains(&1)
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        let Some(viewport) = viewer.active_navigation(cx).selected_viewport() else {
            return false;
        };
        viewer.session_snapshot(cx).views.active().panels.len() == 2
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .all(|panel| {
                    panel
                        .detail
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.series.len() == 1)
                        && !panel.is_pending(ReadKind::Detail)
                        && panel.requested_detail_viewport == Some(viewport)
                })
    });
}

#[gpui::test]
fn metric_picker_lists_only_metrics_not_already_in_the_view(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture(20);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 20);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
        })
        .expect("viewer should remain open");
    let add = cx
        .debug_bounds("add-metric")
        .expect("Add Metric control should render beside the brush");
    cx.simulate_click(add.center(), Modifiers::default());
    let metric_picker = cx
        .debug_bounds("metric-picker")
        .expect("Metric picker should open below the Brush row");
    let candidates = cx
        .debug_bounds("metric-candidates")
        .expect("Metric candidates should use their own scroll region");
    let late_before = cx
        .debug_bounds("metric-candidate:metric-19")
        .expect("late Metric candidate should be laid out");
    assert_eq!(late_before.size.height, px(24.));
    assert!(late_before.origin.y >= candidates.bottom());
    cx.simulate_event(ScrollWheelEvent {
        position: candidates.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-1_000.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    let late_after = cx
        .debug_bounds("metric-candidate:metric-19")
        .expect("late Metric candidate should remain laid out");
    assert!(late_after.origin.y < candidates.bottom());
    let axis = cx
        .debug_bounds("axis-picker")
        .expect("Axis picker should render beside Add Metric");
    let controls = cx
        .debug_bounds("brush-controls")
        .expect("Brush controls should own the fixed Metric label cell");
    let brush_row = cx
        .debug_bounds("brush-row")
        .expect("Brush row should render");
    assert_eq!(metric_picker.size.width, px(180.));
    assert_eq!(metric_picker.top(), brush_row.bottom() + px(4.));
    assert_eq!(metric_picker.left(), add.left());
    assert_eq!(axis.origin.x, controls.origin.x + px(4.));
    assert_eq!(add.right(), controls.right() - px(5.));
    cx.simulate_click(axis.center(), Modifiers::default());
    let axis_menu = cx
        .debug_bounds("axis-menu")
        .expect("Axis menu should open below the Brush row");
    assert_eq!(axis_menu.size.width, px(160.));
    assert_eq!(axis_menu.top(), brush_row.bottom() + px(4.));
    assert_eq!(axis_menu.left(), axis.left());
    window
        .read_with(&cx, |viewer, cx| {
            let workspace = viewer.workspace.read(cx);
            assert!(workspace.axis_picker_open);
            assert!(!workspace.metric_picker_open);
        })
        .expect("viewer should remain open");
    cx.simulate_click(add.center(), Modifiers::default());
    window
        .read_with(&cx, |viewer, cx| {
            let workspace = viewer.workspace.read(cx);
            assert!(!workspace.axis_picker_open);
            assert!(workspace.metric_picker_open);
        })
        .expect("viewer should remain open");
    assert!(cx.debug_bounds("metric-candidate:metric-0").is_none());
    let filter = cx
        .debug_bounds("metric-filter")
        .expect("Metric picker should expose search");
    cx.simulate_click(filter.center(), Modifiers::default());
    cx.simulate_keystrokes("2");
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .workspace
                    .read(cx)
                    .metric_filter
                    .read(cx)
                    .text()
                    .to_owned()
            })
            .expect("viewer should remain open"),
        "2"
    );
    let candidate = cx
        .debug_bounds("metric-candidate:metric-2")
        .expect("matching unadded Metric should be offered");

    cx.simulate_click(candidate.center(), Modifiers::default());
    cx.run_until_parked();

    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                !viewer.workspace.read(cx).metric_picker_open
                    && viewer
                        .session_snapshot(cx)
                        .views
                        .active()
                        .panels
                        .iter()
                        .any(|panel| panel.metric_key.as_str() == "metric-2")
            })
            .expect("viewer should remain open")
    );
}

#[gpui::test]
fn hover_and_locked_cursors_coexist_on_the_shared_ruler(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_extent(100);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    let ruler = cx
        .debug_bounds("ruler-hit-area")
        .expect("shared ruler hit area should render");
    let first = point(ruler.origin.x + ruler.size.width * 0.25, ruler.center().y);
    cx.simulate_mouse_move(first, None, Modifiers::default());
    cx.simulate_mouse_down(first, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(first, MouseButton::Left, Modifiers::default());
    let locked = window
        .read_with(&cx, |viewer, cx| {
            viewer.interaction_snapshot(cx).locked_cursor
        })
        .expect("viewer should remain open")
        .expect("ruler click should lock a cursor");
    let second = point(ruler.origin.x + ruler.size.width * 0.75, ruler.center().y);
    cx.simulate_mouse_move(second, None, Modifiers::default());

    let capsule = cx
        .debug_bounds("ruler-hover-tooltip")
        .expect("hover coordinate capsule should render");
    assert!(f32::from(capsule.center().x - second.x).abs() < 40.);
    assert_eq!(capsule.size.height, px(18.));
    assert!(cx.debug_bounds("track-hover-callout").is_some());
    let track_scroll = cx
        .debug_bounds("metric-track-scroll")
        .expect("Metric track viewport should render");
    let track = cx
        .debug_bounds("metric-track:loss")
        .expect("Metric track should render");
    let cursor_overlay = cx
        .debug_bounds("metric-cursor-overlay")
        .expect("Shared Metric cursor overlay should render");
    assert_eq!(cursor_overlay.origin.x, track.origin.x);
    assert_eq!(cursor_overlay.origin.y, track_scroll.origin.y);
    assert_eq!(cursor_overlay.bottom(), track_scroll.bottom());
    window
        .read_with(&cx, |viewer, cx| {
            let interaction = viewer.interaction_snapshot(cx);
            assert_eq!(interaction.locked_cursor, Some(locked));
            assert!(interaction.ruler_hover.is_some_and(|hover| hover != locked));
        })
        .expect("viewer should remain open");

    cx.dispatch_action(ClearLockedCursor);

    window
        .read_with(&cx, |viewer, cx| {
            let interaction = viewer.interaction_snapshot(cx);
            assert!(interaction.locked_cursor.is_none());
            assert!(interaction.ruler_hover.is_some());
        })
        .expect("viewer should remain open");

    window
        .update(&mut cx, |viewer, _, cx| {
            let emphasized_run = viewer.active_visible_runs(cx).into_iter().next();
            viewer.interaction.update(cx, |interaction, _| {
                interaction.set_locked_cursor(Some(locked));
                interaction.set_ruler_hover(None);
                interaction.set_emphasized_run(emphasized_run)
            });
            viewer.workspace.update(cx, |workspace, _| {
                workspace.track_hovers.clear();
            });
            cx.notify();
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    let callout = cx
        .debug_bounds("track-hover-callout")
        .expect("Sidebar hover should show one locked-cursor tooltip");
    let track = cx
        .debug_bounds("metric-track:loss")
        .expect("Metric track should render");
    assert!(callout.size.height < track.size.height);
    assert!(callout.size.width < px(160.));
    assert!(cx.debug_bounds("track-tooltip-color").is_some());
    assert!(cx.debug_bounds("track-tooltip-label").is_some());
}

#[gpui::test]
fn hover_frames_reuse_static_metric_chart_preparation(cx: &mut TestAppContext) {
    let (root, project_id, first_run_id) = fixture_with_complete_runs(3, 2);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 3);
    window
        .update(&mut cx, |viewer, _, cx| {
            let source_id = first_source_id(viewer, cx);
            viewer.toggle_run(
                RunRef::new(
                    source_id,
                    project_id,
                    RunId::from_string(
                        "run-1-with-a-very-long-identifier-that-requires-horizontal-scrolling",
                    ),
                ),
                cx,
            );
            for index in 0..3 {
                viewer.select_metric(MetricKey::from_string(format!("metric-{index}")), cx);
            }
        })
        .expect("viewer should remain open");
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        viewer.workspace.read(cx).track_charts.len() == 3
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .all(|panel| {
                    panel.detail.as_ref().is_some_and(|snapshot| {
                        snapshot.series.len() == 2 && !panel.is_pending(ReadKind::Detail)
                    })
                })
    });
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.show_metric_inspector(&MetricPanelId::from_string("metric-0"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.active().panels[0]
            .inspector
            .is_some()
    });
    cx.run_until_parked();
    let preparation_counts = |viewer: &ViewerApp, cx: &App| {
        viewer
            .workspace
            .read(cx)
            .track_charts
            .iter()
            .map(|(panel_id, chart)| (panel_id.as_str().to_owned(), chart.read(cx).prepare_count()))
            .collect::<BTreeMap<_, _>>()
    };
    let query_state = window
        .read_with(&cx, |viewer, cx| {
            (
                viewer.session.read(cx).next_generation,
                viewer.active_navigation(cx).selected_viewport(),
            )
        })
        .expect("viewer should remain open");
    let project = cx
        .debug_bounds("project-tree-row-0-0")
        .expect("Project row should render");
    cx.simulate_mouse_move(project.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-information-project").is_some());
    cx.simulate_click(project.center(), Modifiers::default());
    let run = cx
        .debug_bounds("project-tree-run-0-0-0")
        .expect("expanded Project should render Runs");
    cx.simulate_mouse_move(run.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer.interaction_snapshot(cx).emphasized_run.is_some()
            })
            .expect("viewer should remain open")
    );
    let ruler = cx
        .debug_bounds("ruler-hit-area")
        .expect("shared ruler should render");
    cx.simulate_mouse_move(ruler.center(), None, Modifiers::default());
    cx.run_until_parked();
    let before_ruler_hover = window
        .read_with(&cx, |viewer, cx| preparation_counts(viewer, cx))
        .expect("viewer should remain open");
    for index in 1..10 {
        let position = point(
            ruler.origin.x + ruler.size.width * (index as f32 / 10.),
            ruler.center().y,
        );
        cx.simulate_mouse_move(position, None, Modifiers::default());
    }
    let tooltip = cx
        .debug_bounds("track-hover-callout")
        .expect("Ruler hover should render one value per Metric");
    assert_eq!(tooltip.size.height, px(22.));
    let after = window
        .read_with(&cx, |viewer, cx| preparation_counts(viewer, cx))
        .expect("viewer should remain open");

    assert_eq!(after, before_ruler_hover);
    window
        .read_with(&cx, |viewer, cx| {
            assert!(viewer.project_sidebar.read(cx).hovered_project.is_none());
            assert!(viewer.interaction_snapshot(cx).emphasized_run.is_none());
            assert_eq!(viewer.session.read(cx).next_generation, query_state.0);
            assert_eq!(
                viewer.active_navigation(cx).selected_viewport(),
                query_state.1
            );
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn chart_pointer_drives_the_shared_hover_cursor_without_a_curve_hit(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_extent(100);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.session.update(cx, |session, session_cx| {
                session.views.active_mut().panels[0].row_height = 180.;
                session.publish_snapshot();
                session_cx.notify();
            });
            cx.notify();
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    let canvas = cx
        .debug_bounds("metric-canvas:loss")
        .expect("Metric chart should render");
    let pointer = point(
        canvas.origin.x + canvas.size.width * 0.25,
        canvas.bottom() - px(2.),
    );

    cx.simulate_mouse_move(pointer, None, Modifiers::default());

    assert!(cx.debug_bounds("ruler-hover-tooltip").is_some());
    window
        .read_with(&cx, |viewer, cx| {
            let interaction = viewer.interaction_snapshot(cx);
            assert!(interaction.ruler_hover.is_none());
            assert!(
                !viewer
                    .workspace
                    .read(cx)
                    .track_hovers
                    .contains_key(&MetricPanelId::from_string("loss"))
            );
            assert!(
                interaction.track_pointer_hover.as_ref().is_some_and(
                    |(panel_id, axis)| panel_id.as_str() == "loss" && axis.is_finite()
                )
            );
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn zooming_multiple_metrics_repaints_without_pointer_motion(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_complete_runs(3, 1);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 3);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
            viewer.select_metric(MetricKey::from_string("metric-1"), cx);
            viewer.select_metric(MetricKey::from_string("metric-2"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        let Some(viewport) = viewer.active_navigation(cx).selected_viewport() else {
            return false;
        };
        viewer.session_snapshot(cx).views.active().panels.len() == 3
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .all(|panel| {
                    panel.detail.is_some()
                        && !panel.is_pending(ReadKind::Detail)
                        && panel.requested_detail_viewport == Some(viewport)
                })
            && viewer.workspace.read(cx).track_charts.len() == 3
    });
    window
        .update(&mut cx, |viewer, _, cx| {
            zoom_session_navigation(viewer, 2., cx);
            viewer.request_detail(cx);
            cx.notify();
        })
        .expect("viewer should remain open");
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        let Some(viewport) = viewer.active_navigation(cx).selected_viewport() else {
            return false;
        };
        viewer
            .session_snapshot(cx)
            .views
            .active()
            .panels
            .iter()
            .all(|panel| {
                panel.detail.is_some()
                    && !panel.is_pending(ReadKind::Detail)
                    && panel.requested_detail_viewport == Some(viewport)
            })
    });
    let charts = window
        .read_with(&cx, |viewer, cx| {
            let workspace = viewer.workspace.read(cx);
            ["metric-0", "metric-1", "metric-2"]
                .into_iter()
                .map(|metric| {
                    workspace
                        .track_charts
                        .get(&MetricPanelId::from_string(metric))
                        .cloned()
                        .expect("Metric chart should be cached")
                })
                .collect::<Vec<_>>()
        })
        .expect("viewer should remain open");
    let before = charts
        .iter()
        .map(|chart| chart.read_with(&cx, |chart, _| chart.viewport()))
        .collect::<Vec<_>>();
    let prepare_counts = |viewer: &ViewerApp, cx: &App| {
        ["metric-0", "metric-1", "metric-2"]
            .into_iter()
            .map(|metric| {
                let panel_id = MetricPanelId::from_string(metric);
                (
                    metric,
                    viewer
                        .workspace
                        .read(cx)
                        .track_charts
                        .get(&panel_id)
                        .expect("Metric chart should be cached")
                        .read(cx)
                        .prepare_count(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let prepares_before = window
        .read_with(&cx, |viewer, cx| prepare_counts(viewer, cx))
        .expect("viewer should remain open");
    let narrow = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .active_navigation(cx)
                .brush()
                .expect("brush should exist")
                .selected()
        })
        .expect("viewer should remain open");
    assert!(before.iter().all(|viewport| viewport.x == narrow));

    let canvas = cx
        .debug_bounds("metric-canvas:metric-0")
        .expect("Metric chart should render");
    cx.simulate_event(ScrollWheelEvent {
        position: canvas.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(1_000.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();

    let selected = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .active_navigation(cx)
                .brush()
                .expect("brush should exist")
                .selected()
        })
        .expect("viewer should remain open");
    let after = charts
        .iter()
        .map(|chart| chart.read_with(&cx, |chart, _| chart.viewport()))
        .collect::<Vec<_>>();
    assert_ne!(selected, narrow);
    for (before, after) in before.iter().zip(&after) {
        assert_ne!(after.x, before.x);
        assert_eq!(after.x, selected);
    }
    let prepares_after = window
        .read_with(&cx, |viewer, cx| prepare_counts(viewer, cx))
        .expect("viewer should remain open");
    for metric in ["metric-0", "metric-1", "metric-2"] {
        assert!(
            prepares_after[metric] > prepares_before[metric],
            "{metric} should prepare its expanded viewport immediately"
        );
    }

    cx.simulate_event(ScrollWheelEvent {
        position: canvas.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-100.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();

    let contracted = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .active_navigation(cx)
                .brush()
                .expect("brush should exist")
                .selected()
        })
        .expect("viewer should remain open");
    let after_zoom_in = charts
        .iter()
        .map(|chart| chart.read_with(&cx, |chart, _| chart.viewport()))
        .collect::<Vec<_>>();
    assert!(contracted.span() < selected.span());
    assert!(
        after_zoom_in
            .iter()
            .all(|viewport| viewport.x == contracted)
    );
    let prepares_after_zoom_in = window
        .read_with(&cx, |viewer, cx| prepare_counts(viewer, cx))
        .expect("viewer should remain open");
    for metric in ["metric-0", "metric-1", "metric-2"] {
        assert!(
            prepares_after_zoom_in[metric] > prepares_after[metric],
            "{metric} should prepare its contracted viewport immediately"
        );
    }

    cx.executor().advance_clock(Duration::from_millis(101));
    cx.run_until_parked();
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        let Some(detail_viewport) = viewer.active_navigation(cx).selected_viewport() else {
            return false;
        };
        !viewer.workspace.read(cx).metric_repaint_pending
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .all(|panel| {
                    panel.detail.is_some()
                        && !panel.is_pending(ReadKind::Detail)
                        && panel.requested_detail_viewport == Some(detail_viewport)
                })
    });
}

#[gpui::test]
fn ruler_drag_pans_the_shared_viewport_within_home(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_extent(100);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.active_navigation(cx).brush().is_some()
    });
    window
        .update(&mut cx, |viewer, _, cx| {
            zoom_session_navigation(viewer, 2., cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    let before = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .active_navigation(cx)
                .brush()
                .map(|brush| brush.selected())
        })
        .expect("viewer should remain open")
        .expect("selected viewport should exist");
    let ruler = cx
        .debug_bounds("ruler-hit-area")
        .expect("shared ruler hit area should render");
    let start = ruler.center();
    let end = point(ruler.origin.x + ruler.size.width * 0.75, ruler.center().y);

    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());

    window
        .read_with(&cx, |viewer, cx| {
            let brush = viewer
                .active_navigation(cx)
                .brush()
                .expect("brush should remain available");
            assert_ne!(brush.selected(), before);
            assert!(
                (brush.selected().span() - before.span()).abs()
                    <= f64::EPSILON * before.span().abs().max(1.)
            );
            assert!(brush.selected().start() >= brush.home().start());
            assert!(brush.selected().end() <= brush.home().end());
            assert!(viewer.workspace.read(cx).drag.is_none());
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn ruler_scroll_pans_the_viewport_and_clamps_to_home(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_extent(100);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.active_navigation(cx).brush().is_some()
    });
    window
        .update(&mut cx, |viewer, _, cx| {
            zoom_session_navigation(viewer, 2., cx);
        })
        .expect("viewer should remain open");
    let before = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .active_navigation(cx)
                .brush()
                .expect("brush")
                .selected()
        })
        .expect("viewer should remain open");
    let ruler = cx
        .debug_bounds("ruler-hit-area")
        .expect("shared ruler hit area should render");
    let track = cx
        .debug_bounds("metric-track:loss")
        .expect("Metric track should render");
    let position = point(ruler.origin.x + px(10.), ruler.center().y);
    assert!(position.x < track.origin.x);

    for delta in [-80., -10_000.] {
        cx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
    }

    window
        .read_with(&cx, |viewer, cx| {
            let brush = viewer
                .active_navigation(cx)
                .brush()
                .expect("brush should remain available");
            assert!(
                (brush.selected().span() - before.span()).abs()
                    <= f64::EPSILON * before.span().abs().max(1.)
            );
            assert!(brush.selected().start() > before.start());
            assert_eq!(brush.selected().end(), brush.home().end());
            assert!(viewer.workspace.read(cx).detail_refresh_pending);
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn modified_ruler_scroll_zooms_around_the_pointer(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_extent(100);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.active_navigation(cx).brush().is_some()
    });
    window
        .update(&mut cx, |viewer, _, cx| {
            zoom_session_navigation(viewer, 2., cx);
        })
        .expect("viewer should remain open");
    let before = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .active_navigation(cx)
                .brush()
                .expect("brush")
                .selected()
        })
        .expect("viewer should remain open");
    let plot = cx
        .debug_bounds("overview-chart")
        .expect("Overview plot should render");
    let ruler = cx
        .debug_bounds("ruler-hit-area")
        .expect("Shared ruler hit area should render");
    let anchor_ratio = 0.25;
    let position = point(
        plot.origin.x + plot.size.width * anchor_ratio,
        ruler.center().y,
    );
    let anchor = before.start() + before.span() * f64::from(anchor_ratio);
    let modifiers = Modifiers {
        platform: true,
        ..Modifiers::default()
    };

    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(-80.))),
        modifiers,
        touch_phase: TouchPhase::Moved,
    });

    window
        .read_with(&cx, |viewer, cx| {
            let selected = viewer
                .active_navigation(cx)
                .brush()
                .expect("brush")
                .selected();
            let anchored = selected.start() + selected.span() * f64::from(anchor_ratio);
            assert!(selected.span() < before.span());
            assert!((anchored - anchor).abs() < 1e-9);
            assert!(viewer.workspace.read(cx).detail_refresh_pending);
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn track_scheduler_queries_and_prepares_only_visible_overscan(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture(10);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    cx.simulate_resize(size(px(600.), px(420.)));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 10);
    window
        .update(&mut cx, |viewer, _, cx| {
            for index in 0..10 {
                viewer.select_metric(MetricKey::from_string(format!("metric-{index}")), cx);
            }
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.active().panels.len() == 10
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .all(|panel| panel.overview.is_some())
    });
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        let workspace = viewer.workspace.read(cx);
        let schedule = workspace.track_viewport.borrow();
        !schedule.overscan.is_empty() && schedule.physical_width > 0
    });
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        let workspace = viewer.workspace.read(cx);
        let schedule = workspace.track_viewport.borrow();
        viewer.session_snapshot(cx).views.active().panels[schedule.overscan.clone()]
            .iter()
            .all(|panel| panel.detail.is_some())
            && workspace.track_charts.len() == schedule.overscan.len()
    });

    let (visible, overscan, detail_count, adapter_count) = window
        .read_with(&cx, |viewer, cx| {
            let workspace = viewer.workspace.read(cx);
            let schedule = workspace.track_viewport.borrow().clone();
            (
                schedule.visible,
                schedule.overscan,
                viewer
                    .session_snapshot(cx)
                    .views
                    .active()
                    .panels
                    .iter()
                    .filter(|panel| panel.detail.is_some())
                    .count(),
                workspace.track_charts.len(),
            )
        })
        .expect("viewer should remain open");
    assert!(overscan.len() > visible.len());
    assert_eq!(detail_count, overscan.len());
    assert!(detail_count < 10);
    assert_eq!(adapter_count, overscan.len());
    window
        .read_with(&cx, |viewer, cx| {
            for panel in viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .filter(|panel| panel.detail.is_some())
            {
                let detail = panel.detail.as_ref().expect("detail should be present");
                assert_eq!(
                    detail.point_budget,
                    panel.logical_width.saturating_mul(2).clamp(512, 5_000)
                );
            }
        })
        .expect("viewer should remain open");

    let tracks = cx
        .debug_bounds("metric-track-scroll")
        .expect("Metric track list should render");
    cx.simulate_event(ScrollWheelEvent {
        position: tracks.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-10_000.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        let workspace = viewer.workspace.read(cx);
        let schedule = workspace.track_viewport.borrow();
        schedule.visible.contains(&9)
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .last()
                .is_some_and(|panel| panel.detail.is_some())
    });
    assert!(cx.debug_bounds("metric-track:metric-9").is_some());

    let revisions_before_zoom = window
        .read_with(&cx, |viewer, cx| {
            let workspace = viewer.workspace.read(cx);
            let schedule = workspace.track_viewport.borrow();
            viewer.session_snapshot(cx).views.active().panels[schedule.overscan.clone()]
                .iter()
                .map(|panel| (panel.panel_id.clone(), panel.detail_revision))
                .collect::<HashMap<_, _>>()
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            zoom_session_navigation(viewer, 1.25, cx);
            viewer.workspace.update(cx, |workspace, cx| {
                workspace.schedule_detail_refresh(cx);
            });
        })
        .expect("viewer should remain open");
    cx.executor().advance_clock(Duration::from_millis(101));
    cx.run_until_parked();
    wait_for_viewer_with_app(window, &cx, |viewer, cx| {
        let Some(viewport) = viewer.active_navigation(cx).selected_viewport() else {
            return false;
        };
        let workspace = viewer.workspace.read(cx);
        let schedule = workspace.track_viewport.borrow();
        viewer.session_snapshot(cx).views.active().panels[schedule.overscan.clone()]
            .iter()
            .all(|panel| {
                panel.detail.is_some()
                    && !panel.is_pending(ReadKind::Detail)
                    && panel.requested_detail_viewport == Some(viewport)
                    && revisions_before_zoom
                        .get(&panel.panel_id)
                        .is_some_and(|revision| panel.detail_revision > *revision)
            })
    });
}

#[gpui::test]
fn zoom_debounce_commits_only_the_latest_viewport(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_extent(100);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .views
            .active()
            .panels
            .first()
            .is_some_and(|panel| panel.detail.is_some())
    });
    window
        .update(&mut cx, |_, _, cx| cx.notify())
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    let (before_generation, before_revision, before_viewport) = window
        .read_with(&cx, |viewer, cx| {
            let panel = viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .first()
                .cloned()
                .expect("metric panel should exist");
            (
                viewer.session.read(cx).next_generation,
                panel.detail_revision,
                panel.requested_detail_viewport,
            )
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            zoom_session_navigation(viewer, 1.25, cx);
            viewer.workspace.update(cx, |workspace, cx| {
                workspace.schedule_detail_refresh(cx);
            });
            zoom_session_navigation(viewer, 1.25, cx);
            viewer.workspace.update(cx, |workspace, cx| {
                workspace.schedule_detail_refresh(cx);
            });
            assert!(
                viewer
                    .session_snapshot(cx)
                    .views
                    .active()
                    .panels
                    .first()
                    .is_some_and(|panel| !panel.is_pending(ReadKind::Detail))
            );
        })
        .expect("viewer should remain open");
    let final_viewport = window
        .read_with(&cx, |viewer, cx| {
            viewer.active_navigation(cx).selected_viewport()
        })
        .expect("viewer should remain open");
    let immediate = window
        .read_with(&cx, |viewer, cx| {
            let panel = viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .first()
                .cloned()
                .expect("metric panel should exist");
            (panel.detail_revision, panel.requested_detail_viewport)
        })
        .expect("viewer should remain open");
    assert_eq!(immediate, (before_revision, before_viewport));
    assert_ne!(final_viewport, before_viewport);

    cx.executor().advance_clock(Duration::from_millis(101));
    cx.run_until_parked();
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    let after = window
        .read_with(&cx, |viewer, cx| {
            let panel = viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .first()
                .cloned()
                .expect("metric panel should exist");
            (
                viewer.session.read(cx).next_generation,
                panel.detail_revision,
                panel.requested_detail_viewport,
            )
        })
        .expect("viewer should remain open");
    assert_eq!(after.0, before_generation + 1);
    assert!(after.1 > before_revision);
    assert_eq!(after.2, final_viewport);
}

#[gpui::test]
fn keyboard_zoom_reprojects_immediately_and_debounces_detail(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_extent(100);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .views
            .active()
            .panels
            .first()
            .is_some_and(|panel| panel.detail.is_some())
    });
    window
        .update(&mut cx, |_, _, cx| cx.notify())
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    let (before_generation, before_revision, before_viewport, before_span) = window
        .read_with(&cx, |viewer, cx| {
            let panel = viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .first()
                .cloned()
                .expect("metric panel should exist");
            (
                viewer.session.read(cx).next_generation,
                panel.detail_revision,
                panel.requested_detail_viewport,
                viewer
                    .active_navigation(cx)
                    .brush()
                    .expect("timeline brush should exist")
                    .selected()
                    .span(),
            )
        })
        .expect("viewer should remain open");

    cx.dispatch_action(ZoomIn);
    cx.dispatch_action(ZoomIn);
    let immediate = window
        .read_with(&cx, |viewer, cx| {
            let panel = viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .first()
                .cloned()
                .expect("metric panel should exist");
            (
                viewer.session.read(cx).next_generation,
                panel.detail_revision,
                panel.requested_detail_viewport,
                viewer
                    .active_navigation(cx)
                    .brush()
                    .expect("timeline brush should exist")
                    .selected()
                    .span(),
            )
        })
        .expect("viewer should remain open");
    assert_eq!(immediate.0, before_generation);
    assert_eq!(immediate.1, before_revision);
    assert_eq!(immediate.2, before_viewport);
    assert!(immediate.3 < before_span);
    let final_viewport = window
        .read_with(&cx, |viewer, cx| {
            viewer.active_navigation(cx).selected_viewport()
        })
        .expect("viewer should remain open");

    cx.executor().advance_clock(Duration::from_millis(101));
    cx.run_until_parked();
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    let after = window
        .read_with(&cx, |viewer, cx| {
            let panel = viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .first()
                .cloned()
                .expect("metric panel should exist");
            (
                viewer.session.read(cx).next_generation,
                panel.detail_revision,
                panel.requested_detail_viewport,
            )
        })
        .expect("viewer should remain open");
    assert_eq!(after.0, before_generation + 1);
    assert!(after.1 > before_revision);
    assert_eq!(after.2, final_viewport);
}
