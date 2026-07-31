use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use seex_app::data::query::{
    CurveAxis, CurveSelection, CurveSnapshot, DetailRequest, OverviewRequest,
};
use seex_app::data::worker::{
    Generation, ReadEventReceiver, ReadRequest, ReadSnapshot, ReadWorker,
};
use seex_app::domain::{DataSourceId, RunRef};
use seex_core::engine::client::NativeClient;
use seex_model::alignment::{
    AlignmentAxis, AlignmentQuery, AlignmentReason, AlignmentReduction, AlignmentViewport,
};
use seex_model::metric::{MetricKey, MetricQuery, ReductionPolicy, Step};
use seex_model::run::RunId;
use seex_model::types::ProjectId;
use seex_storage::bootstrap::{
    CatalogBackend, NativeStorageConfig, open_existing_native_connection_with_config,
    open_native_connection_with_config,
};
use seex_storage::config::resolve_storage_config;
use seex_storage::{ParquetMetricReader, ParquetSource, ProjectConnection, ProjectMetricReader};

mod support;

use support::receive_event;

const RUNS: usize = 10;
const SOURCE_POINTS: i64 = 1_000_000;
const TRACE_METRICS: [&str; 6] = [
    "loss",
    "accuracy",
    "latency",
    "memory",
    "throughput",
    "error",
];
const QUERY_TIMEOUT: Duration = Duration::from_secs(300);
const FIXTURE_MANIFEST: &str = ".seex/performance-fixture-v2.txt";
const FIXTURE_EPOCH_MILLIS: i64 = 1_700_000_000_000;

fn backend_name(backend: CatalogBackend) -> &'static str {
    match backend {
        CatalogBackend::DuckDb => "duckdb",
        CatalogBackend::Sqlite => "sqlite",
    }
}

fn fixture_manifest(backend: CatalogBackend, metrics: &[&str], source_points: i64) -> String {
    format!(
        "schema=v2\nsemantics=lww-spikes-v1\nbackend={}\nruns={RUNS}\nmetrics={}\neffective_points_per_series={source_points}\n",
        backend_name(backend),
        metrics.join(",")
    )
}

fn validate_fixture(
    root: &Path,
    backend: CatalogBackend,
    metrics: &[&str],
    source_points: i64,
) -> Result<(), Box<dyn Error>> {
    let expected = fixture_manifest(backend, metrics, source_points);
    let actual = fs::read_to_string(root.join(FIXTURE_MANIFEST))?;
    if actual != expected {
        return Err("retained fixture manifest does not match the requested workload".into());
    }
    let resolved = resolve_storage_config(root, None, None, None)?;
    let connection = open_existing_native_connection_with_config(
        NativeStorageConfig::with_backend_and_s3_config(
            resolved.catalog_backend,
            root,
            resolved.catalog_path,
            resolved.data_path,
            None,
        ),
    )?;
    for metric in metrics {
        let matching: i64 = connection.query_row(
            "SELECT count(*) FROM seex_metric_aggregates
             WHERE metric_key = ? AND effective_count = ? AND last_step = ?",
            (*metric, source_points, source_points - 1),
            |row| row.get(0),
        )?;
        if matching != RUNS as i64 {
            return Err(format!("fixture metric {metric} has {matching} valid Runs").into());
        }
    }
    Ok(())
}

fn open_fixture_connection(root: &Path) -> Result<ProjectConnection, Box<dyn Error>> {
    let resolved = resolve_storage_config(root, None, None, None)?;
    Ok(ProjectConnection::new(
        open_existing_native_connection_with_config(
            NativeStorageConfig::with_backend_and_s3_config(
                resolved.catalog_backend,
                root,
                resolved.catalog_path,
                resolved.data_path,
                None,
            ),
        )?,
    ))
}

fn fixture_path(backend: CatalogBackend, root_variable: &str) -> Result<PathBuf, Box<dyn Error>> {
    let name = match backend {
        CatalogBackend::DuckDb => "duckdb",
        CatalogBackend::Sqlite => "sqlite",
    };
    let base = std::env::var_os(root_variable)
        .ok_or_else(|| format!("{root_variable} must name a retained fixture directory"))?;
    Ok(PathBuf::from(base).join(name))
}

