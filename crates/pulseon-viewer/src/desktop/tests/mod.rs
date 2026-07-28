#[cfg(feature = "test-support")]
use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(feature = "test-support")]
use std::sync::Arc;
#[cfg(feature = "test-support")]
use std::time::Duration;

use crate::data::query::CurveAxis;
#[cfg(feature = "test-support")]
use crate::data::worker::ReadKind;
use crate::domain::{DataSourceId, RunRef};
#[cfg(feature = "test-support")]
use crate::workbench::ProjectRef;
#[cfg(feature = "test-support")]
use crate::workbench::document::{SavedProjectRef, WorkbenchDocument};
#[cfg(feature = "test-support")]
use crate::workbench::panel_reads::MetricPanelId;
#[cfg(feature = "test-support")]
use gpui::{App, Context, ScrollWheelEvent};
use gpui::{point, px};
use pulseon_chart_core::BrushState;
#[cfg(feature = "test-support")]
use pulseon_model::alignment::AlignmentAxis;
use pulseon_model::comparison::EvidenceCompleteness;
#[cfg(feature = "test-support")]
use pulseon_model::metric::MetricKey;
use pulseon_model::run::{RunId, RunStatus};
use pulseon_model::types::ProjectId;

use super::chart::HoverPoint;
#[cfg(feature = "test-support")]
use super::command::WorkbenchCommand;
use super::*;

#[cfg(feature = "test-support")]
trait ViewerTestActions {
    fn select_metric(&mut self, metric: MetricKey, cx: &mut Context<ViewerApp>);
    fn activate_tree_project(
        &mut self,
        source_id: DataSourceId,
        project_id: ProjectId,
        cx: &mut Context<ViewerApp>,
    );
    fn toggle_tree_run(&mut self, run: RunRef, cx: &mut Context<ViewerApp>);
    fn toggle_run(&mut self, run: RunRef, cx: &mut Context<ViewerApp>);
    fn set_run_baseline(&mut self, run: RunRef, cx: &mut Context<ViewerApp>);
    fn toggle_pinned_run(&mut self, run: RunRef, cx: &mut Context<ViewerApp>);
    fn archive_run(&mut self, run: RunRef, cx: &mut Context<ViewerApp>);
    fn toggle_project_runs(&mut self, project: &SidebarProject, cx: &mut Context<ViewerApp>);
    fn finish_moved_track_drag(&mut self, cx: &mut Context<ViewerApp>);
    fn finish_drag(&mut self, cx: &mut Context<ViewerApp>);
    fn remove_metric_panel(&mut self, panel_id: &MetricPanelId, cx: &mut Context<ViewerApp>);
}

#[cfg(feature = "test-support")]
impl ViewerTestActions for ViewerApp {
    fn select_metric(&mut self, metric: MetricKey, cx: &mut Context<Self>) {
        self.dispatch_workbench_command(WorkbenchCommand::SelectMetric(metric), cx);
    }

    fn activate_tree_project(
        &mut self,
        source_id: DataSourceId,
        project_id: ProjectId,
        cx: &mut Context<Self>,
    ) {
        let key = (source_id, project_id);
        self.project_sidebar.update(cx, |sidebar, cx| {
            if !sidebar.expanded_projects.insert(key.clone()) {
                sidebar.expanded_projects.remove(&key);
            }
            cx.notify();
        });
    }

