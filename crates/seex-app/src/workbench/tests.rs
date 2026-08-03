use seex::AlignmentAxis;
use seex_plot::AxisRange;

use crate::domain::SourceAlias;
use crate::workbench::toml_document::{
    SavedAnalysisView, SavedLayout, SavedProjectRef, SavedRunRef, TomlWorkbenchDocument,
};

use super::*;

#[test]
fn closing_the_last_view_creates_a_new_empty_view() {
    let mut views = AnalysisViews::default();
    let original = views.active().view_id.clone();

    assert!(views.close(&original));
    assert_eq!(views.views().len(), 1);
    assert_ne!(views.active().view_id, original);
    assert!(views.active().runs.is_empty());
}

#[test]
fn run_limit_allows_removal_and_is_independent_per_view() {
    let mut views = AnalysisViews::default();
    let run = |index: usize| {
        RunRef::new(
            DataSourceId::new("source").expect("test alias should be valid"),
            ProjectId::from_string("project"),
            RunId::from_string(format!("run-{index}")),
        )
    };
    for index in 0..crate::domain::MAX_SELECTED_RUNS {
        assert!(
            views
                .toggle_active_run(run(index), true)
                .expect("the first 20 Runs should be selectable")
        );
    }
    assert_eq!(
        views.toggle_active_run(run(crate::domain::MAX_SELECTED_RUNS), false),
        Err(SelectionError::RunLimit)
    );
    assert!(
        !views
            .toggle_active_run(run(0), false)
            .expect("removing a Run must remain possible while full")
    );

    views.create_empty();
    assert!(views.active().runs.is_empty());
    assert!(
        views
            .toggle_active_run(run(crate::domain::MAX_SELECTED_RUNS), true)
            .expect("a new View should have independent capacity")
    );
}

#[test]
fn run_visibility_changes_preserve_loaded_panel_state() {
    let mut views = AnalysisViews::default();
    let panel_id = views.select_active_metric(MetricKey::from_string("loss"));
    views.begin_active_panel_read(&panel_id, ReadKind::Detail, Generation(7));
    let run = RunRef::new(
        DataSourceId::new("source").expect("test alias should be valid"),
        ProjectId::from_string("project"),
        RunId::from_string("run"),
    );

    assert!(
        views
            .toggle_active_run(run.clone(), true)
            .expect("show should fit")
    );
    assert!(
        views
            .active_panel(&panel_id)
            .expect("panel should remain")
            .is_pending(ReadKind::Detail)
    );
    assert!(
        !views
            .toggle_active_run(run, true)
            .expect("hide should succeed")
    );
    assert!(
        views
            .active_panel(&panel_id)
            .expect("panel should remain")
            .is_pending(ReadKind::Detail)
    );
}

#[test]
fn run_organization_changes_preserve_loaded_panel_state() {
    let mut views = AnalysisViews::default();
    let panel_id = views.select_active_metric(MetricKey::from_string("loss"));
    views.begin_active_panel_read(&panel_id, ReadKind::Detail, Generation(9));
    let run = RunRef::new(
        DataSourceId::new("source").expect("test alias should be valid"),
        ProjectId::from_string("project"),
        RunId::from_string("run"),
    );

    views
        .set_active_baseline(Some(run.clone()), true)
        .expect("baseline should fit");
    views
        .toggle_active_pinned_run(run.clone(), true)
        .expect("pin should succeed");
    views.archive_run(run);

    assert!(
        views
            .active_panel(&panel_id)
            .expect("panel should remain")
            .is_pending(ReadKind::Detail)
    );
}

