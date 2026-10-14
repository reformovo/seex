//! Opt-in release workloads for the public Rust Run SDK.

use std::error::Error as StdError;
use std::fs;
use std::hint::black_box;
use std::io::{self, Write};
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
    let isolated_process = std::env::var_os("SEEX_REPORTING_ADMISSION_ISOLATED").is_some();
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut batch_iterations = 1_024;
    for run_index in 0..LOGICAL_RUNS {
        let fixture_label = format!("{label}-{run_index}");
        let (root, client, run) = run_for_mode(&fixture_label)?;
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
        if isolated_process {
            std::mem::forget(client);
        } else {
            client.shutdown()?;
            fs::remove_dir_all(root)?;
        }
    }
    println!();
    println!(
        "{}",
        metric_record(label, batch_iterations * points_per_call, &samples)
    );
    if isolated_process {
        io::stdout().flush()?;
        std::process::exit(0);
    }
    Ok(())
}

fn duration_record(label: &str, batch_iterations: usize, samples: &[Duration]) -> String {
    assert_eq!(samples.len(), SAMPLES, "reporting benchmark sample count");
    assert!(batch_iterations > 0, "duration batches must contain a Run");
    let raw = samples
        .iter()
        .map(|sample| sample.as_nanos() as f64 / batch_iterations as f64)
        .collect::<Vec<_>>();
    format!(
        "SEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\
         \"domain\":\"reporting\",\"metric\":\"rust.{label}\",\
         \"unit\":\"ns/op\",\"direction\":\"lower\",\
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
    assert!(duration.contains("\"unit\":\"ns/op\""));
    assert!(duration.contains("\"batch_iterations\":3"));
    assert!(duration.contains("8333333.333333333"));
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

fn calibrate_durability(
    mut measure: impl FnMut(usize) -> Result<(Duration, Duration), Box<dyn StdError>>,
) -> Result<usize, Box<dyn StdError>> {
    const MAX_RUNS_PER_BATCH: usize = 32;
    let mut repeats = 1;
    loop {
        let (drain, finish) = measure(repeats)?;
        if drain >= CALIBRATION_TARGET && finish >= CALIBRATION_TARGET {
            return Ok(repeats);
        }
        if repeats == MAX_RUNS_PER_BATCH {
            return Err("durability calibration exceeded 32 Runs per batch".into());
        }
        repeats *= 2;
    }
}

#[test]
fn durability_calibration_requires_both_phases_and_bounds_work() {
    let mut attempted = Vec::new();
    let repeats = calibrate_durability(|runs| {
        attempted.push(runs);
        Ok((
            Duration::from_millis(runs as u64 * 10),
            Duration::from_millis(30),
        ))
    })
    .unwrap();
    assert_eq!(repeats, 4);
    assert_eq!(attempted, [1, 2, 4]);
    assert!(calibrate_durability(|_| Ok((Duration::from_millis(30), Duration::ZERO))).is_err());
    assert!(calibrate_durability(|_| Err("measurement failed".into())).is_err());
}

#[test]
#[ignore = "fresh-process release reporting durability workload"]
fn reporting_durability_phases() -> Result<(), Box<dyn StdError>> {
    const REPORTS: usize = 1_000;
    const WORKLOAD_BUDGET: Duration = Duration::from_secs(25);
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    let started = Instant::now();
    let mut run_index = 0;
    let mut measure_batch = |repeats| -> Result<(Duration, Duration), Box<dyn StdError>> {
        let mut drain = Duration::ZERO;
        let mut finalization = Duration::ZERO;
        for _ in 0..repeats {
            if started.elapsed() >= WORKLOAD_BUDGET {
                return Err("durability workload exceeded its 25-second budget".into());
            }
            let label = format!("durability-{run_index}");
            run_index += 1;
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
        Ok((drain, finalization))
    };
    // Pilot runs are excluded. Both metrics use the same calibrated Run count.
    let repeats = calibrate_durability(&mut measure_batch)?;
    let mut drain_samples = Vec::with_capacity(SAMPLES);
    let mut finalization_samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let (drain, finalization) = measure_batch(repeats)?;
        if drain < CALIBRATION_TARGET || finalization < CALIBRATION_TARGET {
            return Err("durability sample completed in less than 25 ms".into());
        }
        drain_samples.push(drain);
        finalization_samples.push(finalization);
    }
    if started.elapsed() >= WORKLOAD_BUDGET {
        return Err("durability workload exceeded its 25-second budget".into());
    }
    println!();
    println!(
        "{}",
        duration_record("drain_persistence", repeats, &drain_samples)
    );
    println!(
        "{}",
        duration_record("finalization", repeats, &finalization_samples)
    );
    Ok(())
}

#[test]
#[ignore = "fresh-store bounded batch candidate diagnostic"]
fn reporting_bounded_batch_pipeline() -> Result<(), Box<dyn StdError>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    const REPORTS: usize = 32_768;
    let started = Instant::now();
    let mut admission_samples = Vec::with_capacity(SAMPLES);
    let mut persistence_samples = Vec::with_capacity(SAMPLES);
    let mut finish_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        if started.elapsed() >= Duration::from_secs(25) {
            return Err("bounded batch diagnostic exceeded 25 seconds".into());
        }
        let root = fixture_path(&format!("bounded-batch-{sample}"))?;
        let client = Client::builder(&root).open()?;
        let run = client.start_run(RunOptions::new("pipeline"))?;
        let begin = Instant::now();
        for step in 0..REPORTS as i64 {
            run.log_with([("loss", step as f64)], LogOptions::new().step(step))?;
        }
        let admission = begin.elapsed();
        let pending = client.diagnostics().pending_reports;
        wait_for_drain(&client)?;
        let persisted = begin.elapsed();
        let finish_started = Instant::now();
        run.finish()?;
        let finish = finish_started.elapsed();
        assert_eq!(client.diagnostics().pending_reports, 0);
        assert_eq!(client.diagnostics().persisted_reports, REPORTS as u64);
        if persisted < CALIBRATION_TARGET || finish < CALIBRATION_TARGET {
            return Err("bounded batch persistence/finish sample below 25 ms".into());
        }
        println!(
            "bounded_sample={sample},reports={REPORTS},pending_before_drain={pending},admission_ns={},admission_to_persisted_ns={},finish_ns={}",
            admission.as_nanos(),
            persisted.as_nanos(),
            finish.as_nanos()
        );
        admission_samples.push(REPORTS as f64 / admission.as_secs_f64());
        persistence_samples.push(REPORTS as f64 / persisted.as_secs_f64());
        finish_samples.push(finish.as_nanos() as f64);
        client.shutdown()?;
        fs::remove_dir_all(root)?;
    }
    println!(
        "SEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\"domain\":\"reporting\",\"metric\":\"rust.bounded_batch.admission\",\"unit\":\"points/s\",\"direction\":\"higher\",\"batch_iterations\":{REPORTS},\"samples\":{admission_samples:?}}}"
    );
    println!(
        "SEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\"domain\":\"reporting\",\"metric\":\"rust.bounded_batch.persistence\",\"unit\":\"points/s\",\"direction\":\"higher\",\"batch_iterations\":{REPORTS},\"samples\":{persistence_samples:?}}}"
    );
    println!(
        "SEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\"domain\":\"reporting\",\"metric\":\"rust.bounded_batch.finish\",\"unit\":\"ns\",\"direction\":\"lower\",\"batch_iterations\":1,\"samples\":{finish_samples:?}}}"
    );
    Ok(())
}

