use gpui::{KeyBinding, Modifiers, ScrollDelta, TestAppContext, TouchPhase, point, px, size};

use super::super::test_support::*;
use super::*;
use crate::desktop::ToggleProjectSidebar;

#[gpui::test]
fn project_sidebar_toggle_expands_the_analysis_workspace(cx: &mut TestAppContext) {
    let (window, mut cx) = open_viewer(cx, None);
    let analysis_before = cx
        .debug_bounds("analysis-tab")
        .expect("Analysis workspace should render");
    assert!(cx.debug_bounds("project-run-tree").is_some());

    cx.dispatch_action(ToggleProjectSidebar);
    cx.refresh().expect("test window should refresh");
    cx.run_until_parked();

    assert!(
        !window
            .read_with(&cx, |viewer, cx| viewer.sidebar_visible(cx))
            .expect("viewer should remain open")
    );
    let left_controls = cx
        .debug_bounds("analysis-left-controls")
        .expect("sidebar reveal group should render");
    let show_sidebar = cx
        .debug_bounds("show-project-sidebar")
        .expect("sidebar reveal control should render");
    let tabs = cx
        .debug_bounds("analysis-view-tabs")
        .expect("View tabs should remain rendered");
    assert_eq!(show_sidebar.origin.x, left_controls.origin.x + px(4.));
    assert_eq!(tabs.origin.x, left_controls.right());
    let analysis_after = cx
        .debug_bounds("analysis-tab")
        .expect("Analysis workspace should remain rendered");
    assert!(analysis_after.origin.x < analysis_before.origin.x);
}

#[gpui::test]
fn project_sidebar_width_resizes_from_its_boundary(cx: &mut TestAppContext) {
    let (window, mut cx) = open_viewer(cx, None);
    let sidebar_width = cx
        .debug_bounds("project-sidebar")
        .expect("Project sidebar should render")
        .size
        .width;
    let sidebar_resize = cx
        .debug_bounds("project-sidebar-resize")
        .expect("Project sidebar resize boundary should render");
    let resize_target = point(
        sidebar_resize.center().x + px(48.),
        sidebar_resize.center().y,
    );
    cx.simulate_mouse_down(
        sidebar_resize.center(),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.simulate_mouse_move(resize_target, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(resize_target, MouseButton::Left, Modifiers::default());

    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer.sidebar_width(cx))
            .expect("viewer should remain open"),
        sidebar_width + px(48.)
    );
}

#[gpui::test]
fn archived_section_resizes_from_its_top_boundary(cx: &mut TestAppContext) {
    let (window, mut cx) = open_viewer(cx, None);
    let archived = cx
        .debug_bounds("archived-run-tree")
        .expect("Archived section should render");
    let resize = cx
        .debug_bounds("archived-sidebar-resize")
        .expect("Archived resize boundary should render");
    let resize_target = point(resize.center().x, resize.center().y - px(36.));

    cx.simulate_mouse_down(resize.center(), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(resize_target, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(resize_target, MouseButton::Left, Modifiers::default());

    let resized = cx
        .debug_bounds("archived-run-tree")
        .expect("Archived section should remain rendered");
    assert!(
        resized.size.height >= archived.size.height + px(30.),
        "dragging the Archived boundary upward should increase its height",
    );
    assert!(
        window
            .read_with(&cx, |viewer, inner_cx| viewer
                .project_sidebar
                .read(inner_cx)
                .archived_resize
                .is_none())
            .expect("viewer should remain open"),
        "releasing the Archived boundary should clear its active resize state",
    );
}

#[gpui::test]
fn project_filter_uses_placeholder_and_blinking_caret_states(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, None);
    let placeholder = cx
        .debug_bounds("project-run-filter-placeholder")
        .expect("Project filter placeholder should render");

    let filter = cx
        .debug_bounds("project-run-filter")
        .expect("Project filter should render");
    assert!(placeholder.right() <= filter.right());
    cx.simulate_mouse_move(filter.center(), None, Modifiers::default());
    cx.simulate_click(filter.center(), Modifiers::default());
    assert!(
        window
            .update(&mut cx, |viewer, window, cx| {
                viewer
                    .project_sidebar
                    .read(cx)
                    .filter_focus
                    .is_focused(window)
            })
            .expect("viewer should remain open")
    );
    assert!(cx.debug_bounds("project-run-filter-caret").is_some());

    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert!(
        !window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .project_sidebar
                    .read(cx)
                    .filter
                    .read(cx)
                    .cursor_visible()
            })
            .expect("viewer should remain open")
    );
    cx.simulate_keystrokes("viewer left left right x");
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .project_sidebar
                    .read(cx)
                    .filter
                    .read(cx)
                    .text()
                    .to_owned()
            })
            .expect("viewer should remain open"),
        "viewexr"
    );
    let prefix = cx
        .debug_bounds("project-run-filter-value")
        .expect("filter value before the cursor should render");
    let caret = cx
        .debug_bounds("project-run-filter-caret")
        .expect("typing should reveal the filter caret");
    assert_eq!(caret.origin.x, prefix.right() + px(1.));
    assert!(cx.debug_bounds("project-run-filter-suffix").is_some());
}

