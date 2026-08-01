use gpui::{Modifiers, TestAppContext, px, size};

use crate::config::{ConfigScope, EditableConfig};
use crate::domain::SourceAlias;

use super::super::test_support::*;
use super::*;

#[gpui::test]
fn startup_without_configured_sources_opens_an_empty_workbench(cx: &mut TestAppContext) {
    let root = tempfile::tempdir().expect("test directory should be created");
    cx.executor().allow_parking();

    let (window, cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    cx.run_until_parked();

    window
        .read_with(&cx, |viewer, cx| {
            assert!(viewer.session_snapshot(cx).sources.is_empty());
            assert!(viewer.session.read(cx).transient_error.is_none());
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn startup_loads_sources_from_project_configuration(cx: &mut TestAppContext) {
    let (root, project_id, _) = fixture(1);
    let alias = SourceAlias::new("configured-source").expect("test alias should be valid");
    let mut config =
        EditableConfig::load(root.path().join(".seex/config.toml"), ConfigScope::Project)
            .expect("project configuration should load");
    config
        .set_source(&alias, root.path(), std::slice::from_ref(&project_id))
        .expect("test Source should be configured");
    config.save().expect("project configuration should save");
    cx.executor().allow_parking();

    let (window, cx) = open_viewer(cx, Some(root.path().to_path_buf()));
    wait_for_viewer(window, &cx, source_catalog_loaded);

    window
        .read_with(&cx, |viewer, cx| {
            let snapshot = viewer.session_snapshot(cx);
            let source = snapshot
                .sources
                .iter()
                .find(|source| source.source_id == DataSourceId::from_alias(&alias))
                .expect("configured Source should be loaded");
            assert_eq!(source.root_path, root.path());
            assert_eq!(source.project_allowlist, [project_id]);
        })
        .expect("viewer should remain open");
}

#[gpui::test]
fn application_shell_preserves_pinned_geometry_at_representative_sizes(cx: &mut TestAppContext) {
    let (root, _, _) = fixture_with_extent(100);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, root.path().to_path_buf());
    wait_for_viewer(window, &cx, source_catalog_loaded);

    for window_size in [(600., 520.), (800., 600.), (1_440., 900.)] {
        cx.simulate_resize(size(px(window_size.0), px(window_size.1)));
        cx.run_until_parked();

        let sidebar = cx
            .debug_bounds("project-sidebar")
            .expect("Project sidebar should render");
        let analysis = cx
            .debug_bounds("analysis-workspace")
            .expect("Analysis workspace should render");
        let tab_bar = cx
            .debug_bounds("analysis-tab-bar")
            .expect("Analysis tab bar should render");
        let sidebar_header = cx
            .debug_bounds("project-sidebar-header")
            .expect("Project sidebar header should render");
        let filter_row = cx
            .debug_bounds("project-filter-row")
            .expect("Project filter row should render");
        let filter = cx
            .debug_bounds("project-run-filter")
            .expect("Project filter should render");
        let baseline_group = cx
            .debug_bounds("baseline-group-label")
            .expect("Baseline group label should render");
        let brush_row = cx
            .debug_bounds("brush-row")
            .expect("Brush row should render");
        let brush_controls = cx
            .debug_bounds("brush-controls")
            .expect("Brush controls should render");
        let axis_picker = cx
            .debug_bounds("axis-picker")
            .expect("Axis picker should render");
        let add_metric = cx
            .debug_bounds("add-metric")
            .expect("Add Metric control should render");
        let tab = cx
            .debug_bounds("analysis-tab")
            .expect("active Analysis tab should render");
        let close = cx
            .debug_bounds("close-active-view")
            .expect("active View close control should render");
        let controls = cx
            .debug_bounds("analysis-right-controls")
            .expect("View toolbar controls should render");
        let new_view = cx
            .debug_bounds("new-view")
            .expect("View toolbar control should render");
        let inspector = cx
            .debug_bounds("toggle-bottom-inspector")
            .expect("bottom inspector control should render");
        let refresh = cx
            .debug_bounds("refresh-view")
            .expect("refresh control should render");

        assert_eq!(sidebar.origin.y, px(0.));
        assert_eq!(sidebar.size.height, px(window_size.1));
        assert_eq!(analysis.origin.x, sidebar.origin.x + sidebar.size.width);
        assert_eq!(tab_bar.origin.x, analysis.origin.x);
        assert_eq!(tab_bar.size.width, analysis.size.width);
        assert_eq!(tab_bar.size.height, px(32.));
        assert_eq!(sidebar_header.origin.y, tab_bar.origin.y);
        assert_eq!(sidebar_header.size.height, tab_bar.size.height);
        assert_eq!(filter_row.origin.y, brush_row.origin.y);
        assert_eq!(filter_row.size.height, brush_row.size.height);
        assert_eq!(filter_row.bottom(), brush_row.bottom());
        assert_eq!(baseline_group.origin.y - filter_row.bottom(), px(12.));
        assert_eq!(filter.origin.y, axis_picker.origin.y);
        assert_eq!(axis_picker.size.height, filter.size.height);
        assert_eq!(axis_picker.size.width, filter.size.height);
        assert_eq!(add_metric.size, axis_picker.size);
        assert_eq!(axis_picker.origin.x, brush_controls.origin.x + px(4.));
        assert_eq!(add_metric.right(), brush_controls.right() - px(5.));
        assert_eq!(tab.size.height, px(31.));
        assert_eq!(close.right(), tab.right() - px(5.));
        assert_eq!(new_view.origin.x, controls.origin.x + px(4.));
        assert_eq!(inspector.origin.x, new_view.right() + px(4.));
        assert_eq!(refresh.origin.x, inspector.right() + px(4.));
        assert_eq!(refresh.right(), controls.right() - px(4.));
        assert_eq!(new_view.size.height, px(20.));
    }

    let wide_project = cx
        .debug_bounds("project-tree-row-0-0")
        .expect("Project row should render");
    cx.simulate_mouse_move(wide_project.center(), None, Modifiers::default());
    cx.run_until_parked();
    let wide_information = cx
        .debug_bounds("project-information-project")
        .expect("Project information should render on hover");
    assert!(wide_information.origin.x >= wide_project.right());
    assert!(
        f32::from(wide_information.origin.y - wide_project.origin.y).abs() <= 12.,
        "information {wide_information:?} should align with Project {wide_project:?}",
    );
    for selector in ["project-information-name", "project-information-source"] {
        let content = cx
            .debug_bounds(selector)
            .expect("Project information content should render");
        assert!(content.left() >= wide_information.left());
        assert!(content.right() <= wide_information.right());
    }
    cx.simulate_resize(size(px(420.), px(520.)));
    cx.run_until_parked();
    let project_menu = cx
        .debug_bounds("project-menu-project")
        .expect("Project menu control should render while hovered");
    let information = cx
        .debug_bounds("project-information-project")
        .expect("Project information should render while hovered");
    assert!(information.right() <= px(412.));
    assert!(information.bottom() <= px(512.));
    cx.simulate_click(project_menu.center(), Modifiers::default());
    let popover = cx
        .debug_bounds("project-popover-project")
        .expect("Project menu should open");
    let narrow_project = cx
        .debug_bounds("project-tree-row-0-0")
        .expect("Project row should remain rendered");
    assert!(popover.origin.x >= project_menu.origin.x);
    assert!(
        f32::from(popover.origin.y - narrow_project.origin.y).abs() <= 20.,
        "popover {popover:?} should anchor beside Project {narrow_project:?}",
    );
    assert!(popover.right() <= px(412.));
    assert!(popover.bottom() <= px(512.));
    assert!(cx.debug_bounds("project-menu-separator").is_some());
    assert!(cx.debug_bounds("reveal-project-source").is_none());
    assert!(cx.debug_bounds("refresh-project-source").is_none());
    assert!(cx.debug_bounds("remove-project-source").is_none());
    let pin = cx
        .debug_bounds("pin-project")
        .expect("normal Project menu should offer Pin project");
    cx.simulate_click(pin.center(), Modifiers::default());
    assert!(
        window
            .read_with(&cx, |viewer, cx| !viewer
                .session_snapshot(cx)
                .views
                .pinned_projects()
                .is_empty())
            .expect("viewer should remain open")
    );
    let pinned_project = cx
        .debug_bounds("project-tree-row-0-0")
        .expect("Pinned Project row should render");
    cx.simulate_mouse_move(pinned_project.center(), None, Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("project-information-placement-icon")
            .is_some()
    );
    let analysis_tab = cx
        .debug_bounds("analysis-tab")
        .expect("Analysis tab should render");
    cx.simulate_mouse_move(analysis_tab.center(), None, Modifiers::default());
    cx.run_until_parked();
}

#[gpui::test]
fn worker_events_update_the_entity_without_render_polling(cx: &mut TestAppContext) {
    let (root, _, _) = fixture(1);
    cx.executor().allow_parking();
    let (window, cx) = open_viewer_with_configured_source(cx, root.path().to_path_buf());

    wait_for_viewer(window, &cx, source_catalog_loaded);
}

#[gpui::test]
fn popovers_close_after_clicking_outside_their_controls(cx: &mut TestAppContext) {
    let (root, project_id, run_id) = fixture(2);
    cx.executor().allow_parking();
    let (window, mut cx) = open_viewer_with_configured_source(cx, root.path().to_path_buf());
    wait_for_viewer(window, &cx, source_catalog_loaded);
    select_fixture_run(window, &mut cx, project_id, run_id, 2);

    let outside = cx
        .debug_bounds("new-view")
        .expect("New View should provide an outside click target")
        .center();
    let add_metric = cx
        .debug_bounds("add-metric")
        .expect("Add Metric control should render");
    cx.simulate_mouse_move(add_metric.center(), None, Modifiers::default());
    cx.simulate_click(add_metric.center(), Modifiers::default());
    assert!(
        window
            .read_with(&cx, |viewer, cx| {
                viewer.workspace.read(cx).metric_picker_open
            })
            .expect("viewer should remain open")
    );
    cx.simulate_click(add_metric.center(), Modifiers::default());
    assert!(
        !window
            .read_with(&cx, |viewer, cx| {
                viewer.workspace.read(cx).metric_picker_open
            })
            .expect("viewer should remain open")
    );
    cx.simulate_click(add_metric.center(), Modifiers::default());
    let metric_picker = cx
        .debug_bounds("metric-picker")
        .expect("Metric picker should open");
    assert!(
        !metric_picker.contains(&outside),
        "outside target {outside:?} should not overlap picker {metric_picker:?}",
    );
    cx.simulate_mouse_move(outside, None, Modifiers::default());
    cx.simulate_mouse_down(outside, MouseButton::Left, Modifiers::default());
    assert!(
        !window
            .read_with(&cx, |viewer, cx| {
                viewer.workspace.read(cx).metric_picker_open
            })
            .expect("viewer should remain open"),
        "outside mouse-down should clear the Metric picker state",
    );
    cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    assert!(
        !window
            .read_with(&cx, |viewer, cx| {
                viewer.workspace.read(cx).metric_picker_open
            })
            .expect("viewer should remain open")
    );

    let axis_picker = cx
        .debug_bounds("axis-picker")
        .expect("Axis picker control should render");
    cx.simulate_click(axis_picker.center(), Modifiers::default());
    assert!(cx.debug_bounds("axis-menu").is_some());
    cx.simulate_mouse_move(outside, None, Modifiers::default());
    cx.simulate_click(outside, Modifiers::default());
    assert!(
        !window
            .read_with(&cx, |viewer, cx| viewer.workspace.read(cx).axis_picker_open)
            .expect("viewer should remain open")
    );

    let active_tab = cx
        .debug_bounds("analysis-tab")
        .expect("active Analysis View should render");
    cx.simulate_mouse_down(
        active_tab.center(),
        MouseButton::Right,
        Modifiers::default(),
    );
    cx.simulate_mouse_up(
        active_tab.center(),
        MouseButton::Right,
        Modifiers::default(),
    );
    assert!(cx.debug_bounds("view-menu").is_some());
    cx.simulate_mouse_move(outside, None, Modifiers::default());
    cx.simulate_click(outside, Modifiers::default());
    assert!(
        window
            .read_with(&cx, |viewer, cx| !viewer.view_bar_menu_is_open(cx))
            .expect("viewer should remain open")
    );
}
