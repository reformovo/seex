use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{Context, EventEmitter, FocusHandle, KeyDownEvent, Render, Window, div, prelude::*, px};
use seex::{Project, ProjectId};

use crate::config::same_source_path;
use crate::data::SourcePreflight;
use crate::domain::SourceAlias;
use crate::workbench::import::WorkbenchImportPlan;

use super::components::{self, DialogButtonKind, TextInput};
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
    existing_sources: Vec<(SourceAlias, PathBuf)>,
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
        existing_sources: Vec<(SourceAlias, PathBuf)>,
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
            existing_sources,
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
                if let Some((existing_alias, _)) = draft
                    .existing_sources
                    .iter()
                    .find(|(_, root_path)| same_source_path(root_path, &preflight.root_path))
                {
                    draft.source_error =
                        Some(format!("Source is already imported as {existing_alias}"));
                    cx.notify();
                    return;
                }
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
            existing_sources: Vec::new(),
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
    if draft
        .existing_sources
        .iter()
        .any(|(existing, _)| existing == &alias)
    {
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
            let max_height = dialog_max_height(window);
            let rewrites = plan.alias_rewrites.clone();
            let additions = plan.allowlist_additions.clone();
            return components::modal_backdrop("workbench-import-confirmation", theme)
                .child(
                    components::dialog_surface(
                        "workbench-import-dialog",
                        px(520.),
                        max_height,
                        theme,
                    )
                    .child(components::dialog_title("Replace Workbench?"))
                    .child(
                        div()
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
                                components::dialog_button(
                                    "cancel-workbench-import",
                                    "Cancel",
                                    theme,
                                    DialogButtonKind::Secondary,
                                    false,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                            )
                            .child(
                                components::dialog_button(
                                    "confirm-workbench-import",
                                    "Import",
                                    theme,
                                    DialogButtonKind::Primary,
                                    false,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.confirm_workbench(cx);
                                    },
                                )),
                            ),
                    ),
                )
                .into_any_element();
        }
        let Some(draft) = self.draft.as_ref() else {
            return div().into_any_element();
        };
        let theme = ViewerTheme::for_appearance(window.appearance());
        let max_height = dialog_max_height(window);
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
        let selected_count = selected.len();
        let project_count = projects.len();
        let has_projects = !projects.is_empty();
        let preflighting = draft.preflighting.is_some();
        let source_error = draft.source_error.clone();
        let error = draft.validation_error.clone();
        let alias_error = alias_validation_message(draft, &alias);
        let can_confirm = draft.root_path.is_some()
            && !preflighting
            && validate_alias(draft, &alias).is_ok()
            && (!matches!(mode, SourceMode::Import) || !selected.is_empty());
        components::modal_backdrop("source-confirmation-overlay", theme)
            .child(
                components::dialog_surface("source-confirmation", px(440.), max_height, theme)
                    .on_key_down(cx.listener(Self::handle_dialog_key))
                    .child(components::dialog_title(match mode {
                        SourceMode::Import => "Import Source",
                        SourceMode::Manage => "Manage Projects",
                    }))
                    .child(div().text_xs().child("Source"))
                    .children(matches!(mode, SourceMode::Import).then(|| {
                        let tooltip = root_label.clone();
                        div()
                            .id("choose-source")
                            .debug_selector(|| "choose-source".to_owned())
                            .track_focus(&self.source_focus)
                            .tab_index(if preflighting { -1 } else { 0 })
                            .h(theme.spacing.control_height)
                            .px_2()
                            .flex()
                            .items_center()
                            .border_1()
                            .border_color(theme.colors.border)
                            .rounded(theme.spacing.corner_radius)
                            .text_color(theme.colors.text_muted)
                            .gap_2()
                            .when(!preflighting, |element| {
                                element
                                    .cursor_pointer()
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(SourceManagementEvent::ChooseSource);
                                    }))
                            })
                            .when(preflighting, |element| {
                                element.opacity(0.6).cursor_default()
                            })
                            .child(components::icon(components::IconName::Folder, theme))
                            .child(
                                div()
                                    .id("source-path-label")
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .tooltip(components::label_tooltip(tooltip, theme))
                                    .child(root_label.clone()),
                            )
                            .child(div().text_color(theme.colors.accent).child("Browse"))
                    }))
                    .children(
                        matches!(mode, SourceMode::Manage)
                            .then(|| readonly_value("source-readonly", root_label.clone(), theme)),
                    )
                    .children(source_error.map(|error| {
                        div()
                            .id("source-selection-error")
                            .debug_selector(|| "source-selection-error".to_owned())
                            .text_xs()
                            .text_color(theme.colors.error_text)
                            .child(error)
                    }))
                    .child(div().text_xs().child("Alias"))
                    .children(matches!(mode, SourceMode::Import).then(|| {
                        div()
                            .id("source-alias-input")
                            .debug_selector(|| "source-alias-input".to_owned())
                            .track_focus(&self.alias_focus)
                            .h(theme.spacing.control_height)
                            .relative()
                            .px_2()
                            .flex()
                            .items_center()
                            .border_1()
                            .border_color(theme.colors.border)
                            .rounded(theme.spacing.corner_radius)
                            .cursor_text()
                            .on_key_down(cx.listener(Self::edit_alias))
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
                            .children((!alias_select_all).then(|| div().child(alias_suffix)))
                            .child(TextInput::cursor_target(
                                self.alias.clone(),
                                self.alias_focus.clone(),
                                px(8.),
                            ))
                    }))
                    .children(
                        matches!(mode, SourceMode::Manage)
                            .then(|| readonly_value("source-alias-readonly", alias.clone(), theme)),
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
                                    .text_color(theme.colors.text_muted)
                                    .child(format!("{selected_count}/{project_count} selected"))
                                    .child(
                                        div()
                                            .id("select-all-projects")
                                            .debug_selector(|| "select-all-projects".to_owned())
                                            .cursor_pointer()
                                            .text_color(theme.colors.accent)
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
                                            .text_color(theme.colors.accent)
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
                            .debug_selector(|| "source-project-list".to_owned())
                            .min_h(px(72.))
                            .max_h(px(300.))
                            .flex_1()
                            .overflow_y_scroll()
                            .rounded(theme.spacing.corner_radius)
                            .border_1()
                            .border_color(theme.colors.border)
                            .children((!has_projects).then(|| {
                                div()
                                    .h(px(72.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .text_color(theme.colors.text_muted)
                                    .child("Choose a Source to inspect its Projects.")
                            }))
                            .children(projects.into_iter().map(|project| {
                                let project_id = project.project_id.clone();
                                let checked = selected.contains(&project_id);
                                let selector = format!("source-project:{}", project_id.as_str());
                                let id_selector =
                                    format!("source-project-id:{}", project_id.as_str());
                                let checkbox_selector =
                                    format!("source-project-checkbox:{}", project_id.as_str());
                                div()
                                    .id(gpui::SharedString::from(selector.clone()))
                                    .debug_selector(move || selector.clone())
                                    .min_h(px(42.))
                                    .px_2()
                                    .py_1()
                                    .gap_2()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme.colors.element_hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_project(project_id.clone(), cx);
                                    }))
                                    .child(
                                        div()
                                            .min_w(px(0.))
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .child(div().truncate().child(project.name))
                                            .child(
                                                div()
                                                    .id(gpui::SharedString::from(
                                                        id_selector.clone(),
                                                    ))
                                                    .debug_selector(move || id_selector.clone())
                                                    .truncate()
                                                    .text_color(theme.colors.text_muted)
                                                    .child(project.project_id.as_str().to_owned()),
                                            ),
                                    )
                                    .child(components::checkbox(checkbox_selector, theme, checked))
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
                                components::dialog_button(
                                    "cancel-source",
                                    "Cancel",
                                    theme,
                                    DialogButtonKind::Secondary,
                                    false,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                            )
                            .child(
                                components::dialog_button(
                                    "confirm-source",
                                    match mode {
                                        SourceMode::Import => "Import",
                                        SourceMode::Manage => "Save",
                                    },
                                    theme,
                                    DialogButtonKind::Primary,
                                    !can_confirm,
                                )
                                .when(can_confirm, |button| {
                                    button.on_click(cx.listener(|this, _, _, cx| this.confirm(cx)))
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn dialog_max_height(window: &Window) -> gpui::Pixels {
    (window.viewport_size().height - px(32.))
        .max(px(120.))
        .min(px(560.))
}

fn readonly_value(
    id: &'static str,
    value: String,
    theme: ViewerTheme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .h(theme.spacing.control_height)
        .flex()
        .items_center()
        .truncate()
        .text_color(theme.colors.text_muted)
        .child(value)
}

fn import_summary(
    id: &'static str,
    lines: Vec<String>,
    empty: &'static str,
    theme: ViewerTheme,
) -> gpui::AnyElement {
    div()
        .id(id)
        .p_2()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.panel)
        .max_h(px(140.))
        .overflow_y_scroll()
        .text_xs()
        .text_color(theme.colors.text_muted)
        .children(lines.is_empty().then(|| div().child(empty)))
        .children(lines.into_iter().map(|line| div().py_1().child(line)))
        .into_any_element()
}
