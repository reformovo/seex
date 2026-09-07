use std::collections::HashSet;

use gpui::{Context, px};
use seex::AlignmentViewport;

use crate::config::ConfiguredSource;
use crate::data::DiscoveryRequest;
use crate::data::query::CurveAxis;
use crate::data::registry::SourceStatus;
use crate::data::worker::{ReadKind, ReadRequest};
use crate::domain::{DataSourceId, RunRef};
use crate::workbench::panel_reads::{
    MetricPanelId, PanelReadMode, PanelReadRequest, PanelReadTag, PlannedReadCancellation,
    PlannedSourceRead,
};
use crate::workbench::toml_document::TomlWorkbenchDocument;

use super::super::ViewerApp;
use super::super::workspace::ScheduledPanelRead;
use super::persistence::RestoredWorkbench;
use super::{WorkbenchSession, WorkbenchSessionEvent};

struct PanelDetailQuery {
    panel_id: MetricPanelId,
    viewport: AlignmentViewport,
    logical_width: u32,
    runs: Vec<RunRef>,
    axis: CurveAxis,
    mode: PanelReadMode,
}

impl WorkbenchSession {
    pub(crate) fn retain_active_panel_curves(
        &mut self,
        details: &[MetricPanelId],
        overviews: &[MetricPanelId],
        cx: &mut Context<Self>,
    ) {
        let view_id = self.views.active().view_id.clone();
        let detail_cancellations =
            self.panel_reads
                .cancel_except(&view_id, ReadKind::Detail, details);
        let overview_cancellations =
            self.panel_reads
                .cancel_except(&view_id, ReadKind::Overview, overviews);
        self.cancel_planned_reads(detail_cancellations);
        self.cancel_planned_reads(overview_cancellations);
        let changed = self.views.evict_hidden_panel_details(details)
            | self.views.evict_unretained_panel_overviews(overviews);
        if changed {
            self.publish_snapshot();
            cx.notify();
        }
    }

    pub(crate) fn reconcile_active_detail_viewport(
        &mut self,
        viewport: AlignmentViewport,
        cx: &mut Context<Self>,
    ) {
        let view_id = self.views.active().view_id.clone();
        let cancellations = self
            .panel_reads
            .cancel_except(&view_id, ReadKind::Detail, &[]);
        self.cancel_planned_reads(cancellations);
        if self.views.retain_active_detail_coverage(viewport) {
            self.publish_snapshot();
            cx.notify();
        }
    }

    pub(crate) fn cancel_unselected_inspectors(&mut self) {
        let view = self.views.active();
        let view_id = view.view_id.clone();
        let retained = view.selected_panel_id.iter().cloned().collect::<Vec<_>>();
        let cancellations =
            self.panel_reads
                .cancel_except(&view_id, ReadKind::Inspector, &retained);
        self.cancel_planned_reads(cancellations);
    }

    pub(crate) fn cancel_removed_panel_reads(&mut self) {
        let view = self.views.active();
        let view_id = view.view_id.clone();
        let retained = view
            .panels
            .iter()
            .map(|panel| panel.panel_id.clone())
            .collect::<Vec<_>>();
        let cancellations = self.panel_reads.cancel_missing_panels(&view_id, &retained);
        self.cancel_planned_reads(cancellations);
    }

    pub(crate) fn cancel_planned_reads(&mut self, cancellations: Vec<PlannedReadCancellation>) {
        for cancellation in cancellations {
            self.sources.cancel(
                &cancellation.source_id,
                cancellation.generation,
                cancellation.kind,
                &cancellation.metric_key,
            );
        }
    }

    pub(crate) fn replace_sources(
        &mut self,
        sources: Vec<ConfiguredSource>,
        visible_runs: &[RunRef],
        cx: &mut Context<Self>,
    ) {
        self.event_tasks.clear();
        self.sources = crate::data::registry::SourceRegistry::default();
        self.panel_reads = crate::workbench::panel_reads::PanelReadCoordinator::default();
        self.configure_sources(sources, visible_runs, cx);
        self.publish_snapshot();
        cx.notify();
    }

    pub(crate) fn configure_sources(
        &mut self,
        sources: Vec<ConfiguredSource>,
        visible_runs: &[RunRef],
        cx: &mut Context<Self>,
    ) {
        for source in sources {
            let source_id = self.sources.configure(source);
            self.request_discovery(source_id, visible_runs, cx);
        }
    }

