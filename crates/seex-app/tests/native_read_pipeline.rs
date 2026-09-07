use std::error::Error;
use std::fs;
use std::path::Path;
use std::time::Duration;

use seex::AlignmentViewport;
use seex::CatalogBackend;
use seex::EvidenceCompleteness;
use seex::MetricKey;
use seex::ProjectId;
use seex::RunId;
use seex::{Client, LogOptions, RunOptions};
use seex_app::SourceError;
use seex_app::config::ConfiguredSource;
use seex_app::data::DiscoveryRequest;
use seex_app::data::query::{CurveAxis, CurveSelection, DetailRequest, OverviewRequest};
use seex_app::data::registry::{SourceRegistry, SourceStatus};
use seex_app::data::worker::{
    Generation, ReadEventReceiver, ReadRequest, ReadSnapshot, ReadWorker, WorkerError,
};
use seex_app::domain::{DataSourceId, RunRef, SourceAlias};

mod support;

use support::receive_event;

const SOURCE_POINTS: i64 = 2_100;
const EVENT_TIMEOUT: Duration = Duration::from_secs(20);

struct Fixture {
    root: tempfile::TempDir,
    project_id: ProjectId,
    complete_run_id: RunId,
    running_run_id: RunId,
    invalid_run_id: RunId,
    unavailable_run_id: RunId,
}

impl Fixture {
    fn root_path(&self) -> &Path {
        self.root.path()
    }

    fn selection(&self) -> CurveSelection {
        let source_id = self.source_id();
        CurveSelection {
            source_id: source_id.clone(),
            runs: [
                &self.complete_run_id,
                &self.running_run_id,
                &self.invalid_run_id,
                &self.unavailable_run_id,
            ]
            .into_iter()
            .map(|run_id| RunRef::new(source_id.clone(), self.project_id.clone(), run_id.clone()))
            .collect(),
            metric_key: MetricKey::from_string("loss"),
            axis: CurveAxis::Step,
        }
    }

    fn source_id(&self) -> DataSourceId {
        DataSourceId::new("source").expect("test alias should be valid")
    }

    fn run_ref(&self, run_id: &RunId) -> RunRef {
        RunRef::new(self.source_id(), self.project_id.clone(), run_id.clone())
    }
}

fn fixture(backend: CatalogBackend, absolute_paths: bool) -> Result<Fixture, Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let config_dir = root.path().join(".seex");
    fs::create_dir(&config_dir)?;
    let backend_name = match backend {
        CatalogBackend::DuckDb => "duckdb",
        CatalogBackend::Sqlite => "sqlite",
    };
    let catalog_name = match backend {
        CatalogBackend::DuckDb => "catalog.ducklake",
        CatalogBackend::Sqlite => "catalog.sqlite",
    };
    let catalog_relative = Path::new("custom").join(catalog_name);
    let data_relative = Path::new("custom").join("data");
    let catalog_path = root.path().join(&catalog_relative);
    let data_path = root.path().join(&data_relative);
    let configured_catalog = if absolute_paths {
        catalog_path.to_string_lossy().into_owned()
    } else {
        catalog_relative.to_string_lossy().into_owned()
    };
    let configured_data = if absolute_paths {
        data_path.to_string_lossy().into_owned()
    } else {
        data_relative.to_string_lossy().into_owned()
    };
    fs::write(
        config_dir.join("config.toml"),
        format!(
            "schema_version = 1\ncatalog_backend = \"{backend_name}\"\n\
             catalog_path = \"{configured_catalog}\"\n\
             data_path = \"{configured_data}\"\n"
        ),
    )?;

    let client = Client::builder(root.path())
        .catalog_backend(backend)
        .catalog_path(catalog_path)
        .data_path(data_path)
        .metric_queue_capacity(65_536)
        .open()?;
    let project_id = ProjectId::from_string("project-1");
    let complete_run_id = RunId::from_string("complete");
    let complete_handle = client.start_run(
        RunOptions::new(project_id.as_str())
            .id(complete_run_id.as_str())
            .name("complete"),
    )?;
    for step in 0..SOURCE_POINTS {
        complete_handle.log_with([("loss", step as f64)], LogOptions::new().step(step))?;
    }
    complete_handle.finish()?;

    let invalid_run_id = RunId::from_string("invalid");
    let invalid = client.start_run(
        RunOptions::new(project_id.as_str())
            .id(invalid_run_id.as_str())
            .name("invalid"),
    )?;
    invalid.log_with([("loss", 0.25)], LogOptions::new().step(-1))?;
    invalid.fail()?;

    let unavailable_run_id = RunId::from_string("unavailable");
    let unavailable = client.start_run(
        RunOptions::new(project_id.as_str())
            .id(unavailable_run_id.as_str())
            .name("unavailable"),
    )?;
    unavailable.finish()?;

    let running_run_id = RunId::from_string("running");
    let running = client.start_run(
        RunOptions::new(project_id.as_str())
            .id(running_run_id.as_str())
            .name("running"),
    )?;
    running.log_with([("loss", 0.5)], LogOptions::new().step(0))?;
    client.shutdown()?;
    Ok(Fixture {
        root,
        project_id,
        complete_run_id,
        running_run_id,
        invalid_run_id,
        unavailable_run_id,
    })
}

