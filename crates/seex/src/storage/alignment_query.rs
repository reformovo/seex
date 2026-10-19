use crate::model::alignment::{
    AlignedMetricPoint, AlignmentAxis, AlignmentQuery, AlignmentQueryResult, AlignmentReduction,
};
use crate::model::metric::{MetricKey, MetricPoint, Step};
use crate::model::run::RunId;
use duckdb::Connection;

use crate::storage::StorageError;
use crate::storage::metric_query::percent_encode_metric_key;
use crate::storage::sampling::{self, Observation as StoredAlignedPoint};
use crate::storage::time::timestamp_from_millis;

#[derive(Clone, Copy)]
pub(crate) enum AlignmentSource<'a> {
    Project,
    Parquet(&'a str),
}

#[derive(Clone, Copy)]
enum AlignmentPlan {
    WholeSeries,
    NarrowStep,
}

pub(crate) fn query_aligned_metric(
    connection: &Connection,
    source: AlignmentSource<'_>,
    query: &AlignmentQuery,
    run_start_millis: Option<i64>,
) -> Result<AlignmentQueryResult, StorageError> {
    execute_aligned_metric(
        connection,
        source,
        query,
        run_start_millis,
        AlignmentPlan::WholeSeries,
    )
}

pub(crate) fn query_narrow_step_metric(
    connection: &Connection,
    source: AlignmentSource<'_>,
    query: &AlignmentQuery,
) -> Result<AlignmentQueryResult, StorageError> {
    if query.axis != AlignmentAxis::Step {
        return Err(StorageError::InvalidIdentity);
    }
    execute_aligned_metric(connection, source, query, None, AlignmentPlan::NarrowStep)
}

pub(crate) fn query_bounded_full_aligned_metric(
    connection: &Connection,
    source: AlignmentSource<'_>,
    query: &AlignmentQuery,
    run_start_millis: Option<i64>,
    _source_row_count: u64,
) -> Result<AlignmentQueryResult, StorageError> {
    validate_alignment_identity(query)?;
    let max_points = query
        .reduction
        .max_points()
        .ok_or(StorageError::InvalidIdentity)?;
    let sql = bounded_full_sql(source, query.axis);
    let mut values = base_values(source, query, run_start_millis);
    values.extend([
        Box::new(query.viewport.start()) as Box<dyn duckdb::ToSql>,
        Box::new(query.viewport.end()),
    ]);
    let mut statement = connection.prepare(&sql)?;
    let selected = sampling::select(&mut statement, &values, max_points, None)?;
    Ok(AlignmentQueryResult {
        points: selected
            .points
            .into_iter()
            .map(|p| p.into_aligned_metric_point(&query.run_id, &query.metric_key))
            .collect::<Result<Vec<_>, _>>()?,
        source_row_count: selected.source_count,
        reasons: Vec::new(),
    })
}