    pub(crate) fn refresh_sources(
        &mut self,
        source_ids: impl IntoIterator<Item = DataSourceId>,
        visible_runs: &[RunRef],
        cx: &mut Context<Self>,
    ) {
        for source_id in source_ids {
            self.request_discovery(source_id, visible_runs, cx);
        }
    }

    pub(crate) fn request_panel_overview(
        &mut self,
        panel_id: &MetricPanelId,
        runs: Vec<RunRef>,
        axis: CurveAxis,
        logical_width: u32,
        mode: PanelReadMode,
        cx: &mut Context<Self>,
    ) {
        let runs = self.requestable_runs(runs);
        if runs.is_empty() {
            return;
        }
        let Some(metric_key) = self
            .views
            .active_panel(panel_id)
            .map(|panel| panel.metric_key.clone())
        else {
            return;
        };
        let request = PanelReadRequest::Overview {
            runs,
            metric_key,
            axis,
            logical_width,
        };
        self.begin_panel_read(panel_id, mode, request, None, cx);
    }

    fn request_panel_detail(&mut self, query: PanelDetailQuery, cx: &mut Context<Self>) {
        let runs = self.requestable_runs(query.runs);
        if runs.is_empty() {
            return;
        }
        let Some(metric_key) = self
            .views
            .active_panel(&query.panel_id)
            .map(|panel| panel.metric_key.clone())
        else {
            return;
        };
        let request = PanelReadRequest::Detail {
            runs,
            metric_key,
            axis: query.axis,
            viewport: query.viewport,
            logical_width: query.logical_width,
        };
        self.begin_panel_read(
            &query.panel_id,
            query.mode,
            request,
            Some((query.viewport, query.logical_width)),
            cx,
        );
    }

    pub(crate) fn request_inspector(
        &mut self,
        panel_id: &MetricPanelId,
        runs: Vec<RunRef>,
        cx: &mut Context<Self>,
    ) {
        let runs = self.requestable_runs(runs);
        if runs.is_empty() {
            return;
        }
        let Some(metric_key) = self
            .views
            .active_panel(panel_id)
            .map(|panel| panel.metric_key.clone())
        else {
            return;
        };
        self.begin_panel_read(
            panel_id,
            PanelReadMode::Replace,
            PanelReadRequest::Inspector { runs, metric_key },
            None,
            cx,
        );
    }

    fn request_discovery(
        &mut self,
        source_id: DataSourceId,
        visible_runs: &[RunRef],
        cx: &mut Context<Self>,
    ) {
        let request = DiscoveryRequest {
            project_allowlist: None,
            project_id: None,
            selected_run_ids: Vec::new(),
            metric_runs: visible_runs
                .iter()
                .filter(|run| run.source_id == source_id)
                .map(|run| (run.project_id.clone(), run.run_id.clone()))
                .collect(),
        };
        let generation = self.allocate_generation();
        self.submit(source_id, generation, ReadRequest::Discover(request), cx);
    }

    fn requestable_runs(&self, runs: Vec<RunRef>) -> Vec<RunRef> {
        runs.into_iter()
            .filter(|run| {
                self.sources.source(&run.source_id).is_some_and(|source| {
                    !matches!(source.status, SourceStatus::Failed(_))
                        && source.catalog.runs.iter().any(|candidate| {
                            candidate.project_id == run.project_id && candidate.run_id == run.run_id
                        })
                })
            })
            .collect()
    }

    fn begin_panel_read(
        &mut self,
        panel_id: &MetricPanelId,
        mode: PanelReadMode,
        request: PanelReadRequest,
        detail: Option<(AlignmentViewport, u32)>,
        cx: &mut Context<Self>,
    ) {
        let kind = match &request {
            PanelReadRequest::Overview { .. } => ReadKind::Overview,
            PanelReadRequest::Detail { .. } => ReadKind::Detail,
            PanelReadRequest::Inspector { .. } => ReadKind::Inspector,
        };
        let overview_logical_width = match &request {
            PanelReadRequest::Overview { logical_width, .. } => Some(*logical_width),
            PanelReadRequest::Detail { .. } | PanelReadRequest::Inspector { .. } => None,
        };
        let generation = self.allocate_generation();
        let tag = PanelReadTag {
            view_id: self.views.active().view_id.clone(),
            panel_id: panel_id.clone(),
            generation,
            mode,
        };
        let planned = match self.panel_reads.begin(tag, request) {
            Ok(planned) => planned,
            Err(error) => {
                self.transient_error = Some(error.to_string());
                self.publish_snapshot();
                cx.notify();
                return;
            }
        };
        if let Some((viewport, logical_width)) = detail {
            self.views
                .begin_active_panel_detail(panel_id, generation, viewport, logical_width);
        } else if let Some(logical_width) = overview_logical_width {
            self.views
                .begin_active_panel_overview(panel_id, generation, logical_width);
        } else {
            self.views
                .begin_active_panel_read(panel_id, kind, generation);
        }
        self.publish_snapshot();
        self.submit_planned_reads(planned, cx);
    }

