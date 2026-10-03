use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::storage::sql::string_literal as sql_string_literal;
use crate::storage::{CatalogWriteGuard, StorageError};

const DUCKLAKE_ALIAS: &str = "dl";
const DUCKDB_CATALOG_ALIAS: &str = "seex_catalog";
const S3_SECRET_NAME: &str = "seex_s3";
const SQLITE_BUSY_TIMEOUT_MS: u64 = 30_000;
const SQLITE_JOURNAL_MODE: &str = "WAL";
const STORAGE_FORMAT_VERSION: i64 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeConnectionMode {
    ReadWrite,
    ReadWriteExisting,
    ReadOnly,
}

impl NativeConnectionMode {
    const fn must_exist(self) -> bool {
        !matches!(self, Self::ReadWrite)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CatalogBackend {
    DuckDb,
    Sqlite,
}

impl CatalogBackend {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "duckdb" => Some(Self::DuckDb),
            "sqlite" => Some(Self::Sqlite),
            _ => None,
        }
    }

    fn adapter(self) -> CatalogAdapter {
        match self {
            Self::DuckDb => CatalogAdapter::duckdb(),
            Self::Sqlite => CatalogAdapter::sqlite(),
        }
    }
}

#[derive(Clone)]
pub struct NativeStorageConfig {
    catalog_backend: CatalogBackend,
    catalog_path: PathBuf,
    data_path: PathBuf,
    s3_connection: Option<S3ConnectionConfig>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct NativeReadIdentity {
    catalog_backend: CatalogBackend,
    catalog_path: PathBuf,
    data_path: PathBuf,
}

pub(crate) struct NativeReadDatabase(Arc<Mutex<duckdb::Connection>>);

type NativeReadRegistry = HashMap<NativeReadIdentity, Weak<Mutex<duckdb::Connection>>>;

static NATIVE_READ_DATABASES: OnceLock<Mutex<NativeReadRegistry>> = OnceLock::new();

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S3ConnectionConfig {
    pub endpoint: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub region: Option<String>,
    pub path_style: Option<bool>,
    pub use_ssl: Option<bool>,
}

impl S3ConnectionConfig {
    pub fn new(
        endpoint: String,
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
        region: Option<String>,
        path_style: Option<bool>,
        use_ssl: Option<bool>,
    ) -> Self {
        Self {
            endpoint,
            access_key_id,
            secret_access_key,
            session_token,
            region,
            path_style,
            use_ssl,
        }
    }
}

struct CatalogAdapter {
    backend: CatalogBackend,
    default_catalog_filename: &'static str,
    ducklake_alias: &'static str,
    catalog_application_database: &'static str,
    ducklake_path_prefix: &'static str,
    attach_type_clause: Option<&'static str>,
}

impl CatalogAdapter {
    const fn duckdb() -> Self {
        Self {
            backend: CatalogBackend::DuckDb,
            default_catalog_filename: "catalog.ducklake",
            ducklake_alias: DUCKLAKE_ALIAS,
            catalog_application_database: DUCKDB_CATALOG_ALIAS,
            ducklake_path_prefix: "",
            attach_type_clause: Some("TYPE ducklake"),
        }
    }

    const fn sqlite() -> Self {
        Self {
            backend: CatalogBackend::Sqlite,
            default_catalog_filename: "catalog.sqlite",
            ducklake_alias: DUCKLAKE_ALIAS,
            catalog_application_database: DUCKDB_CATALOG_ALIAS,
            ducklake_path_prefix: "ducklake:sqlite:",
            attach_type_clause: None,
        }
    }

    fn attach_ducklake_statement(
        &self,
        catalog_path: &Path,
        data_path: &Path,
        mode: NativeConnectionMode,
    ) -> String {
        let catalog_uri = format!(
            "{}{}",
            self.ducklake_path_prefix,
            catalog_path.to_string_lossy()
        );
        let catalog_uri = sql_string_literal(&catalog_uri);
        let data_path = sql_string_literal(data_path.to_string_lossy().as_ref());
        let attach_type_clause = self
            .attach_type_clause
            .map(|clause| format!("                 {clause},\n"))
            .unwrap_or_default();
        let read_only_clause = match mode {
            NativeConnectionMode::ReadWrite | NativeConnectionMode::ReadWriteExisting => {
                String::new()
            }
            NativeConnectionMode::ReadOnly => "                 READ_ONLY,\n".to_owned(),
        };
        let metadata_parameters = match (self.backend, mode) {
            (CatalogBackend::DuckDb, _) => String::new(),
            (
                CatalogBackend::Sqlite,
                NativeConnectionMode::ReadWrite | NativeConnectionMode::ReadWriteExisting,
            ) => format!(
                "                 METADATA_PARAMETERS MAP {{'busy_timeout': \
                 '{SQLITE_BUSY_TIMEOUT_MS}', 'journal_mode': '{SQLITE_JOURNAL_MODE}'}},\n"
            ),
            (CatalogBackend::Sqlite, NativeConnectionMode::ReadOnly) => format!(
                "                 METADATA_PARAMETERS MAP {{'busy_timeout': \
                 '{SQLITE_BUSY_TIMEOUT_MS}'}},\n"
            ),
        };
        format!(
            "ATTACH {catalog_uri} AS {} (
{attach_type_clause}{read_only_clause}{metadata_parameters}                 DATA_PATH {data_path},
                 OVERRIDE_DATA_PATH true,
                 METADATA_CATALOG '{}'
             );",
            self.ducklake_alias, self.catalog_application_database
        )
    }

