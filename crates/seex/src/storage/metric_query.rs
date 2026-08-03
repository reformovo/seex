use std::path::Path;

use crate::model::alignment::{AlignmentAxis, AlignmentQuery, AlignmentQueryResult};
use crate::model::metric::{
    MetricAggregate, MetricKey, MetricPoint, MetricQuery, MetricQueryResult, ReductionPolicy, Step,
};
use crate::model::run::{RunId, RunStatus};
use chrono::{TimeZone, Utc};
use duckdb::Connection;

use crate::storage::alignment_query::{
    AlignmentSource, query_aligned_metric, query_narrow_step_metric, validate_alignment_identity,
};
use crate::storage::rows::StoredMetricAggregate;
use crate::storage::sql::string_literal as sql_string_literal;
use crate::storage::{MetricReader, StorageError};

const EXTREMA_PER_BUCKET: usize = 4;
const LTTB_AUTO_INSTALL_ENV: &str = "SEEX_LTTB_AUTO_INSTALL";
const LTTB_EXTENSION_PATH_ENV: &str = "SEEX_LTTB_EXTENSION_PATH";

#[derive(Clone, Copy)]
pub(crate) enum MetricSource<'a> {
    Project,
    Parquet(&'a str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeriesDiagnostics {
    pub effective_count: u64,
    pub min_step: Option<i64>,
    pub max_step: Option<i64>,
    pub has_negative_step: bool,
    pub has_decreasing_timestamp: bool,
    pub has_non_finite_value: bool,
}

/// Metric reader for the authoritative DuckLake project relation.
pub struct ProjectMetricReader<'connection> {
    connection: &'connection Connection,
}

impl<'connection> ProjectMetricReader<'connection> {
    pub const fn new(connection: &'connection Connection) -> Self {
        Self { connection }
    }

    /// Queries effective project metric points.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for query, extension, or conversion failures.
    pub fn query_metric(&self, query: &MetricQuery) -> Result<MetricQueryResult, StorageError> {
        query_metric(self.connection, MetricSource::Project, query)
    }

    #[doc(hidden)]
    pub fn series_diagnostics(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
    ) -> Result<SeriesDiagnostics, StorageError> {
        query_series_diagnostics(self.connection, MetricSource::Project, run_id, metric_key)
    }

    /// Queries project metric points on a derived comparison axis.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] for query or conversion failures.
    pub fn query_aligned_metric(
        &self,
        query: &AlignmentQuery,
    ) -> Result<AlignmentQueryResult, StorageError> {
        validate_alignment_identity(query)?;
        let run_start_millis = match query.axis {
            AlignmentAxis::Step => None,
            AlignmentAxis::ElapsedTime => Some(self.run_start_millis(&query.run_id)?),
        };
        query_aligned_metric(
            self.connection,
            AlignmentSource::Project,
            query,
            run_start_millis,
        )
    }

    #[doc(hidden)]
    pub fn query_narrow_step_metric(
        &self,
        query: &AlignmentQuery,
    ) -> Result<AlignmentQueryResult, StorageError> {
        query_narrow_step_metric(self.connection, AlignmentSource::Project, query)
    }

    fn run_start_millis(&self, run_id: &RunId) -> Result<i64, StorageError> {
        let result = self.connection.query_row(
            "SELECT epoch_ms(started_at::TIMESTAMPTZ)
             FROM seex_runs WHERE run_id = ?",
            [run_id.as_str()],
            |row| row.get(0),
        );
        match result {
            Ok(started_at) => Ok(started_at),
            Err(duckdb::Error::QueryReturnedNoRows) => Err(StorageError::RunNotFound {
                run_id: run_id.as_str().to_owned(),
            }),
            Err(source) => Err(source.into()),
        }
    }

    pub fn metric_aggregate(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
    ) -> Result<MetricAggregate, StorageError> {
        let stored = self.connection.query_row(
            "SELECT run_id, metric_key, effective_count, last_step, last_value_f64,
                    min_value_f64, max_value_f64
             FROM seex_metric_aggregates
             WHERE run_id = ? AND metric_key = ?",
            (run_id.as_str(), metric_key.as_str()),
            stored_metric_aggregate_from_row,
        )?;
        Ok(stored.into_metric_aggregate())
    }

