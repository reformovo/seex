use std::collections::HashSet;
use std::path::PathBuf;

use gpui::{
    Context, EventEmitter, FocusHandle, KeyDownEvent, Render, SharedString, Window, div,
    prelude::*, px,
};
use seex::{Project, ProjectId};

use crate::config::{ConfiguredSource, same_source_path};
use crate::data::SourcePreflight;
use crate::domain::{SourceAlias, suggest_source_alias};
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
pub(crate) struct SourceBatchPlan {
    pub updates: Vec<ConfiguredSource>,
    pub removed_roots: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct SourcePreflightRequest {
    pub draft_id: u64,
    pub generation: u64,
    pub root_path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) enum SourceManagementEvent {
    ChooseSources(Option<u64>),
    Confirmed(SourceBatchPlan),
    ConfirmedWorkbench,
    Cancelled,
}

pub(crate) struct SourceManagement {
    alias: gpui::Entity<TextInput>,
    alias_focus: FocusHandle,
    source_focus: FocusHandle,
    next_draft_id: u64,
    next_generation: u64,
    sources: Option<SourcesDraft>,
    workbench: Option<WorkbenchImportPlan>,
}

#[derive(Clone)]
struct SourcesDraft {
    items: Vec<SourceDraft>,
    active: Option<u64>,
    reserved_aliases: Vec<SourceAlias>,
    saving: bool,
    error: Option<String>,
}

#[derive(Clone)]
struct SourceDraft {
    id: u64,
    original: Option<ConfiguredSource>,
    alias: String,
    root_path: PathBuf,
    projects: Vec<Project>,
    unavailable_projects: Vec<ProjectId>,
    selected: HashSet<ProjectId>,
    status: DraftStatus,
    pending: Option<(u64, PathBuf)>,
    source_error: Option<String>,
    removed: bool,
}

#[derive(Clone)]
enum DraftStatus {
    Loading,
    Ready,
    Unavailable(String),
}

impl SourceDraft {
    fn is_new(&self) -> bool {
        self.original.is_none()
    }

    fn is_ready(&self) -> bool {
        matches!(self.status, DraftStatus::Ready)
    }

    fn is_dirty(&self) -> bool {
        if self.removed || self.is_new() {
            return true;
        }
        self.original.as_ref().is_some_and(|original| {
            let original = original.projects.iter().collect::<HashSet<_>>();
            let selected = self.selected.iter().collect::<HashSet<_>>();
            original != selected
        })
    }

    fn selected_projects(&self) -> Vec<ProjectId> {
        self.projects
            .iter()
            .map(|project| &project.project_id)
            .chain(self.unavailable_projects.iter())
            .filter(|project| self.selected.contains(*project))
            .cloned()
            .collect()
    }

    fn status_label(&self) -> &'static str {
        if self.removed {
            "Will remove"
        } else if self.pending.is_some() {
            "Loading"
        } else if matches!(self.status, DraftStatus::Unavailable(_)) {
            "Unavailable"
        } else if self.is_new() {
            "New"
        } else if self.is_dirty() {
            "Modified"
        } else {
            "Ready"
        }
    }
}

impl SourcesDraft {
    fn active(&self) -> Option<&SourceDraft> {
        let active = self.active?;
        self.items.iter().find(|item| item.id == active)
    }

    fn active_mut(&mut self) -> Option<&mut SourceDraft> {
        let active = self.active?;
        self.items.iter_mut().find(|item| item.id == active)
    }

    fn can_save(&self) -> bool {
        !self.saving
            && self.items.iter().any(SourceDraft::is_dirty)
            && self.items.iter().all(|item| {
                if item.removed || !item.is_dirty() {
                    return true;
                }
                item.pending.is_none()
                    && item.is_ready()
                    && !item.selected.is_empty()
                    && (!item.is_new() || validate_alias(self, item.id, &item.alias).is_ok())
            })
    }