    fn setup_catalog_application_tables(
        &self,
        connection: &duckdb::Connection,
        _catalog_path: &Path,
    ) -> Result<(), StorageError> {
        connection
            .execute_batch(&format!("USE {};", self.catalog_application_database))
            .map_err(|source| StorageError::StorageDuckDb {
                operation: "selecting Seex catalog tables",
                name: self.catalog_application_database.to_owned(),
                source,
            })?;
        Ok(())
    }
}

impl NativeStorageConfig {
    pub fn duckdb(
        root_path: &Path,
        catalog_path: Option<PathBuf>,
        data_path: Option<PathBuf>,
    ) -> Self {
        Self::with_backend_and_s3_config(
            CatalogBackend::DuckDb,
            root_path,
            catalog_path,
            data_path,
            None,
        )
    }

    pub fn with_backend_and_s3_config(
        catalog_backend: CatalogBackend,
        root_path: &Path,
        catalog_path: Option<PathBuf>,
        data_path: Option<PathBuf>,
        s3_connection: Option<S3ConnectionConfig>,
    ) -> Self {
        let seex_path = root_path.join(".seex");
        let adapter = catalog_backend.adapter();
        Self {
            catalog_backend,
            catalog_path: catalog_path
                .unwrap_or_else(|| seex_path.join(adapter.default_catalog_filename)),
            data_path: data_path.unwrap_or_else(|| seex_path.join("data")),
            s3_connection,
        }
    }

    pub(crate) fn catalog_path(&self) -> &Path {
        self.catalog_path.as_path()
    }

    pub(crate) fn catalog_backend(&self) -> CatalogBackend {
        self.catalog_backend
    }
}

pub(crate) fn catalog_lock_namespace(catalog_path: &Path) -> Result<PathBuf, StorageError> {
    let catalog_path =
        std::fs::canonicalize(catalog_path).map_err(|source| StorageError::Storage {
            operation: "resolving catalog lock identity",
            name: path_basename(catalog_path),
            source,
        })?;
    let parent = catalog_path.parent().ok_or_else(|| StorageError::Storage {
        operation: "resolving catalog lock identity",
        name: path_basename(&catalog_path),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "catalog path has no parent",
        ),
    })?;
    let filename = catalog_path.file_name().unwrap_or_default();
    let conventional = parent.file_name().is_some_and(|name| name == ".seex")
        && matches!(
            filename.to_str(),
            Some("catalog.ducklake" | "catalog.sqlite")
        );
    if conventional {
        return Ok(parent.join("locks"));
    }
    let mut namespace = filename.to_os_string();
    namespace.push(".seex-locks");
    Ok(parent.join(namespace))
}

pub fn open_native_connection(root_path: &Path) -> Result<duckdb::Connection, StorageError> {
    open_native_connection_with_config(NativeStorageConfig::duckdb(root_path, None, None))
}

pub fn open_native_connection_with_config(
    config: NativeStorageConfig,
) -> Result<duckdb::Connection, StorageError> {
    open_native_connection_with_mode(config, NativeConnectionMode::ReadWrite)
}

pub fn open_existing_native_connection_with_config(
    config: NativeStorageConfig,
) -> Result<duckdb::Connection, StorageError> {
    open_native_connection_with_mode(config, NativeConnectionMode::ReadOnly)
}

pub(crate) fn open_existing_writer_connection_with_config(
    config: NativeStorageConfig,
) -> Result<duckdb::Connection, StorageError> {
    open_native_connection_with_mode(config, NativeConnectionMode::ReadWriteExisting)
}

pub(crate) fn open_existing_client_connection_with_config(
    config: NativeStorageConfig,
) -> Result<duckdb::Connection, StorageError> {
    let _guard =
        CatalogWriteGuard::acquire(&config.catalog_path, std::time::Duration::from_secs(30))?;
    let writer = open_existing_writer_connection_with_config(config.clone())?;
    flush_pending_in_locked_connection(&writer)?;
    repair_pending_series(&writer)?;
    drop(writer);
    open_existing_native_connection_with_config(config)
}

pub(crate) fn open_shared_existing_local_connection_with_config(
    config: NativeStorageConfig,
) -> Result<(duckdb::Connection, NativeReadDatabase), StorageError> {
    debug_assert!(!is_s3_data_path(&config.data_path));
    let identity = NativeReadIdentity {
        catalog_backend: config.catalog_backend,
        catalog_path: config.catalog_path.clone(),
        data_path: config.data_path.clone(),
    };
    let databases = NATIVE_READ_DATABASES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut databases = databases.lock().unwrap_or_else(|error| error.into_inner());
    let owner = match databases.get(&identity).and_then(Weak::upgrade) {
        Some(owner) => owner,
        None => {
            let connection =
                open_native_connection_with_mode(config, NativeConnectionMode::ReadOnly)?;
            connection.execute_batch("SET enable_external_file_cache=false;")?;
            let owner = Arc::new(Mutex::new(connection));
            databases.insert(identity, Arc::downgrade(&owner));
            owner
        }
    };
    drop(databases);
    let connection = owner
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .try_clone()?;
    connection.execute_batch("USE seex_catalog;")?;
    Ok((connection, NativeReadDatabase(owner)))
}

