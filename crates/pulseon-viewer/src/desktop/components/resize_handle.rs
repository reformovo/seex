use gpui::{SharedString, div, prelude::*, px};

use super::super::theme::ViewerTheme;

pub(crate) fn horizontal_resize_handle(
    id: SharedString,
    theme: ViewerTheme,
    active: bool,
    top_edge: bool,
) -> gpui::Stateful<gpui::Div> {
    let group = SharedString::from(format!("resize-boundary:{id}"));
    div()
        .id(id)
        .group(group.clone())
        .absolute()
        .left_0()
        .right_0()
        .h(px(5.))
        .when(top_edge, |handle| handle.top_0())
        .when(!top_edge, |handle| handle.bottom_0())
        .cursor(gpui::CursorStyle::ResizeUpDown)
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(1.))
                .when(top_edge, |line| line.top_0())
                .when(!top_edge, |line| line.bottom_0())
                .bg(if active {
                    theme.colors.focus
                } else {
                    theme.colors.transparent
                })
                .group_hover(group, |line| line.bg(theme.colors.focus)),
        )
}

pub(crate) fn vertical_resize_handle(
    id: SharedString,
    theme: ViewerTheme,
    active: bool,
    owns_boundary: bool,
) -> gpui::Stateful<gpui::Div> {
    let group = SharedString::from(format!("resize-boundary:{id}"));
    div()
        .id(id)
        .group(group.clone())
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(px(5.))
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(1.))
                .bg(if active {
                    theme.colors.focus
                } else if owns_boundary {
                    theme.colors.border
                } else {
                    theme.colors.transparent
                })
                .group_hover(group, |line| line.bg(theme.colors.focus)),
        )
}
