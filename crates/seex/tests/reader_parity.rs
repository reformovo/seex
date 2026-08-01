use seex::{
    Error, EvidenceReason, MetricAxis, MetricCoordinate, MetricKey, MetricQuery, MetricRange,
    Reader, RelativeTime, Step, Timestamp,
};
use seex_core::engine::client::NativeClient;
use seex_model::run::RunId;
use seex_model::types::ProjectId;

struct Fixture {
    _root: tempfile::TempDir,
    native: Reader,
    standalone: Reader,
    run_id: RunId,
    started_at: i64,
}

impl Fixture {
    fn open() -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = NativeClient::open_with_storage_config(root.path(), None, None, 1_024)?;
        let project =
            client.create_project("reader parity", Some(ProjectId::from_string("project-1")))?;
        let run = client.create_run(
            &project.project_id,
            "run",
            Some(RunId::from_string("run-1")),
        )?;
        let handle = client.run_handle(run.clone());
        for step in 0..8 {
            handle.log_metric_at_step("loss", step, step as f64)?;
        }
        handle.log_metric_at_step("loss", 2, 42.0)?;
        handle.log_metric_at_step("loss", 4, 1_000.0)?;
        client.finish_run(&run.run_id)?;
        client.shutdown(None)?;
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
