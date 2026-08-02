//! Opt-in release workloads for the public Rust Run SDK.

use std::error::Error as StdError;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use seex::{Client, LogOptions, RunHandle, RunOptions};

const SAMPLES: usize = 10;
const LOGICAL_RUNS: usize = 5;
const SAMPLES_PER_RUN: usize = SAMPLES / LOGICAL_RUNS;
const CALIBRATION_TARGET: Duration = Duration::from_millis(25);
const QUEUE_CAPACITY: usize = 1_048_576;
const MAPPING_KEYS: [&str; 8] = [
    "metric-0", "metric-1", "metric-2", "metric-3", "metric-4", "metric-5", "metric-6", "metric-7",
];

fn fixture_path(label: &str) -> Result<PathBuf, Box<dyn StdError>> {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos();
    Ok(std::env::temp_dir().join(format!(
        "seex-reporting-{label}-{}-{nonce}",
        std::process::id()
    )))
}

fn run_for_mode(label: &str) -> Result<(PathBuf, Client, RunHandle), Box<dyn StdError>> {
    let root = fixture_path(label)?;
    let client = Client::builder(&root)
        .metric_queue_capacity(QUEUE_CAPACITY)
        .open()?;
    let run = client.start_run(RunOptions::new(format!("project-{label}")))?;
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

fn wait_for_drain(client: &Client) -> Result<(), Box<dyn StdError>> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while client.diagnostics().pending_reports != 0 {
        if Instant::now() >= deadline {
            return Err("reporting admission drain timed out".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn measure_mode(
    label: &str,
    points_per_call: usize,
    operation: impl Fn(&RunHandle, usize) -> Result<(), seex::Error>,
) -> Result<(), Box<dyn StdError>> {
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut batch_iterations = 1_024;
    for run_index in 0..LOGICAL_RUNS {
        let fixture_label = format!("{label}-{run_index}");
        let (_root, client, run) = run_for_mode(&fixture_label)?;
        for index in 0..20 {
            operation(&run, index)?;
        }
        if run_index == 0 {
            loop {
                let started = Instant::now();
                for index in 0..batch_iterations {
                    operation(&run, index)?;
                }
                let elapsed = started.elapsed();
                if elapsed >= CALIBRATION_TARGET {
                    break;
                }
                batch_iterations *= 2;
            }
        }
        for _ in 0..SAMPLES_PER_RUN {
            let started = Instant::now();
            for index in 0..batch_iterations {
                operation(&run, index)?;
            }
            let elapsed = started.elapsed();
            if elapsed < CALIBRATION_TARGET {
                return Err("calibrated reporting sample completed in less than 25 ms".into());
            }
            samples.push((batch_iterations * points_per_call) as f64 / elapsed.as_secs_f64());
        }
        // Admission is isolated from persistence. Each ignored benchmark runs
        // in a fresh process, which releases the intentionally leaked writer.
        std::mem::forget(client);
    }
    println!();
    println!(
        "{}",
        metric_record(label, batch_iterations * points_per_call, &samples)
    );
    Ok(())
}

fn duration_record(label: &str, batch_iterations: usize, samples: &[Duration]) -> String {
    assert_eq!(samples.len(), SAMPLES, "reporting benchmark sample count");
    let raw = samples.iter().map(Duration::as_nanos).collect::<Vec<_>>();
    format!(
        "SEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\
         \"domain\":\"reporting\",\"metric\":\"rust.{label}\",\
         \"unit\":\"ns\",\"direction\":\"lower\",\
         \"batch_iterations\":{batch_iterations},\"samples\":{raw:?}}}"
    )
}

#[test]
fn metric_record_contains_only_raw_samples() {
    let record = metric_record("explicit_single", 10, &[1.0; SAMPLES]);
    let duration = duration_record("finalization", 3, &[Duration::from_millis(25); SAMPLES]);

    assert!(record.starts_with("SEEX_BENCH {\"schema_version\":3"));
    assert!(record.contains("\"samples\":[1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]"));
    assert!(!record.contains("p50"));
    assert!(duration.contains("\"metric\":\"rust.finalization\""));
}

#[test]
#[ignore = "hardware-sensitive release reporting admission workload"]
fn reporting_explicit_admission() -> Result<(), Box<dyn StdError>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    measure_mode("explicit_single", 1, |run, index| {
        run.log_with([("loss", 1.0)], LogOptions::new().step(index as i64))
    })
}

#[test]
#[ignore = "hardware-sensitive release reporting admission workload"]
fn reporting_implicit_admission() -> Result<(), Box<dyn StdError>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    measure_mode("implicit_single", 1, |run, _| run.log([("loss", 1.0)]))
}

#[test]
#[ignore = "hardware-sensitive release reporting admission workload"]
fn reporting_mapping_admission() -> Result<(), Box<dyn StdError>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    measure_mode("mapping_8", MAPPING_KEYS.len(), |run, index| {
        run.log_with(
            MAPPING_KEYS.map(|key| (key, 1.0)),
            LogOptions::new().step(index as i64),
        )
    })
}

#[test]
#[ignore = "fresh-process release reporting durability workload"]
fn reporting_durability_phases() -> Result<(), Box<dyn StdError>> {
    const REPORTS: usize = 1_000;
    const REPEATS_PER_SAMPLE: usize = 3;
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    let mut drain_samples = Vec::with_capacity(SAMPLES);
    let mut finalization_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let mut drain = Duration::ZERO;
        let mut finalization = Duration::ZERO;
        for repeat in 0..REPEATS_PER_SAMPLE {
            let label = format!("durability-{sample}-{repeat}");
            let (root, client, run) = run_for_mode(&label)?;
            for step in 0..REPORTS {
                run.log_with([("loss", step as f64)], LogOptions::new().step(step as i64))?;
            }
            let drain_started = Instant::now();
            wait_for_drain(&client)?;
            drain += drain_started.elapsed();
            let finalization_started = Instant::now();
            run.finish()?;
            finalization += finalization_started.elapsed();
            client.shutdown()?;
            fs::remove_dir_all(root)?;
        }
        drain_samples.push(drain);
        finalization_samples.push(finalization);
    }
    println!();
    println!(
        "{}",
        duration_record("drain_persistence", REPEATS_PER_SAMPLE, &drain_samples)
    );
    println!(
        "{}",
        duration_record("finalization", REPEATS_PER_SAMPLE, &finalization_samples)
    );
    Ok(())
}

#[test]
#[ignore = "fresh-process release reporting RSS workload"]
fn reporting_peak_rss() -> Result<(), Box<dyn StdError>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    let (root, client, run) = run_for_mode("rss")?;
    for step in 0..10_000 {
        run.log_with([("loss", step as f64)], LogOptions::new().step(step as i64))?;
    }
    wait_for_drain(&client)?;
    println!("SEEX_RSS_PHASE warm");
    thread::sleep(Duration::from_millis(60));
    for cycle in 0..5 {
        for offset in 0..10_000 {
            let step = 10_000 + cycle * 10_000 + offset;
            run.log_with([("loss", step as f64)], LogOptions::new().step(step as i64))?;
        }
        wait_for_drain(&client)?;
    }
    println!("SEEX_RSS_PHASE cycles_done");
    thread::sleep(Duration::from_millis(60));
    run.finish()?;
    println!("SEEX_RSS_PHASE final");
    thread::sleep(Duration::from_millis(60));
    client.shutdown()?;
    fs::remove_dir_all(root)?;
    Ok(())
}
