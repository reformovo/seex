use crate::model::alignment::{
    AlignedMetricPoint, AlignmentAxis, AlignmentQuery, AlignmentQueryResult, AlignmentReduction,
};
use crate::model::metric::{MetricKey, MetricPoint, Step};
use crate::model::run::RunId;
use duckdb::Connection;

use crate::storage::StorageError;
use crate::storage::metric_query::percent_encode_metric_key;
use crate::storage::time::timestamp_from_millis;

const EXTREMA_PER_BUCKET: usize = 4;

#[derive(Clone, Copy)]
pub(crate) enum AlignmentSource<'a> {
    Project,
    Parquet(&'a str),
}

#[derive(Clone, Copy)]
enum AlignmentPlan {
    Full,
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
        AlignmentPlan::Full,
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
    source_row_count: u64,
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
    let mut reducer = ScreenReducer::new(max_points, source_row_count);
    let started = std::time::Instant::now();
    if std::env::var_os("SEEX_DIAGNOSTIC_BATCH_READER").is_some() {
        let batches = statement.query_arrow(duckdb::params_from_iter(
            values.iter().map(|value| value.as_ref()),
        ))?;
        crate::storage::bootstrap::record_write_timing("bounded_execute_batch", started.elapsed());
        let started = std::time::Instant::now();
        for batch in batches {
            reduce_batch(&batch, &mut reducer)?;
        }
        crate::storage::bootstrap::record_write_timing("bounded_consume_batch", started.elapsed());
        if reducer.ordinal != statement.row_count() as u64 {
            return Err(batch_decode_error("incomplete bounded result"));
        }
    } else {
        let rows = statement.query_map(
            duckdb::params_from_iter(values.iter().map(|value| value.as_ref())),
            stored_bounded_alignment_row,
        )?;
        crate::storage::bootstrap::record_write_timing("bounded_execute_rows", started.elapsed());
        let started = std::time::Instant::now();
        for row in rows {
            reducer.push(row?);
        }
        crate::storage::bootstrap::record_write_timing("bounded_consume_rows", started.elapsed());
    }
    let points = reducer
        .finish()
        .into_iter()
        .map(|point| point.into_aligned_metric_point(&query.run_id, &query.metric_key))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AlignmentQueryResult {
        points,
        source_row_count,
        reasons: Vec::new(),
    })
}

fn batch_decode_error(message: &'static str) -> StorageError {
    StorageError::Storage {
        operation: "decoding bounded query result",
        name: String::from("Arrow batch"),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, message),
    }
}

fn reduce_batch(
    batch: &duckdb::arrow::record_batch::RecordBatch,
    reducer: &mut ScreenReducer,
) -> Result<(), StorageError> {
    use duckdb::arrow::array::{Array, Float64Array, Int64Array};
    if batch.num_columns() != 5 {
        return Err(batch_decode_error("expected five bounded result columns"));
    }
    let integer = |index| {
        batch
            .column(index)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| batch_decode_error("expected BIGINT bounded result column"))
    };
    let columns = [integer(0)?, integer(1)?, integer(3)?, integer(4)?];
    let values = batch
        .column(2)
        .as_any()
        .downcast_ref::<Float64Array>()
        .ok_or_else(|| batch_decode_error("expected DOUBLE bounded value"))?;
    if columns.iter().any(|column| column.null_count() != 0) || values.null_count() != 0 {
        return Err(batch_decode_error("unexpected NULL bounded value"));
    }
    for index in 0..batch.num_rows() {
        reducer.push(StoredAlignedPoint {
            step: columns[0].value(index),
            timestamp_millis: columns[1].value(index),
            value_f64: values.value(index),
            ingested_at_millis: columns[2].value(index),
            axis_value: columns[3].value(index),
        });
    }
    Ok(())
}

