use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, AppContext, Context};
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    px, size,
};
use pulseon_core::engine::client::NativeClient;
use pulseon_model::alignment::AlignmentAxis;
use pulseon_model::run::RunId;
use pulseon_model::types::ProjectId;

use crate::data::worker::ReadKind;
use crate::domain::{DataSourceId, RunRef};
use crate::workbench::document::{SavedAnalysisView, SavedRunRef, WorkbenchDocument};

use super::ViewerApp;
use super::command::WorkbenchCommand;

pub(super) fn fixture(metric_count: usize) -> (tempfile::TempDir, ProjectId, RunId) {
    fixture_with_runs(metric_count, 1)
}

pub(super) fn fixture_with_metric(metric_key: &str) -> (tempfile::TempDir, ProjectId, RunId) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let client = NativeClient::open(root.path()).expect("test client should open");
    let project = client
        .create_project("viewer", Some(ProjectId::from_string("project")))
        .expect("test project should be created");
    let run = client
        .create_run(&project.project_id, "run", Some(RunId::from_string("run")))
        .expect("test Run should be created");
    client
        .run_handle(run.clone())
        .log_metric_at_step(metric_key, 0, 1.)
        .expect("test metric should be logged");
    client.finish_run(&run.run_id).expect("Run should finish");
    client.shutdown(None).expect("test client should shut down");
    (root, project.project_id, run.run_id)
}

pub(super) fn fixture_with_runs(
    metric_count: usize,
    run_count: usize,
) -> (tempfile::TempDir, ProjectId, RunId) {
    fixture_with_run_coverage(metric_count, run_count, false)
}

pub(super) fn fixture_with_complete_runs(
    metric_count: usize,
    run_count: usize,
) -> (tempfile::TempDir, ProjectId, RunId) {
    fixture_with_run_coverage(metric_count, run_count, true)
}

pub(super) fn fixture_with_run_coverage(
    metric_count: usize,
    run_count: usize,
    populate_all_runs: bool,
) -> (tempfile::TempDir, ProjectId, RunId) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let client = NativeClient::open(root.path()).expect("test client should open");
    let project = client
        .create_project("viewer", Some(ProjectId::from_string("project")))
        .expect("test project should be created");
    let mut first_run_id = None;
    for run_index in 0..run_count {
        let run_name = format!("baseline {run_index}");
        let run = client
            .create_run(
                &project.project_id,
                &run_name,
                Some(RunId::from_string(format!(
                    "run-{run_index}-with-a-very-long-identifier-that-requires-horizontal-scrolling"
                ))),
            )
            .expect("test Run should be created");
        if run_index == 0 || populate_all_runs {
            let handle = client.run_handle(run.clone());
            for index in 0..metric_count {
                let metric_key = format!("metric-{index}");
                handle
                    .log_metric_at_step(&metric_key, 0, (run_index + index) as f64)
                    .expect("test metric should be logged");
                if populate_all_runs {
                    handle
                        .log_metric_at_step(&metric_key, 100, (run_index + index + 1) as f64)
                        .expect("test metric extent should be logged");
                }
            }
        }
        client
            .finish_run(&run.run_id)
            .expect("test Run should finish");
        first_run_id.get_or_insert(run.run_id);
    }
    client.shutdown(None).expect("test client should shut down");
    (
        root,
        project.project_id,
        first_run_id.expect("fixture should contain at least one Run"),
    )
}

pub(super) fn fixture_with_extent(end_step: i64) -> (tempfile::TempDir, ProjectId, RunId) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let client = NativeClient::open(root.path()).expect("test client should open");
    let project = client
        .create_project("viewer", Some(ProjectId::from_string("project")))
        .expect("test project should be created");
    let run = client
        .create_run(
            &project.project_id,
            "baseline",
            Some(RunId::from_string("run")),
        )
        .expect("test Run should be created");
    let handle = client.run_handle(run.clone());
    handle
        .log_metric_at_step("loss", 0, 1.)
        .expect("test metric should be logged");
    handle
        .log_metric_at_step("loss", end_step, 0.5)
        .expect("test metric should be logged");
    client
        .finish_run(&run.run_id)
        .expect("test Run should finish");
    client.shutdown(None).expect("test client should shut down");
    (root, project.project_id, run.run_id)
}

pub(super) fn saved_workbench(
    source_path: PathBuf,
    project_id: ProjectId,
    runs: Vec<RunId>,
    metric: &str,
) -> WorkbenchDocument {
    WorkbenchDocument {
        sources: vec![source_path.clone()],
        pinned_projects: Vec::new(),
        archived_projects: Vec::new(),
        removed_projects: Vec::new(),
        archived_runs: Vec::new(),
        views: vec![SavedAnalysisView {
            name: "Restored".to_owned(),
            runs: runs
                .into_iter()
                .map(|run_id| SavedRunRef {
                    source_path: source_path.clone(),
                    project_id: project_id.clone(),
                    run_id,
                })
                .collect(),
            baseline: None,
            pinned_runs: Vec::new(),
            metrics: vec![metric.to_owned()],
            metric_heights: Vec::new(),
            selected_metric: Some(metric.to_owned()),
            axis: AlignmentAxis::Step,
            viewport: Some(
                pulseon_chart_core::AxisRange::new(0., 10.).expect("test viewport should be valid"),
            ),
        }],
        active_view: 0,
        project_sidebar_visible: false,
        project_sidebar_width: 280.,
        metric_sidebar_compact: true,
        bottom_inspector_visible: true,
        bottom_inspector_height: 260.,
    }
}

