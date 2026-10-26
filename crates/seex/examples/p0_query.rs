//! Calibrated Step query probe used by the deterministic P0 workload.

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use seex::{
    CatalogBackend, MAX_CHART_POINTS, MetricAxis, MetricKey, MetricQuery, MetricRange, Reader,
    RunId,
};

const SAMPLE_COUNT: usize = 10;
const MIN_SAMPLE: Duration = Duration::from_millis(25);

struct Options {
    root: PathBuf,
    run_id: RunId,
    metric: MetricKey,
    data_path: Option<PathBuf>,
    catalog_backend: Option<CatalogBackend>,
    catalog_path: Option<PathBuf>,
    once: bool,
}

impl Options {
    fn parse() -> Result<Self, Box<dyn Error>> {
        let mut arguments = env::args().skip(1);
        let root = PathBuf::from(arguments.next().ok_or(Self::usage())?);
        let run_id = RunId::from_string(arguments.next().ok_or(Self::usage())?);
        let metric = MetricKey::from_string(arguments.next().ok_or(Self::usage())?);
        let mut options = Self {
            root,
            run_id,
            metric,
            data_path: None,
            catalog_backend: None,
            catalog_path: None,
            once: false,
        };
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--data-path" => {
                    options.data_path = Some(PathBuf::from(Self::value(&mut arguments)?))
                }
                "--catalog-backend" => {
                    options.catalog_backend = Some(match Self::value(&mut arguments)?.as_str() {
                        "duckdb" => CatalogBackend::DuckDb,
                        "sqlite" => CatalogBackend::Sqlite,
                        _ => return Err(Self::usage().into()),
                    });
                }
                "--catalog-path" => {
                    options.catalog_path = Some(PathBuf::from(Self::value(&mut arguments)?));
                }
                "--once" => options.once = true,
                _ => return Err(Self::usage().into()),
            }
        }
        Ok(options)
    }

    fn value(arguments: &mut impl Iterator<Item = String>) -> Result<String, Box<dyn Error>> {
        arguments.next().ok_or_else(|| Self::usage().into())
    }

    fn usage() -> &'static str {
        "usage: p0_query <root> <run-id> <metric> [--data-path URI] \
         [--catalog-backend duckdb|sqlite] [--catalog-path PATH] [--once]"
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = Options::parse()?;
    let mut builder = Reader::builder(&options.root);
    if let Some(data_path) = options.data_path {
        builder = builder.data_path(data_path);
    }
    if let Some(catalog_backend) = options.catalog_backend {
        builder = builder.catalog_backend(catalog_backend);
    }
    if let Some(catalog_path) = options.catalog_path {
        builder = builder.catalog_path(catalog_path);
    }
    let reader = builder.open()?;
    let query = MetricQuery::new(MetricRange::All(MetricAxis::Step), MAX_CHART_POINTS)?;

    let cold_start = Instant::now();
    let cold = reader.query_metric(&options.run_id, &options.metric, &query)?;
    let cold_ns = cold_start.elapsed().as_nanos();
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    if !options.once {
        for _ in 0..SAMPLE_COUNT {
            let mut iterations = 1_u64;
            loop {
                let started = Instant::now();
                for _ in 0..iterations {
                    std::hint::black_box(reader.query_metric(
                        &options.run_id,
                        &options.metric,
                        &query,
                    )?);
                }
                let elapsed = started.elapsed();
                if elapsed >= MIN_SAMPLE {
                    let elapsed_ns = elapsed.as_nanos();
                    samples.push((
                        iterations,
                        elapsed_ns,
                        elapsed_ns as f64 / iterations as f64,
                    ));
                    break;
                }
                iterations = iterations
                    .checked_mul(2)
                    .ok_or("query calibration overflow")?;
            }
        }
    }
    let samples_json = samples
        .iter()
        .map(|(iterations, elapsed_ns, ns_per_op)| {
            format!(
                "{{\"iterations\":{iterations},\"elapsed_ns\":{elapsed_ns},\"ns_per_op\":{ns_per_op}}}"
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "{{\"schema_version\":2,\"record_type\":\"query_probe\",\"cold_query_ns\":{cold_ns},\"source_count\":{},\"points\":{},\"hot_samples\":[{samples_json}]}}",
        cold.source_count(),
        cold.samples().len(),
    );
    Ok(())
}
