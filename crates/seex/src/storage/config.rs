use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml::Table;

use crate::storage::bootstrap::CatalogBackend;

const MAX_METRIC_QUEUE_CAPACITY: i64 = 1_048_576;
const CONFIG_SCHEMA_VERSION: i64 = 1;

struct ConfigDocument {
    table: Table,
    base_path: PathBuf,
}

struct ConfigLayers {
    global: Option<ConfigDocument>,
    project: Option<ConfigDocument>,
}

impl ConfigLayers {
    fn documents(&self) -> impl Iterator<Item = &ConfigDocument> {
        [self.project.as_ref(), self.global.as_ref()]
            .into_iter()
            .flatten()
    }
}

pub struct ResolvedInitConfig {
    pub catalog_backend: CatalogBackend,
    pub catalog_path: Option<PathBuf>,
    pub data_path: Option<PathBuf>,
    pub metric_queue_capacity: usize,
}

/// Storage paths and catalog backend resolved without object-store credentials.
pub struct ResolvedStorageConfig {
    pub catalog_backend: CatalogBackend,
    pub catalog_path: Option<PathBuf>,
    pub data_path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum InitConfigError {
    #[error("failed to read {scope} config.toml: {source}")]
    ReadConfig {
        scope: &'static str,
        #[source]
        source: io::Error,
    },

    #[error("invalid {scope} config.toml: {source}")]
    ParseConfig {
        scope: &'static str,
        #[source]
        source: toml::de::Error,
    },

    #[error("{0}")]
    Invalid(String),
}

pub fn resolve_init_config(
    root_path: &Path,
    data_path: Option<PathBuf>,
    catalog_backend: Option<&str>,
    catalog_path: Option<PathBuf>,
    metric_queue_capacity: i64,
) -> Result<ResolvedInitConfig, InitConfigError> {
    let layers = load_config_layers(root_path)?;
    let storage = resolve_storage_config_from_layers(
        root_path,
        data_path,
        catalog_backend,
        catalog_path,
        &layers,
    )?;
    let metric_queue_capacity = validate_metric_queue_capacity(metric_queue_capacity)?;

    Ok(ResolvedInitConfig {
        catalog_backend: storage.catalog_backend,
        catalog_path: storage.catalog_path,
        data_path: storage.data_path,
        metric_queue_capacity,
    })
}

/// Resolves native catalog and data paths from layered configuration.
///
/// # Errors
///
/// Returns [`InitConfigError`] when the project configuration cannot be read or
/// contains an invalid catalog backend or path.
fn resolve_storage_config_from_layers(
    root_path: &Path,
    data_path: Option<PathBuf>,
    catalog_backend: Option<&str>,
    catalog_path: Option<PathBuf>,
    layers: &ConfigLayers,
) -> Result<ResolvedStorageConfig, InitConfigError> {
    let data_path = resolve_layered_data_path(root_path, data_path, layers)?;
    let catalog_backend = resolve_layered_catalog_backend(catalog_backend, layers)?;
    let catalog_path = resolve_layered_catalog_path(root_path, catalog_path, layers)?;
    validate_path_configuration(data_path.as_deref(), catalog_path.as_deref())?;

    Ok(ResolvedStorageConfig {
        catalog_backend,
        catalog_path,
        data_path,
    })
}

fn load_config_layers(root_path: &Path) -> Result<ConfigLayers, InitConfigError> {
    let home_path = std::env::var_os("HOME").map(PathBuf::from);
    load_config_layers_from(root_path, home_path.as_deref())
}

fn load_config_layers_from(
    root_path: &Path,
    home_path: Option<&Path>,
) -> Result<ConfigLayers, InitConfigError> {
    let global = home_path
        .map(|home| load_config_document("global", &home.join(".seex/config.toml"), home))
        .transpose()?
        .flatten();
    let project = load_config_document("project", &root_path.join(".seex/config.toml"), root_path)?;
    Ok(ConfigLayers { global, project })
}

fn load_config_document(
    scope: &'static str,
    config_path: &Path,
    base_path: &Path,
) -> Result<Option<ConfigDocument>, InitConfigError> {
    let content = match fs::read_to_string(config_path) {
        Ok(content) => content,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(InitConfigError::ReadConfig { scope, source }),
    };
    let table = content
        .parse::<Table>()
        .map_err(|source| InitConfigError::ParseConfig { scope, source })?;
    match table
        .get("schema_version")
        .and_then(toml::Value::as_integer)
    {
        Some(CONFIG_SCHEMA_VERSION) => Ok(Some(ConfigDocument {
            table,
            base_path: base_path.to_owned(),
        })),
        _ => Err(invalid(format!(
            "{scope} config.toml schema_version must be {CONFIG_SCHEMA_VERSION}"
        ))),
    }
}

fn resolve_layered_data_path(
    root_path: &Path,
    explicit: Option<PathBuf>,
    layers: &ConfigLayers,
) -> Result<Option<PathBuf>, InitConfigError> {
    if explicit.is_some() {
        return resolve_data_path(root_path, explicit, None);
    }
    for document in layers.documents() {
        let value = resolve_data_path(&document.base_path, None, Some(&document.table))?;
        if value.is_some() {
            return Ok(value);
        }
    }
    Ok(None)
}

fn resolve_layered_catalog_backend(
    explicit: Option<&str>,
    layers: &ConfigLayers,
) -> Result<CatalogBackend, InitConfigError> {
    if explicit.is_some() {
        return resolve_catalog_backend(explicit, None);
    }
    for document in layers.documents() {
        if document.table.contains_key("catalog_backend") {
            return resolve_catalog_backend(None, Some(&document.table));
        }
    }
    resolve_catalog_backend(None, None)
}

fn resolve_layered_catalog_path(
    root_path: &Path,
    explicit: Option<PathBuf>,
    layers: &ConfigLayers,
) -> Result<Option<PathBuf>, InitConfigError> {
    if explicit.is_some() {
        return resolve_catalog_path(root_path, explicit, None);
    }
    for document in layers.documents() {
        let value = resolve_catalog_path(&document.base_path, None, Some(&document.table))?;
        if value.is_some() {
            return Ok(value);
        }
    }
    Ok(None)
}

fn resolve_data_path(
    root_path: &Path,
    explicit: Option<PathBuf>,
    config: Option<&Table>,
) -> Result<Option<PathBuf>, InitConfigError> {
    if explicit.is_some() {
        return Ok(explicit);
    }
    config
        .map(|config| optional_string(config, "data_path", "config.toml data_path"))
        .transpose()
        .map(|value| {
            value
                .flatten()
                .map(PathBuf::from)
                .map(|path| resolve_config_path(root_path, path))
        })
}

fn resolve_catalog_backend(
    explicit: Option<&str>,
    config: Option<&Table>,
) -> Result<CatalogBackend, InitConfigError> {
    let configured = if explicit.is_some() {
        None
    } else {
        config
            .map(|config| optional_string(config, "catalog_backend", "config.toml catalog_backend"))
            .transpose()?
            .flatten()
    };
    let name = explicit.or(configured.as_deref()).unwrap_or("sqlite");
    let backend = CatalogBackend::from_name(name)
        .ok_or_else(|| invalid(format!("unsupported catalog_backend: {name}")))?;
    if backend != CatalogBackend::Sqlite {
        return Err(invalid(
            "catalog_backend must be 'sqlite'; other catalog backends are not supported",
        ));
    }
    Ok(backend)
}

fn resolve_catalog_path(
    root_path: &Path,
    explicit: Option<PathBuf>,
    config: Option<&Table>,
) -> Result<Option<PathBuf>, InitConfigError> {
    if explicit.is_some() {
        return Ok(explicit);
    }
    config
        .map(|config| optional_string(config, "catalog_path", "config.toml catalog_path"))
        .transpose()
        .map(|value| {
            value
                .flatten()
                .map(PathBuf::from)
                .map(|path| resolve_config_path(root_path, path))
        })
}

fn resolve_config_path(root_path: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() || is_uri_path(&path) {
        path
    } else {
        root_path.join(path)
    }
}

fn validate_path_configuration(
    data_path: Option<&Path>,
    catalog_path: Option<&Path>,
) -> Result<(), InitConfigError> {
    if data_path.is_some_and(is_uri_path) {
        return Err(invalid("data_path must be a local filesystem path"));
    }
    if catalog_path.is_some_and(is_uri_path) {
        return Err(invalid("catalog_path must be a local filesystem path"));
    }
    Ok(())
}

fn validate_metric_queue_capacity(value: i64) -> Result<usize, InitConfigError> {
    if !(1..=MAX_METRIC_QUEUE_CAPACITY).contains(&value) {
        return Err(invalid(
            "metric_queue_capacity must be between 1 and 1048576",
        ));
    }
    usize::try_from(value)
        .map_err(|_| invalid("metric_queue_capacity must be between 1 and 1048576"))
}

fn optional_string(
    config: &Table,
    key: &str,
    label: &str,
) -> Result<Option<String>, InitConfigError> {
    let Some(value) = config.get(key) else {
        return Ok(None);
    };
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(format!("{label} must be a string")))
        .map(Some)
}

