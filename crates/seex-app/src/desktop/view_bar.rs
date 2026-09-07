use std::sync::Arc;

#[cfg(all(test, feature = "test-support"))]
use gpui::App;
use gpui::{
    Context, Corner, EventEmitter, FocusHandle, KeyDownEvent, ListAlignment, ListState,
    MouseButton, Render, SharedString, Window, anchored, deferred, div, point, prelude::*, px,
};

use crate::workbench::panel_reads::AnalysisViewId;

use super::ViewerApp;
use super::command::WorkbenchCommand;
use super::components::{self, IconName, TextInput};
use super::session::SessionSnapshot;
use super::theme::ViewerTheme;
use super::workspace::TrackViewport;

pub(crate) struct AnalysisViewBar {
    pub rename_focus: FocusHandle,
    pub renaming_view: Option<AnalysisViewId>,
    pub menu: Option<AnalysisViewId>,
    pub name_input: gpui::Entity<TextInput>,
    snapshot: Option<Arc<SessionSnapshot>>,
    sidebar_visible: bool,
    inspector_visible: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum AnalysisViewBarEvent {
    Command(WorkbenchCommand),
    DismissOtherPopovers,
    ShowProjectSidebar,
    ToggleBottomInspector,
    RefreshSources,
}

impl EventEmitter<AnalysisViewBarEvent> for AnalysisViewBar {}

impl AnalysisViewBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|_| TextInput::default());
        cx.observe(&name_input, |_, _, cx| cx.notify()).detach();
        Self {
            rename_focus: cx.focus_handle().tab_stop(true),
            renaming_view: None,
            menu: None,
            name_input,
            snapshot: None,
            sidebar_visible: true,
            inspector_visible: false,
        }
    }

    pub(crate) fn sync(
        &mut self,
        snapshot: Arc<SessionSnapshot>,
        sidebar_visible: bool,
        inspector_visible: bool,
    ) {
        self.snapshot = Some(snapshot);
        self.sidebar_visible = sidebar_visible;
        self.inspector_visible = inspector_visible;
    }
}

