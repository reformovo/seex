use std::fs;
use std::sync::Arc;

use gpui::{Modifiers, TestAppContext, point, px, size};
use seex::{
    AlignmentViewport, CatalogBackend, Client, EvidenceCompleteness, EvidenceReason, LogOptions,
    ResumePolicy, RunOptions,
};

use super::super::test_support::*;
use super::*;
use crate::data::query::CurveSnapshot;
use crate::desktop::UseElapsed;
use crate::desktop::app::command::{CommandEffect, WorkbenchCommand};

#[test]
fn typed_view_commands_own_the_view_lifecycle() {
    let mut session = WorkbenchSession::new(None);
    let original = session.views.active().view_id.clone();
    let panel_id = session
        .views
        .select_active_metric(MetricKey::from_string("loss"));
    let viewport = AlignmentViewport::new(0, 10).expect("viewport should be valid");
    session
        .views
        .active_panel_mut(&panel_id)
        .expect("test panel should exist")
        .overview = Some(Arc::new(CurveSnapshot {
        viewport,
        point_budget: 10,
        real_range: Some(viewport),
        series: Vec::new(),
    }));

    assert_eq!(
        session.apply_command(WorkbenchCommand::CreateView),
        CommandEffect {
            changed: true,
            active_view_changed: true,
            ..CommandEffect::default()
        }
    );
    let created = session.views.active().view_id.clone();
    assert_ne!(created, original);
    assert!(
        session
            .views
            .views()
            .iter()
            .flat_map(|view| &view.panels)
            .all(|panel| panel.overview.is_none())
    );
    assert!(
        session
            .apply_command(WorkbenchCommand::RenameView {
                view_id: created.clone(),
                name: "Renamed".to_owned(),
            })
            .changed
    );
    assert_eq!(session.views.active().name, "Renamed");
    assert!(
        session
            .apply_command(WorkbenchCommand::ActivateView(original))
            .active_view_changed
    );
    assert!(
        session
            .apply_command(WorkbenchCommand::CloseView(created))
            .changed
    );
    assert_eq!(session.views.views().len(), 1);
}

#[gpui::test]
fn view_actions_dispatch_through_the_focused_root(cx: &mut TestAppContext) {
    let (window, mut cx) = open_viewer(cx, None);

    cx.dispatch_action(UseElapsed);

    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer.active_navigation(cx).axis())
            .expect("viewer should remain open"),
        AlignmentAxis::ElapsedTime
    );
}

#[gpui::test]
fn no_sources_render_a_blank_workspace(cx: &mut TestAppContext) {
    let (_window, mut cx) = open_viewer(cx, None);

    assert!(cx.debug_bounds("open-project").is_none());
    assert!(cx.debug_bounds("metric-track-scroll").is_none());
}

