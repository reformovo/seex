use super::*;

const RUN_PAGE_SIZE: usize = 5;

impl ViewerApp {
    fn sidebar_projects(&self) -> Vec<SidebarProject> {
        let mut projects = Vec::new();
        for (source_index, source) in self.sources.sources().enumerate() {
            let source_label = source
                .root_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(source.source_id.as_str())
                .to_owned();
            for (project_index, project) in source.catalog.projects.iter().enumerate() {
                let project_ref =
                    ProjectRef::new(source.source_id.clone(), project.project_id.clone());
                if self.views.removed_projects().contains(&project_ref) {
                    continue;
                }
                let placement = if self.views.archived_projects().contains(&project_ref) {
                    ProjectPlacement::Archived
                } else if self.views.pinned_projects().contains(&project_ref) {
                    ProjectPlacement::Pinned
                } else {
                    ProjectPlacement::Projects
                };
                projects.push(SidebarProject {
                    source_index,
                    project_index,
                    project_ref,
                    project: project.clone(),
                    runs: source
                        .catalog
                        .runs
                        .iter()
                        .filter(|run| run.project_id == project.project_id)
                        .cloned()
                        .collect(),
                    source_label: source_label.clone(),
                    placement,
                });
            }
        }
        projects
    }

    fn sidebar_run(&self, run_ref: &RunRef) -> Option<Run> {
        let source = self.sources.source(&run_ref.source_id)?;
        source
            .catalog
            .runs
            .iter()
            .find(|run| run.project_id == run_ref.project_id && run.run_id == run_ref.run_id)
            .cloned()
    }

