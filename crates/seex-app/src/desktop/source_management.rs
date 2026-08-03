use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{Context, EventEmitter, FocusHandle, KeyDownEvent, Render, Window, div, prelude::*, px};
use seex::{Project, ProjectId};

use crate::data::SourcePreflight;
use crate::domain::SourceAlias;
use crate::workbench::import::WorkbenchImportPlan;

use super::components::TextInput;
use super::theme::ViewerTheme;

#[derive(Clone, Debug)]
pub(crate) struct ConfirmedSource {
    pub manage: bool,
    pub alias: SourceAlias,
    pub root_path: PathBuf,
    pub projects: Vec<ProjectId>,
}

#[derive(Clone, Debug)]
pub(crate) enum SourceManagementEvent {
    ChooseSource,
    Confirmed(ConfirmedSource),
    ConfirmedWorkbench,
    Cancelled,
}

pub(crate) struct SourceManagement {
    alias: gpui::Entity<TextInput>,
    alias_focus: FocusHandle,
    source_focus: FocusHandle,
    source_generation: u64,
    draft: Option<SourceDraft>,
    workbench: Option<WorkbenchImportPlan>,
}

struct SourceDraft {
    mode: SourceMode,
    root_path: Option<PathBuf>,
    projects: Vec<Project>,
    selected: HashSet<ProjectId>,
    preflighting: Option<(u64, PathBuf)>,
    source_error: Option<String>,
    existing_aliases: Vec<SourceAlias>,
    validation_error: Option<String>,
}

#[derive(Clone, Copy)]
enum SourceMode {
    Import,
    Manage,
}

impl EventEmitter<SourceManagementEvent> for SourceManagement {}

