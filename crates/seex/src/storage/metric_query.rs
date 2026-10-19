use crate::model::alignment::{AlignmentAxis, AlignmentQuery, AlignmentQueryResult};
use crate::model::metric::{
    MetricAggregate, MetricKey, MetricPoint, MetricQuery, MetricQueryResult, ReductionPolicy, Step,
};
use crate::model::run::{RunId, RunStatus};
use chrono::{TimeZone, Utc};
use duckdb::Connection;

use crate::storage::alignment_query::{
    AlignmentSource, query_aligned_metric, query_bounded_full_aligned_metric,
    query_narrow_step_metric, validate_alignment_identity,
};
use crate::storage::rows::StoredMetricAggregate;
use crate::storage::sql::string_literal as sql_string_literal;
use crate::storage::{MetricReader, StorageError};

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

    /// Queries append-only project metric observations.
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

    pub(crate) fn query_bounded_full_aligned_metric(
        &self,
        query: &AlignmentQuery,
        source_row_count: u64,
    ) -> Result<AlignmentQueryResult, StorageError> {
        validate_alignment_identity(query)?;
        let run_start_millis = match query.axis {
            AlignmentAxis::Step => None,
            AlignmentAxis::ElapsedTime => Some(self.run_start_millis(&query.run_id)?),
        };
        query_bounded_full_aligned_metric(
            self.connection,
            AlignmentSource::Project,
            query,
            run_start_millis,
            source_row_count,
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
                 JOIN dl.metric_points AS points USING (run_id)
                 WHERE run.status = 'running' AND points.metric_key = ?
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
                "SELECT run_id, metric_key, count(*) AS effective_count,
                        max(step) AS last_step,
                        arg_max(value_f64, step) AS last_value_f64,
                        min(value_f64) AS min_value_f64,
                        max(value_f64) AS max_value_f64
                 FROM dl.metric_points WHERE run_id = ?
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
    let relation = source_relation(source);
    let sql = format!(
        "WITH ordered AS (
             SELECT step, timestamp, value_f64,
                    lag(timestamp) OVER (ORDER BY step, timestamp) AS previous_timestamp
             FROM {relation}
             WHERE run_id = ? AND metric_key = ? AND metric_key_encoded = ?
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

    if let Some(budget) = query.reduction.max_points() {
        let sql = format!(
            "{}, source_stats AS (
                 SELECT count(*)::UBIGINT AS total,
                        count(*) FILTER (WHERE isfinite(value_f64))::UBIGINT AS finite_count
                 FROM effective
             )
             SELECT points.step, epoch_ms(points.timestamp), points.value_f64,
                    epoch_ms(points.ingested_at), points.step,
                    source_stats.total, source_stats.finite_count, 0::UBIGINT
             FROM source_stats LEFT JOIN effective AS points ON true ORDER BY points.step",
            effective_cte(source)
        );
        let values = query_params(source, query);
        let selected = crate::storage::sampling::select(
            &mut connection.prepare(&sql)?,
            &values,
            budget,
            None,
        )?;
        let points = selected
            .points
            .into_iter()
            .map(|p| {
                StoredPoint {
                    run_id: query.run_id.as_str().to_owned(),
                    metric_key: query.metric_key.as_str().to_owned(),
                    step: p.step,
                    timestamp_millis: p.timestamp_millis,
                    value_f64: p.value_f64,
                    ingested_at_millis: p.ingested_at_millis,
                }
                .into_metric_point()
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(MetricQueryResult {
            points,
            source_row_count: selected.source_count,
        });
    }
    let mut statement = connection.prepare(&full_query_sql(source))?;
    let values = query_params(source, query);
    let mut points = Vec::new();
    let mut source_row_count = 0;
    for row in statement.query_map(
        duckdb::params_from_iter(values.iter().map(|v| v.as_ref())),
        stored_point_from_row,
    )? {
        let (point, count) = row?;
        points.push(point.into_metric_point()?);
        source_row_count = count;
    }
    Ok(MetricQueryResult {
        points,
        source_row_count,
    })
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
    let relation = source_relation(source);
    format!(
        "WITH effective AS (
             SELECT run_id, metric_key, step, timestamp, value_f64, ingested_at
             FROM {relation}
             WHERE run_id = ? AND metric_key = ? AND metric_key_encoded = ?
               AND (? IS NULL OR step >= ?)
               AND (? IS NULL OR step < ?)
         )"
    )
}

fn source_relation(source: MetricSource<'_>) -> &'static str {
    match source {
        MetricSource::Project => "dl.metric_points",
        MetricSource::Parquet(_) => {
            "read_parquet(?, hive_partitioning = true, union_by_name = true)"
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_cover_all_observations() -> Result<(), Box<dyn std::error::Error>> {
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

        assert_eq!(diagnostics.effective_count, 3);
        assert_eq!(diagnostics.min_step, Some(-1));
        assert_eq!(diagnostics.max_step, Some(0));
        assert!(diagnostics.has_negative_step);
        assert!(diagnostics.has_decreasing_timestamp);
        assert!(diagnostics.has_non_finite_value);
        let reader = ProjectMetricReader::new(&connection);
        let bounded = MetricQuery::new(
            RunId::from_string("run-1"),
            MetricKey::from_string("loss"),
            Some(Step::new(0)),
            Some(Step::new(1)),
            ReductionPolicy::Full,
        )?;
        let result = reader.query_metric(&bounded)?;
        assert_eq!(result.source_row_count, 2);
        assert_eq!(result.points.len(), 2);
        assert!(result.points.iter().all(|p| p.step.value() == 0));
        assert!(result.points.iter().any(|p| p.value_f64 == 2.0));
        assert!(result.points.iter().any(|p| p.value_f64.is_nan()));
        Ok(())
    }
}