fn open_native_connection_with_mode(
    config: NativeStorageConfig,
    mode: NativeConnectionMode,
) -> Result<duckdb::Connection, StorageError> {
    let connection_started = std::time::Instant::now();
    let must_exist = mode.must_exist();
    let _catalog_guard = if must_exist {
        None
    } else {
        Some(CatalogWriteGuard::acquire(
            &config.catalog_path,
            std::time::Duration::from_secs(30),
        )?)
    };
    let catalog_preexisted = config.catalog_path.exists();
    if must_exist {
        verify_catalog_exists(&config.catalog_path)?;
    } else if let Some(catalog_parent) = config.catalog_path.parent() {
        std::fs::create_dir_all(catalog_parent).map_err(|source| StorageError::Storage {
            operation: "creating catalog directory",
            name: path_basename(catalog_parent),
            source,
        })?;
    }
    if !must_exist && !is_s3_data_path(&config.data_path) {
        std::fs::create_dir_all(&config.data_path).map_err(|source| StorageError::Storage {
            operation: "creating data directory",
            name: path_basename(&config.data_path),
            source,
        })?;
    }
    let connection = open_duckdb_connection()?;
    if is_s3_data_path(&config.data_path)
        && let Some(s3_connection) = config.s3_connection.as_ref()
    {
        configure_s3_connection(&connection, s3_connection)?;
    }
    attach_ducklake_with_backend_and_mode(
        &connection,
        config.catalog_backend,
        &config.catalog_path,
        &config.data_path,
        mode,
    )?;
    setup_catalog_adapter(&connection, config.catalog_backend, &config.catalog_path)?;
    if !matches!(mode, NativeConnectionMode::ReadOnly) {
        record_write_timing("connection_attach", connection_started.elapsed());
    }
    if !must_exist && !catalog_preexisted {
        initialize_storage_tables(&connection)?;
    } else {
        validate_storage_format(&connection)?;
    }
    if matches!(mode, NativeConnectionMode::ReadWrite) {
        flush_pending_in_locked_connection(&connection)?;
        repair_pending_series(&connection)?;
    }
    Ok(connection)
}

fn repair_pending_series(connection: &duckdb::Connection) -> Result<(), StorageError> {
    let mut pending = connection
        .prepare("SELECT run_id, metric_key FROM seex_series_directory WHERE status = 'pending'")?;
    let rows = pending.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    drop(pending);
    for (run_id, metric_key) in rows {
        let exists: bool = connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM dl.metric_points WHERE run_id = ? AND metric_key = ?)",
            [&run_id, &metric_key],
            |row| row.get(0),
        )?;
        if exists {
            connection.execute(
                "UPDATE seex_series_directory SET status = 'published' WHERE run_id = ? AND metric_key = ?",
                [&run_id, &metric_key],
            )?;
        } else {
            connection.execute(
                "DELETE FROM seex_series_directory WHERE run_id = ? AND metric_key = ?",
                [&run_id, &metric_key],
            )?;
            connection.execute(
                "DELETE FROM seex_metric_keys WHERE metric_key = ? AND NOT EXISTS (
                     SELECT 1 FROM seex_series_directory WHERE metric_key = ?
                 )",
                [&metric_key, &metric_key],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn record_write_timing(stage: &str, elapsed: std::time::Duration) {
    if std::env::var_os("SEEX_WRITE_TIMING").is_some() {
        eprintln!(
            "seex_write_timing,pid={},stage={stage},elapsed_ns={}",
            std::process::id(),
            elapsed.as_nanos()
        );
    }
}

/// Retries a pending terminal flush while the caller holds the catalog write lock.
pub(crate) fn flush_pending_in_locked_connection(
    connection: &duckdb::Connection,
) -> Result<bool, StorageError> {
    let pending: i64 = connection.query_row(
        "SELECT value FROM seex_storage_metadata WHERE key = 'flush_pending'",
        [],
        |row| row.get(0),
    )?;
    if pending == 0 {
        return Ok(false);
    }
    connection
        .execute_batch("CALL ducklake_flush_inlined_data('dl', table_name => 'metric_points');")?;
    connection.execute(
        "UPDATE seex_storage_metadata SET value = 0 WHERE key = 'flush_pending'",
        [],
    )?;
    Ok(true)
}

fn verify_catalog_exists(catalog_path: &Path) -> Result<(), StorageError> {
    match std::fs::metadata(catalog_path) {
        Ok(_) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Err(StorageError::CatalogNotFound {
                name: path_basename(catalog_path),
            })
        }
        Err(source) => Err(StorageError::Storage {
            operation: "checking catalog",
            name: path_basename(catalog_path),
            source,
        }),
    }
}

pub fn attach_ducklake(
    connection: &duckdb::Connection,
    catalog_path: &Path,
    data_path: &Path,
) -> Result<(), StorageError> {
    attach_ducklake_with_backend(connection, CatalogBackend::DuckDb, catalog_path, data_path)
}

pub fn attach_ducklake_with_backend(
    connection: &duckdb::Connection,
    catalog_backend: CatalogBackend,
    catalog_path: &Path,
    data_path: &Path,
) -> Result<(), StorageError> {
    attach_ducklake_with_backend_and_mode(
        connection,
        catalog_backend,
        catalog_path,
        data_path,
        NativeConnectionMode::ReadWrite,
    )
}

