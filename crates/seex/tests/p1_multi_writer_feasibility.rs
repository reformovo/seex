//! Isolated P1 probe for concurrent SQLite-backed DuckLake writers.

use std::fs;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use seex::{
    CatalogBackend, Client, LogOptions, MetricAxis, MetricKey, MetricQuery, MetricRange, Reader,
    ResumePolicy, RunOptions, RunStatus,
};

const WRITERS: usize = 4;
const POINTS: i64 = 32;
const TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn p1_open_child() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os("SEEX_P1_OPEN_ROOT") else {
        return Ok(());
    };
    let root = Path::new(&root);
    let index = std::env::var("SEEX_P1_OPEN_INDEX")?;
    fs::write(root.join(format!("open-{index}.ready")), b"ready")?;
    wait_for(&root.join("open.go"))?;
    Client::builder(root)
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?
        .shutdown()?;
    Ok(())
}

#[test]
fn four_processes_open_existing_sqlite_store() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    Client::builder(root.path())
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?
        .shutdown()?;
    let mut children = Children(Vec::new());
    for index in 0..WRITERS {
        children.0.push(
            Command::new(std::env::current_exe()?)
                .args(["--exact", "p1_open_child"])
                .env("SEEX_P1_OPEN_ROOT", root.path())
                .env("SEEX_P1_OPEN_INDEX", index.to_string())
                .spawn()?,
        );
    }
    for index in 0..WRITERS {
        wait_for(&root.path().join(format!("open-{index}.ready")))?;
    }
    fs::write(root.path().join("open.go"), b"go")?;
    for child in &mut children.0 {
        assert!(child.wait()?.success(), "concurrent store open failed");
    }
    Ok(())
}

#[test]
fn p1_writer_child() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os("SEEX_P1_PROBE_ROOT") else {
        return Ok(());
    };
    let root = Path::new(&root);
    let index: usize = std::env::var("SEEX_P1_PROBE_INDEX")?.parse()?;
    let client = Client::builder(root)
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?;
    fs::write(root.join(format!("writer-{index}.ready")), b"ready")?;
    wait_for(&root.join("writers.go"))?;
    let run = client.start_run(RunOptions::new("p1-probe").id(format!("run-{index}")))?;
    for step in 0..POINTS {
        run.log_with(
            [("loss", index as f64 + step as f64)],
            LogOptions::new().step(step),
        )?;
    }
    run.finish()?;
    client.shutdown()?;
    Ok(())
}

#[test]
fn p1_crash_child() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os("SEEX_P1_CRASH_ROOT") else {
        return Ok(());
    };
    let root = Path::new(&root);
    let client = Client::builder(root)
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?;
    let run = client.start_run(
        RunOptions::new("p1-crash")
            .id("crash-run")
            .resume(ResumePolicy::Never),
    )?;
    run.log_with([("loss", 1.0)], LogOptions::new().step(0).commit(true))?;
    while run.diagnostics().pending_reports != 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    fs::write(root.join("crash.ready"), b"ready")?;
    wait_for(&root.join("crash.go"))?;
    std::process::exit(0);
}

fn wait_for(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + TIMEOUT;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {}", path.display()).into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn simultaneous_writers_keep_reader_consistent() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let seed = Client::builder(root.path())
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?;
    seed.start_run(RunOptions::new("p1-probe").id("seed"))?
        .finish()?;
    seed.shutdown()?;

    let mut children = Children(Vec::new());
    for index in 0..WRITERS {
        children.0.push(
            Command::new(std::env::current_exe()?)
                .args(["--exact", "p1_writer_child", "--nocapture"])
                .env("SEEX_P1_PROBE_ROOT", root.path())
                .env("SEEX_P1_PROBE_INDEX", index.to_string())
                .spawn()?,
        );
        let ready = root.path().join(format!("writer-{index}.ready"));
        let deadline = Instant::now() + TIMEOUT;
        while !ready.exists() {
            if let Some(status) = children.0[index].try_wait()? {
                return Err(format!("writer {index} exited before ready: {status}").into());
            }
            if Instant::now() >= deadline {
                return Err(format!("writer {index} did not become ready").into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let reader = Reader::builder(root.path())
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?;
    fs::write(root.path().join("writers.go"), b"go")?;
    let project = seex::ProjectId::from_string("p1-probe");
    let query = MetricQuery::new(MetricRange::All(MetricAxis::Step), 1_024)?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        for (index, child) in children.0.iter_mut().enumerate() {
            if let Some(status) = child.try_wait()?
                && !status.success()
            {
                return Err(format!("writer {index} exited during write: {status}").into());
            }
        }
        let runs = reader
            .runs(&project)
            .map_err(|error| format!("reader runs: {error:?}"))?;
        for run in runs.iter().filter(|run| run.status == RunStatus::Finished) {
            if run.run_id.as_str() == "seed" {
                continue;
            }
            let series = reader
                .query_metric(&run.run_id, &MetricKey::from_string("loss"), &query)
                .map_err(|error| format!("reader metric {}: {error:?}", run.run_id.as_str()))?;
            assert_eq!(series.samples().len(), POINTS as usize);
            assert_eq!(series.source_count(), POINTS as u64);
            let summary = reader
                .metric_summary(run, &MetricKey::from_string("loss"))
                .map_err(|error| format!("reader summary {}: {error:?}", run.run_id.as_str()))?
                .ok_or("finished Run has no metric summary")?;
            assert_eq!(summary.effective_count, POINTS as u64);
        }
        if runs.len() == WRITERS + 1 && runs.iter().all(|run| run.status == RunStatus::Finished) {
            break;
        }
        if Instant::now() >= deadline {
            return Err("writers did not become visible to the Reader".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for child in &mut children.0 {
        assert!(child.wait()?.success(), "P1 writer child failed");
    }
    Ok(())
}

#[test]
fn crashed_writer_can_restart_and_finish_the_same_run() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", "p1_crash_child", "--nocapture"])
        .env("SEEX_P1_CRASH_ROOT", root.path())
        .spawn()?;
    wait_for(&root.path().join("crash.ready"))?;
    fs::write(root.path().join("crash.go"), b"go")?;
    assert!(
        child.wait()?.success(),
        "crashed writer child did not exit cleanly"
    );

    let reader = Reader::builder(root.path())
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?;
    let project = seex::ProjectId::from_string("p1-crash");
    let run_id = seex::RunId::from_string("crash-run");
    let running = reader
        .run(&project, &run_id)?
        .ok_or("crashed Run is missing")?;
    assert_eq!(running.status, RunStatus::Running);
    let query = MetricQuery::new(MetricRange::All(MetricAxis::Step), 1_024)?;
    let persisted = reader.query_metric(&run_id, &MetricKey::from_string("loss"), &query)?;
    assert_eq!(persisted.samples().len(), 1);

    let client = Client::builder(root.path())
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?;
    let resumed = client.start_run(
        RunOptions::new("p1-crash")
            .id("crash-run")
            .resume(ResumePolicy::Allow),
    )?;
    resumed.log_with([("loss", 2.0)], LogOptions::new().step(1).commit(true))?;
    resumed.finish()?;
    let reader = Reader::builder(root.path())
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?;
    let finished = reader
        .run(&project, &run_id)?
        .ok_or("restarted Run is missing")?;
    assert_eq!(finished.status, RunStatus::Finished);
    let series = reader.query_metric(&run_id, &MetricKey::from_string("loss"), &query)?;
    assert_eq!(series.samples().len(), 2);
    client.shutdown()?;
    Ok(())
}
