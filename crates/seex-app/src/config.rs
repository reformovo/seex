//! Desktop-owned, syntax-preserving configuration updates.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use seex::ProjectId;
use toml_edit::{Array, DocumentMut, value};

use crate::domain::SourceAlias;

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
pub struct SourceConfiguration {
    pub global: EditableConfig,
    pub project: Option<EditableConfig>,
    pub global_base: PathBuf,
    pub project_base: Option<PathBuf>,
    pub sources: Vec<OwnedConfiguredSource>,
}

/// One loaded configuration document and its exact stale-read fingerprint.
pub struct EditableConfig {
    path: PathBuf,
    scope: ConfigScope,
    original: Option<Vec<u8>>,
    document: DocumentMut,
}

/// One validated machine-local Source definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfiguredSource {
    pub alias: SourceAlias,
    pub root_path: PathBuf,
    pub projects: Vec<ProjectId>,
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
        self.sources
            .iter()
            .map(|source| source.configured.clone())
            .collect()
    }

    fn load_for_scope_at(
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
                if document["schema_version"].as_integer() != Some(SCHEMA_VERSION) {
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
        let root_path = root_path.to_str().ok_or(ConfigEditError::NonUtf8Path)?;
        let mut seen = HashSet::with_capacity(projects.len());
        if projects.is_empty() || !projects.iter().all(|project| seen.insert(project.as_str())) {
            return Err(ConfigEditError::InvalidProjectAllowlist);
        }
        let source = &mut self.document["sources"][alias.as_str()];
        source["path"] = value(root_path);
        let mut allowlist = Array::new();
        allowlist.extend(projects.iter().map(|project| project.as_str()));
        source["projects"] = value(allowlist);
        Ok(())
    }

    /// Atomically saves the document if its original bytes are still current.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigEditError::StaleRead`] rather than overwriting an
    /// external edit. I/O failures leave the destination untouched.
    pub fn save(&self) -> Result<(), ConfigEditError> {
        let parent = self.path.parent().ok_or(ConfigEditError::MissingParent)?;
        fs::create_dir_all(parent)?;
        let temporary = temporary_path(&self.path);
        let result = self.write_and_replace(&temporary);
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn write_and_replace(&self, temporary: &Path) -> Result<(), ConfigEditError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary)?;
        #[cfg(unix)]
        if self.scope == ConfigScope::Global {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(self.document.to_string().as_bytes())?;
        file.sync_all()?;
        if read_optional(&self.path)? != self.original {
            return Err(ConfigEditError::StaleRead);
        }
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
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
    #[error("configuration changed since it was read")]
    StaleRead,
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
    fn new_document_saves_schema_version_one() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("config.toml");
        let config = EditableConfig::load(&path, ConfigScope::Global)?;

        config.save()?;

        assert_eq!(fs::read_to_string(&path)?, "schema_version = 1\n");
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
}