impl Render for AnalysisViewBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ViewerTheme::for_appearance(window.appearance());
        let Some(session) = self.snapshot.clone() else {
            return components::tab_bar(theme)
                .id("analysis-tab-bar")
                .debug_selector(|| "analysis-tab-bar".to_owned());
        };
        let views = session.views.views().to_vec();
        let active_view_id = session.views.active().view_id.clone();
        let renaming_view = self.renaming_view.clone();
        let view_menu = self.menu.clone();
        let view_name_focus = self.rename_focus.clone();
        let name_input = self.name_input.clone();
        let view_name_input = name_input.read(cx);
        let view_name_draft = view_name_input.text().to_owned();
        let (view_name_prefix, view_name_suffix) =
            view_name_draft.split_at(view_name_input.cursor());
        let view_name_prefix = view_name_prefix.to_owned();
        let view_name_suffix = view_name_suffix.to_owned();
        let view_name_select_all = view_name_input.is_select_all();
        let view_name_cursor_visible = view_name_input.cursor_visible();
        let has_sources = !session.sources.is_empty();
        let can_refresh = has_sources;
        let inspector_visible = self.inspector_visible;
        let can_toggle_inspector = has_sources
            && (inspector_visible || session.views.active().selected_panel_id.is_some());
        let sidebar_visible = self.sidebar_visible;

        components::tab_bar(theme)
            .id("analysis-tab-bar")
            .debug_selector(|| "analysis-tab-bar".to_owned())
            .flex_shrink_0()
            .children((!sidebar_visible).then(|| {
                div()
                    .id("analysis-left-controls")
                    .debug_selector(|| "analysis-left-controls".to_owned())
                    .h_full()
                    .flex_none()
                    .px_1()
                    .flex()
                    .items_center()
                    .child(
                        components::top_bar_icon_button(
                            "show-project-sidebar",
                            theme,
                            false,
                            false,
                        )
                        .debug_selector(|| "show-project-sidebar".to_owned())
                        .tooltip(components::label_tooltip("Show Projects", theme))
                        .cursor_pointer()
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(AnalysisViewBarEvent::ShowProjectSidebar);
                        }))
                        .child(components::icon(IconName::PanelLeft, theme)),
                    )
            }))
            .child(
                div()
                    .id("analysis-view-tabs")
                    .debug_selector(|| "analysis-view-tabs".to_owned())
                    .h_full()
                    .flex_1()
                    .flex()
                    .overflow_x_scroll()
                    .border_r_1()
                    .border_color(theme.colors.border)
                    .when(!sidebar_visible, |tabs| tabs.border_l_1())
                    .children(views.into_iter().enumerate().map(|(index, view)| {
                        let selected = view.view_id == active_view_id;
                        let activate_id = view.view_id.clone();
                        let close_id = view.view_id.clone();
                        let menu_id = view.view_id.clone();
                        let editing = renaming_view.as_ref() == Some(&view.view_id);
                        let focus = view_name_focus.clone();
                        let menu_open = view_menu.as_ref() == Some(&view.view_id);
                        let hover_group = SharedString::from(format!("view-tab-{index}"));
                        let mut tab = components::analysis_tab(
                            SharedString::from(format!("analysis-tab:{}", view.view_id)),
                            theme,
                            selected,
                        )
                        .group(hover_group.clone())
                        .relative()
                        .flex_none()
                        .min_w(px(112.))
                        .pr_1()
                        .debug_selector(move || {
                            if selected {
                                "analysis-tab".to_owned()
                            } else {
                                format!("analysis-tab-{index}")
                            }
                        })
                        .tab_index(0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu = None;
                            this.activate_analysis_view(&activate_id, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _, _, cx| {
                                cx.emit(AnalysisViewBarEvent::DismissOtherPopovers);
                                this.menu = Some(menu_id.clone());
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .child(if editing {
                            div()
                                .id(SharedString::from(format!("rename-view:{}", view.view_id)))
                                .debug_selector(|| "rename-view-input".to_owned())
                                .track_focus(&focus)
                                .relative()
                                .flex_1()
                                .min_w(px(0.))
                                .px_1()
                                .overflow_hidden()
                                .flex()
                                .items_center()
                                .cursor_text()
                                .on_key_down(cx.listener(Self::on_view_name_key))
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.finish_rename_analysis_view(true, cx);
                                }))
                                .children(view_name_select_all.then(|| {
                                    div()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .rounded(px(2.))
                                        .bg(theme.colors.element_active)
                                        .debug_selector(|| "rename-view-selection".to_owned())
                                        .child(view_name_draft.clone())
                                }))
                                .children((!view_name_select_all).then(|| {
                                    div()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .debug_selector(|| "rename-view-prefix".to_owned())
                                        .child(view_name_prefix.clone())
                                }))
                                .children(view_name_cursor_visible.then(|| {
                                    div()
                                        .id("rename-view-caret")
                                        .debug_selector(|| "rename-view-caret".to_owned())
                                        .ml(px(1.))
                                        .w(px(1.))
                                        .h(px(14.))
                                        .flex_none()
                                        .bg(theme.colors.text)
                                }))
                                .children((!view_name_select_all).then(|| {
                                    div()
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .debug_selector(|| "rename-view-suffix".to_owned())
                                        .child(view_name_suffix.clone())
                                }))
                                .child(TextInput::cursor_target(
                                    name_input.clone(),
                                    focus.clone(),
                                    px(4.),
                                ))
                        } else {
                            div()
                                .id(SharedString::from(format!("view-name:{}", view.view_id)))
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
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
                            tab = tab.child(
                                div().absolute().top_0().left_0().child(deferred(
                                    anchored()
                                        .anchor(Corner::TopLeft)
                                        .snap_to_window_with_margin(px(8.))
                                        .offset(point(px(0.), theme.spacing.tab_height + px(4.)))
                                        .child(self.render_view_menu(view.view_id, theme, cx)),
                                )),
                            );
                        }
                        tab
                    })),
            )
            .child(
                div()
                    .id("analysis-right-controls")
                    .debug_selector(|| "analysis-right-controls".to_owned())
                    .h_full()
                    .flex_none()
                    .px_1()
                    .gap_1()
                    .flex()
                    .items_center()
                    .child(
                        components::top_bar_icon_button("new-view", theme, false, false)
                            .debug_selector(|| "new-view".to_owned())
                            .tooltip(components::label_tooltip("New View", theme))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.create_analysis_view(cx)))
                            .child(components::icon(IconName::Plus, theme)),
                    )
                    .child(
                        components::top_bar_icon_button(
                            "toggle-bottom-inspector",
                            theme,
                            inspector_visible,
                            !can_toggle_inspector,
                        )
                        .debug_selector(|| "toggle-bottom-inspector".to_owned())
                        .tooltip(components::label_tooltip(
                            if inspector_visible {
                                "Hide bottom inspector"
                            } else {
                                "Show bottom inspector"
                            },
                            theme,
                        ))
                        .when(can_toggle_inspector, |button| {
                            button.cursor_pointer().on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(AnalysisViewBarEvent::ToggleBottomInspector);
                            }))
                        })
                        .child(components::icon(
                            if inspector_visible {
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
                                button.cursor_pointer().on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(AnalysisViewBarEvent::RefreshSources);
                                }))
                            })
                            .child(components::icon(IconName::Refresh, theme)),
                    ),
            )
    }
}

