use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pulseon_core::engine::client::NativeClient;
use pulseon_model::alignment::AlignmentViewport;
use pulseon_model::metric::MetricKey;
use pulseon_model::run::RunId;
use pulseon_model::types::ProjectId;
use pulseon_storage::ProjectConnection;
use pulseon_storage::bootstrap::{
    CatalogBackend, NativeStorageConfig, open_native_connection_with_config,
};
use pulseon_viewer::data::query::{
    CurveAxis, CurveSelection, CurveSnapshot, DetailRequest, OverviewRequest,
};
use pulseon_viewer::data::worker::{
    Generation, ReadEventReceiver, ReadRequest, ReadSnapshot, ReadWorker,
};
use pulseon_viewer::domain::{DataSourceId, RunRef};

mod support;

use support::receive_event;

const RUNS: usize = 10;
const SOURCE_POINTS: i64 = 1_000_000;
const TRACE_SOURCE_POINTS: i64 = 100_000;
const TRACE_METRICS: [&str; 6] = [
    "loss",
    "accuracy",
    "latency",
    "memory",
    "throughput",
    "error",
];
const QUERY_TIMEOUT: Duration = Duration::from_secs(300);

fn fixture_root(backend: CatalogBackend, root_variable: &str) -> Result<PathBuf, Box<dyn Error>> {
    let name = match backend {
        CatalogBackend::DuckDb => "duckdb",
        CatalogBackend::Sqlite => "sqlite",
    };
    let base = std::env::var_os(root_variable)
        .ok_or_else(|| format!("{root_variable} must name a retained fixture directory"))?;
    let path = PathBuf::from(base).join(name);
    if path.exists() && fs::read_dir(&path)?.next().is_some() {
        return Err(format!("fixture directory is not empty: {}", path.display()).into());
    }
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn build_fixture(
    backend: CatalogBackend,
    root_variable: &str,
    metrics: &[&str],
    source_points: i64,
) -> Result<(PathBuf, Vec<RunId>), Box<dyn Error>> {
    let root = fixture_root(backend, root_variable)?;
    let pulseon_dir = root.join(".pulseon");
    fs::create_dir_all(&pulseon_dir)?;
    let catalog_name = match backend {
        CatalogBackend::DuckDb => "catalog.ducklake",
        CatalogBackend::Sqlite => "catalog.sqlite",
    };
    fs::write(
        pulseon_dir.join("config.toml"),
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
    let run_ids = (0..RUNS)
        .map(|index| RunId::from_string(format!("run-{index}")))
        .collect::<Vec<_>>();
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
            "UPDATE pulseon_runs SET status = 'finished', finished_at = now() WHERE run_id = ?",
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
    let mut samples = Vec::with_capacity(6);
    let mut first = None;
    for _ in 0..6 {
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
    warm.sort_unstable();
    println!(
        "{label}: cold={:.3} ms, warm min/median/max={:.3}/{:.3}/{:.3} ms",
        samples[0].as_secs_f64() * 1_000.,
        warm[0].as_secs_f64() * 1_000.,
        warm[warm.len() / 2].as_secs_f64() * 1_000.,
        warm[warm.len() - 1].as_secs_f64() * 1_000.,
    );
    first.ok_or_else(|| "measurement produced no snapshot".into())
}

fn assert_snapshot(snapshot: &CurveSnapshot, budget: u32, max_total: usize, source_rows: u64) {
    assert_eq!(snapshot.point_budget, budget);
    assert_eq!(snapshot.series.len(), RUNS);
    assert!(
        snapshot
            .series
            .iter()
            .map(|curve| curve.evidence.points.len())
            .sum::<usize>()
            <= max_total
    );
    for curve in &snapshot.series {
        assert_eq!(curve.evidence.source_row_count, source_rows);
        let chart = curve.chart_series.as_ref().expect("series should draw");
        assert_eq!(chart.points().len(), curve.evidence.points.len());
        assert!(
            chart
                .points()
                .iter()
                .zip(&curve.evidence.points)
                .all(|(chart, evidence)| chart.x == evidence.axis_value as f64
                    && chart.y == evidence.point.value_f64)
        );
    }
}

fn validate_backend(backend: CatalogBackend) -> Result<(), Box<dyn Error>> {
    let (root, run_ids) = build_fixture(
        backend,
        "PULSEON_VIEWER_SCALE_FIXTURE_ROOT",
        &["loss"],
        SOURCE_POINTS,
    )?;
    let mut worker = ReadWorker::spawn(&root)?;
    let events = worker
        .take_event_receiver()
        .ok_or("worker event receiver should be available")?;
    let source_id = DataSourceId::from_path(&root);
    let project_id = ProjectId::from_string("viewer-scale");
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
        "overview",
        ReadRequest::Overview(OverviewRequest {
            selection: selection.clone(),
            physical_width: 2_000,
        }),
    )?;
    let ReadSnapshot::Overview(overview) = overview else {
        return Err("expected overview snapshot".into());
    };
    assert_snapshot(&overview, 2_000, 20_020, SOURCE_POINTS as u64);

    let full_viewport = AlignmentViewport::new(0, SOURCE_POINTS - 1)?;
    let full = measure(
        &worker,
        &events,
        &source_id,
        &mut generation,
        "full detail",
        ReadRequest::Detail(DetailRequest {
            selection: selection.clone(),
            viewport: full_viewport,
            physical_width: 5_000,
        }),
    )?;
    let ReadSnapshot::Detail(full) = full else {
        return Err("expected full detail snapshot".into());
    };
    assert_eq!(full.viewport, full_viewport);
    assert_snapshot(&full, 10_000, 100_020, SOURCE_POINTS as u64);

    let narrow_viewport = AlignmentViewport::new(450_000, 550_000)?;
    let narrow = measure(
        &worker,
        &events,
        &source_id,
        &mut generation,
        "narrow detail",
        ReadRequest::Detail(DetailRequest {
            selection,
            viewport: narrow_viewport,
            physical_width: 5_000,
        }),
    )?;
    let ReadSnapshot::Detail(narrow) = narrow else {
        return Err("expected narrow detail snapshot".into());
    };
    assert_eq!(narrow.viewport, narrow_viewport);
    assert_snapshot(&narrow, 10_000, 100_020, 100_003);
    assert!(narrow.series[0].evidence.source_row_count < full.series[0].evidence.source_row_count);
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
    let (root, run_ids) = build_fixture(
        CatalogBackend::DuckDb,
        "PULSEON_VIEWER_TRACE_FIXTURE_ROOT",
        &TRACE_METRICS,
        TRACE_SOURCE_POINTS,
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
                viewport: AlignmentViewport::new(0, TRACE_SOURCE_POINTS - 1)?,
                physical_width: 5_000,
            }),
        )?;
        let event = receive_event(&events, QUERY_TIMEOUT)?;
        let ReadSnapshot::Detail(snapshot) = event.result? else {
            return Err("expected detail snapshot".into());
        };
        assert_snapshot(&snapshot, 10_000, 100_020, TRACE_SOURCE_POINTS as u64);
    }
    println!("retained trace fixture: {}", root.display());
    Ok(())
}
