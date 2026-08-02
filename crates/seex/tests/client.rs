use std::fs;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

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
fn configuration_errors_redact_credentials() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join(".seex"))?;
    let secret = "must-not-escape";
    fs::write(
        root.path().join(".seex/config.toml"),
        format!(
            "schema_version = 1\ncatalog_backend = \"invalid\"\n[s3]\nsecret_access_key = \"{secret}\"\n"
        ),
    )?;

    let error = match Client::builder(root.path()).open() {
        Ok(_) => return Err("invalid catalog backend unexpectedly opened".into()),
        Err(error) => error,
    };
    assert_eq!(error, Error::Configuration);
    assert!(!format!("{error:?} {error}").contains(secret));
    Ok(())
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
fn project_race_child_process() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os("SEEX_PROJECT_RACE_ROOT") else {
        return Ok(());
    };
    let run_id = std::env::var("SEEX_PROJECT_RACE_RUN_ID")?;
    let ready = std::env::var_os("SEEX_PROJECT_RACE_READY").ok_or("child ready path is missing")?;
    let go = std::env::var_os("SEEX_PROJECT_RACE_GO").ok_or("child go path is missing")?;
    let client = Client::builder(&root).open()?;
    fs::write(ready, b"ready")?;
    wait_for_path(Path::new(&go), Duration::from_secs(15))?;
    client.start_run(RunOptions::new("shared").id(run_id))?;
    client.shutdown()?;
    Ok(())
}

fn wait_for_path(path: &Path, timeout: Duration) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {}", path.display()).into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn spawn_project_race_child(
    root: &Path,
    run_id: &str,
    ready: &Path,
    go: &Path,
) -> Result<Child, Box<dyn std::error::Error>> {
    Ok(Command::new(std::env::current_exe()?)
        .args(["--exact", "project_race_child_process", "--nocapture"])
        .env("SEEX_PROJECT_RACE_ROOT", root)
        .env("SEEX_PROJECT_RACE_RUN_ID", run_id)
        .env("SEEX_PROJECT_RACE_READY", ready)
        .env("SEEX_PROJECT_RACE_GO", go)
        .spawn()?)
}

#[test]
fn subprocess_clients_get_or_create_one_project() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let seex_dir = root.path().join(".seex");
    fs::create_dir_all(seex_dir.join("locks/projects"))?;
    fs::write(
        seex_dir.join("config.toml"),
        "schema_version = 1\ncatalog_backend = \"sqlite\"\n",
    )?;
    Client::builder(root.path()).open()?.shutdown()?;
    fs::write(seex_dir.join("locks/projects/shared.lock"), b"legacy")?;
    let left_ready = root.path().join("left.ready");
    let right_ready = root.path().join("right.ready");
    let go = root.path().join("go");
    let mut left = spawn_project_race_child(root.path(), "left", &left_ready, &go)?;
    wait_for_path(&left_ready, Duration::from_secs(15))?;
    let mut right = spawn_project_race_child(root.path(), "right", &right_ready, &go)?;
    wait_for_path(&right_ready, Duration::from_secs(15))?;
    fs::write(&go, b"go")?;
    assert!(left.wait()?.success(), "left child failed");
    assert!(right.wait()?.success(), "right child failed");

    let reader = Reader::builder(root.path()).open()?;
    assert_eq!(reader.projects()?.len(), 1);
    assert_eq!(
        reader.runs(&seex::ProjectId::from_string("shared"))?.len(),
        2
    );
    Ok(())
}

