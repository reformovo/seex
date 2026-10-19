use crate::storage::bootstrap::{
    CatalogBackend, NativeStorageConfig, open_native_connection_with_config,
};
use crate::storage::{MetricWrite, ProjectConnection};
use crate::{
    Error, MAX_CHART_POINTS, MetricCoordinate, MetricKey, MetricQuery, MetricRange, Project,
    ProjectId, Reader, RunId, RunStatus, Step,
};

struct Fixture {
    _root: tempfile::TempDir,
    native: Reader,
    run_id: RunId,
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
        .catalog_backend(crate::CatalogBackend::Sqlite)
        .catalog_path(root.join(".seex/catalog.sqlite"))
        .data_path(root.join(".seex/data"))
        .open()?;

    assert_eq!(reader.projects()?.len(), 1);
    let empty = tempfile::tempdir()?;
    assert_eq!(
        Reader::builder(empty.path()).open().err(),
        Some(Error::CatalogNotFound {
            name: String::from("catalog.sqlite")
        })
    );
    assert!(!empty.path().join(".seex/catalog.sqlite").exists());
    Ok(())
}

#[test]
fn native_reader_scopes_run_lookup_and_optional_metric_summaries()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open()?;
    let project_id = ProjectId::from_string("project-1");
    let run = fixture
        .native
        .run(&project_id, &fixture.run_id)?
        .expect("fixture Run should exist");

    assert!(
        fixture
            .native
            .run(&ProjectId::from_string("other"), &fixture.run_id)?
            .is_none()
    );
    assert!(
        fixture
            .native
            .run(&project_id, &RunId::from_string("missing"))?
            .is_none()
    );
    assert_eq!(
        fixture
            .native
            .metric_summary(&run, &MetricKey::from_string("loss"))?
            .map(|summary| summary.effective_count),
        Some(10)
    );
    let series = fixture.query(MetricQuery::new(
        MetricRange::All(crate::MetricAxis::Step),
        MAX_CHART_POINTS,
    )?)?;
    assert_eq!(series.source_count(), 10);
    assert_eq!(series.samples().len(), 10);
    let mut duplicates = series
        .samples()
        .iter()
        .filter(|sample| sample.point.step == Step::new(2))
        .map(|sample| sample.point.value_f64)
        .collect::<Vec<_>>();
    duplicates.sort_by(f64::total_cmp);
    assert_eq!(duplicates, vec![2.0, 42.0]);
    Ok(())
}

impl Fixture {
    fn open() -> Result<Self, Box<dyn std::error::Error>> {
        Self::open_with_timestamp_order(false)
    }

    fn open_with_timestamp_order(decreasing: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let connection = ProjectConnection::new(open_native_connection_with_config(
            NativeStorageConfig::with_backend_and_s3_config(
                CatalogBackend::Sqlite,
                root.path(),
                None,
                None,
                None,
            ),
        )?);
        let created_at = crate::storage::time::current_timestamp("created_at")?;
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
        if decreasing {
            for row in &mut rows {
                row.timestamp_millis = started_at + 100 - row.step;
            }
        }
        connection.append_metric_batch(&rows)?;
        connection.rebuild_metric_aggregates_for_run(&run.run_id)?;
        connection.mark_run_terminal(&run.run_id, RunStatus::Finished, created_at)?;
        connection.flush_metric_points()?;
        drop(connection);
        std::fs::write(
            root.path().join(".seex/config.toml"),
            "schema_version = 1\ncatalog_backend = 'sqlite'\ncatalog_path = '.seex/catalog.sqlite'\n\
             data_path = '.seex/data'\n",
        )?;
        Ok(Self {
            native: Reader::builder(root.path()).open()?,
            _root: root,
            run_id: run.run_id,
        })
    }

    fn query(&self, query: MetricQuery) -> crate::Result<crate::MetricSeries> {
        self.native
            .query_metric(&self.run_id, &MetricKey::from_string("loss"), &query)
    }
}

#[test]
fn step_overview_keeps_invalid_timestamp_evidence_and_observations()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open_with_timestamp_order(true)?;
    let metric = MetricKey::from_string("loss");
    for budget in [4, MAX_CHART_POINTS] {
        let query = MetricQuery::new(MetricRange::All(crate::MetricAxis::Step), budget)?;
        let expected = fixture.query(query.clone())?;
        for _ in 0..2 {
            let actual = fixture.native.query_metric_overview_for_desktop(
                &fixture.run_id,
                &metric,
                &query,
            )?;
            assert_eq!(actual.source_count(), 10);
            assert!(actual.samples().len() <= budget);
            assert_eq!(actual.completeness(), crate::EvidenceCompleteness::Invalid);
            assert_eq!(actual.reasons(), &[crate::EvidenceReason::DecreasingAxis]);
            let payloads = |series: &crate::MetricSeries| {
                let mut rows = series
                    .samples()
                    .iter()
                    .map(|s| {
                        (
                            s.point.step.value(),
                            s.point.timestamp.timestamp_millis(),
                            s.point.value_f64.to_bits(),
                            s.point.ingested_at.timestamp_millis(),
                        )
                    })
                    .collect::<Vec<_>>();
                rows.sort();
                rows
            };
            assert_eq!(payloads(&actual), payloads(&expected));
        }
    }
    Ok(())
}