#[gpui::test]
fn run_markers_align_with_project_icons_and_run_labels(cx: &mut TestAppContext) {
    let (root, project_id, _) = fixture_with_runs(0, 9);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .next()
            .is_some_and(|source| source.catalog.runs.len() == 9)
    });
    window
        .update(&mut cx, |viewer, _, cx| {
            let source = viewer
                .session_snapshot(cx)
                .sources
                .iter()
                .next()
                .expect("source")
                .clone();
            let run_ref = |index: usize| {
                RunRef::new(
                    source.source_id.clone(),
                    project_id.clone(),
                    source.catalog.runs[index].run_id.clone(),
                )
            };
            viewer.set_run_baseline(run_ref(0), cx);
            viewer.toggle_pinned_run(run_ref(1), cx);
            for index in 2..8 {
                viewer.archive_run(run_ref(index), cx);
            }
        })
        .expect("viewer should remain open");

    let project_label = cx
        .debug_bounds("project-tree-label-0-0")
        .expect("Project label should render");
    let project_folder = cx
        .debug_bounds("project-folder-0-0")
        .expect("Project folder should render");
    let baseline_label = cx
        .debug_bounds("baseline-run-name-0")
        .expect("Baseline Run label should render");
    let pinned_label = cx
        .debug_bounds("pinned-run-name-0")
        .expect("Pinned Run label should render");
    let baseline_color = cx
        .debug_bounds("run-color-baseline-0")
        .expect("Baseline color marker should render");
    let pinned_color = cx
        .debug_bounds("run-color-pinned-0")
        .expect("Pinned color marker should render");
    let archived_label = cx
        .debug_bounds("archived-run-name-0")
        .expect("Archived Run label should render");
    let archived_color = cx
        .debug_bounds("run-color-archived-0")
        .expect("Archived color marker should render");
    let archived_more = cx
        .debug_bounds("show-more-archived")
        .expect("Archived pagination should render");
    assert_eq!(baseline_label.origin.x, pinned_label.origin.x);
    assert_eq!(baseline_label.origin.x, archived_label.origin.x);
    assert_eq!(archived_more.origin.x, archived_label.origin.x);
    for color in [baseline_color, pinned_color, archived_color] {
        assert_eq!(color.origin.x, project_folder.origin.x);
    }
    let sidebar = cx
        .debug_bounds("project-sidebar")
        .expect("Project sidebar should render");
    let archived_tree = cx
        .debug_bounds("archived-run-tree")
        .expect("Archived section should render");
    assert_eq!(archived_tree.bottom(), sidebar.bottom());
    let project_row = cx
        .debug_bounds("project-tree-row-0-0")
        .expect("Project row should render");
    cx.simulate_click(project_row.center(), Modifiers::default());
    let nested_run = cx
        .debug_bounds("project-run-name-0")
        .expect("nested Run label should render");
    let nested_eye = cx
        .debug_bounds("run-eye-0")
        .expect("nested Run eye should render");
    let nested_color = cx
        .debug_bounds("run-color-project-0")
        .expect("nested Run color marker should render");
    assert_eq!(nested_eye.origin.x, project_folder.origin.x);
    assert!(nested_color.origin.x > baseline_color.origin.x);
    assert!(nested_run.origin.x > baseline_label.origin.x);
    assert!(nested_run.origin.x > project_label.origin.x);
    assert!(nested_color.center().y >= nested_run.center().y);
    assert!(f32::from(nested_color.center().y - nested_run.center().y) <= 2.);

    window
        .update(&mut cx, |viewer, _, cx| {
            let run_ref = {
                let session = viewer.session_snapshot(cx);
                let source = session.sources.iter().next().expect("source");
                RunRef::new(
                    source.source_id.clone(),
                    project_id.clone(),
                    source.catalog.runs[8].run_id.clone(),
                )
            };
            viewer.archive_run(run_ref, cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    let no_runs = cx
        .debug_bounds("project-no-runs")
        .expect("Empty Project should render its placeholder");
    assert_eq!(no_runs.origin.x, project_label.origin.x);
}

#[gpui::test]
fn project_tree_scrolls_to_runs_in_an_expanded_project(cx: &mut TestAppContext) {
    let (root, project_id, _) = fixture_with_runs(0, 12);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    let folder = cx
        .debug_bounds("project-folder-0-0")
        .expect("first Project folder should be rendered");
    cx.simulate_click(folder.center(), Modifiers::default());
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .next()
            .is_some_and(|source| {
                source
                    .catalog
                    .runs
                    .iter()
                    .any(|run| run.project_id == project_id)
            })
    });
    let tree = cx
        .debug_bounds("project-run-tree")
        .expect("Project tree should be rendered");
    let first_row = cx
        .debug_bounds("project-tree-run-0-0-0")
        .expect("first Run row should be rendered");
    let second_row = cx
        .debug_bounds("project-tree-run-0-0-1")
        .expect("second Run row should be rendered");
    let project_label = cx
        .debug_bounds("project-tree-label-0-0")
        .expect("Project label should render");
    let show_more = cx
        .debug_bounds("show-more-0-0")
        .expect("Project pagination should render");
    assert_eq!(show_more.origin.x, project_label.origin.x);
    let expected_row_height = window
        .read_with(&cx, |viewer, _| viewer.theme.spacing.tree_row_height)
        .expect("viewer should remain open");
    assert_eq!(first_row.size.height, expected_row_height);
    let eye_width = cx
        .debug_bounds("run-eye-0")
        .expect("Run visibility control should render")
        .size
        .width;
    assert!(cx.debug_bounds("run-status-0").is_some());
    let action_bounds = cx
        .debug_bounds("run-actions-0")
        .expect("Run actions should keep a stable layout slot");

    cx.simulate_mouse_move(
        point(first_row.origin.x + px(40.), first_row.center().y),
        None,
        Modifiers::default(),
    );

    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer.interaction_snapshot(cx).emphasized_run.is_some()
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
    cx.simulate_mouse_move(
        point(second_row.origin.x + px(40.), second_row.center().y),
        None,
        Modifiers::default(),
    );
    let second_run = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .interaction_snapshot(cx)
                .emphasized_run
                .expect("second Run should replace the first emphasis")
        })
        .expect("viewer should remain open");
    assert_ne!(second_run, first_run);
    cx.simulate_mouse_move(
        point(first_row.origin.x + px(40.), first_row.center().y),
        None,
        Modifiers::default(),
    );
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .interaction_snapshot(cx)
                .emphasized_run)
            .expect("viewer should remain open"),
        Some(first_run),
        "Run emphasis should hand off correctly in both vertical directions",
    );
    assert_eq!(
        cx.debug_bounds("run-actions-0")
            .expect("Run actions should remain laid out"),
        action_bounds
    );
    assert_eq!(
        cx.debug_bounds("run-eye-0")
            .expect("Run visibility control should keep its width")
            .size
            .width,
        eye_width
    );
    let analysis_tab = cx
        .debug_bounds("analysis-tab")
        .expect("Analysis tab should render");
    cx.simulate_mouse_move(analysis_tab.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer.interaction_snapshot(cx).emphasized_run.is_none()
            })
            .expect("viewer should remain open")
    );
    window
        .update(&mut cx, |viewer, window, cx| {
            let session = viewer.session_snapshot(cx);
            let source = session.sources.iter().next().expect("source");
            let run = source.catalog.runs.first().expect("first Run");
            let run_ref = RunRef::new(
                source.source_id.clone(),
                run.project_id.clone(),
                run.run_id.clone(),
            );
            let focus = viewer
                .project_sidebar
                .read(cx)
                .run_focuses
                .get(&run_ref)
                .expect("rendered Run should own focus")
                .clone();
            focus.focus(window);
            cx.notify();
        })
        .expect("viewer should remain open");
    assert_eq!(
        cx.debug_bounds("run-eye-0")
            .expect("focused Run eye should keep its width")
            .size
            .width,
        eye_width
    );

    for _ in 0..2 {
        let show_more = cx
            .debug_bounds("show-more-0-0")
            .expect("Project pagination should expose more Runs");
        cx.simulate_click(show_more.center(), Modifiers::default());
    }

    cx.simulate_event(ScrollWheelEvent {
        position: tree.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-1_000.))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });

    assert!(cx.debug_bounds("project-tree-run-0-0-11").is_some());
}

