//! Opt-in 1 Run × 1 Metric Reader benchmark for schema-v3 performance gates.

use std::env;
use std::error::Error;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use seex::{MetricKey, MetricQuery, MetricRange, Reader, RelativeTime, Step, Timestamp};
use seex_model::run::RunId;
use seex_storage::bootstrap::CatalogBackend;

const POINTS: i64 = 1_000_000;
const SAMPLES: usize = 10;
const TARGET: Duration = Duration::from_millis(25);
const EPOCH_MILLIS: i64 = 1_700_000_000_000;

#[derive(Clone, Copy)]
enum Axis {
    Step,
    RelativeTime,
    Timestamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReaderBoundary {
    Detail,
    Overview,
}

const fn reader_boundary(narrow: bool) -> ReaderBoundary {
    if narrow {
        ReaderBoundary::Detail
    } else {
        ReaderBoundary::Overview
    }
}

impl Axis {
    fn selected() -> Result<Self, Box<dyn Error>> {
        match env::var("SEEX_QUERY_BENCH_AXIS")
            .as_deref()
            .unwrap_or("step")
        {
            "step" => Ok(Self::Step),
            "relative_time" => Ok(Self::RelativeTime),
            "timestamp" => Ok(Self::Timestamp),
            value => Err(format!("unsupported query benchmark axis: {value}").into()),
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::RelativeTime => "relative_time",
            Self::Timestamp => "timestamp",
        }
    }
}

fn backend() -> Result<CatalogBackend, Box<dyn Error>> {
    match env::var("SEEX_QUERY_BENCH_BACKEND")
        .as_deref()
        .unwrap_or("duckdb")
    {
        "duckdb" => Ok(CatalogBackend::DuckDb),
        "sqlite" => Ok(CatalogBackend::Sqlite),
        value => Err(format!("unsupported query benchmark backend: {value}").into()),
    }
}

fn backend_label(backend: CatalogBackend) -> &'static str {
    match backend {
        CatalogBackend::DuckDb => "duckdb",
        CatalogBackend::Sqlite => "sqlite",
    }
}

fn dataset_root() -> Result<PathBuf, Box<dyn Error>> {
    env::var_os("SEEX_QUERY_BENCH_ROOT")
        .map(PathBuf::from)
        .ok_or_else(|| "SEEX_QUERY_BENCH_ROOT must name the prepared dataset".into())
}

fn selected_ranges() -> Result<Vec<bool>, Box<dyn Error>> {
    match env::var("SEEX_QUERY_BENCH_RANGE")
        .as_deref()
        .unwrap_or("narrow")
    {
        "narrow" => Ok(vec![true]),
        "full" => Ok(vec![false]),
        "both" => Ok(vec![true, false]),
        value => Err(format!("unsupported query benchmark range: {value}").into()),
    }
}

fn query(reader: &Reader, axis: Axis, narrow: bool) -> Result<usize, Box<dyn Error>> {
    let (start, end) = if narrow {
        (450_000, 550_000)
    } else {
        (0, POINTS)
    };
    let range = match axis {
        Axis::Step => MetricRange::Steps {
            start: Step::new(start),
            end: Step::new(end),
        },
        Axis::RelativeTime => MetricRange::RelativeTime {
            start: RelativeTime::from_millis(start),
            end: RelativeTime::from_millis(end),
        },
        Axis::Timestamp => MetricRange::Timestamps {
            start: Timestamp::from_millis(EPOCH_MILLIS + start),
            end: Timestamp::from_millis(EPOCH_MILLIS + end),
        },
    };
    let query = MetricQuery::new(range, Some(5_000))?;
    let series = match reader_boundary(narrow) {
        ReaderBoundary::Detail => reader.query_metric_for_desktop(
            &RunId::from_string("run-1"),
            &MetricKey::from_string("loss"),
            &query,
        )?,
        ReaderBoundary::Overview => reader.query_metric_overview_for_desktop(
            &RunId::from_string("run-1"),
            &MetricKey::from_string("loss"),
            &query,
        )?,
    };
    Ok(series.samples().len())
}

fn capture(reader: &Reader, axis: Axis, narrow: bool) -> Result<(usize, Vec<f64>), Box<dyn Error>> {
    let mut iterations = 1;
    loop {
        let started = Instant::now();
        for _ in 0..iterations {
            let points = query(reader, axis, narrow)?;
            if points == 0 || points > 5_000 + usize::from(narrow) * 2 {
                return Err("Reader benchmark violated its point budget".into());
            }
        }
        let elapsed = started.elapsed();
        if elapsed >= TARGET {
            break;
        }
        iterations *= 2;
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for _ in 0..iterations {
            black_box(query(reader, axis, narrow)?);
        }
        let elapsed = started.elapsed();
        if elapsed < TARGET {
            return Err("calibrated Reader sample completed in less than 25 ms".into());
        }
        samples.push(elapsed.as_nanos() as f64 / iterations as f64);
    }
    Ok((iterations, samples))
}

fn record(
    backend: CatalogBackend,
    axis: Axis,
    narrow: bool,
    iterations: usize,
    samples: &[f64],
) -> String {
    assert_eq!(samples.len(), SAMPLES, "Reader benchmark sample count");
    let range = if narrow { "narrow" } else { "full" };
    format!(
        "SEEX_BENCH {{\"schema_version\":3,\"record_type\":\"metric\",\
         \"domain\":\"query\",\"metric\":\"{}.reader.{}.{}\",\
         \"unit\":\"ns/op\",\"direction\":\"lower\",\
         \"batch_iterations\":{iterations},\"samples\":{samples:?}}}",
        backend_label(backend),
        axis.label(),
        range,
    )
}

#[test]
fn reader_record_contains_only_ten_raw_v3_samples() {
    let record = record(CatalogBackend::DuckDb, Axis::Step, true, 2, &[1.0; SAMPLES]);
    assert!(record.starts_with("SEEX_BENCH {\"schema_version\":3"));
    assert!(record.contains("duckdb.reader.step.narrow"));
    assert!(!record.contains("p50"));
}

#[test]
fn full_range_measures_the_overview_boundary() {
    assert_eq!(reader_boundary(true), ReaderBoundary::Detail);
    assert_eq!(reader_boundary(false), ReaderBoundary::Overview);
}

#[test]
#[ignore = "hardware-sensitive release Reader benchmark"]
fn reader_benchmark() -> Result<(), Box<dyn Error>> {
    assert!(
        black_box(!cfg!(debug_assertions)),
        "benchmark requires --release"
    );
    let backend = backend()?;
    let axis = Axis::selected()?;
    let reader = Reader::builder(dataset_root()?).open()?;
    for narrow in selected_ranges()? {
        let (iterations, samples) = capture(&reader, axis, narrow)?;
        println!("{}", record(backend, axis, narrow, iterations, &samples));
    }
    Ok(())
}
