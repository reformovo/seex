use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{App, AppContext, Context};
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    px, size,
};
use seex::AlignmentAxis;
use seex::ProjectId;
use seex::RunId;
use seex::{Client, LogOptions, RunOptions};

use crate::config::ConfiguredSource;
use crate::data::worker::ReadKind;
use crate::domain::{DataSourceId, RunRef, SourceAlias};
use crate::workbench::toml_document::{
    SavedAnalysisView, SavedLayout, SavedRunRef, TomlWorkbenchDocument,
};

use super::ViewerApp;
use super::command::WorkbenchCommand;

static NEXT_TEST_HOME: AtomicU64 = AtomicU64::new(0);

pub(super) fn fixture(metric_count: usize) -> (tempfile::TempDir, ProjectId, RunId) {
    fixture_with_runs(metric_count, 1)
}

pub(super) fn fixture_with_metric(metric_key: &str) -> (tempfile::TempDir, ProjectId, RunId) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let client = Client::builder(root.path())
        .open()
        .expect("test client should open");
    let run = client
        .start_run(RunOptions::new("project").id("run").name("run"))
        .expect("test Run should be created");
    run.log_with([(metric_key, 1.)], LogOptions::new().step(0))
        .expect("test metric should be logged");
    run.finish().expect("Run should finish");
    client.shutdown().expect("test client should shut down");
    (
        root,
        ProjectId::from_string("project"),
        run.run_id().clone(),
    )
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
    let client = Client::builder(root.path())
        .open()
        .expect("test client should open");
    let mut first_run_id = None;
    for run_index in 0..run_count {
        let run_name = format!("baseline {run_index}");
        let run_id = RunId::from_string(format!(
            "run-{run_index}-with-a-very-long-identifier-that-requires-horizontal-scrolling"
        ));
        let run = client
            .start_run(
                RunOptions::new("project")
                    .id(run_id.as_str())
                    .name(&run_name),
            )
            .expect("test Run should be created");
        if run_index == 0 || populate_all_runs {
            for index in 0..metric_count {
                let metric_key = format!("metric-{index}");
                run.log_with(
                    [(&metric_key, (run_index + index) as f64)],
                    LogOptions::new().step(0),
                )
                .expect("test metric should be logged");
                if populate_all_runs {
                    run.log_with(
                        [(&metric_key, (run_index + index + 1) as f64)],
                        LogOptions::new().step(100),
                    )
                    .expect("test metric extent should be logged");
                }
            }
        }
        run.finish().expect("test Run should finish");
        first_run_id.get_or_insert(run_id);
    }
    client.shutdown().expect("test client should shut down");
    (
        root,
        ProjectId::from_string("project"),
        first_run_id.expect("fixture should contain at least one Run"),
    )
}

pub(super) fn fixture_with_extent(end_step: i64) -> (tempfile::TempDir, ProjectId, RunId) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let client = Client::builder(root.path())
        .open()
        .expect("test client should open");
    let run = client
        .start_run(RunOptions::new("project").id("run").name("baseline"))
        .expect("test Run should be created");
    run.log_with([("loss", 1.)], LogOptions::new().step(0))
        .expect("test metric should be logged");
    run.log_with([("loss", 0.5)], LogOptions::new().step(end_step))
        .expect("test metric should be logged");
    run.finish().expect("test Run should finish");
    client.shutdown().expect("test client should shut down");
    (
        root,
        ProjectId::from_string("project"),
        run.run_id().clone(),
    )
}

pub(super) fn saved_workbench(
    source_alias: SourceAlias,
    project_id: ProjectId,
    runs: Vec<RunId>,
    metric: &str,
) -> TomlWorkbenchDocument {
    TomlWorkbenchDocument {
        active_view: 0,
        layout: SavedLayout {
            project_sidebar_visible: false,
            project_sidebar_width: 280.,
            metric_sidebar_compact: true,
            bottom_inspector_visible: true,
            bottom_inspector_height: 260.,
        },
        expanded_projects: Vec::new(),
        pinned_projects: Vec::new(),
        archived_projects: Vec::new(),
        archived_runs: Vec::new(),
        views: vec![SavedAnalysisView {
            name: "Restored".to_owned(),
            runs: runs
                .into_iter()
                .map(|run_id| SavedRunRef {
                    source_alias: source_alias.clone(),
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
                seex_plot::AxisRange::new(0., 10.).expect("test viewport should be valid"),
            ),
        }],
    }
}

pub(super) fn open_viewer(
    cx: &mut TestAppContext,
    project_path: Option<PathBuf>,
) -> (WindowHandle<ViewerApp>, VisualTestContext) {
    open_viewer_with_optional_source(cx, project_path, None)
}

pub(super) fn open_viewer_with_configured_source(
    cx: &mut TestAppContext,
    project_path: PathBuf,
) -> (WindowHandle<ViewerApp>, VisualTestContext) {
    let source = configured_source("test-source", &project_path);
    open_viewer_with_optional_source(cx, Some(project_path), Some(source))
}

fn open_viewer_with_optional_source(
    cx: &mut TestAppContext,
    project_path: Option<PathBuf>,
    source: Option<ConfiguredSource>,
) -> (WindowHandle<ViewerApp>, VisualTestContext) {
    let test_home = std::env::temp_dir().join(format!(
        "seex-viewer-test-home-{}-{}",
        std::process::id(),
        NEXT_TEST_HOME.fetch_add(1, Ordering::Relaxed),
    ));
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(800.), px(600.)),
                ))),
                ..WindowOptions::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let mut viewer = ViewerApp::new_for_test(project_path, &test_home, window, cx);
                    if let Some(source) = source {
                        viewer.open_configured_sources(vec![source], cx);
                    }
                    viewer
                })
            },
        )
        .expect("test viewer window should open")
    });
    let visual = VisualTestContext::from_window(window.into(), cx);
    (window, visual)
}

pub(super) fn configured_source(alias: &str, root_path: &std::path::Path) -> ConfiguredSource {
    let reader = seex::Reader::builder(root_path)
        .open()
        .expect("test Source should open");
    let projects = reader
        .projects()
        .expect("test Source projects should load")
        .into_iter()
        .map(|project| project.project_id)
        .collect::<Vec<_>>();
    assert!(!projects.is_empty(), "test Source should contain a Project");
    ConfiguredSource {
        alias: SourceAlias::new(alias).expect("test Source alias should be valid"),
        root_path: root_path.to_owned(),
        projects,
    }
}

pub(super) fn source_id(alias: &str) -> DataSourceId {
    DataSourceId::from_alias(&SourceAlias::new(alias).expect("test Source alias should be valid"))
}

pub(super) fn test_source_id() -> DataSourceId {
    source_id("test-source")
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
                && panel.logical_width > 0
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
