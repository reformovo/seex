//! One-time preparation for the retained schema-v3 Reader benchmark dataset.

use std::env;
use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};

use seex_core::engine::client::NativeClient;
use seex_model::run::RunId;
use seex_model::types::ProjectId;
use seex_storage::ProjectConnection;
use seex_storage::bootstrap::{
    CatalogBackend, NativeStorageConfig, open_native_connection_with_config,
};

const POINTS: i64 = 1_000_000;
const EPOCH_MILLIS: i64 = 1_700_000_000_000;
const MANIFEST_PATH: &str = ".seex/reader-benchmark.txt";

fn dataset_manifest(backend: CatalogBackend) -> &'static str {
    match backend {
        CatalogBackend::DuckDb => {
            "schema=v3\nbackend=duckdb\nruns=1\nmetrics=1\npoints_per_series=1000000\n"
        }
        CatalogBackend::Sqlite => {
            "schema=v3\nbackend=sqlite\nruns=1\nmetrics=1\npoints_per_series=1000000\n"
        }
    }
}

fn reuse_complete_dataset(root: &Path, backend: CatalogBackend) -> Result<bool, Box<dyn Error>> {
    if !root.join(".seex/config.toml").is_file() {
        return Ok(false);
    }
    let manifest_path = root.join(MANIFEST_PATH);
    if !manifest_path.is_file() {
        return Err(
            "Reader benchmark dataset is incomplete: completion manifest is missing".into(),
        );
    }
    if fs::read_to_string(manifest_path)? != dataset_manifest(backend) {
        return Err("Reader benchmark dataset manifest does not match backend or scale".into());
    }
    Ok(true)
}

fn backend() -> Result<CatalogBackend, Box<dyn Error>> {
    match env::var("SEEX_QUERY_BENCH_BACKEND")
        .as_deref()
        .unwrap_or("duckdb")
    {
        "duckdb" => Ok(CatalogBackend::DuckDb),
        "sqlite" => Ok(CatalogBackend::Sqlite),
        value => Err(format!("unsupported query benchmark backend: {value}").into()),
    }
}

fn ensure_reader_benchmark_dataset(
    root: &Path,
    backend: CatalogBackend,
) -> Result<(), Box<dyn Error>> {
    if reuse_complete_dataset(root, backend)? {
        return Ok(());
    }
    if root.exists() && fs::read_dir(root)?.next().is_some() {
        return Err("refusing to replace a non-empty Reader benchmark dataset".into());
    }
    fs::create_dir_all(root.join(".seex"))?;
    let (backend_name, catalog) = match backend {
        CatalogBackend::DuckDb => ("duckdb", "catalog.ducklake"),
        CatalogBackend::Sqlite => ("sqlite", "catalog.sqlite"),
    };
    fs::write(
        root.join(".seex/config.toml"),
        format!(
            "schema_version = 1\ncatalog_backend = \"{backend_name}\"\n\
             catalog_path = \"custom/{catalog}\"\ndata_path = \"custom/data\"\n"
        ),
    )?;
    let catalog_path = root.join("custom").join(catalog);
    let data_path = root.join("custom/data");
    let client = NativeClient::open_with_catalog_backend_storage_config(
        root,
        backend,
        Some(catalog_path.clone()),
        Some(data_path.clone()),
        None,
        65_536,
    )?;
    let project_id = ProjectId::from_string("reader-benchmark");
    client.create_project("reader benchmark", Some(project_id.clone()))?;
    let run_id = RunId::from_string("run-1");
    client.create_run(&project_id, "run", Some(run_id.clone()))?;
    client.shutdown(None)?;
    drop(client);
    let connection = ProjectConnection::new(open_native_connection_with_config(
        NativeStorageConfig::with_backend_and_s3_config(
            backend,
            root,
            Some(catalog_path),
            Some(data_path),
            None,
        ),
    )?);
    connection.execute(
        "INSERT INTO dl.metric_points
         (run_id, metric_key, metric_key_encoded, step, timestamp, value_f64, ingested_at)
         SELECT 'run-1', 'loss', 'loss', step, epoch_ms(? + step),
                (step % 1000)::DOUBLE / 1000, epoch_ms(? + step)
         FROM range(?) AS points(step)",
        (EPOCH_MILLIS, EPOCH_MILLIS, POINTS),
    )?;
    connection.rebuild_metric_aggregates_for_run(&run_id)?;
    connection.execute(
        "UPDATE seex_runs SET status = 'finished', started_at = epoch_ms(?), finished_at = now()
         WHERE run_id = 'run-1'",
        [EPOCH_MILLIS],
    )?;
    connection.flush_metric_points()?;
    drop(connection);
    fs::write(root.join(MANIFEST_PATH), dataset_manifest(backend))?;
    Ok(())
}

#[test]
fn retained_dataset_requires_a_completion_manifest() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join(".seex"))?;
    fs::write(
        root.path().join(".seex/config.toml"),
        "schema_version = 1\n",
    )?;

    let error = ensure_reader_benchmark_dataset(root.path(), CatalogBackend::DuckDb)
        .expect_err("an interrupted dataset must not be reused");

    assert!(error.to_string().contains("completion manifest"));
    Ok(())
}

#[test]
fn retained_dataset_manifest_binds_the_backend() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join(".seex"))?;
    fs::write(
        root.path().join(".seex/config.toml"),
        "schema_version = 1\n",
    )?;
    fs::write(
        root.path().join(MANIFEST_PATH),
        dataset_manifest(CatalogBackend::DuckDb),
    )?;

    let error = ensure_reader_benchmark_dataset(root.path(), CatalogBackend::Sqlite)
        .expect_err("a DuckDB dataset must not be labeled as SQLite");

    assert!(error.to_string().contains("backend or scale"));
    Ok(())
}

#[test]
#[ignore = "creates the retained 1 Run x 1 Metric x 1M Reader benchmark dataset"]
fn prepare_reader_benchmark_dataset() -> Result<(), Box<dyn Error>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "benchmark dataset preparation requires --release"
    );
    let root = env::var_os("SEEX_QUERY_BENCH_ROOT")
        .map(PathBuf::from)
        .ok_or("SEEX_QUERY_BENCH_ROOT must name the prepared dataset")?;
    ensure_reader_benchmark_dataset(&root, backend()?)
}
