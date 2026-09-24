//! Calibrated Step query probe used by the deterministic P0 workload.

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use seex::{
    CatalogBackend, MAX_CHART_POINTS, MetricAxis, MetricKey, MetricQuery, MetricRange, Reader,
    RunId, S3Options,
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
    s3: S3Options,
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
            s3: S3Options::new(),
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
                "--s3-endpoint" => {
                    options.s3 = options.s3.endpoint(Self::value(&mut arguments)?);
                }
                "--s3-access-key-id" => {
                    options.s3 = options.s3.access_key_id(Self::value(&mut arguments)?);
                }
                "--s3-secret-access-key" => {
                    options.s3 = options.s3.secret_access_key(Self::value(&mut arguments)?);
                }
                "--s3-region" => options.s3 = options.s3.region(Self::value(&mut arguments)?),
                "--s3-path-style" => {
                    options.s3 = options.s3.path_style(Self::bool_value(&mut arguments)?)
                }
                "--s3-use-ssl" => {
                    options.s3 = options.s3.use_ssl(Self::bool_value(&mut arguments)?)
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

    fn bool_value(arguments: &mut impl Iterator<Item = String>) -> Result<bool, Box<dyn Error>> {
        match Self::value(arguments)?.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(Self::usage().into()),
        }
    }

    fn usage() -> &'static str {
        "usage: p0_query <root> <run-id> <metric> [--data-path URI] [--catalog-backend duckdb|sqlite] [--catalog-path PATH] [--s3-endpoint URL] [--s3-access-key-id ID] [--s3-secret-access-key SECRET] [--s3-region REGION] [--s3-path-style true|false] [--s3-use-ssl true|false] [--once]"
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
    let reader = builder.s3_options(options.s3).open()?;
    let query = MetricQuery::new(MetricRange::All(MetricAxis::Step), MAX_CHART_POINTS)?;

    let cold_start = Instant::now();
    let cold = reader.query_metric(&options.run_id, &options.metric, &query)?;
    let cold_ns = cold_start.elapsed().as_nanos();
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    if !options.once {
        for _ in 0..SAMPLE_COUNT {
            let mut iterations = 1_u64;
            let started = Instant::now();
            loop {
                for _ in 0..iterations {
                    std::hint::black_box(reader.query_metric(
                        &options.run_id,
                        &options.metric,
                        &query,
                    )?);
                }
                if started.elapsed() >= MIN_SAMPLE {
                    break;
                }
                iterations = iterations.saturating_mul(2);
            }
            samples.push(started.elapsed().as_nanos());
        }
    }
    let samples_json = samples
        .iter()
        .map(u128::to_string)
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "{{\"cold_ns\":{cold_ns},\"source_count\":{},\"points\":{},\"hot_samples_ns\":[{samples_json}],\"plan\":\"step_lttb_1024\"}}",
        cold.source_count(),
        cold.samples().len(),
    );
    Ok(())
}
