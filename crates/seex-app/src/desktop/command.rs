use std::collections::{BTreeMap, HashSet};

use crate::desktop::{
    ClearLockedCursor, Refresh, ResetView, ShowMetricInspector, ToggleBottomInspector,
    ToggleMetricSidebar, ToggleProjectSidebar, UseElapsed, UseStep, ZoomIn, ZoomOut,
};
use gpui::{Context, Window};
use seex::MetricKey;
use seex::{AlignmentAxis, AlignmentViewport};

use crate::domain::RunRef;
use crate::workbench::ProjectRef;
use crate::workbench::panel_reads::{AnalysisViewId, MetricPanelId};

use super::ViewerApp;
use super::project_sidebar::ProjectPlacement;
use super::session::WorkbenchSession;

#[derive(Clone, Debug)]
pub(crate) enum WorkbenchCommand {
    CreateView,
    DuplicateActiveView,
    ActivateView(AnalysisViewId),
    CloseView(AnalysisViewId),
    RenameView {
        view_id: AnalysisViewId,
        name: String,
    },
    ToggleRun(RunRef),
    SetBaseline(Option<RunRef>),
    TogglePinnedRun(RunRef),
    SetRunArchived {
        run: RunRef,
        archived: bool,
    },
    SetProjectPlacement {
        project: ProjectRef,
        placement: ProjectPlacement,
    },
    ToggleProjectExpanded(ProjectRef),
    SetProjectRuns {
        runs: Vec<RunRef>,
        selected: bool,
    },
    SelectMetric(MetricKey),
    SelectPanel(MetricPanelId),
    RemoveMetric(MetricPanelId),
    ResizeMetric {
        panel_id: MetricPanelId,
        height: f32,
    },
    SelectAxis(AlignmentAxis),
    ResetViewport,
    ZoomViewport {
        anchor: f64,
        factor: f64,
    },
    PanViewport(f64),
    ResizeBrushStart(f64),
    ResizeBrushEnd(f64),
    SetTimelineHome(AlignmentViewport),
    ClearTimeline,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CommandEffect {
    pub changed: bool,
    pub active_view_changed: bool,
    pub selected: Option<bool>,
    pub panel_id: Option<MetricPanelId>,
}

impl WorkbenchSession {
    pub(crate) fn apply_command(&mut self, command: WorkbenchCommand) -> CommandEffect {
        let selection = self.snapshot();
        let effect = match command {
            WorkbenchCommand::CreateView => {
                self.deactivate_active_view();
                self.views.create_empty();
                CommandEffect {
                    changed: true,
                    active_view_changed: true,
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::DuplicateActiveView => {
                self.deactivate_active_view();
                self.views.duplicate_active();
                CommandEffect {
                    changed: true,
                    active_view_changed: true,
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::ActivateView(view_id) => {
                if self.views.active().view_id == view_id {
                    CommandEffect::default()
                } else {
                    self.deactivate_active_view();
                    let changed = self.views.activate(&view_id);
                    CommandEffect {
                        changed,
                        active_view_changed: changed,
                        ..CommandEffect::default()
                    }
                }
            }
            WorkbenchCommand::CloseView(view_id) => {
                let active_view_changed = self.views.active().view_id == view_id;
                if active_view_changed {
                    self.deactivate_active_view();
                }
                let changed = self.views.close(&view_id);
                CommandEffect {
                    changed,
                    active_view_changed: changed && active_view_changed,
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::RenameView { view_id, name } => CommandEffect {
                changed: self.views.rename(&view_id, &name),
                active_view_changed: false,
                ..CommandEffect::default()
            },
            WorkbenchCommand::ToggleRun(run) => {
                let has_capacity = self.views.active().runs.contains(&run)
                    || selection.has_capacity_for_runs(std::slice::from_ref(&run));
                match self.views.toggle_active_run(run, has_capacity) {
                    Ok(selected) => {
                        self.transient_error = None;
                        CommandEffect {
                            changed: true,
                            selected: Some(selected),
                            ..CommandEffect::default()
                        }
                    }
                    Err(error) => {
                        self.transient_error = Some(error.to_string());
                        CommandEffect::default()
                    }
                }
            }
            WorkbenchCommand::SetBaseline(baseline) => {
                let changed = self.views.active().baseline != baseline;
                let has_capacity = baseline
                    .as_ref()
                    .is_none_or(|run| selection.has_capacity_for_runs(std::slice::from_ref(run)));
                if !has_capacity {
                    self.transient_error =
                        Some(crate::domain::SelectionError::RunLimit.to_string());
                    CommandEffect::default()
                } else {
                    if let Some(run) = &baseline {
                        self.views.restore_run(run);
                    }
                    match self.views.set_active_baseline(baseline, true) {
                        Ok(()) => {
                            self.transient_error = None;
                            CommandEffect {
                                changed,
                                ..CommandEffect::default()
                            }
                        }
                        Err(error) => {
                            self.transient_error = Some(error.to_string());
                            CommandEffect::default()
                        }
                    }
                }
            }
            WorkbenchCommand::TogglePinnedRun(run) => {
                let adding = !self.views.active().pinned_runs.contains(&run);
                let has_capacity =
                    !adding || selection.has_capacity_for_runs(std::slice::from_ref(&run));
                if !has_capacity {
                    self.transient_error =
                        Some(crate::domain::SelectionError::RunLimit.to_string());
                    CommandEffect::default()
                } else {
                    if adding {
                        self.views.restore_run(&run);
                    }
                    match self.views.toggle_active_pinned_run(run, true) {
                        Ok(selected) => CommandEffect {
                            changed: true,
                            selected: Some(selected),
                            ..CommandEffect::default()
                        },
                        Err(error) => {
                            self.transient_error = Some(error.to_string());
                            CommandEffect::default()
                        }
                    }
                }
            }
            WorkbenchCommand::SetRunArchived { run, archived } => {
                let was_archived = self.views.archived_runs().contains(&run);
                if archived {
                    self.views.archive_run(run);
                } else if selection.has_capacity_for_runs(std::slice::from_ref(&run)) {
                    self.views.restore_run(&run);
                } else {
                    self.transient_error =
                        Some(crate::domain::SelectionError::RunLimit.to_string());
                    return CommandEffect::default();
                }
                CommandEffect {
                    changed: was_archived != archived,
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::SetProjectPlacement { project, placement } => {
                match placement {
                    ProjectPlacement::Pinned => self.views.pin_project(project),
                    ProjectPlacement::Projects => {
                        self.views.unpin_project(&project);
                        self.views.restore_project(&project);
                    }
                    ProjectPlacement::Archived => self.views.archive_project(project),
                }
                CommandEffect {
                    changed: true,
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::ToggleProjectExpanded(project) => {
                self.views.toggle_project_expanded(project);
                CommandEffect {
                    changed: true,
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::SetProjectRuns { runs, selected } => {
                if selected && !selection.has_capacity_for_runs(&runs) {
                    self.transient_error =
                        Some(crate::domain::SelectionError::RunLimit.to_string());
                    return CommandEffect::default();
                }
                let mut changed = false;
                for run in runs {
                    if self.views.active().runs.contains(&run) == selected {
                        continue;
                    }
                    match self.views.toggle_active_run(run, true) {
                        Ok(_) => changed = true,
                        Err(error) => {
                            self.transient_error = Some(error.to_string());
                            break;
                        }
                    }
                }
                CommandEffect {
                    changed,
                    selected: changed.then_some(selected),
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::SelectMetric(metric_key) => {
                let panel_id = self.views.select_active_metric(metric_key);
                CommandEffect {
                    changed: true,
                    panel_id: Some(panel_id),
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::SelectPanel(panel_id) => CommandEffect {
                changed: self.views.select_active_panel(&panel_id),
                panel_id: Some(panel_id),
                ..CommandEffect::default()
            },
            WorkbenchCommand::RemoveMetric(panel_id) => CommandEffect {
                changed: self.views.remove_active_panel(&panel_id),
                panel_id: Some(panel_id),
                ..CommandEffect::default()
            },
            WorkbenchCommand::ResizeMetric { panel_id, height } => CommandEffect {
                changed: self.views.set_active_panel_height(&panel_id, height),
                panel_id: Some(panel_id),
                ..CommandEffect::default()
            },
            WorkbenchCommand::SelectAxis(axis) => {
                let changed = self.views.active().navigation.axis() != axis;
                if changed {
                    self.views.clear_active_timeline_extents();
                    self.views.active_mut().navigation.select_axis(axis);
                }
                CommandEffect {
                    changed,
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::ResetViewport => CommandEffect {
                changed: self.views.active_mut().navigation.reset_view(),
                ..CommandEffect::default()
            },
            WorkbenchCommand::ZoomViewport { anchor, factor } => CommandEffect {
                changed: self.views.active_mut().navigation.zoom_at(anchor, factor),
                ..CommandEffect::default()
            },
            WorkbenchCommand::PanViewport(delta) => CommandEffect {
                changed: self.views.active_mut().navigation.pan_by(delta),
                ..CommandEffect::default()
            },
            WorkbenchCommand::ResizeBrushStart(position) => CommandEffect {
                changed: self.views.active_mut().navigation.resize_start(position),
                ..CommandEffect::default()
            },
            WorkbenchCommand::ResizeBrushEnd(position) => CommandEffect {
                changed: self.views.active_mut().navigation.resize_end(position),
                ..CommandEffect::default()
            },
            WorkbenchCommand::SetTimelineHome(viewport) => {
                self.views
                    .active_mut()
                    .navigation
                    .set_timeline_home(viewport);
                CommandEffect {
                    changed: true,
                    ..CommandEffect::default()
                }
            }
            WorkbenchCommand::ClearTimeline => {
                let changed = self.views.active().navigation.brush().is_some();
                self.views.active_mut().navigation.clear_timeline();
                CommandEffect {
                    changed,
                    ..CommandEffect::default()
                }
            }
        };
        if effect.changed {
            self.persistence_dirty = true;
            self.publish_semantic_snapshot();
        }
        effect
    }

    fn deactivate_active_view(&mut self) {
        let view_id = self.views.active().view_id.clone();
        self.panel_reads.deactivate_view(&view_id);
        self.views.cancel_active_panel_reads();
    }
}

impl ViewerApp {
    pub(crate) fn dispatch_workbench_command(
        &mut self,
        command: WorkbenchCommand,
        cx: &mut Context<Self>,
    ) {
        self.pending_commands.push(command);
        if self.command_dispatch_pending {
            return;
        }
        self.command_dispatch_pending = true;
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = this.update(cx, |this, cx| {
                this.command_dispatch_pending = false;
                let commands = std::mem::take(&mut this.pending_commands);
                for command in commands {
                    let effect = this.session.update(cx, |session, session_cx| {
                        let effect = session.apply_command(command.clone());
                        if effect.changed {
                            session_cx.notify();
                        }
                        effect
                    });
                    if effect.changed {
                        this.sync_child_snapshots(cx);
                    }
                    this.handle_command_effect(&command, effect, cx);
                }
                cx.notify();
            });
        });
    }

    fn handle_command_effect(
        &mut self,
        command: &WorkbenchCommand,
        effect: CommandEffect,
        cx: &mut Context<Self>,
    ) {
        if !effect.changed {
            return;
        }
        if effect.active_view_changed {
            self.sync_active_view_workspace(cx);
            self.refresh_catalog(cx);
        }
        match command {
            WorkbenchCommand::ToggleRun(_) => {
                if effect.selected == Some(true) {
                    self.refresh_catalog(cx);
                    self.request_missing_panel_curves(cx);
                }
                if self.inspector_visible(cx) {
                    self.request_inspector(cx);
                }
            }
            WorkbenchCommand::SetBaseline(_)
            | WorkbenchCommand::TogglePinnedRun(_)
            | WorkbenchCommand::SetRunArchived { .. } => {
                self.request_missing_panel_curves(cx);
                if self.inspector_visible(cx) {
                    self.request_inspector(cx);
                }
            }
            WorkbenchCommand::SetProjectRuns { selected, .. } => {
                if *selected {
                    self.refresh_catalog(cx);
                    self.request_missing_panel_curves(cx);
                }
                if self.inspector_visible(cx) {
                    self.request_inspector(cx);
                }
            }
            WorkbenchCommand::SelectMetric(_) => {
                let Some(panel_id) = effect.panel_id else {
                    return;
                };
                self.workspace.update(cx, |workspace, cx| {
                    workspace.reset_metric_track_schedule(cx);
                });
                self.request_panel_overview(&panel_id, cx);
                if self.inspector_visible(cx) {
                    self.request_inspector(cx);
                }
            }
            WorkbenchCommand::SelectPanel(_) => {
                self.bottom_inspector.update(cx, |inspector, cx| {
                    inspector.visible = true;
                    cx.notify();
                });
                self.request_inspector(cx);
            }
            WorkbenchCommand::RemoveMetric(panel_id) => {
                self.workspace.update(cx, |workspace, cx| {
                    workspace.reset_metric_track_schedule(cx);
                    workspace.track_charts.remove(panel_id);
                    workspace.track_hovers.remove(panel_id);
                    cx.notify();
                });
                let session = self.session_snapshot(cx);
                if let Some(home) = session
                    .views
                    .active()
                    .timeline_extents
                    .values()
                    .copied()
                    .reduce(|left, right| {
                        AlignmentViewport::new(
                            left.start().min(right.start()),
                            left.end().max(right.end()),
                        )
                        .expect("valid panel extents must have a valid union")
                    })
                {
                    self.dispatch_workbench_command(WorkbenchCommand::SetTimelineHome(home), cx);
                } else {
                    self.dispatch_workbench_command(WorkbenchCommand::ClearTimeline, cx);
                }
            }
            WorkbenchCommand::ResizeMetric { panel_id, .. } => {
                let session = self.session_snapshot(cx);
                if let Some(index) = session
                    .views
                    .active()
                    .panels
                    .iter()
                    .position(|panel| &panel.panel_id == panel_id)
                {
                    self.workspace
                        .read(cx)
                        .metric_scroll
                        .splice(index..index + 1, 1);
                }
            }
            WorkbenchCommand::SelectAxis(_) => self.request_overview(cx),
            WorkbenchCommand::ResetViewport => {
                self.request_detail(cx);
            }
            WorkbenchCommand::CreateView
            | WorkbenchCommand::DuplicateActiveView
            | WorkbenchCommand::ActivateView(_)
            | WorkbenchCommand::CloseView(_)
            | WorkbenchCommand::RenameView { .. }
            | WorkbenchCommand::SetProjectPlacement { .. }
            | WorkbenchCommand::ToggleProjectExpanded(_)
            | WorkbenchCommand::ZoomViewport { .. }
            | WorkbenchCommand::PanViewport(_)
            | WorkbenchCommand::ResizeBrushStart(_)
            | WorkbenchCommand::ResizeBrushEnd(_)
            | WorkbenchCommand::SetTimelineHome(_)
            | WorkbenchCommand::ClearTimeline => {}
        }
    }

    pub(super) fn on_refresh(&mut self, _: &Refresh, _: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |session, session_cx| {
            session.transient_error = None;
            session.publish_snapshot();
            session_cx.notify();
        });
        self.refresh_all_sources(cx);
        self.request_overview(cx);
        if self.inspector_visible(cx) {
            self.request_inspector(cx);
        }
        cx.notify();
    }
    pub(super) fn on_toggle_project_sidebar(
        &mut self,
        _: &ToggleProjectSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.project_sidebar.update(cx, |sidebar, cx| {
            sidebar.visible = !sidebar.visible;
            sidebar.menu = None;
            sidebar.hovered_project = None;
            cx.notify();
        });
        self.interaction.update(cx, |interaction, cx| {
            if interaction.set_emphasized_run(None) {
                cx.notify();
            }
        });
        self.focus.focus(window);
        cx.notify();
    }
    pub(super) fn on_toggle_metric_sidebar(
        &mut self,
        _: &ToggleMetricSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace.update(cx, |workspace, cx| {
            workspace.metric_sidebar_compact = !workspace.metric_sidebar_compact;
            cx.notify();
        });
        self.focus.focus(window);
        cx.notify();
    }
    pub(super) fn on_toggle_bottom_inspector(
        &mut self,
        _: &ToggleBottomInspector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.inspector_visible(cx) {
            self.bottom_inspector.update(cx, |inspector, cx| {
                inspector.visible = false;
                cx.notify();
            });
        } else if self
            .session_snapshot(cx)
            .views
            .active()
            .selected_panel_id
            .is_some()
        {
            self.bottom_inspector.update(cx, |inspector, cx| {
                inspector.visible = true;
                cx.notify();
            });
            self.request_inspector(cx);
        }
        self.focus.focus(window);
        cx.notify();
    }
    pub(super) fn on_show_metric_inspector(
        &mut self,
        _: &ShowMetricInspector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel_id) = self
            .session_snapshot(cx)
            .views
            .active()
            .selected_panel_id
            .clone()
        {
            self.show_metric_inspector(&panel_id, cx);
        }
        self.focus.focus(window);
    }
    pub(super) fn on_reset(&mut self, _: &ResetView, _: &mut Window, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, cx| {
            workspace.cancel_detail_refresh(cx);
        });
        self.dispatch_workbench_command(WorkbenchCommand::ResetViewport, cx);
        cx.notify();
    }
    pub(super) fn on_zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_from_keyboard(1.25, cx);
    }
    pub(super) fn on_zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom_from_keyboard(0.8, cx);
    }
    pub(super) fn on_clear_locked_cursor(
        &mut self,
        _: &ClearLockedCursor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.interaction.update(cx, |interaction, cx| {
            let changed = interaction.set_locked_cursor(None);
            if changed {
                cx.notify();
            }
            changed
        }) {
            cx.notify();
        }
    }
    pub(super) fn zoom_from_keyboard(&mut self, factor: f64, cx: &mut Context<Self>) {
        let Some(selected) = self
            .session_snapshot(cx)
            .views
            .active()
            .navigation
            .brush()
            .map(|brush| brush.selected())
        else {
            return;
        };
        let anchor = selected.start() + selected.span() / 2.;
        let mut preview = self.session_snapshot(cx).views.active().navigation.clone();
        if !preview.zoom_at(anchor, factor) {
            return;
        }
        self.dispatch_workbench_command(WorkbenchCommand::ZoomViewport { anchor, factor }, cx);
        self.workspace.update(cx, |workspace, cx| {
            workspace.defer_metric_repaint(cx);
            workspace.schedule_detail_refresh(cx);
        });
    }
    pub(super) fn on_step(&mut self, _: &UseStep, _: &mut Window, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, cx| {
            workspace.axis_picker_open = false;
            cx.notify();
        });
        self.dispatch_workbench_command(WorkbenchCommand::SelectAxis(AlignmentAxis::Step), cx);
        cx.notify();
    }
    pub(super) fn on_elapsed(&mut self, _: &UseElapsed, _: &mut Window, cx: &mut Context<Self>) {
        self.workspace.update(cx, |workspace, cx| {
            workspace.axis_picker_open = false;
            cx.notify();
        });
        self.dispatch_workbench_command(
            WorkbenchCommand::SelectAxis(AlignmentAxis::ElapsedTime),
            cx,
        );
        cx.notify();
    }
    pub(super) fn available_metric_keys(&self, cx: &gpui::App) -> Vec<MetricKey> {
        let source_ids = self
            .active_visible_runs(cx)
            .into_iter()
            .map(|run| run.source_id)
            .collect::<HashSet<_>>();
        self.session_snapshot(cx)
            .sources
            .iter()
            .filter(|source| source_ids.contains(&source.source_id))
            .flat_map(|source| source.catalog.metric_keys.iter().cloned())
            .map(|metric| (metric.as_str().to_owned(), metric))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect()
    }
}