fn read(
    worker: &ReadWorker,
    events: &ReadEventReceiver,
    source_id: DataSourceId,
    generation: u64,
    request: ReadRequest,
) -> Result<ReadSnapshot, Box<dyn Error>> {
    let expected_kind = request.kind();
    worker.submit(source_id.clone(), Generation(generation), request)?;
    let event = receive_event(events, EVENT_TIMEOUT)?;
    assert_eq!(event.source_id, source_id);
    assert_eq!(event.generation, Generation(generation));
    assert_eq!(event.kind, expected_kind);
    Ok(event.result?)
}

fn assert_backend_contract(fixture: &Fixture) -> Result<(), Box<dyn Error>> {
    let mut worker = ReadWorker::spawn(fixture.root_path())?;
    let events = worker
        .take_event_receiver()
        .ok_or("worker event receiver should be available")?;
    let source_id = fixture.source_id();
    let selection = fixture.selection();
    let discovery_request = DiscoveryRequest {
        project_allowlist: None,
        project_id: Some(fixture.project_id.clone()),
        selected_run_ids: selection
            .runs
            .iter()
            .map(|run| run.run_id.clone())
            .collect(),
        metric_runs: Vec::new(),
    };
    let catalog = match read(
        &worker,
        &events,
        source_id.clone(),
        1,
        ReadRequest::Discover(discovery_request.clone()),
    )? {
        ReadSnapshot::Catalog(snapshot) => snapshot,
        other => return Err(format!("unexpected discovery snapshot: {other:?}").into()),
    };
    assert_eq!(catalog.projects.len(), 1);
    assert_eq!(catalog.runs.len(), 4);
    assert_eq!(
        catalog
            .metric_keys
            .iter()
            .map(MetricKey::as_str)
            .collect::<Vec<_>>(),
        ["loss"]
    );

    let overview_request = OverviewRequest {
        selection: fixture.selection(),
        logical_width: 1,
    };
    let overview = match read(
        &worker,
        &events,
        source_id.clone(),
        2,
        ReadRequest::Overview(overview_request),
    )? {
        ReadSnapshot::Overview(snapshot) => snapshot,
        other => return Err(format!("unexpected overview snapshot: {other:?}").into()),
    };
    assert_eq!(overview.point_budget, 128);
    assert_eq!(
        overview
            .real_range
            .map(|range| (range.start(), range.end())),
        Some((0, SOURCE_POINTS - 1))
    );
    assert_eq!(overview.series[0].source_row_count, SOURCE_POINTS as u64);
    assert!(overview.series[0].returned_point_count <= 130);
    assert!(overview.series[0].downsampled());
    assert_eq!(
        overview
            .series
            .iter()
            .map(|series| series.completeness)
            .collect::<Vec<_>>(),
        [
            EvidenceCompleteness::Complete,
            EvidenceCompleteness::Partial,
            EvidenceCompleteness::Invalid,
            EvidenceCompleteness::Unavailable,
        ]
    );
    assert_eq!(
        overview
            .series
            .iter()
            .map(|series| series.chart_series.is_some())
            .collect::<Vec<_>>(),
        [true, true, false, false]
    );

    let detail_selection = CurveSelection {
        source_id: source_id.clone(),
        runs: vec![fixture.run_ref(&fixture.complete_run_id)],
        metric_key: MetricKey::from_string("loss"),
        axis: CurveAxis::Step,
    };
    let full_detail = match read(
        &worker,
        &events,
        source_id.clone(),
        3,
        ReadRequest::Detail(DetailRequest {
            selection: detail_selection.clone(),
            viewport: AlignmentViewport::new(0, SOURCE_POINTS - 1)?,
            logical_width: 1,
        }),
    )? {
        ReadSnapshot::Detail(snapshot) => snapshot,
        other => return Err(format!("unexpected detail snapshot: {other:?}").into()),
    };
    assert_eq!(full_detail.point_budget, 256);
    assert!(full_detail.series[0].returned_point_count <= 258);
    assert!(full_detail.series[0].downsampled());

    let detail_request = DetailRequest {
        selection: detail_selection,
        viewport: AlignmentViewport::new(500, 1_500)?,
        logical_width: 1,
    };
    let detail = match read(
        &worker,
        &events,
        source_id,
        4,
        ReadRequest::Detail(detail_request.clone()),
    )? {
        ReadSnapshot::Detail(snapshot) => snapshot,
        other => return Err(format!("unexpected detail snapshot: {other:?}").into()),
    };
    assert_eq!(detail.point_budget, full_detail.point_budget);
    assert_eq!(detail.series[0].source_row_count, 1_003);
    assert!(detail.series[0].returned_point_count <= 258);
    #[cfg(feature = "test-support")]
    {
        let resources = detail.resource_snapshot();
        assert_eq!(resources.requested_budget, 256);
        assert_eq!(resources.source_points, 1_003);
        assert_eq!(
            resources.returned_points,
            detail.series[0].returned_point_count
        );
        assert_eq!(resources.snapshot_points, resources.returned_points);
        assert_eq!(
            resources.snapshot_bytes,
            resources.snapshot_points * std::mem::size_of::<seex_plot::DataPoint>() as u64
        );
    }
    assert_eq!(
        detail.series[0]
            .chart_series
            .as_ref()
            .expect("complete evidence should draw")
            .points()
            .first()
            .map(|point| point.x as i64),
        Some(499),
    );
    assert_eq!(
        detail.series[0]
            .chart_series
            .as_ref()
            .expect("complete evidence should draw")
            .points()
            .last()
            .map(|point| point.x as i64),
        Some(1_501),
    );

    Ok(())
}

