use gpui::{Div, ElementId, Rgba, Stateful, Svg, div, prelude::*, px, svg};

use super::theme::ViewerTheme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusTone {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IconName {
    ChevronDown,
    ChevronRight,
    Close,
    Ellipsis,
    Plus,
    Refresh,
}

impl IconName {
    const fn path(self) -> &'static str {
        match self {
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::Close => "icons/close.svg",
            Self::Ellipsis => "icons/ellipsis.svg",
            Self::Plus => "icons/plus.svg",
            Self::Refresh => "icons/refresh.svg",
        }
    }
}

pub fn icon(name: IconName, theme: ViewerTheme) -> Svg {
    svg()
        .path(name.path())
        .size(px(16.))
        .text_color(theme.colors.text_muted)
}

pub fn focus_ring(id: impl Into<ElementId>, theme: ViewerTheme) -> Stateful<Div> {
    div()
        .id(id)
        .border_1()
        .border_color(theme.colors.transparent)
        .focus(|style| style.border_color(theme.colors.focus))
}

pub fn tab_bar(theme: ViewerTheme) -> Div {
    div()
        .flex()
        .items_center()
        .h(theme.spacing.tab_height)
        .w_full()
        .bg(theme.colors.panel)
        .border_b_1()
        .border_color(theme.colors.border)
}

pub fn analysis_tab(id: impl Into<ElementId>, theme: ViewerTheme, selected: bool) -> Stateful<Div> {
    focus_ring(id, theme)
        .h_full()
        .px_3()
        .flex()
        .items_center()
        .cursor_pointer()
        .text_color(if selected {
            theme.colors.text
        } else {
            theme.colors.text_muted
        })
        .bg(if selected {
            theme.colors.surface
        } else {
            theme.colors.panel
        })
        .when(!selected, |tab| {
            tab.hover(|style| style.bg(theme.colors.element_hover))
        })
}

pub fn sidebar_tree_row(
    id: impl Into<ElementId>,
    theme: ViewerTheme,
    selected: bool,
    disabled: bool,
) -> Stateful<Div> {
    focus_ring(id, theme)
        .h(theme.spacing.tree_row_height)
        .px_2()
        .gap_2()
        .rounded(theme.spacing.corner_radius)
        .flex()
        .items_center()
        .when(selected, |row| row.bg(theme.colors.element_active))
        .when(!selected && !disabled, |row| {
            row.hover(|style| style.bg(theme.colors.element_hover))
        })
        .when(disabled, |row| row.opacity(0.45))
}

pub fn toolbar_button(
    id: impl Into<ElementId>,
    theme: ViewerTheme,
    selected: bool,
    disabled: bool,
) -> Stateful<Div> {
    focus_ring(id, theme)
        .h(theme.spacing.control_height)
        .px_3()
        .rounded(theme.spacing.corner_radius)
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |button| {
            button
                .bg(theme.colors.accent)
                .text_color(theme.colors.accent_text)
        })
        .when(!selected && !disabled, |button| {
            button.hover(|style| style.bg(theme.colors.element_hover))
        })
        .when(disabled, |button| {
            button.text_color(theme.colors.disabled).cursor_default()
        })
}

pub fn icon_button(
    id: impl Into<ElementId>,
    theme: ViewerTheme,
    selected: bool,
    disabled: bool,
) -> Stateful<Div> {
    toolbar_button(id, theme, selected, disabled)
        .w(theme.spacing.control_height)
        .px_0()
}

pub fn popover(theme: ViewerTheme) -> Div {
    div()
        .p_3()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
}

pub fn tooltip(theme: ViewerTheme) -> Div {
    popover(theme)
        .bg(theme.colors.tooltip_background)
        .text_color(theme.colors.tooltip_text)
        .text_sm()
}

pub fn status_badge(theme: ViewerTheme, tone: StatusTone) -> Div {
    let (background, text) = status_colors(theme, tone);
    div()
        .px_3()
        .py_2()
        .rounded(theme.spacing.corner_radius)
        .bg(background)
        .text_color(text)
        .text_sm()
}

pub fn empty_state(theme: ViewerTheme) -> Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(theme.spacing.content_gap)
        .text_color(theme.colors.text_muted)
}

fn status_colors(theme: ViewerTheme, tone: StatusTone) -> (Rgba, Rgba) {
    match tone {
        StatusTone::Info => (theme.colors.element_active, theme.colors.text),
        StatusTone::Warning => (theme.colors.warning_background, theme.colors.warning_text),
        StatusTone::Error => (theme.colors.error_background, theme.colors.error_text),
    }
}

#[cfg(test)]
mod tests {
    use gpui::WindowAppearance;

    use super::*;

    #[test]
    fn status_tones_use_distinct_semantic_pairs() {
        for appearance in [WindowAppearance::Light, WindowAppearance::Dark] {
            let theme = ViewerTheme::for_appearance(appearance);
            let info = status_colors(theme, StatusTone::Info);
            let warning = status_colors(theme, StatusTone::Warning);
            let error = status_colors(theme, StatusTone::Error);

            assert_ne!(info, warning);
            assert_ne!(warning, error);
            assert_ne!(info, error);
        }
    }

    #[test]
    fn icon_name_has_a_stable_asset_path() {
        assert_eq!(IconName::Refresh.path(), "icons/refresh.svg");
        assert_eq!(IconName::ChevronDown.path(), "icons/chevron-down.svg");
        assert_eq!(IconName::ChevronRight.path(), "icons/chevron-right.svg");
        assert_eq!(IconName::Close.path(), "icons/close.svg");
        assert_eq!(IconName::Ellipsis.path(), "icons/ellipsis.svg");
        assert_eq!(IconName::Plus.path(), "icons/plus.svg");
    }
}