#[test]
#[ignore = "large calibrated reporting baseline; retained store required"]
fn reporting_large_baseline() -> Result<(), Box<dyn StdError>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    let root = PathBuf::from(std::env::var("SEEX_LARGE_ROOT")?);
    fs::create_dir(&root)?; // Refuse an existing store, never append to user data.
    let client = Client::builder(&root).open()?;
    let run = client.start_run(RunOptions::new("large-baseline").id("large-run"))?;
    let started = Instant::now();
    let mut step = 0_i64;
    let mut measure = |reports: usize| -> Result<(Duration, Duration, u64), Box<dyn StdError>> {
        let begin = Instant::now();
        let mut admission = Duration::ZERO;
        let mut peak_pending = 0;
        for _ in 0..reports / 32_768 {
            if started.elapsed() >= Duration::from_secs(110) {
                return Err("large baseline exceeded 110-second cooperative budget".into());
            }
            let log_started = Instant::now();
            for _ in 0..32_768 {
                run.log_with([("loss", 1.0)], LogOptions::new().step(step))?;
                step += 1;
            }
            admission += log_started.elapsed();
            peak_pending = peak_pending.max(client.diagnostics().pending_reports);
            wait_for_drain(&client)?;
        }
        Ok((admission, begin.elapsed(), peak_pending))
    };
    let mut reports = 1_048_576;
    loop {
        let (admission, persisted, pending) = measure(reports)?;
        println!(
            "large_pilot,reports={reports},admission_ns={},persistence_ns={},pending={pending}",
            admission.as_nanos(),
            persisted.as_nanos()
        );
        if admission >= CALIBRATION_TARGET && persisted >= CALIBRATION_TARGET {
            break;
        }
        if reports >= 4_194_304 {
            return Err("large admission calibration exceeded bound".into());
        }
        reports *= 2;
    }
    let mut admission_samples = Vec::with_capacity(SAMPLES);
    let mut persistence_samples = Vec::with_capacity(SAMPLES);
    for sample in 0..SAMPLES {
        let (admission, persisted, pending) = measure(reports)?;
        if admission < CALIBRATION_TARGET || persisted < CALIBRATION_TARGET {
            return Err("large sample below 25 ms floor".into());
        }
        admission_samples.push(reports as f64 / admission.as_secs_f64());
        persistence_samples.push(reports as f64 / persisted.as_secs_f64());
        println!(
            "large_sample,index={sample},reports={reports},admission_ns={},persistence_ns={},observed_pending={pending}",
            admission.as_nanos(),
            persisted.as_nanos()
        );
    }
    let finish = Instant::now();
    run.finish()?;
    let finish_ns = finish.elapsed().as_nanos();
    assert_eq!(client.diagnostics().persisted_reports, step as u64);
    assert_eq!(client.diagnostics().pending_reports, 0);
    assert_eq!(client.diagnostics().queue_full_errors, 0);
    println!("{}", metric_record("large", reports, &admission_samples));
    println!(
        "SEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\"domain\":\"reporting\",\"metric\":\"rust.large.persistence\",\"unit\":\"points/s\",\"direction\":\"higher\",\"batch_iterations\":{reports},\"samples\":{persistence_samples:?}}}"
    );
    println!(
        "large_final,total_observations={step},measured_observations={},finish_ns={finish_ns},wall_ns={}",
        reports * SAMPLES,
        started.elapsed().as_nanos()
    );
    client.shutdown()?;
    Ok(())
}

