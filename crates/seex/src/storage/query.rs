use duckdb::Connection;
use seex_model::alignment::{AlignmentAxis, AlignmentQuery, AlignmentQueryResult, AlignmentReason};
use seex_model::metric::{MetricQuery, MetricQueryResult};

use crate::storage::alignment_query::{
    AlignmentSource, query_aligned_metric, validate_alignment_identity,
};
use crate::storage::metric_query::{MetricSource, query_metric};
use crate::storage::{
    ColumnSchema, MetricReader, SchemaReport, StorageError, validate_metric_point_schema,
};

/// A DuckDB-readable Parquet file, glob, or object-store URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParquetSource(String);

impl ParquetSource {
    /// Creates a non-empty DuckDB Parquet location.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::EmptySource`] for a blank location.
    pub fn new(location: impl Into<String>) -> Result<Self, StorageError> {
        let location = location.into();
        if location.trim().is_empty() {
            return Err(StorageError::EmptySource);
        }
        Ok(Self(location))
    }

    pub fn location(&self) -> &str {
        &self.0
    }
}

/// Validated reader for a standalone Seex Parquet dataset.
pub struct ParquetMetricReader<'connection> {
    connection: &'connection Connection,
    source: ParquetSource,
    schema: SchemaReport,
}

impl<'connection> ParquetMetricReader<'connection> {
    /// Inspects and validates a Parquet source before it becomes queryable.
    ///
    /// # Errors
    ///
    /// Returns an error for DuckDB failures or an incompatible schema.
    pub fn open(
        connection: &'connection Connection,
        source: ParquetSource,
    ) -> Result<Self, StorageError> {
        let schema = inspect_schema(connection, &source)?;
        Ok(Self {
            connection,
            source,
            schema,
        })
    }

    pub const fn schema(&self) -> &SchemaReport {
        &self.schema
    }

    /// Queries one effective metric series with its requested reduction policy.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for storage, extension, or conversion failures.
    pub fn query_metric(&self, query: &MetricQuery) -> Result<MetricQueryResult, StorageError> {
        query_metric(
            self.connection,
            MetricSource::Parquet(self.source.location()),
            query,
        )
    }

    /// Queries standalone metric facts on a derived comparison axis.
    ///
    /// Elapsed alignment is unavailable because standalone facts contain no
    /// Run start metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for storage or conversion failures.
    pub fn query_aligned_metric(
        &self,
        query: &AlignmentQuery,
    ) -> Result<AlignmentQueryResult, StorageError> {
        validate_alignment_identity(query)?;
        if matches!(query.axis, AlignmentAxis::ElapsedTime) {
            return Ok(AlignmentQueryResult {
                points: Vec::new(),
                source_row_count: 0,
                reasons: vec![AlignmentReason::MissingRunStart],
            });
        }
        query_aligned_metric(
            self.connection,
            AlignmentSource::Parquet(self.source.location()),
            query,
            None,
        )
    }
}

impl MetricReader for ParquetMetricReader<'_> {
    fn query_metric(&self, query: &MetricQuery) -> Result<MetricQueryResult, StorageError> {
        Self::query_metric(self, query)
    }

    fn query_aligned_metric(
        &self,
        query: &AlignmentQuery,
    ) -> Result<AlignmentQueryResult, StorageError> {
        Self::query_aligned_metric(self, query)
    }
}

/// Owned reader for a standalone Seex Parquet dataset.
pub struct StandaloneMetricReader {
    connection: Connection,
    source: ParquetSource,
    schema: SchemaReport,
}

