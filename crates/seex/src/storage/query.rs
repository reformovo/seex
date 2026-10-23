use crate::model::metric::{MetricQuery, MetricQueryResult};
use duckdb::Connection;

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

    /// Queries one append-only metric series with its requested reduction policy.
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
}

impl MetricReader for ParquetMetricReader<'_> {
    fn query_metric(&self, query: &MetricQuery) -> Result<MetricQueryResult, StorageError> {
        Self::query_metric(self, query)
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

    /// Queries one append-only metric series.
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
}

impl MetricReader for StandaloneMetricReader {
    fn query_metric(&self, query: &MetricQuery) -> Result<MetricQueryResult, StorageError> {
        Self::query_metric(self, query)
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

    use crate::model::metric::{MetricKey, ReductionPolicy, Step};
    use crate::model::run::RunId;

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
    fn reader_queries_full_and_screen_reduced_observations() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        let parquet = TestParquet::create(&connection)?;
        let reader =
            ParquetMetricReader::open(&connection, ParquetSource::new(parquet.location.clone())?)?;

        let full = reader.query_metric(&query(ReductionPolicy::Full)?)?;
        let screen = reader.query_metric(&query(ReductionPolicy::screen_budget(1, 1)?)?)?;
        let lttb = reader.query_metric(&query(ReductionPolicy::lttb(10)?)?)?;

        assert_eq!(full.source_row_count, 4);
        assert_eq!(full.points.len(), 4);
        assert_eq!(full.points[0].step.value(), 2);
        let mut duplicate_values = full
            .points
            .iter()
            .filter(|p| p.step.value() == 2)
            .map(|p| p.value_f64)
            .collect::<Vec<_>>();
        duplicate_values.sort_by(f64::total_cmp);
        assert_eq!(duplicate_values, vec![0.2, 0.5]);
        assert!(screen.points.len() <= 4);
        assert_eq!(screen.source_row_count, 4);
        assert_eq!(lttb.points, full.points);
        Ok(())
    }

    #[test]
    fn owned_reader_queries_step_facts() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        let parquet = TestParquet::create(&connection)?;
        let reader = StandaloneMetricReader::open(ParquetSource::new(parquet.location.clone())?)?;

        let steps = reader.query_metric(&query(ReductionPolicy::Full)?)?;

        assert_eq!(steps.source_row_count, 4);
        Ok(())
    }
}