#[test]
fn duplicated_views_copy_selection_without_sharing_mutation() {
    let mut views = AnalysisViews::default();
    let run = RunRef::new(
        DataSourceId::new("source").expect("test alias should be valid"),
        seex::ProjectId::from_string("project"),
        seex::RunId::from_string("run"),
    );
    views
        .toggle_active_run(run, true)
        .expect("first Run should be selected");
    let baseline = RunRef::new(
        DataSourceId::new("source").expect("test alias should be valid"),
        ProjectId::from_string("project"),
        RunId::from_string("baseline"),
    );
    views
        .set_active_baseline(Some(baseline.clone()), true)
        .expect("baseline should fit the visible Run limit");
    let pinned = RunRef::new(
        DataSourceId::new("source").expect("test alias should be valid"),
        ProjectId::from_string("project"),
        RunId::from_string("pinned"),
    );
    assert!(
        views
            .toggle_active_pinned_run(pinned, true)
            .expect("pinned Run should fit the visible Run limit")
    );
    views.select_active_metric(MetricKey::from_string("loss"));
    views
        .active_mut()
        .navigation
        .select_axis(seex::AlignmentAxis::ElapsedTime);

    let duplicate = views.duplicate_active();
    views.active_mut().runs.clear();
    views.active_mut().baseline = None;
    views.active_mut().pinned_runs.clear();
    views.active_mut().panels.clear();
    views
        .active_mut()
        .navigation
        .select_axis(seex::AlignmentAxis::Step);
    assert!(views.activate(&AnalysisViewId::from_string("view-1")));

    assert_eq!(views.active().runs.len(), 3);
    assert!(views.active().baseline.is_some());
    assert_eq!(views.active().pinned_runs.len(), 1);
    assert_eq!(
        views
            .active()
            .panels
            .iter()
            .map(|panel| panel.metric_key.clone())
            .collect::<Vec<_>>(),
        [MetricKey::from_string("loss")]
    );
    assert_eq!(
        views.active().navigation.axis(),
        seex::AlignmentAxis::ElapsedTime
    );
    assert!(views.activate(&duplicate));
    assert!(views.active().runs.is_empty());
    assert!(views.active().baseline.is_none());
    assert!(views.active().pinned_runs.is_empty());
    assert!(views.active().panels.is_empty());
    assert_eq!(views.active().navigation.axis(), seex::AlignmentAxis::Step);
}

#[test]
fn metric_selection_is_isolated_per_view() {
    let mut views = AnalysisViews::default();
    let first_view = views.active().view_id.clone();
    let loss = views.select_active_metric(MetricKey::from_string("loss"));

    let second_view = views.create_empty();
    let accuracy = views.select_active_metric(MetricKey::from_string("accuracy"));
    assert!(views.select_active_panel(&accuracy));
    assert!(views.activate(&first_view));

    assert_eq!(views.active().selected_panel_id.as_ref(), Some(&loss));
    assert!(views.activate(&second_view));
    assert_eq!(views.active().selected_panel_id.as_ref(), Some(&accuracy));
}

#[test]
fn metric_panel_heights_use_the_compact_bounded_range() {
    let mut views = AnalysisViews::default();
    let panel_id = views.select_active_metric(MetricKey::from_string("loss"));

    assert_eq!(views.active().panels[0].row_height, 52.);
    assert!(views.set_active_panel_height(&panel_id, 1.));
    assert_eq!(views.active().panels[0].row_height, 52.);
    assert!(views.set_active_panel_height(&panel_id, 1_000.));
    assert_eq!(views.active().panels[0].row_height, 180.);
}

#[test]
fn restore_reports_duplicate_identities_and_unknown_selection() {
    let source_alias = SourceAlias::new("source").expect("test alias should be valid");
    let saved_run = SavedRunRef {
        source_alias: source_alias.clone(),
        project_id: ProjectId::from_string("project"),
        run_id: RunId::from_string("run"),
    };
    let document = TomlWorkbenchDocument {
        active_view: 0,
        layout: SavedLayout::default(),
        expanded_projects: Vec::new(),
        pinned_projects: vec![SavedProjectRef {
            source_alias,
            project_id: ProjectId::from_string("project"),
        }],
        archived_projects: Vec::new(),
        archived_runs: Vec::new(),
        views: vec![SavedAnalysisView {
            name: "Duplicates".to_owned(),
            runs: vec![saved_run.clone(), saved_run],
            baseline: None,
            pinned_runs: Vec::new(),
            metrics: vec!["loss".to_owned(), "loss".to_owned()],
            metric_heights: Vec::new(),
            selected_metric: Some("unknown".to_owned()),
            axis: AlignmentAxis::Step,
            viewport: Some(AxisRange::new(0., 1.).expect("viewport should be valid")),
        }],
    };

    let (views, issues) = AnalysisViews::restore(&document);

    assert_eq!(views.active().runs.len(), 1);
    assert_eq!(views.active().panels.len(), 1);
    assert_eq!(views.pinned_projects().len(), 1);
    assert!(views.active().selected_panel_id.is_none());
    assert_eq!(issues.len(), 3);
}