    pub fn query_metric_summaries(
        &self,
        run_ids: &[RunId],
        metric_key: &MetricKey,
    ) -> Result<Vec<MetricAggregate>, StorageError> {
        if run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let requested_rows = (0..run_ids.len())
            .map(|ordinal| format!("(?, {ordinal})"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "WITH requested(run_id, ordinal) AS (VALUES {requested_rows})
             SELECT run_id, metric_key, effective_count, last_step, last_value_f64,
                    min_value_f64, max_value_f64
             FROM (
                 SELECT requested.ordinal, summary.*
                 FROM requested
                 JOIN seex_runs AS run USING (run_id)
                 JOIN seex_metric_aggregates AS summary USING (run_id)
                 WHERE run.status <> 'running' AND summary.metric_key = ?
                 UNION ALL
                 SELECT requested.ordinal, points.run_id, points.metric_key,
                        count(*) AS effective_count, max(points.step) AS last_step,
                        arg_max(points.value_f64, points.step) AS last_value_f64,
                        min(points.value_f64) AS min_value_f64,
                        max(points.value_f64) AS max_value_f64
                 FROM requested
                 JOIN seex_runs AS run USING (run_id)
                 JOIN (
                     SELECT *, row_number() OVER (
                         PARTITION BY run_id, metric_key, step
                         ORDER BY ingested_at DESC, rowid DESC
                     ) AS write_rank
                     FROM dl.metric_points WHERE metric_key = ?
                 ) AS points USING (run_id)
                 WHERE run.status = 'running' AND points.write_rank = 1
                 GROUP BY requested.ordinal, points.run_id, points.metric_key
             )
             ORDER BY ordinal"
        );
        let mut params: Vec<&str> = Vec::with_capacity(run_ids.len() + 2);
        params.extend(run_ids.iter().map(RunId::as_str));
        params.push(metric_key.as_str());
        params.push(metric_key.as_str());
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            duckdb::params_from_iter(params),
            stored_metric_aggregate_from_row,
        )?;
        rows.map(|row| Ok(row?.into_metric_aggregate())).collect()
    }

    pub fn list_metrics(
        &self,
        run_id: &RunId,
        run_status: RunStatus,
    ) -> Result<Vec<MetricAggregate>, StorageError> {
        let sql = match run_status {
            RunStatus::Running => {
                "WITH effective AS (
                     SELECT *, row_number() OVER (
                         PARTITION BY run_id, metric_key, step
                         ORDER BY ingested_at DESC, rowid DESC
                     ) AS write_rank
                     FROM dl.metric_points WHERE run_id = ?
                 )
                 SELECT run_id, metric_key, count(*) AS effective_count,
                        max(step) AS last_step,
                        arg_max(value_f64, step) AS last_value_f64,
                        min(value_f64) AS min_value_f64,
                        max(value_f64) AS max_value_f64
                 FROM effective WHERE write_rank = 1
                 GROUP BY run_id, metric_key ORDER BY metric_key"
            }
            RunStatus::Finished | RunStatus::Failed => {
                "SELECT run_id, metric_key, effective_count, last_step, last_value_f64,
                        min_value_f64, max_value_f64
                 FROM seex_metric_aggregates
                 WHERE run_id = ? ORDER BY metric_key"
            }
        };
        let mut statement = self.connection.prepare(sql)?;
        let rows = statement.query_map([run_id.as_str()], stored_metric_aggregate_from_row)?;
        rows.map(|row| Ok(row?.into_metric_aggregate())).collect()
    }
}

impl MetricReader for ProjectMetricReader<'_> {
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

