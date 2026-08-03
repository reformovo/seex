//! Desktop-owned, syntax-preserving configuration updates.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use seex::ProjectId;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, value};

use crate::domain::SourceAlias;
use crate::workbench::toml_document::TomlWorkbenchDocument;

const SCHEMA_VERSION: i64 = 1;
static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigScope {
    Global,
    Project,
}

/// One Source together with every configuration document that defines it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedConfiguredSource {
    pub configured: ConfiguredSource,
    pub owners: Vec<ConfigScope>,
}

/// The fixed Viewer scope, editable documents, and merged Source definitions.
#[derive(Clone)]
pub struct SourceConfiguration {
    pub global: EditableConfig,
    pub project: Option<EditableConfig>,
    pub global_base: PathBuf,
    pub project_base: Option<PathBuf>,
    pub sources: Vec<OwnedConfiguredSource>,
}

/// One loaded configuration document and its exact stale-read fingerprint.
#[derive(Clone)]
pub struct EditableConfig {
    path: PathBuf,
    scope: ConfigScope,
    original: Option<Vec<u8>>,
    document: DocumentMut,
    dirty: bool,
}

/// One validated machine-local Source definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredSource {
    pub alias: SourceAlias,
    pub root_path: PathBuf,
    pub projects: Vec<ProjectId>,
}