    pub(super) fn render_converged_project_sidebar(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let projects = self.sidebar_projects();
        let baseline = self.views.active().baseline.clone();
        let pinned_runs = self.views.active().pinned_runs.clone();
        let archived_runs = self.views.archived_runs().to_vec();
        let active_view = self.views.active().view_id.clone();
        let pinned_limit = self
            .pinned_run_limits
            .get(&active_view)
            .copied()
            .unwrap_or(RUN_PAGE_SIZE);
        let archived_limit = self.archived_run_limit;
        let query = self.run_filter.trim().to_lowercase();
        let (filter_prefix, filter_suffix) = self.run_filter.split_at(self.run_filter_cursor);
        let filter_prefix = filter_prefix.to_owned();
        let filter_suffix = filter_suffix.to_owned();
        let filter_focus = self.filter_focus.clone();
        let click_filter_focus = filter_focus.clone();
        let filter_focused = filter_focus.is_focused(window);

        let mut resources = div()
            .id("project-run-tree")
            .debug_selector(|| "project-run-tree".to_owned())
            .flex_1()
            .overflow_y_scroll()
            .px(theme.spacing.panel_padding)
            .pt_2()
            .pb_3();

        resources = resources.child(sidebar_group_label("Baseline", theme));
        if let Some(run) = baseline {
            resources = resources.child(self.render_sidebar_run(
                run,
                RunPlacement::Baseline,
                0,
                window,
                cx,
            ));
        }

        resources = resources.child(sidebar_group_label("Pinned", theme));
        for project in projects
            .iter()
            .filter(|project| project.placement == ProjectPlacement::Pinned)
            .cloned()
        {
            resources = resources.child(self.render_sidebar_project(project, &query, window, cx));
        }
        for (index, run) in pinned_runs.iter().take(pinned_limit).cloned().enumerate() {
            resources = resources.child(self.render_sidebar_run(
                run,
                RunPlacement::Pinned,
                index,
                window,
                cx,
            ));
        }
        if pinned_runs.len() > pinned_limit {
            let view_id = active_view.clone();
            resources = resources.child(
                sidebar_text_button("show-more-pinned", "Show more", theme)
                    .debug_selector(|| "show-more-pinned".to_owned())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        *this
                            .pinned_run_limits
                            .entry(view_id.clone())
                            .or_insert(RUN_PAGE_SIZE) += RUN_PAGE_SIZE;
                        cx.notify();
                    })),
            );
        }

        resources = resources.child(sidebar_group_label("Projects", theme));
        for project in projects
            .iter()
            .filter(|project| project.placement == ProjectPlacement::Projects)
            .cloned()
        {
            resources = resources.child(self.render_sidebar_project(project, &query, window, cx));
        }

        resources = resources.child(sidebar_group_label("Archived", theme));
        for project in projects
            .into_iter()
            .filter(|project| project.placement == ProjectPlacement::Archived)
        {
            resources = resources.child(self.render_sidebar_project(project, &query, window, cx));
        }
        for (index, run) in archived_runs
            .iter()
            .take(archived_limit)
            .cloned()
            .enumerate()
        {
            resources = resources.child(self.render_sidebar_run(
                run,
                RunPlacement::Archived,
                index,
                window,
                cx,
            ));
        }
        if archived_runs.len() > archived_limit {
            resources = resources.child(
                sidebar_text_button("show-more-archived", "Show more", theme)
                    .debug_selector(|| "show-more-archived".to_owned())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.archived_run_limit += RUN_PAGE_SIZE;
                        cx.notify();
                    })),
            );
        }

        div()
            .id("project-sidebar")
            .debug_selector(|| "project-sidebar".to_owned())
            .w(self.project_sidebar_width)
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .text_xs()
            .bg(theme.colors.panel)
            .border_r_1()
            .border_color(theme.colors.border)
            .child(
                div()
                    .id("project-sidebar-header")
                    .debug_selector(|| "project-sidebar-header".to_owned())
                    .h(theme.spacing.tab_height)
                    .flex_none()
                    .px(theme.spacing.panel_padding)
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("PulseOn"),
                    )
                    .child(
                        div()
                            .id("project-header-controls")
                            .debug_selector(|| "project-header-controls".to_owned())
                            .h_full()
                            .flex_none()
                            .pl_1()
                            .gap_1()
                            .flex()
                            .items_center()
                            .child(
                                components::top_bar_icon_button(
                                    "import-source",
                                    theme,
                                    false,
                                    false,
                                )
                                .debug_selector(|| "import-source".to_owned())
                                .tooltip(components::label_tooltip("Import Source", theme))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.open_picker(cx)))
                                .child(components::icon(IconName::Plus, theme)),
                            )
                            .child(
                                components::top_bar_icon_button(
                                    "hide-project-sidebar",
                                    theme,
                                    false,
                                    false,
                                )
                                .tooltip(components::label_tooltip("Hide Projects", theme))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.project_sidebar_visible = false;
                                    this.project_menu = None;
                                    cx.notify();
                                }))
                                .child(components::icon(IconName::PanelLeft, theme)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("project-filter-row")
                    .debug_selector(|| "project-filter-row".to_owned())
                    .h(px(40.))
                    .flex_none()
                    .px(theme.spacing.panel_padding)
                    .py(px(6.))
                    .flex()
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .id("project-run-filter")
                            .debug_selector(|| "project-run-filter".to_owned())
                            .track_focus(&filter_focus)
                            .cursor_text()
                            .px_3()
                            .h(theme.spacing.control_height)
                            .w_full()
                            .flex()
                            .items_center()
                            .text_xs()
                            .rounded(theme.spacing.corner_radius)
                            .border_1()
                            .border_color(theme.colors.border)
                            .on_key_down(cx.listener(Self::on_filter_key))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.run_filter_cursor = this.run_filter.len();
                                click_filter_focus.focus(window);
                                this.start_filter_cursor_blink(cx);
                            }))
                            .children((!filter_focused && self.run_filter.is_empty()).then(|| {
                                div()
                                    .id("project-run-filter-placeholder")
                                    .debug_selector(|| "project-run-filter-placeholder".to_owned())
                                    .text_color(theme.colors.disabled)
                                    .child("Filter Projects and Runs")
                            }))
                            .children((!self.run_filter.is_empty()).then(|| {
                                div()
                                    .id("project-run-filter-prefix")
                                    .debug_selector(|| "project-run-filter-value".to_owned())
                                    .text_color(theme.colors.text)
                                    .child(filter_prefix)
                            }))
                            .children((filter_focused && self.filter_cursor_visible).then(|| {
                                div()
                                    .id("project-run-filter-caret")
                                    .debug_selector(|| "project-run-filter-caret".to_owned())
                                    .ml(px(1.))
                                    .w(px(1.))
                                    .h(px(14.))
                                    .bg(theme.colors.text)
                            }))
                            .children((!self.run_filter.is_empty()).then(|| {
                                div()
                                    .id("project-run-filter-suffix")
                                    .debug_selector(|| "project-run-filter-suffix".to_owned())
                                    .text_color(theme.colors.text)
                                    .child(filter_suffix)
                            })),
                    ),
            )
            .child(resources)
    }

    fn render_sidebar_project(
        &mut self,
        project: SidebarProject,
        query: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme;
        let project_ref = project.project_ref.clone();
        let expanded_key = (
            project.project_ref.source_id.clone(),
            project.project_ref.project_id.clone(),
        );
        let expanded = self.expanded_projects.contains(&expanded_key);
        let menu_open = self.project_menu.as_ref() == Some(&project_ref);
        let hovered = self.hovered_project.as_ref() == Some(&project_ref);
        let project_focus = self
            .project_focuses
            .entry(project_ref.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
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
        let baseline = self.views.active().baseline.clone();
        let pinned = self.views.active().pinned_runs.clone();
        let archived = self.views.archived_runs().to_vec();
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
            runs.retain(|run| {
                run.name.to_lowercase().contains(query)
                    || run.run_id.as_str().to_lowercase().contains(query)
            });
        }
        if !project_matches && runs.is_empty() {
            return div();
        }
        let limit = self
            .project_run_limits
            .get(&project_ref)
            .copied()
            .unwrap_or(RUN_PAGE_SIZE);
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
        let duplicate_name = self
            .sources
            .sources()
            .flat_map(|source| &source.catalog.projects)
            .filter(|candidate| candidate.name.eq_ignore_ascii_case(&project.project.name))
            .count()
            > 1;
        let project_label = project_tree_label(
            &project.project.name,
            &project.project_ref.source_id,
            duplicate_name,
        );

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
                    if *is_hovered {
                        this.hovered_project = Some(hover_ref.clone());
                    } else if this.hovered_project.as_ref() == Some(&hover_ref) {
                        this.hovered_project = None;
                    }
                    cx.notify();
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
                .children((hovered || focused || menu_open).then(|| {
                    components::sidebar_icon_button(
                        SharedString::from(format!(
                            "project-menu:{}",
                            project_ref.project_id.as_str()
                        )),
                        theme,
                        menu_open,
                    )
                    .tooltip(components::label_tooltip("Project actions", theme))
                    .debug_selector({
                        let project_ref = project_ref.clone();
                        move || format!("project-menu-{}", project_ref.project_id.as_str())
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.dismiss_popovers();
                            this.project_menu = (!menu_open).then(|| menu_ref.clone());
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, event, _, cx| {
                        if !matches!(event, gpui::ClickEvent::Keyboard(_)) {
                            return;
                        }
                        this.dismiss_popovers();
                        this.project_menu = (!menu_open).then(|| keyboard_menu_ref.clone());
                        cx.notify();
                    }))
                    .child(components::icon(IconName::Ellipsis, theme))
                }));

        if menu_open {
            row = row.child(
                div().absolute().top_0().left_0().child(deferred(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .snap_to_window_with_margin(px(8.))
                        .offset(point(self.project_sidebar_width - px(12.), px(12.)))
                        .child(self.render_project_menu(project.clone(), cx)),
                )),
            );
        } else if hovered {
            row = row.child(
                div().absolute().top_0().left_0().child(deferred(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .snap_to_window_with_margin(px(8.))
                        .offset(point(self.project_sidebar_width - px(12.), px(-8.)))
                        .child(project_information_card(&project, theme)),
                )),
            );
        }
        let mut tree = div().relative().child(row);

        if expanded {
            if visible_runs.is_empty() {
                tree = tree.child(
                    div()
                        .h(theme.spacing.tree_row_height)
                        .ml_8()
                        .pl_5()
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
                    .pl_5()
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

    fn render_sidebar_run(
        &mut self,
        run_ref: RunRef,
        placement: RunPlacement,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let selected = self.views.active().runs.contains(&run_ref);
        let hovered = self.hovered_run.as_ref() == Some(&run_ref);
        let run_focus = self
            .run_focuses
            .entry(run_ref.clone())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone();
        let focused = run_focus.contains_focused(window, cx);
        let active = hovered || focused;
        let (name, status) = self.sidebar_run(&run_ref).map_or_else(
            || (run_ref.run_id.as_str().to_owned(), "Unavailable".to_owned()),
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
        let visible_count = self.active_visible_runs().len();
        let run_color = theme
            .colors
            .series_color(renderer::series_color_index(&run_ref));

        components::sidebar_tree_row(
            SharedString::from(format!("run:{}:{index}", run_ref.cache_key())),
            theme,
            false,
            placement == RunPlacement::Projects && !selected && visible_count >= MAX_SELECTED_RUNS,
        )
        .debug_selector({
            let run_ref = run_ref.clone();
            move || format!("project-tree-run-{}", run_ref.run_id.as_str())
        })
        .track_focus(&run_focus)
        .tab_index(0)
        .gap_1()
        .text_xs()
        .when(
            placement == RunPlacement::Projects && (selected || visible_count < MAX_SELECTED_RUNS),
            |row| {
                row.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_tree_run(row_run.clone(), cx);
                    }))
            },
        )
        .on_hover(cx.listener(move |this, is_hovered, _, cx| {
            if *is_hovered {
                this.hovered_run = Some(hover_run.clone());
            } else if this.hovered_run.as_ref() == Some(&hover_run) {
                this.hovered_run = None;
            }
            cx.notify();
        }))
        .children((placement != RunPlacement::Projects).then(|| div().size(px(20.)).flex_none()))
        .children((placement == RunPlacement::Projects).then(|| {
            components::sidebar_icon_button(
                SharedString::from(format!("run-eye:{}", eye_run.cache_key())),
                theme,
                selected,
            )
            .debug_selector(move || format!("run-eye-{index}"))
            .size(px(20.))
            .tooltip(components::label_tooltip(
                if selected { "Hide Run" } else { "Show Run" },
                theme,
            ))
            .when(selected || visible_count < MAX_SELECTED_RUNS, |button| {
                button.on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_tree_run(eye_run.clone(), cx);
                    cx.stop_propagation();
                }))
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
                .size(px(7.))
                .flex_none()
                .rounded(px(3.5))
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
        .children((!active).then(|| {
            div()
                .debug_selector(move || format!("run-status-{index}"))
                .flex_none()
                .text_xs()
                .text_color(theme.colors.text_muted)
                .child(status)
        }))
        .children(active.then(|| {
            div()
                .debug_selector(move || format!("run-actions-{index}"))
                .flex_none()
                .flex()
                .child(
                    components::sidebar_icon_button(
                        SharedString::from(format!("run-baseline:{}", baseline_run.cache_key())),
                        theme,
                        is_baseline,
                    )
                    .tooltip(components::label_tooltip(
                        if is_baseline {
                            "Clear Baseline"
                        } else {
                            "Set Baseline"
                        },
                        theme,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_run_baseline(baseline_run.clone(), cx);
                        cx.stop_propagation();
                    }))
                    .child(components::icon(IconName::Baseline, theme)),
                )
                .child(
                    components::sidebar_icon_button(
                        SharedString::from(format!("run-pin:{}", pin_run.cache_key())),
                        theme,
                        is_pinned,
                    )
                    .tooltip(components::label_tooltip(
                        if is_pinned { "Unpin Run" } else { "Pin Run" },
                        theme,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_pinned_run(pin_run.clone(), cx);
                        cx.stop_propagation();
                    }))
                    .child(components::icon(IconName::Pin, theme)),
                )
                .child(
                    components::sidebar_icon_button(
                        SharedString::from(format!("run-archive:{}", archive_run.cache_key())),
                        theme,
                        is_archived,
                    )
                    .tooltip(components::label_tooltip(
                        if is_archived {
                            "Restore Run"
                        } else {
                            "Archive Run"
                        },
                        theme,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.archive_run(archive_run.clone(), cx);
                        cx.stop_propagation();
                    }))
                    .child(components::icon(IconName::Archive, theme)),
                )
        }))
    }

    fn render_project_menu(
        &mut self,
        project: SidebarProject,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let selected = self.views.active().runs.clone();
        let project_runs = self.project_listing_runs(&project);
        let selected_count = project_runs
            .iter()
            .filter(|run| selected.contains(run))
            .count();
        let visibility_icon = if selected_count == project_runs.len() && !project_runs.is_empty() {
            IconName::Eye
        } else if selected_count == 0 {
            IconName::EyeClosed
        } else {
            IconName::EyeOff
        };
        let visibility_label = if selected_count == project_runs.len() && !project_runs.is_empty() {
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
                if this.dismiss_popovers() {
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_project_runs(&toggle_project, cx);
                    cx.stop_propagation();
                })),
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
                    this.set_project_placement(placement_ref.clone(), ProjectPlacement::Projects);
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
                    this.set_project_placement(archive_ref.clone(), ProjectPlacement::Archived);
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

fn sidebar_group_label(label: &str, theme: ViewerTheme) -> gpui::Div {
    div()
        .mt_3()
        .mb_1()
        .px_2()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.colors.text_muted)
        .child(label.to_owned())
}

