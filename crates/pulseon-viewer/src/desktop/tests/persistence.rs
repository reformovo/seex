use gpui::{TestAppContext, px};

use super::super::test_support::*;
use super::*;
use crate::workbench::document::SavedRunRef;

#[gpui::test]
fn viewer_owned_workbench_state_is_saved_without_query_snapshots(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let path = root.path().join("workbench.state");
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, None);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.project_sidebar.update(cx, |sidebar, _| {
                sidebar.visible = false;
                sidebar.width = px(288.);
            });
            viewer.workspace.update(cx, |workspace, _| {
                workspace.metric_sidebar_compact = true;
            });
            viewer.bottom_inspector.update(cx, |inspector, _| {
                inspector.height = px(260.);
            });
            let source_id = DataSourceId::from_path(root.path());
            let project_id = ProjectId::from_string("project");
            viewer.session.update(cx, |session, session_cx| {
                session.workbench_path = Some(path.clone());
                session.last_saved_workbench = None;
                session
                    .views
                    .pin_project(ProjectRef::new(source_id.clone(), project_id.clone()));
                session.views.archive_project(ProjectRef::new(
                    source_id.clone(),
                    ProjectId::from_string("archive"),
                ));
                session.views.remove_project(ProjectRef::new(
                    source_id.clone(),
                    ProjectId::from_string("removed"),
                ));
                session
                    .views
                    .set_active_baseline(
                        Some(RunRef::new(
                            source_id.clone(),
                            project_id.clone(),
                            RunId::from_string("baseline"),
                        )),
                        true,
                    )
                    .expect("baseline should fit the visible Run limit");
                session
                    .views
                    .toggle_active_pinned_run(
                        RunRef::new(
                            source_id.clone(),
                            project_id.clone(),
                            RunId::from_string("pinned"),
                        ),
                        true,
                    )
                    .expect("pinned Run should fit the visible Run limit");
                session.views.archive_run(RunRef::new(
                    source_id,
                    project_id,
                    RunId::from_string("archived"),
                ));
                session.publish_snapshot();
                session_cx.notify();
            });
            cx.notify();
        })
        .expect("viewer should remain open");
    let mut loaded = None;
    for _ in 0..1_000 {
        cx.run_until_parked();
        loaded = WorkbenchDocument::load(&path).expect("saved document should remain readable");
        if loaded.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let loaded = loaded.expect("workbench document should be saved");

    assert!(!loaded.project_sidebar_visible);
    assert_eq!(loaded.project_sidebar_width, 288.);
    assert!(loaded.metric_sidebar_compact);
    assert_eq!(loaded.bottom_inspector_height, 260.);
    assert_eq!(loaded.views.len(), 1);
    assert!(loaded.views[0].metrics.is_empty());
    assert_eq!(loaded.pinned_projects.len(), 1);
    assert_eq!(loaded.archived_projects.len(), 1);
    assert_eq!(loaded.removed_projects.len(), 1);
    assert_eq!(loaded.archived_runs.len(), 1);
    assert!(loaded.views[0].baseline.is_some());
    assert_eq!(loaded.views[0].pinned_runs.len(), 1);
}

#[gpui::test]
fn restored_state_reconciles_removed_runs_and_unknown_metrics(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_complete_runs(2, 1);
    let document = saved_workbench(
        root.path().to_path_buf(),
        project_id,
        vec![run_id.clone(), RunId::from_string("removed")],
        "unknown-metric",
    );
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, None);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.restore_workbench(document, cx);
            cx.notify();
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.session_snapshot(cx).views.active().runs.len() == 1
            && viewer.session_snapshot(cx).views.active().runs[0].run_id == run_id
            && viewer.session_snapshot(cx).views.active().panels[0]
                .overview
                .as_ref()
                .is_some_and(|snapshot| {
                    snapshot.series.iter().all(|series| {
                        series.evidence.completeness == EvidenceCompleteness::Unavailable
                    })
                })
    });

    assert!(root.path().exists());
    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .session
                    .read(cx)
                    .transient_error
                    .as_deref()
                    .is_some_and(|error| error.contains("no longer available"))
            })
            .expect("viewer should remain open")
    );
}

#[gpui::test]
fn restored_missing_sources_retain_state_without_rendering_runs(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let missing = root.path().join("moved-source");
    let mut document = saved_workbench(
        missing.clone(),
        ProjectId::from_string("project"),
        vec![RunId::from_string("run")],
        "loss",
    );
    let saved_run = |run_id: &str| SavedRunRef {
        source_path: missing.clone(),
        project_id: ProjectId::from_string("project"),
        run_id: RunId::from_string(run_id),
    };
    document.views[0].baseline = Some(saved_run("baseline"));
    document.views[0].pinned_runs = vec![saved_run("pinned")];
    document.archived_runs = vec![saved_run("archived")];
    document.project_sidebar_visible = true;
    let (window, mut cx) = open_viewer(cx, None);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.restore_workbench(document, cx);
            cx.notify();
        })
        .expect("viewer should remain open");

    let status = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .session_snapshot(cx)
                .sources
                .iter()
                .next()
                .map(|source| (source.root_path.clone(), source.status.clone()))
        })
        .expect("viewer should remain open")
        .expect("missing source should remain listed");
    assert_eq!(status.0, missing);
    assert!(
        matches!(&status.1, SourceStatus::Failed(_)),
        "missing source should remain failed, got {:?}",
        status.1,
    );
    assert!(!missing.exists());
    window
        .read_with(&cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            assert_eq!(snapshot.views.active().runs.len(), 3);
            assert!(snapshot.views.active().baseline.is_some());
            assert_eq!(snapshot.views.active().pinned_runs.len(), 1);
            assert_eq!(snapshot.views.archived_runs().len(), 1);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    assert!(cx.debug_bounds("baseline-run-name-0").is_none());
    assert!(cx.debug_bounds("pinned-run-name-0").is_none());
    assert!(cx.debug_bounds("archived-run-name-0").is_none());
    assert!(cx.debug_bounds("empty-metric-chart:loss").is_some());
}

#[gpui::test]
fn restored_organization_records_import_their_referenced_sources(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().expect("test directory should be created");
    let missing = root.path().join("archived-source");
    let mut document = saved_workbench(
        missing.clone(),
        ProjectId::from_string("project"),
        Vec::new(),
        "loss",
    );
    document.sources.clear();
    document.views.clear();
    document.archived_projects.push(SavedProjectRef {
        source_path: missing.clone(),
        project_id: ProjectId::from_string("project"),
    });
    let (window, mut cx) = open_viewer(cx, None);

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.restore_workbench(document, cx);
            cx.notify();
        })
        .expect("viewer should remain open");

    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .sources
                .iter()
                .any(|source| source.root_path == missing))
            .expect("viewer should remain open")
    );
    assert!(!missing.exists());
}
