//! Opt-in release workloads for the pre-migration reporting engine.

use std::error::Error;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use seex_core::engine::client::{NativeClient, NativeRun};
use seex_model::types::ProjectId;

const SAMPLES: usize = 10;
const CALIBRATION_TARGET: Duration = Duration::from_millis(25);
const QUEUE_CAPACITY: usize = 1_048_576;
const MAPPING_KEYS: [&str; 8] = [
    "metric-0", "metric-1", "metric-2", "metric-3", "metric-4", "metric-5", "metric-6", "metric-7",
];

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

fn metric_record(label: &str, batch_iterations: usize, samples: &[f64]) -> String {
    assert_eq!(samples.len(), SAMPLES, "reporting benchmark sample count");
    format!(
        "SEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\
         \"domain\":\"reporting\",\"metric\":\"rust.{label}.admission\",\
         \"unit\":\"points/s\",\"direction\":\"higher\",\
         \"batch_iterations\":{batch_iterations},\"samples\":{samples:?}}}"
    )
}

fn measure(
    label: &str,
    points_per_call: usize,
    mut operation: impl FnMut(usize) -> Result<(), Box<dyn Error>>,
    mut settle: impl FnMut() -> Result<(), Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    for index in 0..20 {
        operation(index)?;
    }
    let mut batch_iterations = 1_024;
    let (batch_iterations, first_sample) = loop {
        let started = Instant::now();
        for index in 0..batch_iterations {
            operation(index)?;
        }
        let elapsed = started.elapsed();
        settle()?;
        if elapsed >= CALIBRATION_TARGET {
            break (
                batch_iterations,
                (batch_iterations * points_per_call) as f64 / elapsed.as_secs_f64(),
            );
        }
        batch_iterations *= 2;
    };
    let mut samples = vec![first_sample];
    for _ in 1..SAMPLES {
        let started = Instant::now();
        for index in 0..batch_iterations {
            operation(index)?;
        }
        let elapsed = started.elapsed();
        settle()?;
        if elapsed < CALIBRATION_TARGET {
            return Err("calibrated reporting sample completed in less than 25 ms".into());
        }
        samples.push((batch_iterations * points_per_call) as f64 / elapsed.as_secs_f64());
    }
    println!(
        "{}",
        metric_record(label, batch_iterations * points_per_call, &samples)
    );
    Ok(())
}

fn wait_for_drain(client: &NativeClient) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while client.diagnostics().pending_reports != 0 {
        if Instant::now() >= deadline {
            return Err("reporting admission drain timed out".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn duration_record(label: &str, samples: &[Duration]) -> String {
    assert_eq!(samples.len(), SAMPLES, "reporting benchmark sample count");
    let raw = samples.iter().map(Duration::as_nanos).collect::<Vec<_>>();
    format!(
        "SEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\
         \"domain\":\"reporting\",\"metric\":\"rust.{label}\",\
         \"unit\":\"ns\",\"direction\":\"lower\",\"batch_iterations\":1,\
         \"samples\":{raw:?}}}"
    )
}

fn emit_durations(label: &str, samples: &[Duration]) {
    println!("{}", duration_record(label, samples));
}

#[test]
fn metric_record_contains_only_raw_samples() {
    let record = metric_record("explicit_single", 10, &[1.0; SAMPLES]);
    let duration = duration_record("finalization", &[Duration::from_millis(25); SAMPLES]);

    assert!(record.starts_with("SEEX_BENCH {\"schema_version\":3"));
    assert!(record.contains("\"samples\":[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]"));
    assert!(!record.contains("p50"));
    assert!(duration.contains("\"metric\":\"rust.finalization\""));
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
        measure(
            label,
            usize::from(label == "mapping_8") * 7 + 1,
            |index| {
                if label == "mapping_8" {
                    for metric in MAPPING_KEYS {
                        run.log_metric_at_step(metric, index as i64, 1.0)?;
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
            },
            || wait_for_drain(&client),
        )?;
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
    let mut drain_samples = Vec::with_capacity(SAMPLES);
    let mut finalization_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let label = format!("durability-{sample}");
        let (root, client, run) = run_for_mode(&label)?;
        for step in 0..REPORTS {
            run.log_metric_at_step("loss", step as i64, step as f64)?;
        }
        let drain_started = Instant::now();
        wait_for_drain(&client)?;
        drain_samples.push(drain_started.elapsed());
        let finalization_started = Instant::now();
        client.finish_run(&run.run_id)?;
        finalization_samples.push(finalization_started.elapsed());
        client.shutdown(None)?;
        fs::remove_dir_all(root)?;
    }
    emit_durations("drain_persistence", &drain_samples);
    emit_durations("finalization", &finalization_samples);
    Ok(())
}