fn execute_aligned_metric(
    connection: &Connection,
    source: AlignmentSource<'_>,
    query: &AlignmentQuery,
    run_start_millis: Option<i64>,
    plan: AlignmentPlan,
) -> Result<AlignmentQueryResult, StorageError> {
    validate_alignment_identity(query)?;

    let max_points = query.reduction.max_points();
    let sql = aligned_points_sql(source, query.axis, max_points.is_some(), plan);
    let values = query_values(source, query, run_start_millis, plan)?;
    if let Some(budget) = max_points {
        let selected = sampling::select(
            &mut connection.prepare(&sql)?,
            &values,
            budget,
            Some((query.viewport.start(), query.viewport.end())),
        )?;
        return Ok(AlignmentQueryResult {
            points: selected
                .points
                .into_iter()
                .map(|p| p.into_aligned_metric_point(&query.run_id, &query.metric_key))
                .collect::<Result<Vec<_>, _>>()?,
            source_row_count: selected.source_count,
            reasons: Vec::new(),
        });
    }
    let started = std::time::Instant::now();
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        duckdb::params_from_iter(values.iter().map(|value| value.as_ref())),
        stored_alignment_row,
    )?;
    crate::storage::bootstrap::record_write_timing("aligned_execute", started.elapsed());
    let started = std::time::Instant::now();
    let mut points = Vec::new();
    let mut source_row_count = 0;
    for row in rows {
        let (point, count) = row?;
        if let Some(point) = point {
            points.push(point.into_aligned_metric_point(&query.run_id, &query.metric_key)?);
        }
        source_row_count = count;
    }
    crate::storage::bootstrap::record_write_timing("aligned_consume", started.elapsed());
    Ok(AlignmentQueryResult {
        points,
        source_row_count,
        reasons: Vec::new(),
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
    reduced: bool,
    plan: AlignmentPlan,
) -> String {
    let stats = if reduced {
        ", count(*) FILTER (WHERE in_view AND isfinite(value_f64))::UBIGINT AS finite_count,
           count(*) FILTER (WHERE NOT in_view)::UBIGINT AS neighbor_count"
    } else {
        ""
    };
    let metadata = if reduced {
        ", source_stats.finite_count, source_stats.neighbor_count"
    } else {
        ""
    };
    let order = if reduced {
        "selected.axis_value, selected.step"
    } else {
        "selected.step"
    };
    format!(
        "{},
         inside AS (SELECT * FROM ordered WHERE axis_value >= ? AND axis_value <= ?),
         left_neighbor AS (
             SELECT * FROM ordered WHERE axis_value < ?
             ORDER BY axis_value DESC, step DESC LIMIT 1
         ),
         right_neighbor AS (
             SELECT * FROM ordered WHERE axis_value > ?
             ORDER BY axis_value, step LIMIT 1
         ),
         visible_source AS (
             SELECT *, true AS in_view FROM inside
             UNION ALL SELECT *, false AS in_view FROM left_neighbor
             UNION ALL SELECT *, false AS in_view FROM right_neighbor
         ),
         source_stats AS (SELECT count(*)::UBIGINT AS source_row_count {stats} FROM visible_source)
         SELECT selected.step, epoch_ms(selected.timestamp), selected.value_f64,
                epoch_ms(selected.ingested_at), selected.axis_value, source_stats.source_row_count {metadata}
         FROM source_stats LEFT JOIN visible_source AS selected ON true ORDER BY {order}",
        match plan { AlignmentPlan::WholeSeries => ordered_ctes(source, axis),
                     AlignmentPlan::NarrowStep => narrow_step_ctes(source) }
    )
}

fn bounded_full_sql(source: AlignmentSource<'_>, axis: AlignmentAxis) -> String {
    format!(
        "{},
         selected AS (
             SELECT * FROM derived WHERE axis_value >= ? AND axis_value <= ?
         )
         SELECT step, epoch_ms(timestamp), value_f64, epoch_ms(ingested_at),
                axis_value
         FROM selected ORDER BY step",
        derived_ctes(source, axis)
    )
}

fn ordered_ctes(source: AlignmentSource<'_>, axis: AlignmentAxis) -> String {
    format!(
        "{},
         ordered AS MATERIALIZED (
             SELECT *
             FROM derived
         )",
        derived_ctes(source, axis)
    )
}

fn derived_ctes(source: AlignmentSource<'_>, axis: AlignmentAxis) -> String {
    let relation = source_relation(source);
    let axis_expression = match axis {
        AlignmentAxis::Step => "step",
        AlignmentAxis::ElapsedTime => "epoch_ms(timestamp) - ?",
    };
    format!(
        "WITH effective AS (
             SELECT step, timestamp, value_f64, ingested_at
             FROM {relation}
             WHERE run_id = ? AND metric_key = ? AND metric_key_encoded = ?
         ),
         derived AS (
             SELECT *, {axis_expression} AS axis_value FROM effective
         )"
    )
}