#[gpui::test]
fn project_filter_finds_runs_beyond_the_revealed_page(cx: &mut TestAppContext) {
    let (root, _, _) = fixture_with_runs(0, 12);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    window
        .update(&mut cx, |viewer, _, cx| {
            let filter = viewer.project_sidebar.read(cx).filter.clone();
            filter.update(cx, |input, _| input.set_text("baseline 11"));
            cx.notify();
        })
        .expect("viewer should remain open");
    let folder = cx
        .debug_bounds("project-folder-0-0")
        .expect("matching Project should remain visible");

    cx.simulate_click(folder.center(), Modifiers::default());
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .next()
            .is_some_and(|source| source.catalog.runs.len() == 12)
    });

    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .project_sidebar
                    .read(cx)
                    .filter
                    .read(cx)
                    .text()
                    .to_owned()
            })
            .expect("viewer should remain open"),
        "baseline 11"
    );
    assert!(cx.debug_bounds("project-tree-run-0-0-0").is_some());
    assert!(cx.debug_bounds("project-tree-run-0-0-1").is_none());
    assert!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .sources
                .iter()
                .next()
                .is_some_and(|source| source
                    .catalog
                    .runs
                    .iter()
                    .any(|run| run.name == "baseline 11")))
            .expect("viewer should remain open")
    );
    assert!(cx.debug_bounds("show-more-0-0").is_none());

    window
        .update(&mut cx, |viewer, _, cx| {
            let filter = viewer.project_sidebar.read(cx).filter.clone();
            filter.update(cx, |input, _| input.set_text("viewer"));
            cx.notify();
        })
        .expect("viewer should remain open");
    for selector in [
        "project-tree-run-0-0-0",
        "project-tree-run-0-0-1",
        "project-tree-run-0-0-2",
        "project-tree-run-0-0-3",
        "project-tree-run-0-0-4",
    ] {
        assert!(cx.debug_bounds(selector).is_some());
    }
    assert!(cx.debug_bounds("show-more-0-0").is_some());
}