fn is_uri_path(path: &Path) -> bool {
    path.to_string_lossy().contains("://")
}

fn invalid(message: impl Into<String>) -> InitConfigError {
    InitConfigError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_config(raw: &str) -> Table {
        raw.parse::<Table>().expect("test config should parse")
    }

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("seex-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn schema_v1_layers_keep_precedence_and_path_bases() -> Result<(), Box<dyn std::error::Error>> {
        let base = test_directory("config-layers");
        let home = base.join("home");
        let root = base.join("project");
        fs::create_dir_all(home.join(".seex"))?;
        fs::create_dir_all(root.join(".seex"))?;
        fs::write(
            home.join(".seex/config.toml"),
            include_str!("../../tests/fixtures/config/v1-global.toml"),
        )?;
        fs::write(
            root.join(".seex/config.toml"),
            include_str!("../../tests/fixtures/config/v1-project.toml"),
        )?;

        let layers = load_config_layers_from(&root, Some(&home))?;
        assert_eq!(
            resolve_layered_catalog_backend(None, &layers)?,
            CatalogBackend::Sqlite
        );
        assert_eq!(
            resolve_layered_data_path(&root, None, &layers)?,
            Some(root.join(".seex/data"))
        );
        let global_only = ConfigLayers {
            global: layers.global,
            project: None,
        };
        assert_eq!(
            resolve_layered_catalog_path(&root, None, &global_only)?,
            Some(home.join(".seex/global-catalog.ducklake"))
        );
        fs::remove_dir_all(base)?;
        Ok(())
    }

    #[test]
    fn loaded_config_requires_schema_version_one() -> Result<(), Box<dyn std::error::Error>> {
        let root = test_directory("config-version");
        fs::create_dir_all(root.join(".seex"))?;
        fs::write(root.join(".seex/config.toml"), "data_path = 'data'\n")?;

        let error = load_config_layers_from(&root, None)
            .err()
            .ok_or("missing schema version should fail")?;

        assert_eq!(
            error.to_string(),
            "project config.toml schema_version must be 1"
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn catalog_config_merges_explicit_values_over_file_values() {
        let config = parse_config(
            r#"
            catalog_backend = "sqlite"
            catalog_path = "configured/catalog.sqlite"
            "#,
        );

        let backend = resolve_catalog_backend(Some("sqlite"), Some(&config))
            .expect("explicit catalog backend should resolve");
        let path = resolve_catalog_path(
            Path::new("project"),
            Some(PathBuf::from("explicit/catalog.db")),
            Some(&config),
        )
        .expect("explicit catalog path should resolve");

        assert_eq!(backend, CatalogBackend::Sqlite);
        assert_eq!(path, Some(PathBuf::from("explicit/catalog.db")));
    }

    #[test]
    fn catalog_config_uses_file_values_or_sqlite_defaults() {
        let config = parse_config(
            r#"
            catalog_backend = "sqlite"
            catalog_path = "configured/catalog.sqlite"
            "#,
        );

        let configured_backend = resolve_catalog_backend(None, Some(&config))
            .expect("configured catalog backend should resolve");
        let configured_path = resolve_catalog_path(Path::new("project"), None, Some(&config))
            .expect("configured catalog path should resolve");
        let default_backend =
            resolve_catalog_backend(None, None).expect("default catalog backend should resolve");

        assert_eq!(configured_backend, CatalogBackend::Sqlite);
        assert_eq!(
            configured_path,
            Some(PathBuf::from("project/configured/catalog.sqlite"))
        );
        assert_eq!(default_backend, CatalogBackend::Sqlite);
    }

    #[test]
    fn catalog_config_rejects_invalid_file_values() {
        let wrong_type = parse_config("catalog_backend = 123");
        let unsupported = parse_config("catalog_backend = \"postgres\"");

        let wrong_type_error = resolve_catalog_backend(None, Some(&wrong_type))
            .expect_err("non-string catalog backend should fail");
        let unsupported_error = resolve_catalog_backend(None, Some(&unsupported))
            .expect_err("unsupported catalog backend should fail");

        assert_eq!(
            wrong_type_error.to_string(),
            "config.toml catalog_backend must be a string"
        );
        assert_eq!(
            unsupported_error.to_string(),
            "unsupported catalog_backend: postgres"
        );
    }

    #[test]
    fn init_config_rejects_remote_data_path() {
        let root_path = std::env::temp_dir().join(format!("seex-config-{}", uuid::Uuid::new_v4()));

        let error = match resolve_init_config(
            &root_path,
            Some(PathBuf::from("s3://bucket/seex")),
            Some("sqlite"),
            None,
            1024,
        ) {
            Ok(_) => panic!("remote data paths must be rejected in the local-only store"),
            Err(error) => error,
        };

        assert_eq!(
            error.to_string(),
            "data_path must be a local filesystem path"
        );
    }
}