#[test]
fn different_roots_share_store_locks() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    let left_root = fixture.path().join("left");
    let right_root = fixture.path().join("right");
    let catalog_path = fixture.path().join("shared/catalog.sqlite");
    let data_path = fixture.path().join("shared/data");
    let open = |root: &Path| {
        Client::builder(root)
            .catalog_backend(CatalogBackend::Sqlite)
            .catalog_path(&catalog_path)
            .data_path(&data_path)
            .open()
    };
    open(&left_root)?.shutdown()?;
    let left = open(&left_root)?;
    let right = open(&right_root)?;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let left_barrier = std::sync::Arc::clone(&barrier);
    let right_barrier = std::sync::Arc::clone(&barrier);
    let left_thread = std::thread::spawn(move || {
        left_barrier.wait();
        let run = left.start_run(RunOptions::new("shared").id("left"))?;
        Ok::<_, Error>((left, run))
    });
    let right_thread = std::thread::spawn(move || {
        right_barrier.wait();
        let run = right.start_run(RunOptions::new("shared").id("right"))?;
        Ok::<_, Error>((right, run))
    });
    let (left, left_run) = left_thread.join().expect("left client should not panic")?;
    let (right, right_run) = right_thread
        .join()
        .expect("right client should not panic")?;

    assert!(matches!(
        right.start_run(
            RunOptions::new("shared")
                .id("left")
                .resume(ResumePolicy::Must)
        ),
        Err(Error::RunAlreadyActive { .. })
    ));
    drop((left_run, right_run));
    left.shutdown()?;
    right.shutdown()?;

    let reader_root = fixture.path().join("reader/.seex");
    fs::create_dir_all(&reader_root)?;
    fs::write(
        reader_root.join("config.toml"),
        format!(
            "schema_version = 1\ncatalog_backend = \"sqlite\"\ncatalog_path = {:?}\ndata_path = {:?}\n",
            catalog_path.to_string_lossy(),
            data_path.to_string_lossy()
        ),
    )?;
    let reader = Reader::builder(fixture.path().join("reader")).open()?;
    assert_eq!(reader.projects()?.len(), 1);
    assert_eq!(
        reader.runs(&seex::ProjectId::from_string("shared"))?.len(),
        2
    );
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
    let reader = Reader::builder(root.path()).open()?;
    let query = seex::MetricQuery::new(seex::MetricRange::All(seex::MetricAxis::Step), None)?;
    let loss = reader.query_metric(run.run_id(), &seex::MetricKey::from_string("loss"), &query)?;
    let accuracy = reader.query_metric(
        run.run_id(),
        &seex::MetricKey::from_string("accuracy"),
        &query,
    )?;
    assert_eq!(loss.samples().len(), 1);
    assert!(
        accuracy.samples().is_empty(),
        "failed Mapping persisted a subset"
    );
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
fn matching_terminal_calls_are_idempotent_and_conflicts_are_typed()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let client = Client::builder(root.path()).open()?;
    let run = client.start_run(RunOptions::new("demo").id("run-1"))?;
    run.log([("loss", 1.0)])?;
    let left = run.clone();
    let right = run.clone();

    let left_finish = std::thread::spawn(move || left.finish());
    let right_finish = std::thread::spawn(move || right.finish());
    left_finish
        .join()
        .expect("finish thread should not panic")?;
    right_finish
        .join()
        .expect("finish thread should not panic")?;

    assert_eq!(run.status(), seex::RunStatus::Finished);
    assert!(matches!(
        run.fail(),
        Err(Error::TerminalOutcomeConflict {
            selected: seex::RunStatus::Finished,
            requested: seex::RunStatus::Failed
        })
    ));
    assert!(matches!(
        run.log([("loss", 2.0)]),
        Err(Error::RunClosed { .. })
    ));
    let reader = Reader::builder(root.path()).open()?;
    let series = reader.query_metric(
        run.run_id(),
        &seex::MetricKey::from_string("loss"),
        &seex::MetricQuery::new(seex::MetricRange::All(seex::MetricAxis::Step), None)?,
    )?;
    assert_eq!(
        series.samples().len(),
        1,
        "terminal barrier lost admitted data"
    );
    client.shutdown()?;
    Ok(())
}

#[test]
fn separately_started_handles_share_terminal_intent() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let client = Client::builder(root.path()).open()?;
    let first = client.start_run(RunOptions::new("demo").id("run-1"))?;
    let second = client.start_run(
        RunOptions::new("demo")
            .id("run-1")
            .resume(ResumePolicy::Allow),
    )?;

    first.finish()?;

    assert_eq!(second.status(), seex::RunStatus::Finished);
    assert!(matches!(
        second.fail(),
        Err(Error::TerminalOutcomeConflict {
            selected: seex::RunStatus::Finished,
            requested: seex::RunStatus::Failed
        })
    ));
    second.finish()?;
    client.shutdown()?;
    Ok(())
}

#[test]
fn matching_terminal_call_retries_an_incomplete_flush() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let client = Client::builder(root.path()).open()?;
    let run = client.start_run(RunOptions::new("demo").id("run-1"))?;
    run.log([("loss", 1.0)])?;
    wait_for_drain(&client);
    let metric_points_path = root.path().join(".seex/data/main/metric_points");
    fs::create_dir_all(
        metric_points_path
            .parent()
            .expect("data parent should exist"),
    )?;
    if metric_points_path.is_dir() {
        fs::remove_dir_all(&metric_points_path)?;
    }
    fs::write(&metric_points_path, b"not a directory")?;

    assert!(matches!(run.finish(), Err(Error::MetricFlushFailed)));
    assert_eq!(run.status(), seex::RunStatus::Finished);
    assert!(matches!(
        run.fail(),
        Err(Error::TerminalOutcomeConflict { .. })
    ));
    fs::remove_file(&metric_points_path)?;
    run.finish()?;
    run.finish()?;
    client.shutdown()?;
    Ok(())
}

#[test]
fn dropping_run_handle_does_not_finalize_the_run() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let client = Client::builder(root.path()).open()?;
    let run = client.start_run(RunOptions::new("demo").id("run-1"))?;
    let run_id = run.run_id().clone();
    drop(run);
    client.shutdown()?;

    let reader = Reader::builder(root.path()).open()?;
    let stored = reader
        .runs(&seex::ProjectId::from_string("demo"))?
        .into_iter()
        .find(|run| run.run_id == run_id)
        .expect("running Run should remain stored");
    assert_eq!(stored.status, seex::RunStatus::Running);
    Ok(())
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
    assert_eq!(
        series
            .samples()
            .iter()
            .map(|sample| sample.point.step.value())
            .collect::<Vec<_>>(),
        (0..20).collect::<Vec<_>>()
    );
    client.shutdown()?;
    Ok(())
}
