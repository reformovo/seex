use std::path::PathBuf;

use crate::engine::EngineError;
use crate::engine::bootstrap::{
    attach_ducklake as attach_ducklake_dataset, initialize_storage_tables,
    setup_duckdb_catalog_adapter,
};

pub struct TestDataset {
    root: PathBuf,
    catalog_path: PathBuf,
    data_path: PathBuf,
}

impl TestDataset {
    pub fn new() -> std::io::Result<Self> {
        let root = std::env::temp_dir().join(format!("seex-ducklake-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        Ok(Self {
            catalog_path: root.join("catalog.ducklake"),
            data_path: root.join("data"),
            root,
        })
    }
}

impl Drop for TestDataset {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn attach_ducklake(
    connection: &duckdb::Connection,
    dataset: &TestDataset,
) -> Result<(), EngineError> {
    attach_ducklake_dataset(connection, &dataset.catalog_path, &dataset.data_path)?;
    Ok(setup_duckdb_catalog_adapter(
        connection,
        &dataset.catalog_path,
    )?)
}

pub fn initialize_test_storage_tables(connection: &duckdb::Connection) -> Result<(), EngineError> {
    Ok(initialize_storage_tables(connection)?)
}

pub fn seed_test_storage_data(connection: &duckdb::Connection) -> duckdb::Result<()> {
    connection.execute_batch(
        "INSERT INTO seex_projects VALUES
             ('project-1', 'local training', now());
         INSERT INTO seex_runs VALUES
             ('run-1', 'project-1', 'baseline', 'running', now(), now(), NULL);
         INSERT INTO dl.metric_points VALUES
             ('run-1', 'train/loss', 'train%2Floss', 0, now(), 0.25, now()),
             ('run-1', 'train/loss', 'train%2Floss', 1, now(), 0.125, now());
         INSERT INTO seex_metric_aggregates VALUES
             ('run-1', 'train/loss', 2, 1, 0.125, 0.125, 0.25);",
    )
}
