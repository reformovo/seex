use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{Context, EventEmitter, FocusHandle, KeyDownEvent, Render, Window, div, prelude::*, px};
use seex::{Project, ProjectId};

use crate::data::SourcePreflight;
use crate::domain::{SourceAlias, SourceAliasError};

use super::components::TextInput;
use super::theme::ViewerTheme;

#[derive(Clone, Debug)]
#[expect(dead_code, reason = "consumed by the next U6 source import slice")]
pub(crate) struct ConfirmedSource {
    pub alias: SourceAlias,
    pub root_path: PathBuf,
    pub projects: Vec<ProjectId>,
}

#[derive(Clone, Debug)]
#[expect(dead_code, reason = "consumed by the next U6 source import slice")]
pub(crate) enum SourceManagementEvent {
    Confirmed(ConfirmedSource),
    Cancelled,
}

pub(crate) struct SourceManagement {
    alias: gpui::Entity<TextInput>,
    alias_focus: FocusHandle,
    draft: Option<SourceDraft>,
}

struct SourceDraft {
    root_path: PathBuf,
    projects: Vec<Project>,
    selected: HashSet<ProjectId>,
    validation_error: Option<String>,
}

impl EventEmitter<SourceManagementEvent> for SourceManagement {}

impl SourceManagement {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let alias = cx.new(|_| TextInput::default());
        cx.observe(&alias, |_, _, cx| cx.notify()).detach();
        Self {
            alias,
            alias_focus: cx.focus_handle().tab_stop(true),
            draft: None,
        }
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "invoked by the next U6 source import slice")
    )]
    pub(crate) fn begin(
        &mut self,
        preflight: SourcePreflight,
        alias: SourceAlias,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.alias.update(cx, |input, cx| {
            input.set_text(alias.as_str());
            input.select_all();
            input.start_blink(cx);
        });
        let selected = preflight
            .projects
            .iter()
            .map(|project| project.project_id.clone())
            .collect();
        self.draft = Some(SourceDraft {
            root_path: preflight.root_path,
            projects: preflight.projects,
            selected,
            validation_error: None,
        });
        self.alias_focus.focus(window);
        cx.notify();
    }

    #[cfg(feature = "test-support")]
    #[cfg_attr(not(test), expect(dead_code, reason = "used by GPUI black-box tests"))]
    pub(crate) fn is_open(&self) -> bool {
        self.draft.is_some()
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

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.draft = None;
        cx.emit(SourceManagementEvent::Cancelled);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let alias = self.alias.read(cx).text().to_owned();
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        let alias = match SourceAlias::new(alias) {
            Ok(alias) => alias,
            Err(SourceAliasError::Invalid) => {
                draft.validation_error =
                    Some("Alias must be a lowercase portable identifier".to_owned());
                cx.notify();
                return;
            }
        };
        if draft.selected.is_empty() {
            draft.validation_error = Some("Select at least one Project".to_owned());
            cx.notify();
            return;
        }
        let confirmed = ConfirmedSource {
            alias,
            root_path: draft.root_path.clone(),
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
}

impl Render for SourceManagement {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(draft) = self.draft.as_ref() else {
            return div().into_any_element();
        };
        let theme = ViewerTheme::for_appearance(window.appearance());
        let alias = self.alias.read(cx).text().to_owned();
        let root_label = draft.root_path.to_string_lossy().into_owned();
        let projects = draft.projects.clone();
        let selected = draft.selected.clone();
        let error = draft.validation_error.clone();
        div()
            .id("source-confirmation-overlay")
            .debug_selector(|| "source-confirmation-overlay".to_owned())
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000066))
            .child(
                div()
                    .id("source-confirmation")
                    .debug_selector(|| "source-confirmation".to_owned())
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
                            .child("Import Source"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.text_muted)
                            .child(root_label),
                    )
                    .child(div().text_xs().child("Alias"))
                    .child(
                        div()
                            .id("source-alias-input")
                            .track_focus(&self.alias_focus)
                            .h(theme.spacing.control_height)
                            .px_2()
                            .flex()
                            .items_center()
                            .border_1()
                            .border_color(theme.colors.border)
                            .rounded(theme.spacing.corner_radius)
                            .on_key_down(cx.listener(Self::edit_alias))
                            .child(alias),
                    )
                    .child(div().text_xs().child("Projects"))
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
                                dialog_button("confirm-source", "Import", theme)
                                    .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
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