pub(crate) fn query_series_diagnostics(
    connection: &Connection,
    source: MetricSource<'_>,
    run_id: &RunId,
    metric_key: &MetricKey,
) -> Result<SeriesDiagnostics, StorageError> {
    if run_id.as_str().trim().is_empty() || metric_key.as_str().trim().is_empty() {
        return Err(StorageError::InvalidIdentity);
    }
    let (relation, tie_breaker) = source_relation(source);
    let sql = format!(
        "WITH ranked AS (
             SELECT step, timestamp, value_f64,
                    row_number() OVER (
                        PARTITION BY step ORDER BY ingested_at DESC, {tie_breaker}
                    ) AS write_rank
             FROM {relation}
             WHERE run_id = ? AND metric_key = ? AND metric_key_encoded = ?
         ),
         effective AS (
             SELECT step, timestamp, value_f64 FROM ranked WHERE write_rank = 1
         ),
         ordered AS (
             SELECT *, lag(timestamp) OVER (ORDER BY step) AS previous_timestamp
             FROM effective
         )
         SELECT count(*)::UBIGINT, min(step), max(step),
                coalesce(bool_or(step < 0), false),
                coalesce(bool_or(previous_timestamp > timestamp), false),
                coalesce(bool_or(NOT isfinite(value_f64)), false)
         FROM ordered"
    );
    let mut values: Vec<Box<dyn duckdb::ToSql>> = Vec::with_capacity(4);
    if let MetricSource::Parquet(location) = source {
        values.push(Box::new(location.to_owned()));
    }
    values.extend([
        Box::new(run_id.as_str().to_owned()) as Box<dyn duckdb::ToSql>,
        Box::new(metric_key.as_str().to_owned()),
        Box::new(percent_encode_metric_key(metric_key.as_str())),
    ]);
    connection
        .query_row(
            &sql,
            duckdb::params_from_iter(values.iter().map(|value| value.as_ref())),
            |row| {
                Ok(SeriesDiagnostics {
                    effective_count: row.get(0)?,
                    min_step: row.get(1)?,
                    max_step: row.get(2)?,
                    has_negative_step: row.get(3)?,
                    has_decreasing_timestamp: row.get(4)?,
                    has_non_finite_value: row.get(5)?,
                })
            },
        )
        .map_err(StorageError::from)
}

pub(crate) fn query_metric(
    connection: &Connection,
    source: MetricSource<'_>,
    query: &MetricQuery,
) -> Result<MetricQueryResult, StorageError> {
    if query.run_id.as_str().trim().is_empty() || query.metric_key.as_str().trim().is_empty() {
        return Err(StorageError::InvalidIdentity);
    }

    match query.reduction {
        ReductionPolicy::Full => execute_query(connection, source, query, QueryShape::Full),
        ReductionPolicy::ScreenBudget { .. } => {
            let max_points = query
                .reduction
                .max_points()
                .expect("screen budgets always have a point limit");
            let bucket_count = (max_points / EXTREMA_PER_BUCKET).max(1);
            execute_query(
                connection,
                source,
                query,
                QueryShape::Screen { bucket_count },
            )
        }
        ReductionPolicy::Lttb { max_points } => {
            let source_row_count = count_effective(connection, source, query)?;
            if source_row_count <= max_points as u64 {
                return execute_query(connection, source, query, QueryShape::Full);
            }
            let max_points = i64::try_from(max_points)
                .map_err(|_| StorageError::QueryMaxPointsTooLarge { max_points })?;
            ensure_lttb_extension_loaded(connection)?;
            execute_query(connection, source, query, QueryShape::Lttb { max_points })
        }
    }
}

enum QueryShape {
    Full,
    Screen { bucket_count: usize },
    Lttb { max_points: i64 },
}

fn execute_query(
    connection: &Connection,
    source: MetricSource<'_>,
    query: &MetricQuery,
    shape: QueryShape,
) -> Result<MetricQueryResult, StorageError> {
    let sql = match shape {
        QueryShape::Full => full_query_sql(source),
        QueryShape::Screen { .. } => screen_query_sql(source),
        QueryShape::Lttb { .. } => lttb_query_sql(source),
    };
    let mut values = query_params(source, query);
    match shape {
        QueryShape::Full => {}
        QueryShape::Screen { bucket_count } => {
            values.push(Box::new(i64::try_from(bucket_count).map_err(|_| {
                StorageError::QueryMaxPointsTooLarge {
                    max_points: bucket_count.saturating_mul(EXTREMA_PER_BUCKET),
                }
            })?));
        }
        QueryShape::Lttb { max_points } => values.push(Box::new(max_points)),
    }

    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        duckdb::params_from_iter(values.iter().map(|value| value.as_ref())),
        stored_point_from_row,
    )?;
    let mut points = Vec::new();
    let mut source_row_count = 0;
    for row in rows {
        let (point, row_count) = row?;
        points.push(point.into_metric_point()?);
        source_row_count = row_count;
    }
    Ok(MetricQueryResult {
        points,
        source_row_count,
    })
}

fn count_effective(
    connection: &Connection,
    source: MetricSource<'_>,
    query: &MetricQuery,
) -> Result<u64, StorageError> {
    let sql = format!(
        "{} SELECT count(*)::UBIGINT FROM effective",
        effective_cte(source)
    );
    let values = query_params(source, query);
    connection
        .query_row(
            &sql,
            duckdb::params_from_iter(values.iter().map(|value| value.as_ref())),
            |row| row.get(0),
        )
        .map_err(StorageError::from)
}

