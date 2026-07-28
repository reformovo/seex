use std::path::{Path, PathBuf};

use crate::StorageError;
use crate::sql::string_literal as sql_string_literal;

const DUCKLAKE_ALIAS: &str = "dl";
const DUCKDB_CATALOG_ALIAS: &str = "seex_catalog";
const S3_SECRET_NAME: &str = "seex_s3";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

pub struct NativeStorageConfig {
    catalog_backend: CatalogBackend,
    catalog_path: PathBuf,
    data_path: PathBuf,
    s3_connection: Option<S3ConnectionConfig>,
}

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
    default_catalog_filename: &'static str,
    ducklake_alias: &'static str,
    catalog_application_database: &'static str,
    ducklake_path_prefix: &'static str,
    attach_type_clause: Option<&'static str>,
}

impl CatalogAdapter {
    const fn duckdb() -> Self {
        Self {
            default_catalog_filename: "catalog.ducklake",
            ducklake_alias: DUCKLAKE_ALIAS,
            catalog_application_database: DUCKDB_CATALOG_ALIAS,
            ducklake_path_prefix: "",
            attach_type_clause: Some("TYPE ducklake"),
        }
    }

    const fn sqlite() -> Self {
        Self {
            default_catalog_filename: "catalog.sqlite",
            ducklake_alias: DUCKLAKE_ALIAS,
            catalog_application_database: DUCKDB_CATALOG_ALIAS,
            ducklake_path_prefix: "ducklake:sqlite:",
            attach_type_clause: None,
        }
    }

    fn attach_ducklake_statement(&self, catalog_path: &Path, data_path: &Path) -> String {
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
        format!(
            "ATTACH {catalog_uri} AS {} (
{attach_type_clause}                 DATA_PATH {data_path},
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
}

pub fn open_native_connection(root_path: &Path) -> Result<duckdb::Connection, StorageError> {
    open_native_connection_with_config(NativeStorageConfig::duckdb(root_path, None, None))
}

pub fn open_native_connection_with_config(
    config: NativeStorageConfig,
) -> Result<duckdb::Connection, StorageError> {
    open_native_connection_with_mode(config, false)
}

pub fn open_existing_native_connection_with_config(
    config: NativeStorageConfig,
) -> Result<duckdb::Connection, StorageError> {
    open_native_connection_with_mode(config, true)
}

fn open_native_connection_with_mode(
    config: NativeStorageConfig,
    must_exist: bool,
) -> Result<duckdb::Connection, StorageError> {
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
    attach_ducklake_with_backend(
        &connection,
        config.catalog_backend,
        &config.catalog_path,
        &config.data_path,
    )?;
    setup_catalog_adapter(&connection, config.catalog_backend, &config.catalog_path)?;
    if !must_exist {
        create_v1_tables(&connection)?;
    }
    Ok(connection)
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
            adapter.attach_ducklake_statement(catalog_path, data_path)
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

pub fn create_v1_tables(connection: &duckdb::Connection) -> Result<(), StorageError> {
    connection.execute_batch(
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
         );",
    )?;
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
        let statement =
            adapter.attach_ducklake_statement(Path::new("catalog.ducklake"), Path::new("data"));

        assert!(statement.contains("ATTACH 'catalog.ducklake' AS dl"));
        assert!(statement.contains("TYPE ducklake"));
        assert!(statement.contains("DATA_PATH 'data'"));
        assert!(statement.contains("OVERRIDE_DATA_PATH true"));
        assert!(statement.contains("METADATA_CATALOG 'seex_catalog'"));
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
    fn create_v1_tables_partitions_metric_points_by_run_and_encoded_key()
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
    fn create_v1_tables_sets_metric_points_inlining_row_limit()
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