    fn toggle_tree_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        self.toggle_run(run, cx);
    }

    fn toggle_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        self.dispatch_workbench_command(WorkbenchCommand::ToggleRun(run), cx);
    }

    fn set_run_baseline(&mut self, run: RunRef, cx: &mut Context<Self>) {
        let baseline = (self.session_snapshot(cx).views.active().baseline.as_ref() != Some(&run))
            .then_some(run);
        self.dispatch_workbench_command(WorkbenchCommand::SetBaseline(baseline), cx);
    }

    fn toggle_pinned_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        self.dispatch_workbench_command(WorkbenchCommand::TogglePinnedRun(run), cx);
    }

    fn archive_run(&mut self, run: RunRef, cx: &mut Context<Self>) {
        let archived = !self
            .session_snapshot(cx)
            .views
            .archived_runs()
            .contains(&run);
        self.dispatch_workbench_command(WorkbenchCommand::SetRunArchived { run, archived }, cx);
    }

    fn toggle_project_runs(&mut self, project: &SidebarProject, cx: &mut Context<Self>) {
        let session = self.session_snapshot(cx);
        let baseline = session.views.active().baseline.as_ref();
        let pinned = &session.views.active().pinned_runs;
        let archived = session.views.archived_runs();
        let runs = project
            .runs
            .iter()
            .map(|run| {
                RunRef::new(
                    project.project_ref.source_id.clone(),
                    run.project_id.clone(),
                    run.run_id.clone(),
                )
            })
            .filter(|run| baseline != Some(run) && !pinned.contains(run) && !archived.contains(run))
            .collect::<Vec<_>>();
        let selected = !runs
            .iter()
            .all(|run| session.views.active().runs.contains(run));
        self.dispatch_workbench_command(WorkbenchCommand::SetProjectRuns { runs, selected }, cx);
    }

    fn finish_moved_track_drag(&mut self, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |workspace, cx| workspace.finish_moved_track_drag(cx));
    }

    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |workspace, cx| workspace.finish_drag(cx));
    }

    fn remove_metric_panel(&mut self, panel_id: &MetricPanelId, cx: &mut Context<Self>) {
        self.dispatch_workbench_command(WorkbenchCommand::RemoveMetric(panel_id.clone()), cx);
    }
}

#[test]
fn picker_cancellation_has_no_source_effect() {
    assert_eq!(picked_directory(None), None);
    assert_eq!(picked_directory(Some(Vec::new())), None);
}

#[test]
fn picker_uses_the_single_selected_directory() {
    assert_eq!(
        picked_directory(Some(vec![PathBuf::from("project")])),
        Some(PathBuf::from("project"))
    );
}

#[test]
fn duplicate_project_names_are_qualified_by_source_identity() {
    assert_eq!(
        project_tree_label("viewer", &DataSourceId::from_string("source-b"), true),
        "viewer — source-b"
    );
    assert_eq!(
        project_tree_label("viewer", &DataSourceId::from_string("source-b"), false),
        "viewer"
    );
}

#[test]
fn hover_values_format_visible_and_contextual_evidence() {
    let hover = HoverPoint {
        run_ref: RunRef::new(
            DataSourceId::from_string("source"),
            ProjectId::from_string("project"),
            RunId::from_string("run"),
        ),
        axis_value: 2_904,
        value: 0.506,
        canvas_position: point(px(10.), px(20.)),
        align_left: false,
    };

    assert_eq!(
        hover_value_label(CurveAxis::Step, &hover, Some(0.55), "training"),
        "2904: 0.51 (+0.55) training"
    );
    assert_eq!(
        hover_value_label(CurveAxis::Step, &hover, Some(-0.55), "training"),
        "2904: 0.51 (−0.55) training"
    );
    assert_eq!(
        hover_value_label(CurveAxis::Step, &hover, None, "training"),
        "2904: 0.51 training"
    );
    assert_eq!(format_ruler_coordinate(CurveAxis::Step, 26_432.4), "26432");
    assert_eq!(track_tooltip_width(["0.28"]), 104.);
    assert_eq!(track_tooltip_width(["19433: 0.44 run-8"]), 138.5);
    let long_label = "x".repeat(40);
    assert_eq!(track_tooltip_width([long_label.as_str()]), 248.);
}

#[test]
fn brush_handle_drag_stops_before_crossing_the_opposite_handle() {
    let home =
        pulseon_chart_core::AxisRange::new(0., 100.).expect("test brush range should be valid");
    let mut brush = BrushState::new(home).expect("test brush should initialize");
    brush.resize_start(20.).expect("start should resize");
    brush.resize_end(80.).expect("end should resize");

    assert_eq!(
        update_brush_drag(brush, &mut DragGesture::BrushStart, 90.),
        None
    );
    assert_eq!(
        update_brush_drag(brush, &mut DragGesture::BrushEnd, 10.),
        None
    );
}