fn query_params(source: MetricSource<'_>, query: &MetricQuery) -> Vec<Box<dyn duckdb::ToSql>> {
    let mut values: Vec<Box<dyn duckdb::ToSql>> = Vec::with_capacity(9);
    if let MetricSource::Parquet(location) = source {
        values.push(Box::new(location.to_owned()));
    }
    values.extend([
        Box::new(query.run_id.as_str().to_owned()) as Box<dyn duckdb::ToSql>,
        Box::new(query.metric_key.as_str().to_owned()),
        Box::new(percent_encode_metric_key(query.metric_key.as_str())),
        Box::new(query.start_step.map(Step::value)),
        Box::new(query.start_step.map(Step::value)),
        Box::new(query.end_step.map(Step::value)),
        Box::new(query.end_step.map(Step::value)),
    ]);
    values
}

fn effective_cte(source: MetricSource<'_>) -> String {
    let (relation, tie_breaker) = source_relation(source);
    format!(
        "WITH ranked AS (
             SELECT run_id, metric_key, step, timestamp, value_f64, ingested_at,
                    row_number() OVER (
                        PARTITION BY run_id, metric_key, step
                        ORDER BY ingested_at DESC, {tie_breaker}
                    ) AS write_rank
             FROM {relation}
             WHERE run_id = ? AND metric_key = ? AND metric_key_encoded = ?
               AND (? IS NULL OR step >= ?)
               AND (? IS NULL OR step < ?)
         ),
         effective AS (
             SELECT run_id, metric_key, step, timestamp, value_f64, ingested_at
             FROM ranked WHERE write_rank = 1
         )"
    )
}

fn source_relation(source: MetricSource<'_>) -> (&'static str, &'static str) {
    match source {
        MetricSource::Project => ("dl.metric_points", "rowid DESC"),
        MetricSource::Parquet(_) => (
            "read_parquet(?, hive_partitioning = true, union_by_name = true, \
             filename = true, file_row_number = true)",
            "filename DESC, file_row_number DESC",
        ),
    }
}

fn full_query_sql(source: MetricSource<'_>) -> String {
    format!(
        "{}
         SELECT run_id, metric_key, step, epoch_ms(timestamp), value_f64,
                epoch_ms(ingested_at), count(*) OVER ()::UBIGINT
         FROM effective ORDER BY step",
        effective_cte(source)
    )
}

fn screen_query_sql(source: MetricSource<'_>) -> String {
    format!(
        "{},
         numbered AS (
             SELECT *, row_number() OVER (ORDER BY step) - 1 AS ordinal,
                    count(*) OVER ()::UBIGINT AS source_row_count
             FROM effective
         ),
         bucketed AS (
             SELECT *, floor(
                 ordinal::DOUBLE * ?::DOUBLE / source_row_count::DOUBLE
             )::UBIGINT AS bucket
             FROM numbered
         ),
         candidates AS (
             SELECT *,
                    row_number() OVER (PARTITION BY bucket ORDER BY step) AS first_rank,
                    row_number() OVER (PARTITION BY bucket ORDER BY step DESC) AS last_rank,
                    row_number() OVER (PARTITION BY bucket ORDER BY value_f64, step) AS min_rank,
                    row_number() OVER (PARTITION BY bucket ORDER BY value_f64 DESC, step) AS max_rank
             FROM bucketed
         )
         SELECT run_id, metric_key, step, epoch_ms(timestamp), value_f64,
                epoch_ms(ingested_at), source_row_count
         FROM candidates
         WHERE first_rank = 1 OR last_rank = 1 OR min_rank = 1 OR max_rank = 1
         ORDER BY step",
        effective_cte(source)
    )
}

fn lttb_query_sql(source: MetricSource<'_>) -> String {
    format!(
        "{},
         normalized AS (
             SELECT *, CAST(step AS HUGEINT) -
                       min(CAST(step AS HUGEINT)) OVER () AS lttb_step
             FROM effective
         ),
         sampled AS (
             SELECT unnest(lttb(lttb_step, value_f64, ?)) AS point,
                    count(*)::UBIGINT AS source_row_count
             FROM normalized
         )
         SELECT normalized.run_id, normalized.metric_key, normalized.step,
                epoch_ms(normalized.timestamp), normalized.value_f64,
                epoch_ms(normalized.ingested_at), sampled.source_row_count
         FROM sampled
         JOIN normalized ON normalized.lttb_step = sampled.point.x
         ORDER BY normalized.step",
        effective_cte(source)
    )
}