#[test]
fn workbench_round_trip_retains_runs_beyond_the_live_limit() {
    let source_alias = SourceAlias::new("offline-source").expect("test alias should be valid");
    let project_id = ProjectId::from_string("project");
    let runs = (0..25)
        .map(|index| SavedRunRef {
            source_alias: source_alias.clone(),
            project_id: project_id.clone(),
            run_id: RunId::from_string(format!("run-{index}")),
        })
        .collect::<Vec<_>>();
    let document = TomlWorkbenchDocument {
        active_view: 0,
        layout: SavedLayout::default(),
        expanded_projects: Vec::new(),
        pinned_projects: Vec::new(),
        archived_projects: Vec::new(),
        archived_runs: Vec::new(),
        views: vec![SavedAnalysisView {
            name: "Over limit".to_owned(),
            runs,
            baseline: None,
            pinned_runs: Vec::new(),
            metrics: Vec::new(),
            metric_heights: Vec::new(),
            selected_metric: None,
            axis: AlignmentAxis::Step,
            viewport: None,
        }],
    };

    let decoded = TomlWorkbenchDocument::decode(&document.encode())
        .expect("schema-v1 workbench document should round-trip");
    let (views, issues) = AnalysisViews::restore(&decoded);

    assert!(issues.is_empty());
    assert_eq!(decoded.views[0].runs.len(), 25);
    assert_eq!(views.active().runs.len(), 25);
}

#[test]
fn restored_organization_uses_composite_source_identities() {
    let project_id = ProjectId::from_string("shared-project");
    let saved_project = |source: &str| SavedProjectRef {
        source_alias: SourceAlias::new(source).expect("test alias should be valid"),
        project_id: project_id.clone(),
    };
    let document = TomlWorkbenchDocument {
        active_view: 0,
        layout: SavedLayout::default(),
        expanded_projects: Vec::new(),
        pinned_projects: vec![saved_project("source-a")],
        archived_projects: vec![saved_project("source-b")],
        archived_runs: Vec::new(),
        views: Vec::new(),
    };

    let (views, issues) = AnalysisViews::restore(&document);

    assert!(issues.is_empty());
    assert_ne!(views.pinned_projects()[0], views.archived_projects()[0]);
    assert_eq!(views.pinned_projects()[0].project_id, project_id);
}

#[test]
fn view_organization_is_local_while_archived_runs_are_shared() {
    let mut views = AnalysisViews::default();
    let run = |name: &str| {
        RunRef::new(
            DataSourceId::new("source").expect("test alias should be valid"),
            ProjectId::from_string("project"),
            RunId::from_string(name),
        )
    };
    views
        .set_active_baseline(Some(run("baseline")), true)
        .expect("baseline should fit the visible Run limit");
    assert!(
        views
            .toggle_active_pinned_run(run("pinned"), true)
            .expect("pinned Run should fit the visible Run limit")
    );
    let first = views.active().view_id.clone();
    let second = views.create_empty();

    assert!(views.active().baseline.is_none());
    assert!(views.active().pinned_runs.is_empty());
    let archived = run("archived");
    views
        .toggle_active_run(archived.clone(), true)
        .expect("archived candidate should fit the visible Run limit");
    views.archive_run(archived.clone());
    assert!(views.active().runs.contains(&archived));
    assert!(views.activate(&first));
    assert_eq!(views.active().baseline.as_ref(), Some(&run("baseline")));
    assert_eq!(views.active().pinned_runs, [run("pinned")]);
    assert_eq!(views.archived_runs(), std::slice::from_ref(&archived));
    assert!(views.activate(&second));
    assert_eq!(views.archived_runs(), std::slice::from_ref(&archived));
    views.restore_run(&archived);
    assert!(views.active().runs.contains(&archived));
}

