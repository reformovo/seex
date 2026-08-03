use std::collections::{HashMap, HashSet};

use seex::ProjectId;

use crate::domain::SourceAlias;

use super::toml_document::{
    SavedProjectRef, SavedRunRef, TomlWorkbenchDocument, TomlWorkbenchError,
};

#[derive(Clone, Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired by the next U6 import UI slice")
)]
pub(crate) struct ImportSourceMapping {
    pub external_alias: SourceAlias,
    pub local_alias: SourceAlias,
    pub available_projects: Vec<ProjectId>,
    pub imported_projects: Vec<ProjectId>,
}

#[derive(Clone, Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired by the next U6 import UI slice")
)]
pub(crate) struct WorkbenchImportPlan {
    pub document: TomlWorkbenchDocument,
    pub alias_rewrites: Vec<(SourceAlias, SourceAlias)>,
    pub allowlist_additions: Vec<(SourceAlias, Vec<ProjectId>)>,
}

/// Validates mappings and rewrites an imported workbench to local aliases.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired by the next U6 import UI slice")
)]
pub(crate) fn preflight_workbench_import(
    mut document: TomlWorkbenchDocument,
    mappings: &[ImportSourceMapping],
) -> Result<WorkbenchImportPlan, WorkbenchImportError> {
    let mut by_external = HashMap::new();
    for mapping in mappings {
        if by_external
            .insert(mapping.external_alias.clone(), mapping)
            .is_some()
        {
            return Err(WorkbenchImportError::DuplicateAlias(
                mapping.external_alias.to_string(),
            ));
        }
    }
    let referenced = referenced_projects(&document);
    let mut additions = HashMap::<SourceAlias, Vec<ProjectId>>::new();
    for (external_alias, project_id) in referenced {
        let mapping = by_external
            .get(&external_alias)
            .ok_or_else(|| WorkbenchImportError::MissingAlias(external_alias.to_string()))?;
        if !mapping.available_projects.contains(&project_id) {
            return Err(WorkbenchImportError::MissingProject {
                alias: mapping.local_alias.to_string(),
                project_id: project_id.as_str().to_owned(),
            });
        }
        if !mapping.imported_projects.contains(&project_id) {
            let projects = additions.entry(mapping.local_alias.clone()).or_default();
            if !projects.contains(&project_id) {
                projects.push(project_id);
            }
        }
    }
    rewrite_document(&mut document, &by_external)?;
    document
        .validate()
        .map_err(WorkbenchImportError::InvalidDocument)?;
    let alias_rewrites = mappings
        .iter()
        .filter(|mapping| mapping.external_alias != mapping.local_alias)
        .map(|mapping| (mapping.external_alias.clone(), mapping.local_alias.clone()))
        .collect();
    let mut allowlist_additions = additions.into_iter().collect::<Vec<_>>();
    allowlist_additions.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
    Ok(WorkbenchImportPlan {
        document,
        alias_rewrites,
        allowlist_additions,
    })
}

fn referenced_projects(document: &TomlWorkbenchDocument) -> HashSet<(SourceAlias, ProjectId)> {
    let mut projects = document
        .expanded_projects
        .iter()
        .chain(&document.pinned_projects)
        .chain(&document.archived_projects)
        .map(|project| (project.source_alias.clone(), project.project_id.clone()))
        .collect::<HashSet<_>>();
    for run in document
        .archived_runs
        .iter()
        .chain(document.views.iter().flat_map(|view| {
            view.runs
                .iter()
                .chain(&view.pinned_runs)
                .chain(&view.baseline)
        }))
    {
        projects.insert((run.source_alias.clone(), run.project_id.clone()));
    }
    projects
}

fn rewrite_document(
    document: &mut TomlWorkbenchDocument,
    mappings: &HashMap<SourceAlias, &ImportSourceMapping>,
) -> Result<(), WorkbenchImportError> {
    for project in document
        .expanded_projects
        .iter_mut()
        .chain(&mut document.pinned_projects)
        .chain(&mut document.archived_projects)
    {
        rewrite_project(project, mappings)?;
    }
    for run in &mut document.archived_runs {
        rewrite_run(run, mappings)?;
    }
    for view in &mut document.views {
        for run in view
            .runs
            .iter_mut()
            .chain(&mut view.pinned_runs)
            .chain(&mut view.baseline)
        {
            rewrite_run(run, mappings)?;
        }
    }
    Ok(())
}

