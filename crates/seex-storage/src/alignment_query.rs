use duckdb::Connection;
use seex_model::alignment::{
    AlignedMetricPoint, AlignmentAxis, AlignmentQuery, AlignmentQueryResult, AlignmentReason,
    AlignmentReduction,
};
use seex_model::metric::{MetricKey, MetricPoint, Step};
use seex_model::run::RunId;

use crate::StorageError;
use crate::metric_query::percent_encode_metric_key;
use crate::time::timestamp_from_millis;

const EXTREMA_PER_BUCKET: usize = 4;

#[derive(Clone, Copy)]
pub(crate) enum AlignmentSource<'a> {
    Project,
    Parquet(&'a str),
}

pub(crate) fn query_aligned_metric(
    connection: &Connection,
    source: AlignmentSource<'_>,
    query: &AlignmentQuery,
    run_start_millis: Option<i64>,
) -> Result<AlignmentQueryResult, StorageError> {
    validate_alignment_identity(query)?;

    let screen_limits = match query.reduction {
        AlignmentReduction::Full => None,
        AlignmentReduction::ScreenBudget(_) => {
            let max_points = query
                .reduction
                .max_points()
                .expect("screen budgets always have a point limit");
            Some(((max_points / EXTREMA_PER_BUCKET).max(1), max_points))
        }
    };
    let sql = aligned_points_sql(source, query.axis, screen_limits.is_some());
    let values = query_values(source, query, run_start_millis, screen_limits)?;
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        duckdb::params_from_iter(values.iter().map(|value| value.as_ref())),
        stored_alignment_row,
    )?;
    let mut points = Vec::new();
    let mut source_row_count = 0;
    let mut has_negative_axis = false;
    let mut has_decreasing_axis = false;
    for row in rows {
        let (point, count, negative, decreasing) = row?;
        if let Some(point) = point {
            points.push(point.into_aligned_metric_point(&query.run_id, &query.metric_key)?);
        }
        source_row_count = count;
        has_negative_axis = negative;
        has_decreasing_axis = decreasing;
    }
    let mut reasons = Vec::with_capacity(2);
    if has_negative_axis {
        reasons.push(AlignmentReason::NegativeAxis);
    }
    if has_decreasing_axis {
        reasons.push(AlignmentReason::DecreasingAxis);
    }
    Ok(AlignmentQueryResult {
        points,
        source_row_count,
        reasons,
    })
}

pub(crate) fn validate_alignment_identity(query: &AlignmentQuery) -> Result<(), StorageError> {
    if query.run_id.as_str().trim().is_empty() || query.metric_key.as_str().trim().is_empty() {
        return Err(StorageError::InvalidIdentity);
    }
    Ok(())
}

fn aligned_points_sql(
    source: AlignmentSource<'_>,
    axis: AlignmentAxis,
    screen_reduced: bool,
) -> String {
    let selection = if screen_reduced {
        "inside_numbered AS (
             SELECT *, row_number() OVER (ORDER BY step) - 1 AS ordinal,
                    count(*) OVER ()::UBIGINT AS inside_count
             FROM inside
         ),
         inside_bucketed AS (
             SELECT *, floor(
                 ordinal::DOUBLE * ?::DOUBLE / inside_count::DOUBLE
             )::UBIGINT AS bucket
             FROM inside_numbered
         ),
         inside_candidates AS (
             SELECT *,
                    row_number() OVER (PARTITION BY bucket ORDER BY axis_value, step) AS first_rank,
                    row_number() OVER (PARTITION BY bucket ORDER BY axis_value DESC, step DESC) AS last_rank,
                    row_number() OVER (PARTITION BY bucket ORDER BY value_f64, axis_value, step) AS min_rank,
                    row_number() OVER (PARTITION BY bucket ORDER BY value_f64 DESC, axis_value, step) AS max_rank
             FROM inside_bucketed
         ),
         selected AS (
             SELECT * EXCLUDE (ordinal, inside_count, bucket, first_rank, last_rank, min_rank, max_rank)
             FROM inside_candidates
             WHERE inside_count <= ? OR first_rank = 1 OR last_rank = 1
                   OR min_rank = 1 OR max_rank = 1
             UNION ALL SELECT * FROM left_neighbor
             UNION ALL SELECT * FROM right_neighbor
         )"
    } else {
        "selected AS (SELECT * FROM visible_source)"
    };
    format!(
        "{},
         axis_stats AS (
             SELECT coalesce(bool_or(axis_value < 0), false) AS has_negative_axis,
                    coalesce(bool_or(
                        previous_axis_value IS NOT NULL AND axis_value < previous_axis_value
                    ), false) AS has_decreasing_axis
             FROM ordered
         ),
         inside AS (
             SELECT * FROM ordered WHERE axis_value >= ? AND axis_value <= ?
         ),
         left_neighbor AS (
             SELECT * FROM ordered WHERE axis_value < ?
             QUALIFY row_number() OVER (ORDER BY axis_value DESC, step DESC) = 1
         ),
         right_neighbor AS (
             SELECT * FROM ordered WHERE axis_value > ?
             QUALIFY row_number() OVER (ORDER BY axis_value, step) = 1
         ),
         visible_source AS (
             SELECT * FROM inside
             UNION ALL SELECT * FROM left_neighbor
             UNION ALL SELECT * FROM right_neighbor
         ),
         source_stats AS (
             SELECT count(*)::UBIGINT AS source_row_count FROM visible_source
         ),
         {selection}
         SELECT selected.step, epoch_ms(selected.timestamp), selected.value_f64,
                epoch_ms(selected.ingested_at), selected.axis_value,
                source_stats.source_row_count, axis_stats.has_negative_axis,
                axis_stats.has_decreasing_axis
         FROM source_stats CROSS JOIN axis_stats
         LEFT JOIN selected ON true
         ORDER BY selected.step",
        ordered_ctes(source, axis)
    )
}