#[gpui::test]
fn analysis_view_tabs_manage_the_active_view_lifecycle(cx: &mut TestAppContext) {
    let (window, mut cx) = open_viewer(cx, None);
    let original_view = window
        .update(&mut cx, |viewer, _, cx| {
            viewer.session.update(cx, |session, session_cx| {
                session
                    .views
                    .active_mut()
                    .navigation
                    .select_axis(AlignmentAxis::ElapsedTime);
                session.publish_snapshot();
                session_cx.notify();
            });
            viewer.session_snapshot(cx).views.active().view_id.clone()
        })
        .expect("viewer should remain open");

    let new_view = cx
        .debug_bounds("new-view")
        .expect("new View control should render");
    cx.simulate_mouse_move(new_view.center(), None, Modifiers::default());
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert!(cx.debug_bounds("label-tooltip").is_some());
    cx.simulate_click(new_view.center(), Modifiers::default());
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer.active_navigation(cx).axis())
            .expect("viewer should remain open"),
        AlignmentAxis::Step
    );
    let active_tab = cx
        .debug_bounds("analysis-tab")
        .expect("active View tab should render");
    cx.simulate_mouse_down(
        active_tab.center(),
        MouseButton::Right,
        Modifiers::default(),
    );
    let menu = cx
        .debug_bounds("view-menu")
        .expect("View menu should render");
    assert!(menu.top() >= active_tab.bottom());
    let duplicate = cx
        .debug_bounds("duplicate-view")
        .expect("duplicate View control should render");
    cx.simulate_click(duplicate.center(), Modifiers::default());
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .views
                .views()
                .len())
            .expect("viewer should remain open"),
        3
    );

    let active_tab = cx
        .debug_bounds("analysis-tab")
        .expect("duplicated View tab should be active");
    cx.simulate_mouse_move(active_tab.center(), None, Modifiers::default());
    cx.simulate_mouse_down(
        active_tab.center(),
        MouseButton::Right,
        Modifiers::default(),
    );
    let rename = cx
        .debug_bounds("rename-view")
        .expect("View menu should expose Rename");
    cx.simulate_mouse_move(rename.center(), None, Modifiers::default());
    cx.simulate_click(rename.center(), Modifiers::default());
    assert!(cx.debug_bounds("rename-view-input").is_some());
    assert!(cx.debug_bounds("rename-view-selection").is_some());
    assert!(cx.debug_bounds("rename-view-caret").is_some());
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert!(cx.debug_bounds("rename-view-caret").is_none());
    cx.simulate_keystrokes("view left left right x");
    let prefix = cx
        .debug_bounds("rename-view-prefix")
        .expect("View name before the cursor should render");
    assert!(cx.debug_bounds("rename-view-suffix").is_some());
    let caret = cx
        .debug_bounds("rename-view-caret")
        .expect("typing should reveal the View name caret");
    assert_eq!(caret.origin.x, prefix.right() + px(1.));
    cx.simulate_keystrokes("enter");
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .views
                .active()
                .name
                .clone())
            .expect("viewer should remain open"),
        "viexw"
    );

    let active_tab = cx
        .debug_bounds("analysis-tab")
        .expect("renamed View tab should remain active");
    cx.simulate_mouse_down(
        active_tab.center(),
        MouseButton::Right,
        Modifiers::default(),
    );
    let rename = cx
        .debug_bounds("rename-view")
        .expect("View menu should expose Rename");
    cx.simulate_click(rename.center(), Modifiers::default());
    cx.simulate_keystrokes("outside");
    let controls = cx
        .debug_bounds("analysis-right-controls")
        .expect("View toolbar controls should render");
    cx.simulate_click(
        point(controls.origin.x + px(2.), controls.center().y),
        Modifiers::default(),
    );
    assert!(
        window
            .read_with(&cx, |viewer, cx| !viewer.view_bar_is_renaming(cx))
            .expect("viewer should remain open")
    );
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .views
                .active()
                .name
                .clone())
            .expect("viewer should remain open"),
        "outside"
    );

    let close = cx
        .debug_bounds("close-active-view")
        .expect("active View close control should render");
    cx.simulate_click(close.center(), Modifiers::default());
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
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.dispatch_workbench_command(
                WorkbenchCommand::ActivateView(original_view.clone()),
                cx,
            );
        })
        .expect("viewer should remain open");
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer.active_navigation(cx).axis())
            .expect("viewer should remain open"),
        AlignmentAxis::ElapsedTime
    );
}

