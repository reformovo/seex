use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use seex_app::data::query::{
    CurveAxis, CurveSelection, CurveSnapshot, DetailRequest, OverviewRequest,
};
use seex_app::data::worker::{
    Generation, ReadEventReceiver, ReadRequest, ReadSnapshot, ReadWorker,
};
use seex_app::domain::{DataSourceId, RunRef};
use seex_core::engine::client::NativeClient;
use seex_model::alignment::AlignmentViewport;
use seex_model::metric::MetricKey;
use seex_model::run::RunId;
use seex_model::types::ProjectId;
use seex_storage::ProjectConnection;
use seex_storage::bootstrap::{
    CatalogBackend, NativeStorageConfig, open_native_connection_with_config,
};

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
                        ((step % 1000) + ?)::DOUBLE / 1000,
                        epoch_ms(1700000000000 + step)
                 FROM range(?) AS points(step)",
                (
                    run_id.as_str(),
                    *metric_key,
                    *metric_key,
                    (run_index + metric_index) as i64,
                    source_points,
                ),
            )?;
        }
        connection.rebuild_metric_aggregates_for_run(run_id)?;
        connection.execute(
            "UPDATE seex_runs SET status = 'finished', finished_at = now() WHERE run_id = ?",
            [run_id.as_str()],
        )?;
    }
    connection.flush_metric_points()?;
    drop(connection);
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
    let (root, run_ids) = read_fixture(backend, "SEEX_APP_SCALE_FIXTURE_ROOT")?;
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
