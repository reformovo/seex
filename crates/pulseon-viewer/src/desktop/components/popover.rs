use gpui::{Div, ElementId, Stateful, div, prelude::*};

use super::{IconName, icon};
use crate::desktop::app::theme::ViewerTheme;

pub(in crate::desktop::app) fn popover(theme: ViewerTheme) -> Div {
    div()
        .occlude()
        .p_3()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
}

pub(in crate::desktop::app) fn popover_menu_item(
    id: impl Into<ElementId>,
    label: &str,
    icon_name: Option<IconName>,
    theme: ViewerTheme,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(theme.spacing.control_height)
        .px_2()
        .gap_2()
        .rounded(theme.spacing.corner_radius)
        .flex()
        .items_center()
        .cursor_pointer()
        .hover(|style| style.bg(theme.colors.element_hover))
        .children(icon_name.map(|icon_name| icon(icon_name, theme)))
        .child(label.to_owned())
}
