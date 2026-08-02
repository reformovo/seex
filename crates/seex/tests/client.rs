use std::fs;

use seex::{CatalogBackend, Client, Error, S3Options, WriterState};

#[test]
fn builder_overrides_project_storage_configuration() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join(".seex"))?;
    fs::write(
        root.path().join(".seex/config.toml"),
        "schema_version = 1\ncatalog_backend = \"duckdb\"\n",
    )?;
    let catalog_path = root.path().join("explicit.sqlite");
    let data_path = root.path().join("explicit-data");

    let client = Client::builder(root.path())
        .catalog_backend(CatalogBackend::Sqlite)
        .catalog_path(&catalog_path)
        .data_path(&data_path)
        .metric_queue_capacity(32)
        .open()?;
    client.shutdown()?;

    assert!(
        catalog_path.is_file(),
        "explicit catalog path was not created"
    );
    assert!(data_path.is_dir(), "explicit data path was not created");
    Ok(())
}

#[test]
fn builder_rejects_invalid_queue_capacity() {
    let root = tempfile::tempdir().expect("temporary root should be created");
    let result = Client::builder(root.path()).metric_queue_capacity(0).open();

    assert!(matches!(result, Err(Error::Configuration)));
}

#[test]
fn s3_debug_output_redacts_credentials() {
    let options = S3Options::new()
        .endpoint("https://objects.example")
        .access_key_id("visible-key")
        .secret_access_key("secret-value")
        .session_token("session-value");
    let rendered = format!("{options:?}");

    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("visible-key"));
    assert!(!rendered.contains("secret-value"));
    assert!(!rendered.contains("session-value"));
}

#[test]
fn shutdown_closes_diagnostics_without_finalizing_runs() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let client = Client::builder(root.path()).open()?;

    assert_eq!(client.diagnostics().writer_state, WriterState::Drained);
    client.shutdown()?;

    assert_eq!(client.diagnostics().writer_state, WriterState::Closed);
    Ok(())
}