fn attach_ducklake_with_backend_and_mode(
    connection: &duckdb::Connection,
    catalog_backend: CatalogBackend,
    catalog_path: &Path,
    data_path: &Path,
    mode: NativeConnectionMode,
) -> Result<(), StorageError> {
    let storage_name = format!(
        "{}, {}",
        path_basename(catalog_path),
        path_basename(data_path)
    );
    let adapter = catalog_backend.adapter();
    connection
        .execute_batch(&format!(
            "INSTALL ducklake;
         LOAD ducklake;
         {}",
            adapter.attach_ducklake_statement(catalog_path, data_path, mode)
        ))
        .map_err(|source| StorageError::StorageDuckDb {
            operation: "attaching DuckLake catalog",
            name: storage_name,
            source,
        })?;
    Ok(())
}

pub fn setup_catalog_adapter(
    connection: &duckdb::Connection,
    catalog_backend: CatalogBackend,
    catalog_path: &Path,
) -> Result<(), StorageError> {
    catalog_backend
        .adapter()
        .setup_catalog_application_tables(connection, catalog_path)
}

pub fn setup_duckdb_catalog_adapter(
    connection: &duckdb::Connection,
    catalog_path: &Path,
) -> Result<(), StorageError> {
    setup_catalog_adapter(connection, CatalogBackend::DuckDb, catalog_path)
}

fn open_duckdb_connection() -> Result<duckdb::Connection, StorageError> {
    let mut config = duckdb::Config::default();
    if std::env::var_os("SEEX_LTTB_EXTENSION_PATH").is_some() {
        config = config.allow_unsigned_extensions()?;
    }
    Ok(duckdb::Connection::open_in_memory_with_flags(config)?)
}

fn configure_s3_connection(
    connection: &duckdb::Connection,
    config: &S3ConnectionConfig,
) -> Result<(), StorageError> {
    connection
        .execute_batch("INSTALL httpfs; LOAD httpfs;")
        .map_err(|source| StorageError::StorageDuckDb {
            operation: "loading DuckDB HTTPFS extension",
            name: "s3 data path".to_owned(),
            source,
        })?;

    connection
        .execute_batch(&create_s3_secret_statement(config))
        .map_err(|source| StorageError::StorageDuckDb {
            operation: "configuring DuckDB S3 secret",
            name: "s3 data path".to_owned(),
            source,
        })?;
    Ok(())
}

fn create_s3_secret_statement(config: &S3ConnectionConfig) -> String {
    let mut options = vec![
        "TYPE s3".to_owned(),
        format!("KEY_ID {}", sql_string_literal(&config.access_key_id)),
        format!("SECRET {}", sql_string_literal(&config.secret_access_key)),
        format!("ENDPOINT {}", sql_string_literal(&config.endpoint)),
    ];
    if let Some(session_token) = config.session_token.as_ref() {
        options.push(format!(
            "SESSION_TOKEN {}",
            sql_string_literal(session_token)
        ));
    }
    if let Some(region) = config.region.as_ref() {
        options.push(format!("REGION {}", sql_string_literal(region)));
    }
    if let Some(path_style) = config.path_style {
        let url_style = if path_style { "path" } else { "vhost" };
        options.push(format!("URL_STYLE {}", sql_string_literal(url_style)));
    }
    if let Some(use_ssl) = config.use_ssl {
        options.push(format!("USE_SSL {}", sql_boolean_literal(use_ssl)));
    }
    format!(
        "CREATE OR REPLACE SECRET {S3_SECRET_NAME} (
             {}
         );",
        options.join(",\n             ")
    )
}

pub fn initialize_storage_tables(connection: &duckdb::Connection) -> Result<(), StorageError> {
    connection.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS seex_projects (
             project_id VARCHAR NOT NULL,
             name VARCHAR NOT NULL,
             created_at TIMESTAMPTZ NOT NULL
         );
         CREATE TABLE IF NOT EXISTS seex_runs (
             run_id VARCHAR NOT NULL,
             project_id VARCHAR NOT NULL,
             name VARCHAR NOT NULL,
             status VARCHAR NOT NULL,
             created_at TIMESTAMPTZ NOT NULL,
             started_at TIMESTAMPTZ NOT NULL,
             finished_at TIMESTAMPTZ
         );
         CREATE TABLE IF NOT EXISTS dl.metric_points (
             run_id VARCHAR NOT NULL,
             metric_key VARCHAR NOT NULL,
             metric_key_encoded VARCHAR NOT NULL,
             step BIGINT NOT NULL,
             timestamp TIMESTAMPTZ NOT NULL,
             value_f64 DOUBLE NOT NULL,
             ingested_at TIMESTAMPTZ NOT NULL
         );
         ALTER TABLE dl.metric_points SET PARTITIONED BY (run_id, metric_key_encoded);
         CALL dl.set_option(
             'data_inlining_row_limit',
             8192,
             table_name => 'metric_points'
         );
         CREATE TABLE IF NOT EXISTS seex_metric_aggregates (
             run_id VARCHAR NOT NULL,
             metric_key VARCHAR NOT NULL,
             effective_count UBIGINT NOT NULL,
             last_step BIGINT NOT NULL,
             last_value_f64 DOUBLE NOT NULL,
             min_value_f64 DOUBLE NOT NULL,
             max_value_f64 DOUBLE NOT NULL
         );
         CREATE TABLE IF NOT EXISTS seex_storage_metadata (
             key VARCHAR PRIMARY KEY,
             value BIGINT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS seex_runs_project_cursor
             ON seex_runs(project_id, created_at, run_id);
         CREATE UNIQUE INDEX IF NOT EXISTS seex_projects_id
             ON seex_projects(project_id);
         CREATE UNIQUE INDEX IF NOT EXISTS seex_runs_id
             ON seex_runs(run_id);
         CREATE UNIQUE INDEX IF NOT EXISTS seex_aggregates_run_key
             ON seex_metric_aggregates(run_id, metric_key);
         CREATE TABLE IF NOT EXISTS seex_metric_keys (
             metric_key VARCHAR PRIMARY KEY
         );
         CREATE TABLE IF NOT EXISTS seex_series_directory (
             run_id VARCHAR NOT NULL,
             metric_key VARCHAR NOT NULL,
             status VARCHAR NOT NULL,
             PRIMARY KEY (run_id, metric_key)
         );
         CREATE INDEX IF NOT EXISTS seex_series_pending
             ON seex_series_directory(status);
         CREATE INDEX IF NOT EXISTS seex_series_key
             ON seex_series_directory(metric_key, run_id);
         INSERT INTO seex_storage_metadata (key, value)
         SELECT 'format_version', {STORAGE_FORMAT_VERSION}
         WHERE NOT EXISTS (
             SELECT 1 FROM seex_storage_metadata WHERE key = 'format_version'
         );
         INSERT INTO seex_storage_metadata (key, value)
         SELECT 'flush_pending', 0
         WHERE NOT EXISTS (
             SELECT 1 FROM seex_storage_metadata WHERE key = 'flush_pending'
         );",
    ))?;
    Ok(())
}

