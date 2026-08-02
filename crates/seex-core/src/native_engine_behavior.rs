#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::ducklake_test_support::{
        TestDataset, attach_ducklake, initialize_test_storage_tables, seed_test_storage_data,
    };
    use crate::engine::query::NativeQueryStore;
    use crate::engine::write::NativeWriteStore;
    use crate::model::metric::{MetricKey, Step};
    use crate::model::run::RunId;
    use crate::model::types::ProjectId;

    const PROJECT_ID: &str = "project-1";
    const PROJECT_NAME: &str = "local training";

    struct BehaviorDataset {
        _dataset: TestDataset,
        connection: duckdb::Connection,
    }

    impl BehaviorDataset {
        fn connection(&self) -> &duckdb::Connection {
            &self.connection
        }
    }

    fn open_behavior_dataset() -> Result<BehaviorDataset, Box<dyn Error>> {
        let dataset = TestDataset::new()?;
        let connection = duckdb::Connection::open_in_memory()?;
        attach_ducklake(&connection, &dataset)?;
        initialize_test_storage_tables(&connection)?;
        Ok(BehaviorDataset {
            _dataset: dataset,
            connection,
        })
    }

    fn open_project_dataset() -> Result<BehaviorDataset, Box<dyn Error>> {
        let dataset = open_behavior_dataset()?;
        insert_project(dataset.connection())?;
        Ok(dataset)
    }

    fn insert_project(connection: &duckdb::Connection) -> Result<(), duckdb::Error> {
        connection.execute(
            "INSERT INTO seex_projects VALUES (?1, ?2, now())",
            duckdb::params![PROJECT_ID, PROJECT_NAME],
        )?;
        Ok(())
    }

    #[test]
    fn ducklake_round_trips_seeded_storage_data() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_behavior_dataset()?;
        let connection = dataset.connection();

        // Then
        let project_count: i64 =
            connection.query_row("SELECT count(*) FROM seex_projects", [], |row| row.get(0))?;
        let run_count: i64 =
            connection.query_row("SELECT count(*) FROM seex_runs", [], |row| row.get(0))?;
        let aggregate_count: i64 =
            connection.query_row("SELECT count(*) FROM seex_metric_aggregates", [], |row| {
                row.get(0)
            })?;
        let point_count: i64 =
            connection.query_row("SELECT count(*) FROM dl.metric_points", [], |row| {
                row.get(0)
            })?;
        assert_eq!(
            (project_count, run_count, aggregate_count, point_count),
            (0, 0, 0, 0),
        );
        seed_test_storage_data(connection)?;

        let metric_total: f64 = connection.query_row(
            "SELECT sum(value_f64)
             FROM dl.metric_points
             WHERE run_id = 'run-1'
               AND metric_key = 'train/loss'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(metric_total, 0.375);
        Ok(())
    }

    #[test]
    fn create_run_persists_generated_and_user_supplied_ids() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);

        // When
        let generated = store.create_run(&project_id, "generated", None)?;
        let supplied = store.create_run(
            &project_id,
            "supplied",
            Some(RunId::from_string("run-user-1")),
        )?;

        // Then
        assert_ne!(generated.run_id, supplied.run_id);
        assert_eq!(supplied.run_id.as_str(), "run-user-1");
        let run_count: i64 =
            connection.query_row("SELECT count(*) FROM seex_runs", [], |row| row.get(0))?;
        assert_eq!(run_count, 2);
        Ok(())
    }

    #[test]
    fn create_run_requires_explicit_resume_for_existing_run_id() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);
        let run_id = RunId::from_string("run-user-1");
        let created = store.create_run(&project_id, "first", Some(run_id.clone()))?;

        // When
        let duplicate = store.create_run(&project_id, "second", Some(run_id.clone()));
        let resumed = store.resume_run(&run_id)?;

        // Then
        assert!(matches!(
            duplicate,
            Err(seex_storage::StorageError::RunAlreadyExists { .. })
        ));
        assert_eq!(resumed, created);
        let run_count: i64 =
            connection.query_row("SELECT count(*) FROM seex_runs", [], |row| row.get(0))?;
        assert_eq!(run_count, 1);
        Ok(())
    }

    #[test]
    fn log_metric_at_step_stores_explicit_steps() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);
        let run = store.create_run(&project_id, "metrics", Some(RunId::from_string("run-1")))?;
        let metric_key = MetricKey::from_string("train/loss");

        // When
        let first = store.log_metric_at_step(&run.run_id, &metric_key, Step::new(0), 0.25)?;
        let second = store.log_metric_at_step(&run.run_id, &metric_key, Step::new(1), 0.125)?;

        // Then
        assert_eq!(first.step.value(), 0);
        assert_eq!(second.step.value(), 1);
        let stored: Vec<(i64, f64, String, bool)> = connection
            .prepare(
                "SELECT step, value_f64, metric_key_encoded, ingested_at IS NOT NULL
                 FROM dl.metric_points
                 WHERE run_id = 'run-1'
                   AND metric_key = 'train/loss'
                 ORDER BY step",
            )?
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<_, _>>()?;
        assert_eq!(
            stored,
            vec![
                (0, 0.25, "train%2Floss".to_owned(), true),
                (1, 0.125, "train%2Floss".to_owned(), true),
            ]
        );
        Ok(())
    }

    #[test]
    fn log_metric_at_step_stores_ingested_at_for_metric_points() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);
        let run = store.create_run(&project_id, "metrics", Some(RunId::from_string("run-1")))?;
        let metric_key = MetricKey::from_string("train/loss");

        // When
        let point = store.log_metric_at_step(&run.run_id, &metric_key, Step::new(0), 0.25)?;

        // Then
        assert!(point.ingested_at >= point.timestamp);
        let stored_ingest_is_after_timestamp: bool = connection.query_row(
            "SELECT ingested_at >= timestamp
             FROM dl.metric_points
             WHERE run_id = 'run-1'
               AND metric_key = 'train/loss'
               AND step = 0",
            [],
            |row| row.get(0),
        )?;
        assert!(stored_ingest_is_after_timestamp);
        Ok(())
    }

    #[test]
    fn query_metric_uses_last_write_wins_for_duplicate_steps() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let query = NativeQueryStore::from_duckdb(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);
        let run = store.create_run(&project_id, "metrics", Some(RunId::from_string("run-1")))?;
        let metric_key = MetricKey::from_string("train/loss");
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(0), 0.25)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(0), 0.125)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(1), 0.0625)?;

        // When
        let effective = query.query_metric_effective(&run.run_id, &metric_key)?;

        // Then
        let values: Vec<(i64, f64)> = effective
            .iter()
            .map(|point| (point.step.value(), point.value_f64))
            .collect();
        assert_eq!(values, vec![(0, 0.125), (1, 0.0625)]);
        Ok(())
    }

    #[test]
    fn query_metric_filters_effective_series_by_step_range() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let query = NativeQueryStore::from_duckdb(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);
        let run = store.create_run(&project_id, "metrics", Some(RunId::from_string("run-1")))?;
        let metric_key = MetricKey::from_string("train/loss");
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(0), 0.5)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(1), 0.25)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(1), 0.125)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(2), 0.0625)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(3), 0.03125)?;

        // When
        let points = query.query_metric(
            &run.run_id,
            &metric_key,
            Some(Step::new(1)),
            Some(Step::new(2)),
            None,
        )?;

        // Then
        let values: Vec<(i64, f64)> = points
            .iter()
            .map(|point| (point.step.value(), point.value_f64))
            .collect();
        assert_eq!(values, vec![(1, 0.125)]);
        Ok(())
    }

    #[test]
    fn query_metric_returns_short_series_unchanged_under_max_points() -> Result<(), Box<dyn Error>>
    {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let query = NativeQueryStore::from_duckdb(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);
        let run = store.create_run(&project_id, "metrics", Some(RunId::from_string("run-1")))?;
        let metric_key = MetricKey::from_string("train/loss");
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(0), 0.25)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(1), 0.125)?;

        // When
        let points = query.query_metric(&run.run_id, &metric_key, None, None, Some(2))?;

        // Then
        let values: Vec<(i64, f64)> = points
            .iter()
            .map(|point| (point.step.value(), point.value_f64))
            .collect();
        assert_eq!(values, vec![(0, 0.25), (1, 0.125)]);
        Ok(())
    }

    #[test]
    fn query_metric_downsamples_long_series_with_duckdb_lttb() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_behavior_dataset()?;
        let connection = dataset.connection();
        connection.execute_batch(
            "CREATE MACRO lttb(x, y, n) AS [
                 list({x: x::DOUBLE, y: y} ORDER BY x)[1],
                 list({x: x::DOUBLE, y: y} ORDER BY x)[len(list({x: x, y: y} ORDER BY x))]];
             INSERT INTO seex_projects VALUES ('project-1', 'local training', now());
             INSERT INTO seex_runs VALUES
                 ('run-1', 'project-1', 'metrics', 'running', now(), now(), NULL);
             INSERT INTO dl.metric_points VALUES
                 ('run-1', 'train/loss', 'train%2Floss', 9223372036854775804, now(), 0.5, now()),
                 ('run-1', 'train/loss', 'train%2Floss', 9223372036854775805, now(), 0.25, now()),
                 ('run-1', 'train/loss', 'train%2Floss', 9223372036854775806, now(), 0.125, now()),
                 ('run-1', 'train/loss', 'train%2Floss', 9223372036854775807, now(), 0.0625, now());",
        )?;
        let query = NativeQueryStore::from_duckdb(connection);
        let run_id = RunId::from_string("run-1");
        let metric_key = MetricKey::from_string("train/loss");

        // When
        let result = query.query_metric_with_metadata(&run_id, &metric_key, None, None, Some(2))?;

        // Then
        let values: Vec<(i64, f64)> = result
            .points
            .iter()
            .map(|point| (point.step.value(), point.value_f64))
            .collect();
        assert_eq!(values, vec![(i64::MAX - 3, 0.5), (i64::MAX, 0.0625),],);
        assert_eq!(result.points.len(), 2);
        assert_eq!(result.source_row_count, 4);
        Ok(())
    }

    #[test]
    fn query_metric_rejects_max_points_below_two() -> Result<(), Box<dyn Error>> {
        let dataset = open_project_dataset()?;
        let query = NativeQueryStore::from_duckdb(dataset.connection());
        let error = query
            .query_metric(
                &RunId::from_string("run-1"),
                &MetricKey::from_string("train/loss"),
                None,
                None,
                Some(1),
            )
            .unwrap_err();

        assert!(
            matches!(
                error,
                crate::engine::EngineError::MetricQueryMaxPointsTooSmall { max_points: 1 }
            ),
            "expected max-points validation error, got {error:?}",
        );
        Ok(())
    }

    #[test]
    fn metric_aggregate_tracks_effective_series_values() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let query = NativeQueryStore::from_duckdb(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);
        let run = store.create_run(&project_id, "metrics", Some(RunId::from_string("run-1")))?;
        let metric_key = MetricKey::from_string("train/loss");
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(0), 0.25)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(1), 0.5)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(1), 0.125)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(2), 0.375)?;
        store.rebuild_metric_aggregates_for_run(&run.run_id)?;

        // When
        let aggregate = query.metric_aggregate(&run.run_id, &metric_key)?;

        // Then
        assert_eq!(aggregate.effective_count, 3);
        assert_eq!(aggregate.last_step, Step::new(2));
        assert_eq!(aggregate.last_value_f64, 0.375);
        assert_eq!(aggregate.min_value_f64, 0.125);
        assert_eq!(aggregate.max_value_f64, 0.375);
        Ok(())
    }

    #[test]
    fn query_metric_summaries_returns_multi_run_comparison() -> Result<(), Box<dyn Error>> {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let query = NativeQueryStore::from_duckdb(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);
        let run_a = store.create_run(&project_id, "a", Some(RunId::from_string("run-a")))?;
        let run_b = store.create_run(&project_id, "b", Some(RunId::from_string("run-b")))?;
        let metric_key = MetricKey::from_string("train/loss");
        store.log_metric_at_step(&run_a.run_id, &metric_key, Step::new(0), 0.5)?;
        store.log_metric_at_step(&run_a.run_id, &metric_key, Step::new(1), 0.25)?;
        store.log_metric_at_step(&run_b.run_id, &metric_key, Step::new(0), 0.4)?;
        store.log_metric_at_step(&run_b.run_id, &metric_key, Step::new(1), 0.2)?;
        store.log_metric_at_step(&run_b.run_id, &metric_key, Step::new(2), 0.1)?;
        // When
        let summaries = query
            .query_metric_summaries(&[run_b.run_id.clone(), run_a.run_id.clone()], &metric_key)?;

        // Then
        let values: Vec<(&str, u64, i64, f64, f64, f64)> = summaries
            .iter()
            .map(|summary| {
                (
                    summary.run_id.as_str(),
                    summary.effective_count,
                    summary.last_step.value(),
                    summary.last_value_f64,
                    summary.min_value_f64,
                    summary.max_value_f64,
                )
            })
            .collect();
        assert_eq!(
            values,
            vec![
                ("run-b", 3, 2, 0.1, 0.1, 0.4),
                ("run-a", 2, 1, 0.25, 0.25, 0.5),
            ],
        );
        Ok(())
    }

    #[test]
    fn rebuild_metric_aggregates_refreshes_stale_old_step_overwrite() -> Result<(), Box<dyn Error>>
    {
        // Given
        let dataset = open_project_dataset()?;
        let connection = dataset.connection();
        let store = NativeWriteStore::new(connection);
        let query = NativeQueryStore::from_duckdb(connection);
        let project_id = ProjectId::from_string(PROJECT_ID);
        let run = store.create_run(&project_id, "metrics", Some(RunId::from_string("run-1")))?;
        let metric_key = MetricKey::from_string("train/loss");
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(0), 0.25)?;
        store.log_metric_at_step(&run.run_id, &metric_key, Step::new(1), 0.5)?;
        store.rebuild_metric_aggregates_for_run(&run.run_id)?;
        connection.execute(
            "INSERT INTO dl.metric_points VALUES
                 ('run-1', 'train/loss', 'train%2Floss', 0, now(), 0.125, now())",
            [],
        )?;
        let stale = query.metric_aggregate(&run.run_id, &metric_key)?;

        // When
        store.rebuild_metric_aggregates_for_run(&run.run_id)?;
        let repaired = query.metric_aggregate(&run.run_id, &metric_key)?;

        // Then
        assert_eq!(stale.min_value_f64, 0.25);
        assert_eq!(repaired.effective_count, 2);
        assert_eq!(repaired.min_value_f64, 0.125);
        assert_eq!(repaired.max_value_f64, 0.5);
        Ok(())
    }
}