fn run_ids() -> Vec<RunId> {
    (0..RUNS)
        .map(|index| RunId::from_string(format!("run-{index}")))
        .collect()
}

fn read_fixture(
    backend: CatalogBackend,
    root_variable: &str,
    metrics: &[&str],
    source_points: i64,
) -> Result<(PathBuf, Vec<RunId>), Box<dyn Error>> {
    let root = fixture_path(backend, root_variable)?;
    let config = root.join(".seex/config.toml");
    if !config.is_file() {
        return Err(format!(
            "retained fixture is missing {}; run prepare_large_native_series_fixture first",
            config.display()
        )
        .into());
    }
    validate_fixture(&root, backend, metrics, source_points)?;
    Ok((root, run_ids()))
}

fn prepare_fixture(
    backend: CatalogBackend,
    root_variable: &str,
    metrics: &[&str],
    source_points: i64,
) -> Result<(PathBuf, Vec<RunId>), Box<dyn Error>> {
    let root = fixture_path(backend, root_variable)?;
    let run_ids = run_ids();
    if root.join(".seex/config.toml").is_file() {
        validate_fixture(&root, backend, metrics, source_points)?;
        return Ok((root, run_ids));
    }
    if root.exists() && fs::read_dir(&root)?.next().is_some() {
        return Err(format!("refusing to replace incomplete fixture: {}", root.display()).into());
    }
    fs::create_dir_all(&root)?;
    let seex_dir = root.join(".seex");
    fs::create_dir_all(&seex_dir)?;
    let catalog_name = match backend {
        CatalogBackend::DuckDb => "catalog.ducklake",
        CatalogBackend::Sqlite => "catalog.sqlite",
    };
    fs::write(
        seex_dir.join("config.toml"),
        format!(
            "catalog_backend = \"{}\"\ncatalog_path = \"custom/{catalog_name}\"\n\
             data_path = \"custom/data\"\n",
            if backend == CatalogBackend::DuckDb {
                "duckdb"
            } else {
                "sqlite"
            }
        ),
    )?;
    let catalog_path = root.join("custom").join(catalog_name);
    let data_path = root.join("custom/data");
    let client = NativeClient::open_with_catalog_backend_storage_config(
        &root,
        backend,
        Some(catalog_path.clone()),
        Some(data_path.clone()),
        None,
        65_536,
    )?;
    let project_id = ProjectId::from_string("viewer-scale");
    client.create_project("viewer scale", Some(project_id.clone()))?;
    for run_id in &run_ids {
        client.create_run(&project_id, run_id.as_str(), Some(run_id.clone()))?;
    }
    client.shutdown(None)?;
    drop(client);

    let connection = ProjectConnection::new(open_native_connection_with_config(
        NativeStorageConfig::with_backend_and_s3_config(
            backend,
            &root,
            Some(catalog_path),
            Some(data_path),
            None,
        ),
    )?);
    for (run_index, run_id) in run_ids.iter().enumerate() {
        for (metric_index, metric_key) in metrics.iter().enumerate() {
            connection.execute(
                "INSERT INTO dl.metric_points
                     (run_id, metric_key, metric_key_encoded, step, timestamp, value_f64, ingested_at)
                 SELECT ?, ?, ?, step,
                        epoch_ms(1700000000000 + step),
                        CASE WHEN step = 500000 THEN 1000 + ?
                             ELSE ((step % 1000) + ?)::DOUBLE / 1000 END,
                        epoch_ms(1700000000000 + step)
                 FROM range(?) AS points(step)",
                (
                    run_id.as_str(),
                    *metric_key,
                    *metric_key,
                    (run_index + metric_index) as i64,
                    (run_index + metric_index) as i64,
                    source_points,
                ),
            )?;
            connection.execute(
                "INSERT INTO dl.metric_points
                     (run_id, metric_key, metric_key_encoded, step, timestamp, value_f64, ingested_at)
                 VALUES (?, ?, ?, 250000, epoch_ms(?), 42.0, epoch_ms(?))",
                (
                    run_id.as_str(),
                    *metric_key,
                    *metric_key,
                    FIXTURE_EPOCH_MILLIS + 250_000,
                    FIXTURE_EPOCH_MILLIS + source_points + 1,
                ),
            )?;
        }
        connection.rebuild_metric_aggregates_for_run(run_id)?;
        connection.execute(
            "UPDATE seex_runs SET status = 'finished',
                    started_at = epoch_ms(?), finished_at = now() WHERE run_id = ?",
            (FIXTURE_EPOCH_MILLIS, run_id.as_str()),
        )?;
    }
    connection.flush_metric_points()?;
    drop(connection);
    fs::write(
        root.join(FIXTURE_MANIFEST),
        fixture_manifest(backend, metrics, source_points),
    )?;
    validate_fixture(&root, backend, metrics, source_points)?;
    Ok((root, run_ids))
}

