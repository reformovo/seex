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

    fn sum_total_files(plan: &str) -> u64 {
        let mut sum = 0;
        let mut rest = plan;
        while let Some(position) = rest.find("\"Total Files Read\"") {
            rest = &rest[position + "\"Total Files Read\"".len()..];
            let Some(colon) = rest.find(':') else { break };
            let after = rest[colon + 1..].trim_start();
            let digits: String = after
                .trim_start_matches('"')
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();
            sum += digits.parse::<u64>().unwrap_or(0);
            rest = after;
        }
        sum
    }

    fn explain(
        connection: &Connection,
        sql: &str,
        values: &[Box<dyn duckdb::ToSql>],
    ) -> Result<String, Box<dyn std::error::Error>> {
        let (key, plan): (String, String) = connection.query_row(
            &format!("EXPLAIN (ANALYZE, FORMAT JSON) {sql}"),
            duckdb::params_from_iter(values.iter().map(|value| value.as_ref())),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(key, "analyzed_plan");
        Ok(plan)
    }

    #[test]
    #[ignore = "read-only production SQL plans; release and local fixture required"]
    fn production_query_plans() -> Result<(), Box<dyn std::error::Error>> {
        if cfg!(debug_assertions) {
            return Err("requires --release".into());
        }
        let root = std::path::PathBuf::from(std::env::var("SEEX_PANEL_ROOT")?).canonicalize()?;
        assert!(root.ends_with(".tmp/v5-panel-million/store"));
        let output = std::path::PathBuf::from(std::env::var("SEEX_STORAGE_PROFILE_OUT")?);
        assert_eq!(
            output.parent().unwrap().canonicalize()?,
            root.parent().and_then(|path| path.parent()).unwrap()
        );
        assert!(
            output
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("v5-")
        );
        std::fs::create_dir(&output)?; // Refuse overwriting old evidence.
        let config = crate::storage::bootstrap::NativeStorageConfig::with_backend(
            crate::storage::bootstrap::CatalogBackend::Sqlite,
            &root,
            None,
            None,
        );
        let connection =
            crate::storage::bootstrap::open_existing_native_connection_with_config(config)?;
        let reader = ProjectMetricReader::new(&connection);
        let run_id = RunId::from_string("run-0");
        let metric_key = MetricKey::from_string("loss");

        for (name, start, end, rows, files) in [
            (
                "detail-prefix",
                Some(Step::new(0)),
                Some(Step::new(2049)),
                2049,
                1,
            ),
            (
                "detail-middle",
                Some(Step::new(450_000)),
                Some(Step::new(550_001)),
                100_002,
                8,
            ),
            (
                "detail-outside",
                Some(Step::new(2_000_000)),
                Some(Step::new(3_000_001)),
                0,
                0,
            ),
            ("overview", Some(Step::new(0)), None, 1_000_001, 62),
        ] {
            let query = MetricQuery::new(
                run_id.clone(),
                metric_key.clone(),
                start,
                end,
                ReductionPolicy::screen_budget(1024, 1)?,
            )?;
            let result = reader.query_metric(&query)?;
            assert_eq!(result.source_row_count, rows, "{name} source count");
            assert_eq!(result.points.len() as u64, rows.min(1024), "{name} points");
            let sql = sampled_sql(MetricSource::Project);
            let values = query_params(MetricSource::Project, &query);
            let plan = explain(&connection, &sql, &values)?;
            assert_eq!(sum_total_files(&plan), files, "{name} files");
            std::fs::write(output.join(format!("{name}.json")), plan)?;
            std::fs::write(output.join(format!("{name}.sql")), sql)?;
        }

        let run_ids = (0..16)
            .map(|index| RunId::from_string(format!("run-{index}")))
            .collect::<Vec<_>>();
        let summaries = reader.query_metric_summaries(&run_ids, &metric_key)?;
        assert_eq!(summaries.len(), 16);
        assert_eq!(summaries[0].effective_count, 1_000_001);
        // The fixture has no running Run, so the production query omits the
        // fact-table fallback entirely.
        let sql = summaries_sql(run_ids.len(), false);
        let values: Vec<Box<dyn duckdb::ToSql>> = run_ids
            .iter()
            .map(|id| Box::new(id.as_str().to_owned()) as Box<dyn duckdb::ToSql>)
            .chain(std::iter::once(Box::new("loss") as Box<dyn duckdb::ToSql>))
            .collect();
        let plan = explain(&connection, &sql, &values)?;
        assert_eq!(
            sum_total_files(&plan),
            0,
            "finished-only summaries must not scan facts"
        );
        std::fs::write(output.join("summaries.json"), plan)?;
        std::fs::write(output.join("summaries.sql"), sql)?;
        println!(
            "SEEX_PRODUCTION_PLANS summaries_files=0 summaries_ok={}",
            summaries.len()
        );
        Ok(())
    }

    #[test]
    #[ignore = "storage-boundary per-statement profile; release and local fixture required"]
    fn sampling_stage_profile() -> Result<(), Box<dyn std::error::Error>> {
        fn median(values: &mut [f64]) -> f64 {
            values.sort_by(f64::total_cmp);
            let middle = values.len() / 2;
            if values.len().is_multiple_of(2) {
                (values[middle - 1] + values[middle]) / 2.0
            } else {
                values[middle]
            }
        }

        if cfg!(debug_assertions) {
            return Err("requires --release".into());
        }
        let root = std::path::PathBuf::from(std::env::var("SEEX_PANEL_ROOT")?).canonicalize()?;
        assert!(root.ends_with(".tmp/v5-panel-million/store"));
        let output = std::path::PathBuf::from(std::env::var("SEEX_STORAGE_PROFILE_OUT")?);
        assert_eq!(
            output.parent().unwrap().canonicalize()?,
            root.parent().and_then(|path| path.parent()).unwrap()
        );
        assert!(
            output
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("v5-")
        );
        std::fs::create_dir(&output)?; // Refuse overwriting old evidence.
        let config = crate::storage::bootstrap::NativeStorageConfig::with_backend(
            crate::storage::bootstrap::CatalogBackend::Sqlite,
            &root,
            None,
            None,
        );
        let connection =
            crate::storage::bootstrap::open_existing_native_connection_with_config(config)?;
        let metric_key = MetricKey::from_string("loss");
        let runs = (0..16)
            .map(|index| RunId::from_string(format!("run-{index}")))
            .collect::<Vec<_>>();
        let budget = 1024;
        let mut csv = String::from(
            "range,scope,rows,prepare_ns,execute_ns,fetch_ns,arrow_ns,rows_ns,finish_ns,consume_ns\n",
        );
        for (name, start, end, expected_rows) in [
            (
                "prefix",
                Some(Step::new(0)),
                Some(Step::new(2049)),
                2049_u64,
            ),
            (
                "middle",
                Some(Step::new(450_000)),
                Some(Step::new(550_001)),
                100_002,
            ),
            (
                "outside",
                Some(Step::new(2_000_000)),
                Some(Step::new(3_000_001)),
                0,
            ),
            ("overview", Some(Step::new(0)), None, 1_000_001),
        ] {
            let mut first_prepare = Vec::new();
            let mut first_execute = Vec::new();
            let mut first_fetch = Vec::new();
            let mut first_arrow = Vec::new();
            let mut first_rows = Vec::new();
            let mut first_finish = Vec::new();
            let mut first_consume = Vec::new();
            let mut prepare_sum = 0.0;
            let mut execute_sum = 0.0;
            let mut fetch_sum = 0.0;
            let mut arrow_sum = 0.0;
            let mut rows_time_sum = 0.0;
            let mut finish_sum = 0.0;
            let mut consume_sum = 0.0;
            let mut rows_sum = 0;
            for run in &runs {
                let query = MetricQuery::new(
                    run.clone(),
                    metric_key.clone(),
                    start,
                    end,
                    ReductionPolicy::screen_budget(1024, 1)?,
                )?;
                let sql = sampled_sql(MetricSource::Project);
                let values = query_params(MetricSource::Project, &query);
                let mut prepares = Vec::new();
                let mut executes = Vec::new();
                let mut fetches = Vec::new();
                let mut arrows = Vec::new();
                let mut row_times = Vec::new();
                let mut finishes = Vec::new();
                let mut consumes = Vec::new();
                let mut rows = 0;
                for _ in 0..10 {
                    let started = std::time::Instant::now();
                    let mut statement = connection.prepare(&sql)?;
                    prepares.push(started.elapsed().as_nanos() as f64);
                    let (selected, timings) =
                        crate::storage::sampling::select_timed(&mut statement, &values, budget)?;
                    executes.push(timings.execute.as_nanos() as f64);
                    fetches.push(timings.fetch.as_nanos() as f64);
                    arrows.push(timings.arrow.as_nanos() as f64);
                    row_times.push(timings.rows.as_nanos() as f64);
                    finishes.push(timings.finish.as_nanos() as f64);
                    consumes.push(timings.consume.as_nanos() as f64);
                    rows = selected.source_count;
                }
                assert_eq!(rows, expected_rows, "{name} source count");
                let prepare = median(&mut prepares);
                let execute = median(&mut executes);
                let fetch = median(&mut fetches);
                let arrow = median(&mut arrows);
                let rows_time = median(&mut row_times);
                let finish = median(&mut finishes);
                let consume = median(&mut consumes);
                if run == &runs[0] {
                    first_prepare.push(prepare);
                    first_execute.push(execute);
                    first_fetch.push(fetch);
                    first_arrow.push(arrow);
                    first_rows.push(rows_time);
                    first_finish.push(finish);
                    first_consume.push(consume);
                }
                prepare_sum += prepare;
                execute_sum += execute;
                fetch_sum += fetch;
                arrow_sum += arrow;
                rows_time_sum += rows_time;
                finish_sum += finish;
                consume_sum += consume;
                rows_sum += rows;
                csv.push_str(&format!(
                    "{name},{},{rows},{prepare:.0},{execute:.0},{fetch:.0},{arrow:.0},{rows_time:.0},{finish:.0},{consume:.0}\n",
                    run.as_str()
                ));
            }
            csv.push_str(&format!(
                "{name},16-curves,{rows_sum},{prepare_sum:.0},{execute_sum:.0},{fetch_sum:.0},{arrow_sum:.0},{rows_time_sum:.0},{finish_sum:.0},{consume_sum:.0}\n"
            ));
            println!(
                "SEEX_STAGE_PROFILE range={name} scope=one rows={expected_rows} prepare_ns={:.0} execute_ns={:.0} fetch_ns={:.0} arrow_ns={:.0} rows_ns={:.0} finish_ns={:.0} consume_ns={:.0}",
                first_prepare[0],
                first_execute[0],
                first_fetch[0],
                first_arrow[0],
                first_rows[0],
                first_finish[0],
                first_consume[0]
            );
            println!(
                "SEEX_STAGE_PROFILE range={name} scope=sixteen rows={rows_sum} prepare_ns={prepare_sum:.0} execute_ns={execute_sum:.0} fetch_ns={fetch_sum:.0} arrow_ns={arrow_sum:.0} rows_ns={rows_time_sum:.0} finish_ns={finish_sum:.0} consume_ns={consume_sum:.0}"
            );
        }

        // Isolate reducer cost from Arrow conversion and payload extraction.
        let mut lttb_csv = String::from("scope,rows,push_ns,finish_ns\n");
        for (name, count) in [
            ("prefix", 2_049_usize),
            ("middle", 100_002),
            ("overview", 1_000_001),
        ] {
            let mut pushes = Vec::new();
            let mut finishes = Vec::new();
            for _ in 0..10 {
                let mut reducer = crate::storage::lttb::OrderedLttb::new(count, budget);
                let started = std::time::Instant::now();
                for index in 0..count {
                    reducer
                        .push(crate::storage::sampling::Observation {
                            step: index as i64,
                            timestamp_millis: index as i64,
                            value_f64: (index % 97) as f64,
                            ingested_at_millis: index as i64,
                            axis_value: index as i64,
                        })
                        .map_err(|error| format!("{error:?}"))?;
                }
                pushes.push(started.elapsed().as_nanos() as f64);
                let started = std::time::Instant::now();
                let points = reducer.finish().map_err(|error| format!("{error:?}"))?;
                finishes.push(started.elapsed().as_nanos() as f64);
                assert_eq!(points.len(), count.min(budget));
            }
            let push = median(&mut pushes);
            let finish = median(&mut finishes);
            lttb_csv.push_str(&format!("{name},{count},{push:.0},{finish:.0}\n"));
            println!(
                "SEEX_STAGE_PROFILE reducer={name} rows={count} push_ns={push:.0} finish_ns={finish:.0}"
            );
        }
        std::fs::write(output.join("sampling-stages.csv"), csv)?;
        std::fs::write(output.join("lttb-stages.csv"), lttb_csv)?;
        Ok(())
    }

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