#[gpui::test]
fn top_refresh_requests_every_source_from_an_empty_view(cx: &mut TestAppContext) {
    let (first, _, _) = fixture_with_metric("loss");
    let (second, _, _) = fixture_with_metric("accuracy");
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, first.path().to_path_buf());
    wait_for_viewer(window, &cx, source_catalog_loaded);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.open_configured_sources(
                vec![configured_source("second-source", second.path())],
                cx,
            );
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).sources.iter().len() == 2
    });
    let before = window
        .update(&mut cx, |viewer, _, cx| {
            viewer.dispatch_workbench_command(WorkbenchCommand::CreateView, cx);
            assert!(viewer.session_snapshot(cx).views.active().runs.is_empty());
            viewer.session.read(cx).next_generation
        })
        .expect("viewer should remain open");
    assert!(cx.debug_bounds("brush-controls").is_some());
    assert!(cx.debug_bounds("viewport-ruler").is_some());
    assert!(cx.debug_bounds("metric-track-scroll").is_some());
    assert!(cx.debug_bounds("open-project").is_none());

    let refresh = cx
        .debug_bounds("refresh-view")
        .expect("Refresh should remain available in an empty View");
    cx.simulate_click(refresh.center(), Modifiers::default());

    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer.session.read(cx).next_generation)
            .expect("viewer should remain open")
            >= before + 2
    );
}

#[derive(Clone)]
struct PanelCurveState {
    overview: Arc<crate::data::query::CurveSnapshot>,
    detail: Arc<crate::data::query::CurveSnapshot>,
    overview_source_count: u64,
    detail_source_count: u64,
    overview_revision: u64,
    detail_revision: u64,
    catalog_run_status: RunStatus,
    run_status: RunStatus,
    completeness: EvidenceCompleteness,
    reasons: Vec<EvidenceReason>,
}

fn first_panel_curve_state(viewer: &ViewerApp, cx: &App) -> Option<PanelCurveState> {
    let snapshot = viewer.session_snapshot(cx);
    let panel = snapshot.views.active().panels.first()?;
    if panel.is_pending(ReadKind::Overview) {
        return None;
    }
    let overview = panel.overview.clone()?;
    let overview_source_count = overview.series.first()?.source_row_count;
    let curve = overview.series.first()?;
    let detail_source_count = curve.source_row_count;
    let catalog_run_status = snapshot.sources.first()?.catalog.runs.first()?.status;
    let run_status = curve.run.status;
    let completeness = curve.completeness;
    let reasons = curve.reasons.clone();
    Some(PanelCurveState {
        detail: Arc::clone(&overview),
        overview,
        overview_source_count,
        detail_source_count,
        overview_revision: panel.overview_revision,
        detail_revision: panel.overview_revision,
        catalog_run_status,
        run_status,
        completeness,
        reasons,
    })
}

fn append_loss_points(root_path: &std::path::Path, points: &[(i64, f64)]) {
    let client = Client::builder(root_path)
        .catalog_backend(CatalogBackend::Sqlite)
        .open()
        .expect("test client should open");
    let run = client
        .start_run(
            RunOptions::new("project")
                .id("run")
                .name("running")
                .resume(ResumePolicy::Allow),
        )
        .expect("test Run should start or resume");
    for &(step, value) in points {
        run.log_with([("loss", value)], LogOptions::new().step(step))
            .expect("test metric should be admitted");
    }
    client
        .shutdown()
        .expect("test client should persist admitted metrics");
}

fn finish_loss_run(root_path: &std::path::Path) {
    let client = Client::builder(root_path)
        .catalog_backend(CatalogBackend::Sqlite)
        .open()
        .expect("test client should reopen");
    let run = client
        .start_run(
            RunOptions::new("project")
                .id("run")
                .resume(ResumePolicy::Must),
        )
        .expect("test Run should resume");
    run.finish().expect("test Run should finish and flush");
    client.shutdown().expect("test client should shut down");
}