fn measure(
    worker: &ReadWorker,
    events: &ReadEventReceiver,
    source_id: &DataSourceId,
    generation: &mut u64,
    label: &str,
    request: ReadRequest,
) -> Result<ReadSnapshot, Box<dyn Error>> {
    let mut samples = Vec::with_capacity(8);
    let mut first = None;
    for _ in 0..8 {
        *generation += 1;
        let started = Instant::now();
        worker.submit(source_id.clone(), Generation(*generation), request.clone())?;
        let event = receive_event(events, QUERY_TIMEOUT)?;
        assert_eq!(&event.source_id, source_id);
        samples.push(started.elapsed());
        let snapshot = event.result?;
        first.get_or_insert(snapshot);
    }
    let mut warm = samples[1..].to_vec();
    let raw_samples = warm.iter().map(Duration::as_nanos).collect::<Vec<_>>();
    warm.sort_unstable();
    let median = warm[warm.len() / 2];
    let mut deviations = warm
        .iter()
        .map(|sample| sample.abs_diff(median))
        .collect::<Vec<_>>();
    deviations.sort_unstable();
    let relative_mad = deviations[deviations.len() / 2].as_secs_f64() / median.as_secs_f64();
    println!(
        "{label}: cold={:.3} ms, warm min/median/max={:.3}/{:.3}/{:.3} ms",
        samples[0].as_secs_f64() * 1_000.,
        warm[0].as_secs_f64() * 1_000.,
        warm[warm.len() / 2].as_secs_f64() * 1_000.,
        warm[warm.len() - 1].as_secs_f64() * 1_000.,
    );
    println!(
        "SEEX_PERF {{\"schema_version\":1,\"metric\":\"{label} query\",\"unit\":\"ns/op\",\"batch_iterations\":1,\"samples\":7,\"raw_samples\":{raw_samples:?},\"p50\":{},\"p95\":{},\"max_batch\":{},\"max_single\":{},\"relative_mad\":{relative_mad:.6},\"reliable\":{}}}",
        median.as_nanos(),
        warm[6].as_nanos(),
        warm[6].as_nanos(),
        warm[6].as_nanos(),
        relative_mad <= 0.02,
    );
    first.ok_or_else(|| "measurement produced no snapshot".into())
}