#[gpui::test]
fn project_visibility_keeps_baseline_and_pinned_runs_visible(cx: &mut TestAppContext) {
    let (root, project_id, _) = fixture_with_runs(0, 3);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .next()
            .is_some_and(|source| source.catalog.runs.len() == 3)
    });
    let (project, baseline, pinned, ordinary) = window
        .update(&mut cx, |viewer, _, cx| {
            let source = viewer
                .session_snapshot(cx)
                .sources
                .iter()
                .next()
                .expect("source")
                .clone();
            let project = SidebarProject {
                source_index: 0,
                project_index: 0,
                project_ref: ProjectRef::new(source.source_id.clone(), project_id.clone()),
                project: source.catalog.projects[0].clone(),
                runs: source.catalog.runs.clone(),
                source_label: "source".to_owned(),
                placement: ProjectPlacement::Projects,
            };
            let run_ref = |index: usize| {
                RunRef::new(
                    source.source_id.clone(),
                    project_id.clone(),
                    project.runs[index].run_id.clone(),
                )
            };
            let baseline = run_ref(0);
            let pinned = run_ref(1);
            let ordinary = run_ref(2);
            viewer.set_run_baseline(baseline.clone(), cx);
            viewer.toggle_pinned_run(pinned.clone(), cx);
            viewer.toggle_run(ordinary.clone(), cx);
            (project, baseline, pinned, ordinary)
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.toggle_project_runs(&project, cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            assert!(
                viewer
                    .session_snapshot(cx)
                    .views
                    .active()
                    .runs
                    .contains(&baseline)
            );
            assert!(
                viewer
                    .session_snapshot(cx)
                    .views
                    .active()
                    .runs
                    .contains(&pinned)
            );
            assert!(
                !viewer
                    .session_snapshot(cx)
                    .views
                    .active()
                    .runs
                    .contains(&ordinary)
            );
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn run_limit_disables_the_twenty_first_run_and_keeps_project_actions_atomic(
    cx: &mut TestAppContext,
) {
    let (root, project_id, _) = fixture_with_runs(0, 21);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .next()
            .is_some_and(|source| source.catalog.runs.len() == 21)
    });
    let (runs, blocked_name) = window
        .read_with(&cx, |viewer, cx| {
            let source = &viewer.session_snapshot(cx).sources[0];
            let runs = source
                .catalog
                .runs
                .iter()
                .map(|run| {
                    RunRef::new(
                        source.source_id.clone(),
                        project_id.clone(),
                        run.run_id.clone(),
                    )
                })
                .collect::<Vec<_>>();
            (runs, source.catalog.runs[20].name.clone())
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            for run in runs.iter().take(20) {
                viewer.toggle_run(run.clone(), cx);
            }
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .active_catalog_run_count())
            .expect("viewer should remain open"),
        20
    );
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.session.update(cx, |session, session_cx| {
                session.views.active_mut().runs.push(RunRef::new(
                    DataSourceId::new("offline-source").expect("test alias should be valid"),
                    project_id.clone(),
                    RunId::from_string("offline-run"),
                ));
                session.publish_snapshot();
                session_cx.notify();
            });
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            assert_eq!(snapshot.views.active().runs.len(), 21);
            assert_eq!(snapshot.active_catalog_run_count(), 20);
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.project_sidebar.update(cx, |sidebar, cx| {
                sidebar.menu = Some(ProjectRef::new(
                    runs[0].source_id.clone(),
                    project_id.clone(),
                ));
                cx.notify();
            });
        })
        .expect("viewer should remain open");
    cx.refresh().expect("Project menu should render");
    assert!(cx.debug_bounds("hide-all-project-runs").is_some());
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.project_sidebar.update(cx, |sidebar, cx| {
                sidebar.menu = None;
                cx.notify();
            });
        })
        .expect("viewer should remain open");
    cx.refresh().expect("Project menu should close");

    window
        .update(&mut cx, |viewer, _, cx| {
            let filter = viewer.project_sidebar.read(cx).filter.clone();
            filter.update(cx, |input, _| input.set_text(&blocked_name));
            cx.notify();
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    let project_row = cx
        .debug_bounds("project-tree-row-0-0")
        .expect("filtered Project row should remain visible");
    cx.simulate_click(project_row.center(), Modifiers::default());
    cx.run_until_parked();
    let blocked = cx
        .debug_bounds("project-tree-run-0-0-0")
        .expect("the filtered twenty-first Run should render");
    cx.simulate_click(blocked.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .active_catalog_run_count())
            .expect("viewer should remain open"),
        20
    );
    let blocked_eye = cx
        .debug_bounds("run-eye-0")
        .expect("the disabled eye should retain its layout slot");
    cx.simulate_mouse_move(blocked_eye.center(), None, Modifiers::default());
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert!(cx.debug_bounds("label-tooltip").is_some());

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.toggle_run(runs[0].clone(), cx)
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    cx.simulate_click(blocked.center(), Modifiers::default());
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            assert_eq!(snapshot.active_catalog_run_count(), 20);
            assert!(snapshot.views.active().runs.contains(&runs[20]));
        })
        .expect("viewer should remain open");

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.dispatch_workbench_command(
                WorkbenchCommand::SetProjectRuns {
                    runs: runs.clone(),
                    selected: false,
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .active_catalog_run_count())
            .expect("viewer should remain open"),
        0
    );

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.dispatch_workbench_command(
                WorkbenchCommand::SetProjectRuns {
                    runs: runs.clone(),
                    selected: true,
                },
                cx,
            );
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .active_catalog_run_count())
            .expect("viewer should remain open"),
        0
    );
}