pub(crate) fn same_source_path(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (fs::canonicalize(left), fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

/// Loads and merges global and project Source definitions.
///
/// # Errors
///
/// Returns [`ConfigEditError`] when either document or Source definition is
/// invalid, or when the same alias has different effective definitions.
pub fn load_sources(
    global_path: &Path,
    global_base: &Path,
    project_path: &Path,
    project_base: &Path,
) -> Result<Vec<ConfiguredSource>, ConfigEditError> {
    Ok(
        load_source_configuration(global_path, global_base, Some((project_path, project_base)))?
            .configured_sources(),
    )
}

/// Loads Sources for the Viewer scope selected by an optional project root.
pub fn load_sources_for_scope(
    project_root: Option<&Path>,
) -> Result<Vec<ConfiguredSource>, ConfigEditError> {
    Ok(SourceConfiguration::load_for_scope(project_root)?.configured_sources())
}

impl SourceConfiguration {
    /// Loads the global document and the optional fixed project document.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigEditError`] when a document or Source is invalid.
    pub fn load_for_scope(project_root: Option<&Path>) -> Result<Self, ConfigEditError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self::load_for_scope_at(project_root, home.as_deref())
    }

    pub fn active_scope(&self) -> ConfigScope {
        if self.project.is_some() {
            ConfigScope::Project
        } else {
            ConfigScope::Global
        }
    }

    pub fn configured_sources(&self) -> Vec<ConfiguredSource> {
        let active_scope = self.active_scope();
        let mut candidates = self.sources.iter().collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            let priority = |source: &OwnedConfiguredSource| {
                usize::from(!source.owners.contains(&active_scope))
            };
            priority(left).cmp(&priority(right)).then_with(|| {
                left.configured
                    .alias
                    .as_str()
                    .cmp(right.configured.alias.as_str())
            })
        });
        let mut configured = Vec::<ConfiguredSource>::new();
        for candidate in candidates {
            if configured
                .iter()
                .any(|source| same_source_path(&source.root_path, &candidate.configured.root_path))
            {
                continue;
            }
            configured.push(candidate.configured.clone());
        }
        configured.sort_by(|left, right| left.alias.as_str().cmp(right.alias.as_str()));
        configured
    }

    /// Adds or updates a Source in every document that owns its alias.
    pub fn set_source(
        &mut self,
        alias: &SourceAlias,
        root_path: &Path,
        projects: &[ProjectId],
    ) -> Result<(), ConfigEditError> {
        let owners = self
            .sources
            .iter()
            .find(|source| &source.configured.alias == alias)
            .map(|source| source.owners.clone())
            .unwrap_or_else(|| vec![self.active_scope()]);
        for owner in &owners {
            self.document_mut(*owner)?
                .set_source(alias, root_path, projects)?;
        }
        let configured = ConfiguredSource {
            alias: alias.clone(),
            root_path: root_path.to_owned(),
            projects: projects.to_vec(),
        };
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| &source.configured.alias == alias)
        {
            source.configured = configured;
        } else {
            self.sources
                .push(OwnedConfiguredSource { configured, owners });
            self.sources.sort_by(|left, right| {
                left.configured
                    .alias
                    .as_str()
                    .cmp(right.configured.alias.as_str())
            });
        }
        Ok(())
    }

    /// Removes one Project and deletes the Source definition when it becomes empty.
    pub fn remove_project(
        &mut self,
        alias: &SourceAlias,
        project_id: &ProjectId,
    ) -> Result<(), ConfigEditError> {
        let index = self
            .sources
            .iter()
            .position(|source| &source.configured.alias == alias)
            .ok_or_else(|| ConfigEditError::UnknownSource(alias.to_string()))?;
        let source = self.sources[index].clone();
        let mut projects = source.configured.projects.clone();
        let previous = projects.len();
        projects.retain(|project| project != project_id);
        if projects.len() == previous {
            return Err(ConfigEditError::UnknownProject(
                project_id.as_str().to_owned(),
            ));
        }
        for owner in &source.owners {
            let document = self.document_mut(*owner)?;
            if projects.is_empty() {
                document.remove_source(alias);
            } else {
                document.set_source(alias, &source.configured.root_path, &projects)?;
            }
        }
        if projects.is_empty() {
            self.sources.remove(index);
        } else {
            self.sources[index].configured.projects = projects;
        }
        Ok(())
    }

    pub fn remove_source(&mut self, alias: &SourceAlias) -> Result<(), ConfigEditError> {
        let index = self
            .sources
            .iter()
            .position(|source| &source.configured.alias == alias)
            .ok_or_else(|| ConfigEditError::UnknownSource(alias.to_string()))?;
        let source = self.sources.remove(index);
        for owner in source.owners {
            self.document_mut(owner)?.remove_source(alias);
        }
        Ok(())
    }

    /// Atomically replaces all changed configuration documents after validating
    /// every stale-read fingerprint.
    pub fn save(&mut self) -> Result<(), ConfigEditError> {
        save_files(
            std::iter::once(&self.global)
                .chain(self.project.as_ref())
                .filter(|document| document.dirty)
                .map(|document| StagedFile::configuration_in_scope(document, &self.global.path)),
        )?;
        self.mark_saved();
        Ok(())
    }

    /// Atomically replaces changed configuration documents and one workbench.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigEditError`] when validation, staging, stale-read checks,
    /// promotion, or rollback fails.
    pub fn save_with_workbench(
        &mut self,
        path: &Path,
        original: Option<Vec<u8>>,
        workbench: &TomlWorkbenchDocument,
    ) -> Result<(), ConfigEditError> {
        workbench
            .validate()
            .map_err(|error| ConfigEditError::InvalidWorkbench(error.to_string()))?;
        let workbench = StagedFile {
            path: path.to_owned(),
            temporary: temporary_path(path),
            original,
            contents: workbench.encode().into_bytes(),
            global: false,
            configuration: false,
        };
        save_files(
            std::iter::once(&self.global)
                .chain(self.project.as_ref())
                .filter(|document| document.dirty)
                .map(|document| StagedFile::configuration_in_scope(document, &self.global.path))
                .chain(std::iter::once(workbench)),
        )?;
        self.mark_saved();
        Ok(())
    }

    fn mark_saved(&mut self) {
        if let Some(project) = self.project.as_mut().filter(|project| {
            project.path == self.global.path && (project.dirty || self.global.dirty)
        }) {
            let document = if project.dirty {
                project.document.clone()
            } else {
                self.global.document.clone()
            };
            let saved = document.to_string().into_bytes();
            self.global.document = document.clone();
            self.global.original = Some(saved.clone());
            self.global.dirty = false;
            project.document = document;
            project.original = Some(saved);
            project.dirty = false;
            return;
        }
        if self.global.dirty {
            self.global.mark_saved();
        }
        if let Some(project) = self.project.as_mut().filter(|project| project.dirty) {
            project.mark_saved();
        }
    }

    fn document_mut(&mut self, scope: ConfigScope) -> Result<&mut EditableConfig, ConfigEditError> {
        match scope {
            ConfigScope::Global => Ok(&mut self.global),
            ConfigScope::Project => self
                .project
                .as_mut()
                .ok_or(ConfigEditError::MissingProjectScope),
        }
    }

    pub(crate) fn load_for_scope_at(
        project_root: Option<&Path>,
        home: Option<&Path>,
    ) -> Result<Self, ConfigEditError> {
        let global_base = home.or(project_root).unwrap_or_else(|| Path::new("."));
        let global_path = global_base.join(".seex/config.toml");
        let project = project_root.map(|root| (root.join(".seex/config.toml"), root));
        load_source_configuration(
            &global_path,
            global_base,
            project.as_ref().map(|(path, base)| (path.as_path(), *base)),
        )
    }
}