#[test]
fn removed_projects_clear_every_view_without_touching_other_projects() {
    let mut views = AnalysisViews::default();
    let project = ProjectRef::new(
        DataSourceId::new("source").expect("test alias should be valid"),
        ProjectId::from_string("removed"),
    );
    let run = |name: &str| {
        RunRef::new(
            project.source_id.clone(),
            project.project_id.clone(),
            RunId::from_string(name),
        )
    };
    views
        .set_active_baseline(Some(run("baseline")), true)
        .expect("baseline should fit the visible Run limit");
    views
        .toggle_active_pinned_run(run("pinned"), true)
        .expect("pinned Run should fit the visible Run limit");
    views.archive_run(run("archived"));
    views.pin_project(project.clone());
    views.create_empty();
    views
        .toggle_active_run(run("second-view"), true)
        .expect("second View Run should fit");
    let retained = RunRef::new(
        project.source_id.clone(),
        ProjectId::from_string("retained"),
        RunId::from_string("retained"),
    );
    views
        .toggle_active_run(retained.clone(), true)
        .expect("other Project Run should fit");

    views.remove_project(project.clone());

    assert_eq!(views.removed_projects(), std::slice::from_ref(&project));
    assert!(views.pinned_projects().is_empty());
    assert!(views.archived_projects().is_empty());
    assert!(views.archived_runs().is_empty());
    assert!(views.views().iter().all(|view| {
        view.baseline.is_none()
            && view.pinned_runs.is_empty()
            && view.runs.iter().all(|run| !project.contains_run(run))
    }));
    assert!(views.active().runs.contains(&retained));
}

#[test]
fn timeline_home_is_the_union_of_loaded_metric_extents() {
    let mut views = AnalysisViews::default();
    views.record_active_metric_extent(
        MetricKey::from_string("loss"),
        Some(AlignmentViewport::new(10, 20).expect("test extent should be valid")),
    );

    let home = views
        .record_active_metric_extent(
            MetricKey::from_string("accuracy"),
            Some(AlignmentViewport::new(5, 15).expect("test extent should be valid")),
        )
        .expect("metric extents should produce a timeline home");

    assert_eq!((home.start(), home.end()), (5, 20));
}

#[test]
fn metric_panels_keep_independent_generations_and_source_errors() {
    let mut views = AnalysisViews::default();
    let first = views.select_active_metric(MetricKey::from_string("loss"));
    let second = views.select_active_metric(MetricKey::from_string("accuracy"));
    views.begin_active_panel_read(&first, ReadKind::Detail, Generation(1));
    views.begin_active_panel_read(&second, ReadKind::Detail, Generation(2));

    assert!(!views.complete_active_panel_read(
        &first,
        ReadKind::Detail,
        Generation(2),
        PanelReadMode::Replace,
        None,
        Vec::new(),
    ));
    assert!(views.complete_active_panel_read(
        &second,
        ReadKind::Detail,
        Generation(2),
        PanelReadMode::Replace,
        None,
        vec![SourceReadFailure {
            source_id: DataSourceId::new("source-b").expect("test alias should be valid"),
            message: "unavailable".to_owned(),
        }],
    ));

    assert!(
        views
            .active_panel(&first)
            .expect("first panel should exist")
            .is_pending(ReadKind::Detail)
    );
    assert_eq!(
        views
            .active_panel(&second)
            .expect("second panel should exist")
            .source_errors
            .len(),
        1
    );
    views.begin_active_panel_read(&first, ReadKind::Inspector, Generation(3));
    assert!(!views.complete_active_inspector_read(&first, Generation(2), None, Vec::new(),));
    assert!(views.complete_active_inspector_read(&first, Generation(3), None, Vec::new(),));
}

#[test]
fn deactivating_a_view_cancels_every_panel_generation() {
    let mut views = AnalysisViews::default();
    let panel = views.select_active_metric(MetricKey::from_string("loss"));
    let viewport = AlignmentViewport::new(10, 20).expect("viewport should be valid");
    views.begin_active_panel_read(&panel, ReadKind::Overview, Generation(1));
    views.begin_active_panel_detail(&panel, Generation(2), viewport, 800);
    views.begin_active_panel_read(&panel, ReadKind::Inspector, Generation(3));

    views.cancel_active_panel_reads();

    let panel = views.active_panel(&panel).expect("panel should remain");
    assert!(!panel.is_pending(ReadKind::Overview));
    assert!(!panel.is_pending(ReadKind::Detail));
    assert!(!panel.is_pending(ReadKind::Inspector));
    assert!(panel.requested_detail_viewport.is_none());
}
