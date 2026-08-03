use crate::domain::DataSourceId;
use crate::workbench::ProjectRef;
use gpui::Pixels;
use seex::Project;
use seex::{Run, RunStatus};

#[derive(Clone, Copy, Debug)]
pub(crate) struct SidebarResize {
    pub start_x: Pixels,
    pub start_width: Pixels,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectPlacement {
    Pinned,
    Projects,
    Archived,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunPlacement {
    Baseline,
    Pinned,
    Projects,
    Archived,
}

#[derive(Clone)]
pub(crate) struct SidebarProject {
    pub source_index: usize,
    pub project_index: usize,
    pub project_ref: ProjectRef,
    pub project: Project,
    pub runs: Vec<Run>,
    pub source_label: String,
    pub placement: ProjectPlacement,
}

pub(crate) fn project_tree_label(name: &str, source_id: &DataSourceId, duplicate: bool) -> String {
    if duplicate {
        format!("{name} — {source_id}")
    } else {
        name.to_owned()
    }
}

pub(crate) const fn run_status(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Finished => "finished",
        RunStatus::Failed => "failed",
    }
}

use gpui::{
    Context, Corner, MouseButton, SharedString, Window, anchored, deferred, div, point, prelude::*,
    px,
};

use crate::desktop::{ActivateSelection, SELECTABLE_CONTEXT};
use crate::domain::{MAX_SELECTED_RUNS, RunRef, run_matches_filter};

use super::super::chart;
use super::super::components::{self, IconName};
use super::{
    PROJECT_TEXT_OFFSET, ProjectSidebar, ProjectSidebarEvent, RUN_COLOR_MARKER_SIZE, RUN_PAGE_SIZE,
    project_information_card, sidebar_menu_item, sidebar_text_button,
};

impl ProjectSidebar {
    pub(super) fn render_sidebar_project(
        &mut self,
        project: SidebarProject,
        query: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = super::ViewerTheme::for_appearance(window.appearance());
        let project_ref = project.project_ref.clone();
        let expanded_key = (
            project.project_ref.source_id.clone(),
            project.project_ref.project_id.clone(),
        );
        let expanded = self.expanded_projects.contains(&expanded_key);
        let menu_open = self.menu.as_ref() == Some(&project_ref);
        let hovered = self.hovered_project.as_ref() == Some(&project_ref);
        let project_focus = self
            .project_focuses
            .entry(project_ref.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let limit = self
            .project_run_limits
            .get(&project_ref)
            .copied()
            .unwrap_or(RUN_PAGE_SIZE);
        let sidebar_width = self.width;
        let focused = project_focus.contains_focused(window, cx);
        let project_matches = query.is_empty()
            || project.project.name.to_lowercase().contains(query)
            || project
                .project
                .project_id
                .as_str()
                .to_lowercase()
                .contains(query)
            || project.source_label.to_lowercase().contains(query);
        let Some(session) = self.snapshot.clone() else {
            return div();
        };
        let baseline = session.views.active().baseline.clone();
        let pinned = session.views.active().pinned_runs.clone();
        let archived = session.views.archived_runs().to_vec();
        let mut runs = project
            .runs
            .iter()
            .filter(|run| {
                let run_ref = RunRef::new(
                    project.project_ref.source_id.clone(),
                    run.project_id.clone(),
                    run.run_id.clone(),
                );
                baseline.as_ref() != Some(&run_ref)
                    && !pinned.contains(&run_ref)
                    && !archived.contains(&run_ref)
            })
            .cloned()
            .collect::<Vec<_>>();
        let filtering_runs = !query.is_empty() && !project_matches;
        if filtering_runs {
            runs.retain(|run| run_matches_filter(run, query));
        }
        if !project_matches && runs.is_empty() {
            return div();
        }
        let visible_runs = if filtering_runs {
            runs.clone()
        } else {
            runs.iter().take(limit).cloned().collect::<Vec<_>>()
        };
        let row_source = project.project_ref.source_id.clone();
        let row_project = project.project_ref.project_id.clone();
        let keyboard_source = project.project_ref.source_id.clone();
        let keyboard_project = project.project_ref.project_id.clone();
        let hover_ref = project_ref.clone();
        let menu_ref = project_ref.clone();
        let keyboard_menu_ref = project_ref.clone();
        let row_id = format!(
            "project:{}:{}",
            project_ref.source_id,
            project_ref.project_id.as_str()
        );
        let duplicate_name = session
            .sources
            .iter()
            .flat_map(|source| &source.catalog.projects)
            .filter(|candidate| candidate.name.eq_ignore_ascii_case(&project.project.name))
            .count()
            > 1;
        let project_label = project_tree_label(
            &project.project.name,
            &project.project_ref.source_id,
            duplicate_name,
        );
        let hover_group = SharedString::from(format!("project-hover:{row_id}"));

        let selected = false;
        let mut row =
            components::sidebar_tree_row(SharedString::from(row_id), theme, selected, false)
                .relative()
                .debug_selector({
                    let source_index = project.source_index;
                    let project_index = project.project_index;
                    move || format!("project-tree-row-{source_index}-{project_index}")
                })
                .key_context(SELECTABLE_CONTEXT)
                .track_focus(&project_focus)
                .tab_index(0)
                .group(hover_group.clone())
                .gap_1()
                .text_xs()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.activate_tree_project(row_source.clone(), row_project.clone(), cx);
                }))
                .on_action(cx.listener(move |this, _: &ActivateSelection, _, cx| {
                    this.activate_tree_project(
                        keyboard_source.clone(),
                        keyboard_project.clone(),
                        cx,
                    );
                }))
                .on_hover(cx.listener(move |this, is_hovered, _, cx| {
                    this.set_hovered_project(&hover_ref, *is_hovered, cx);
                }))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "project-folder:{}",
                            project_ref.project_id.as_str()
                        )))
                        .debug_selector({
                            let source_index = project.source_index;
                            let project_index = project.project_index;
                            move || format!("project-folder-{source_index}-{project_index}")
                        })
                        .size(px(20.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(components::icon(
                            if expanded {
                                IconName::FolderOpen
                            } else {
                                IconName::Folder
                            },
                            theme,
                        )),
                )
                .child(
                    div()
                        .debug_selector({
                            let source_index = project.source_index;
                            let project_index = project.project_index;
                            move || format!("project-tree-label-{source_index}-{project_index}")
                        })
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(project_label),
                )
                .child({
                    components::sidebar_hover_icon_button(
                        SharedString::from(format!(
                            "project-menu:{}",
                            project_ref.project_id.as_str()
                        )),
                        theme,
                        menu_open,
                        hover_group,
                        hovered || focused || menu_open,
                    )
                    .tooltip(components::label_tooltip("Project actions", theme))
                    .debug_selector({
                        let project_ref = project_ref.clone();
                        move || format!("project-menu-{}", project_ref.project_id.as_str())
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.emit(ProjectSidebarEvent::DismissOtherPopovers);
                            this.menu = (!menu_open).then(|| menu_ref.clone());
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, event, _, cx| {
                        if !matches!(event, gpui::ClickEvent::Keyboard(_)) {
                            return;
                        }
                        cx.emit(ProjectSidebarEvent::DismissOtherPopovers);
                        this.menu = (!menu_open).then(|| keyboard_menu_ref.clone());
                        cx.notify();
                    }))
                    .child(components::icon(IconName::Ellipsis, theme))
                });

        if menu_open {
            row = row.child(
                div().absolute().top_0().left_0().child(deferred(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .snap_to_window_with_margin(px(8.))
                        .offset(point(sidebar_width - px(12.), px(12.)))
                        .child(self.render_project_menu(project.clone(), theme, cx)),
                )),
            );
        } else if hovered {
            row = row.child(
                div().absolute().top_0().left_0().child(deferred(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .snap_to_window_with_margin(px(8.))
                        .offset(point(sidebar_width - px(12.), px(-8.)))
                        .child(project_information_card(&project, theme)),
                )),
            );
        }
        let mut tree = div().relative().child(row);

        if expanded {
            if visible_runs.is_empty() {
                tree = tree.child(
                    div()
                        .debug_selector(|| "project-no-runs".to_owned())
                        .h(theme.spacing.tree_row_height)
                        .ml(px(PROJECT_TEXT_OFFSET))
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_color(theme.colors.text_muted)
                        .child("No runs"),
                );
            } else {
                for (index, run) in visible_runs.into_iter().enumerate() {
                    let run_ref = RunRef::new(
                        project_ref.source_id.clone(),
                        run.project_id.clone(),
                        run.run_id.clone(),
                    );
                    let source_index = project.source_index;
                    let project_index = project.project_index;
                    tree = tree.child(
                        self.render_sidebar_run(run_ref, RunPlacement::Projects, index, window, cx)
                            .debug_selector(move || {
                                format!("project-tree-run-{source_index}-{project_index}-{index}")
                            }),
                    );
                }
            }
            if !filtering_runs && runs.len() > limit {
                let more_ref = project_ref.clone();
                tree = tree.child(
                    sidebar_text_button(
                        SharedString::from(format!(
                            "show-more:{}",
                            project_ref.project_id.as_str()
                        )),
                        "Show more",
                        theme,
                    )
                    .ml(px(PROJECT_TEXT_OFFSET))
                    .debug_selector({
                        let source_index = project.source_index;
                        let project_index = project.project_index;
                        move || format!("show-more-{source_index}-{project_index}")
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        *this
                            .project_run_limits
                            .entry(more_ref.clone())
                            .or_insert(RUN_PAGE_SIZE) += RUN_PAGE_SIZE;
                        cx.notify();
                    })),
                );
            }
        }
        tree
    }
    pub(super) fn render_sidebar_run(
        &mut self,
        run_ref: RunRef,
        placement: RunPlacement,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = super::ViewerTheme::for_appearance(window.appearance());
        let selected = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.views.active().runs.contains(&run_ref));
        let hovered = self.interaction.emphasized_run.as_ref() == Some(&run_ref);
        let run_focus = self
            .run_focuses
            .entry(run_ref.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let focused = run_focus.contains_focused(window, cx);
        let active = hovered || focused;
        let (name, status) = self.sidebar_run(&run_ref).map_or_else(
            || ("Unavailable".to_owned(), "Unavailable".to_owned()),
            |run| (run.name, run_status(run.status).to_owned()),
        );
        let hover_run = run_ref.clone();
        let row_run = run_ref.clone();
        let eye_run = run_ref.clone();
        let baseline_run = run_ref.clone();
        let pin_run = run_ref.clone();
        let archive_run = run_ref.clone();
        let is_baseline = placement == RunPlacement::Baseline;
        let is_pinned = placement == RunPlacement::Pinned;
        let is_archived = placement == RunPlacement::Archived;
        let (placement_selector, name_selector) = match placement {
            RunPlacement::Baseline => ("baseline", "baseline-run-name"),
            RunPlacement::Pinned => ("pinned", "pinned-run-name"),
            RunPlacement::Projects => ("project", "project-run-name"),
            RunPlacement::Archived => ("archived", "archived-run-name"),
        };
        let can_make_visible = self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.has_capacity_for_runs(std::slice::from_ref(&run_ref)));
        let row_disabled = placement == RunPlacement::Projects && !selected && !can_make_visible;
        let baseline_disabled = !is_baseline && (!selected || is_archived) && !can_make_visible;
        let pin_disabled = !is_pinned && (!selected || is_archived) && !can_make_visible;
        let archive_disabled = is_archived && !can_make_visible;
        let limit_tooltip =
            format!("{MAX_SELECTED_RUNS}/{MAX_SELECTED_RUNS} Runs selected in this View");
        let run_color = theme
            .colors
            .series_color(chart::series_color_index(&run_ref));
        let hover_group = SharedString::from(format!("run-hover:{}", run_ref.cache_key()));
        let hover_region = SharedString::from(format!(
            "sidebar-run:{placement_selector}:{index}:{}",
            run_ref.cache_key()
        ));

        components::sidebar_tree_row(
            SharedString::from(format!("run:{}:{index}", run_ref.cache_key())),
            theme,
            false,
            row_disabled,
        )
        .debug_selector({
            let run_ref = run_ref.clone();
            move || format!("project-tree-run-{}", run_ref.run_id.as_str())
        })
        .track_focus(&run_focus)
        .tab_index(if row_disabled { -1 } else { 0 })
        .group(hover_group.clone())
        .gap_1()
        .text_xs()
        .when(
            placement == RunPlacement::Projects && !row_disabled,
            |row| {
                row.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_tree_run(row_run.clone(), cx);
                    }))
            },
        )
        .on_hover(cx.listener(move |_, is_hovered: &bool, _, cx| {
            cx.emit(ProjectSidebarEvent::HoveredRun {
                run: hover_run.clone(),
                region: hover_region.clone(),
                hovered: *is_hovered,
            });
        }))
        .children((placement == RunPlacement::Projects).then(|| {
            components::sidebar_icon_button(
                SharedString::from(format!("run-eye:{}", eye_run.cache_key())),
                theme,
                selected,
            )
            .debug_selector(move || format!("run-eye-{index}"))
            .size(px(20.))
            .tooltip(components::label_tooltip(
                if row_disabled {
                    limit_tooltip.clone()
                } else if selected {
                    "Hide Run".to_owned()
                } else {
                    "Show Run".to_owned()
                },
                theme,
            ))
            .when(!row_disabled, |button| {
                button.on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_tree_run(eye_run.clone(), cx);
                    cx.stop_propagation();
                }))
            })
            .when(row_disabled, |button| {
                button.opacity(0.35).cursor_default().tab_index(-1)
            })
            .child(components::icon(
                if selected {
                    IconName::Eye
                } else {
                    IconName::EyeClosed
                },
                theme,
            ))
        }))
        .child(
            div()
                .debug_selector(move || format!("run-color-{placement_selector}-{index}"))
                .size(px(RUN_COLOR_MARKER_SIZE))
                .flex_none()
                .mt(px(1.))
                .rounded(px(RUN_COLOR_MARKER_SIZE / 2.))
                .bg(run_color),
        )
        .child(
            div()
                .debug_selector(move || format!("{name_selector}-{index}"))
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(name),
        )
        .child(
            div()
                .w(px(84.))
                .h_full()
                .flex_none()
                .relative()
                .child(
                    div()
                        .debug_selector(move || format!("run-status-{index}"))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_xs()
                        .text_color(theme.colors.text_muted)
                        .group_hover(hover_group.clone(), |style| style.invisible())
                        .when(active, |status| status.invisible())
                        .child(status),
                )
                .child(
                    div()
                        .debug_selector(move || format!("run-actions-{index}"))
                        .absolute()
                        .top_0()
                        .right_0()
                        .flex()
                        .invisible()
                        .group_hover(hover_group, |style| style.visible())
                        .when(active, |actions| actions.visible())
                        .child(
                            components::sidebar_icon_button(
                                SharedString::from(format!(
                                    "run-baseline:{}",
                                    baseline_run.cache_key()
                                )),
                                theme,
                                is_baseline,
                            )
                            .tooltip(components::label_tooltip(
                                if baseline_disabled {
                                    limit_tooltip.clone()
                                } else if is_baseline {
                                    "Clear Baseline".to_owned()
                                } else {
                                    "Set Baseline".to_owned()
                                },
                                theme,
                            ))
                            .when(!baseline_disabled, |button| {
                                button.on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_run_baseline(baseline_run.clone(), cx);
                                    cx.stop_propagation();
                                }))
                            })
                            .when(baseline_disabled, |button| {
                                button.opacity(0.35).cursor_default().tab_index(-1)
                            })
                            .child(components::icon(IconName::Baseline, theme)),
                        )
                        .child(
                            components::sidebar_icon_button(
                                SharedString::from(format!("run-pin:{}", pin_run.cache_key())),
                                theme,
                                is_pinned,
                            )
                            .tooltip(components::label_tooltip(
                                if pin_disabled {
                                    limit_tooltip.clone()
                                } else if is_pinned {
                                    "Unpin Run".to_owned()
                                } else {
                                    "Pin Run".to_owned()
                                },
                                theme,
                            ))
                            .when(!pin_disabled, |button| {
                                button.on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_pinned_run(pin_run.clone(), cx);
                                    cx.stop_propagation();
                                }))
                            })
                            .when(pin_disabled, |button| {
                                button.opacity(0.35).cursor_default().tab_index(-1)
                            })
                            .child(components::icon(IconName::Pin, theme)),
                        )
                        .child(
                            components::sidebar_icon_button(
                                SharedString::from(format!(
                                    "run-archive:{}",
                                    archive_run.cache_key()
                                )),
                                theme,
                                is_archived,
                            )
                            .tooltip(components::label_tooltip(
                                if archive_disabled {
                                    limit_tooltip.clone()
                                } else if is_archived {
                                    "Restore Run".to_owned()
                                } else {
                                    "Archive Run".to_owned()
                                },
                                theme,
                            ))
                            .when(!archive_disabled, |button| {
                                button.on_click(cx.listener(move |this, _, _, cx| {
                                    this.archive_run(archive_run.clone(), cx);
                                    cx.stop_propagation();
                                }))
                            })
                            .when(archive_disabled, |button| {
                                button.opacity(0.35).cursor_default().tab_index(-1)
                            })
                            .child(components::icon(IconName::Archive, theme)),
                        ),
                ),
        )
    }
    pub(super) fn render_project_menu(
        &mut self,
        project: SidebarProject,
        theme: super::ViewerTheme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let snapshot = self.snapshot.clone();
        let selected = snapshot
            .as_ref()
            .map(|snapshot| snapshot.views.active().runs.clone())
            .unwrap_or_default();
        let project_runs = self.project_listing_runs(&project);
        let selected_count = project_runs
            .iter()
            .filter(|run| selected.contains(run))
            .count();
        let at_limit = snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.active_selection_has_capacity());
        let hide_all =
            at_limit || (selected_count == project_runs.len() && !project_runs.is_empty());
        let can_toggle = hide_all
            || snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.has_capacity_for_runs(&project_runs));
        let visibility_icon = if hide_all {
            IconName::Eye
        } else if selected_count == 0 {
            IconName::EyeClosed
        } else {
            IconName::EyeOff
        };
        let visibility_label = if hide_all {
            "Hide all Runs"
        } else {
            "Show all Runs"
        };
        let toggle_project = project.clone();
        let placement_ref = project.project_ref.clone();
        let archive_ref = project.project_ref.clone();
        let remove_ref = project.project_ref.clone();

        components::popover(theme)
            .id(SharedString::from(format!(
                "project-popover:{}",
                project.project_ref.project_id.as_str()
            )))
            .debug_selector({
                let project_ref = project.project_ref.clone();
                move || format!("project-popover-{}", project_ref.project_id.as_str())
            })
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.menu.take().is_some() {
                    cx.notify();
                }
            }))
            .w(px(210.))
            .p_1()
            .flex()
            .flex_col()
            .text_xs()
            .child(
                sidebar_menu_item(
                    "project-visibility",
                    visibility_label,
                    visibility_icon,
                    theme,
                )
                .tooltip(components::label_tooltip(
                    if can_toggle {
                        visibility_label.to_owned()
                    } else {
                        format!(
                            "{MAX_SELECTED_RUNS}/{MAX_SELECTED_RUNS} Runs selected in this View"
                        )
                    },
                    theme,
                ))
                .when(can_toggle, |item| {
                    item.on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_project_runs(&toggle_project, cx);
                        cx.stop_propagation();
                    }))
                })
                .when(!can_toggle, |item| {
                    item.opacity(0.35).cursor_default().tab_index(-1)
                })
                .debug_selector(move || {
                    if hide_all {
                        "hide-all-project-runs".to_owned()
                    } else {
                        "show-all-project-runs".to_owned()
                    }
                }),
            )
            .child(
                div()
                    .id("project-menu-separator")
                    .debug_selector(|| "project-menu-separator".to_owned())
                    .h(px(1.))
                    .my_1()
                    .bg(theme.colors.border),
            )
            .child(match project.placement {
                ProjectPlacement::Pinned => {
                    sidebar_menu_item("unpin-project", "Unpin project", IconName::Pin, theme)
                        .debug_selector(|| "unpin-project".to_owned())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_project_placement(
                                placement_ref.clone(),
                                ProjectPlacement::Projects,
                                cx,
                            );
                            cx.stop_propagation();
                            cx.notify();
                        }))
                }
                ProjectPlacement::Archived => sidebar_menu_item(
                    "restore-project",
                    "Restore project",
                    IconName::Archive,
                    theme,
                )
                .debug_selector(|| "restore-project".to_owned())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_project_placement(
                        placement_ref.clone(),
                        ProjectPlacement::Projects,
                        cx,
                    );
                    cx.stop_propagation();
                    cx.notify();
                })),
                ProjectPlacement::Projects => {
                    sidebar_menu_item("pin-project", "Pin project", IconName::Pin, theme)
                        .debug_selector(|| "pin-project".to_owned())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_project_placement(
                                placement_ref.clone(),
                                ProjectPlacement::Pinned,
                                cx,
                            );
                            cx.stop_propagation();
                            cx.notify();
                        }))
                }
            })
            .children((project.placement != ProjectPlacement::Archived).then(|| {
                sidebar_menu_item(
                    "archive-project",
                    "Archive project",
                    IconName::Archive,
                    theme,
                )
                .debug_selector(|| "archive-project".to_owned())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_project_placement(archive_ref.clone(), ProjectPlacement::Archived, cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
            }))
            .child(
                sidebar_menu_item("remove-project", "Remove project", IconName::Close, theme)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remove_project(remove_ref.clone(), cx);
                        cx.stop_propagation();
                    })),
            )
    }
}
