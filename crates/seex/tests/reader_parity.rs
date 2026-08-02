use seex::storage::bootstrap::open_native_connection;
use seex::storage::{MetricWrite, ProjectConnection};
use seex::{
    CatalogBackend, Error, EvidenceCompleteness, EvidenceReason, MetricAxis, MetricCoordinate,
    MetricKey, MetricQuery, MetricRange, Project, ProjectId, Reader, RelativeTime, RunId,
    RunStatus, Step, Timestamp,
};

struct Fixture {
    _root: tempfile::TempDir,
    native: Reader,
    standalone: Reader,
    run_id: RunId,
    started_at: i64,
}

#[test]
fn native_reader_honors_explicit_storage_overrides_without_creating_a_store()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open()?;
    let root = fixture._root.path();
    std::fs::write(
        root.join(".seex/config.toml"),
        "schema_version = 1\ncatalog_backend = 'sqlite'\n",
    )?;

    let reader = Reader::builder(root)
        .catalog_backend(CatalogBackend::DuckDb)
        .catalog_path(root.join(".seex/catalog.ducklake"))
        .data_path(root.join(".seex/data"))
        .open()?;

    assert_eq!(reader.projects()?.len(), 1);
    let empty = tempfile::tempdir()?;
    assert_eq!(
        Reader::builder(empty.path()).open().err(),
        Some(Error::Storage)
    );
    assert!(!empty.path().join(".seex/catalog.ducklake").exists());
    Ok(())
}

impl Fixture {
    fn open() -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let connection = ProjectConnection::new(open_native_connection(root.path())?);
        let created_at = seex::storage::time::current_timestamp("created_at")?;
        let project = Project {
            project_id: ProjectId::from_string("project-1"),
            name: String::from("reader parity"),
            created_at,
        };
        connection.create_project(&project)?;
        let run = connection.create_run(&project.project_id, "run", RunId::from_string("run-1"))?;
        let started_at = run.started_at.timestamp_millis();
        let mut rows = (0..8)
            .map(|step| MetricWrite {
                run_id: run.run_id.as_str().to_owned(),
                metric_key: String::from("loss"),
                step,
                timestamp_millis: started_at + step,
                value_f64: step as f64,
                ingested_at_millis: started_at + step,
            })
            .collect::<Vec<_>>();
        rows.extend(
            [(2, 42.0), (4, 1_000.0)].map(|(step, value_f64)| MetricWrite {
                run_id: run.run_id.as_str().to_owned(),
                metric_key: String::from("loss"),
                step,
                timestamp_millis: started_at + step,
                value_f64,
                ingested_at_millis: started_at + 10 + step,
            }),
        );
        connection.append_metric_batch(&rows)?;
        connection.rebuild_metric_aggregates_for_run(&run.run_id)?;
        connection.mark_run_terminal(&run.run_id, RunStatus::Finished, created_at)?;
        connection.flush_metric_points()?;
        drop(connection);
        std::fs::write(
            root.path().join(".seex/config.toml"),
            "schema_version = 1\ncatalog_path = '.seex/catalog.ducklake'\n\
             data_path = '.seex/data'\n",
        )?;
        let native = Reader::builder(root.path()).open()?;
        let started_at = native.runs(&project.project_id)?[0]
            .started_at
            .timestamp_millis();
        let parquet = root
            .path()
            .join(".seex/data/main/metric_points/**/*.parquet")
            .to_string_lossy()
            .into_owned();
        let standalone = Reader::parquet(parquet).open()?;
        Ok(Self {
            _root: root,
            native,
            standalone,
            run_id: run.run_id,
            started_at,
        })
    }

    fn query(&self, reader: &Reader, query: MetricQuery) -> seex::Result<seex::MetricSeries> {
        reader.query_metric(&self.run_id, &MetricKey::from_string("loss"), &query)
    }
}

#[test]
fn native_and_standalone_ranges_have_strict_bounded_parity()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open()?;
    let queries = [
        MetricQuery::new(MetricRange::All(MetricAxis::Step), Some(3))?,
        MetricQuery::new(
            MetricRange::Steps {
                start: Step::new(1),
                end: Step::new(6),
            },
            Some(2),
        )?,
        MetricQuery::new(MetricRange::All(MetricAxis::Timestamp), Some(3))?,
        MetricQuery::new(
            MetricRange::Timestamps {
                start: Timestamp::from_millis(fixture.started_at),
                end: Timestamp::from_millis(fixture.started_at + 60_000),
            },
            Some(3),
        )?,
    ];

    for query in queries {
        let native = fixture.query(&fixture.native, query.clone())?;
        let standalone = fixture.query(&fixture.standalone, query.clone())?;

        assert_eq!(standalone, native);
        assert!(native.samples().len() <= query.max_points().unwrap_or(usize::MAX));
        match query.range() {
            MetricRange::Steps { start, end } => assert!(native.samples().iter().all(|sample| {
                matches!(sample.coordinate, MetricCoordinate::Step(step)
                    if start <= &step && &step < end)
            })),
            MetricRange::Timestamps { start, end } => {
                assert!(native.samples().iter().all(|sample| {
                    matches!(sample.coordinate, MetricCoordinate::Timestamp(timestamp)
                        if start <= &timestamp && &timestamp < end)
                }));
            }
            _ => {}
        }
    }
    assert_eq!(fixture.standalone.projects(), Err(Error::UnsupportedQuery));
    Ok(())
}

#[test]
fn standalone_relative_ranges_report_missing_run_start() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open()?;
    for range in [
        MetricRange::All(MetricAxis::RelativeTime),
        MetricRange::RelativeTime {
            start: RelativeTime::from_millis(0),
            end: RelativeTime::from_millis(60_000),
        },
    ] {
        let query = MetricQuery::new(range, Some(2))?;
        let native = fixture.query(&fixture.native, query.clone())?;
        let standalone = fixture.query(&fixture.standalone, query)?;

        assert!(!native.samples().is_empty());
        assert!(native.samples().len() <= 2);
        assert!(standalone.samples().is_empty());
        assert_eq!(standalone.completeness(), EvidenceCompleteness::Unavailable);
        assert_eq!(standalone.reasons(), [EvidenceReason::MissingRunStart]);
    }
    Ok(())
}

#[test]
fn small_fixture_preserves_lww_spikes_and_true_step_neighbors()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open()?;
    let metric = MetricKey::from_string("loss");
    let narrow = MetricQuery::new(
        MetricRange::Steps {
            start: Step::new(2),
            end: Step::new(3),
        },
        None,
    )?;

    let native = fixture
        .native
        .query_metric_for_desktop(&fixture.run_id, &metric, &narrow)?;
    let standalone =
        fixture
            .standalone
            .query_metric_for_desktop(&fixture.run_id, &metric, &narrow)?;
    let points = native
        .samples()
        .iter()
        .map(|sample| match sample.coordinate {
            MetricCoordinate::Step(step) => (step.value(), sample.point.value_f64),
            _ => panic!("Step query returned another coordinate axis"),
        })
        .collect::<Vec<_>>();

    assert_eq!(standalone, native);
    assert_eq!(points, [(1, 1.0), (2, 42.0), (3, 3.0)]);
    let spike = fixture.query(
        &fixture.native,
        MetricQuery::new(
            MetricRange::Steps {
                start: Step::new(4),
                end: Step::new(5),
            },
            None,
        )?,
    )?;
    assert_eq!(spike.samples().len(), 1);
    assert_eq!(spike.samples()[0].point.value_f64, 1_000.0);
    Ok(())
}