#[test]
fn panel_queries_match_single_curve_order_and_cancel_without_partial_results()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open()?;
    let reader = &fixture.native;
    let client = crate::Client::builder(fixture._root.path()).open()?;
    let run = client.start_run(crate::RunOptions::new("project-1").id("run-2"))?;
    run.log_with([("loss", 5.0)], crate::LogOptions::new().step(0))?;
    run.finish()?;
    let ids = [
        RunId::from_string("run-2"),
        fixture.run_id.clone(),
        RunId::from_string("run-2"),
    ];
    let key = MetricKey::from_string("loss");
    for overview in [false, true] {
        let query = MetricQuery::new(
            MetricRange::Steps {
                start: Step::new(2),
                end: Step::new(4),
            },
            8,
        )?;
        let actual = reader
            .query_metrics_for_desktop(&ids, &key, &query, overview, &mut || false)?
            .unwrap();
        for (id, series) in ids.iter().zip(actual) {
            let expected = if overview {
                reader.query_metric_overview_for_desktop(id, &key, &query)?
            } else {
                reader.query_metric_for_desktop(id, &key, &query)?
            };
            assert_eq!(series.source_count(), expected.source_count());
            assert_eq!(series.completeness(), expected.completeness());
            assert_eq!(series.reasons(), expected.reasons());
            let payloads = |s: &crate::MetricSeries| {
                let mut rows = s
                    .samples()
                    .iter()
                    .map(|p| format!("{:?}", p.point))
                    .collect::<Vec<_>>();
                rows.sort();
                rows
            };
            assert_eq!(payloads(&series), payloads(&expected));
        }
    }
    let query = MetricQuery::new(MetricRange::All(crate::MetricAxis::Step), 8)?;
    let panel = |ids: &[RunId], cancel: &mut dyn FnMut() -> bool| {
        reader.query_metrics_for_desktop(ids, &key, &query, true, cancel)
    };
    let mut checks = 0;
    let cancelled = panel(&ids, &mut || {
        checks += 1;
        checks >= 3
    })?;
    assert!(cancelled.is_none());
    assert!(panel(&[], &mut || false)?.unwrap().is_empty());
    let too_many = vec![fixture.run_id.clone(); 17];
    assert_eq!(
        panel(&too_many, &mut || false).unwrap_err(),
        Error::UnsupportedQuery
    );
    Ok(())
}

#[test]
fn panel_reopens_after_external_inline_flush() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open()?;
    let reader = &fixture.native;
    let client = crate::Client::builder(fixture._root.path()).open()?;
    let run = client.start_run(crate::RunOptions::new("project-1").id("live"))?;
    for step in 0..3 {
        run.log_with([("loss", step as f64)], crate::LogOptions::new().step(step))?;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while client.diagnostics().pending_reports != 0 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let ids = [
        RunId::from_string("live"),
        fixture.run_id.clone(),
        RunId::from_string("live"),
    ];
    let key = MetricKey::from_string("loss");
    let query = MetricQuery::new(MetricRange::All(crate::MetricAxis::Step), 8)?;
    let mut checks = 0;
    let during = reader
        .query_metrics_for_desktop(&ids, &key, &query, true, &mut || {
            checks += 1;
            if checks == 3 {
                run.finish().expect("external finish/flush must succeed");
            }
            false
        })?
        .unwrap();
    assert_eq!(
        during.iter().map(|s| s.source_count()).collect::<Vec<_>>(),
        vec![3, 10, 3]
    );
    let after = reader
        .query_metrics_for_desktop(&ids, &key, &query, true, &mut || false)?
        .unwrap();
    assert_eq!(after[0].source_count(), 3);
    let status = after[0].completeness();
    assert_eq!(status, crate::EvidenceCompleteness::Complete);
    Ok(())
}

#[test]
fn native_ranges_are_strictly_bounded() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open()?;
    let queries = [
        MetricQuery::new(MetricRange::All(crate::MetricAxis::Step), 3)?,
        MetricQuery::new(
            MetricRange::Steps {
                start: Step::new(1),
                end: Step::new(6),
            },
            2,
        )?,
        MetricQuery::new(
            MetricRange::StepsFrom {
                start: Step::new(5),
            },
            2,
        )?,
        MetricQuery::new(MetricRange::StepsUntil { end: Step::new(3) }, 2)?,
    ];

    for query in queries {
        let native = fixture.query(query.clone())?;
        assert!(native.samples().len() <= query.max_points());
    }
    assert!(MetricQuery::new(MetricRange::All(crate::MetricAxis::Step), MAX_CHART_POINTS).is_ok());
    Ok(())
}

#[test]
fn relative_time_and_timestamp_ranges_are_rejected() {
    assert!(MetricQuery::new(MetricRange::All(crate::MetricAxis::RelativeTime), 2).is_err());
    assert!(MetricQuery::new(MetricRange::All(crate::MetricAxis::Timestamp), 2).is_err());
}

#[test]
fn small_fixture_preserves_repeated_observations_and_true_step_neighbors()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::open()?;
    let metric = MetricKey::from_string("loss");
    let narrow = MetricQuery::new(
        MetricRange::Steps {
            start: Step::new(2),
            end: Step::new(3),
        },
        4,
    )?;

    let native = fixture
        .native
        .query_metric_for_desktop(&fixture.run_id, &metric, &narrow)?;
    let points = native
        .samples()
        .iter()
        .map(|sample| match sample.coordinate {
            MetricCoordinate::Step(step) => (step.value(), sample.point.value_f64),
            _ => panic!("Step query returned another coordinate axis"),
        })
        .collect::<Vec<_>>();
    assert_eq!(points.len(), 4);
    assert_eq!(points[0], (1, 1.0));
    assert_eq!(points[3], (3, 3.0));
    let mut repeated = points[1..3].to_vec();
    repeated.sort_by(|left, right| left.1.total_cmp(&right.1));
    assert_eq!(repeated, [(2, 2.0), (2, 42.0)]);
    Ok(())
}
