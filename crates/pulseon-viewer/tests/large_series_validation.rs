use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pulseon_core::engine::client::NativeClient;
use pulseon_model::alignment::{AlignmentAxis, AlignmentViewport};
use pulseon_model::metric::MetricKey;
use pulseon_model::run::RunId;
use pulseon_model::types::ProjectId;
use pulseon_storage::ProjectConnection;
use pulseon_storage::bootstrap::{
    CatalogBackend, NativeStorageConfig, open_native_connection_with_config,
};
use pulseon_viewer::core::{DataSourceId, RunRef};
use pulseon_viewer::query::{CurveSelection, CurveSnapshot, DetailRequest, OverviewRequest};
use pulseon_viewer::worker::{Generation, ReadRequest, ReadSnapshot, ReadWorker};

const RUNS: usize = 10;
const SOURCE_POINTS: i64 = 1_000_000;
const QUERY_TIMEOUT: Duration = Duration::from_secs(300);

fn fixture_root(backend: CatalogBackend) -> Result<PathBuf, Box<dyn Error>> {
    let name = match backend {
        CatalogBackend::DuckDb => "duckdb",
        CatalogBackend::Sqlite => "sqlite",
    };
    let base = std::env::var_os("PULSEON_VIEWER_SCALE_FIXTURE_ROOT")
        .ok_or("PULSEON_VIEWER_SCALE_FIXTURE_ROOT must name a retained fixture directory")?;
    let path = PathBuf::from(base).join(name);
    if path.exists() && fs::read_dir(&path)?.next().is_some() {
        return Err(format!("fixture directory is not empty: {}", path.display()).into());
    }
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn build_fixture(backend: CatalogBackend) -> Result<(PathBuf, Vec<RunId>), Box<dyn Error>> {
    let root = fixture_root(backend)?;
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
        connection.execute(
            "INSERT INTO dl.metric_points
                 (run_id, metric_key, metric_key_encoded, step, timestamp, value_f64, ingested_at)
             SELECT ?, 'loss', 'loss', step,
                    epoch_ms(1700000000000 + step),
                    ((step % 1000) + ?)::DOUBLE / 1000,
                    epoch_ms(1700000000000 + step)
             FROM range(?) AS points(step)",
            (run_id.as_str(), run_index as i64, SOURCE_POINTS),
        )?;
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
        let event = worker.recv_timeout(QUERY_TIMEOUT)?;
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
    let (root, run_ids) = build_fixture(backend)?;
    let worker = ReadWorker::spawn(&root)?;
    let source_id = DataSourceId::from_path(&root);
    let project_id = ProjectId::from_string("viewer-scale");
    let selection = CurveSelection {
        source_id: source_id.clone(),
        runs: run_ids
            .into_iter()
            .map(|run_id| RunRef::new(source_id.clone(), project_id.clone(), run_id))
            .collect(),
        metric_key: MetricKey::from_string("loss"),
        axis: AlignmentAxis::Step,
    };
    let mut generation = 0;
    let overview = measure(
        &worker,
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
