//! Opt-in multi-Run, multi-metric scale fixture generator. Not a performance gate.
//!
//! 20 Runs x 16 metrics x 1,000,000 Steps (320M observations, about 6.1 GB and
//! 26 minutes on the local Mac):
//!
//! ```sh
//! SEEX_SCALE_ROOT="$PWD/.tmp/v5-panel-20x16/store" \
//! SEEX_SCALE_RUNS=20 SEEX_SCALE_METRICS=16 SEEX_SCALE_POINTS=1000000 \
//! cargo test -p seex-app --no-default-features --release --test scale_case \
//!   prepare_scale_case -- --ignored --exact --nocapture
//! ```
//!
//! The writer's 16,384-report batch cap means a 16-metric `log_with` call
//! produces roughly 1,024 Steps per Parquet file per series (about 312,000
//! files), so this is a file-count stress case as well as a point-count case.
//! To write one metric at a time instead (about 19,500 files, 61 per series),
//! set `SEEX_SCALE_WRITE_ORDER=metric`.
use seex::{Client, LogOptions, RunOptions};
use std::{
    env,
    error::Error,
    path::PathBuf,
    time::{Duration, Instant},
};

fn value(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 11) as f64 / (1_u64 << 53) as f64
}

fn drain(client: &Client) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while client.diagnostics().pending_reports != 0 {
        if Instant::now() >= deadline {
            return Err("scale fixture drain timeout".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[test]
#[ignore = "creates a large local fixture; set SEEX_SCALE_ROOT, SEEX_SCALE_RUNS, SEEX_SCALE_METRICS, SEEX_SCALE_POINTS"]
fn prepare_scale_case() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env::var("SEEX_SCALE_ROOT")?);
    if !root.is_absolute() || !root.to_string_lossy().contains("/.tmp/v5-") {
        return Err("SEEX_SCALE_ROOT must be an absolute path under .tmp/v5-*".into());
    }
    let runs: usize = env::var("SEEX_SCALE_RUNS").unwrap_or("2".into()).parse()?;
    let run_start: usize = env::var("SEEX_SCALE_RUN_START")
        .unwrap_or("0".into())
        .parse()?;
    let run_count: usize = env::var("SEEX_SCALE_RUN_COUNT")
        .unwrap_or_else(|_| runs.to_string())
        .parse()?;
    if run_count == 0
        || run_start
            .checked_add(run_count)
            .is_none_or(|end| end > runs)
    {
        return Err("run range must fit inside SEEX_SCALE_RUNS".into());
    }
    if root.exists() && run_start == 0 {
        return Err("refusing to overwrite an existing scale fixture".into());
    }
    if !root.exists() && run_start != 0 {
        return Err("a run range after zero requires an existing scale fixture".into());
    }
    let metrics: usize = env::var("SEEX_SCALE_METRICS")
        .unwrap_or("2".into())
        .parse()?;
    let points: i64 = env::var("SEEX_SCALE_POINTS")
        .unwrap_or("10000".into())
        .parse()?;
    if runs == 0 || metrics == 0 || points <= 0 {
        return Err("scale counts must be positive".into());
    }
    let per_metric = match env::var("SEEX_SCALE_WRITE_ORDER").as_deref() {
        Ok("step") | Err(_) => false,
        Ok("metric") => true,
        Ok(other) => return Err(format!("unsupported SEEX_SCALE_WRITE_ORDER: {other}").into()),
    };
    // One metric per call batches at 16,384 Steps; one call per Step with 16
    // metrics batches at 1,024 Steps. Drain after each full batch.
    let drain_interval = if per_metric { 16_384 } else { 2_048 };
    let keys = (0..metrics)
        .map(|index| format!("metric-{index:02}"))
        .collect::<Vec<_>>();
    let client = Client::builder(&root).open()?;
    let started = Instant::now();
    for run_index in run_start..run_start + run_count {
        let run = client.start_run(RunOptions::new("panel").id(format!("run-{run_index:02}")))?;
        let mut states = (0..metrics)
            .map(|metric| 0x5eec_0000 + run_index as u64 * 131 + metric as u64)
            .collect::<Vec<_>>();
        if per_metric {
            for metric in 0..metrics {
                for step in 0..points {
                    run.log_with(
                        [(keys[metric].as_str(), value(&mut states[metric]))],
                        LogOptions::new().step(step),
                    )?;
                    if (step + 1) % drain_interval == 0 {
                        drain(&client)?;
                    }
                    if (step + 1) % 65_536 == 0 {
                        eprintln!(
                            "SEEX_SCALE run={run_index} metric={metric} step={step} elapsed_s={:.1}",
                            started.elapsed().as_secs_f64()
                        );
                    }
                }
            }
        } else {
            for step in 0..points {
                let values = keys
                    .iter()
                    .enumerate()
                    .map(|(metric, key)| (key.as_str(), value(&mut states[metric])));
                run.log_with(values, LogOptions::new().step(step))?;
                if (step + 1) % drain_interval == 0 {
                    drain(&client)?;
                }
                if (step + 1) % 65_536 == 0 {
                    eprintln!(
                        "SEEX_SCALE run={run_index} step={step} elapsed_s={:.1}",
                        started.elapsed().as_secs_f64()
                    );
                }
            }
        }
        run.finish()?;
        eprintln!(
            "SEEX_SCALE finished run={run_index} elapsed_s={:.1}",
            started.elapsed().as_secs_f64()
        );
    }
    eprintln!(
        "SEEX_SCALE complete run_start={run_start} run_count={run_count} runs={runs} metrics={metrics} points={points} observations={} elapsed_s={:.1}",
        run_count as u64 * metrics as u64 * points as u64,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

#[test]
#[ignore = "checks the viewer source config; set SEEX_VIEWER_ROOT"]
fn verify_viewer_source_config() -> Result<(), Box<dyn Error>> {
    let project_root = PathBuf::from(env::var("SEEX_VIEWER_ROOT")?);
    let sources = seex_app::config::load_sources_for_scope(Some(&project_root))?;
    println!(
        "SEEX_VIEWER_SOURCES {:?}",
        sources
            .iter()
            .map(|source| (
                source.alias.as_str(),
                source.root_path.clone(),
                source.projects.len()
            ))
            .collect::<Vec<_>>()
    );
    let source = sources
        .iter()
        .find(|source| source.alias.as_str() == "panel")
        .ok_or("panel source is missing")?;
    assert_eq!(source.projects.len(), 1);
    assert_eq!(source.projects[0].as_str(), "panel");
    Ok(())
}

#[test]
#[ignore = "reads an existing scale fixture; set SEEX_SCALE_ROOT"]
fn verify_scale_case() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(env::var("SEEX_SCALE_ROOT")?);
    let expected_runs: usize = env::var("SEEX_SCALE_RUNS").unwrap_or("20".into()).parse()?;
    let expected_metrics: usize = env::var("SEEX_SCALE_METRICS")
        .unwrap_or("16".into())
        .parse()?;
    let expected_points: i64 = env::var("SEEX_SCALE_POINTS")
        .unwrap_or("1000000".into())
        .parse()?;
    let reader = seex::Reader::builder(&root).open()?;
    let projects = reader.projects()?;
    let project = projects
        .iter()
        .find(|project| project.project_id.as_str() == "panel")
        .ok_or("panel project is missing")?;
    let runs = reader.runs(&project.project_id)?;
    let metrics = reader.metrics(&runs[0])?;
    let query = seex::MetricQuery::new(seex::MetricRange::All(seex::MetricAxis::Step), 1024)?;
    let result = reader.query_metric(&runs[0].run_id, &metrics[0].metric_key, &query)?;
    println!(
        "SEEX_SCALE_VERIFY projects={} runs={} metrics={} source_count={} points={}",
        projects.len(),
        runs.len(),
        metrics.len(),
        result.source_count(),
        result.samples().len()
    );
    assert_eq!(runs.len(), expected_runs);
    assert_eq!(metrics.len(), expected_metrics);
    assert_eq!(result.source_count(), expected_points as u64);
    assert_eq!(result.samples().len(), 1024);
    Ok(())
}
