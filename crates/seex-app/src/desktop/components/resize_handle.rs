use gpui::{SharedString, div, prelude::*, px};

use super::super::theme::ViewerTheme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResizeEdge {
    Top,
    Right,
    Bottom,
}

pub(crate) fn resize_handle(
    id: SharedString,
    theme: ViewerTheme,
    active: bool,
    edge: ResizeEdge,
) -> gpui::Stateful<gpui::Div> {
    let handle = div()
        .id(id)
        .absolute()
        .border_color(if active {
            theme.colors.focus
        } else {
            theme.colors.border
        })
        .hover(move |style| style.border_color(theme.colors.focus));

    match edge {
        ResizeEdge::Top => handle
            .left_0()
            .right_0()
            .top_0()
            .h(px(5.))
            .border_t_1()
            .cursor(gpui::CursorStyle::ResizeUpDown),
        ResizeEdge::Right => handle
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(5.))
            .border_r_1()
            .cursor(gpui::CursorStyle::ResizeLeftRight),
        ResizeEdge::Bottom => handle
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(5.))
            .border_b_1()
            .cursor(gpui::CursorStyle::ResizeUpDown),
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AbsoluteLength, CursorStyle, Styled, WindowAppearance};

    use super::*;

    #[test]
    fn inactive_handle_uses_the_boundary_color() {
        let theme = ViewerTheme::for_appearance(WindowAppearance::Light);
        let mut handle = resize_handle("resize".into(), theme, false, ResizeEdge::Top);

        assert_eq!(
            handle.style().border_color,
            Some(theme.colors.border.into())
        );
    }

    #[test]
    fn active_handle_uses_the_focus_color() {
        let theme = ViewerTheme::for_appearance(WindowAppearance::Light);
        let mut handle = resize_handle("resize".into(), theme, true, ResizeEdge::Top);

        assert_eq!(handle.style().border_color, Some(theme.colors.focus.into()));
    }

    #[test]
    fn edge_selects_the_boundary_and_resize_cursor() {
        let theme = ViewerTheme::for_appearance(WindowAppearance::Light);
        let one_pixel = Some(AbsoluteLength::Pixels(px(1.)));

        for (edge, expected_cursor, expected_borders) in [
            (
                ResizeEdge::Top,
                CursorStyle::ResizeUpDown,
                (one_pixel, None, None, None),
            ),
            (
                ResizeEdge::Right,
                CursorStyle::ResizeLeftRight,
                (None, one_pixel, None, None),
            ),
            (
                ResizeEdge::Bottom,
                CursorStyle::ResizeUpDown,
                (None, None, one_pixel, None),
            ),
        ] {
            let mut handle = resize_handle("resize".into(), theme, false, edge);
            let style = handle.style();
            assert_eq!(
                (
                    style.border_widths.top,
                    style.border_widths.right,
                    style.border_widths.bottom,
                    style.border_widths.left,
                ),
                expected_borders,
            );
            assert_eq!(style.mouse_cursor, Some(expected_cursor));
        }
    }
}