#[gpui::test]
fn refresh_and_resize_replace_snapshots_after_sqlite_appends(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let config_dir = root.path().join(".seex");
    fs::create_dir_all(&config_dir).expect("test config directory should be created");
    fs::write(
        config_dir.join("config.toml"),
        "schema_version = 1\ncatalog_backend = \"sqlite\"\n",
    )
    .expect("test config should be written");
    append_loss_points(root.path(), &[(0, 1.), (25, 0.5)]);
    let project_id = ProjectId::from_string("project");
    let run_id = RunId::from_string("run");

    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, root.path().to_path_buf());
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("loss"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        first_panel_curve_state(viewer, cx)
            .is_some_and(|state| state.overview_source_count == 2 && state.detail_source_count == 2)
    });
    let viewport = AlignmentViewport::new(0, 100).expect("test viewport should be valid");
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.dispatch_workbench_command(WorkbenchCommand::ClearTimeline, cx);
            viewer.dispatch_workbench_command(WorkbenchCommand::SetTimelineHome(viewport), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.active_navigation(cx).selected_viewport() == Some(viewport)
    });
    let initial = window
        .read_with(&cx, first_panel_curve_state)
        .expect("viewer should remain open")
        .expect("expanded viewport curves should load");

    append_loss_points(root.path(), &[(50, 0.7)]);
    let refresh = cx
        .debug_bounds("refresh-view")
        .expect("Refresh should render");
    cx.simulate_click(refresh.center(), Modifiers::default());
    wait_for_viewer(window, &cx, |viewer, cx| {
        first_panel_curve_state(viewer, cx).is_some_and(|state| {
            state.overview_source_count == 3
                && state.detail_source_count == 3
                && state.overview_revision > initial.overview_revision
                && state.detail_revision > initial.detail_revision
        })
    });
    let refreshed = window
        .read_with(&cx, first_panel_curve_state)
        .expect("viewer should remain open");
    let refreshed = refreshed.expect("refreshed curves should load");
    assert!(!Arc::ptr_eq(&initial.overview, &refreshed.overview));
    assert!(!Arc::ptr_eq(&initial.detail, &refreshed.detail));

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.dispatch_workbench_command(WorkbenchCommand::ClearTimeline, cx);
            viewer.dispatch_workbench_command(WorkbenchCommand::SetTimelineHome(viewport), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.active_navigation(cx).selected_viewport() == Some(viewport)
    });
    append_loss_points(root.path(), &[(75, 0.6)]);
    cx.simulate_resize(size(px(1_000.), px(700.)));
    wait_for_viewer(window, &cx, |viewer, cx| {
        first_panel_curve_state(viewer, cx).is_some_and(|state| state.overview_source_count == 4)
    });
    wait_for_viewer(window, &cx, |viewer, cx| {
        first_panel_curve_state(viewer, cx).is_some_and(|state| state.detail_source_count == 4)
    });
    let resized = window
        .read_with(&cx, first_panel_curve_state)
        .expect("viewer should remain open")
        .expect("resized curves should load");
    assert!(resized.overview_revision > refreshed.overview_revision);
    assert!(resized.detail_revision > refreshed.detail_revision);
    assert!(!Arc::ptr_eq(&refreshed.overview, &resized.overview));
    assert!(!Arc::ptr_eq(&refreshed.detail, &resized.detail));
    cx.refresh().expect("updated curves should render");
    assert!(cx.debug_bounds("metric-canvas:loss").is_some());

    finish_loss_run(root.path());
    let refresh = cx
        .debug_bounds("refresh-view")
        .expect("Refresh should remain available");
    cx.simulate_click(refresh.center(), Modifiers::default());
    wait_for_viewer(window, &cx, |viewer, cx| {
        first_panel_curve_state(viewer, cx).is_some_and(|state| {
            state.catalog_run_status == RunStatus::Finished
                && state.run_status == RunStatus::Finished
                && state.completeness == EvidenceCompleteness::Complete
                && state.reasons.is_empty()
                && state.overview_revision > resized.overview_revision
                && state.detail_revision > resized.detail_revision
        })
    });
    let finished = window
        .read_with(&cx, first_panel_curve_state)
        .expect("viewer should remain open")
        .expect("finished curves should load");
    assert!(!Arc::ptr_eq(&resized.overview, &finished.overview));
    assert!(!Arc::ptr_eq(&resized.detail, &finished.detail));
}