struct StoredPoint {
    run_id: String,
    metric_key: String,
    step: i64,
    timestamp_millis: i64,
    value_f64: f64,
    ingested_at_millis: i64,
}

impl StoredPoint {
    fn into_metric_point(self) -> Result<MetricPoint, StorageError> {
        let timestamp = timestamp_from_millis("timestamp", self.timestamp_millis)?;
        let ingested_at = timestamp_from_millis("ingested_at", self.ingested_at_millis)?;
        Ok(MetricPoint {
            run_id: RunId::from_string(self.run_id),
            metric_key: MetricKey::from_string(self.metric_key),
            step: Step::new(self.step),
            timestamp,
            value_f64: self.value_f64,
            ingested_at,
        })
    }
}

fn stored_point_from_row(row: &duckdb::Row<'_>) -> duckdb::Result<(StoredPoint, u64)> {
    Ok((
        StoredPoint {
            run_id: row.get(0)?,
            metric_key: row.get(1)?,
            step: row.get(2)?,
            timestamp_millis: row.get(3)?,
            value_f64: row.get(4)?,
            ingested_at_millis: row.get(5)?,
        },
        row.get(6)?,
    ))
}

fn stored_metric_aggregate_from_row(
    row: &duckdb::Row<'_>,
) -> duckdb::Result<StoredMetricAggregate> {
    Ok(StoredMetricAggregate {
        run_id: row.get(0)?,
        metric_key: row.get(1)?,
        effective_count: row.get(2)?,
        last_step: row.get(3)?,
        last_value_f64: row.get(4)?,
        min_value_f64: row.get(5)?,
        max_value_f64: row.get(6)?,
    })
}

fn timestamp_from_millis(
    field: &'static str,
    millis: i64,
) -> Result<chrono::DateTime<Utc>, StorageError> {
    Utc.timestamp_millis_opt(millis)
        .single()
        .ok_or(StorageError::InvalidTimestamp { field, millis })
}