fn ordered_ctes(source: AlignmentSource<'_>, axis: AlignmentAxis) -> String {
    let (relation, tie_breaker) = match source {
        AlignmentSource::Project => ("dl.metric_points", "rowid DESC"),
        AlignmentSource::Parquet(_) => (
            "read_parquet(?, hive_partitioning = true, union_by_name = true, \
             filename = true, file_row_number = true)",
            "filename DESC, file_row_number DESC",
        ),
    };
    let axis_expression = match axis {
        AlignmentAxis::Step => "step",
        AlignmentAxis::ElapsedTime => "epoch_ms(timestamp) - ?",
    };
    format!(
        "WITH ranked AS (
             SELECT step, timestamp, value_f64, ingested_at,
                    row_number() OVER (
                        PARTITION BY step
                        ORDER BY ingested_at DESC, {tie_breaker}
                    ) AS write_rank
             FROM {relation}
             WHERE run_id = ? AND metric_key = ? AND metric_key_encoded = ?
         ),
         effective AS (
             SELECT step, timestamp, value_f64, ingested_at
             FROM ranked WHERE write_rank = 1
         ),
         derived AS (
             SELECT *, {axis_expression} AS axis_value FROM effective
         ),
         ordered AS MATERIALIZED (
             SELECT *, lag(axis_value) OVER (ORDER BY step) AS previous_axis_value
             FROM derived
         )"
    )
}

fn base_values(
    source: AlignmentSource<'_>,
    query: &AlignmentQuery,
    run_start_millis: Option<i64>,
) -> Vec<Box<dyn duckdb::ToSql>> {
    let mut values: Vec<Box<dyn duckdb::ToSql>> = Vec::with_capacity(5);
    if let AlignmentSource::Parquet(location) = source {
        values.push(Box::new(location.to_owned()));
    }
    values.extend([
        Box::new(query.run_id.as_str().to_owned()) as Box<dyn duckdb::ToSql>,
        Box::new(query.metric_key.as_str().to_owned()),
        Box::new(percent_encode_metric_key(query.metric_key.as_str())),
    ]);
    if matches!(query.axis, AlignmentAxis::ElapsedTime) {
        values.push(Box::new(run_start_millis));
    }
    values
}

fn query_values(
    source: AlignmentSource<'_>,
    query: &AlignmentQuery,
    run_start_millis: Option<i64>,
    screen_limits: Option<(usize, usize)>,
) -> Result<Vec<Box<dyn duckdb::ToSql>>, StorageError> {
    let mut values = base_values(source, query, run_start_millis);
    values.extend([
        Box::new(query.viewport.start()) as Box<dyn duckdb::ToSql>,
        Box::new(query.viewport.end()),
        Box::new(query.viewport.start()),
        Box::new(query.viewport.end()),
    ]);
    if let Some((bucket_count, max_points)) = screen_limits {
        values.push(Box::new(i64::try_from(bucket_count).map_err(|_| {
            StorageError::QueryMaxPointsTooLarge {
                max_points: bucket_count.saturating_mul(EXTREMA_PER_BUCKET),
            }
        })?));
        values
            .push(Box::new(i64::try_from(max_points).map_err(|_| {
                StorageError::QueryMaxPointsTooLarge { max_points }
            })?));
    }
    Ok(values)
}