    fn plan(&self) -> Option<SourceBatchPlan> {
        if !self.can_save() {
            return None;
        }
        let mut updates = Vec::new();
        let mut removed_roots = Vec::new();
        for item in &self.items {
            if item.removed {
                removed_roots.push(item.root_path.clone());
            } else if item.is_dirty() {
                updates.push(ConfiguredSource {
                    alias: SourceAlias::new(&item.alias).ok()?,
                    root_path: item.root_path.clone(),
                    projects: item.selected_projects(),
                });
            }
        }
        Some(SourceBatchPlan {
            updates,
            removed_roots,
        })
    }
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
            next_draft_id: 1,
            next_generation: 1,
            sources: None,
            workbench: None,
        }
    }

    pub(crate) fn begin_sources(
        &mut self,
        existing: Vec<ConfiguredSource>,
        reserved_aliases: Vec<SourceAlias>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<SourcePreflightRequest> {
        self.workbench = None;
        let mut items = Vec::with_capacity(existing.len());
        let mut requests = Vec::with_capacity(existing.len());
        for source in existing {
            let id = self.take_draft_id();
            let generation = self.take_generation();
            requests.push(SourcePreflightRequest {
                draft_id: id,
                generation,
                root_path: source.root_path.clone(),
            });
            items.push(SourceDraft {
                id,
                alias: source.alias.as_str().to_owned(),
                root_path: source.root_path.clone(),
                projects: Vec::new(),
                unavailable_projects: source.projects.clone(),
                selected: source.projects.iter().cloned().collect(),
                status: DraftStatus::Loading,
                pending: Some((generation, source.root_path.clone())),
                source_error: None,
                removed: false,
                original: Some(source),
            });
        }
        let active = items.first().map(|item| item.id);
        self.sources = Some(SourcesDraft {
            items,
            active,
            reserved_aliases,
            saving: false,
            error: None,
        });
        self.sync_active_alias(false, cx);
        self.source_focus.focus(window);
        cx.notify();
        requests
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn begin_import(
        &mut self,
        existing: Vec<ConfiguredSource>,
        reserved_aliases: Vec<SourceAlias>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.begin_sources(existing, reserved_aliases, window, cx);
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn begin_manage(
        &mut self,
        preflight: SourcePreflight,
        alias: SourceAlias,
        selected_projects: &[ProjectId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let configured = ConfiguredSource {
            alias: alias.clone(),
            root_path: preflight.root_path.clone(),
            projects: selected_projects.to_vec(),
        };
        let mut requests = self.begin_sources(vec![configured], vec![alias], window, cx);
        if let Some(request) = requests.pop() {
            self.finish_preflight(request, Ok(preflight), window, cx);
        }
    }

    pub(crate) fn queue_source_paths(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Vec<SourcePreflightRequest> {
        let Some(draft) = self.sources.as_mut().filter(|draft| !draft.saving) else {
            return Vec::new();
        };
        let mut used_aliases = draft.reserved_aliases.clone();
        used_aliases.extend(
            draft
                .items
                .iter()
                .filter_map(|item| SourceAlias::new(&item.alias).ok()),
        );
        let mut requests = Vec::new();
        let mut first_new = None;
        for path in paths {
            if let Some(existing) = draft
                .items
                .iter_mut()
                .find(|item| same_source_path(&item.root_path, &path))
            {
                existing.removed = false;
                draft.active = Some(existing.id);
                continue;
            }
            let id = self.next_draft_id;
            self.next_draft_id = self.next_draft_id.saturating_add(1);
            let generation = self.next_generation;
            self.next_generation = self.next_generation.saturating_add(1);
            let alias = suggest_source_alias(&path, &used_aliases);
            used_aliases.push(alias.clone());
            first_new.get_or_insert(id);
            requests.push(SourcePreflightRequest {
                draft_id: id,
                generation,
                root_path: path.clone(),
            });
            draft.items.push(SourceDraft {
                id,
                original: None,
                alias: alias.as_str().to_owned(),
                root_path: path.clone(),
                projects: Vec::new(),
                unavailable_projects: Vec::new(),
                selected: HashSet::new(),
                status: DraftStatus::Loading,
                pending: Some((generation, path)),
                source_error: None,
                removed: false,
            });
        }
        if let Some(id) = first_new {
            draft.active = Some(id);
        }
        draft.error = None;
        self.sync_active_alias(false, cx);
        cx.notify();
        requests
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn begin_source_preflight(
        &mut self,
        root_path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        self.queue_source_paths(vec![root_path], cx)
            .into_iter()
            .next()
            .map(|request| request.generation)
    }

    pub(crate) fn replace_source_path(
        &mut self,
        draft_id: u64,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Option<SourcePreflightRequest> {
        let draft = self
            .sources
            .as_mut()?
            .items
            .iter_mut()
            .find(|item| item.id == draft_id && item.is_new() && !item.removed)?;
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        draft.pending = Some((generation, path.clone()));
        draft.source_error = None;
        cx.notify();
        Some(SourcePreflightRequest {
            draft_id,
            generation,
            root_path: path,
        })
    }

    pub(crate) fn finish_preflight(
        &mut self,
        request: SourcePreflightRequest,
        result: Result<SourcePreflight, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.sources.as_mut() else {
            return;
        };
        let Some(index) = draft.items.iter().position(|item| {
            item.id == request.draft_id
                && item.pending.as_ref().is_some_and(|(generation, path)| {
                    *generation == request.generation && path == &request.root_path
                })
        }) else {
            return;
        };
        match result {
            Ok(preflight) => {
                if draft.items[index].is_new()
                    && let Some(existing) = draft.items.iter().find(|item| {
                        item.id != request.draft_id
                            && same_source_path(&item.root_path, &preflight.root_path)
                    })
                {
                    let existing_id = existing.id;
                    draft.items.remove(index);
                    if let Some(existing) =
                        draft.items.iter_mut().find(|item| item.id == existing_id)
                    {
                        existing.removed = false;
                    }
                    draft.active = Some(existing_id);
                    self.sync_active_alias(false, cx);
                    cx.notify();
                    return;
                }
                let item = &mut draft.items[index];
                item.root_path = preflight.root_path;
                item.projects = preflight.projects;
                item.unavailable_projects = item
                    .original
                    .as_ref()
                    .map(|source| {
                        source
                            .projects
                            .iter()
                            .filter(|project_id| {
                                !item
                                    .projects
                                    .iter()
                                    .any(|project| &project.project_id == *project_id)
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                item.status = DraftStatus::Ready;
                item.pending = None;
                item.source_error = None;
                if item.is_new() && draft.active == Some(item.id) {
                    self.alias.update(cx, |input, cx| {
                        input.set_text(&item.alias);
                        input.select_all();
                        input.start_blink(cx);
                    });
                    self.alias_focus.focus(window);
                }
            }
            Err(error) => {
                let item = &mut draft.items[index];
                item.pending = None;
                item.source_error = Some(error.clone());
                if matches!(item.status, DraftStatus::Loading) {
                    item.status = DraftStatus::Unavailable(error);
                }
            }
        }
        cx.notify();
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn finish_source_preflight(
        &mut self,
        generation: u64,
        result: Result<(SourcePreflight, SourceAlias), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = self.sources.as_ref().and_then(|draft| {
            draft.items.iter().find_map(|item| {
                item.pending
                    .as_ref()
                    .filter(|(current, _)| *current == generation)
                    .map(|(_, path)| SourcePreflightRequest {
                        draft_id: item.id,
                        generation,
                        root_path: path.clone(),
                    })
            })
        });
        let Some(request) = request else {
            return;
        };
        let result = result.map(|(preflight, alias)| {
            if let Some(item) = self.sources.as_mut().and_then(|draft| {
                draft
                    .items
                    .iter_mut()
                    .find(|item| item.id == request.draft_id)
            }) {
                item.alias = alias.as_str().to_owned();
            }
            preflight
        });
        self.finish_preflight(request, result, window, cx);
    }

    pub(crate) fn report_source_selection_error(&mut self, error: String, cx: &mut Context<Self>) {
        if let Some(draft) = self.sources.as_mut() {
            draft.error = Some(error);
            cx.notify();
        }
    }

    pub(crate) fn finish_save(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        if let Some(draft) = self.sources.as_mut() {
            draft.saving = false;
            draft.error = error;
            cx.notify();
        }
    }

    pub(crate) fn complete_save(&mut self, cx: &mut Context<Self>) {
        self.sources = None;
        self.alias.update(cx, |input, cx| input.stop_blink(cx));
        cx.notify();
    }

    #[cfg(feature = "test-support")]
    #[cfg_attr(not(test), expect(dead_code, reason = "used by GPUI black-box tests"))]
    pub(crate) fn is_open(&self) -> bool {
        self.sources.is_some() || self.workbench.is_some()
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn active_alias_state(&self, cx: &gpui::App) -> (Option<String>, bool) {
        (
            self.sources
                .as_ref()
                .and_then(SourcesDraft::active)
                .map(|item| item.alias.clone()),
            self.alias.read(cx).is_select_all(),
        )
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn active_source_state(&self) -> Option<(String, usize, Option<String>)> {
        self.sources
            .as_ref()
            .and_then(SourcesDraft::active)
            .map(|item| {
                (
                    item.alias.clone(),
                    item.projects.len(),
                    item.source_error.clone(),
                )
            })
    }

    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn select_source_alias(&mut self, alias: &str, cx: &mut Context<Self>) {
        let id = self.sources.as_ref().and_then(|draft| {
            draft
                .items
                .iter()
                .find(|item| item.alias == alias)
                .map(|item| item.id)
        });
        if let Some(id) = id {
            self.select_source(id, cx);
        }
    }

    pub(crate) fn begin_workbench(&mut self, plan: WorkbenchImportPlan, cx: &mut Context<Self>) {
        self.sources = None;
        self.workbench = Some(plan);
        cx.notify();
    }

    fn take_draft_id(&mut self) -> u64 {
        let id = self.next_draft_id;
        self.next_draft_id = self.next_draft_id.saturating_add(1);
        id
    }

    fn take_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        generation
    }

    fn sync_active_alias(&mut self, select_all: bool, cx: &mut Context<Self>) {
        let alias = self
            .sources
            .as_ref()
            .and_then(SourcesDraft::active)
            .map(|item| item.alias.clone())
            .unwrap_or_default();
        self.alias.update(cx, |input, cx| {
            input.set_text(alias);
            if select_all {
                input.select_all();
                input.start_blink(cx);
            } else {
                input.stop_blink(cx);
            }
        });
    }

    fn select_source(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(draft) = self.sources.as_mut().filter(|draft| !draft.saving) else {
            return;
        };
        draft.active = Some(id);
        draft.error = None;
        self.sync_active_alias(false, cx);
        cx.notify();
    }

    fn toggle_project(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        let Some(item) = self
            .sources
            .as_mut()
            .filter(|draft| !draft.saving)
            .and_then(SourcesDraft::active_mut)
            .filter(|item| item.is_ready() && !item.removed)
        else {
            return;
        };
        if !item.selected.insert(project_id.clone()) {
            item.selected.remove(&project_id);
        }
        cx.notify();
    }

    fn select_all_projects(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self
            .sources
            .as_mut()
            .filter(|draft| !draft.saving)
            .and_then(SourcesDraft::active_mut)
            .filter(|item| item.is_ready() && !item.removed)
        else {
            return;
        };
        item.selected = item
            .projects
            .iter()
            .map(|project| project.project_id.clone())
            .chain(item.unavailable_projects.iter().cloned())
            .collect();
        cx.notify();
    }

    fn clear_projects(&mut self, cx: &mut Context<Self>) {
        let Some(item) = self
            .sources
            .as_mut()
            .filter(|draft| !draft.saving)
            .and_then(SourcesDraft::active_mut)
            .filter(|item| item.is_ready() && !item.removed)
        else {
            return;
        };
        item.selected.clear();
        cx.notify();
    }

    fn toggle_remove_source(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.sources.as_mut().filter(|draft| !draft.saving) else {
            return;
        };
        let Some(active) = draft.active else {
            return;
        };
        let Some(index) = draft.items.iter().position(|item| item.id == active) else {
            return;
        };
        if draft.items[index].is_new() {
            draft.items.remove(index);
            draft.active = draft
                .items
                .get(index)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|index| draft.items.get(index))
                })
                .map(|item| item.id);
            self.sync_active_alias(false, cx);
        } else {
            draft.items[index].removed = !draft.items[index].removed;
        }
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.sources.as_ref().is_some_and(|draft| draft.saving) {
            return;
        }
        self.next_generation = self.next_generation.saturating_add(1);
        self.alias.update(cx, |input, cx| input.stop_blink(cx));
        self.sources = None;
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
        let Some(plan) = self.sources.as_ref().and_then(SourcesDraft::plan) else {
            return;
        };
        if let Some(draft) = self.sources.as_mut() {
            draft.saving = true;
            draft.error = None;
        }
        cx.emit(SourceManagementEvent::Confirmed(plan));
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
            let alias = self.alias.read(cx).text().to_owned();
            if let Some(item) = self.sources.as_mut().and_then(SourcesDraft::active_mut)
                && item.is_new()
            {
                item.alias = alias;
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
            "enter" if self.sources.as_ref().is_some_and(SourcesDraft::can_save) => {
                self.confirm(cx);
            }
            "enter" => {}
            _ => return,
        }
        cx.stop_propagation();
    }
}

fn validate_alias(draft: &SourcesDraft, id: u64, alias: &str) -> Result<SourceAlias, &'static str> {
    if alias.is_empty() {
        return Err("Enter a Source alias");
    }
    let alias =
        SourceAlias::new(alias).map_err(|_| "Alias must be a lowercase portable identifier")?;
    if draft.reserved_aliases.contains(&alias)
        || draft
            .items
            .iter()
            .any(|item| item.id != id && !item.removed && item.alias == alias.as_str())
    {
        return Err("Source alias is already configured");
    }
    Ok(alias)
}

impl Render for SourceManagement {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(plan) = self.workbench.as_ref() {
            return render_workbench_import(plan, window, cx).into_any_element();
        }
        let Some(draft) = self.sources.clone() else {
            return div().into_any_element();
        };
        let theme = ViewerTheme::for_appearance(window.appearance());
        let max_height = dialog_max_height(window);
        let width = (window.viewport_size().width - px(32.))
            .max(px(280.))
            .min(px(640.));
        let narrow = width < px(560.);
        let can_save = draft.can_save();
        let saving = draft.saving;
        let active = draft.active().cloned();

        components::modal_backdrop("sources-overlay", theme)
            .child(
                components::dialog_surface("sources-dialog", width, max_height, theme)
                    .on_key_down(cx.listener(Self::handle_dialog_key))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(components::dialog_title("Sources"))
                            .child(
                                components::dialog_button(
                                    "add-sources",
                                    "Add Sources…",
                                    theme,
                                    DialogButtonKind::Secondary,
                                    saving,
                                )
                                .track_focus(&self.source_focus)
                                .when(!saving, |button| {
                                    button.on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(SourceManagementEvent::ChooseSources(None));
                                    }))
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("sources-body")
                            .debug_selector(|| "sources-body".to_owned())
                            .w_full()
                            .min_h(px(300.))
                            .gap_3()
                            .flex()
                            .when(narrow, |body| body.flex_col())
                            .child(self.render_source_list(&draft, narrow, theme, cx))
                            .child(self.render_source_detail(
                                &draft,
                                active.as_ref(),
                                theme,
                                window,
                                cx,
                            )),
                    )
                    .children(draft.error.map(|error| {
                        div()
                            .id("sources-error")
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
                                    "cancel-sources",
                                    "Cancel",
                                    theme,
                                    DialogButtonKind::Secondary,
                                    saving,
                                )
                                .when(!saving, |button| {
                                    button.on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))
                                }),
                            )
                            .child(
                                components::dialog_button(
                                    "save-sources",
                                    if saving { "Saving…" } else { "Save" },
                                    theme,
                                    DialogButtonKind::Primary,
                                    !can_save,
                                )
                                .when(can_save, |button| {
                                    button.on_click(cx.listener(|this, _, _, cx| this.confirm(cx)))
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl SourceManagement {
    fn render_source_list(
        &self,
        draft: &SourcesDraft,
        narrow: bool,
        theme: ViewerTheme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .id("sources-list")
            .debug_selector(|| "sources-list".to_owned())
            .flex_none()
            .when(!narrow, |list| list.w(px(180.)).h_full())
            .when(narrow, |list| list.w_full().max_h(px(132.)))
            .overflow_y_scroll()
            .rounded(theme.spacing.corner_radius)
            .border_1()
            .border_color(theme.colors.border)
            .children(draft.items.iter().map(|item| {
                let id = item.id;
                let selector = SharedString::from(format!("source-item:{}", item.alias));
                let path_selector = SharedString::from(format!("source-item-path:{id}"));
                let selected = draft.active == Some(id);
                let path = item.root_path.to_string_lossy().into_owned();
                let status_color = if matches!(item.status, DraftStatus::Unavailable(_)) {
                    theme.colors.error_text
                } else if item.removed || item.is_new() || item.is_dirty() {
                    theme.colors.accent
                } else {
                    theme.colors.text_muted
                };
                div()
                    .id(selector.clone())
                    .debug_selector(move || selector.to_string())
                    .min_h(px(44.))
                    .px_2()
                    .py_1()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .when(selected, |row| row.bg(theme.colors.element_active))
                    .when(!draft.saving, |row| {
                        row.cursor_pointer()
                            .hover(|style| style.bg(theme.colors.element_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_source(id, cx);
                            }))
                    })
                    .child(div().truncate().child(item.alias.clone()))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_1()
                            .text_color(theme.colors.text_muted)
                            .child(
                                div()
                                    .id(path_selector)
                                    .min_w(px(0.))
                                    .truncate()
                                    .tooltip(components::label_tooltip(path.clone(), theme))
                                    .child(path),
                            )
                            .child(div().text_color(status_color).child(item.status_label())),
                    )
            }))
            .children(draft.items.is_empty().then(|| {
                div()
                    .h(px(72.))
                    .p_2()
                    .flex()
                    .items_center()
                    .text_color(theme.colors.text_muted)
                    .child("No Sources configured")
            }))
            .into_any_element()
    }

    fn render_source_detail(
        &self,
        draft: &SourcesDraft,
        item: Option<&SourceDraft>,
        theme: ViewerTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(item) = item else {
            return div()
                .id("source-detail-empty")
                .flex_1()
                .rounded(theme.spacing.corner_radius)
                .border_1()
                .border_color(theme.colors.border)
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.text_muted)
                .child("Add a Source to choose Projects.")
                .into_any_element();
        };
        let alias_input = self.alias.read(cx);
        let alias = alias_input.text().to_owned();
        let (alias_prefix, alias_suffix) = alias.split_at(alias_input.cursor());
        let alias_select_all = alias_input.is_select_all();
        let alias_cursor_visible = alias_input.cursor_visible();
        let alias_focused = self.alias_focus.is_focused(window);
        let editable = item.is_new() && !item.removed && !draft.saving;
        let selectable = item.is_ready() && !item.removed && !draft.saving;
        let alias_error = editable
            .then(|| validate_alias(draft, item.id, &item.alias).err())
            .flatten();
        let selected_count = item.selected.len();
        let project_count = item.projects.len() + item.unavailable_projects.len();
        let id = item.id;
        let root_label = item.pending.as_ref().map_or_else(
            || item.root_path.to_string_lossy().into_owned(),
            |(_, path)| format!("Reading {}…", path.to_string_lossy()),
        );

        div()
            .id("source-detail")
            .debug_selector(|| "source-detail".to_owned())
            .min_w(px(0.))
            .h_full()
            .flex_1()
            .gap_2()
            .flex()
            .flex_col()
            .child(div().text_xs().child("Source"))
            .child(
                div()
                    .id("source-path")
                    .debug_selector(|| "source-path".to_owned())
                    .h(theme.spacing.control_height)
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(theme.spacing.corner_radius)
                    .border_1()
                    .border_color(theme.colors.border)
                    .text_color(theme.colors.text_muted)
                    .child(components::icon(components::IconName::Folder, theme))
                    .child(
                        div()
                            .id("source-path-label")
                            .debug_selector(|| "source-path-label".to_owned())
                            .min_w(px(0.))
                            .flex_1()
                            .truncate()
                            .tooltip(components::label_tooltip(root_label.clone(), theme))
                            .child(root_label.clone()),
                    )
                    .children(editable.then(|| {
                        div()
                            .id("replace-source")
                            .cursor_pointer()
                            .text_color(theme.colors.accent)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SourceManagementEvent::ChooseSources(Some(id)));
                            }))
                            .child("Replace")
                    })),
            )
            .child(div().text_xs().child("Alias"))
            .child(if editable {
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
                    .children((!alias_select_all).then(|| div().child(alias_prefix.to_owned())))
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
                    .children((!alias_select_all).then(|| div().child(alias_suffix.to_owned())))
                    .child(TextInput::cursor_target(
                        self.alias.clone(),
                        self.alias_focus.clone(),
                        px(8.),
                    ))
                    .into_any_element()
            } else {
                readonly_value("source-alias-readonly", item.alias.clone(), theme)
                    .into_any_element()
            })
            .children(alias_error.map(|error| {
                div()
                    .id("source-alias-error")
                    .debug_selector(|| "source-alias-error".to_owned())
                    .text_color(theme.colors.error_text)
                    .child(error)
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().child("Projects"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .text_color(theme.colors.text_muted)
                            .child(format!("{selected_count}/{project_count} selected"))
                            .children(selectable.then(|| {
                                div()
                                    .id("select-all-projects")
                                    .debug_selector(|| "select-all-projects".to_owned())
                                    .cursor_pointer()
                                    .text_color(theme.colors.accent)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.select_all_projects(cx);
                                    }))
                                    .child("Select all")
                            }))
                            .children(selectable.then(|| {
                                div()
                                    .id("clear-projects")
                                    .debug_selector(|| "clear-projects".to_owned())
                                    .cursor_pointer()
                                    .text_color(theme.colors.accent)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.clear_projects(cx);
                                    }))
                                    .child("Clear")
                            })),
                    ),
            )
            .child(render_projects(item, selectable, theme, cx))
            .children(item.source_error.clone().map(|error| {
                div()
                    .id("source-selection-error")
                    .text_color(theme.colors.error_text)
                    .child(error)
            }))
            .children(
                (!item.removed && item.is_ready() && item.selected.is_empty()).then(|| {
                    div()
                        .text_color(theme.colors.error_text)
                        .child("Select at least one Project or remove this Source.")
                }),
            )
            .child(
                div().flex().justify_end().child(
                    components::dialog_button(
                        "remove-source",
                        if item.removed {
                            "Undo Remove"
                        } else {
                            "Remove Source"
                        },
                        theme,
                        DialogButtonKind::Secondary,
                        draft.saving,
                    )
                    .when(!item.removed, |button| {
                        button.text_color(theme.colors.error_text)
                    })
                    .when(!draft.saving, |button| {
                        button.on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_remove_source(cx);
                        }))
                    }),
                ),
            )
            .into_any_element()
    }
}