#[gpui::test]
fn baseline_pin_and_archived_restore_cannot_bypass_the_run_limit(cx: &mut TestAppContext) {
    let (root, project_id, _) = fixture_with_runs(0, 21);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .next()
            .is_some_and(|source| source.catalog.runs.len() == 21)
    });
    let runs = window
        .read_with(&cx, |viewer, cx| {
            let source = &viewer.session_snapshot(cx).sources[0];
            source
                .catalog
                .runs
                .iter()
                .map(|run| {
                    RunRef::new(
                        source.source_id.clone(),
                        project_id.clone(),
                        run.run_id.clone(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            for run in runs.iter().take(20) {
                viewer.toggle_run(run.clone(), cx);
            }
        })
        .expect("viewer should remain open");
    cx.run_until_parked();

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.set_run_baseline(runs[20].clone(), cx);
            viewer.toggle_pinned_run(runs[20].clone(), cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            assert_eq!(snapshot.active_catalog_run_count(), 20);
            assert!(snapshot.views.active().baseline.is_none());
            assert!(!snapshot.views.active().pinned_runs.contains(&runs[20]));
            assert!(!snapshot.views.active().runs.contains(&runs[20]));
        })
        .expect("viewer should remain open");

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.archive_run(runs[0].clone(), cx)
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.set_run_baseline(runs[20].clone(), cx)
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.archive_run(runs[0].clone(), cx)
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            assert_eq!(snapshot.active_catalog_run_count(), 20);
            assert_eq!(snapshot.views.active().baseline.as_ref(), Some(&runs[20]));
            assert!(snapshot.views.archived_runs().contains(&runs[0]));
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn project_row_click_toggles_runs_without_changing_analysis(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture_with_complete_runs(1, 1);
    cx.executor().allow_parking();
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new(
            "enter",
            ActivateSelection,
            Some(SELECTABLE_CONTEXT),
        )]);
    });
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 1);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, first_panel_detail_is_settled);
    let before = window
        .read_with(&cx, |viewer, cx| {
            let panel = viewer.session_snapshot(cx).views.active().panels[0].clone();
            (
                viewer
                    .active_navigation(cx)
                    .brush()
                    .expect("timeline should be loaded"),
                viewer.session.read(cx).next_generation,
                panel.overview.clone().expect("overview should be loaded"),
                panel.detail.clone().expect("detail should be loaded"),
                panel.overview_revision,
                panel.detail_revision,
            )
        })
        .expect("viewer should remain open");
    let project = cx
        .debug_bounds("project-tree-row-0-0")
        .expect("first Project row should be rendered");
    cx.simulate_click(project.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(cx.debug_bounds("project-tree-run-0-0-0").is_some());
    window
        .read_with(&cx, |viewer, cx| {
            let panel = viewer.session_snapshot(cx).views.active().panels[0].clone();
            assert_eq!(viewer.active_navigation(cx).brush(), Some(before.0));
            assert_eq!(viewer.session.read(cx).next_generation, before.1);
            assert!(Arc::ptr_eq(
                panel
                    .overview
                    .as_ref()
                    .expect("overview should remain loaded"),
                &before.2,
            ));
            assert!(Arc::ptr_eq(
                panel.detail.as_ref().expect("detail should remain loaded"),
                &before.3,
            ));
            assert_eq!(panel.overview_revision, before.4);
            assert_eq!(panel.detail_revision, before.5);
        })
        .expect("viewer should remain open");
    assert!(cx.debug_bounds("overview-chart").is_some());
    assert!(cx.debug_bounds("ruler-major-tick-0").is_some());
    assert!(cx.debug_bounds("project-information-project").is_none());
    cx.simulate_mouse_move(project.center(), None, Modifiers::default());
    assert!(cx.debug_bounds("project-information-project").is_some());
    assert!(cx.debug_bounds("project-menu-project").is_some());
    cx.simulate_click(project.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer.project_sidebar.read(cx).expanded_projects.len()
            })
            .expect("viewer should remain open"),
        0,
    );
    window
        .read_with(&cx, |viewer, cx| {
            assert_eq!(viewer.active_navigation(cx).brush(), Some(before.0));
            assert_eq!(viewer.session.read(cx).next_generation, before.1);
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn project_sidebar_retains_multiple_configured_sources(cx: &mut TestAppContext) {
    let (first, _, _) = fixture_with_metric("loss");
    let (second, _, _) = fixture_with_metric("accuracy");
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(first.path().to_path_buf()));
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .sources
            .iter()
            .next()
            .is_some_and(|source| !source.catalog.projects.is_empty())
    });
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
            && viewer
                .session_snapshot(cx)
                .sources
                .iter()
                .all(|source| !source.catalog.projects.is_empty())
    });

    assert!(cx.debug_bounds("project-tree-row-0-0").is_some());
    assert!(cx.debug_bounds("project-tree-row-1-0").is_some());
    assert!(cx.debug_bounds("analysis-tab").is_some());

    for (source_index, folder_selector, run_selector) in [
        (0, "project-folder-0-0", "project-tree-run-0-0-0"),
        (1, "project-folder-1-0", "project-tree-run-1-0-0"),
    ] {
        let folder = cx
            .debug_bounds(folder_selector)
            .expect("Project folder should be rendered");
        cx.simulate_click(folder.center(), Modifiers::default());
        wait_for_viewer(window, &cx, |viewer, cx| {
            viewer
                .session_snapshot(cx)
                .sources
                .get(source_index)
                .is_some_and(|source| !source.catalog.runs.is_empty())
        });
        let run = cx
            .debug_bounds(run_selector)
            .expect("Run row should be rendered");
        cx.simulate_click(run.center(), Modifiers::default());
    }
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| viewer
                .session_snapshot(cx)
                .views
                .active()
                .runs
                .len())
            .expect("viewer should remain open"),
        2
    );
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.available_metric_keys(cx).len() == 2
    });
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .available_metric_keys(cx)
                    .into_iter()
                    .map(|metric| metric.as_str().to_owned())
                    .collect::<Vec<_>>()
            })
            .expect("viewer should remain open"),
        ["accuracy", "loss"]
    );
}

