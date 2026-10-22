use crate::model::metric::{
    MetricAggregate, MetricKey, MetricPoint, MetricQuery, MetricQueryResult, ReductionPolicy, Step,
};
use crate::model::run::{RunId, RunStatus};
use chrono::{TimeZone, Utc};
use duckdb::Connection;

use crate::storage::rows::StoredMetricAggregate;
use crate::storage::sql::string_literal as sql_string_literal;
use crate::storage::{MetricReader, StorageError};

#[derive(Clone, Copy)]
pub(crate) enum MetricSource<'a> {
    Project,
    Parquet(&'a str),
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
        // The catalog decides whether the fact-table fallback is needed. In-memory
        // Run status can lag the terminal write, and a finished Run never becomes
        // running again, so this check is conservative.
        let include_running = self.running_run_count(run_ids)? > 0;
        let sql = summaries_sql(run_ids.len(), include_running);
        let mut params: Vec<&str> = Vec::with_capacity(run_ids.len() + 1);
        params.extend(run_ids.iter().map(RunId::as_str));
        if include_running {
            params.push(metric_key.as_str());
        }
        params.push(metric_key.as_str());
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            duckdb::params_from_iter(params),
            stored_metric_aggregate_from_row,
        )?;
        rows.map(|row| Ok(row?.into_metric_aggregate())).collect()
    }

    fn running_run_count(&self, run_ids: &[RunId]) -> Result<u64, StorageError> {
        let requested_rows = (0..run_ids.len())
            .map(|_| "(?)")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "WITH requested(run_id) AS (VALUES {requested_rows})
             SELECT count(*)::UBIGINT
             FROM requested JOIN seex_runs AS run USING (run_id)
             WHERE run.status = 'running'"
        );
        let params = run_ids.iter().map(RunId::as_str);
        self.connection
            .query_row(&sql, duckdb::params_from_iter(params), |row| row.get(0))
            .map_err(StorageError::from)
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
        let sql = sampled_sql(source);
        let values = query_params(source, query);
        let selected =
            crate::storage::sampling::select(&mut connection.prepare(&sql)?, &values, budget)?;
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

fn sampled_sql(source: MetricSource<'_>) -> String {
    format!(
        "{}
         SELECT step, epoch_ms(timestamp), value_f64, epoch_ms(ingested_at), step
         FROM effective WHERE isfinite(value_f64) ORDER BY step",
        effective_cte(source)
    )
}

fn summaries_sql(run_count: usize, include_running: bool) -> String {
    let requested_rows = (0..run_count)
        .map(|ordinal| format!("(?, {ordinal})"))
        .collect::<Vec<_>>()
        .join(", ");
    let running_ctes = if include_running {
        ", running AS MATERIALIZED (
             SELECT requested.ordinal, requested.run_id
             FROM requested
             JOIN seex_runs AS run USING (run_id)
             WHERE run.status = 'running'
         ),
         running_points AS (
             SELECT running.ordinal, points.run_id, points.metric_key,
                    points.step, points.value_f64
             FROM running
             JOIN dl.metric_points AS points USING (run_id)
             WHERE points.metric_key = ?
         )"
    } else {
        ""
    };
    let running_branch = if include_running {
        "UNION ALL
             SELECT ordinal, run_id, metric_key,
                    count(*) AS effective_count, max(step) AS last_step,
                    arg_max(value_f64, step) AS last_value_f64,
                    min(value_f64) AS min_value_f64,
                    max(value_f64) AS max_value_f64
             FROM running_points
             GROUP BY ordinal, run_id, metric_key"
    } else {
        ""
    };
    format!(
        "WITH requested(run_id, ordinal) AS (VALUES {requested_rows}){running_ctes}
         SELECT run_id, metric_key, effective_count, last_step, last_value_f64,
                min_value_f64, max_value_f64
         FROM (
             SELECT requested.ordinal, summary.*
             FROM requested
             JOIN seex_runs AS run USING (run_id)
             JOIN seex_metric_aggregates AS summary USING (run_id)
             WHERE run.status <> 'running' AND summary.metric_key = ?
             {running_branch}
         )
         ORDER BY ordinal"
    )
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
    fn sampled_query_excludes_non_finite_values_and_counts_finite_rows()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE SCHEMA dl;
             CREATE TABLE dl.metric_points (
                 run_id VARCHAR, metric_key VARCHAR, metric_key_encoded VARCHAR,
                 step BIGINT, timestamp TIMESTAMPTZ, value_f64 DOUBLE,
                 ingested_at TIMESTAMPTZ
             );
             INSERT INTO dl.metric_points
             SELECT 'run-1', 'loss', 'loss', step, epoch_ms(1000 + step),
                    step::DOUBLE, epoch_ms(2000 + step)
             FROM range(0, 6) AS generated(step);
             INSERT INTO dl.metric_points VALUES
                 ('run-1', 'loss', 'loss', 2, epoch_ms(1002), 42.0, epoch_ms(2002)),
                 ('run-1', 'loss', 'loss', 6, epoch_ms(1006), 'NaN'::DOUBLE, epoch_ms(2006)),
                 ('run-1', 'loss', 'loss', 7, epoch_ms(1007), 'Infinity'::DOUBLE, epoch_ms(2007));",
        )?;
        let query = MetricQuery::new(
            RunId::from_string("run-1"),
            MetricKey::from_string("loss"),
            Some(Step::new(0)),
            None,
            ReductionPolicy::screen_budget(4, 1)?,
        )?;

        let result = query_metric(&connection, MetricSource::Project, &query)?;

        assert_eq!(result.source_row_count, 7);
        assert!(result.points.len() <= 4);
        assert!(
            result
                .points
                .iter()
                .all(|point| point.value_f64.is_finite())
        );
        assert!(result.points.iter().all(|point| point.step.value() <= 5));
        Ok(())
    }
}