impl StandaloneMetricReader {
    /// Opens an isolated in-memory query connection and validates the source.
    ///
    /// # Errors
    ///
    /// Returns an error for DuckDB failures or an incompatible Parquet schema.
    pub fn open(source: ParquetSource) -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory()?;
        let schema = inspect_schema(&connection, &source)?;
        Ok(Self {
            connection,
            source,
            schema,
        })
    }

    pub const fn schema(&self) -> &SchemaReport {
        &self.schema
    }

    /// Queries one effective metric series.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for query or conversion failures.
    pub fn query_metric(&self, query: &MetricQuery) -> Result<MetricQueryResult, StorageError> {
        query_metric(
            &self.connection,
            MetricSource::Parquet(self.source.location()),
            query,
        )
    }

    /// Queries Step facts or reports a missing Run start for elapsed time.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for query or conversion failures.
    pub fn query_aligned_metric(
        &self,
        query: &AlignmentQuery,
    ) -> Result<AlignmentQueryResult, StorageError> {
        validate_alignment_identity(query)?;
        if matches!(query.axis, AlignmentAxis::ElapsedTime) {
            return Ok(AlignmentQueryResult {
                points: Vec::new(),
                source_row_count: 0,
                reasons: vec![AlignmentReason::MissingRunStart],
            });
        }
        query_aligned_metric(
            &self.connection,
            AlignmentSource::Parquet(self.source.location()),
            query,
            None,
        )
    }

    /// Queries absolute timestamps without requiring Run metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for query or conversion failures.
    pub fn query_timestamp_metric(
        &self,
        query: &AlignmentQuery,
    ) -> Result<AlignmentQueryResult, StorageError> {
        let mut query = query.clone();
        query.axis = AlignmentAxis::ElapsedTime;
        query_aligned_metric(
            &self.connection,
            AlignmentSource::Parquet(self.source.location()),
            &query,
            Some(0),
        )
    }
}

impl MetricReader for StandaloneMetricReader {
    fn query_metric(&self, query: &MetricQuery) -> Result<MetricQueryResult, StorageError> {
        Self::query_metric(self, query)
    }

    fn query_aligned_metric(
        &self,
        query: &AlignmentQuery,
    ) -> Result<AlignmentQueryResult, StorageError> {
        Self::query_aligned_metric(self, query)
    }
}

fn inspect_schema(
    connection: &Connection,
    source: &ParquetSource,
) -> Result<SchemaReport, StorageError> {
    for file_name in parquet_file_names(connection, source)? {
        let columns = describe_parquet(connection, &file_name, false)?;
        validate_metric_point_schema(columns)?;
    }

    let columns = describe_parquet(connection, source.location(), true)?;
    validate_metric_point_schema(columns)
}