fn render_projects(
    item: &SourceDraft,
    selectable: bool,
    theme: ViewerTheme,
    cx: &mut Context<SourceManagement>,
) -> gpui::AnyElement {
    let available = item
        .projects
        .iter()
        .map(|project| (project.project_id.clone(), project.name.clone(), false));
    let unavailable = item
        .unavailable_projects
        .iter()
        .map(|project_id| (project_id.clone(), project_id.as_str().to_owned(), true));
    let rows = available.chain(unavailable).collect::<Vec<_>>();
    div()
        .id("source-project-list")
        .min_h(px(72.))
        .max_h(px(240.))
        .flex_1()
        .overflow_y_scroll()
        .rounded(theme.spacing.corner_radius)
        .border_1()
        .border_color(theme.colors.border)
        .children(rows.is_empty().then(|| {
            div()
                .h(px(72.))
                .px_3()
                .flex()
                .items_center()
                .text_color(theme.colors.text_muted)
                .child(match &item.status {
                    DraftStatus::Loading => "Reading Projects…".to_owned(),
                    DraftStatus::Unavailable(error) => error.clone(),
                    DraftStatus::Ready => "This Source contains no Projects.".to_owned(),
                })
        }))
        .children(rows.into_iter().map(|(project_id, name, unavailable)| {
            let checked = item.selected.contains(&project_id);
            let selector = SharedString::from(format!("source-project:{}", project_id.as_str()));
            let id_selector =
                SharedString::from(format!("source-project-id:{}", project_id.as_str()));
            let checkbox = format!("source-project-checkbox:{}", project_id.as_str());
            let click_id = project_id.clone();
            div()
                .id(selector.clone())
                .debug_selector(move || selector.to_string())
                .min_h(px(42.))
                .px_2()
                .py_1()
                .gap_2()
                .flex()
                .items_center()
                .when(selectable, |row| {
                    row.cursor_pointer()
                        .hover(|style| style.bg(theme.colors.element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_project(click_id.clone(), cx);
                        }))
                })
                .when(!selectable, |row| row.opacity(0.55))
                .child(
                    div()
                        .min_w(px(0.))
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(div().truncate().child(name))
                        .child(
                            div()
                                .id(id_selector.clone())
                                .debug_selector(move || id_selector.to_string())
                                .truncate()
                                .text_color(theme.colors.text_muted)
                                .child(if unavailable {
                                    format!("{} · Unavailable", project_id.as_str())
                                } else {
                                    project_id.as_str().to_owned()
                                }),
                        ),
                )
                .child(components::checkbox(checkbox, theme, checked))
        }))
        .into_any_element()
}

