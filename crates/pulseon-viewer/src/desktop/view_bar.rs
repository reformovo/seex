use super::*;

impl ViewerApp {
    pub(super) fn render_analysis_bar(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let views = self.views.views().to_vec();
        let active_view_id = self.views.active().view_id.clone();
        let renaming_view = self.renaming_view.clone();
        let view_name_focus = self.view_name_focus.clone();
        let view_name_draft = self.view_name_draft.clone();
        let can_refresh = self.sources.sources().next().is_some();

        components::tab_bar(theme)
            .id("analysis-tab-bar")
            .debug_selector(|| "analysis-tab-bar".to_owned())
            .flex_shrink_0()
            .children((!self.project_sidebar_visible).then(|| {
                components::top_bar_icon_button("show-project-sidebar", theme, false, false)
                    .debug_selector(|| "show-project-sidebar".to_owned())
                    .tooltip(components::label_tooltip("Show Projects", theme))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.project_sidebar_visible = true;
                        cx.notify();
                    }))
                    .child(components::icon(IconName::PanelLeft, theme))
            }))
            .child(
                div()
                    .id("analysis-view-tabs")
                    .debug_selector(|| "analysis-view-tabs".to_owned())
                    .h_full()
                    .flex_1()
                    .flex()
                    .overflow_x_scroll()
                    .border_l_1()
                    .border_r_1()
                    .border_color(theme.colors.border)
                    .children(views.into_iter().enumerate().map(|(index, view)| {
                        let selected = view.view_id == active_view_id;
                        let activate_id = view.view_id.clone();
                        let close_id = view.view_id.clone();
                        let menu_id = view.view_id.clone();
                        let editing = renaming_view.as_ref() == Some(&view.view_id);
                        let focus = view_name_focus.clone();
                        let menu_open = self.view_menu.as_ref() == Some(&view.view_id);
                        let hover_group = SharedString::from(format!("view-tab-{index}"));
                        let mut tab = components::analysis_tab(
                            SharedString::from(format!("analysis-tab:{}", view.view_id)),
                            theme,
                            selected,
                        )
                        .group(hover_group.clone())
                        .relative()
                        .flex_none()
                        .debug_selector(move || {
                            if selected {
                                "analysis-tab".to_owned()
                            } else {
                                format!("analysis-tab-{index}")
                            }
                        })
                        .tab_index(0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.view_menu = None;
                            this.activate_analysis_view(&activate_id, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _, _, cx| {
                                this.dismiss_popovers();
                                this.view_menu = Some(menu_id.clone());
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .child(if editing {
                            div()
                                .id(SharedString::from(format!("rename-view:{}", view.view_id)))
                                .track_focus(&focus)
                                .on_key_down(cx.listener(Self::on_view_name_key))
                                .child(view_name_draft.clone())
                        } else {
                            div()
                                .id(SharedString::from(format!("view-name:{}", view.view_id)))
                                .whitespace_nowrap()
                                .child(view.name)
                        })
                        .child(
                            div()
                                .id(SharedString::from(format!("close-view:{}", close_id)))
                                .debug_selector(move || {
                                    if selected {
                                        "close-active-view".to_owned()
                                    } else {
                                        format!("close-view-{index}")
                                    }
                                })
                                .size(px(20.))
                                .flex_none()
                                .border_1()
                                .border_color(theme.colors.transparent)
                                .rounded(theme.spacing.corner_radius)
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .opacity(if selected { 0.62 } else { 0. })
                                .group_hover(hover_group, |style| style.opacity(1.))
                                .tab_index(0)
                                .focus(|style| style.opacity(1.))
                                .tooltip(components::label_tooltip("Close View", theme))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.close_analysis_view(&close_id, cx);
                                    cx.stop_propagation();
                                }))
                                .child(components::icon(IconName::Close, theme)),
                        );
                        if menu_open {
                            tab = tab.child(deferred(
                                anchored()
                                    .anchor(Corner::TopLeft)
                                    .snap_to_window_with_margin(px(8.))
                                    .offset(point(px(0.), theme.spacing.tab_height))
                                    .child(self.render_view_menu(view.view_id, cx)),
                            ));
                        }
                        tab
                    })),
            )
            .child(
                components::top_bar_icon_button("new-view", theme, false, false)
                    .debug_selector(|| "new-view".to_owned())
                    .tooltip(components::label_tooltip("New View", theme))
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.create_analysis_view(cx)))
                    .child(components::icon(IconName::Plus, theme)),
            )
            .child(
                components::top_bar_icon_button(
                    "toggle-bottom-inspector",
                    theme,
                    self.bottom_inspector_visible,
                    self.views.active().selected_panel_id.is_none(),
                )
                .debug_selector(|| "toggle-bottom-inspector".to_owned())
                .tooltip(components::label_tooltip(
                    if self.bottom_inspector_visible {
                        "Hide bottom inspector"
                    } else {
                        "Show bottom inspector"
                    },
                    theme,
                ))
                .when(self.views.active().selected_panel_id.is_some(), |button| {
                    button
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_toggle_bottom_inspector(&ToggleBottomInspector, window, cx);
                        }))
                })
                .child(components::icon(
                    if self.bottom_inspector_visible {
                        IconName::PanelBottomClose
                    } else {
                        IconName::PanelBottomOpen
                    },
                    theme,
                )),
            )
            .child(
                components::top_bar_icon_button("refresh-view", theme, false, !can_refresh)
                    .debug_selector(|| "refresh-view".to_owned())
                    .tooltip(components::label_tooltip("Refresh", theme))
                    .when(can_refresh, |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.local_error = None;
                                this.refresh_all_sources(cx);
                                cx.notify();
                            }))
                    })
                    .child(components::icon(IconName::Refresh, theme)),
            )
    }

    fn render_view_menu(
        &mut self,
        view_id: AnalysisViewId,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let duplicate_id = view_id.clone();
        let rename_id = view_id.clone();
        let close_id = view_id.clone();
        components::popover(theme)
            .id(SharedString::from(format!("view-menu:{view_id}")))
            .debug_selector(|| "view-menu".to_owned())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.dismiss_popovers() {
                    cx.notify();
                }
            }))
            .w(px(180.))
            .flex()
            .flex_col()
            .child(
                view_menu_item("duplicate-view", "Duplicate View", theme)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_analysis_view(&duplicate_id, cx);
                        this.duplicate_analysis_view(cx);
                        this.view_menu = None;
                    }))
                    .debug_selector(|| "duplicate-view".to_owned()),
            )
            .child(
                view_menu_item("rename-view", "Rename View", theme)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.begin_rename_analysis_view(rename_id.clone(), window, cx);
                        this.view_menu = None;
                    }))
                    .debug_selector(|| "rename-view".to_owned()),
            )
            .child(
                view_menu_item("close-view", "Close View", theme).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.close_analysis_view(&close_id, cx);
                        this.view_menu = None;
                    },
                )),
            )
    }
}

fn view_menu_item(
    id: impl Into<gpui::ElementId>,
    label: &str,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(theme.spacing.control_height)
        .px_2()
        .rounded(theme.spacing.corner_radius)
        .flex()
        .items_center()
        .cursor_pointer()
        .hover(|style| style.bg(theme.colors.element_hover))
        .child(label.to_owned())
}