fn sidebar_text_button(
    id: impl Into<gpui::ElementId>,
    label: &str,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .ml_8()
        .h(theme.spacing.tree_row_height)
        .flex()
        .items_center()
        .cursor_pointer()
        .text_xs()
        .text_color(theme.colors.text_muted)
        .hover(|style| style.text_color(theme.colors.text))
        .child(label.to_owned())
}

fn sidebar_menu_item(
    id: impl Into<gpui::ElementId>,
    label: &str,
    icon: IconName,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    components::popover_menu_item(id, label, Some(icon), theme)
}

fn project_information_card(
    project: &SidebarProject,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!(
            "project-information:{}",
            project.project_ref.project_id.as_str()
        )))
        .debug_selector({
            let project_ref = project.project_ref.clone();
            move || format!("project-information-{}", project_ref.project_id.as_str())
        })
        .w(px(300.))
        .p_2()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .flex()
        .flex_col()
        .gap_2()
        .text_xs()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .child(components::icon(IconName::Folder, theme)),
                )
                .child(
                    div()
                        .id("project-information-name")
                        .debug_selector(|| "project-information-name".to_owned())
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(project.project.name.clone()),
                )
                .children((project.placement != ProjectPlacement::Projects).then(|| {
                    div()
                        .id("project-information-placement-icon")
                        .debug_selector(|| "project-information-placement-icon".to_owned())
                        .flex_none()
                        .child(components::icon(
                            match project.placement {
                                ProjectPlacement::Pinned => IconName::Pin,
                                ProjectPlacement::Archived => IconName::Archive,
                                ProjectPlacement::Projects => IconName::Folder,
                            },
                            theme,
                        ))
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(components::icon(IconName::CirclePlay, theme))
                .child(format!("{} Runs", project.runs.len())),
        )
        .child(
            div()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(theme.colors.text_muted)
                .child(
                    div()
                        .flex_none()
                        .child(components::icon(IconName::Folder, theme)),
                )
                .child(
                    div()
                        .id("project-information-source")
                        .debug_selector(|| "project-information-source".to_owned())
                        .flex_1()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(project.source_label.clone()),
                ),
        )
}
