//! Opt-in release workloads for the pre-migration reporting engine.

use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use seex_core::engine::client::{NativeClient, NativeRun};
use seex_model::types::ProjectId;

const SAMPLES: usize = 7;
const CALIBRATION_TARGET: Duration = Duration::from_millis(10);
const QUEUE_CAPACITY: usize = 1_048_576;

fn fixture_path(label: &str) -> Result<PathBuf, Box<dyn Error>> {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos();
    Ok(std::env::temp_dir().join(format!(
        "seex-reporting-{label}-{}-{nonce}",
        std::process::id()
    )))
}

fn run_for_mode(label: &str) -> Result<(PathBuf, NativeClient, NativeRun), Box<dyn Error>> {
    let root = fixture_path(label)?;
    let client = NativeClient::open_with_metric_queue_capacity(&root, QUEUE_CAPACITY)?;
    let project = client.create_project(
        "reporting performance",
        Some(ProjectId::from_string(format!("project-{label}"))),
    )?;
    let run = client.create_run(&project.project_id, label, None)?;
    let run = client.run_handle(run);
    Ok((root, client, run))
}

fn percentile(sorted: &[f64], percent: usize) -> f64 {
    sorted[(sorted.len() * percent).div_ceil(100) - 1]
}

fn emit_metric(label: &str, batch_iterations: usize, mut raw_samples: Vec<f64>) {
    let raw = raw_samples.clone();
    raw_samples.sort_by(f64::total_cmp);
    let p50 = percentile(&raw_samples, 50);
    let p95 = percentile(&raw_samples, 95);
    let maximum = raw_samples[raw_samples.len() - 1];
    let mut deviations = raw_samples
        .iter()
        .map(|sample| (sample - p50).abs())
        .collect::<Vec<_>>();
    deviations.sort_by(f64::total_cmp);
    let mad = deviations[deviations.len() / 2];
    let relative_mad = mad / p50;
    println!(
        "SEEX_PERF {{\"schema_version\":2,\"record_type\":\"metric\",\
         \"domain\":\"reporting\",\"metric\":\"rust.{label}.admission\",\
         \"unit\":\"points/s\",\"direction\":\"higher\",\
         \"batch_iterations\":{batch_iterations},\"samples\":{SAMPLES},\
         \"raw_samples\":{raw:?},\"mad\":{mad},\"relative_mad\":{relative_mad},\
         \"p50\":{p50},\"p95\":{p95},\"max\":{maximum},\
         \"reliable\":{}}}",
        relative_mad <= 0.02,
    );
}

fn measure(
    label: &str,
    points_per_call: usize,
    mut operation: impl FnMut(usize) -> Result<(), Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    for index in 0..20 {
        operation(index)?;
    }
    let mut batch_iterations = 1_024;
    loop {
        let started = Instant::now();
        for index in 0..batch_iterations {
            operation(index)?;
        }
        if started.elapsed() >= CALIBRATION_TARGET {
            break;
        }
        batch_iterations *= 2;
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let started = Instant::now();
        for index in 0..batch_iterations {
            operation(sample * batch_iterations + index)?;
        }
        samples.push((batch_iterations * points_per_call) as f64 / started.elapsed().as_secs_f64());
    }
    emit_metric(label, batch_iterations * points_per_call, samples);
    Ok(())
}

fn emit_queue_check(client: &NativeClient) {
    let diagnostics = client.diagnostics();
    println!(
        "SEEX_PERF {{\"schema_version\":2,\"record_type\":\"check\",\
         \"domain\":\"reporting\",\"check\":\"rust.queue_admission\",\
         \"passed\":{},\"detail\":\"queue_full_errors={}\"}}",
        diagnostics.queue_full_errors == 0,
        diagnostics.queue_full_errors,
    );
}

fn emit_duration(label: &str, elapsed: Duration) {
    let value = elapsed.as_nanos();
    let reliable = elapsed >= CALIBRATION_TARGET;
    println!(
        "SEEX_PERF {{\"schema_version\":2,\"record_type\":\"metric\",\
         \"domain\":\"reporting\",\"metric\":\"rust.{label}\",\
         \"unit\":\"ns\",\"direction\":\"lower\",\"batch_iterations\":1,\
         \"samples\":1,\"raw_samples\":[{value}],\"mad\":0,\"relative_mad\":0,\
         \"p50\":{value},\"p95\":{value},\"max\":{value},\"reliable\":{reliable}}}"
    );
}

#[test]
#[ignore = "hardware-sensitive release reporting admission workload"]
fn reporting_admission_modes() -> Result<(), Box<dyn Error>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    for label in ["explicit_single", "implicit_single", "mapping_8"] {
        let (root, client, run) = run_for_mode(label)?;
        let mut implicit_step = 0_i64;
        measure(label, usize::from(label == "mapping_8") * 7 + 1, |index| {
            if label == "mapping_8" {
                for metric in 0..8 {
                    run.log_metric_at_step(&format!("metric-{metric}"), index as i64, 1.0)?;
                }
            } else {
                let step = if label == "implicit_single" {
                    let step = implicit_step;
                    implicit_step += 1;
                    step
                } else {
                    index as i64
                };
                run.log_metric_at_step("loss", step, 1.0)?;
            }
            Ok(())
        })?;
        emit_queue_check(&client);
        client.shutdown(None)?;
        fs::remove_dir_all(root)?;
    }
    Ok(())
}

#[test]
#[ignore = "fresh-process release reporting durability workload"]
fn reporting_durability_phases() -> Result<(), Box<dyn Error>> {
    const REPORTS: u64 = 1_000;
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    let (root, client, run) = run_for_mode("durability")?;
    println!("SEEX_RSS_PHASE warm");

    let admission_started = Instant::now();
    for step in 0..REPORTS {
        run.log_metric_at_step("loss", step as i64, step as f64)?;
    }
    emit_duration("queue_admission", admission_started.elapsed());
    println!("SEEX_RSS_PHASE admitted");

    let drain_started = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(120);
    while client.diagnostics().pending_reports != 0 {
        if Instant::now() >= deadline {
            return Err("reporting durability drain timed out".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    emit_duration("drain_persistence", drain_started.elapsed());
    println!("SEEX_RSS_PHASE cycles_done");

    let finalization_started = Instant::now();
    client.finish_run(&run.run_id)?;
    emit_duration("finalization", finalization_started.elapsed());
    let diagnostics = client.diagnostics();
    let passed = diagnostics.pending_reports == 0
        && diagnostics.persisted_reports == REPORTS
        && diagnostics.queue_full_errors == 0
        && diagnostics.last_flush_status == "succeeded";
    println!(
        "SEEX_PERF {{\"schema_version\":2,\"record_type\":\"check\",\
         \"domain\":\"reporting\",\"check\":\"rust.durability\",\
         \"passed\":{passed},\"detail\":\"persisted={},pending={},queue_full={}\"}}",
        diagnostics.persisted_reports, diagnostics.pending_reports, diagnostics.queue_full_errors,
    );
    println!("SEEX_RSS_PHASE final");
    client.shutdown(None)?;
    fs::remove_dir_all(root)?;
    Ok(())
}