fn narrow_step_ctes(source: AlignmentSource<'_>) -> String {
    let relation = source_relation(source);
    format!(
        "WITH requested AS (
             SELECT ?::BIGINT AS start_step, ?::BIGINT AS end_step
         ),
         step_bounds AS (
             SELECT max(points.step) FILTER (
                        WHERE points.step < requested.start_step
                    ) AS left_step,
                    min(points.step) FILTER (
                        WHERE points.step > requested.end_step
                    ) AS right_step
             FROM requested CROSS JOIN {relation} AS points
             WHERE points.run_id = ? AND points.metric_key = ?
                   AND points.metric_key_encoded = ?
         ),
         effective AS (
             SELECT points.step, points.timestamp, points.value_f64, points.ingested_at
             FROM requested CROSS JOIN step_bounds
             CROSS JOIN {relation} AS points
             WHERE points.run_id = ? AND points.metric_key = ?
                   AND points.metric_key_encoded = ?
                   AND points.step >= coalesce(step_bounds.left_step, requested.start_step)
                   AND points.step <= coalesce(step_bounds.right_step, requested.end_step)
         ),
         derived AS (
             SELECT *, step AS axis_value FROM effective
         ),
         ordered AS MATERIALIZED (
             SELECT *
             FROM derived
         )"
    )
}