fn parquet_file_names(
    connection: &Connection,
    source: &ParquetSource,
) -> Result<Vec<String>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT file_name
         FROM parquet_schema(?)
         ORDER BY file_name",
    )?;
    let rows = statement.query_map([source.location()], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

fn describe_parquet(
    connection: &Connection,
    location: &str,
    union_by_name: bool,
) -> Result<Vec<ColumnSchema>, StorageError> {
    let sql = if union_by_name {
        "DESCRIBE SELECT * FROM read_parquet(
             ?, hive_partitioning = true, union_by_name = true
         )"
    } else {
        "DESCRIBE SELECT * FROM read_parquet(?, hive_partitioning = true)"
    };
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([location], |row| {
        let nullable: String = row.get(2)?;
        Ok(ColumnSchema::new(
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            nullable.eq_ignore_ascii_case("YES"),
        ))
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use seex_model::alignment::{AlignmentReduction, AlignmentViewport};
    use seex_model::metric::{MetricKey, ReductionPolicy, Step};
    use seex_model::run::RunId;

    use super::*;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestParquet {
        directory: PathBuf,
        location: String,
    }

    impl TestParquet {
        fn create(connection: &Connection) -> Result<Self, Box<dyn Error>> {
            let directory = test_directory();
            fs::create_dir_all(&directory)?;
            let path = directory.join("metric_points.parquet");
            connection.execute_batch(&format!(
                "CREATE TABLE metric_points (
                     run_id VARCHAR NOT NULL,
                     metric_key VARCHAR NOT NULL,
                     metric_key_encoded VARCHAR NOT NULL,
                     step BIGINT NOT NULL,
                     timestamp TIMESTAMPTZ NOT NULL,
                     value_f64 DOUBLE NOT NULL,
                     ingested_at TIMESTAMPTZ NOT NULL
                 );
                 INSERT INTO metric_points
                 SELECT 'run-1', 'train/loss', 'train%2Floss', step,
                        to_timestamp((1767225600 + step)::DOUBLE), 1.0 / step,
                        to_timestamp((1767225610 + step)::DOUBLE)
                 FROM range(1, 26) AS generated(step);
                 INSERT INTO metric_points VALUES
                     ('run-1', 'train/loss', 'train%2Floss', 2,
                      '2026-01-01 00:00:02+00', 0.2, '2026-01-01 00:01:00+00');
                 COPY metric_points TO '{}' (FORMAT PARQUET);",
                path.display()
            ))?;
            Ok(Self {
                directory,
                location: path.to_string_lossy().into_owned(),
            })
        }

        fn create_missing_column(connection: &Connection) -> Result<Self, Box<dyn Error>> {
            let directory = test_directory();
            fs::create_dir_all(&directory)?;
            let path = directory.join("metric_points.parquet");
            connection.execute_batch(&format!(
                "COPY (SELECT 'run-1'::VARCHAR AS run_id,
                              'loss'::VARCHAR AS metric_key,
                              1::BIGINT AS step,
                              now() AS timestamp,
                              1.0::DOUBLE AS value_f64,
                              now() AS ingested_at)
                 TO '{}' (FORMAT PARQUET);",
                path.display()
            ))?;
            Ok(Self {
                directory,
                location: path.to_string_lossy().into_owned(),
            })
        }
    }

    impl Drop for TestParquet {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn test_directory() -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("seex-storage-{}-{id}", std::process::id()))
    }

    fn query(reduction: ReductionPolicy) -> Result<MetricQuery, Box<dyn Error>> {
        Ok(MetricQuery::new(
            RunId::from_string("run-1"),
            MetricKey::from_string("train/loss"),
            Some(Step::new(2)),
            Some(Step::new(5)),
            reduction,
        )?)
    }

    fn aligned_query(
        axis: AlignmentAxis,
        viewport: AlignmentViewport,
        reduction: AlignmentReduction,
    ) -> AlignmentQuery {
        AlignmentQuery {
            run_id: RunId::from_string("run-1"),
            metric_key: MetricKey::from_string("train/loss"),
            axis,
            viewport,
            reduction,
        }
    }

    #[test]
    fn reader_rejects_a_physical_file_missing_a_contract_column() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        let parquet = TestParquet::create_missing_column(&connection)?;
        let source = ParquetSource::new(parquet.location.clone())?;

        let result = ParquetMetricReader::open(&connection, source);

        assert!(matches!(
            result,
            Err(StorageError::MissingColumn {
                name: "metric_key_encoded"
            })
        ));
        Ok(())
    }

    #[test]
    fn reader_queries_full_and_screen_reduced_effective_series() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        let parquet = TestParquet::create(&connection)?;
        let reader =
            ParquetMetricReader::open(&connection, ParquetSource::new(parquet.location.clone())?)?;

        let full = reader.query_metric(&query(ReductionPolicy::Full)?)?;
        let screen = reader.query_metric(&query(ReductionPolicy::screen_budget(1, 1)?)?)?;
        let lttb = reader.query_metric(&query(ReductionPolicy::lttb(10)?)?)?;

        assert_eq!(full.source_row_count, 3);
        assert_eq!(full.points.len(), 3);
        assert_eq!(full.points[0].step.value(), 2);
        assert_eq!(full.points[0].value_f64, 0.2);
        assert!(screen.points.len() <= 4);
        assert_eq!(screen.source_row_count, 3);
        assert_eq!(lttb.points, full.points);
        Ok(())
    }

    #[test]
    fn reader_aligns_step_facts_with_closed_viewport_neighbors() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        let parquet = TestParquet::create(&connection)?;
        let reader =
            ParquetMetricReader::open(&connection, ParquetSource::new(parquet.location.clone())?)?;
        let result = reader.query_aligned_metric(&aligned_query(
            AlignmentAxis::Step,
            AlignmentViewport::new(2, 4)?,
            AlignmentReduction::Full,
        ))?;

        assert_eq!(
            result
                .points
                .iter()
                .map(|point| (point.axis_value, point.point.value_f64))
                .collect::<Vec<_>>(),
            vec![(1, 1.0), (2, 0.2), (3, 1.0 / 3.0), (4, 0.25), (5, 0.2)]
        );
        assert_eq!(result.source_row_count, 5);
        assert!(result.reasons.is_empty());

        let screen = reader.query_aligned_metric(&aligned_query(
            AlignmentAxis::Step,
            AlignmentViewport::new(2, 24)?,
            AlignmentReduction::screen_budget(1, 1)?,
        ))?;
        assert_eq!(screen.source_row_count, 25);
        assert!(screen.downsampled());
        assert_eq!(screen.points.first().map(|point| point.axis_value), Some(1));
        assert_eq!(screen.points.last().map(|point| point.axis_value), Some(25));
        Ok(())
    }

    #[test]
    fn reader_reports_missing_run_start_for_elapsed_facts() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        let parquet = TestParquet::create(&connection)?;
        let reader =
            ParquetMetricReader::open(&connection, ParquetSource::new(parquet.location.clone())?)?;
        let result = MetricReader::query_aligned_metric(
            &reader,
            &aligned_query(
                AlignmentAxis::ElapsedTime,
                AlignmentViewport::new(0, 10_000)?,
                AlignmentReduction::screen_budget(100, 2)?,
            ),
        )?;

        assert!(result.points.is_empty());
        assert_eq!(result.source_row_count, 0);
        assert_eq!(result.reasons, vec![AlignmentReason::MissingRunStart]);
        Ok(())
    }

    #[test]
    fn owned_reader_queries_step_and_absolute_timestamp_facts() -> Result<(), Box<dyn Error>> {
        const EPOCH_MILLIS: i64 = 1_767_225_600_000;

        let connection = Connection::open_in_memory()?;
        let parquet = TestParquet::create(&connection)?;
        let reader = StandaloneMetricReader::open(ParquetSource::new(parquet.location.clone())?)?;

        let steps = reader.query_metric(&query(ReductionPolicy::Full)?)?;
        let timestamps = reader.query_timestamp_metric(&aligned_query(
            AlignmentAxis::Step,
            AlignmentViewport::new(EPOCH_MILLIS + 1_000, EPOCH_MILLIS + 3_000)?,
            AlignmentReduction::Full,
        ))?;
        let relative = reader.query_aligned_metric(&aligned_query(
            AlignmentAxis::ElapsedTime,
            AlignmentViewport::new(0, 10_000)?,
            AlignmentReduction::Full,
        ))?;

        assert_eq!(steps.source_row_count, 3);
        assert!(
            timestamps
                .points
                .iter()
                .all(|point| { (EPOCH_MILLIS..=EPOCH_MILLIS + 4_000).contains(&point.axis_value) })
        );
        assert_eq!(relative.reasons, [AlignmentReason::MissingRunStart]);
        Ok(())
    }

    #[test]
    fn elapsed_reader_validates_identity_before_missing_run_start() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        let parquet = TestParquet::create(&connection)?;
        let reader =
            ParquetMetricReader::open(&connection, ParquetSource::new(parquet.location.clone())?)?;
        let mut invalid = aligned_query(
            AlignmentAxis::ElapsedTime,
            AlignmentViewport::new(0, 1)?,
            AlignmentReduction::Full,
        );
        invalid.metric_key = MetricKey::from_string(" ");

        let result = reader.query_aligned_metric(&invalid);

        assert!(matches!(result, Err(StorageError::InvalidIdentity)));
        Ok(())
    }
}