impl SourceManagement {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let alias = cx.new(|_| TextInput::default());
        cx.observe(&alias, |_, _, cx| cx.notify()).detach();
        Self {
            alias,
            alias_focus: cx.focus_handle().tab_stop(true),
            source_focus: cx.focus_handle().tab_stop(true),
            source_generation: 0,
            draft: None,
            workbench: None,
        }
    }

    pub(crate) fn begin_import(
        &mut self,
        existing_aliases: Vec<SourceAlias>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workbench = None;
        self.alias.update(cx, |input, cx| {
            input.set_text("");
            input.stop_blink(cx);
        });
        self.draft = Some(SourceDraft {
            mode: SourceMode::Import,
            root_path: None,
            projects: Vec::new(),
            selected: HashSet::new(),
            preflighting: None,
            source_error: None,
            existing_aliases,
            validation_error: None,
        });
        self.source_generation = self.source_generation.saturating_add(1);
        self.source_focus.focus(window);
        cx.notify();
    }

    pub(crate) fn begin_source_preflight(
        &mut self,
        root_path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let draft = self
            .draft
            .as_mut()
            .filter(|draft| matches!(draft.mode, SourceMode::Import))?;
        self.source_generation = self.source_generation.saturating_add(1);
        let generation = self.source_generation;
        draft.preflighting = Some((generation, root_path));
        draft.source_error = None;
        draft.validation_error = None;
        cx.notify();
        Some(generation)
    }

    pub(crate) fn finish_source_preflight(
        &mut self,
        generation: u64,
        result: Result<(SourcePreflight, SourceAlias), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.draft.as_mut().filter(|draft| {
            matches!(draft.mode, SourceMode::Import)
                && draft
                    .preflighting
                    .as_ref()
                    .is_some_and(|(current, _)| *current == generation)
        }) else {
            return;
        };
        draft.preflighting = None;
        match result {
            Ok((preflight, alias)) => {
                draft.root_path = Some(preflight.root_path);
                draft.projects = preflight.projects;
                draft.selected.clear();
                draft.source_error = None;
                self.alias.update(cx, |input, cx| {
                    input.set_text(alias.as_str());
                    input.select_all();
                    input.start_blink(cx);
                });
                self.alias_focus.focus(window);
            }
            Err(error) => draft.source_error = Some(error),
        }
        cx.notify();
    }

    pub(crate) fn report_source_selection_error(&mut self, error: String, cx: &mut Context<Self>) {
        if let Some(draft) = self
            .draft
            .as_mut()
            .filter(|draft| matches!(draft.mode, SourceMode::Import))
        {
            draft.source_error = Some(error);
            cx.notify();
        }
    }

    pub(crate) fn begin_manage(
        &mut self,
        preflight: SourcePreflight,
        alias: SourceAlias,
        selected_projects: &[ProjectId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workbench = None;
        self.alias.update(cx, |input, cx| {
            input.set_text(alias.as_str());
            input.stop_blink(cx);
        });
        self.draft = Some(SourceDraft {
            mode: SourceMode::Manage,
            root_path: Some(preflight.root_path),
            projects: preflight.projects,
            selected: selected_projects.iter().cloned().collect(),
            preflighting: None,
            source_error: None,
            existing_aliases: Vec::new(),
            validation_error: None,
        });
        self.alias_focus.focus(window);
        cx.notify();
    }

    #[cfg(feature = "test-support")]
    #[cfg_attr(not(test), expect(dead_code, reason = "used by GPUI black-box tests"))]
    pub(crate) fn is_open(&self) -> bool {
        self.draft.is_some() || self.workbench.is_some()
    }

    pub(crate) fn begin_workbench(&mut self, plan: WorkbenchImportPlan, cx: &mut Context<Self>) {
        self.draft = None;
        self.workbench = Some(plan);
        cx.notify();
    }

    fn toggle_project(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        if !draft.selected.insert(project_id.clone()) {
            draft.selected.remove(&project_id);
        }
        draft.validation_error = None;
        cx.notify();
    }

    fn select_all_projects(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        draft.selected = draft
            .projects
            .iter()
            .map(|project| project.project_id.clone())
            .collect();
        draft.validation_error = None;
        cx.notify();
    }

    fn clear_projects(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        draft.selected.clear();
        draft.validation_error = None;
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.source_generation = self.source_generation.saturating_add(1);
        self.alias.update(cx, |input, cx| input.stop_blink(cx));
        self.draft = None;
        self.workbench = None;
        cx.emit(SourceManagementEvent::Cancelled);
        cx.notify();
    }

    fn confirm_workbench(&mut self, cx: &mut Context<Self>) {
        self.workbench = None;
        cx.emit(SourceManagementEvent::ConfirmedWorkbench);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let alias = self.alias.read(cx).text().to_owned();
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        let Some(root_path) = draft.root_path.clone() else {
            draft.validation_error = Some("Choose a Source folder".to_owned());
            cx.notify();
            return;
        };
        let Ok(alias) = validate_alias(draft, &alias) else {
            return;
        };
        if draft.selected.is_empty() && matches!(draft.mode, SourceMode::Import) {
            return;
        }
        let confirmed = ConfirmedSource {
            manage: matches!(draft.mode, SourceMode::Manage),
            alias,
            root_path,
            projects: draft
                .projects
                .iter()
                .filter(|project| draft.selected.contains(&project.project_id))
                .map(|project| project.project_id.clone())
                .collect(),
        };
        self.draft = None;
        cx.emit(SourceManagementEvent::Confirmed(confirmed));
        cx.notify();
    }

    fn edit_alias(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let outcome = self.alias.update(cx, |input, cx| {
            let outcome = input.edit(event);
            if outcome.handled {
                input.start_blink(cx);
            }
            outcome
        });
        if outcome.changed {
            if let Some(draft) = self.draft.as_mut() {
                draft.validation_error = None;
            }
            cx.notify();
        }
        if outcome.handled {
            cx.stop_propagation();
        }
    }

    fn focus_alias(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.alias.update(cx, |input, cx| {
            input.move_to_end();
            input.start_blink(cx);
        });
        self.alias_focus.focus(window);
        cx.notify();
    }

    fn handle_dialog_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => self.cancel(cx),
            "enter" if self.can_confirm(cx) => self.confirm(cx),
            "enter" => {}
            _ => return,
        }
        cx.stop_propagation();
    }

    fn can_confirm(&self, cx: &gpui::App) -> bool {
        let Some(draft) = self.draft.as_ref() else {
            return false;
        };
        let alias = self.alias.read(cx).text();
        draft.root_path.is_some()
            && draft.preflighting.is_none()
            && validate_alias(draft, alias).is_ok()
            && (!matches!(draft.mode, SourceMode::Import) || !draft.selected.is_empty())
    }
}

fn validate_alias(draft: &SourceDraft, alias: &str) -> Result<SourceAlias, &'static str> {
    if alias.is_empty() {
        return Err("Enter a Source alias");
    }
    let alias =
        SourceAlias::new(alias).map_err(|_| "Alias must be a lowercase portable identifier")?;
    if draft.existing_aliases.contains(&alias) {
        return Err("Source alias is already configured");
    }
    Ok(alias)
}

fn alias_validation_message(draft: &SourceDraft, alias: &str) -> Option<&'static str> {
    if !matches!(draft.mode, SourceMode::Import) || draft.root_path.is_none() {
        return None;
    }
    validate_alias(draft, alias).err()
}