pub(super) fn open_viewer(
    cx: &mut TestAppContext,
    project_path: Option<PathBuf>,
) -> (WindowHandle<ViewerApp>, VisualTestContext) {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(800.), px(600.)),
                ))),
                ..WindowOptions::default()
            },
            move |window, cx| cx.new(|cx| ViewerApp::new(project_path, window, cx)),
        )
        .expect("test viewer window should open")
    });
    let visual = VisualTestContext::from_window(window.into(), cx);
    (window, visual)
}

#[track_caller]
pub(super) fn wait_for_viewer(
    window: WindowHandle<ViewerApp>,
    cx: &VisualTestContext,
    condition: impl Fn(&ViewerApp, &App) -> bool,
) {
    for _ in 0..1_000 {
        cx.run_until_parked();
        let ready = window
            .read_with(cx, |viewer, cx| condition(viewer, cx))
            .expect("viewer should remain open");
        if ready {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("viewer state did not arrive before the test deadline");
}

pub(super) fn wait_for_viewer_with_app(
    window: WindowHandle<ViewerApp>,
    cx: &VisualTestContext,
    condition: impl Fn(&ViewerApp, &App) -> bool,
) {
    for _ in 0..1_000 {
        cx.run_until_parked();
        let ready = window
            .read_with(cx, |viewer, cx| condition(viewer, cx))
            .expect("viewer should remain open");
        if ready {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("viewer state did not arrive before the test deadline");
}

pub(super) fn first_panel_detail_is_settled(viewer: &ViewerApp, cx: &App) -> bool {
    let Some(viewport) = viewer.active_navigation(cx).selected_viewport() else {
        return false;
    };
    viewer
        .session_snapshot(cx)
        .views
        .active()
        .panels
        .first()
        .is_some_and(|panel| {
            panel.detail.is_some()
                && !panel.is_pending(ReadKind::Detail)
                && panel.requested_detail_viewport == Some(viewport)
                && panel.physical_width > 0
        })
}

pub(super) fn source_catalog_loaded(viewer: &ViewerApp, cx: &App) -> bool {
    viewer
        .session_snapshot(cx)
        .sources
        .iter()
        .any(|source| !source.catalog.projects.is_empty())
}

pub(super) fn first_source_id(viewer: &ViewerApp, cx: &App) -> DataSourceId {
    viewer
        .session_snapshot(cx)
        .sources
        .iter()
        .next()
        .expect("fixture source should be imported")
        .source_id
        .clone()
}

pub(super) fn zoom_session_navigation(
    viewer: &mut ViewerApp,
    factor: f64,
    cx: &mut Context<ViewerApp>,
) {
    viewer.session.update(cx, |session, session_cx| {
        let navigation = &mut session.views.active_mut().navigation;
        let brush = navigation.brush().expect("timeline brush should exist");
        let anchor = brush.selected().start() + brush.selected().span() / 2.;
        assert!(navigation.zoom_at(anchor, factor));
        session.publish_snapshot();
        session_cx.notify();
    });
    let session = viewer.session_snapshot(cx);
    let interaction = viewer.interaction_snapshot(cx);
    let visible_runs = viewer.active_visible_runs(cx);
    let available_metrics = viewer.available_metric_keys(cx);
    let sidebar_visible = viewer.sidebar_visible(cx);
    let sidebar_width = viewer.sidebar_width(cx);
    viewer.workspace.update(cx, |workspace, cx| {
        workspace.sync(
            session,
            interaction,
            visible_runs,
            available_metrics,
            sidebar_visible,
            sidebar_width,
        );
        workspace.sync_track_charts(cx);
    });
}

pub(super) fn select_fixture_run(
    window: WindowHandle<ViewerApp>,
    cx: &mut VisualTestContext,
    project_id: ProjectId,
    run_id: RunId,
    metric_count: usize,
) {
    wait_for_viewer(window, cx, source_catalog_loaded);
    window
        .update(cx, |viewer, _, cx| {
            let source_id = first_source_id(viewer, cx);
            viewer.dispatch_workbench_command(
                WorkbenchCommand::ToggleRun(RunRef::new(source_id, project_id, run_id)),
                cx,
            )
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, cx, |viewer, cx| {
        viewer.available_metric_keys(cx).len() == metric_count
    });
}
