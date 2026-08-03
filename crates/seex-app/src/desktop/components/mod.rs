use gpui::{
    AnyView, App, Context, Div, ElementId, Pixels, Render, SharedString, Stateful, Svg, Window,
    div, prelude::*, px, svg,
};

use super::theme::ViewerTheme;

mod popover;
mod resize_handle;
mod text_input;

pub(super) use popover::{popover, popover_menu_item};
pub(super) use resize_handle::{ResizeEdge, resize_handle};
pub(super) use text_input::TextInput;

#[derive(Clone, Copy)]
pub(super) enum DialogButtonKind {
    Primary,
    Secondary,
}

pub(super) fn modal_backdrop(id: &'static str, theme: ViewerTheme) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .occlude()
        .absolute()
        .inset_0()
        .p_4()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.colors.modal_backdrop)
}

pub(super) fn dialog_surface(
    id: &'static str,
    width: Pixels,
    max_height: Pixels,
    theme: ViewerTheme,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .w(width)
        .max_h(max_height)
        .p_4()
        .gap_3()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .text_xs()
        .text_color(theme.colors.text)
}

pub(super) fn dialog_title(label: &'static str) -> Div {
    div()
        .text_sm()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(label)
}

pub(super) fn dialog_button(
    id: &'static str,
    label: &'static str,
    theme: ViewerTheme,
    kind: DialogButtonKind,
    disabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .tab_index(if disabled { -1 } else { 0 })
        .h(theme.spacing.control_height)
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .text_xs()
        .when(matches!(kind, DialogButtonKind::Secondary), |button| {
            button
                .bg(theme.colors.surface)
                .border_color(theme.colors.border)
                .text_color(theme.colors.text)
        })
        .when(
            matches!(kind, DialogButtonKind::Primary) && !disabled,
            |button| {
                button
                    .bg(theme.colors.accent)
                    .border_color(theme.colors.accent)
                    .text_color(theme.colors.accent_text)
            },
        )
        .when(!disabled, |button| {
            button.cursor_pointer().hover(move |style| {
                style.bg(match kind {
                    DialogButtonKind::Primary => theme.colors.accent_hover,
                    DialogButtonKind::Secondary => theme.colors.element_hover,
                })
            })
        })
        .when(disabled, |button| {
            button
                .bg(theme.colors.disabled)
                .border_color(theme.colors.disabled)
                .text_color(theme.colors.accent_text)
                .opacity(0.45)
                .cursor_default()
        })
        .child(label)
}

pub(super) fn checkbox(
    id: impl Into<SharedString>,
    theme: ViewerTheme,
    checked: bool,
) -> Stateful<Div> {
    let id = id.into();
    let selector = id.clone();
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(if checked {
            theme.colors.accent
        } else {
            theme.colors.border
        })
        .bg(if checked {
            theme.colors.accent
        } else {
            theme.colors.surface
        })
        .children(checked.then(|| {
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.colors.accent_text)
                .child("✓")
        }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IconName {
    Archive,
    ArrowUp,
    Baseline,
    ChevronUp,
    CirclePlay,
    Clock,
    Close,
    Duplicate,
    Edit,
    Ellipsis,
    Eye,
    EyeClosed,
    EyeOff,
    Folder,
    FolderOpen,
    PanelBottomClose,
    PanelBottomOpen,
    PanelLeft,
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
            Self::ChevronUp => "icons/chevron-up.svg",
            Self::CirclePlay => "icons/circle-play.svg",
            Self::Clock => "icons/clock.svg",
            Self::Close => "icons/close.svg",
            Self::Duplicate => "icons/duplicate.svg",
            Self::Edit => "icons/edit.svg",
            Self::Ellipsis => "icons/ellipsis.svg",
            Self::Eye => "icons/eye.svg",
            Self::EyeClosed => "icons/eye-closed.svg",
            Self::EyeOff => "icons/eye-off.svg",
            Self::Folder => "icons/folder.svg",
            Self::FolderOpen => "icons/folder-open.svg",
            Self::PanelBottomClose => "icons/panel-bottom-close.svg",
            Self::PanelBottomOpen => "icons/panel-bottom-open.svg",
            Self::PanelLeft => "icons/panel-left.svg",
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
        .px_2()
        .flex()
        .items_center()
        .text_xs()
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

pub fn top_bar_icon_button(
    id: impl Into<ElementId>,
    theme: ViewerTheme,
    active: bool,
    disabled: bool,
) -> Stateful<Div> {
    focus_ring(id, theme)
        .tab_index(if disabled { -1 } else { 0 })
        .size(px(20.))
        .flex_none()
        .rounded(theme.spacing.corner_radius)
        .flex()
        .items_center()
        .justify_center()
        .opacity(if active { 1. } else { 0.62 })
        .when(!disabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(theme.colors.element_hover).opacity(1.))
        })
        .when(disabled, |button| button.opacity(0.35).cursor_default())
}

pub fn sidebar_icon_button(
    id: impl Into<ElementId>,
    theme: ViewerTheme,
    active: bool,
) -> Stateful<Div> {
    focus_ring(id, theme)
        .tab_index(0)
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

pub fn sidebar_hover_icon_button(
    id: impl Into<ElementId>,
    theme: ViewerTheme,
    active: bool,
    hover_group: SharedString,
    force_visible: bool,
) -> Stateful<Div> {
    sidebar_icon_button(id, theme, active)
        .invisible()
        .opacity(0.)
        .group_hover(hover_group, |style| style.visible().opacity(1.))
        .when(force_visible, |button| button.visible().opacity(1.))
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

#[cfg(test)]
mod tests {
    use super::IconName;

    #[test]
    fn icon_name_has_a_stable_asset_path() {
        assert_eq!(IconName::Refresh.path(), "icons/refresh.svg");
        assert_eq!(IconName::Folder.path(), "icons/folder.svg");
        assert_eq!(IconName::FolderOpen.path(), "icons/folder-open.svg");
        assert_eq!(IconName::Eye.path(), "icons/eye.svg");
        assert_eq!(IconName::Close.path(), "icons/close.svg");
        assert_eq!(IconName::Duplicate.path(), "icons/duplicate.svg");
        assert_eq!(IconName::Edit.path(), "icons/edit.svg");
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