struct StoredAlignedPoint {
    step: i64,
    timestamp_millis: i64,
    value_f64: f64,
    ingested_at_millis: i64,
    axis_value: i64,
}

impl StoredAlignedPoint {
    fn into_aligned_metric_point(
        self,
        run_id: &RunId,
        metric_key: &MetricKey,
    ) -> Result<AlignedMetricPoint, StorageError> {
        Ok(AlignedMetricPoint {
            point: MetricPoint {
                run_id: run_id.clone(),
                metric_key: metric_key.clone(),
                step: Step::new(self.step),
                timestamp: timestamp_from_millis("timestamp", self.timestamp_millis)?,
                value_f64: self.value_f64,
                ingested_at: timestamp_from_millis("ingested_at", self.ingested_at_millis)?,
            },
            axis_value: self.axis_value,
        })
    }
}

fn stored_alignment_row(
    row: &duckdb::Row<'_>,
) -> duckdb::Result<(Option<StoredAlignedPoint>, u64, bool, bool)> {
    let step: Option<i64> = row.get(0)?;
    let point = match step {
        Some(step) => Some(StoredAlignedPoint {
            step,
            timestamp_millis: row.get(1)?,
            value_f64: row.get(2)?,
            ingested_at_millis: row.get(3)?,
            axis_value: row.get(4)?,
        }),
        None => None,
    };
    Ok((point, row.get(5)?, row.get(6)?, row.get(7)?))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use seex_model::alignment::{AlignmentReduction, AlignmentViewport};

    use super::*;
    use crate::ProjectMetricReader;

    fn connection() -> Result<Connection, Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE SCHEMA dl;
             CREATE TABLE dl.metric_points (
                 run_id VARCHAR NOT NULL,
                 metric_key VARCHAR NOT NULL,
                 metric_key_encoded VARCHAR NOT NULL,
                 step BIGINT NOT NULL,
                 timestamp TIMESTAMPTZ NOT NULL,
                 value_f64 DOUBLE NOT NULL,
                 ingested_at TIMESTAMPTZ NOT NULL
             );
             CREATE TABLE seex_runs (run_id VARCHAR, started_at TIMESTAMPTZ);
             INSERT INTO seex_runs VALUES ('run-1', epoch_ms(1000));
             INSERT INTO dl.metric_points
             SELECT 'run-1', 'loss', 'loss', step, epoch_ms(1000 + step * 10),
                    step::DOUBLE, epoch_ms(2000 + step)
             FROM range(0, 7) AS generated(step);
             INSERT INTO dl.metric_points VALUES
                 ('run-1', 'loss', 'loss', 3, epoch_ms(1030), -1.0, epoch_ms(3000));",
        )?;
        Ok(connection)
    }

    fn query(axis: AlignmentAxis, reduction: AlignmentReduction) -> AlignmentQuery {
        AlignmentQuery {
            run_id: RunId::from_string("run-1"),
            metric_key: MetricKey::from_string("loss"),
            axis,
            viewport: AlignmentViewport::new(2, 4).expect("test viewport should be valid"),
            reduction,
        }
    }

    #[test]
    fn full_step_query_keeps_closed_viewport_neighbors_and_effective_points()
    -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let result = ProjectMetricReader::new(&connection)
            .query_aligned_metric(&query(AlignmentAxis::Step, AlignmentReduction::Full))?;

        assert_eq!(
            result
                .points
                .iter()
                .map(|point| (point.axis_value, point.point.value_f64))
                .collect::<Vec<_>>(),
            vec![(1, 1.0), (2, 2.0), (3, -1.0), (4, 4.0), (5, 5.0)]
        );
        assert_eq!(result.source_row_count, 5);
        assert!(result.reasons.is_empty());
        Ok(())
    }

    #[test]
    fn elapsed_query_retains_equal_axes_and_reports_decreases() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        connection.execute_batch(
            "UPDATE dl.metric_points SET timestamp = epoch_ms(1020) WHERE step = 3;
             UPDATE dl.metric_points SET timestamp = epoch_ms(1010) WHERE step = 4;",
        )?;
        let mut elapsed = query(AlignmentAxis::ElapsedTime, AlignmentReduction::Full);
        elapsed.viewport = AlignmentViewport::new(10, 20)?;
        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&elapsed)?;

        assert_eq!(result.reasons, vec![AlignmentReason::DecreasingAxis]);
        assert_eq!(
            result
                .points
                .iter()
                .map(|point| point.axis_value)
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 20, 10, 50]
        );
        Ok(())
    }

    #[test]
    fn elapsed_screen_query_uses_extrema_without_lttb() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let mut elapsed = query(
            AlignmentAxis::ElapsedTime,
            AlignmentReduction::screen_budget(1, 1)?,
        );
        elapsed.viewport = AlignmentViewport::new(10, 50)?;
        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&elapsed)?;

        assert_eq!(result.source_row_count, 7);
        assert!(result.downsampled());
        assert!(result.reasons.is_empty());
        assert_eq!(result.points.first().map(|point| point.axis_value), Some(0));
        assert_eq!(result.points.last().map(|point| point.axis_value), Some(60));
        Ok(())
    }

    #[test]
    fn screen_query_keeps_neighbors_and_bucket_extrema() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let mut screen = query(
            AlignmentAxis::Step,
            AlignmentReduction::screen_budget(1, 1)?,
        );
        screen.viewport = AlignmentViewport::new(1, 5)?;
        let result = query_aligned_metric(&connection, AlignmentSource::Project, &screen, None)?;

        assert_eq!(result.source_row_count, 7);
        assert!(result.downsampled());
        assert_eq!(
            result
                .points
                .iter()
                .map(|point| (point.axis_value, point.point.value_f64))
                .collect::<Vec<_>>(),
            vec![(0, 0.0), (1, 1.0), (3, -1.0), (5, 5.0), (6, 6.0)]
        );
        Ok(())
    }

    #[test]
    fn screen_query_returns_all_points_when_inside_count_fits_budget() -> Result<(), Box<dyn Error>>
    {
        let connection = connection()?;
        let mut screen = query(
            AlignmentAxis::Step,
            AlignmentReduction::screen_budget(100, 1)?,
        );
        screen.viewport = AlignmentViewport::new(1, 5)?;

        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&screen)?;

        assert_eq!(result.points.len(), 7);
        assert!(!result.downsampled());
        Ok(())
    }

    #[test]
    fn screen_extrema_ties_choose_the_lowest_axis_point() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        connection
            .execute_batch("UPDATE dl.metric_points SET value_f64 = -2.0 WHERE step IN (2, 4);")?;
        let mut screen = query(
            AlignmentAxis::Step,
            AlignmentReduction::screen_budget(1, 1)?,
        );
        screen.viewport = AlignmentViewport::new(1, 5)?;

        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&screen)?;

        assert!(result.points.iter().any(|point| point.axis_value == 2));
        assert!(!result.points.iter().any(|point| point.axis_value == 4));
        Ok(())
    }

    #[test]
    fn step_query_reports_negative_axis_without_repairing_points() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        connection.execute_batch(
            "INSERT INTO dl.metric_points VALUES
                 ('run-1', 'loss', 'loss', -1, epoch_ms(990), 9.0, epoch_ms(1999));",
        )?;
        let mut negative = query(AlignmentAxis::Step, AlignmentReduction::Full);
        negative.viewport = AlignmentViewport::new(-1, 0)?;
        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&negative)?;

        assert_eq!(result.reasons, vec![AlignmentReason::NegativeAxis]);
        assert!(result.points.iter().any(|point| point.axis_value == -1));
        Ok(())
    }

    #[test]
    fn native_elapsed_query_validates_identity_before_run_lookup() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let mut invalid = query(AlignmentAxis::ElapsedTime, AlignmentReduction::Full);
        invalid.run_id = RunId::from_string("");

        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&invalid);

        assert!(matches!(result, Err(StorageError::InvalidIdentity)));
        Ok(())
    }

    #[test]
    fn aligned_query_materializes_one_effective_source_scan() {
        let sql = aligned_points_sql(AlignmentSource::Project, AlignmentAxis::Step, true);

        assert_eq!(sql.matches("FROM dl.metric_points").count(), 1);
        assert!(sql.contains("ordered AS MATERIALIZED"));
    }

    #[test]
    fn aligned_query_returns_empty_result_from_metadata_row() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let mut missing = query(AlignmentAxis::Step, AlignmentReduction::Full);
        missing.metric_key = MetricKey::from_string("missing");

        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&missing)?;

        assert!(result.points.is_empty());
        assert_eq!(result.source_row_count, 0);
        assert!(result.reasons.is_empty());
        Ok(())
    }
}