fn validate_storage_format(connection: &duckdb::Connection) -> Result<(), StorageError> {
    let version = connection
        .query_row(
            "SELECT value FROM seex_storage_metadata WHERE key = 'format_version'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|source| StorageError::StorageDuckDb {
            operation: "reading native storage format marker",
            name: "seex_storage_metadata".to_owned(),
            source,
        })?;
    if version != STORAGE_FORMAT_VERSION {
        return Err(StorageError::StorageFormatMismatch {
            expected: STORAGE_FORMAT_VERSION,
            found: version,
        });
    }
    connection
        .query_row(
            "SELECT value FROM seex_storage_metadata WHERE key = 'flush_pending'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|source| StorageError::StorageDuckDb {
            operation: "reading native flush recovery marker",
            name: "seex_storage_metadata".to_owned(),
            source,
        })?;
    Ok(())
}

fn sql_boolean_literal(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn path_basename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("storage path")
        .to_owned()
}

pub fn is_s3_data_path(path: &Path) -> bool {
    path.to_string_lossy().starts_with("s3://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_creates_project_run_and_sequence_indexes() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let config = NativeStorageConfig::with_backend_and_s3_config(
            CatalogBackend::Sqlite,
            root.path(),
            None,
            None,
            None,
        );
        let connection = open_native_connection_with_config(config.clone())?;
        let catalog = sql_string_literal(config.catalog_path.to_string_lossy().as_ref());
        let index_count: i64 = connection.query_row(
            &format!(
                "SELECT count(*) FROM sqlite_scan({catalog}, 'sqlite_master')
             WHERE type = 'index' AND name IN (
                 'seex_projects_id', 'seex_runs_id', 'seex_runs_project_cursor',
                 'seex_series_pending', 'seex_series_key', 'seex_aggregates_run_key'
             )"
            ),
            [],
            |row| row.get(0),
        )?;
        assert_eq!(index_count, 6);
        Ok(())
    }

    #[test]
    fn existing_connection_rejects_missing_catalog_without_creating_store() {
        let root_path = std::env::temp_dir().join(format!("seex-missing-{}", uuid::Uuid::new_v4()));
        let config = NativeStorageConfig::duckdb(&root_path, None, None);

        let error = open_existing_native_connection_with_config(config)
            .expect_err("missing catalog should fail");

        assert!(
            matches!(error, StorageError::CatalogNotFound { .. }),
            "expected missing catalog error, got {error:?}",
        );
        assert_eq!(error.to_string(), "catalog not found: catalog.ducklake");
        assert!(!root_path.exists());
    }

    #[test]
    fn existing_connection_rejects_catalog_without_storage_format_marker()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config = NativeStorageConfig::duckdb(root.path(), None, None);
        drop(open_native_connection_with_config(config.clone())?);
        let connection = open_duckdb_connection()?;
        attach_ducklake_with_backend(
            &connection,
            CatalogBackend::DuckDb,
            config.catalog_path(),
            &config.data_path,
        )?;
        connection.execute_batch("DROP TABLE seex_catalog.seex_storage_metadata;")?;
        drop(connection);

        let error = open_existing_native_connection_with_config(config).unwrap_err();
        assert!(error.to_string().contains("native storage format marker"));

        let writer_error = open_native_connection_with_config(NativeStorageConfig::duckdb(
            root.path(),
            None,
            None,
        ))
        .expect_err("writers must reject stores without a format marker");
        assert!(
            writer_error
                .to_string()
                .contains("native storage format marker")
        );
        Ok(())
    }

    #[test]
    fn older_stores_are_rejected_by_readers_and_writers() -> Result<(), Box<dyn std::error::Error>>
    {
        for version in [1, 2] {
            let root = tempfile::tempdir()?;
            let config = NativeStorageConfig::duckdb(root.path(), None, None);
            let connection = open_native_connection_with_config(config.clone())?;
            connection.execute(
                "UPDATE seex_storage_metadata SET value = ? WHERE key = 'format_version'",
                [version],
            )?;
            drop(connection);

            for error in [
                open_existing_native_connection_with_config(config.clone()).unwrap_err(),
                open_native_connection_with_config(config).unwrap_err(),
            ] {
                assert!(
                    matches!(
                        error,
                        StorageError::StorageFormatMismatch {
                            expected: 3,
                            found
                        } if found == version
                    ),
                    "unexpected format error: {error:?}",
                );
            }
        }
        Ok(())
    }

    #[test]
    fn writer_open_skips_flush_without_pending_marker() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config = NativeStorageConfig::duckdb(root.path(), None, None);
        drop(open_native_connection_with_config(config.clone())?);
        let metric_points_path = config.data_path.join("main").join("metric_points");
        std::fs::create_dir_all(metric_points_path.parent().expect("metric_points parent"))?;
        std::fs::write(&metric_points_path, b"flush must not run")?;

        drop(open_native_connection_with_config(config)?);
        Ok(())
    }

    #[test]
    fn local_readers_share_only_live_matching_database_handles()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config = || NativeStorageConfig::duckdb(root.path(), None, None);
        drop(open_native_connection_with_config(config())?);

        let (first, first_owner) = open_shared_existing_local_connection_with_config(config())?;
        first.execute_batch(
            "CREATE TABLE memory.shared_reader_probe(value INTEGER);\
             INSERT INTO memory.shared_reader_probe VALUES (1);",
        )?;
        let (second, second_owner) = open_shared_existing_local_connection_with_config(config())?;
        let shared_count: i64 = second.query_row(
            "SELECT count(*) FROM memory.shared_reader_probe",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(shared_count, 1);

        drop((first, first_owner, second, second_owner));
        let (fresh, _fresh_owner) = open_shared_existing_local_connection_with_config(config())?;
        let fresh_query = fresh.query_row(
            "SELECT count(*) FROM memory.shared_reader_probe",
            [],
            |row| row.get::<_, i64>(0),
        );
        assert!(
            fresh_query.is_err(),
            "expired host database must not be reused"
        );
        Ok(())
    }

    #[test]
    fn local_shared_readers_disable_external_file_cache_without_changing_normal_connections()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config = || NativeStorageConfig::duckdb(root.path(), None, None);
        let writer = open_native_connection_with_config(config())?;
        let writer_cache_enabled: bool = writer.query_row(
            "SELECT current_setting('enable_external_file_cache')",
            [],
            |row| row.get(0),
        )?;
        assert!(writer_cache_enabled);
        writer.execute_batch(
            "INSERT INTO dl.metric_points
                 SELECT 'run', 'metric', 'metric', range, now(), range::DOUBLE, now()
                 FROM range(9000);",
        )?;
        drop(writer);

        let (reader, _owner) = open_shared_existing_local_connection_with_config(config())?;
        let reader_cache_enabled: bool = reader.query_row(
            "SELECT current_setting('enable_external_file_cache')",
            [],
            |row| row.get(0),
        )?;
        assert!(!reader_cache_enabled);
        for _ in 0..2 {
            let point_count: i64 = reader.query_row(
                "SELECT count(*) FROM dl.metric_points WHERE metric_key = 'metric'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(point_count, 9000);
            let cached_files: i64 = reader.query_row(
                "SELECT count(*) FROM duckdb_external_file_cache()",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(cached_files, 0);
        }
        Ok(())
    }

    #[test]
    fn attach_ducklake_sanitizes_storage_error_paths() -> Result<(), Box<dyn std::error::Error>> {
        let root_path =
            std::env::temp_dir().join(format!("seex-bootstrap-{}", uuid::Uuid::new_v4()));
        let catalog_path = root_path.join("private").join("catalog.ducklake");
        let data_path = root_path.join("secret-data");
        std::fs::create_dir_all(&catalog_path)?;
        std::fs::create_dir_all(&data_path)?;
        let connection = open_duckdb_connection()?;

        let error = attach_ducklake(&connection, &catalog_path, &data_path).unwrap_err();
        let message = error.to_string();

        assert!(
            matches!(error, StorageError::StorageDuckDb { .. }),
            "expected sanitized storage error, got {error:?}",
        );
        assert!(
            message.contains("attaching DuckLake catalog"),
            "expected operation in error message, got {message}",
        );
        assert!(
            message.contains("catalog.ducklake") && message.contains("secret-data"),
            "expected storage basenames in error message, got {message}",
        );
        assert!(
            !message.contains(root_path.to_string_lossy().as_ref()),
            "expected sanitized message without full path, got {message}",
        );
        std::fs::remove_dir_all(root_path)?;
        Ok(())
    }

    #[test]
    fn catalog_adapter_uses_duckdb_default_catalog_filename() {
        let adapter = CatalogAdapter::duckdb();

        assert_eq!(adapter.default_catalog_filename, "catalog.ducklake");
    }

    #[test]
    fn catalog_adapter_builds_ducklake_attach_statement() {
        let adapter = CatalogAdapter::duckdb();
        let statement = adapter.attach_ducklake_statement(
            Path::new("catalog.ducklake"),
            Path::new("data"),
            NativeConnectionMode::ReadWrite,
        );
        let read_only = adapter.attach_ducklake_statement(
            Path::new("catalog.ducklake"),
            Path::new("data"),
            NativeConnectionMode::ReadOnly,
        );

        assert!(statement.contains("ATTACH 'catalog.ducklake' AS dl"));
        assert!(statement.contains("TYPE ducklake"));
        assert!(statement.contains("DATA_PATH 'data'"));
        assert!(statement.contains("OVERRIDE_DATA_PATH true"));
        assert!(statement.contains("METADATA_CATALOG 'seex_catalog'"));
        assert!(!statement.contains("READ_ONLY"));
        assert!(read_only.contains("READ_ONLY"));
        assert!(!read_only.contains("METADATA_PARAMETERS"));
    }

    #[test]
    fn sqlite_attach_options_separate_writer_and_reader_capabilities() {
        let adapter = CatalogAdapter::sqlite();
        let writer = adapter.attach_ducklake_statement(
            Path::new("catalog.sqlite"),
            Path::new("data"),
            NativeConnectionMode::ReadWrite,
        );
        let reader = adapter.attach_ducklake_statement(
            Path::new("catalog.sqlite"),
            Path::new("data"),
            NativeConnectionMode::ReadOnly,
        );

        assert!(writer.contains("'busy_timeout': '30000'"));
        assert!(writer.contains("'journal_mode': 'WAL'"));
        assert!(!writer.contains("READ_ONLY"));
        assert!(reader.contains("READ_ONLY"));
        assert!(reader.contains("'busy_timeout': '30000'"));
        assert!(!reader.contains("journal_mode"));
    }

    fn sqlite_journal_mode(catalog_path: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let header = std::fs::read(catalog_path)?;
        match header.get(18..20) {
            Some([2, 2]) => Ok("wal".to_owned()),
            Some([1, 1]) => Ok("delete".to_owned()),
            value => Err(format!("unexpected SQLite journal header: {value:?}").into()),
        }
    }

    fn create_legacy_sqlite_catalog(root_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let seex_path = root_path.join(".seex");
        let catalog_path = seex_path.join("catalog.sqlite");
        let data_path = seex_path.join("data");
        std::fs::create_dir_all(&data_path)?;
        let connection = open_duckdb_connection()?;
        let catalog_uri = format!("ducklake:sqlite:{}", catalog_path.to_string_lossy());
        connection.execute_batch(&format!(
            "INSTALL ducklake; LOAD ducklake;
             ATTACH {} AS dl (
                 DATA_PATH {},
                 OVERRIDE_DATA_PATH true,
                 METADATA_CATALOG 'seex_catalog'
             );",
            sql_string_literal(&catalog_uri),
            sql_string_literal(data_path.to_string_lossy().as_ref()),
        ))?;
        setup_catalog_adapter(&connection, CatalogBackend::Sqlite, &catalog_path)?;
        initialize_storage_tables(&connection)?;
        Ok(())
    }

    #[test]
    fn sqlite_writer_migrates_wal_and_existing_connection_is_read_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let new_root = tempfile::tempdir()?;
        let new_catalog_path = new_root.path().join(".seex/catalog.sqlite");
        let new_config = || {
            NativeStorageConfig::with_backend_and_s3_config(
                CatalogBackend::Sqlite,
                new_root.path(),
                None,
                None,
                None,
            )
        };

        drop(open_native_connection_with_config(new_config())?);
        assert_eq!(sqlite_journal_mode(&new_catalog_path)?, "wal");

        let reader = open_existing_native_connection_with_config(new_config())?;
        let read_only: bool = reader.query_row(
            "SELECT readonly FROM duckdb_databases() WHERE database_name = 'dl'",
            [],
            |row| row.get(0),
        )?;
        let write_error = reader
            .execute_batch("CREATE TABLE dl.reader_must_not_write(value INTEGER)")
            .expect_err("native Reader connection should reject writes");

        assert!(read_only);
        assert!(write_error.to_string().contains("read-only"));

        let legacy_root = tempfile::tempdir()?;
        let legacy_catalog_path = legacy_root.path().join(".seex/catalog.sqlite");
        create_legacy_sqlite_catalog(legacy_root.path())?;
        assert_eq!(sqlite_journal_mode(&legacy_catalog_path)?, "delete");
        let legacy_config = || {
            NativeStorageConfig::with_backend_and_s3_config(
                CatalogBackend::Sqlite,
                legacy_root.path(),
                None,
                None,
                None,
            )
        };
        let legacy_reader = open_existing_native_connection_with_config(legacy_config())?;
        let project_count: i64 = legacy_reader.query_row(
            "SELECT count(*) FROM seex_catalog.seex_projects",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(project_count, 0);
        drop(legacy_reader);
        assert_eq!(sqlite_journal_mode(&legacy_catalog_path)?, "delete");

        drop(open_native_connection_with_config(legacy_config())?);
        assert_eq!(sqlite_journal_mode(&legacy_catalog_path)?, "wal");
        Ok(())
    }

    #[test]
    fn absolute_root_reopens_catalog_created_from_relative_root()
    -> Result<(), Box<dyn std::error::Error>> {
        struct RemoveTestDir(PathBuf);

        impl Drop for RemoveTestDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let relative_root =
            PathBuf::from("target").join(format!("seex-relative-root-{}", uuid::Uuid::new_v4()));
        let _cleanup = RemoveTestDir(relative_root.clone());
        let connection = open_native_connection(&relative_root)?;
        drop(connection);
        let absolute_root = std::fs::canonicalize(&relative_root)?;

        let connection = open_existing_native_connection_with_config(NativeStorageConfig::duckdb(
            &absolute_root,
            None,
            None,
        ))?;
        let project_count: i64 = connection.query_row(
            "SELECT count(*) FROM seex_catalog.seex_projects",
            [],
            |row| row.get(0),
        )?;

        assert_eq!(project_count, 0);
        Ok(())
    }

    #[test]
    fn catalog_adapters_accept_s3_data_path() {
        let adapters = [CatalogAdapter::duckdb(), CatalogAdapter::sqlite()];

        for adapter in adapters {
            let statement = adapter.attach_ducklake_statement(
                Path::new("catalog.ducklake"),
                Path::new("s3://bucket/prefix"),
                NativeConnectionMode::ReadWrite,
            );

            assert!(statement.contains("DATA_PATH 's3://bucket/prefix'"));
        }
    }

    #[test]
    fn local_data_path_does_not_configure_httpfs_or_s3_secret()
    -> Result<(), Box<dyn std::error::Error>> {
        let root_path =
            std::env::temp_dir().join(format!("seex-bootstrap-{}", uuid::Uuid::new_v4()));
        let config = NativeStorageConfig::with_backend_and_s3_config(
            CatalogBackend::DuckDb,
            &root_path,
            None,
            None,
            Some(s3_test_config()),
        );
        let connection = open_native_connection_with_config(config)?;

        let httpfs_loaded: bool = connection.query_row(
            "SELECT loaded
             FROM duckdb_extensions()
             WHERE extension_name = 'httpfs'",
            [],
            |row| row.get(0),
        )?;
        let secret_count: i64 =
            connection.query_row("SELECT count(*) FROM duckdb_secrets()", [], |row| {
                row.get(0)
            })?;

        assert!(!httpfs_loaded);
        assert_eq!(secret_count, 0);
        std::fs::remove_dir_all(root_path)?;
        Ok(())
    }

    #[test]
    fn configure_s3_connection_creates_temporary_redacted_secret()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = open_duckdb_connection()?;
        let config = s3_test_config();

        configure_s3_connection(&connection, &config)?;

        let (name, secret_type, persistent, storage, secret_string): (
            String,
            String,
            bool,
            String,
            String,
        ) = connection.query_row(
            "SELECT name, type, persistent, storage, secret_string
             FROM duckdb_secrets()",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;

        assert_eq!(name, S3_SECRET_NAME);
        assert_eq!(secret_type, "s3");
        assert!(!persistent);
        assert_eq!(storage, "memory");
        assert!(secret_string.contains("endpoint=127.0.0.1:9000"));
        assert!(secret_string.contains("key_id=seex-key"));
        assert!(secret_string.contains("region=us-east-1"));
        assert!(secret_string.contains("secret=redacted"));
        assert!(secret_string.contains("session_token=redacted"));
        assert!(secret_string.contains("url_style=path"));
        assert!(secret_string.contains("use_ssl=false"));
        assert!(!secret_string.contains("seex-secret"));
        assert!(!secret_string.contains("seex-session"));
        Ok(())
    }

    fn s3_test_config() -> S3ConnectionConfig {
        S3ConnectionConfig::new(
            "127.0.0.1:9000".to_owned(),
            "seex-key".to_owned(),
            "seex-secret".to_owned(),
            Some("seex-session".to_owned()),
            Some("us-east-1".to_owned()),
            Some(true),
            Some(false),
        )
    }

    #[test]
    fn initialize_storage_tables_partitions_metric_points_by_run_and_encoded_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let root_path =
            std::env::temp_dir().join(format!("seex-bootstrap-{}", uuid::Uuid::new_v4()));
        let connection = open_native_connection(&root_path)?;

        let partition_columns: Vec<String> = connection
            .prepare(
                "SELECT columns.column_name
                 FROM seex_catalog.ducklake_partition_column AS partitions
                 JOIN seex_catalog.ducklake_column AS columns
                   ON columns.column_id = partitions.column_id
                 JOIN seex_catalog.ducklake_table AS tables
                   ON tables.table_id = columns.table_id
                 WHERE tables.table_name = 'metric_points'
                 ORDER BY partitions.partition_key_index",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;

        assert_eq!(partition_columns, ["run_id", "metric_key_encoded"]);
        std::fs::remove_dir_all(root_path)?;
        Ok(())
    }

    #[test]
    fn initialize_storage_tables_sets_metric_points_inlining_row_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let root_path =
            std::env::temp_dir().join(format!("seex-bootstrap-{}", uuid::Uuid::new_v4()));
        let connection = open_native_connection(&root_path)?;

        let inlining_row_limit: i64 = connection.query_row(
            "SELECT CAST(value AS BIGINT)
             FROM seex_catalog.ducklake_metadata AS metadata
             JOIN seex_catalog.ducklake_table AS tables
               ON tables.table_id = metadata.scope_id
             WHERE tables.table_name = 'metric_points'
               AND metadata.scope = 'table'
               AND metadata.key = 'data_inlining_row_limit'",
            [],
            |row| row.get(0),
        )?;

        assert_eq!(inlining_row_limit, 8192);
        std::fs::remove_dir_all(root_path)?;
        Ok(())
    }
}