    fn submit_planned_reads(&mut self, planned: Vec<PlannedSourceRead>, cx: &mut Context<Self>) {
        for read in planned {
            self.submit(read.source_id, read.generation, read.request, cx);
        }
    }
}

impl ViewerApp {
    pub(in crate::desktop::app) fn retain_active_panel_curves(
        &mut self,
        details: &[MetricPanelId],
        overviews: &[MetricPanelId],
        cx: &mut Context<Self>,
    ) {
        self.session.update(cx, |session, cx| {
            session.retain_active_panel_curves(details, overviews, cx);
        });
    }

    pub(in crate::desktop::app) fn cancel_unselected_inspectors(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |session, _| {
            session.cancel_unselected_inspectors();
        });
    }

    pub(in crate::desktop::app) fn cancel_removed_panel_reads(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |session, _| {
            session.cancel_removed_panel_reads();
        });
    }

    pub(in crate::desktop::app) fn reconcile_active_detail_viewport(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(viewport) = self
            .session_snapshot(cx)
            .views
            .active()
            .navigation
            .selected_viewport()
        else {
            return;
        };
        self.session.update(cx, |session, cx| {
            session.reconcile_active_detail_viewport(viewport, cx);
        });
    }

    pub(in crate::desktop::app) fn open_configured_sources(
        &mut self,
        sources: Vec<ConfiguredSource>,
        cx: &mut Context<Self>,
    ) {
        let visible_runs = self.active_visible_runs(cx);
        self.session.update(cx, |session, cx| {
            session.configure_sources(sources, &visible_runs, cx);
        });
    }

    pub(in crate::desktop::app) fn restore_toml_workbench(
        &mut self,
        document: TomlWorkbenchDocument,
        cx: &mut Context<Self>,
    ) {
        let restored = self.session.update(cx, |session, cx| {
            session.restore_toml_document(document, cx)
        });
        self.apply_restored_workbench(restored, cx);
    }

    fn apply_restored_workbench(&mut self, restored: RestoredWorkbench, cx: &mut Context<Self>) {
        let layout = restored.layout;
        self.project_sidebar.update(cx, |sidebar, _| {
            sidebar.visible = layout.project_sidebar_visible;
            sidebar.width = px(layout.project_sidebar_width);
        });
        self.workspace.update(cx, |workspace, _| {
            workspace.metric_sidebar_compact = layout.metric_sidebar_compact;
        });
        self.bottom_inspector.update(cx, |inspector, _| {
            inspector.height = px(layout.bottom_inspector_height);
        });
        self.sync_active_view_workspace(cx);
        let inspector_visible = layout.bottom_inspector_visible
            && self
                .session_snapshot(cx)
                .views
                .active()
                .selected_panel_id
                .is_some();
        self.bottom_inspector.update(cx, |inspector, _| {
            inspector.visible = inspector_visible;
        });
        let visible_runs = self.active_visible_runs(cx);
        self.session.update(cx, |session, cx| {
            session.refresh_sources(restored.available_source_ids, &visible_runs, cx);
        });
    }

    pub(in crate::desktop::app) fn refresh_catalog(&mut self, cx: &mut Context<Self>) {
        let visible_runs = self.active_visible_runs(cx);
        let mut source_ids = visible_runs
            .iter()
            .map(|run| run.source_id.clone())
            .collect::<HashSet<_>>();
        if source_ids.is_empty() {
            source_ids.extend(
                self.session_snapshot(cx)
                    .sources
                    .iter()
                    .map(|source| source.source_id.clone()),
            );
        }
        self.session.update(cx, |session, cx| {
            session.refresh_sources(source_ids, &visible_runs, cx);
        });
    }

    pub(in crate::desktop::app) fn refresh_all_sources(&mut self, cx: &mut Context<Self>) {
        let visible_runs = self.active_visible_runs(cx);
        let source_ids = self
            .session_snapshot(cx)
            .sources
            .iter()
            .map(|source| source.source_id.clone())
            .collect::<Vec<_>>();
        self.session.update(cx, |session, cx| {
            session.refresh_sources(source_ids, &visible_runs, cx);
        });
    }

    pub(in crate::desktop::app) fn handle_session_event(
        &mut self,
        event: WorkbenchSessionEvent,
        cx: &mut Context<Self>,
    ) {
        self.sync_child_snapshots(cx);
        match event {
            WorkbenchSessionEvent::ReadApplied(effect) => {
                if effect.accepted && matches!(effect.kind, ReadKind::Overview | ReadKind::Detail) {
                    self.workspace.update(cx, |workspace, cx| {
                        workspace.defer_metric_repaint(cx);
                    });
                }
                let session = self.session_snapshot(cx);
                if effect.succeeded
                    && effect.kind == ReadKind::Catalog
                    && !self.active_visible_runs(cx).is_empty()
                    && !session.views.active().panels.is_empty()
                {
                    self.request_missing_panel_curves(cx);
                    if self.inspector_visible(cx) {
                        self.request_inspector(cx);
                    }
                }
            }
            WorkbenchSessionEvent::AutosaveFinished {
                revision,
                succeeded,
            } => {
                let _ = (revision, succeeded);
            }
        }
        cx.notify();
    }

    pub(in crate::desktop::app) fn request_overview(&mut self, cx: &mut Context<Self>) {
        self.request_detail(cx);
    }

    pub(in crate::desktop::app) fn request_panel_overview_for_runs(
        &mut self,
        panel_id: &MetricPanelId,
        runs: Vec<RunRef>,
        mode: PanelReadMode,
        cx: &mut Context<Self>,
    ) {
        if runs.is_empty() {
            return;
        }
        let logical_width = self
            .workspace
            .read(cx)
            .overview_logical_width
            .ceil()
            .max(1.) as u32;
        let axis = self.curve_axis(cx);
        self.session.update(cx, |session, cx| {
            session.request_panel_overview(panel_id, runs, axis, logical_width, mode, cx);
        });
    }

    pub(in crate::desktop::app) fn request_detail(&mut self, cx: &mut Context<Self>) {
        let scheduled = self
            .workspace
            .update(cx, |workspace, cx| workspace.reconcile_track_schedule(cx));
        self.submit_scheduled_panel_reads(&scheduled, cx);
    }

    pub(in crate::desktop::app) fn submit_scheduled_panel_reads(
        &mut self,
        scheduled: &[ScheduledPanelRead],
        cx: &mut Context<Self>,
    ) {
        for request in scheduled {
            match request {
                ScheduledPanelRead::Overview {
                    panel_id,
                    runs,
                    mode,
                } => self.request_panel_overview_for_runs(panel_id, runs.clone(), *mode, cx),
                ScheduledPanelRead::Detail {
                    panel_id,
                    viewport,
                    logical_width,
                } => self.request_panel_detail(panel_id, *viewport, *logical_width, cx),
            }
        }
    }

    pub(in crate::desktop::app) fn request_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(panel_id) = self
            .session_snapshot(cx)
            .views
            .active()
            .selected_panel_id
            .clone()
        else {
            return;
        };
        let runs = self.active_visible_runs(cx);
        if runs.is_empty() {
            return;
        }
        self.session.update(cx, |session, cx| {
            session.request_inspector(&panel_id, runs, cx);
        });
    }

    pub(in crate::desktop::app) fn request_panel_detail(
        &mut self,
        panel_id: &MetricPanelId,
        viewport: AlignmentViewport,
        logical_width: u32,
        cx: &mut Context<Self>,
    ) {
        self.request_panel_detail_for_runs(
            panel_id,
            viewport,
            logical_width,
            self.active_visible_runs(cx),
            PanelReadMode::Replace,
            cx,
        );
    }

    pub(in crate::desktop::app) fn request_panel_detail_for_runs(
        &mut self,
        panel_id: &MetricPanelId,
        viewport: AlignmentViewport,
        logical_width: u32,
        runs: Vec<RunRef>,
        mode: PanelReadMode,
        cx: &mut Context<Self>,
    ) {
        if runs.is_empty() {
            return;
        }
        let axis = self.curve_axis(cx);
        self.session.update(cx, |session, cx| {
            session.request_panel_detail(
                PanelDetailQuery {
                    panel_id: panel_id.clone(),
                    viewport,
                    logical_width,
                    runs,
                    axis,
                    mode,
                },
                cx,
            );
        });
    }

    pub(in crate::desktop::app) fn request_missing_panel_curves(&mut self, cx: &mut Context<Self>) {
        self.request_detail(cx);
    }
}