impl AnalysisViewBar {
    fn render_view_menu(
        &mut self,
        view_id: AnalysisViewId,
        theme: ViewerTheme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let duplicate_id = view_id.clone();
        let rename_id = view_id.clone();
        let close_id = view_id.clone();
        components::popover(theme)
            .id(SharedString::from(format!("view-menu:{view_id}")))
            .debug_selector(|| "view-menu".to_owned())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.menu.take().is_some() {
                    cx.notify();
                }
            }))
            .w(px(180.))
            .p_1()
            .flex()
            .flex_col()
            .text_xs()
            .child(
                components::popover_menu_item(
                    "duplicate-view",
                    "Duplicate View",
                    Some(IconName::Duplicate),
                    theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_analysis_view(&duplicate_id, cx);
                    this.duplicate_analysis_view(cx);
                    this.menu = None;
                    cx.notify();
                    cx.stop_propagation();
                }))
                .debug_selector(|| "duplicate-view".to_owned()),
            )
            .child(
                components::popover_menu_item(
                    "rename-view",
                    "Rename View",
                    Some(IconName::Edit),
                    theme,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.menu = None;
                    this.begin_rename_analysis_view(rename_id.clone(), window, cx);
                    cx.stop_propagation();
                }))
                .debug_selector(|| "rename-view".to_owned()),
            )
            .child(
                components::popover_menu_item(
                    "close-view",
                    "Close View",
                    Some(IconName::Close),
                    theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_analysis_view(&close_id, cx);
                    this.menu = None;
                    cx.notify();
                    cx.stop_propagation();
                })),
            )
    }

    fn create_analysis_view(&mut self, cx: &mut Context<Self>) {
        self.renaming_view = None;
        cx.emit(AnalysisViewBarEvent::Command(WorkbenchCommand::CreateView));
        cx.notify();
    }

    fn duplicate_analysis_view(&mut self, cx: &mut Context<Self>) {
        self.renaming_view = None;
        cx.emit(AnalysisViewBarEvent::Command(
            WorkbenchCommand::DuplicateActiveView,
        ));
        cx.notify();
    }

    fn activate_analysis_view(&mut self, view_id: &AnalysisViewId, cx: &mut Context<Self>) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.views.active().view_id == *view_id)
        {
            return;
        }
        self.renaming_view = None;
        cx.emit(AnalysisViewBarEvent::Command(
            WorkbenchCommand::ActivateView(view_id.clone()),
        ));
        cx.notify();
    }

    fn close_analysis_view(&mut self, view_id: &AnalysisViewId, cx: &mut Context<Self>) {
        self.renaming_view = None;
        cx.emit(AnalysisViewBarEvent::Command(WorkbenchCommand::CloseView(
            view_id.clone(),
        )));
        cx.notify();
    }

    fn begin_rename_analysis_view(
        &mut self,
        view_id: AnalysisViewId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view_name) = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .views
                .views()
                .iter()
                .find(|view| view.view_id == view_id)
                .map(|view| view.name.clone())
        }) else {
            return;
        };
        self.renaming_view = Some(view_id);
        let view_name_input = self.name_input.clone();
        let focus = self.rename_focus.clone();
        view_name_input.update(cx, |input, cx| {
            input.set_text(view_name);
            input.select_all();
            input.start_blink(cx);
        });
        window.defer(cx, move |window, _| focus.focus(window));
        cx.notify();
    }

    fn on_view_name_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let name_input = self.name_input.clone();
        if event.keystroke.key == "enter" {
            self.finish_rename_analysis_view(true, cx);
        } else if event.keystroke.key == "escape" {
            self.finish_rename_analysis_view(false, cx);
        } else if !name_input.update(cx, |input, cx| {
            let handled = input.edit(event).handled;
            if handled {
                input.start_blink(cx);
            }
            handled
        }) {
            return;
        }
        cx.stop_propagation();
    }

    pub(crate) fn finish_rename_analysis_view(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(view_id) = self.renaming_view.take() else {
            return;
        };
        if commit {
            cx.emit(AnalysisViewBarEvent::Command(
                WorkbenchCommand::RenameView {
                    view_id,
                    name: self.name_input.read(cx).text().to_owned(),
                },
            ));
        }
        self.name_input.update(cx, |input, cx| input.stop_blink(cx));
        cx.notify();
    }
}