#[gpui::test]
fn run_organization_reuses_every_loaded_metric_snapshot(cx: &mut TestAppContext) {
    let (root, project_id, first_run_id) = fixture_with_complete_runs(2, 2);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 2);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.select_metric(MetricKey::from_string("metric-0"), cx);
            viewer.select_metric(MetricKey::from_string("metric-1"), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
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
            })
    });
    let initial_revisions = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .map(|panel| panel.detail_revision)
                .collect::<Vec<_>>()
        })
        .expect("viewer should remain open");
    let second_run = window
        .read_with(&cx, |viewer, cx| {
            RunRef::new(
                first_source_id(viewer, cx),
                project_id.clone(),
                RunId::from_string(
                    "run-1-with-a-very-long-identifier-that-requires-horizontal-scrolling",
                ),
            )
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.toggle_run(second_run.clone(), cx);
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer
            .session_snapshot(cx)
            .views
            .active()
            .panels
            .iter()
            .all(|panel| {
                panel
                    .detail
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.series.len() == 2)
            })
    });
    window
        .read_with(&cx, |viewer, cx| {
            assert_eq!(
                viewer
                    .session_snapshot(cx)
                    .views
                    .active()
                    .panels
                    .iter()
                    .map(|panel| panel.detail_revision)
                    .collect::<Vec<_>>(),
                initial_revisions,
            );
        })
        .expect("viewer should remain open");
    let before = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .map(|panel| {
                    (
                        panel.detail.clone().expect("detail should be loaded"),
                        panel.detail_revision,
                    )
                })
                .collect::<Vec<_>>()
        })
        .expect("viewer should remain open");
    let timeline_before = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .active_navigation(cx)
                .brush()
                .expect("timeline should be loaded")
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            let hidden = viewer.session_snapshot(cx).views.active().runs[1].clone();
            viewer.toggle_run(hidden, cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            assert_eq!(viewer.session_snapshot(cx).views.active().runs.len(), 1);
            for (panel, (snapshot, revision)) in viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .zip(&before)
            {
                assert!(Arc::ptr_eq(
                    panel.detail.as_ref().expect("detail should remain loaded"),
                    snapshot,
                ));
                assert_eq!(panel.detail_revision, *revision);
            }
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            let baseline = viewer.session_snapshot(cx).views.active().runs[0].clone();
            viewer.set_run_baseline(baseline, cx);
            viewer.toggle_pinned_run(second_run.clone(), cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            assert_eq!(viewer.active_navigation(cx).brush(), Some(timeline_before));
            for (panel, (snapshot, revision)) in viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .zip(&before)
            {
                assert!(Arc::ptr_eq(
                    panel.detail.as_ref().expect("detail should remain loaded"),
                    snapshot,
                ));
                assert_eq!(panel.detail_revision, *revision);
            }
        })
        .expect("viewer should remain open");
    assert!(cx.debug_bounds("overview-chart").is_some());
    assert!(cx.debug_bounds("ruler-major-tick-0").is_some());
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.archive_run(second_run.clone(), cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            for (panel, (snapshot, revision)) in viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .zip(&before)
            {
                assert!(Arc::ptr_eq(
                    panel.detail.as_ref().expect("detail should remain loaded"),
                    snapshot,
                ));
                assert_eq!(panel.detail_revision, *revision);
            }
        })
        .expect("viewer should remain open");
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.archive_run(second_run.clone(), cx);
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    window
        .read_with(&cx, |viewer, cx| {
            for (panel, (snapshot, revision)) in viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .zip(&before)
            {
                assert!(Arc::ptr_eq(
                    panel.detail.as_ref().expect("detail should remain loaded"),
                    snapshot,
                ));
                assert_eq!(panel.detail_revision, *revision);
            }
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn metric_sidebar_rows_align_with_independent_chart_tracks(cx: &mut TestAppContext) {
    let (root, project_id, first_run_id) = fixture_with_complete_runs(2, 2);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id.clone(), first_run_id, 2);
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
        })
        .expect("viewer should remain open");
    wait_for_viewer(window, &cx, |viewer, cx| {
        viewer.available_metric_keys(cx).len() == 2
    });
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
                .all(|panel| panel.detail.is_some())
    });

    let workspace = cx
        .debug_bounds("analysis-workspace")
        .expect("Analysis workspace should render");
    let track_scroll = cx
        .debug_bounds("metric-track-scroll")
        .expect("Metric track viewport should render");
    assert_eq!(track_scroll.origin.x, workspace.origin.x);
    assert_eq!(track_scroll.size.width, workspace.size.width);
    let overview = cx
        .debug_bounds("overview-chart")
        .expect("Global overview should render");
    let ruler = cx
        .debug_bounds("viewport-ruler")
        .expect("Viewport ruler should render");
    assert_eq!(ruler.origin.x, workspace.origin.x);
    assert_eq!(ruler.size.width, workspace.size.width);
    assert!(overview.origin.x > ruler.origin.x);
    assert_eq!(overview.right(), ruler.right());
    let major_tick = cx
        .debug_bounds("ruler-major-mark-0")
        .expect("Ruler should render major tick marks");
    let minor_tick = cx
        .debug_bounds("ruler-minor-tick-0")
        .expect("Ruler should render minor tick marks");
    assert_eq!(major_tick.size, size(px(1.), px(8.)));
    assert_eq!(minor_tick.size, size(px(1.), px(5.)));
    assert_eq!(major_tick.origin.x, overview.origin.x);

    let gutter_width = f64::from(overview.origin.x - ruler.origin.x);
    let plot_width = f64::from(overview.size.width);
    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.session.update(cx, |session, session_cx| {
                let brush = session
                    .views
                    .active_mut()
                    .navigation
                    .brush_mut()
                    .expect("brush should exist");
                let center = brush.home().start() + brush.home().span() / 2.;
                brush.zoom_at(center, 2.).expect("zoom should succeed");
                let selected = brush.selected();
                let target_start =
                    brush.home().start() + selected.span() * gutter_width / plot_width;
                brush
                    .pan_by(target_start - selected.start())
                    .expect("ruler viewport should pan");
                session.publish_snapshot();
                session_cx.notify();
            });
            viewer.sync_child_snapshots(cx);
            cx.notify();
        })
        .expect("viewer should remain open");
    cx.run_until_parked();
    let home_tick = cx
        .debug_bounds("ruler-major-mark-0")
        .expect("Home tick should remain visible over the Metric label gutter");
    assert!(
        f32::from(home_tick.origin.x - ruler.origin.x).abs() < 0.5,
        "home tick {home_tick:?} should align with ruler {ruler:?}",
    );

    for metric in ["metric-0", "metric-1"] {
        let sidebar = cx
            .debug_bounds(if metric == "metric-0" {
                "metric-sidebar-row:metric-0"
            } else {
                "metric-sidebar-row:metric-1"
            })
            .expect("Metric sidebar row should render");
        let track = cx
            .debug_bounds(if metric == "metric-0" {
                "metric-track:metric-0"
            } else {
                "metric-track:metric-1"
            })
            .expect("Metric track should render");
        let canvas = cx
            .debug_bounds(if metric == "metric-0" {
                "metric-canvas:metric-0"
            } else {
                "metric-canvas:metric-1"
            })
            .expect("Metric canvas should render");
        let metadata = cx
            .debug_bounds(if metric == "metric-0" {
                "metric-metadata:metric-0"
            } else {
                "metric-metadata:metric-1"
            })
            .expect("Metric metadata should render on the second label line");
        assert_eq!(sidebar.origin.y, track.origin.y);
        assert_eq!(sidebar.size.height, track.size.height);
        assert_eq!(track.origin.x, sidebar.origin.x + sidebar.size.width);
        assert_eq!(
            track.origin.x + track.size.width,
            workspace.origin.x + workspace.size.width
        );
        assert!(track.origin.x > ruler.origin.x);
        assert_eq!(track.right(), ruler.right());
        assert_eq!(canvas.origin.x, track.origin.x);
        assert_eq!(canvas.size.width, track.size.width);
        assert!(canvas.size.width > px(0.));
        assert_eq!(
            canvas.size.height,
            track.size.height - px(METRIC_TRACK_VERTICAL_PADDING * 2.)
        );
        assert!(metadata.size.height > px(0.));
        assert!(
            metadata.origin.y + metadata.size.height <= sidebar.origin.y + sidebar.size.height,
            "metadata {metadata:?} must remain inside sidebar {sidebar:?}",
        );
    }
    let (ranges, unavailable) = window
        .read_with(&cx, |viewer, cx| {
            let selected = viewer
                .active_navigation(cx)
                .brush()
                .map(|brush| brush.selected());
            let ranges = viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .map(|panel| {
                    chart::detail_viewport(
                        panel.detail.as_deref().expect("detail should be loaded"),
                        selected,
                        None,
                    )
                    .expect("detail should be drawable")
                    .y
                })
                .collect::<Vec<_>>();
            let unavailable = viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .all(|panel| {
                    panel.detail.as_ref().is_some_and(|snapshot| {
                        snapshot
                            .series
                            .iter()
                            .any(|series| series.completeness == EvidenceCompleteness::Unavailable)
                    })
                });
            (ranges, unavailable)
        })
        .expect("viewer should remain open");
    assert_ne!(ranges[0], ranges[1]);
    assert!(!unavailable);

    for selector in ["metric-resize:metric-0", "metric-resize:metric-1"] {
        let resize = cx
            .debug_bounds(selector)
            .expect("Every Metric row resize handle should render");
        let target = point(resize.center().x, resize.center().y + px(40.));
        cx.simulate_mouse_down(resize.center(), MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(target, Some(MouseButton::Left), Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    }
    let heights = window
        .read_with(&cx, |viewer, cx| {
            viewer
                .session_snapshot(cx)
                .views
                .active()
                .panels
                .iter()
                .map(|panel| panel.row_height)
                .collect::<Vec<_>>()
        })
        .expect("viewer should remain open");
    assert_eq!(heights, [92., 92.]);

    window
        .update(&mut cx, |viewer, _, cx| {
            viewer.remove_metric_panel(&MetricPanelId::from_string("metric-0"), cx);
        })
        .expect("viewer should remain open");
    assert_eq!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer
                    .session_snapshot(cx)
                    .views
                    .active()
                    .panels
                    .iter()
                    .map(|panel| panel.metric_key.as_str().to_owned())
                    .collect::<Vec<_>>()
            })
            .expect("viewer should remain open"),
        ["metric-1".to_owned()]
    );
}
