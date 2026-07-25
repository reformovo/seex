use gpui::{
    AnyView, App, Context, Div, ElementId, Render, Rgba, SharedString, Stateful, Svg, Window, div,
    prelude::*, px, svg,
};

use super::theme::ViewerTheme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusTone {
    Info,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IconName {
    Archive,
    ArrowUp,
    Baseline,
    CirclePlay,
    Clock,
    Close,
    Ellipsis,
    Eye,
    EyeClosed,
    EyeOff,
    Folder,
    FolderOpen,
    PanelBottomClose,
    PanelBottomOpen,
    Pin,
    Plus,
    Refresh,
}

impl IconName {
    const fn path(self) -> &'static str {
        match self {
            Self::Archive => "icons/archive.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::Baseline => "icons/baseline.svg",
            Self::CirclePlay => "icons/circle-play.svg",
            Self::Clock => "icons/clock.svg",
            Self::Close => "icons/close.svg",
            Self::Ellipsis => "icons/ellipsis.svg",
            Self::Eye => "icons/eye.svg",
            Self::EyeClosed => "icons/eye-closed.svg",
            Self::EyeOff => "icons/eye-off.svg",
            Self::Folder => "icons/folder.svg",
            Self::FolderOpen => "icons/folder-open.svg",
            Self::PanelBottomClose => "icons/panel-bottom-close.svg",
            Self::PanelBottomOpen => "icons/panel-bottom-open.svg",
            Self::Pin => "icons/pin.svg",
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

pub fn sidebar_icon_button(
    id: impl Into<ElementId>,
    theme: ViewerTheme,
    active: bool,
) -> Stateful<Div> {
    focus_ring(id, theme)
        .size(theme.spacing.control_height)
        .flex_none()
        .rounded(theme.spacing.corner_radius)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .opacity(if active { 1. } else { 0.62 })
        .hover(|style| style.opacity(1.))
}

pub fn popover(theme: ViewerTheme) -> Div {
    div()
        .occlude()
        .p_3()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
}

pub fn tooltip(theme: ViewerTheme) -> Div {
    div()
        .p_3()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.tooltip_background)
        .text_color(theme.colors.tooltip_text)
        .text_sm()
}

pub fn label_tooltip(
    label: impl Into<SharedString>,
    theme: ViewerTheme,
) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let label = label.into();
    move |_, cx| {
        let label = label.clone();
        cx.new(|_| LabelTooltip { label, theme }).into()
    }
}

struct LabelTooltip {
    label: SharedString,
    theme: ViewerTheme,
}

impl Render for LabelTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        tooltip(self.theme)
            .id("label-tooltip")
            .debug_selector(|| "label-tooltip".to_owned())
            .px_2()
            .py_1()
            .whitespace_nowrap()
            .child(self.label.clone())
    }
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
            let error = status_colors(theme, StatusTone::Error);

            assert_ne!(info, error);
        }
    }

    #[test]
    fn icon_name_has_a_stable_asset_path() {
        assert_eq!(IconName::Refresh.path(), "icons/refresh.svg");
        assert_eq!(IconName::Folder.path(), "icons/folder.svg");
        assert_eq!(IconName::FolderOpen.path(), "icons/folder-open.svg");
        assert_eq!(IconName::Eye.path(), "icons/eye.svg");
        assert_eq!(IconName::Close.path(), "icons/close.svg");
        assert_eq!(IconName::Ellipsis.path(), "icons/ellipsis.svg");
        assert_eq!(IconName::Plus.path(), "icons/plus.svg");
        assert_eq!(IconName::ArrowUp.path(), "icons/arrow-up.svg");
        assert_eq!(IconName::Clock.path(), "icons/clock.svg");
        assert_eq!(
            IconName::PanelBottomOpen.path(),
            "icons/panel-bottom-open.svg"
        );
    }
}
