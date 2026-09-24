//! Isolated P1 probe for concurrent SQLite-backed DuckLake writers.

use std::fs::{self, File};
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use seex::{
    CatalogBackend, Client, LogOptions, MetricAxis, MetricKey, MetricQuery, MetricRange, Reader,
    RunOptions, RunStatus,
};

const WRITERS: usize = 4;
const POINTS: i64 = 32;
const TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn p1_writer_child() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os("SEEX_P1_PROBE_ROOT") else {
        return Ok(());
    };
    let root = Path::new(&root);
    let index: usize = std::env::var("SEEX_P1_PROBE_INDEX")?.parse()?;
    fs::write(root.join(format!("writer-{index}.ready")), b"ready")?;
    wait_for(&root.join("writers.go"))?;
    let lifecycle_lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("p1-probe.lock"))?;
    lifecycle_lock.lock()?;
    eprintln!("writer {index}: open");
    let client = Client::builder(root)
        .catalog_backend(CatalogBackend::Sqlite)
        .open()?;
    eprintln!("writer {index}: start_run");
    let run = client.start_run(RunOptions::new("p1-probe").id(format!("run-{index}")))?;
    for step in 0..POINTS {
        run.log_with(
            [("loss", index as f64 + step as f64)],
            LogOptions::new().step(step),
        )?;
    }
    eprintln!("writer {index}: finish");
    run.finish()?;
    client.shutdown()?;
    lifecycle_lock.unlock()?;
    Ok(())
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
#[ignore = "P1 baseline serializes whole lifecycles; not a multi-writer acceptance test"]
fn serialized_lifecycles_keep_reader_consistent() -> Result<(), Box<dyn std::error::Error>> {
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
        let runs = reader.runs(&project)?;
        for run in runs.iter().filter(|run| run.status == RunStatus::Finished) {
            if run.run_id.as_str() == "seed" {
                continue;
            }
            let series =
                reader.query_metric(&run.run_id, &MetricKey::from_string("loss"), &query)?;
            assert_eq!(series.samples().len(), POINTS as usize);
            assert_eq!(series.source_count(), POINTS as u64);
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