pub fn percent_encode_metric_key(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'~' | b'-' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn ensure_lttb_extension_loaded(connection: &Connection) -> Result<(), StorageError> {
    let (loaded, installed) = lttb_extension_state(connection);
    if loaded {
        return Ok(());
    }
    if let Some(path) = std::env::var_os(LTTB_EXTENSION_PATH_ENV) {
        return load_lttb_extension_from_path(connection, Path::new(&path));
    }

    let load_error = match installed {
        true => match connection.execute_batch("LOAD lttb;") {
            Ok(()) if lttb_function_available(connection) => return Ok(()),
            Ok(()) => None,
            Err(source) => Some(source.to_string()),
        },
        false => None,
    };
    if lttb_auto_install_allowed(std::env::var_os(LTTB_AUTO_INSTALL_ENV).as_deref()) {
        return install_and_load_lttb_extension(connection);
    }

    let mut message = format!(
        "lttb is not loaded and Seex will not download it automatically; \
         set {LTTB_AUTO_INSTALL_ENV}=1 to allow INSTALL lttb FROM community, \
         or set {LTTB_EXTENSION_PATH_ENV} to a local lttb.duckdb_extension"
    );
    if let Some(load_error) = load_error {
        message.push_str("; LOAD lttb failed: ");
        message.push_str(&load_error);
    }
    Err(StorageError::LttbExtensionUnavailable { message })
}

fn install_and_load_lttb_extension(connection: &Connection) -> Result<(), StorageError> {
    match connection.execute_batch("INSTALL lttb FROM community; LOAD lttb;") {
        Ok(()) if lttb_function_available(connection) => Ok(()),
        Ok(()) => Err(StorageError::LttbExtensionUnavailable {
            message: "INSTALL/LOAD lttb did not register lttb".to_owned(),
        }),
        Err(source) => Err(StorageError::LttbExtensionUnavailable {
            message: source.to_string(),
        }),
    }
}

fn load_lttb_extension_from_path(connection: &Connection, path: &Path) -> Result<(), StorageError> {
    let path = sql_string_literal(path.to_string_lossy().as_ref());
    match connection.execute_batch(&format!("LOAD {path};")) {
        Ok(()) if lttb_function_available(connection) => Ok(()),
        Ok(()) => Err(StorageError::LttbExtensionUnavailable {
            message: "LOAD lttb from SEEX_LTTB_EXTENSION_PATH did not register lttb".to_owned(),
        }),
        Err(source) => Err(StorageError::LttbExtensionUnavailable {
            message: source.to_string(),
        }),
    }
}

fn lttb_function_available(connection: &Connection) -> bool {
    connection
        .query_row(
            "SELECT count(*) FROM (SELECT lttb(1::BIGINT, 1::DOUBLE, 1::BIGINT))",
            [],
            |row| row.get::<_, i64>(0),
        )
        .is_ok()
}

fn lttb_extension_state(connection: &Connection) -> (bool, bool) {
    connection
        .query_row(
            "SELECT loaded, installed
             FROM duckdb_extensions()
             WHERE extension_name = 'lttb'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((false, false))
}

fn lttb_auto_install_allowed(value: Option<&std::ffi::OsStr>) -> bool {
    value
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|value| matches!(value, "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn lttb_auto_install_is_opt_in() {
        assert!(!lttb_auto_install_allowed(None));
        assert!(!lttb_auto_install_allowed(Some(OsStr::new("0"))));
        assert!(lttb_auto_install_allowed(Some(OsStr::new("true"))));
    }

    #[test]
    fn lttb_install_check_does_not_download_the_extension() -> Result<(), Box<dyn std::error::Error>>
    {
        let home = tempfile::tempdir()?;
        let config =
            duckdb::Config::default().with("home_directory", home.path().to_string_lossy())?;
        let connection = Connection::open_in_memory_with_flags(config)?;

        assert_eq!(lttb_extension_state(&connection), (false, false));
        assert!(!home.path().join(".duckdb/extensions").exists());
        Ok(())
    }

    #[test]
    #[ignore = "network-dependent release LTTB acceptance"]
    fn lttb_online_and_explicit_offline_paths() -> Result<(), Box<dyn std::error::Error>> {
        let online_home = tempfile::tempdir()?;
        let online_config = duckdb::Config::default()
            .with("home_directory", online_home.path().to_string_lossy())?;
        let online = Connection::open_in_memory_with_flags(online_config)?;
        install_and_load_lttb_extension(&online)?;
        let extension_path: String = online.query_row(
            "SELECT install_path FROM duckdb_extensions() WHERE extension_name = 'lttb'",
            [],
            |row| row.get(0),
        )?;
        assert!(Path::new(&extension_path).is_file());

        let offline_home = tempfile::tempdir()?;
        let offline_config = duckdb::Config::default()
            .with("home_directory", offline_home.path().to_string_lossy())?
            .allow_unsigned_extensions()?;
        let offline = Connection::open_in_memory_with_flags(offline_config)?;
        load_lttb_extension_from_path(&offline, Path::new(&extension_path))?;

        assert_eq!(lttb_extension_state(&offline), (true, false));
        Ok(())
    }

    #[test]
    fn diagnostics_cover_the_whole_effective_series_after_lww()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE SCHEMA dl;
             CREATE TABLE dl.metric_points (
                 run_id VARCHAR, metric_key VARCHAR, metric_key_encoded VARCHAR,
                 step BIGINT, timestamp TIMESTAMPTZ, value_f64 DOUBLE,
                 ingested_at TIMESTAMPTZ
             );
             INSERT INTO dl.metric_points VALUES
                 ('run-1', 'loss', 'loss', -1, epoch_ms(30), 1.0, epoch_ms(1)),
                 ('run-1', 'loss', 'loss', 0, epoch_ms(25), 2.0, epoch_ms(1)),
                 ('run-1', 'loss', 'loss', 0, epoch_ms(20), 'NaN'::DOUBLE, epoch_ms(2));",
        )?;

        let diagnostics = query_series_diagnostics(
            &connection,
            MetricSource::Project,
            &RunId::from_string("run-1"),
            &MetricKey::from_string("loss"),
        )?;

        assert_eq!(diagnostics.effective_count, 2);
        assert_eq!(diagnostics.min_step, Some(-1));
        assert_eq!(diagnostics.max_step, Some(0));
        assert!(diagnostics.has_negative_step);
        assert!(diagnostics.has_decreasing_timestamp);
        assert!(diagnostics.has_non_finite_value);
        Ok(())
    }

    #[test]
    fn project_and_parquet_sources_share_effective_query_shape() {
        let project = full_query_sql(MetricSource::Project);
        let parquet = full_query_sql(MetricSource::Parquet("points.parquet"));

        assert!(project.contains("FROM dl.metric_points"));
        assert!(parquet.contains("FROM read_parquet"));
        assert!(project.contains("metric_key_encoded = ?"));
        assert!(parquet.contains("metric_key_encoded = ?"));
    }
}