fn source_relation(source: AlignmentSource<'_>) -> &'static str {
    match source {
        AlignmentSource::Project => "dl.metric_points",
        AlignmentSource::Parquet(_) => {
            "read_parquet(?, hive_partitioning = true, union_by_name = true)"
        }
    }
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
    plan: AlignmentPlan,
) -> Result<Vec<Box<dyn duckdb::ToSql>>, StorageError> {
    let mut values = match plan {
        AlignmentPlan::WholeSeries => base_values(source, query, run_start_millis),
        AlignmentPlan::NarrowStep => narrow_step_values(source, query),
    };
    values.extend([
        Box::new(query.viewport.start()) as Box<dyn duckdb::ToSql>,
        Box::new(query.viewport.end()),
        Box::new(query.viewport.start()),
        Box::new(query.viewport.end()),
    ]);
    Ok(values)
}

fn narrow_step_values(
    source: AlignmentSource<'_>,
    query: &AlignmentQuery,
) -> Vec<Box<dyn duckdb::ToSql>> {
    let mut values: Vec<Box<dyn duckdb::ToSql>> = vec![
        Box::new(query.viewport.start()),
        Box::new(query.viewport.end()),
    ];
    for _ in 0..2 {
        if let AlignmentSource::Parquet(location) = source {
            values.push(Box::new(location.to_owned()));
        }
        values.extend([
            Box::new(query.run_id.as_str().to_owned()) as Box<dyn duckdb::ToSql>,
            Box::new(query.metric_key.as_str().to_owned()),
            Box::new(percent_encode_metric_key(query.metric_key.as_str())),
        ]);
    }
    values
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
) -> duckdb::Result<(Option<StoredAlignedPoint>, u64)> {
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
    Ok((point, row.get(5)?))
}

#[cfg(test)]
fn stored_bounded_alignment_row(row: &duckdb::Row<'_>) -> duckdb::Result<StoredAlignedPoint> {
    Ok(StoredAlignedPoint {
        step: row.get(0)?,
        timestamp_millis: row.get(1)?,
        value_f64: row.get(2)?,
        ingested_at_millis: row.get(3)?,
        axis_value: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::model::alignment::{AlignmentReduction, AlignmentViewport};

    use super::*;
    use crate::storage::ProjectMetricReader;

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
    fn fused_stats_preserve_observations_and_counts() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        connection.execute_batch(
            "INSERT INTO dl.metric_points SELECT * FROM dl.metric_points WHERE step=3;
             UPDATE dl.metric_points SET value_f64=CASE WHEN step=1 THEN 'NaN'::DOUBLE
                 WHEN step=2 THEN 'Infinity'::DOUBLE WHEN step=4 THEN '-0.0'::DOUBLE ELSE value_f64 END;"
        )?;
        for plan in [AlignmentPlan::WholeSeries, AlignmentPlan::NarrowStep] {
            for (start, end, key, expected) in [
                (0, 6, "loss", (10, 8, 0)),
                (2, 4, "loss", (8, 5, 2)),
                (3, 3, "loss", (6, 4, 2)),
                (-10, -5, "loss", (1, 0, 1)),
                (20, 30, "loss", (1, 0, 1)),
                (2, 4, "missing", (0, 0, 0)),
            ] {
                let mut query = query(AlignmentAxis::Step, AlignmentReduction::Full);
                query.viewport = AlignmentViewport::new(start, end)?;
                query.metric_key = MetricKey::from_string(key);
                let values = query_values(AlignmentSource::Project, &query, None, plan)?;
                let mut payloads = Vec::new();
                for reduced in [false, true] {
                    let sql = aligned_points_sql(
                        AlignmentSource::Project,
                        AlignmentAxis::Step,
                        reduced,
                        plan,
                    );
                    let mut statement = connection.prepare(&sql)?;
                    let rows = statement.query_map(
                        duckdb::params_from_iter(values.iter().map(|v| v.as_ref())),
                        |row| {
                            let (point, total) = stored_alignment_row(row)?;
                            assert_eq!(total, expected.0);
                            if reduced {
                                assert_eq!(
                                    (total, row.get::<_, u64>(6)?, row.get::<_, u64>(7)?),
                                    expected
                                );
                            }
                            Ok(point.map(|p| {
                                (
                                    p.step,
                                    p.timestamp_millis,
                                    p.value_f64.to_bits(),
                                    p.ingested_at_millis,
                                    p.axis_value,
                                )
                            }))
                        },
                    )?;
                    let mut rows = rows.collect::<Result<Vec<_>, _>>()?;
                    rows.sort();
                    payloads.push(rows);
                }
                assert_eq!(
                    payloads[0], payloads[1],
                    "viewport [{start},{end}] metric {key}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn full_step_query_keeps_closed_viewport_neighbors_and_observations()
    -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let result = ProjectMetricReader::new(&connection)
            .query_aligned_metric(&query(AlignmentAxis::Step, AlignmentReduction::Full))?;

        let mut observed = result
            .points
            .iter()
            .map(|point| (point.axis_value, point.point.value_f64))
            .collect::<Vec<_>>();
        observed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
        assert_eq!(
            observed,
            vec![(1, 1.0), (2, 2.0), (3, -1.0), (3, 3.0), (4, 4.0), (5, 5.0)]
        );
        assert_eq!(result.source_row_count, 6);
        assert!(result.reasons.is_empty());
        Ok(())
    }

    #[test]
    fn elapsed_query_retains_equal_axes_without_inline_diagnostics() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        connection.execute_batch(
            "UPDATE dl.metric_points SET timestamp = epoch_ms(1020) WHERE step = 3;
             UPDATE dl.metric_points SET timestamp = epoch_ms(1010) WHERE step = 4;",
        )?;
        let mut elapsed = query(AlignmentAxis::ElapsedTime, AlignmentReduction::Full);
        elapsed.viewport = AlignmentViewport::new(10, 20)?;
        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&elapsed)?;

        assert!(result.reasons.is_empty());
        assert_eq!(
            result
                .points
                .iter()
                .map(|point| point.axis_value)
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 20, 20, 10, 50]
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

        assert_eq!(result.points.len(), 8);
        assert!(!result.downsampled());
        Ok(())
    }

    #[test]
    fn step_query_retains_negative_axis_without_inline_diagnostics() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        connection.execute_batch(
            "INSERT INTO dl.metric_points VALUES
                 ('run-1', 'loss', 'loss', -1, epoch_ms(990), 9.0, epoch_ms(1999));",
        )?;
        let mut negative = query(AlignmentAxis::Step, AlignmentReduction::Full);
        negative.viewport = AlignmentViewport::new(-1, 0)?;
        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&negative)?;

        assert!(result.reasons.is_empty());
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
        let sql = aligned_points_sql(
            AlignmentSource::Project,
            AlignmentAxis::Step,
            true,
            AlignmentPlan::WholeSeries,
        );

        assert_eq!(sql.matches("FROM dl.metric_points").count(), 1);
        assert!(sql.contains("ordered AS MATERIALIZED"));
    }

    #[test]
    fn neighbors_are_real_nearest_observations_with_ties_and_empty_ranges()
    -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        connection.execute_batch(
            "INSERT INTO dl.metric_points SELECT * FROM dl.metric_points WHERE step IN (1,3,5);
             INSERT INTO dl.metric_points VALUES
                ('run-1','loss','loss',1,epoch_ms(1111),'NaN'::DOUBLE,epoch_ms(9111)),
                ('run-1','loss','loss',5,epoch_ms(1555),'Infinity'::DOUBLE,epoch_ms(9555));",
        )?;
        let source = connection.prepare(
            "SELECT step,epoch_ms(timestamp),value_f64,epoch_ms(ingested_at),step FROM dl.metric_points ORDER BY step"
        )?.query_map([],stored_bounded_alignment_row)?.collect::<Result<Vec<_>,_>>()?;
        let reader = ProjectMetricReader::new(&connection);
        for (start, end, key) in [
            (2, 4, "loss"),
            (3, 3, "loss"),
            (-10, -5, "loss"),
            (20, 30, "loss"),
            (0, 6, "loss"),
            (2, 4, "missing"),
        ] {
            let candidates = if key == "loss" {
                source.as_slice()
            } else {
                &[]
            };
            let left = candidates
                .iter()
                .filter(|p| p.step < start)
                .map(|p| p.step)
                .max();
            let right = candidates
                .iter()
                .filter(|p| p.step > end)
                .map(|p| p.step)
                .min();
            let count = candidates
                .iter()
                .filter(|p| p.step >= start && p.step <= end)
                .count()
                + usize::from(left.is_some())
                + usize::from(right.is_some());
            for reduction in [
                AlignmentReduction::Full,
                AlignmentReduction::screen_budget(4, 1)?,
                AlignmentReduction::screen_budget(100, 1)?,
            ] {
                let mut query = query(AlignmentAxis::Step, reduction);
                query.viewport = AlignmentViewport::new(start, end)?;
                query.metric_key = MetricKey::from_string(key);
                for narrow in [false, true] {
                    let result = if narrow {
                        reader.query_narrow_step_metric(&query)?
                    } else {
                        reader.query_aligned_metric(&query)?
                    };
                    assert_eq!(result.source_row_count, count as u64);
                    if let Some(budget) = reduction.max_points() {
                        assert!(result.points.len() <= budget);
                    } else {
                        assert_eq!(result.points.len(), count);
                    }
                    let lefts = result
                        .points
                        .iter()
                        .filter(|p| p.axis_value < start)
                        .map(|p| p.axis_value)
                        .collect::<Vec<_>>();
                    let rights = result
                        .points
                        .iter()
                        .filter(|p| p.axis_value > end)
                        .map(|p| p.axis_value)
                        .collect::<Vec<_>>();
                    assert_eq!(lefts, left.into_iter().collect::<Vec<_>>());
                    assert_eq!(rights, right.into_iter().collect::<Vec<_>>());
                    let mut available = candidates.iter().collect::<Vec<_>>();
                    for selected in result.points {
                        let point = selected.point;
                        let index = available.iter().position(|p|p.step==point.step.value()
                            && p.timestamp_millis==point.timestamp.timestamp_millis()
                            && p.ingested_at_millis==point.ingested_at.timestamp_millis()
                            && (p.value_f64==point.value_f64 || (p.value_f64.is_nan()&&point.value_f64.is_nan())))
                            .expect("neighbor ties must select a whole real observation, never fabricate payload");
                        available.remove(index);
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn narrow_step_plan_keeps_duplicates_and_real_neighbors() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let query = query(
            AlignmentAxis::Step,
            AlignmentReduction::screen_budget(100, 1)?,
        );
        let reader = ProjectMetricReader::new(&connection);

        let mut full = reader.query_aligned_metric(&query)?;
        let mut narrow = reader.query_narrow_step_metric(&query)?;

        for points in [&mut narrow.points, &mut full.points] {
            points.sort_by(|a, b| {
                a.point
                    .step
                    .value()
                    .cmp(&b.point.step.value())
                    .then_with(|| a.point.value_f64.total_cmp(&b.point.value_f64))
            });
        }
        assert_eq!(narrow, full);
        assert_eq!(narrow.source_row_count, 6);
        assert_eq!(
            narrow
                .points
                .iter()
                .filter(|p| p.point.step.value() == 3)
                .count(),
            2
        );
        let sql = aligned_points_sql(
            AlignmentSource::Project,
            AlignmentAxis::Step,
            true,
            AlignmentPlan::NarrowStep,
        );
        assert_eq!(sql.matches("dl.metric_points").count(), 2);
        assert!(sql.contains("coalesce(step_bounds.left_step"));
        assert!(sql.contains("coalesce(step_bounds.right_step"));
        Ok(())
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