fn assert_snapshot(
    label: &str,
    snapshot: &CurveSnapshot,
    budget: u32,
    max_total: usize,
    source_rows: u64,
    value_offset: usize,
) {
    assert_eq!(snapshot.point_budget, budget);
    assert_eq!(snapshot.series.len(), RUNS);
    assert!(
        snapshot
            .series
            .iter()
            .map(|curve| curve.returned_point_count)
            .sum::<u64>()
            <= max_total as u64
    );
    for (run_index, curve) in snapshot.series.iter().enumerate() {
        assert_eq!(curve.source_row_count, source_rows);
        let chart = curve.chart_series.as_ref().expect("series should draw");
        assert_eq!(chart.points().len() as u64, curve.returned_point_count);
        if let Some(point) = chart.points().iter().find(|point| {
            point.y
                != ((point.x as i64 % 1_000) + run_index as i64 + value_offset as i64) as f64
                    / 1_000.
        }) {
            let expected =
                ((point.x as i64 % 1_000) + run_index as i64 + value_offset as i64) as f64 / 1_000.;
            panic!(
                "{label} {} point x={} has y={}, expected {expected}",
                curve.run_ref.run_id.as_str(),
                point.x,
                point.y,
            );
        }
    }
    #[cfg(feature = "test-support")]
    {
        let resources = snapshot.resource_snapshot();
        for (metric, value) in [
            ("requested budget", resources.requested_budget),
            ("source points", resources.source_points),
            ("returned points", resources.returned_points),
            ("snapshot points", resources.snapshot_points),
            ("snapshot bytes", resources.snapshot_bytes),
        ] {
            println!(
                "SEEX_PERF {{\"schema_version\":1,\"metric\":\"{label} {metric}\",\"unit\":\"count\",\"batch_iterations\":1,\"samples\":1,\"raw_samples\":[{value}],\"p50\":{value},\"p95\":{value},\"max_batch\":{value},\"max_single\":{value},\"relative_mad\":0.0,\"reliable\":true}}"
            );
        }
    }
}

fn validate_backend(backend: CatalogBackend) -> Result<(), Box<dyn Error>> {
    let (root, run_ids) = read_fixture(
        backend,
        "SEEX_APP_SCALE_FIXTURE_ROOT",
        &["loss"],
        SOURCE_POINTS,
    )?;
    let mut worker = ReadWorker::spawn(&root)?;
    let events = worker
        .take_event_receiver()
        .ok_or("worker event receiver should be available")?;
    let source_id = DataSourceId::from_path(&root);
    let project_id = ProjectId::from_string("viewer-scale");
    let backend_name = if backend == CatalogBackend::DuckDb {
        "duckdb"
    } else {
        "sqlite"
    };
    let selection = CurveSelection {
        source_id: source_id.clone(),
        runs: run_ids
            .into_iter()
            .map(|run_id| RunRef::new(source_id.clone(), project_id.clone(), run_id))
            .collect(),
        metric_key: MetricKey::from_string("loss"),
        axis: CurveAxis::Step,
    };
    let mut generation = 0;
    let overview = measure(
        &worker,
        &events,
        &source_id,
        &mut generation,
        &format!("{backend_name} overview"),
        ReadRequest::Overview(OverviewRequest {
            selection: selection.clone(),
            logical_width: 2_000,
        }),
    )?;
    let ReadSnapshot::Overview(overview) = overview else {
        return Err("expected overview snapshot".into());
    };
    assert_snapshot(
        &format!("{backend_name} overview"),
        &overview,
        2_000,
        20_020,
        SOURCE_POINTS as u64,
        0,
    );

    let full_viewport = AlignmentViewport::new(0, SOURCE_POINTS - 1)?;
    let full = measure(
        &worker,
        &events,
        &source_id,
        &mut generation,
        &format!("{backend_name} full detail"),
        ReadRequest::Detail(DetailRequest {
            selection: selection.clone(),
            viewport: full_viewport,
            logical_width: 2_500,
        }),
    )?;
    let ReadSnapshot::Detail(full) = full else {
        return Err("expected full detail snapshot".into());
    };
    assert_eq!(full.viewport, full_viewport);
    assert_snapshot(
        &format!("{backend_name} full detail"),
        &full,
        5_000,
        50_020,
        SOURCE_POINTS as u64,
        0,
    );

    let narrow_viewport = AlignmentViewport::new(450_000, 550_000)?;
    let narrow = measure(
        &worker,
        &events,
        &source_id,
        &mut generation,
        &format!("{backend_name} narrow detail"),
        ReadRequest::Detail(DetailRequest {
            selection,
            viewport: narrow_viewport,
            logical_width: 2_500,
        }),
    )?;
    let ReadSnapshot::Detail(narrow) = narrow else {
        return Err("expected narrow detail snapshot".into());
    };
    assert_eq!(narrow.viewport, narrow_viewport);
    assert_snapshot(
        &format!("{backend_name} narrow detail"),
        &narrow,
        5_000,
        50_020,
        100_003,
        0,
    );
    assert!(narrow.series[0].source_row_count < full.series[0].source_row_count);
    Ok(())
}