fn rewrite_project(
    project: &mut SavedProjectRef,
    mappings: &HashMap<SourceAlias, &ImportSourceMapping>,
) -> Result<(), WorkbenchImportError> {
    project.source_alias = mappings
        .get(&project.source_alias)
        .ok_or_else(|| WorkbenchImportError::MissingAlias(project.source_alias.to_string()))?
        .local_alias
        .clone();
    Ok(())
}

fn rewrite_run(
    run: &mut SavedRunRef,
    mappings: &HashMap<SourceAlias, &ImportSourceMapping>,
) -> Result<(), WorkbenchImportError> {
    let mut project = SavedProjectRef {
        source_alias: run.source_alias.clone(),
        project_id: run.project_id.clone(),
    };
    rewrite_project(&mut project, mappings)?;
    run.source_alias = project.source_alias;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkbenchImportError {
    #[error("duplicate mapping for Source alias {0}")]
    DuplicateAlias(String),
    #[error("Source alias {0} is not mapped")]
    MissingAlias(String),
    #[error("Source {alias} does not contain Project {project_id}")]
    MissingProject { alias: String, project_id: String },
    #[error("rewritten workbench is invalid: {0}")]
    InvalidDocument(#[source] TomlWorkbenchError),
}

#[cfg(test)]
mod tests {
    use seex::{AlignmentAxis, RunId};

    use super::*;
    use crate::workbench::toml_document::{SavedAnalysisView, SavedLayout};

    fn document(alias: &SourceAlias, project: &str, run: &str) -> TomlWorkbenchDocument {
        TomlWorkbenchDocument {
            active_view: 0,
            layout: SavedLayout::default(),
            expanded_projects: Vec::new(),
            pinned_projects: Vec::new(),
            archived_projects: Vec::new(),
            archived_runs: Vec::new(),
            views: vec![SavedAnalysisView {
                name: "Imported".to_owned(),
                runs: vec![SavedRunRef {
                    source_alias: alias.clone(),
                    project_id: ProjectId::from_string(project),
                    run_id: RunId::from_string(run),
                }],
                baseline: None,
                pinned_runs: Vec::new(),
                metrics: Vec::new(),
                metric_heights: Vec::new(),
                selected_metric: None,
                axis: AlignmentAxis::Step,
                viewport: None,
            }],
        }
    }

    #[test]
    fn import_preflight_rewrites_aliases_and_adds_projects_while_runs_may_be_missing() {
        let external = SourceAlias::new("external").expect("test alias should be valid");
        let local = SourceAlias::new("local").expect("test alias should be valid");
        let plan = preflight_workbench_import(
            document(&external, "project", "missing-run"),
            &[ImportSourceMapping {
                external_alias: external.clone(),
                local_alias: local.clone(),
                available_projects: vec![ProjectId::from_string("project")],
                imported_projects: Vec::new(),
            }],
        )
        .expect("mapped Project should preflight");

        assert_eq!(plan.alias_rewrites, [(external, local.clone())]);
        assert_eq!(plan.allowlist_additions[0].0, local);
        assert_eq!(
            plan.document.views[0].runs[0].source_alias.as_str(),
            "local"
        );
    }

    #[test]
    fn import_preflight_rejects_unmapped_aliases_and_missing_projects() {
        let external = SourceAlias::new("external").expect("test alias should be valid");
        assert!(matches!(
            preflight_workbench_import(document(&external, "project", "run"), &[]),
            Err(WorkbenchImportError::MissingAlias(alias)) if alias == "external"
        ));
        assert!(matches!(
            preflight_workbench_import(
                document(&external, "project", "run"),
                &[ImportSourceMapping {
                    external_alias: external,
                    local_alias: SourceAlias::new("local").expect("test alias should be valid"),
                    available_projects: Vec::new(),
                    imported_projects: Vec::new(),
                }]
            ),
            Err(WorkbenchImportError::MissingProject { project_id, .. }) if project_id == "project"
        ));
    }
}