#[test]
fn inspector_sort_preserves_roles_and_cycles_numeric_order() {
    assert!(InspectorColumn::Run.owns_resize_boundary());
    assert!(InspectorColumn::LastValue.owns_resize_boundary());
    assert!(!InspectorColumn::Project.owns_resize_boundary());

    let make_row =
        |run: &str, value: Option<f64>, baseline: bool, pinned: bool, order| InspectorRow {
            run_ref: RunRef::new(
                DataSourceId::from_string("source"),
                ProjectId::from_string("project"),
                RunId::from_string(run),
            ),
            run_label: run.to_owned(),
            project_label: "project · source".to_owned(),
            status: RunStatus::Finished,
            evidence: EvidenceCompleteness::Complete,
            evidence_label: "Complete".to_owned(),
            count: Some(1),
            last_step: Some(1),
            last_value: value,
            minimum: value,
            maximum: value,
            locked: value.map(|value| (1, value)),
            hover: value.map(|value| (1, value)),
            baseline,
            pinned,
            original_order: order,
        };
    let mut long_project = make_row("path", Some(1.), false, false, 0);
    long_project.project_label =
        "viewer · /tmp/a/very/long/project/path/that/needs/content/sizing".to_owned();
    assert!(inspector_project_width(&[long_project]) > InspectorColumn::Project.default_width());
    let mut rows = vec![
        make_row("run-3", Some(3.), false, false, 0),
        make_row("baseline", Some(2.), true, false, 1),
        make_row("pinned", Some(4.), false, true, 2),
        make_row("run-1", Some(1.), false, false, 3),
        make_row("missing", None, false, false, 4),
    ];

    sort_inspector_rows(
        &mut rows,
        Some(InspectorSort {
            column: InspectorColumn::Minimum,
            direction: InspectorSortDirection::Ascending,
        }),
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.run_label.as_str())
            .collect::<Vec<_>>(),
        ["baseline", "pinned", "run-1", "run-3", "missing"]
    );
    sort_inspector_rows(
        &mut rows,
        Some(InspectorSort {
            column: InspectorColumn::Minimum,
            direction: InspectorSortDirection::Descending,
        }),
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.run_label.as_str())
            .collect::<Vec<_>>(),
        ["baseline", "pinned", "run-3", "run-1", "missing"]
    );
    assert_eq!(inspector_float(Some(2.), Some(2.), true), "2.000000");
    assert_eq!(
        inspector_float(Some(3.), Some(2.), false),
        "3.000000 (+1.000000)"
    );
    assert_eq!(inspector_float(None, Some(2.), false), "");
    assert_eq!(
        inspector_integer(Some(20_512), Some(20_741), false),
        "20,512 (−229)"
    );
    assert_eq!(
        inspector_cursor_cell(Some((20_799, 0.62)), Some((20_800, 0.58)), false,),
        "0.620000 (+0.040000)"
    );
    assert_eq!(inspector_cursor_cell(None, Some((20_800, 0.58)), false), "");
    let baseline = rows.iter().find(|row| row.baseline).expect("baseline row");
    let candidate = rows
        .iter()
        .find(|row| !row.baseline)
        .expect("candidate row");
    assert_eq!(
        inspector_cell_text(InspectorColumn::Count, candidate, Some(baseline)),
        "1"
    );
    assert_eq!(
        inspector_cell_text(InspectorColumn::LastStep, candidate, Some(baseline)),
        "1"
    );
}

#[cfg(feature = "test-support")]
mod inspector;
#[cfg(feature = "test-support")]
mod performance;
#[cfg(feature = "test-support")]
mod persistence;
#[cfg(feature = "test-support")]
mod shell;
#[cfg(feature = "test-support")]
mod sidebar;
#[cfg(feature = "test-support")]
mod views;
#[cfg(feature = "test-support")]
mod workspace;