#[derive(Clone, Copy)]
enum BaselineAxis {
    Step,
    RelativeTime,
    Timestamp,
}

impl BaselineAxis {
    const fn label(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::RelativeTime => "relative_time",
            Self::Timestamp => "timestamp",
        }
    }
}

fn query_reader_equivalent(
    connection: &ProjectConnection,
    run_ids: &[RunId],
    axis: BaselineAxis,
    start: i64,
    end: i64,
    max_points: usize,
) -> Result<usize, Box<dyn Error>> {
    if start >= end {
        return Err("reader-equivalent range must be non-empty and half-open".into());
    }
    let reader = ProjectMetricReader::new(connection);
    let mut returned = 0;
    for run_id in run_ids {
        let count = match axis {
            BaselineAxis::Step => reader
                .query_metric(&MetricQuery::new(
                    run_id.clone(),
                    MetricKey::from_string("loss"),
                    Some(Step::new(start)),
                    Some(Step::new(end)),
                    ReductionPolicy::screen_budget(max_points as u32, 1)?,
                )?)?
                .points
                .len(),
            BaselineAxis::RelativeTime | BaselineAxis::Timestamp => {
                let offset = if matches!(axis, BaselineAxis::Timestamp) {
                    FIXTURE_EPOCH_MILLIS
                } else {
                    0
                };
                let result = reader.query_aligned_metric(&AlignmentQuery {
                    run_id: run_id.clone(),
                    metric_key: MetricKey::from_string("loss"),
                    axis: AlignmentAxis::ElapsedTime,
                    viewport: AlignmentViewport::new(start - offset, end - offset - 1)?,
                    reduction: AlignmentReduction::screen_budget(max_points as u32, 1)?,
                })?;
                result
                    .points
                    .iter()
                    .filter(|point| {
                        let value = point.axis_value + offset;
                        start <= value && value < end
                    })
                    .take(max_points)
                    .count()
            }
        };
        if count > max_points {
            return Err(format!("{} query exceeded max_points", axis.label()).into());
        }
        returned += count;
    }
    Ok(returned)
}

fn measure_reader_axis(
    backend: &str,
    connection: &ProjectConnection,
    run_ids: &[RunId],
    axis: BaselineAxis,
    range: &str,
    start: i64,
    end: i64,
) -> Result<(), Box<dyn Error>> {
    let mut samples = Vec::with_capacity(7);
    for _ in 0..7 {
        let started = Instant::now();
        let returned = query_reader_equivalent(connection, run_ids, axis, start, end, 5_000)?;
        if returned == 0 || returned > RUNS * 5_000 {
            return Err("reader-equivalent query returned an invalid point count".into());
        }
        samples.push(started.elapsed().as_nanos() as f64);
    }
    let mut ordered = samples.clone();
    ordered.sort_by(f64::total_cmp);
    let p50 = ordered[3];
    let mut deviations = ordered
        .iter()
        .map(|sample| (sample - p50).abs())
        .collect::<Vec<_>>();
    deviations.sort_by(f64::total_cmp);
    let mad = deviations[3];
    let relative_mad = mad / p50;
    println!(
        "SEEX_PERF {{\"schema_version\":2,\"record_type\":\"metric\",\
         \"domain\":\"query\",\"metric\":\"{backend}.reader.{}.{range}\",\
         \"unit\":\"ns/op\",\"direction\":\"lower\",\"batch_iterations\":1,\
         \"samples\":7,\"raw_samples\":{samples:?},\"mad\":{mad},\
         \"relative_mad\":{relative_mad},\"p50\":{p50},\"p95\":{},\
         \"max\":{},\"reliable\":{}}}",
        axis.label(),
        ordered[6],
        ordered[6],
        relative_mad <= 0.02,
    );
    Ok(())
}