#[test]
#[ignore = "low-rate release persistence diagnostic"]
fn reporting_low_rate_persistence() -> Result<(), Box<dyn StdError>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    let root = fixture_path("low-rate")?;
    let client = Client::builder(&root).open()?;
    let run = client.start_run(RunOptions::new("low-rate"))?;
    for step in 0..10 {
        let started = Instant::now();
        run.log_with(
            (0..32).map(|key| (format!("key-{key}"), step as f64)),
            LogOptions::new().step(step),
        )?;
        wait_for_drain(&client)?;
        assert_eq!(
            client.diagnostics().persisted_reports,
            (step as u64 + 1) * 32
        );
        println!(
            "low_rate_step={step},enqueue_to_persisted_ns={}",
            started.elapsed().as_nanos()
        );
    }
    run.finish()?;
    client.shutdown()?;
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
#[ignore = "subprocess helper for shared-store reporting diagnostic"]
fn reporting_shared_store_child() -> Result<(), Box<dyn StdError>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    let root = PathBuf::from(std::env::var("SEEX_DIAGNOSTIC_ROOT")?);
    let writer = std::env::var("SEEX_DIAGNOSTIC_WRITER")?;
    let client = Client::builder(&root).open()?;
    let run =
        client.start_run(RunOptions::new("shared-diagnostic").id(format!("writer-{writer}")))?;
    fs::write(root.join(format!("ready-{writer}")), "ready")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !root.join("start").exists() {
        if Instant::now() >= deadline {
            return Err("shared diagnostic barrier timed out".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    let started = Instant::now();
    let paced = std::env::var_os("SEEX_DIAGNOSTIC_PACED_KEYS").is_some();
    let mut admission = Duration::ZERO;
    let mut max_pending = 0;
    if paced {
        for group in 0..16 {
            let begin = Instant::now();
            for step in group * 64..(group + 1) * 64 {
                run.log_with(
                    (0..32).map(|key| (format!("key-{key}"), step as f64)),
                    LogOptions::new().step(step),
                )?;
            }
            admission += begin.elapsed();
            max_pending = max_pending.max(client.diagnostics().pending_reports);
            wait_for_drain(&client)?;
        }
    } else {
        for step in 0..32_768 {
            run.log_with([("loss", step as f64)], LogOptions::new().step(step))?;
        }
        admission = started.elapsed();
        max_pending = client.diagnostics().pending_reports;
    }
    println!("shared_mode_paced={paced},max_observed_pending={max_pending}");
    let pending = client.diagnostics().pending_reports;
    wait_for_drain(&client)?;
    let persisted = started.elapsed();
    let finish_started = Instant::now();
    run.finish()?;
    assert_eq!(client.diagnostics().persisted_reports, 32_768);
    assert_eq!(client.diagnostics().pending_reports, 0);
    println!(
        "shared_writer={writer},admission_ns={},pending_before_drain={pending},persistence_ns={},finish_ns={}",
        admission.as_nanos(),
        persisted.as_nanos(),
        finish_started.elapsed().as_nanos()
    );
    client.shutdown()?;
    Ok(())
}

#[test]
#[ignore = "fresh-process release RSS workload"]
fn reporting_peak_rss() -> Result<(), Box<dyn StdError>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "workload requires --release"
    );
    let root = fixture_path("rss")?;
    let client = Client::builder(&root).open()?;
    let run = client.start_run(RunOptions::new("rss"))?;
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