fn render_workbench_import(
    plan: &WorkbenchImportPlan,
    window: &Window,
    cx: &mut Context<SourceManagement>,
) -> gpui::AnyElement {
    let theme = ViewerTheme::for_appearance(window.appearance());
    let rewrites = plan.alias_rewrites.clone();
    let additions = plan.allowlist_additions.clone();
    components::modal_backdrop("workbench-import-confirmation", theme)
        .child(
            components::dialog_surface(
                "workbench-import-dialog",
                px(520.),
                dialog_max_height(window),
                theme,
            )
            .child(components::dialog_title("Replace Workbench?"))
            .child(
                div()
                    .text_color(theme.colors.text_muted)
                    .child("The imported Views and layout replace the current Workbench."),
            )
            .child(div().child("Alias rewrites"))
            .child(import_summary(
                "workbench-alias-rewrites",
                rewrites
                    .into_iter()
                    .map(|(external, local)| format!("{external} → {local}"))
                    .collect(),
                "No alias rewrites",
                theme,
            ))
            .child(div().child("Project allowlist additions"))
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_workbench(cx);
                        })),
                    ),
            ),
        )
        .into_any_element()
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
        .text_color(theme.colors.text_muted)
        .children(lines.is_empty().then(|| div().child(empty)))
        .children(lines.into_iter().map(|line| div().py_1().child(line)))
        .into_any_element()
}