fn parquet_reader<'connection>(
    connection: &'connection ProjectConnection,
    root: &Path,
) -> Result<ParquetMetricReader<'connection>, Box<dyn Error>> {
    let source = root
        .join("custom/data/main/metric_points/**/*.parquet")
        .to_string_lossy()
        .into_owned();
    Ok(ParquetMetricReader::open(
        connection,
        ParquetSource::new(source)?,
    )?)
}

fn semantic_points(
    reader: &impl seex_storage::MetricReader,
    run_id: &RunId,
) -> Result<Vec<(i64, f64)>, Box<dyn Error>> {
    let result = reader.query_metric(&MetricQuery::new(
        run_id.clone(),
        MetricKey::from_string("loss"),
        Some(Step::new(249_999)),
        Some(Step::new(500_002)),
        ReductionPolicy::Full,
    )?)?;
    Ok(result
        .points
        .into_iter()
        .filter(|point| matches!(point.step.value(), 249_999 | 250_000 | 500_000 | 500_001))
        .map(|point| (point.step.value(), point.value_f64))
        .collect())
}

#[test]
#[ignore = "creates retained DuckDB and SQLite fixtures for release query validation"]
fn prepare_large_native_series_fixture() -> Result<(), Box<dyn Error>> {
    assert!(
        std::hint::black_box(!cfg!(debug_assertions)),
        "fixture preparation requires --release"
    );
    prepare_fixture(
        CatalogBackend::DuckDb,
        "SEEX_APP_SCALE_FIXTURE_ROOT",
        &["loss"],
        SOURCE_POINTS,
    )?;
    prepare_fixture(
        CatalogBackend::Sqlite,
        "SEEX_APP_SCALE_FIXTURE_ROOT",
        &["loss"],
        SOURCE_POINTS,
    )?;
    Ok(())
}

#[test]
#[ignore = "creates and queries twenty million source points; run explicitly in release mode"]
fn large_native_series_respect_viewer_query_budgets() -> Result<(), Box<dyn Error>> {
    assert!(
        std::hint::black_box(!cfg!(debug_assertions)),
        "scale validation requires --release"
    );
    validate_backend(CatalogBackend::DuckDb)?;
    validate_backend(CatalogBackend::Sqlite)
}

#[test]
#[ignore = "queries the immutable scale fixture on all Reader destination axes"]
fn reader_equivalent_axes_and_ranges() -> Result<(), Box<dyn Error>> {
    assert!(
        std::hint::black_box(!cfg!(debug_assertions)),
        "Reader performance validation requires --release"
    );
    for backend in [CatalogBackend::DuckDb, CatalogBackend::Sqlite] {
        let (root, run_ids) = read_fixture(
            backend,
            "SEEX_APP_SCALE_FIXTURE_ROOT",
            &["loss"],
            SOURCE_POINTS,
        )?;
        let connection = open_fixture_connection(&root)?;
        for (axis, offset) in [
            (BaselineAxis::Step, 0),
            (BaselineAxis::RelativeTime, 0),
            (BaselineAxis::Timestamp, FIXTURE_EPOCH_MILLIS),
        ] {
            measure_reader_axis(
                backend_name(backend),
                &connection,
                &run_ids,
                axis,
                "full",
                offset,
                offset + SOURCE_POINTS,
            )?;
            measure_reader_axis(
                backend_name(backend),
                &connection,
                &run_ids,
                axis,
                "narrow",
                offset + 450_000,
                offset + 550_000,
            )?;
        }
    }
    Ok(())
}