fn execute_aligned_metric(
    connection: &Connection,
    source: AlignmentSource<'_>,
    query: &AlignmentQuery,
    run_start_millis: Option<i64>,
    plan: AlignmentPlan,
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
    let sql = aligned_points_sql(source, query.axis, screen_limits.is_some(), plan);
    let values = query_values(source, query, run_start_millis, screen_limits, plan)?;
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        duckdb::params_from_iter(values.iter().map(|value| value.as_ref())),
        stored_alignment_row,
    )?;
    let mut points = Vec::new();
    let mut source_row_count = 0;
    for row in rows {
        let (point, count) = row?;
        if let Some(point) = point {
            points.push(point.into_aligned_metric_point(&query.run_id, &query.metric_key)?);
        }
        source_row_count = count;
    }
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
    screen_reduced: bool,
    plan: AlignmentPlan,
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
                source_stats.source_row_count
         FROM source_stats
         LEFT JOIN selected ON true
         ORDER BY selected.step",
        match plan {
            AlignmentPlan::Full => ordered_ctes(source, axis),
            AlignmentPlan::NarrowStep => narrow_step_ctes(source),
        }
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
             SELECT *, lag(axis_value) OVER (ORDER BY step) AS previous_axis_value
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
             GROUP BY requested.start_step, requested.end_step
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
             SELECT *, lag(axis_value) OVER (ORDER BY step) AS previous_axis_value
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
    screen_limits: Option<(usize, usize)>,
    plan: AlignmentPlan,
) -> Result<Vec<Box<dyn duckdb::ToSql>>, StorageError> {
    let mut values = match plan {
        AlignmentPlan::Full => base_values(source, query, run_start_millis),
        AlignmentPlan::NarrowStep => narrow_step_values(source, query),
    };
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

#[derive(Clone)]
struct StoredAlignedPoint {
    step: i64,
    timestamp_millis: i64,
    value_f64: f64,
    ingested_at_millis: i64,
    axis_value: i64,
}

#[derive(Default)]
struct BucketCandidates {
    first: Option<(u64, StoredAlignedPoint)>,
    last: Option<(u64, StoredAlignedPoint)>,
    min: Option<(u64, StoredAlignedPoint)>,
    max: Option<(u64, StoredAlignedPoint)>,
}

impl BucketCandidates {
    fn push(&mut self, ordinal: u64, point: StoredAlignedPoint) {
        self.first.get_or_insert_with(|| (ordinal, point.clone()));
        self.last = Some((ordinal, point.clone()));
        if self
            .min
            .as_ref()
            .is_none_or(|current| point_cmp(&point, &current.1, false).is_lt())
        {
            self.min = Some((ordinal, point.clone()));
        }
        if self
            .max
            .as_ref()
            .is_none_or(|current| point_cmp(&point, &current.1, true).is_lt())
        {
            self.max = Some((ordinal, point));
        }
    }

    fn append_to(self, points: &mut Vec<StoredAlignedPoint>) {
        // A row may fill several roles, but distinct observations must survive
        // even when their Steps and all payload fields are identical.
        let mut selected = [self.first, self.last, self.min, self.max]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        selected.sort_by_key(|(ordinal, _)| *ordinal);
        selected.dedup_by_key(|(ordinal, _)| *ordinal);
        points.extend(selected.into_iter().map(|(_, point)| point));
    }
}

struct ScreenReducer {
    bucket_count: u64,
    source_count: u64,
    ordinal: u64,
    current_bucket: Option<u64>,
    candidates: BucketCandidates,
    points: Vec<StoredAlignedPoint>,
}

impl ScreenReducer {
    fn new(max_points: usize, source_count: u64) -> Self {
        Self {
            bucket_count: u64::try_from((max_points / EXTREMA_PER_BUCKET).max(1))
                .unwrap_or(u64::MAX),
            source_count,
            ordinal: 0,
            current_bucket: None,
            candidates: BucketCandidates::default(),
            points: Vec::with_capacity(max_points),
        }
    }

    fn push(&mut self, point: StoredAlignedPoint) {
        let bucket = ((u128::from(self.ordinal) * u128::from(self.bucket_count))
            / u128::from(self.source_count)) as u64;
        if self.current_bucket.is_some_and(|current| current != bucket) {
            std::mem::take(&mut self.candidates).append_to(&mut self.points);
        }
        self.current_bucket = Some(bucket);
        self.candidates.push(self.ordinal, point);
        self.ordinal += 1;
    }

    fn finish(mut self) -> Vec<StoredAlignedPoint> {
        self.candidates.append_to(&mut self.points);
        self.points.sort_by_key(|point| point.step);
        self.points
    }
}

fn point_cmp(
    left: &StoredAlignedPoint,
    right: &StoredAlignedPoint,
    maximum: bool,
) -> std::cmp::Ordering {
    // SQL treats signed zeros as equal and breaks both min/max ties by ascending axis.
    let value_order = if left.value_f64 == right.value_f64 {
        std::cmp::Ordering::Equal
    } else {
        left.value_f64.total_cmp(&right.value_f64)
    };
    let value_order = if maximum {
        value_order.reverse()
    } else {
        value_order
    };
    value_order
        .then_with(|| left.axis_value.cmp(&right.axis_value))
        .then_with(|| left.step.cmp(&right.step))
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
    fn elapsed_screen_query_uses_extrema_without_lttb() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let mut elapsed = query(
            AlignmentAxis::ElapsedTime,
            AlignmentReduction::screen_budget(1, 1)?,
        );
        elapsed.viewport = AlignmentViewport::new(10, 50)?;
        let result = ProjectMetricReader::new(&connection).query_aligned_metric(&elapsed)?;

        assert_eq!(result.source_row_count, 8);
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

        assert_eq!(result.source_row_count, 8);
        assert!(result.downsampled());
        let mut observed = result
            .points
            .iter()
            .map(|point| (point.axis_value, point.point.value_f64))
            .collect::<Vec<_>>();
        observed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
        assert_eq!(
            observed,
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

        assert_eq!(result.points.len(), 8);
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
    fn bounded_full_reducer_matches_window_extrema() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let mut screen = query(
            AlignmentAxis::Step,
            AlignmentReduction::screen_budget(1, 1)?,
        );
        screen.viewport = AlignmentViewport::new(0, 6)?;

        let windowed = query_aligned_metric(&connection, AlignmentSource::Project, &screen, None)?;
        let bounded = query_bounded_full_aligned_metric(
            &connection,
            AlignmentSource::Project,
            &screen,
            None,
            8,
        )?;

        assert_eq!(bounded, windowed);
        Ok(())
    }

    #[test]
    fn bounded_step_reducer_preserves_results_with_decreasing_timestamps()
    -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        connection.execute_batch("UPDATE dl.metric_points SET timestamp=epoch_ms(2000-step);")?;
        for budget in [1, 2, 100] {
            let mut screen = query(
                AlignmentAxis::Step,
                AlignmentReduction::screen_budget(budget, 1)?,
            );
            screen.viewport = AlignmentViewport::new(0, 6)?;
            let mut windowed =
                query_aligned_metric(&connection, AlignmentSource::Project, &screen, None)?;
            let mut bounded = query_bounded_full_aligned_metric(
                &connection,
                AlignmentSource::Project,
                &screen,
                None,
                8,
            )?;
            for points in [&mut windowed.points, &mut bounded.points] {
                points.sort_by(|a, b| {
                    a.point
                        .step
                        .value()
                        .cmp(&b.point.step.value())
                        .then_with(|| a.point.value_f64.total_cmp(&b.point.value_f64))
                });
            }
            assert_eq!(bounded, windowed, "screen budget {budget}");
        }
        Ok(())
    }

    #[test]
    fn bounded_step_reducer_matches_finite_ties_and_signed_zero() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        for expression in [
            "CASE WHEN step IN (1, 4) THEN -2.0 WHEN step IN (2, 5) THEN 2.0 ELSE 1.0 END",
            "CASE WHEN step = 1 THEN 0.0::DOUBLE WHEN step = 4 THEN '-0.0'::DOUBLE ELSE 1.0 END",
        ] {
            connection.execute_batch(&format!(
                "DELETE FROM dl.metric_points;
                 INSERT INTO dl.metric_points
                 SELECT 'run-1', 'loss', 'loss', step, epoch_ms(2000-step),
                        {expression}, epoch_ms(3000+step)
                 FROM range(0, 8) AS generated(step);"
            ))?;
            for budget in [1, 2, 3, 100] {
                let mut screen = query(
                    AlignmentAxis::Step,
                    AlignmentReduction::screen_budget(budget, 1)?,
                );
                screen.viewport = AlignmentViewport::new(0, 7)?;
                let expected =
                    query_aligned_metric(&connection, AlignmentSource::Project, &screen, None)?;
                let actual = query_bounded_full_aligned_metric(
                    &connection,
                    AlignmentSource::Project,
                    &screen,
                    None,
                    8,
                )?;
                let payloads = |points: Vec<AlignedMetricPoint>| {
                    let mut values = points
                        .into_iter()
                        .map(|p| {
                            (
                                p.point.step.value(),
                                p.point.timestamp.timestamp_millis(),
                                if p.point.value_f64 == 0.0 {
                                    0
                                } else {
                                    p.point.value_f64.to_bits()
                                },
                                p.point.ingested_at.timestamp_millis(),
                            )
                        })
                        .collect::<Vec<_>>();
                    values.sort();
                    values
                };
                assert_eq!(actual.source_row_count, expected.source_row_count);
                assert_eq!(
                    payloads(actual.points),
                    payloads(expected.points),
                    "expression={expression}, budget={budget}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn batch_decoder_matches_rows_and_rejects_nulls() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let sql = "SELECT step, epoch_ms(timestamp), value_f64, epoch_ms(ingested_at), step
                   FROM dl.metric_points ORDER BY step";
        let mut expected = ScreenReducer::new(4, 8);
        let mut statement = connection.prepare(sql)?;
        for row in statement.query_map([], stored_bounded_alignment_row)? {
            expected.push(row?);
        }
        let mut actual = ScreenReducer::new(4, 8);
        for batch in connection.prepare(sql)?.query_arrow([])? {
            reduce_batch(&batch, &mut actual)?;
        }
        let payloads = |points: Vec<StoredAlignedPoint>| {
            points
                .into_iter()
                .map(|p| {
                    (
                        p.step,
                        p.timestamp_millis,
                        p.value_f64.to_bits(),
                        p.ingested_at_millis,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(actual.ordinal, expected.ordinal);
        assert_eq!(payloads(actual.finish()), payloads(expected.finish()));
        for batch in connection
            .prepare("SELECT 1::BIGINT, 1::BIGINT, NULL::DOUBLE, 1::BIGINT, 1::BIGINT")?
            .query_arrow([])?
        {
            assert!(reduce_batch(&batch, &mut ScreenReducer::new(4, 1)).is_err());
        }
        Ok(())
    }

    #[test]
    fn bounded_reducer_preserves_same_step_extrema_without_repeating_roles() {
        let point = |value| StoredAlignedPoint {
            step: 3,
            timestamp_millis: 1000,
            value_f64: value,
            ingested_at_millis: 2000,
            axis_value: 3,
        };
        let mut reducer = ScreenReducer::new(4, 3);
        reducer.push(point(-1.0));
        reducer.push(point(2.0));
        reducer.push(point(0.0));
        let selected = reducer.finish();
        assert_eq!(selected.len(), 3);
        assert!(selected.iter().any(|p| p.value_f64 == -1.0));
        assert!(selected.iter().any(|p| p.value_f64 == 2.0));

        let mut identical = ScreenReducer::new(4, 2);
        identical.push(point(1.0));
        identical.push(point(1.0));
        assert_eq!(identical.finish().len(), 2);
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
            AlignmentPlan::Full,
        );

        assert_eq!(sql.matches("FROM dl.metric_points").count(), 1);
        assert!(sql.contains("ordered AS MATERIALIZED"));
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