#[gpui::test]
fn switching_metric_tracks_preserves_the_shared_brush(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture(2);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, root.path().to_path_buf());
    cx.simulate_resize(size(px(1_000.), px(1_000.)));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 2);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
            viewer.select_metric(MetricKey::from_string("metric-1"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.active().panels.len() == 2
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .all(|panel| panel.overview.is_some())
    });
    let brush = window
        .read_with(&cx, |viewer, cx| viewer.active_navigation(cx).brush())
        .expect("viewer should remain open")
        .expect("loaded metrics should create a shared brush");

    for metric in ["metric-0", "metric-1"] {
        let track = cx
            .debug_bounds(if metric == "metric-0" {
                "metric-sidebar-row:metric-0"
            } else {
                "metric-sidebar-row:metric-1"
            })
            .expect("Metric label should render");
        cx.simulate_click(track.center(), Modifiers::default());
        cx.run_until_parked();
        wait_for_viewer(window, &cx, |viewer, cx| {
            viewer
                .session_snapshot(cx)
                .views
                .active()
                .selected_panel_id
                .as_ref()
                .is_some_and(|panel_id| {
                    panel_id.as_str() == metric
                        && viewer
                            .session_snapshot(cx)
                            .views
                            .active_panel(panel_id)
                            .is_some_and(|panel| !panel.is_pending(ReadKind::Inspector))
                })
        });

        window
            .read_with(&cx, |viewer, cx| {
                assert_eq!(viewer.active_navigation(cx).brush(), Some(brush));
                assert_eq!(
                    viewer
                        .session_snapshot(cx)
                        .views
                        .active()
                        .selected_panel_id
                        .as_ref()
                        .map(MetricPanelId::as_str),
                    Some(metric),
                );
            })
            .expect("viewer should remain open");
        let controls = cx
            .debug_bounds("brush-controls")
            .expect("Brush controls should render");
        let row = cx
            .debug_bounds("brush-row")
            .expect("Brush row should render");
        let overview = cx
            .debug_bounds("overview-chart")
            .expect("Overview chart should render");
        assert_eq!(row.size.height, px(40.));
        assert_eq!(controls.size.height, overview.size.height);
        assert_eq!(controls.origin.y, overview.origin.y);
        assert_eq!(controls.bottom(), overview.bottom());
    }
}

#[gpui::test]
fn empty_view_keeps_the_converged_shell_and_opens_metric_picker(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture(20);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, root.path().to_path_buf());
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 20);
    cx.simulate_resize(size(px(600.), px(520.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("brush-controls").is_some());
    assert!(cx.debug_bounds("viewport-ruler").is_some());
    assert!(cx.debug_bounds("metric-track-scroll").is_some());
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .len())
            .expect("viewer should remain open"),
        0
    );
    let add = cx
        .debug_bounds("add-metric")
        .expect("empty View should retain Add Metric");
    cx.simulate_click(add.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer.workspace.read(cx).metric_picker_open
            })
            .expect("viewer should remain open")
    );
}

#[gpui::test]
fn axis_picker_switches_the_view_to_elapsed_time(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture(1);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, root.path().to_path_buf());
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
        })
        .expect("viewer should remain open");
    let picker = cx
        .debug_bounds("axis-picker")
        .expect("axis picker should render beside Add Metric");
    cx.simulate_click(picker.center(), Modifiers::default());
    let time = cx
        .debug_bounds("axis-time")
        .expect("axis menu should offer Elapsed time");
    cx.simulate_click(time.center(), Modifiers::default());
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.active_navigation(cx).axis() == AlignmentAxis::ElapsedTime
            && viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .first()
                .and_then(|panel| panel.overview.as_ref())
                .and_then(|snapshot| snapshot.real_range)
                .is_some_and(|range| range.start() >= 0 && range.end() < 60_000)
    });
    assert!(cx.debug_bounds("viewport-ruler").is_some());
}