#[test]
fn both_catalog_backends_preserve_query_and_refresh_contracts() -> Result<(), Box<dyn Error>> {
    let duckdb = fixture(CatalogBackend::DuckDb, false)?;
    assert_backend_contract(&duckdb)?;
    let sqlite = fixture(CatalogBackend::Sqlite, true)?;
    assert_backend_contract(&sqlite)?;
    Ok(())
}

#[test]
fn source_registry_reads_duckdb_and_sqlite_together() -> Result<(), Box<dyn Error>> {
    let duckdb = fixture(CatalogBackend::DuckDb, false)?;
    let sqlite = fixture(CatalogBackend::Sqlite, true)?;
    let mut registry = SourceRegistry::default();
    let duckdb_id = registry.configure(ConfiguredSource {
        alias: SourceAlias::new("duckdb").expect("test alias should be valid"),
        root_path: duckdb.root_path().to_path_buf(),
        projects: vec![duckdb.project_id.clone()],
    });
    let sqlite_id = registry.configure(ConfiguredSource {
        alias: SourceAlias::new("sqlite").expect("test alias should be valid"),
        root_path: sqlite.root_path().to_path_buf(),
        projects: vec![sqlite.project_id.clone()],
    });
    let duckdb_events = registry
        .activate(&duckdb_id)?
        .ok_or("DuckDB source should return an event stream")?;
    let sqlite_events = registry
        .activate(&sqlite_id)?
        .ok_or("SQLite source should return an event stream")?;

    registry.submit(
        &duckdb_id,
        Generation(1),
        ReadRequest::Discover(DiscoveryRequest::default()),
    )?;
    registry.submit(
        &sqlite_id,
        Generation(2),
        ReadRequest::Discover(DiscoveryRequest::default()),
    )?;
    let duckdb_event = receive_event(&duckdb_events, EVENT_TIMEOUT)?;
    let sqlite_event = receive_event(&sqlite_events, EVENT_TIMEOUT)?;
    assert!(matches!(
        &duckdb_event.result,
        Ok(ReadSnapshot::Catalog(catalog)) if catalog.projects.len() == 1
    ));
    assert!(matches!(
        &sqlite_event.result,
        Ok(ReadSnapshot::Catalog(catalog)) if catalog.projects.len() == 1
    ));
    registry.apply_event(&duckdb_event);
    registry.apply_event(&sqlite_event);

    assert_eq!(
        registry.source(&duckdb_id).map(|source| &source.status),
        Some(&SourceStatus::Ready)
    );
    assert_eq!(
        registry.source(&sqlite_id).map(|source| &source.status),
        Some(&SourceStatus::Ready)
    );
    Ok(())
}

#[test]
fn worker_reports_a_missing_catalog_without_creating_it() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let mut worker = ReadWorker::spawn(root.path())?;
    let events = worker
        .take_event_receiver()
        .ok_or("worker event receiver should be available")?;
    let source_id = DataSourceId::new("missing").expect("test alias should be valid");
    worker.submit(
        source_id.clone(),
        Generation(1),
        ReadRequest::Discover(DiscoveryRequest::default()),
    )?;

    let event = receive_event(&events, EVENT_TIMEOUT)?;

    assert_eq!(event.source_id, source_id);
    assert!(matches!(
        event.result,
        Err(WorkerError::Source(SourceError::Sdk(
            seex::Error::CatalogNotFound { .. }
        )))
    ));
    assert!(!root.path().join(".seex").exists());
    Ok(())
}