#[test]
#[ignore = "validates adversarial semantics on the immutable scale fixture"]
fn scale_fixture_preserves_reader_semantics() -> Result<(), Box<dyn Error>> {
    let mut native_results = Vec::new();
    for backend in [CatalogBackend::DuckDb, CatalogBackend::Sqlite] {
        let (root, run_ids) = read_fixture(
            backend,
            "SEEX_APP_SCALE_FIXTURE_ROOT",
            &["loss"],
            SOURCE_POINTS,
        )?;
        let connection = open_fixture_connection(&root)?;
        let native = ProjectMetricReader::new(&connection);
        let expected = semantic_points(&native, &run_ids[0])?;
        assert_eq!(expected[1], (250_000, 42.0));
        assert_eq!(expected[2], (500_000, 1000.0));
        let neighbors = native.query_aligned_metric(&AlignmentQuery {
            run_id: run_ids[0].clone(),
            metric_key: MetricKey::from_string("loss"),
            axis: AlignmentAxis::Step,
            viewport: AlignmentViewport::new(250_000, 250_000)?,
            reduction: AlignmentReduction::Full,
        })?;
        assert_eq!(
            neighbors
                .points
                .iter()
                .map(|point| point.axis_value)
                .collect::<Vec<_>>(),
            [249_999, 250_000, 250_001]
        );
        assert!(neighbors.reasons.is_empty());

        let parquet = parquet_reader(&connection, &root)?;
        assert_eq!(semantic_points(&parquet, &run_ids[0])?, expected);
        let missing_start = parquet.query_aligned_metric(&AlignmentQuery {
            run_id: run_ids[0].clone(),
            metric_key: MetricKey::from_string("loss"),
            axis: AlignmentAxis::ElapsedTime,
            viewport: AlignmentViewport::new(0, 1)?,
            reduction: AlignmentReduction::Full,
        })?;
        assert_eq!(missing_start.reasons, [AlignmentReason::MissingRunStart]);
        native_results.push(expected);
    }
    let passed = native_results[0] == native_results[1];
    println!(
        "SEEX_PERF {{\"schema_version\":2,\"record_type\":\"check\",\
         \"domain\":\"query\",\"check\":\"reader_parity_and_semantics\",\
         \"passed\":{passed},\"detail\":\"lww,spikes,neighbors,standalone\"}}"
    );
    assert!(passed);
    Ok(())
}

#[test]
#[ignore = "creates a retained six-metric native fixture for manual product tracing"]
fn retained_multi_track_fixture_supports_product_tracing() -> Result<(), Box<dyn Error>> {
    assert!(
        std::hint::black_box(!cfg!(debug_assertions)),
        "trace fixture generation requires --release"
    );
    let (root, run_ids) = prepare_fixture(
        CatalogBackend::DuckDb,
        "SEEX_APP_TRACE_FIXTURE_ROOT",
        &TRACE_METRICS,
        SOURCE_POINTS,
    )?;
    let mut worker = ReadWorker::spawn(&root)?;
    let events = worker
        .take_event_receiver()
        .ok_or("worker event receiver should be available")?;
    let source_id = DataSourceId::from_path(&root);
    let project_id = ProjectId::from_string("viewer-scale");
    for (index, metric_key) in TRACE_METRICS.into_iter().enumerate() {
        let selection = CurveSelection {
            source_id: source_id.clone(),
            runs: run_ids
                .iter()
                .cloned()
                .map(|run_id| RunRef::new(source_id.clone(), project_id.clone(), run_id))
                .collect(),
            metric_key: MetricKey::from_string(metric_key),
            axis: CurveAxis::Step,
        };
        worker.submit(
            source_id.clone(),
            Generation(index as u64 + 1),
            ReadRequest::Detail(DetailRequest {
                selection,
                viewport: AlignmentViewport::new(0, SOURCE_POINTS - 1)?,
                logical_width: 2_500,
            }),
        )?;
        let event = receive_event(&events, QUERY_TIMEOUT)?;
        let ReadSnapshot::Detail(snapshot) = event.result? else {
            return Err("expected detail snapshot".into());
        };
        assert_snapshot(
            metric_key,
            &snapshot,
            5_000,
            50_020,
            SOURCE_POINTS as u64,
            index,
        );
    }
    println!("retained trace fixture: {}", root.display());
    Ok(())
}
