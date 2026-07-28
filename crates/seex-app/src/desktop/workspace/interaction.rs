use std::time::Duration;

use gpui::{Context, MouseDownEvent, MouseMoveEvent, ScrollWheelEvent, px};

use crate::workbench::panel_reads::MetricPanelId;

use super::super::command::WorkbenchCommand;
use super::{AnalysisWorkspace, AnalysisWorkspaceEvent, DragGesture, WorkspaceInteractionEvent};

impl AnalysisWorkspace {
    pub(crate) fn defer_metric_repaint(&mut self, cx: &mut Context<Self>) {
        if self.metric_repaint_pending {
            return;
        }
        self.metric_repaint_pending = true;
        // CONTEXT: The next foreground turn mirrors the repaint caused by a later input event,
        // after GPUI has finished dispatching the viewport-changing event.
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |this, cx| {
                this.metric_repaint_pending = false;
                this.sync_track_charts(cx);
                cx.notify();
            });
        })
        .detach();
    }
    pub(crate) fn begin_track_drag(
        &mut self,
        panel_id: MetricPanelId,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.drag = Some(DragGesture::Detail {
            panel_id: panel_id.clone(),
            origin_x: f64::from(event.position.x),
            last_x: f64::from(event.position.x),
            moved: false,
        });
        self.track_hovers.remove(&panel_id);
        cx.notify();
    }
    pub(crate) fn move_detail_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(DragGesture::Detail {
            panel_id,
            origin_x,
            mut last_x,
            mut moved,
        }) = self.drag.clone()
        else {
            return;
        };
        let current_x = f64::from(event.position.x);
        if !moved && (current_x - origin_x).abs() < 3. {
            return;
        }
        moved = true;
        let Some(range) = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
            .map(|brush| brush.selected())
        else {
            self.drag = Some(DragGesture::Detail {
                panel_id,
                origin_x,
                last_x: current_x,
                moved,
            });
            cx.notify();
            return;
        };
        let delta = self
            .track_charts
            .get(&panel_id)
            .and_then(|chart| chart.read(cx).pan_delta(range, last_x, event.position));
        if let Some(delta) = delta {
            cx.emit(AnalysisWorkspaceEvent::Command(
                WorkbenchCommand::PanViewport(delta),
            ));
        }
        last_x = current_x;
        self.drag = Some(DragGesture::Detail {
            panel_id,
            origin_x,
            last_x,
            moved,
        });
        cx.notify();
    }
    pub(crate) fn finish_track_click(
        &mut self,
        panel_id: &MetricPanelId,
        event: &gpui::ClickEvent,
        cx: &mut Context<Self>,
    ) {
        let click_position = match event {
            gpui::ClickEvent::Mouse(event) => {
                let delta = event.up.position - event.down.position;
                (f32::from(delta.x).abs() < 3. && f32::from(delta.y).abs() < 3.)
                    .then_some(event.up.position)
            }
            gpui::ClickEvent::Keyboard(_) => None,
        };
        if let Some(position) = click_position {
            self.drag = None;
            let range = self
                .active_navigation()
                .and_then(|navigation| navigation.brush())
                .map(|brush| brush.selected());
            let cursor = range.and_then(|range| {
                self.track_charts
                    .get(panel_id)
                    .and_then(|chart| chart.read(cx).axis_at(range, position))
            });
            cx.emit(AnalysisWorkspaceEvent::Interaction(
                WorkspaceInteractionEvent::LockedCursor(cursor),
            ));
            cx.emit(AnalysisWorkspaceEvent::ShowInspector(panel_id.clone()));
        } else if matches!(event, gpui::ClickEvent::Keyboard(_)) {
            cx.emit(AnalysisWorkspaceEvent::ShowInspector(panel_id.clone()));
        }
    }
    pub(crate) fn finish_moved_track_drag(&mut self, cx: &mut Context<Self>) {
        let moved = self
            .drag
            .as_ref()
            .is_some_and(|gesture| matches!(gesture, DragGesture::Detail { moved: true, .. }));
        if moved {
            self.drag.take();
            cx.notify();
        }
        if moved {
            cx.emit(AnalysisWorkspaceEvent::RequestDetail);
            cx.notify();
        }
    }
    pub(crate) fn finish_drag(&mut self, cx: &mut Context<Self>) {
        let should_refresh = self
            .drag
            .take()
            .is_some_and(|gesture| !matches!(gesture, DragGesture::Detail { moved: false, .. }));
        cx.notify();
        if should_refresh {
            cx.emit(AnalysisWorkspaceEvent::RequestDetail);
            cx.notify();
        }
    }
    pub(crate) fn zoom_track(
        &mut self,
        panel_id: &MetricPanelId,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
            .map(|brush| brush.selected())
        else {
            return;
        };
        let Some(anchor) = self
            .track_charts
            .get(panel_id)
            .and_then(|chart| chart.read(cx).axis_at(range, event.position))
        else {
            return;
        };
        let delta = f32::from(event.delta.pixel_delta(px(16.)).y);
        let factor = f64::from((-delta / 240.).exp().clamp(0.5, 2.));
        let Some(mut preview) = self.active_navigation().cloned() else {
            return;
        };
        if !preview.zoom_at(anchor, factor) {
            return;
        }
        cx.emit(AnalysisWorkspaceEvent::Command(
            WorkbenchCommand::ZoomViewport { anchor, factor },
        ));
        self.defer_metric_repaint(cx);
        self.schedule_detail_refresh(cx);
        cx.stop_propagation();
    }
    pub(crate) fn update_track_hover(
        &mut self,
        panel_id: &MetricPanelId,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        cx.emit(AnalysisWorkspaceEvent::Interaction(
            WorkspaceInteractionEvent::RulerHover(None),
        ));
        cx.emit(AnalysisWorkspaceEvent::Interaction(
            WorkspaceInteractionEvent::TrackPointerHover(None),
        ));
        let Some(chart) = self.track_charts.get(panel_id).cloned() else {
            return;
        };
        let (pointer_axis, hover) = chart.read(cx).hit_test(event.position);
        cx.emit(AnalysisWorkspaceEvent::Interaction(
            WorkspaceInteractionEvent::TrackPointerHover(
                pointer_axis.map(|axis| (panel_id.clone(), axis)),
            ),
        ));
        if let Some(hover) = hover {
            self.track_hovers.insert(panel_id.clone(), hover);
        } else {
            self.track_hovers.remove(panel_id);
        }
        cx.notify();
    }
    pub(crate) fn schedule_detail_refresh(&mut self, cx: &mut Context<Self>) {
        self.detail_refresh_token = self.detail_refresh_token.saturating_add(1);
        self.detail_refresh_pending = true;
        let token = self.detail_refresh_token;
        let timer = cx.background_executor().timer(Duration::from_millis(100));
        let task = cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                let current = this.detail_refresh_token;
                if current != token {
                    return;
                }
                this.detail_refresh_pending = false;
                cx.emit(AnalysisWorkspaceEvent::RequestDetail);
                cx.notify();
            });
        });
        self.zoom_task = Some(task);
    }
    pub(crate) fn cancel_detail_refresh(&mut self, cx: &mut Context<Self>) {
        self.detail_refresh_token = self.detail_refresh_token.saturating_add(1);
        self.detail_refresh_pending = false;
        self.zoom_task = None;
        cx.notify();
    }
}
