use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{Context, px};
use pulseon_model::alignment::AlignmentViewport;

use crate::data::DiscoveryRequest;
use crate::data::query::CurveAxis;
use crate::data::registry::SourceStatus;
use crate::data::worker::{ReadKind, ReadRequest};
use crate::domain::{DataSourceId, RunRef};
use crate::workbench::document::WorkbenchDocument;
use crate::workbench::panel_reads::{
    MetricPanelId, PanelReadMode, PanelReadRequest, PanelReadTag, PlannedSourceRead,
};

use super::super::ViewerApp;
use super::{WorkbenchSession, WorkbenchSessionEvent};

struct PanelDetailQuery {
    panel_id: MetricPanelId,
    viewport: AlignmentViewport,
    physical_width: u32,
    runs: Vec<RunRef>,
    axis: CurveAxis,
    mode: PanelReadMode,
}

impl WorkbenchSession {
    pub(crate) fn import_source(
        &mut self,
        path: PathBuf,
        visible_runs: &[RunRef],
        cx: &mut Context<Self>,
    ) -> DataSourceId {
        let source_id = self.sources.import(path);
        self.transient_error = None;
        self.persistence_dirty = true;
        self.publish_snapshot();
        self.request_discovery(source_id.clone(), visible_runs, cx);
        source_id
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
        physical_width: u32,
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
            physical_width,
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
            physical_width: query.physical_width,
        };
        self.begin_panel_read(
            &query.panel_id,
            query.mode,
            request,
            Some((query.viewport, query.physical_width)),
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
                self.sources
                    .source(&run.source_id)
                    .is_some_and(|source| !matches!(source.status, SourceStatus::Failed(_)))
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
        if let Some((viewport, physical_width)) = detail {
            self.views
                .begin_active_panel_detail(panel_id, generation, viewport, physical_width);
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
    pub(in crate::desktop::app) fn restore_workbench(
        &mut self,
        document: WorkbenchDocument,
        cx: &mut Context<Self>,
    ) {
        let restored = self
            .session
            .update(cx, |session, cx| session.restore_document(document, cx));
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

    pub(in crate::desktop::app) fn open_source(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let visible_runs = self.active_visible_runs(cx);
        self.session.update(cx, |session, cx| {
            session.import_source(path, &visible_runs, cx);
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
        }
        cx.notify();
    }

    pub(in crate::desktop::app) fn request_overview(&mut self, cx: &mut Context<Self>) {
        let panel_ids = self
            .session_snapshot(cx)
            .views
            .active()
            .panels
            .iter()
            .map(|panel| panel.panel_id.clone())
            .collect::<Vec<_>>();
        for panel_id in panel_ids {
            self.request_panel_overview(&panel_id, cx);
        }
    }

    pub(in crate::desktop::app) fn request_panel_overview(
        &mut self,
        panel_id: &MetricPanelId,
        cx: &mut Context<Self>,
    ) {
        self.request_panel_overview_for_runs(
            panel_id,
            self.active_visible_runs(cx),
            PanelReadMode::Replace,
            cx,
        );
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
        let physical_width = self.workspace.read(cx).overview_width;
        let axis = self.curve_axis(cx);
        self.session.update(cx, |session, cx| {
            session.request_panel_overview(panel_id, runs, axis, physical_width, mode, cx);
        });
    }

    pub(in crate::desktop::app) fn request_detail(&mut self, cx: &mut Context<Self>) {
        let session = self.session_snapshot(cx);
        let Some(viewport) = session.views.active().navigation.selected_viewport() else {
            return;
        };
        let viewport_state = self.workspace.read(cx).track_viewport.borrow().clone();
        let panel_count = session.views.active().panels.len();
        let scheduled = viewport_state.overscan.start.min(panel_count)
            ..viewport_state.overscan.end.min(panel_count);
        let panel_ids = session.views.active().panels[scheduled]
            .iter()
            .map(|panel| panel.panel_id.clone())
            .collect::<Vec<_>>();
        for panel_id in panel_ids {
            self.request_panel_detail(
                &panel_id,
                viewport,
                viewport_state.physical_width.max(1),
                cx,
            );
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
        physical_width: u32,
        cx: &mut Context<Self>,
    ) {
        self.request_panel_detail_for_runs(
            panel_id,
            viewport,
            physical_width,
            self.active_visible_runs(cx),
            PanelReadMode::Replace,
            cx,
        );
    }

    pub(in crate::desktop::app) fn request_panel_detail_for_runs(
        &mut self,
        panel_id: &MetricPanelId,
        viewport: AlignmentViewport,
        physical_width: u32,
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
                    physical_width,
                    runs,
                    axis,
                    mode,
                },
                cx,
            );
        });
    }

    pub(in crate::desktop::app) fn request_missing_panel_curves(&mut self, cx: &mut Context<Self>) {
        let runs = self.active_visible_runs(cx);
        let session = self.session_snapshot(cx);
        let viewport = session.views.active().navigation.selected_viewport();
        let viewport_state = self.workspace.read(cx).track_viewport.borrow().clone();
        let panels = session.views.active().panels.clone();
        for (index, panel) in panels.into_iter().enumerate() {
            let missing_overview = runs
                .iter()
                .filter(|run| {
                    panel.overview.as_ref().is_none_or(|snapshot| {
                        !snapshot.series.iter().any(|curve| &curve.run_ref == *run)
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing_overview.is_empty() {
                self.request_panel_overview_for_runs(
                    &panel.panel_id,
                    missing_overview,
                    PanelReadMode::Merge,
                    cx,
                );
            }
            let Some(viewport) = viewport else {
                continue;
            };
            if !viewport_state.overscan.contains(&index) {
                continue;
            }
            let missing_detail = runs
                .iter()
                .filter(|run| {
                    panel.detail.as_ref().is_none_or(|snapshot| {
                        !snapshot.series.iter().any(|curve| &curve.run_ref == *run)
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing_detail.is_empty() {
                self.request_panel_detail_for_runs(
                    &panel.panel_id,
                    viewport,
                    viewport_state.physical_width.max(1),
                    missing_detail,
                    PanelReadMode::Merge,
                    cx,
                );
            }
        }
    }
}