fn load_source_configuration(
    global_path: &Path,
    global_base: &Path,
    project: Option<(&Path, &Path)>,
) -> Result<SourceConfiguration, ConfigEditError> {
    let global = EditableConfig::load(global_path, ConfigScope::Global)?;
    let project_document = project
        .map(|(path, _)| EditableConfig::load(path, ConfigScope::Project))
        .transpose()?;
    let mut sources = BTreeMap::new();
    merge_sources(
        &mut sources,
        parse_sources(&global.document, global_base)?,
        ConfigScope::Global,
    )?;
    if let (Some(document), Some((_, base))) = (&project_document, project) {
        merge_sources(
            &mut sources,
            parse_sources(&document.document, base)?,
            ConfigScope::Project,
        )?;
    }
    Ok(SourceConfiguration {
        global,
        project: project_document,
        global_base: global_base.to_owned(),
        project_base: project.map(|(_, base)| base.to_owned()),
        sources: sources.into_values().collect(),
    })
}

fn merge_sources(
    merged: &mut BTreeMap<String, OwnedConfiguredSource>,
    sources: Vec<ConfiguredSource>,
    scope: ConfigScope,
) -> Result<(), ConfigEditError> {
    for source in sources {
        let key = source.alias.as_str().to_owned();
        if let Some(existing) = merged.get_mut(&key) {
            if existing.configured != source {
                return Err(ConfigEditError::SourceAliasConflict { alias: key });
            }
            existing.owners.push(scope);
        } else {
            merged.insert(
                key,
                OwnedConfiguredSource {
                    configured: source,
                    owners: vec![scope],
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
fn load_sources_for_scope_at(
    project_root: Option<&Path>,
    home: Option<&Path>,
) -> Result<Vec<ConfiguredSource>, ConfigEditError> {
    Ok(SourceConfiguration::load_for_scope_at(project_root, home)?.configured_sources())
}

fn parse_sources(
    document: &DocumentMut,
    base_path: &Path,
) -> Result<Vec<ConfiguredSource>, ConfigEditError> {
    let Some(item) = document.get("sources") else {
        return Ok(Vec::new());
    };
    let sources = item
        .as_table_like()
        .ok_or_else(|| invalid_source("sources"))?;
    sources
        .iter()
        .map(|(name, item)| parse_source(name, item.as_table_like(), base_path))
        .collect()
}

fn parse_source(
    name: &str,
    source: Option<&dyn toml_edit::TableLike>,
    base_path: &Path,
) -> Result<ConfiguredSource, ConfigEditError> {
    let source = source.ok_or_else(|| invalid_source(name))?;
    let alias = SourceAlias::new(name).map_err(|_| invalid_source(name))?;
    let raw_path = source
        .get("path")
        .and_then(toml_edit::Item::as_str)
        .filter(|path| !path.is_empty() && !path.contains("://"))
        .ok_or_else(|| invalid_source(name))?;
    let path = PathBuf::from(raw_path);
    let root_path = if path.is_absolute() {
        path
    } else {
        base_path.join(path)
    };
    let projects = source
        .get("projects")
        .and_then(toml_edit::Item::as_array)
        .ok_or_else(|| invalid_source(name))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|project| !project.is_empty())
                .map(ProjectId::from_string)
                .ok_or_else(|| invalid_source(name))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = HashSet::with_capacity(projects.len());
    if projects.is_empty() || !projects.iter().all(|project| seen.insert(project.as_str())) {
        return Err(invalid_source(name));
    }
    Ok(ConfiguredSource {
        alias,
        root_path,
        projects,
    })
}

fn invalid_source(alias: &str) -> ConfigEditError {
    ConfigEditError::InvalidSourceDefinition {
        alias: alias.to_owned(),
    }
}

impl EditableConfig {
    /// Loads an existing schema-v1 document or creates a new in-memory one.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigEditError`] for I/O, UTF-8, TOML, or schema failures.
    pub fn load(path: impl Into<PathBuf>, scope: ConfigScope) -> Result<Self, ConfigEditError> {
        let path = path.into();
        let original = read_optional(&path)?;
        let document = match original.as_deref() {
            Some(bytes) => {
                let raw = std::str::from_utf8(bytes).map_err(|_| ConfigEditError::InvalidUtf8)?;
                let document = raw.parse::<DocumentMut>()?;
                if document.get("schema_version").and_then(Item::as_integer) != Some(SCHEMA_VERSION)
                {
                    return Err(ConfigEditError::UnsupportedSchema);
                }
                document
            }
            None => {
                let mut document = DocumentMut::new();
                document["schema_version"] = value(SCHEMA_VERSION);
                document
            }
        };
        Ok(Self {
            path,
            scope,
            original,
            document,
            dirty: false,
        })
    }

    /// Sets the Desktop-owned definition for one Source alias.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigEditError`] for a non-UTF-8 path or an empty or duplicate
    /// Project allowlist.
    pub fn set_source(
        &mut self,
        alias: &SourceAlias,
        root_path: &Path,
        projects: &[ProjectId],
    ) -> Result<(), ConfigEditError> {
        let previous = self.document.to_string();
        let root_path = root_path.to_str().ok_or(ConfigEditError::NonUtf8Path)?;
        let mut seen = HashSet::with_capacity(projects.len());
        if projects.is_empty() || !projects.iter().all(|project| seen.insert(project.as_str())) {
            return Err(ConfigEditError::InvalidProjectAllowlist);
        }
        if self.document.get("sources").is_none() {
            let mut sources = Table::new();
            sources.set_implicit(true);
            self.document["sources"] = Item::Table(sources);
        }
        let sources_item = &mut self.document["sources"];
        let sources_inline = sources_item.is_inline_table();
        let sources = sources_item
            .as_table_like_mut()
            .ok_or_else(|| invalid_source(alias.as_str()))?;
        if !sources.contains_key(alias.as_str()) {
            let source = if sources_inline {
                value(InlineTable::new())
            } else {
                Item::Table(Table::new())
            };
            sources.insert(alias.as_str(), source);
        }
        let source = sources
            .get_mut(alias.as_str())
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| invalid_source(alias.as_str()))?;
        source.insert("path", value(root_path));
        let mut allowlist = Array::new();
        allowlist.extend(projects.iter().map(|project| project.as_str()));
        source.insert("projects", value(allowlist));
        self.dirty |= self.document.to_string() != previous;
        Ok(())
    }

    pub fn remove_source(&mut self, alias: &SourceAlias) {
        if let Some(sources) = self.document.get_mut("sources")
            && let Some(sources) = sources.as_table_like_mut()
        {
            self.dirty |= sources.remove(alias.as_str()).is_some();
        }
    }

    fn mark_saved(&mut self) {
        self.original = Some(self.document.to_string().into_bytes());
        self.dirty = false;
    }

    /// Atomically saves the document if its original bytes are still current.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigEditError::StaleRead`] rather than overwriting an
    /// external edit. I/O failures leave the destination untouched.
    pub fn save(&self) -> Result<(), ConfigEditError> {
        save_documents(std::iter::once(self))
    }
}

struct StagedFile {
    path: PathBuf,
    temporary: PathBuf,
    original: Option<Vec<u8>>,
    contents: Vec<u8>,
    global: bool,
    configuration: bool,
}

impl StagedFile {
    fn configuration(document: &EditableConfig) -> Self {
        Self {
            path: document.path.clone(),
            temporary: temporary_path(&document.path),
            original: document.original.clone(),
            contents: document.document.to_string().into_bytes(),
            global: document.scope == ConfigScope::Global,
            configuration: true,
        }
    }

    fn configuration_in_scope(document: &EditableConfig, global_path: &Path) -> Self {
        let mut staged = Self::configuration(document);
        staged.global |= document.path == global_path;
        staged
    }
}

fn save_documents<'a>(
    documents: impl IntoIterator<Item = &'a EditableConfig>,
) -> Result<(), ConfigEditError> {
    save_files(documents.into_iter().map(StagedFile::configuration))
}

fn save_files(files: impl IntoIterator<Item = StagedFile>) -> Result<(), ConfigEditError> {
    let mut staged = Vec::<StagedFile>::new();
    for file in files {
        if let Some(existing) = staged
            .iter_mut()
            .find(|existing| existing.path == file.path)
        {
            if !existing.configuration || !file.configuration {
                return Err(ConfigEditError::ConflictingDestination(file.path));
            }
            if existing.original != file.original {
                return Err(ConfigEditError::StaleRead);
            }
            existing.contents = file.contents;
            existing.global |= file.global;
        } else {
            staged.push(file);
        }
    }
    for file in &staged {
        let parent = file.path.parent().ok_or(ConfigEditError::MissingParent)?;
        fs::create_dir_all(parent)?;
        if let Err(error) = write_staged(file) {
            remove_temporaries(&staged);
            return Err(error);
        }
    }
    for item in &staged {
        if read_optional(&item.path)? != item.original {
            remove_temporaries(&staged);
            return Err(ConfigEditError::StaleRead);
        }
    }
    let mut promoted = Vec::<&StagedFile>::new();
    for item in &staged {
        if let Err(error) = fs::rename(&item.temporary, &item.path) {
            for previous in promoted.into_iter().rev() {
                restore_original(previous)?;
            }
            remove_temporaries(&staged);
            return Err(ConfigEditError::Io(error));
        }
        promoted.push(item);
    }
    Ok(())
}

fn remove_temporaries(staged: &[StagedFile]) {
    staged.iter().for_each(|item| {
        let _ = fs::remove_file(&item.temporary);
    });
}

fn restore_original(file: &StagedFile) -> Result<(), ConfigEditError> {
    match &file.original {
        Some(bytes) => fs::write(&file.path, bytes)?,
        None => match fs::remove_file(&file.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ConfigEditError::Io(error)),
        },
    }
    Ok(())
}

fn write_staged(staged: &StagedFile) -> Result<(), ConfigEditError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged.temporary)?;
    #[cfg(unix)]
    if staged.global {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&staged.contents)?;
    file.sync_all()?;
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, io::Error> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(".{name}.{}.{id}.tmp", std::process::id()))
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigEditError {
    #[error("configuration file is not UTF-8")]
    InvalidUtf8,
    #[error("configuration schema_version must be 1")]
    UnsupportedSchema,
    #[error("Source path must be UTF-8")]
    NonUtf8Path,
    #[error("Source Project allowlist must be non-empty and contain no duplicates")]
    InvalidProjectAllowlist,
    #[error("invalid Source definition for alias {alias}")]
    InvalidSourceDefinition { alias: String },
    #[error("Source alias {alias} has conflicting global and project definitions")]
    SourceAliasConflict { alias: String },
    #[error("configuration path has no parent directory")]
    MissingParent,
    #[error("multiple transaction documents target {0}")]
    ConflictingDestination(PathBuf),
    #[error("project configuration is unavailable in the global Viewer scope")]
    MissingProjectScope,
    #[error("Source {0} is not configured")]
    UnknownSource(String),
    #[error("Project {0} is not imported")]
    UnknownProject(String),
    #[error("configuration changed since it was read")]
    StaleRead,
    #[error("invalid workbench document: {0}")]
    InvalidWorkbench(String),
    #[error("invalid configuration TOML: {0}")]
    Parse(#[from] toml_edit::TomlError),
    #[error("configuration I/O failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_layers_resolve_paths_allowlists_and_alias_conflicts()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let project = root.path().join("project");
        let global_path = home.join(".seex/config.toml");
        let project_path = project.join(".seex/config.toml");
        fs::create_dir_all(global_path.parent().ok_or("global config parent")?)?;
        fs::create_dir_all(project_path.parent().ok_or("project config parent")?)?;
        fs::write(
            &global_path,
            include_str!("../../../tests/fixtures/config/v1-global.toml"),
        )?;
        fs::write(
            &project_path,
            include_str!("../../../tests/fixtures/config/v1-project.toml"),
        )?;

        let sources = load_sources(&global_path, &home, &project_path, &project)?;

        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].alias.as_str(), "local-benchmarks");
        assert_eq!(sources[0].projects[0].as_str(), "reader-benchmarks");
        assert_eq!(sources[1].root_path, home.join("experiments"));
        let scoped = load_sources_for_scope_at(Some(&project), Some(&home))?;
        assert_eq!(scoped.len(), 2);
        let scoped = SourceConfiguration::load_for_scope_at(Some(&project), Some(&home))?;
        assert_eq!(scoped.active_scope(), ConfigScope::Project);
        assert_eq!(scoped.sources[0].owners, [ConfigScope::Project]);
        assert_eq!(scoped.sources[1].owners, [ConfigScope::Global]);

        fs::write(
            &project_path,
            format!(
                "schema_version = 1\n[sources.research]\npath = {:?}\n\
                 projects = ['vision-baseline', 'vision-large']\n",
                home.join("experiments").to_string_lossy()
            ),
        )?;
        let duplicated = SourceConfiguration::load_for_scope_at(Some(&project), Some(&home))?;
        let research = duplicated
            .sources
            .iter()
            .find(|source| source.configured.alias.as_str() == "research")
            .ok_or("duplicated research Source")?;
        assert_eq!(research.owners, [ConfigScope::Global, ConfigScope::Project]);

        fs::write(
            &project_path,
            "schema_version = 1\n[sources.research]\npath = '/different'\n\
             projects = ['vision-baseline']\n",
        )?;
        assert!(matches!(
            load_sources(&global_path, &home, &project_path, &project),
            Err(ConfigEditError::SourceAliasConflict { alias }) if alias == "research"
        ));
        Ok(())
    }

    #[test]
    fn source_update_preserves_unowned_content() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        fs::write(
            &path,
            include_str!("../../../tests/fixtures/config/v1-global.toml"),
        )?;
        let mut config = EditableConfig::load(&path, ConfigScope::Global)?;
        let alias = SourceAlias::new("new-source")?;
        let project = ProjectId::from_string("project-1");
        assert!(matches!(
            config.set_source(&alias, Path::new("/tmp/new-source"), &[]),
            Err(ConfigEditError::InvalidProjectAllowlist)
        ));
        assert!(matches!(
            config.set_source(
                &alias,
                Path::new("/tmp/new-source"),
                &[project.clone(), project.clone()],
            ),
            Err(ConfigEditError::InvalidProjectAllowlist)
        ));
        config.set_source(&alias, Path::new("/tmp/new-source"), &[project])?;

        config.save()?;

        let saved = fs::read_to_string(&path)?;
        assert!(saved.contains("# Shared schema-v1 fixture"));
        assert!(saved.contains("catalog_path = \".seex/global-catalog.ducklake\""));
        assert!(saved.contains("secret_access_key = \"global-secret\""));
        assert!(saved.contains("preserved = \"unknown application setting\""));
        let parsed = saved.parse::<DocumentMut>()?;
        assert_eq!(
            parsed["sources"]["new-source"]["path"].as_str(),
            Some("/tmp/new-source")
        );
        assert_eq!(
            parsed["sources"]["new-source"]["projects"]
                .as_array()
                .map(Array::len),
            Some(1)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn new_source_uses_standard_table_syntax() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        let mut config = EditableConfig::load(&path, ConfigScope::Project)?;
        config.set_source(
            &SourceAlias::new("pulseon-examples")?,
            Path::new("/Users/kaikai/projects/pulseon-examples"),
            &[ProjectId::from_string("viewer-20260728-182721-050201")],
        )?;

        config.save()?;

        assert_eq!(
            fs::read_to_string(path)?,
            "schema_version = 1\n\n[sources.pulseon-examples]\n\
             path = \"/Users/kaikai/projects/pulseon-examples\"\n\
             projects = [\"viewer-20260728-182721-050201\"]\n"
        );
        Ok(())
    }

    #[test]
    fn existing_inline_sources_remain_inline_when_extended()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        fs::write(
            &path,
            "schema_version = 1\n# keep\nsources = { legacy = { path = '/tmp/legacy', \
             projects = ['one'] } }\n",
        )?;
        let mut config = EditableConfig::load(&path, ConfigScope::Project)?;
        config.set_source(
            &SourceAlias::new("added")?,
            Path::new("/tmp/added"),
            &[ProjectId::from_string("two")],
        )?;

        config.save()?;

        let saved = fs::read_to_string(path)?;
        assert!(saved.contains("# keep\nsources = {") && saved.contains("added = {"));
        assert!(!saved.contains("[sources.added]"));
        let parsed = saved.parse::<DocumentMut>()?;
        assert_eq!(
            parsed["sources"]["added"]["path"].as_str(),
            Some("/tmp/added")
        );
        Ok(())
    }

    #[test]
    fn source_configuration_selects_one_definition_for_duplicate_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let project = root.path().join("project");
        let source_root = root.path().join("source");
        fs::create_dir_all(home.join(".seex"))?;
        fs::create_dir_all(project.join(".seex"))?;
        fs::create_dir(&source_root)?;
        #[cfg(unix)]
        let duplicate_root = {
            let link = root.path().join("source-link");
            std::os::unix::fs::symlink(&source_root, &link)?;
            link
        };
        #[cfg(not(unix))]
        let duplicate_root = source_root.clone();
        fs::write(
            home.join(".seex/config.toml"),
            format!(
                "schema_version = 1\n[sources.alpha]\npath = {:?}\nprojects = ['global']\n\
                 [sources.beta]\npath = {:?}\nprojects = ['ignored-global']\n",
                source_root.to_string_lossy(),
                duplicate_root.to_string_lossy(),
            ),
        )?;
        fs::write(
            project.join(".seex/config.toml"),
            format!(
                "schema_version = 1\n[sources.zeta]\npath = {:?}\nprojects = ['project']\n",
                source_root.to_string_lossy(),
            ),
        )?;

        let global = SourceConfiguration::load_for_scope_at(None, Some(&home))?;
        assert_eq!(global.sources.len(), 2);
        assert_eq!(global.configured_sources()[0].alias.as_str(), "alpha");
        assert_eq!(
            global.configured_sources()[0].projects[0].as_str(),
            "global"
        );

        let scoped = SourceConfiguration::load_for_scope_at(Some(&project), Some(&home))?;
        assert_eq!(scoped.sources.len(), 3);
        assert_eq!(scoped.configured_sources().len(), 1);
        assert_eq!(scoped.configured_sources()[0].alias.as_str(), "zeta");
        assert_eq!(
            scoped.configured_sources()[0].projects[0].as_str(),
            "project"
        );
        Ok(())
    }

    #[test]
    fn new_document_saves_schema_version_one() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        let config = EditableConfig::load(&path, ConfigScope::Global)?;

        config.save()?;

        assert_eq!(fs::read_to_string(&path)?, "schema_version = 1\n");
        Ok(())
    }

    #[test]
    fn existing_document_without_schema_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        fs::write(&path, "# missing schema version\n")?;

        assert!(matches!(
            EditableConfig::load(&path, ConfigScope::Global),
            Err(ConfigEditError::UnsupportedSchema)
        ));
        Ok(())
    }

    #[test]
    fn save_rejects_an_external_edit() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        fs::write(&path, "schema_version = 1\n# original\n")?;
        let config = EditableConfig::load(&path, ConfigScope::Project)?;
        fs::write(&path, "schema_version = 1\n# external\n")?;

        assert!(matches!(config.save(), Err(ConfigEditError::StaleRead)));
        assert_eq!(
            fs::read_to_string(path)?,
            "schema_version = 1\n# external\n"
        );
        Ok(())
    }

    #[test]
    fn multi_document_source_edits_are_all_or_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let project = root.path().join("project");
        let global_path = home.join(".seex/config.toml");
        let project_path = project.join(".seex/config.toml");
        fs::create_dir_all(global_path.parent().ok_or("global parent")?)?;
        fs::create_dir_all(project_path.parent().ok_or("project parent")?)?;
        let source_root = root.path().join("source");
        let document = format!(
            "schema_version = 1\n[sources.research]\npath = {:?}\nprojects = ['one', 'two']\n",
            source_root.to_string_lossy()
        );
        fs::write(&global_path, &document)?;
        fs::write(&project_path, &document)?;
        let alias = SourceAlias::new("research")?;
        let mut configuration =
            SourceConfiguration::load_for_scope_at(Some(&project), Some(&home))?;
        configuration.set_source(&alias, &source_root, &[ProjectId::from_string("one")])?;
        fs::write(&project_path, "schema_version = 1\n# external\n")?;

        assert!(matches!(
            configuration.save(),
            Err(ConfigEditError::StaleRead)
        ));
        assert_eq!(fs::read_to_string(&global_path)?, document);

        fs::write(&project_path, &document)?;
        let mut configuration =
            SourceConfiguration::load_for_scope_at(Some(&project), Some(&home))?;
        configuration.remove_project(&alias, &ProjectId::from_string("one"))?;
        configuration.save()?;
        for path in [&global_path, &project_path] {
            let saved = fs::read_to_string(path)?.parse::<DocumentMut>()?;
            assert_eq!(
                saved["sources"]["research"]["projects"]
                    .as_array()
                    .and_then(|projects| projects.get(0))
                    .and_then(toml_edit::Value::as_str),
                Some("two")
            );
        }

        let mut configuration =
            SourceConfiguration::load_for_scope_at(Some(&project), Some(&home))?;
        configuration.remove_project(&alias, &ProjectId::from_string("two"))?;
        configuration.save()?;
        for path in [&global_path, &project_path] {
            assert!(!fs::read_to_string(path)?.contains("[sources.research]"));
        }
        Ok(())
    }

    #[test]
    fn coincident_global_and_project_paths_preserve_global_permissions()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join(".seex/config.toml");
        fs::create_dir_all(path.parent().ok_or("config parent")?)?;
        fs::write(&path, "schema_version = 1\n")?;
        let mut configuration =
            SourceConfiguration::load_for_scope_at(Some(root.path()), Some(root.path()))?;
        configuration.set_source(
            &SourceAlias::new("research")?,
            root.path(),
            &[ProjectId::from_string("project")],
        )?;

        configuration.save()?;
        configuration.set_source(
            &SourceAlias::new("research")?,
            root.path(),
            &[ProjectId::from_string("updated-project")],
        )?;
        configuration.save()?;

        let saved = fs::read_to_string(&path)?.parse::<DocumentMut>()?;
        assert_eq!(
            saved["sources"]["research"]["path"].as_str(),
            root.path().to_str()
        );
        assert_eq!(
            saved["sources"]["research"]["projects"]
                .as_array()
                .and_then(|projects| projects.get(0))
                .and_then(toml_edit::Value::as_str),
            Some("updated-project")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn project_edit_does_not_create_an_unchanged_global_document()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let project = root.path().join("project");
        let global_path = home.join(".seex/config.toml");
        let project_path = project.join(".seex/config.toml");
        let mut configuration =
            SourceConfiguration::load_for_scope_at(Some(&project), Some(&home))?;
        configuration.set_source(
            &SourceAlias::new("research")?,
            root.path(),
            &[ProjectId::from_string("project")],
        )?;

        configuration.save()?;

        assert!(!global_path.exists());
        assert!(project_path.exists());
        Ok(())
    }

    #[test]
    fn workbench_only_save_ignores_unchanged_configuration_documents()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::workbench::toml_document::SavedLayout;

        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        let project = root.path().join("project");
        let global_path = home.join(".seex/config.toml");
        let project_path = project.join(".seex/config.toml");
        let workbench_path = project.join(".seex/workbench.toml");
        fs::create_dir_all(global_path.parent().ok_or("global parent")?)?;
        fs::write(&global_path, "schema_version = 1\n# loaded\n")?;
        let mut configuration =
            SourceConfiguration::load_for_scope_at(Some(&project), Some(&home))?;
        fs::write(&global_path, "schema_version = 1\n# external\n")?;
        let workbench = TomlWorkbenchDocument {
            active_view: 0,
            layout: SavedLayout::default(),
            expanded_projects: Vec::new(),
            pinned_projects: Vec::new(),
            archived_projects: Vec::new(),
            archived_runs: Vec::new(),
            views: Vec::new(),
        };

        configuration.save_with_workbench(&workbench_path, None, &workbench)?;

        assert_eq!(
            fs::read_to_string(global_path)?,
            "schema_version = 1\n# external\n"
        );
        assert!(!project_path.exists());
        assert_eq!(
            TomlWorkbenchDocument::load(&workbench_path)?,
            Some(workbench)
        );
        Ok(())
    }

    #[test]
    fn workbench_and_configuration_commit_only_after_all_stale_checks()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::workbench::toml_document::SavedLayout;

        let root = tempfile::tempdir()?;
        let config_path = root.path().join(".seex/config.toml");
        let workbench_path = root.path().join(".seex/workbench.toml");
        fs::create_dir_all(config_path.parent().ok_or("config parent")?)?;
        fs::write(&config_path, "schema_version = 1\n# original\n")?;
        fs::write(&workbench_path, "external edit")?;
        let original_config = fs::read_to_string(&config_path)?;
        let mut configuration = SourceConfiguration::load_for_scope_at(None, Some(root.path()))?;
        configuration.set_source(
            &SourceAlias::new("research")?,
            root.path(),
            &[ProjectId::from_string("project")],
        )?;
        let workbench = TomlWorkbenchDocument {
            active_view: 0,
            layout: SavedLayout::default(),
            expanded_projects: Vec::new(),
            pinned_projects: Vec::new(),
            archived_projects: Vec::new(),
            archived_runs: Vec::new(),
            views: Vec::new(),
        };

        assert!(matches!(
            configuration.save_with_workbench(&workbench_path, Some(b"stale".to_vec()), &workbench),
            Err(ConfigEditError::StaleRead)
        ));
        assert_eq!(fs::read_to_string(&config_path)?, original_config);

        configuration.save_with_workbench(
            &workbench_path,
            Some(fs::read(&workbench_path)?),
            &workbench,
        )?;
        let saved = fs::read_to_string(config_path)?.parse::<DocumentMut>()?;
        assert_eq!(
            saved["sources"]["research"]["path"].as_str(),
            root.path().to_str()
        );
        assert_eq!(
            TomlWorkbenchDocument::load(&workbench_path)?,
            Some(workbench)
        );
        Ok(())
    }
}