impl Render for SourceManagement {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(plan) = self.workbench.as_ref() {
            let theme = ViewerTheme::for_appearance(window.appearance());
            let rewrites = plan.alias_rewrites.clone();
            let additions = plan.allowlist_additions.clone();
            return confirmation_overlay(
                "workbench-import-confirmation",
                div()
                    .w(px(520.))
                    .max_h(px(560.))
                    .p_4()
                    .gap_3()
                    .flex()
                    .flex_col()
                    .rounded(theme.spacing.corner_radius)
                    .border_1()
                    .border_color(theme.colors.border)
                    .bg(theme.colors.surface)
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Replace Workbench?"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.text_muted)
                            .child("The imported Views and layout replace the current Workbench."),
                    )
                    .child(div().text_xs().child("Alias rewrites"))
                    .child(import_summary(
                        "workbench-alias-rewrites",
                        rewrites
                            .into_iter()
                            .map(|(external, local)| format!("{external} → {local}"))
                            .collect(),
                        "No alias rewrites",
                        theme,
                    ))
                    .child(div().text_xs().child("Project allowlist additions"))
                    .child(import_summary(
                        "workbench-allowlist-additions",
                        additions
                            .into_iter()
                            .map(|(alias, projects)| {
                                format!(
                                    "{alias}: {}",
                                    projects
                                        .iter()
                                        .map(|project| project.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )
                            })
                            .collect(),
                        "No allowlist changes",
                        theme,
                    ))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                dialog_button("cancel-workbench-import", "Cancel", theme)
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                            )
                            .child(
                                dialog_button("confirm-workbench-import", "Import", theme)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_workbench(cx);
                                    })),
                            ),
                    ),
            );
        }
        let Some(draft) = self.draft.as_ref() else {
            return div().into_any_element();
        };
        let theme = ViewerTheme::for_appearance(window.appearance());
        let alias_input = self.alias.read(cx);
        let alias = alias_input.text().to_owned();
        let (alias_prefix, alias_suffix) = alias.split_at(alias_input.cursor());
        let alias_prefix = alias_prefix.to_owned();
        let alias_suffix = alias_suffix.to_owned();
        let alias_select_all = alias_input.is_select_all();
        let alias_cursor_visible = alias_input.cursor_visible();
        let alias_focused = self.alias_focus.is_focused(window);
        let root_label = draft
            .preflighting
            .as_ref()
            .map(|(_, path)| format!("Reading {}…", path.to_string_lossy()))
            .or_else(|| {
                draft
                    .root_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Choose a Source folder…".to_owned());
        let mode = draft.mode;
        let projects = draft.projects.clone();
        let selected = draft.selected.clone();
        let has_projects = !projects.is_empty();
        let preflighting = draft.preflighting.is_some();
        let source_error = draft.source_error.clone();
        let error = draft.validation_error.clone();
        let alias_error = alias_validation_message(draft, &alias);
        let can_confirm = draft.root_path.is_some()
            && !preflighting
            && validate_alias(draft, &alias).is_ok()
            && (!matches!(mode, SourceMode::Import) || !selected.is_empty());
        confirmation_overlay(
            "source-confirmation-overlay",
            div()
                .id("source-confirmation")
                .debug_selector(|| "source-confirmation".to_owned())
                .on_key_down(cx.listener(Self::handle_dialog_key))
                .w(px(480.))
                .max_h(px(560.))
                .p_4()
                .gap_3()
                .flex()
                .flex_col()
                .rounded(theme.spacing.corner_radius)
                .border_1()
                .border_color(theme.colors.border)
                .bg(theme.colors.surface)
                .child(
                    div()
                        .text_lg()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(match mode {
                            SourceMode::Import => "Import Source",
                            SourceMode::Manage => "Manage Projects",
                        }),
                )
                .child(div().text_xs().child("Source"))
                .child(
                    div()
                        .id("choose-source")
                        .debug_selector(|| "choose-source".to_owned())
                        .track_focus(&self.source_focus)
                        .h(theme.spacing.control_height)
                        .px_2()
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(theme.colors.border)
                        .rounded(theme.spacing.corner_radius)
                        .text_xs()
                        .text_color(theme.colors.text_muted)
                        .when(
                            matches!(mode, SourceMode::Import) && !preflighting,
                            |element| {
                                element
                                    .cursor_pointer()
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(SourceManagementEvent::ChooseSource);
                                    }))
                            },
                        )
                        .when(preflighting, |element| {
                            element.opacity(0.6).cursor_default()
                        })
                        .child(root_label),
                )
                .children(source_error.map(|error| {
                    div()
                        .text_xs()
                        .text_color(theme.colors.error_text)
                        .child(error)
                }))
                .child(div().text_xs().child("Alias"))
                .child(
                    div()
                        .id("source-alias-input")
                        .debug_selector(|| "source-alias-input".to_owned())
                        .track_focus(&self.alias_focus)
                        .h(theme.spacing.control_height)
                        .px_2()
                        .flex()
                        .items_center()
                        .border_1()
                        .border_color(theme.colors.border)
                        .rounded(theme.spacing.corner_radius)
                        .cursor_text()
                        .when(matches!(mode, SourceMode::Import), |element| {
                            element.on_key_down(cx.listener(Self::edit_alias)).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.focus_alias(window, cx);
                                }),
                            )
                        })
                        .children(alias_select_all.then(|| {
                            div()
                                .id("source-alias-selection")
                                .debug_selector(|| "source-alias-selection".to_owned())
                                .rounded(px(2.))
                                .bg(theme.colors.element_active)
                                .child(alias.clone())
                        }))
                        .children((!alias_select_all).then(|| div().child(alias_prefix)))
                        .children((alias_focused && alias_cursor_visible).then(|| {
                            div()
                                .id("source-alias-caret")
                                .debug_selector(|| "source-alias-caret".to_owned())
                                .ml(px(1.))
                                .w(px(1.))
                                .h(px(14.))
                                .flex_none()
                                .bg(theme.colors.text)
                        }))
                        .children((!alias_select_all).then(|| div().child(alias_suffix))),
                )
                .children(alias_error.map(|error| {
                    div()
                        .id("source-alias-error")
                        .debug_selector(|| "source-alias-error".to_owned())
                        .text_xs()
                        .text_color(theme.colors.error_text)
                        .child(error)
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_xs().child("Projects"))
                        .children(has_projects.then(|| {
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .id("select-all-projects")
                                        .debug_selector(|| "select-all-projects".to_owned())
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.select_all_projects(cx);
                                        }))
                                        .child("Select all"),
                                )
                                .child(
                                    div()
                                        .id("clear-projects")
                                        .debug_selector(|| "clear-projects".to_owned())
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.clear_projects(cx);
                                        }))
                                        .child("Clear"),
                                )
                        })),
                )
                .child(
                    div()
                        .id("source-project-list")
                        .max_h(px(300.))
                        .overflow_y_scroll()
                        .children(projects.into_iter().map(|project| {
                            let project_id = project.project_id.clone();
                            let checked = selected.contains(&project_id);
                            let selector = format!("source-project:{}", project_id.as_str());
                            div()
                                .id(gpui::SharedString::from(selector.clone()))
                                .debug_selector(move || selector.clone())
                                .h(theme.spacing.tree_row_height)
                                .px_2()
                                .gap_2()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .hover(|style| style.bg(theme.colors.element_hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_project(project_id.clone(), cx);
                                }))
                                .child(if checked { "☑" } else { "☐" })
                                .child(project.name)
                        })),
                )
                .children(error.map(|error| {
                    div()
                        .text_xs()
                        .text_color(theme.colors.error_text)
                        .child(error)
                }))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            dialog_button("cancel-source", "Cancel", theme)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        )
                        .child(
                            dialog_button(
                                "confirm-source",
                                match mode {
                                    SourceMode::Import => "Import",
                                    SourceMode::Manage => "Save",
                                },
                                theme,
                            )
                            .when(can_confirm, |button| {
                                button.on_click(cx.listener(|this, _, _, cx| this.confirm(cx)))
                            })
                            .when(!can_confirm, |button| button.opacity(0.35).cursor_default()),
                        ),
                ),
        )
    }
}

fn confirmation_overlay(id: &'static str, content: impl IntoElement) -> gpui::AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x00000066))
        .child(content)
        .into_any_element()
}

fn import_summary(
    id: &'static str,
    lines: Vec<String>,
    empty: &'static str,
    theme: ViewerTheme,
) -> gpui::AnyElement {
    div()
        .id(id)
        .max_h(px(140.))
        .overflow_y_scroll()
        .text_xs()
        .text_color(theme.colors.text_muted)
        .children(lines.is_empty().then_some(empty))
        .children(lines)
        .into_any_element()
}

fn dialog_button(
    id: &'static str,
    label: &'static str,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .h(theme.spacing.control_height)
        .px_3()
        .flex()
        .items_center()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .cursor_pointer()
        .hover(|style| style.bg(theme.colors.element_hover))
        .child(label)
}
