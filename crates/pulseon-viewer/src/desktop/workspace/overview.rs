use gpui::{Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, div, prelude::*};

use super::super::command::WorkbenchCommand;
use super::super::{chart, theme::ViewerTheme};
use super::{BrushMutation, DragGesture, update_brush_drag};

use super::{AnalysisWorkspace, AnalysisWorkspaceEvent};

impl AnalysisWorkspace {
    pub(crate) fn render_overview(
        &mut self,
        theme: ViewerTheme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
            .is_none()
        {
            return div().h_full();
        }
        let overview_chart = self.overview_chart.clone();
        div().h_full().child(
            div()
                .id("overview-chart")
                .debug_selector(|| "overview-chart".to_owned())
                .focusable()
                .h_full()
                .w_full()
                .relative()
                .cursor_pointer()
                .bg(theme.colors.surface)
                .child(gpui::AnyView::from(overview_chart))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.begin_brush_drag(event, cx);
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    if event.dragging() {
                        this.move_brush_drag(event, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| this.finish_drag(cx)),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| this.finish_drag(cx)),
                ),
        )
    }
    pub(crate) fn begin_brush_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(brush) = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
        else {
            return;
        };
        let overview_chart = self.overview_chart.clone();
        let overview = overview_chart.read(cx);
        let drag = match overview.brush_target(brush, event.position) {
            Some(chart::BrushDragTarget::Start) => Some(DragGesture::BrushStart),
            Some(chart::BrushDragTarget::End) => Some(DragGesture::BrushEnd),
            Some(chart::BrushDragTarget::Window) => overview
                .axis_at(brush, event.position)
                .map(|last_axis| DragGesture::BrushWindow { last_axis }),
            None => None,
        };
        self.drag = drag;
        cx.notify();
    }
    pub(crate) fn move_brush_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(mut gesture) = self.drag.clone() else {
            return;
        };
        let Some(brush) = self
            .active_navigation()
            .and_then(|navigation| navigation.brush())
        else {
            return;
        };
        let overview_chart = self.overview_chart.clone();
        let Some(axis) = overview_chart.read(cx).axis_at(brush, event.position) else {
            return;
        };
        let command = update_brush_drag(brush, &mut gesture, axis).map(|mutation| match mutation {
            BrushMutation::ResizeStart(position) => WorkbenchCommand::ResizeBrushStart(position),
            BrushMutation::ResizeEnd(position) => WorkbenchCommand::ResizeBrushEnd(position),
            BrushMutation::Pan(delta) => WorkbenchCommand::PanViewport(delta),
        });
        if let Some(command) = command {
            cx.emit(AnalysisWorkspaceEvent::Command(command));
        }
        self.drag = Some(gesture);
        cx.notify();
    }
}
