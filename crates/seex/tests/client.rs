use std::fs;

use seex::{
    CatalogBackend, Client, Error, LogOptions, Reader, ResumePolicy, RunOptions, S3Options,
    WriterState,
};

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

#[test]
fn start_run_creates_project_and_applies_resume_policies() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let first = Client::builder(root.path()).open()?;
    let created = first.start_run(RunOptions::new("demo").id("run-1").name("first"))?;
    assert_eq!(created.name(), "first");
    first.shutdown()?;

    let second = Client::builder(root.path()).open()?;
    let resumed = second.start_run(
        RunOptions::new("demo")
            .id("run-1")
            .name("ignored")
            .resume(ResumePolicy::Must),
    )?;
    assert_eq!(resumed.name(), "first");
    assert!(matches!(
        second.start_run(RunOptions::new("demo").id("run-1")),
        Err(Error::RunAlreadyExists { .. })
    ));
    second.shutdown()?;
    Ok(())
}

#[test]
fn resume_requires_matching_project_and_explicit_must_id() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let client = Client::builder(root.path()).open()?;
    client.start_run(RunOptions::new("first").id("run-1"))?;

    assert!(matches!(
        client.start_run(
            RunOptions::new("second")
                .id("run-1")
                .resume(ResumePolicy::Allow)
        ),
        Err(Error::RunProjectMismatch { .. })
    ));
    assert!(matches!(
        client.start_run(RunOptions::new("first").resume(ResumePolicy::Must)),
        Err(Error::InvalidRunOptions { field: "id" })
    ));
    client.shutdown()?;
    Ok(())
}

#[test]
fn concurrent_clients_get_or_create_one_project() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let left = Client::builder(root.path()).open()?;
    let right = Client::builder(root.path()).open()?;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let left_barrier = std::sync::Arc::clone(&barrier);
    let right_barrier = std::sync::Arc::clone(&barrier);

    let left_thread = std::thread::spawn(move || {
        left_barrier.wait();
        let result = left.start_run(RunOptions::new("shared").id("left"));
        let shutdown = left.shutdown();
        result.and(shutdown)
    });
    let right_thread = std::thread::spawn(move || {
        right_barrier.wait();
        let result = right.start_run(RunOptions::new("shared").id("right"));
        let shutdown = right.shutdown();
        result.and(shutdown)
    });
    left_thread.join().expect("left client should not panic")?;
    right_thread
        .join()
        .expect("right client should not panic")?;

    let reader = Reader::builder(root.path()).open()?;
    assert_eq!(reader.projects()?.len(), 1);
    Ok(())
}

fn wait_for_drain(client: &Client) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while client.diagnostics().pending_reports != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "metric drain timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn log_defaults_and_commit_cursor_are_atomic() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let client = Client::builder(root.path()).open()?;
    let run = client.start_run(RunOptions::new("demo").id("run-1"))?;

    run.log([("loss", 1.0)])?;
    run.log_with([("loss", 5.0)], LogOptions::new().step(5))?;
    run.log([("loss", 2.0)])?;
    run.log_with([("loss", 4.0)], LogOptions::new().step(4).commit(true))?;
    assert!(matches!(
        run.log_with([("loss", 3.0)], LogOptions::new().step(3).commit(true)),
        Err(Error::StepRegression { .. })
    ));
    assert!(matches!(
        run.log_with(
            [("loss", 9.0)],
            LogOptions::new().step(i64::MAX).commit(true)
        ),
        Err(Error::StepOverflow { step: i64::MAX })
    ));
    run.log([("loss", 6.0)])?;
    wait_for_drain(&client);

    let reader = Reader::builder(root.path()).open()?;
    let series = reader.query_metric(
        run.run_id(),
        &seex::MetricKey::from_string("loss"),
        &seex::MetricQuery::new(seex::MetricRange::All(seex::MetricAxis::Step), None)?,
    )?;
    let steps = series
        .samples()
        .iter()
        .map(|sample| sample.coordinate)
        .collect::<Vec<_>>();
    assert_eq!(
        steps,
        vec![
            seex::MetricCoordinate::Step(seex::Step::new(0)),
            seex::MetricCoordinate::Step(seex::Step::new(1)),
            seex::MetricCoordinate::Step(seex::Step::new(4)),
            seex::MetricCoordinate::Step(seex::Step::new(5)),
        ]
    );
    client.shutdown()?;
    Ok(())
}

#[test]
fn failed_mapping_does_not_advance_the_cursor() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let client = Client::builder(root.path())
        .metric_queue_capacity(1)
        .open()?;
    let run = client.start_run(RunOptions::new("demo"))?;

    assert!(matches!(
        run.log([("loss", 1.0), ("accuracy", 0.5)]),
        Err(Error::MetricQueueFull)
    ));
    assert!(matches!(
        run.log(Vec::<(&str, f64)>::new()),
        Err(Error::InvalidMetricMapping)
    ));
    assert!(matches!(
        run.log([("loss", 1.0), ("loss", 2.0)]),
        Err(Error::InvalidMetricMapping)
    ));
    let oversized = (0..8_193)
        .map(|index| (format!("metric-{index}"), index as f64))
        .collect::<Vec<_>>();
    assert!(matches!(
        run.log(oversized),
        Err(Error::MetricMappingTooLarge {
            count: 8_193,
            maximum: 8_192
        })
    ));
    run.log([("loss", 3.0)])?;
    wait_for_drain(&client);
    assert_eq!(client.diagnostics().persisted_reports, 1);
    client.shutdown()?;
    Ok(())
}

#[test]
fn resume_continues_after_the_greatest_persisted_step() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let first = Client::builder(root.path()).open()?;
    let run = first.start_run(RunOptions::new("demo").id("run-1"))?;
    run.log_with([("loss", 7.0)], LogOptions::new().step(7))?;
    first.shutdown()?;

    let second = Client::builder(root.path()).open()?;
    let resumed = second.start_run(
        RunOptions::new("demo")
            .id("run-1")
            .resume(ResumePolicy::Must),
    )?;
    resumed.log([("loss", 8.0)])?;
    wait_for_drain(&second);
    let reader = Reader::builder(root.path()).open()?;
    let series = reader.query_metric(
        resumed.run_id(),
        &seex::MetricKey::from_string("loss"),
        &seex::MetricQuery::new(seex::MetricRange::All(seex::MetricAxis::Step), None)?,
    )?;
    assert_eq!(
        series
            .samples()
            .iter()
            .map(|sample| sample.point.step.value())
            .collect::<Vec<_>>(),
        vec![7, 8]
    );
    second.shutdown()?;
    Ok(())
}

#[test]
fn run_handle_is_clone_send_and_sync() {
    fn assert_contract<T: Clone + Send + Sync>() {}
    assert_contract::<seex::RunHandle>();
}

#[test]
fn cloned_handles_share_one_implicit_cursor() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let client = Client::builder(root.path()).open()?;
    let run = client.start_run(RunOptions::new("demo").id("run-1"))?;
    let threads = (0..20)
        .map(|index| {
            let run = run.clone();
            std::thread::spawn(move || run.log([("loss", index as f64)]))
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().expect("log thread should not panic")?;
    }
    wait_for_drain(&client);

    let reader = Reader::builder(root.path()).open()?;
    let series = reader.query_metric(
        run.run_id(),
        &seex::MetricKey::from_string("loss"),
        &seex::MetricQuery::new(seex::MetricRange::All(seex::MetricAxis::Step), None)?,
    )?;
    assert_eq!(series.samples().len(), 20);
    client.shutdown()?;
    Ok(())
}