impl ViewerApp {
    pub(super) fn sync_active_view_workspace(&mut self, cx: &mut Context<Self>) {
        let panel_count = self.session_snapshot(cx).views.active().panels.len();
        self.workspace.update(cx, |workspace, cx| {
            workspace.overview_chart = cx.new(|_| super::chart::OverviewChart::default());
            workspace.track_charts.clear();
            workspace.track_hovers.clear();
            workspace.metric_scroll = ListState::new(panel_count, ListAlignment::Top, px(480.));
            *workspace.track_viewport.borrow_mut() = TrackViewport::default();
            workspace.drag = None;
            workspace.detail_refresh_token = workspace.detail_refresh_token.saturating_add(1);
            workspace.detail_refresh_pending = false;
            workspace.zoom_task = None;
            cx.notify();
        });
        self.interaction.update(cx, |interaction, cx| {
            if interaction.clear_cursors() {
                cx.notify();
            }
        });
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(super) fn view_bar_menu_is_open(&self, cx: &App) -> bool {
        self.analysis_view_bar.read(cx).menu.is_some()
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(super) fn view_bar_is_renaming(&self, cx: &App) -> bool {
        self.analysis_view_bar.read(cx).renaming_view.is_some()
    }

    pub(super) fn handle_view_bar_event(
        &mut self,
        event: &AnalysisViewBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AnalysisViewBarEvent::Command(command) => {
                self.dispatch_workbench_command(command.clone(), cx);
            }
            AnalysisViewBarEvent::DismissOtherPopovers => {
                self.project_sidebar.update(cx, |sidebar, cx| {
                    if sidebar.menu.take().is_some() {
                        cx.notify();
                    }
                });
                self.workspace.update(cx, |workspace, cx| {
                    let changed = workspace.metric_picker_open || workspace.axis_picker_open;
                    workspace.metric_picker_open = false;
                    workspace.axis_picker_open = false;
                    if changed {
                        cx.notify();
                    }
                });
            }
            AnalysisViewBarEvent::ShowProjectSidebar => {
                self.project_sidebar.update(cx, |sidebar, cx| {
                    sidebar.visible = true;
                    cx.notify();
                });
            }
            AnalysisViewBarEvent::ToggleBottomInspector => {
                self.on_toggle_bottom_inspector(&crate::desktop::ToggleBottomInspector, window, cx);
            }
            AnalysisViewBarEvent::RefreshSources => {
                self.on_refresh(&crate::desktop::Refresh, window, cx);
            }
        }
    }
}
