use crate::model::alignment::{
    AlignedMetricPoint, AlignmentAxis, AlignmentQuery, AlignmentQueryResult, AlignmentReduction,
};
use crate::model::metric::{MetricKey, MetricPoint, Step};
use crate::model::run::RunId;
use duckdb::Connection;

use crate::storage::StorageError;
use crate::storage::lttb::{OrderedLttb, Point};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScreenStrategy {
    Default,
    Grouped,
    Lttb,
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
    let batches = statement.query_arrow(duckdb::params_from_iter(
        values.iter().map(|value| value.as_ref()),
    ))?;
    let points = if std::env::var_os("SEEX_DIAGNOSTIC_NATIVE_LTTB").is_some() {
        let mut reducer =
            OrderedLttb::<StoredAlignedPoint>::new(source_row_count as usize, max_points);
        let mut consumed = 0_u64;
        for batch in batches {
            consumed += reduce_batch_native(&batch, &mut reducer)?;
        }
        check_batch_row_count(consumed, statement.row_count())?;
        reducer
            .finish()
            .map_err(native_lttb_error)?
            .into_iter()
            .map(|point| point.into_aligned_metric_point(&query.run_id, &query.metric_key))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let mut reducer = ScreenReducer::new(max_points, source_row_count);
        for batch in batches {
            reduce_batch(&batch, &mut reducer)?;
        }
        check_batch_row_count(reducer.ordinal, statement.row_count())?;
        reducer
            .finish()
            .into_iter()
            .map(|point| point.into_aligned_metric_point(&query.run_id, &query.metric_key))
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(AlignmentQueryResult {
        points,
        source_row_count,
        reasons: Vec::new(),
    })
}

fn native_lttb_error(error: crate::storage::lttb::SelectionError) -> StorageError {
    StorageError::Storage {
        operation: "native LTTB selection",
        name: format!("{error:?}"),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "native LTTB rejected input",
        ),
    }
}

impl Point for StoredAlignedPoint {
    fn axis(&self) -> i64 {
        self.axis_value
    }
    fn value(&self) -> f64 {
        self.value_f64
    }
}

fn reduce_batch_native(
    batch: &duckdb::arrow::record_batch::RecordBatch,
    reducer: &mut OrderedLttb<StoredAlignedPoint>,
) -> Result<u64, StorageError> {
    use duckdb::arrow::array::{Array, Float64Array, Int64Array};
    if batch.num_columns() != 5 {
        return Err(native_lttb_error(
            crate::storage::lttb::SelectionError::ArithmeticOverflow,
        ));
    }
    let ints = [0, 1, 3, 4].map(|i| batch.column(i).as_any().downcast_ref::<Int64Array>());
    let [Some(step), Some(timestamp), Some(ingested), Some(axis)] = ints else {
        return Err(native_lttb_error(
            crate::storage::lttb::SelectionError::ArithmeticOverflow,
        ));
    };
    let Some(value) = batch.column(2).as_any().downcast_ref::<Float64Array>() else {
        return Err(native_lttb_error(
            crate::storage::lttb::SelectionError::ArithmeticOverflow,
        ));
    };
    if step.null_count()
        + timestamp.null_count()
        + ingested.null_count()
        + axis.null_count()
        + value.null_count()
        != 0
    {
        return Err(native_lttb_error(
            crate::storage::lttb::SelectionError::NonFiniteValue { index: 0 },
        ));
    }
    for i in 0..batch.num_rows() {
        reducer
            .push(StoredAlignedPoint {
                step: step.value(i),
                timestamp_millis: timestamp.value(i),
                value_f64: value.value(i),
                ingested_at_millis: ingested.value(i),
                axis_value: axis.value(i),
            })
            .map_err(native_lttb_error)?;
    }
    Ok(batch.num_rows() as u64)
}

fn check_batch_row_count(consumed: u64, expected: usize) -> Result<(), StorageError> {
    if consumed != expected as u64 {
        return Err(batch_decode_error("incomplete bounded result"));
    }
    Ok(())
}

fn ensure_lttb_detail_extension(connection: &Connection) -> Result<(), StorageError> {
    crate::storage::metric_query::ensure_lttb_extension_loaded(connection)?;
    let available: bool = connection.query_row(
        "SELECT count(*) > 0 FROM duckdb_functions() WHERE function_name = 'lttb_indices'",
        [],
        |row| row.get(0),
    )?;
    if available {
        Ok(())
    } else {
        Err(batch_decode_error("lttb_indices was not registered"))
    }
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
    let strategy = screen_strategy(query.axis, plan, screen_limits.is_some());
    if strategy == ScreenStrategy::Lttb {
        ensure_lttb_detail_extension(connection)?;
    }
    let sql = aligned_points_sql(source, query.axis, screen_limits.is_some(), plan, strategy);
    let values = query_values(source, query, run_start_millis, screen_limits, plan)?;
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

fn screen_strategy(axis: AlignmentAxis, plan: AlignmentPlan, reduced: bool) -> ScreenStrategy {
    if !reduced || axis != AlignmentAxis::Step || !matches!(plan, AlignmentPlan::NarrowStep) {
        return ScreenStrategy::Default;
    }
    if std::env::var_os("SEEX_DIAGNOSTIC_LTTB_DETAIL").is_some() {
        ScreenStrategy::Lttb
    } else if std::env::var_os("SEEX_DIAGNOSTIC_GROUPED_DETAIL").is_some() {
        ScreenStrategy::Grouped
    } else {
        ScreenStrategy::Default
    }
}

fn aligned_points_sql(
    source: AlignmentSource<'_>,
    axis: AlignmentAxis,
    screen_reduced: bool,
    plan: AlignmentPlan,
    strategy: ScreenStrategy,
) -> String {
    let selection = if strategy == ScreenStrategy::Lttb {
        lttb_step_selection()
    } else if strategy == ScreenStrategy::Grouped {
        grouped_step_selection()
    } else if screen_reduced {
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

fn lttb_step_selection() -> &'static str {
    "screen_limit AS (SELECT ?::UBIGINT AS bucket_count, ?::BIGINT AS max_points),
     normalized AS MATERIALIZED (
         SELECT *, row_number() OVER (ORDER BY step) - 1 AS ordinal,
             CASE WHEN max(step::HUGEINT) OVER () - min(step::HUGEINT) OVER () > 9007199254740992
                  THEN error('LTTB experiment Step span exceeds exact coordinate range')
                  ELSE step::HUGEINT - min(step::HUGEINT) OVER () END AS lttb_step
         FROM inside WHERE NOT isnan(value_f64)
     ),
     sampled AS (
         SELECT unnest(lttb_indices(lttb_step, value_f64,
                    (SELECT max_points FROM screen_limit)
                    - (SELECT count(*) FROM left_neighbor)
                    - (SELECT count(*) FROM right_neighbor)
                    ORDER BY step, ordinal)) AS ordinal
         FROM normalized
     ),
     selected AS (
         SELECT normalized.* EXCLUDE (ordinal, lttb_step)
         FROM normalized JOIN sampled USING (ordinal)
         UNION ALL SELECT * FROM left_neighbor
         UNION ALL SELECT * FROM right_neighbor
     )"
}

fn grouped_step_selection() -> &'static str {
    "inside_numbered AS (
         SELECT *, row_number() OVER (ORDER BY step) - 1 AS ordinal,
                count(*) OVER ()::UBIGINT AS inside_count FROM inside
     ),
     inside_bucketed AS (
         SELECT *, floor(ordinal::DOUBLE * ?::DOUBLE / inside_count::DOUBLE)::UBIGINT AS bucket
         FROM inside_numbered
     ),
     screen_limit AS (SELECT ?::UBIGINT AS max_points),
     payloads AS (
         SELECT *, struct_pack(step := step, \"timestamp\" := timestamp, value_f64 := value_f64,
             ingested_at := ingested_at, axis_value := axis_value,
             previous_axis_value := previous_axis_value, ordinal := ordinal) AS payload
         FROM inside_bucketed
     ),
     reduced AS (
         SELECT first(payload ORDER BY axis_value, step) AS first_point,
                first(payload ORDER BY axis_value DESC, step DESC) AS last_point,
                first(payload ORDER BY value_f64, axis_value, step) AS min_point,
                first(payload ORDER BY value_f64 DESC, axis_value, step) AS max_point
         FROM payloads WHERE inside_count > (SELECT max_points FROM screen_limit)
         GROUP BY bucket
     ),
     picked AS (
         SELECT DISTINCT unnest([first_point, last_point, min_point, max_point]) AS point FROM reduced
     ),
     selected AS (
         SELECT point.* EXCLUDE (ordinal) FROM picked
         UNION ALL
         SELECT * EXCLUDE (ordinal, inside_count) FROM inside_numbered
         WHERE inside_count <= (SELECT max_points FROM screen_limit)
         UNION ALL SELECT * FROM left_neighbor
         UNION ALL SELECT * FROM right_neighbor
     )"
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
    fn batch_decoder_handles_multiple_batches_empty_and_malformed_results()
    -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        let sql = "SELECT n // 2 AS step, 20000-n, (n % 97)::DOUBLE, n+30000, n // 2
                   FROM range(10000) r(n) ORDER BY n";
        let mut expected = ScreenReducer::new(1024, 10000);
        for row in connection
            .prepare(sql)?
            .query_map([], stored_bounded_alignment_row)?
        {
            expected.push(row?);
        }
        let mut statement = connection.prepare(sql)?;
        let mut actual = ScreenReducer::new(1024, 10000);
        let mut batches = 0;
        for batch in statement.query_arrow([])? {
            batches += 1;
            reduce_batch(&batch, &mut actual)?;
        }
        assert!(batches > 1);
        check_batch_row_count(actual.ordinal, statement.row_count())?;
        assert!(check_batch_row_count(actual.ordinal - 1, statement.row_count()).is_err());
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
        assert_eq!(payloads(actual.finish()), payloads(expected.finish()));
        for sql in [
            "SELECT 1",
            "SELECT 'bad', 1::BIGINT, 1.0::DOUBLE, 1::BIGINT, 1::BIGINT",
        ] {
            for batch in connection.prepare(sql)?.query_arrow([])? {
                assert!(reduce_batch(&batch, &mut ScreenReducer::new(4, 1)).is_err());
            }
        }
        let mut empty = connection.prepare(
            "SELECT 1::BIGINT, 1::BIGINT, 1.0::DOUBLE, 1::BIGINT, 1::BIGINT WHERE false",
        )?;
        let mut reducer = ScreenReducer::new(4, 0);
        for batch in empty.query_arrow([])? {
            reduce_batch(&batch, &mut reducer)?;
        }
        check_batch_row_count(reducer.ordinal, empty.row_count())?;
        assert!(reducer.finish().is_empty());
        Ok(())
    }

    #[test]
    fn batch_query_propagates_interrupt_and_connection_recovers() -> Result<(), Box<dyn Error>> {
        let connection = duckdb::Connection::open_in_memory()?;
        let interrupt = connection.interrupt_handle();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let outcome = connection
                .prepare(
                    "SELECT sum(a.i*b.j)::DOUBLE FROM range(10000000) a(i), range(1000000) b(j)",
                )
                .and_then(|mut statement| statement.query_arrow([]).map(|_| ()));
            tx.send(()).expect("receiver alive");
            (connection, outcome)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            interrupt.interrupt();
            if rx.recv_timeout(std::time::Duration::from_millis(1)).is_ok() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "interrupt did not stop Arrow execution"
            );
        }
        let (connection, outcome) = worker.join().expect("query worker must not panic");
        assert!(
            outcome
                .expect_err("cancelled query must fail")
                .to_string()
                .to_lowercase()
                .contains("interrupt")
        );
        assert_eq!(
            connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?,
            1
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires a locally maintained lttb_indices DuckDB extension"]
    fn lttb_detail_retains_real_observations_and_neighbors() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory_with_flags(
            duckdb::Config::default().allow_unsigned_extensions()?,
        )?;
        connection.execute_batch(
            "CREATE SCHEMA dl;
             CREATE TABLE dl.metric_points AS
             SELECT 'run-1' AS run_id, 'loss' AS metric_key, 'loss' AS metric_key_encoded,
                    step::BIGINT AS step, epoch_ms(1000+step) AS timestamp,
                    step::DOUBLE AS value_f64, epoch_ms(2000+step) AS ingested_at
             FROM range(0, 7) generated(step);
             INSERT INTO dl.metric_points VALUES ('run-1','loss','loss',3,epoch_ms(1030),-1.0,epoch_ms(3000));"
        )?;
        ensure_lttb_detail_extension(&connection)?;
        connection.execute_batch(
            "INSERT INTO dl.metric_points SELECT * FROM dl.metric_points WHERE step=3;",
        )?;
        let run =
            |start, end, budget| -> Result<(u64, Vec<StoredAlignedPoint>), Box<dyn Error>> {
                let mut screen = query(
                    AlignmentAxis::Step,
                    AlignmentReduction::screen_budget(budget, 1)?,
                );
                screen.viewport = AlignmentViewport::new(start, end)?;
                let limits = screen.reduction.max_points().unwrap();
                let values = query_values(
                    AlignmentSource::Project,
                    &screen,
                    None,
                    Some(((limits / 4).max(1), limits)),
                    AlignmentPlan::NarrowStep,
                )?;
                let sql = aligned_points_sql(
                    AlignmentSource::Project,
                    AlignmentAxis::Step,
                    true,
                    AlignmentPlan::NarrowStep,
                    ScreenStrategy::Lttb,
                );
                assert!(sql.contains("ORDER BY step, ordinal"));
                let mut statement = connection.prepare(&sql)?;
                let rows = statement.query_map(
                    duckdb::params_from_iter(values.iter().map(|v| v.as_ref())),
                    stored_alignment_row,
                )?;
                let mut points = Vec::new();
                let mut count = 0;
                for row in rows {
                    let (point, source) = row?;
                    count = source;
                    points.extend(point);
                }
                assert!(points.len() <= limits);
                let mut statement = connection.prepare(
                    "SELECT step, epoch_ms(timestamp), value_f64, epoch_ms(ingested_at), step
                 FROM dl.metric_points ORDER BY step",
                )?;
                let mut available = statement
                    .query_map([], stored_bounded_alignment_row)?
                    .collect::<Result<Vec<_>, _>>()?;
                for selected in &points {
                    let index = available.iter().position(|original|
                    original.step == selected.step
                    && original.timestamp_millis == selected.timestamp_millis
                    && original.ingested_at_millis == selected.ingested_at_millis
                    && (original.value_f64 == selected.value_f64
                        || (original.value_f64.is_nan() && selected.value_f64.is_nan()))
                ).expect("selected payload must be a real observation with available multiplicity");
                    available.remove(index);
                }
                Ok((count, points))
            };
        let (count, points) = run(2, 4, 100)?;
        assert_eq!(count, 8);
        let steps = points.iter().map(|p| p.step).collect::<Vec<_>>();
        assert_eq!(steps, vec![1, 2, 3, 3, 3, 3, 4, 5]);
        for budget in [4, 5, 8] {
            let (count, points) = run(2, 4, budget)?;
            assert_eq!(count, 8);
            assert_eq!(points.first().map(|p| p.step), Some(1));
            assert_eq!(points.last().map(|p| p.step), Some(5));
        }
        connection.execute_batch("UPDATE dl.metric_points SET step=step+9007199254740993;")?;
        let (_, large) = run(9007199254740995, 9007199254740997, 100)?;
        assert_eq!(
            large
                .iter()
                .map(|p| p.step - 9007199254740993)
                .collect::<Vec<_>>(),
            steps
        );
        connection.execute_batch(
            "UPDATE dl.metric_points SET step=step-9007199254740993, value_f64='NaN'::DOUBLE;",
        )?;
        let (count, empty) = run(2, 4, 100)?;
        assert_eq!(count, 8);
        assert_eq!(
            empty.len(),
            2,
            "NaN sampling may be empty, but neighbors and count survive"
        );
        connection.execute_batch(
            "UPDATE dl.metric_points SET value_f64=CASE WHEN step=2 THEN '-0.0'::DOUBLE
                WHEN step=4 THEN 'Infinity'::DOUBLE ELSE 0.0 END;",
        )?;
        // DuckDB may canonicalize signed zero; this strategy promises numeric, not bitwise, y identity.
        let (count, mixed) = run(2, 4, 100)?;
        assert_eq!(count, 8);
        assert_eq!(mixed.len(), 8);
        assert!(mixed.iter().any(|p| p.step == 2 && p.value_f64 == 0.0));
        assert!(
            mixed
                .iter()
                .any(|p| p.step == 4 && p.value_f64 == f64::INFINITY)
        );
        for budget in [4, 8] {
            assert_eq!(run(2, 4, budget)?.0, 8);
        }
        // Indices refer to the filtered, sorted sequence, not the pre-NaN input positions.
        connection.execute_batch(
            "UPDATE dl.metric_points SET value_f64=CASE WHEN step=2
            THEN 'NaN'::DOUBLE ELSE step::DOUBLE END;",
        )?;
        let (count, filtered) = run(2, 4, 100)?;
        assert_eq!(count, 8);
        assert_eq!(filtered.len(), 7);
        assert!(!filtered.iter().any(|point| point.step == 2));
        for budget in [4, 5] {
            let (count, filtered) = run(2, 4, budget)?;
            assert_eq!(count, 8);
            assert!(!filtered.iter().any(|point| point.value_f64.is_nan()));
            assert_eq!(filtered.first().map(|p| p.step), Some(1));
            assert_eq!(filtered.last().map(|p| p.step), Some(5));
        }
        connection.execute_batch(
            "UPDATE dl.metric_points SET value_f64=1.0;
            UPDATE dl.metric_points SET step=9007199254741000 WHERE step=6;",
        )?;
        assert!(
            run(0, i64::MAX, 100)
                .err()
                .expect("oversized span must fail")
                .to_string()
                .contains("Step span exceeds exact coordinate range")
        );
        connection.execute_batch("DELETE FROM dl.metric_points;")?;
        let (count, empty) = run(2, 4, 100)?;
        assert_eq!(count, 0);
        assert!(empty.is_empty());
        Ok(())
    }

    #[test]
    #[ignore = "actual-extension shape-quality experiment; writes ignored CSV artifacts"]
    fn lttb_shape_quality_experiment() -> Result<(), Box<dyn Error>> {
        use std::io::Write;
        let output = std::path::PathBuf::from(std::env::var("SEEX_LTTB_QUALITY_OUTPUT")?);
        if !output.ends_with(".tmp/v5-lttb-quality") {
            return Err("quality output must be .tmp/v5-lttb-quality".into());
        }
        std::fs::create_dir_all(&output)?;
        let connection = Connection::open_in_memory_with_flags(
            duckdb::Config::default().allow_unsigned_extensions()?,
        )?;
        ensure_lttb_detail_extension(&connection)?;
        connection.execute_batch("CREATE SCHEMA dl;")?;
        for (name, expression) in [
            ("smooth", "sin(step::DOUBLE / 300)"),
            (
                "spikes",
                "CASE WHEN step IN (1234, 2811) THEN 100.0 ELSE 0.0 END",
            ),
            (
                "troughs",
                "CASE WHEN step IN (1234, 2811) THEN -100.0 ELSE 0.0 END",
            ),
            ("step", "CASE WHEN step < 2048 THEN 0.0 ELSE 1.0 END"),
            (
                "oscillation",
                "sin(step::DOUBLE / 7) + 0.2 * sin(step::DOUBLE / 2)",
            ),
            ("duplicates", "sin(step::DOUBLE / 300)"),
        ] {
            connection.execute_batch(&format!(
                "CREATE OR REPLACE TABLE dl.metric_points AS
                 SELECT 'run-1' AS run_id, 'loss' AS metric_key, 'loss' AS metric_key_encoded,
                        step::BIGINT AS step, epoch_ms(step) AS timestamp,
                        ({expression})::DOUBLE AS value_f64, epoch_ms(step) AS ingested_at
                 FROM range(0,4096) generated(step);"
            ))?;
            if name == "duplicates" {
                connection.execute_batch(
                    "INSERT INTO dl.metric_points VALUES
                     ('run-1','loss','loss',2048,epoch_ms(5000),8.0,epoch_ms(5000)),
                     ('run-1','loss','loss',2048,epoch_ms(5001),-8.0,epoch_ms(5001)),
                     ('run-1','loss','loss',2048,epoch_ms(5001),-8.0,epoch_ms(5001));",
                )?;
            }
            let source = connection
                .prepare(
                    "SELECT step, epoch_ms(timestamp), value_f64, epoch_ms(ingested_at), step
                 FROM dl.metric_points WHERE step BETWEEN 99 AND 3997 ORDER BY step, timestamp",
                )?
                .query_map([], stored_bounded_alignment_row)?
                .collect::<Result<Vec<_>, _>>()?;
            let source_count = source.len() as u64;
            let mut csv = std::fs::File::create(output.join(format!("{name}.csv")))?;
            writeln!(csv, "strategy,step,timestamp,value,ingested_at")?;
            for point in &source {
                writeln!(
                    csv,
                    "source,{},{},{},{}",
                    point.step, point.timestamp_millis, point.value_f64, point.ingested_at_millis
                )?;
            }
            for strategy in [ScreenStrategy::Default, ScreenStrategy::Lttb] {
                let mut screen = query(
                    AlignmentAxis::Step,
                    AlignmentReduction::screen_budget(64, 1)?,
                );
                screen.viewport = AlignmentViewport::new(100, 3996)?;
                let values = query_values(
                    AlignmentSource::Project,
                    &screen,
                    None,
                    Some((16, 64)),
                    AlignmentPlan::NarrowStep,
                )?;
                let sql = aligned_points_sql(
                    AlignmentSource::Project,
                    AlignmentAxis::Step,
                    true,
                    AlignmentPlan::NarrowStep,
                    strategy,
                );
                let mut statement = connection.prepare(&sql)?;
                let mut selected = Vec::new();
                for row in statement.query_map(
                    duckdb::params_from_iter(values.iter().map(|v| v.as_ref())),
                    stored_alignment_row,
                )? {
                    let (point, count) = row?;
                    assert_eq!(count, source_count);
                    selected.extend(point);
                }
                // Apply the same final neighbor/budget handling as Reader.
                let aligned = selected
                    .into_iter()
                    .map(|p| p.into_aligned_metric_point(&screen.run_id, &screen.metric_key))
                    .collect::<Result<Vec<_>, _>>()?;
                let selected = crate::reader::bound_quality_samples(aligned, 100, 3997, 64);
                assert!(!selected.is_empty() && selected.len() <= 64);
                assert_eq!(selected.first().map(|p| p.point.step.value()), Some(99));
                assert_eq!(selected.last().map(|p| p.point.step.value()), Some(3997));
                let mut remaining = source.iter().collect::<Vec<_>>();
                for point in &selected {
                    let p = &point.point;
                    let index = remaining
                        .iter()
                        .position(|original| {
                            original.step == p.step.value()
                                && original.timestamp_millis == p.timestamp.timestamp_millis()
                                && original.value_f64 == p.value_f64
                                && original.ingested_at_millis == p.ingested_at.timestamp_millis()
                        })
                        .expect(
                            "quality samples must preserve whole observations and multiplicity",
                        );
                    remaining.remove(index);
                    writeln!(
                        csv,
                        "{strategy:?},{},{},{},{}",
                        p.step.value(),
                        p.timestamp.timestamp_millis(),
                        p.value_f64,
                        p.ingested_at.timestamp_millis()
                    )?;
                }
                println!(
                    "quality_case={name},strategy={strategy:?},source_count={source_count},selected={}",
                    selected.len()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn grouped_detail_matches_windowed_observations() -> Result<(), Box<dyn Error>> {
        let connection = connection()?;
        connection.execute_batch(
            "INSERT INTO dl.metric_points SELECT * FROM dl.metric_points WHERE step=3;",
        )?;
        for (start, end, key) in [
            (0, 6, "loss"),
            (2, 4, "loss"),
            (3, 3, "loss"),
            (-10, -5, "loss"),
            (20, 30, "loss"),
            (2, 4, "missing"),
        ] {
            for width in [1, 2, 100] {
                let mut screen = query(
                    AlignmentAxis::Step,
                    AlignmentReduction::screen_budget(width, 1)?,
                );
                screen.viewport = AlignmentViewport::new(start, end)?;
                screen.metric_key = MetricKey::from_string(key);
                let max = screen.reduction.max_points().unwrap();
                let values = query_values(
                    AlignmentSource::Project,
                    &screen,
                    None,
                    Some(((max / 4).max(1), max)),
                    AlignmentPlan::NarrowStep,
                )?;
                let mut results = Vec::new();
                for grouped in [false, true] {
                    let sql = aligned_points_sql(
                        AlignmentSource::Project,
                        AlignmentAxis::Step,
                        true,
                        AlignmentPlan::NarrowStep,
                        if grouped {
                            ScreenStrategy::Grouped
                        } else {
                            ScreenStrategy::Default
                        },
                    );
                    let mut statement = connection.prepare(&sql)?;
                    let rows = statement.query_map(
                        duckdb::params_from_iter(values.iter().map(|v| v.as_ref())),
                        |row| {
                            let (point, count) = stored_alignment_row(row)?;
                            Ok((
                                point.map(|p| {
                                    (
                                        p.step,
                                        p.timestamp_millis,
                                        p.value_f64.to_bits(),
                                        p.ingested_at_millis,
                                        p.axis_value,
                                    )
                                }),
                                count,
                            ))
                        },
                    )?;
                    let mut rows = rows.collect::<Result<Vec<_>, _>>()?;
                    rows.sort();
                    results.push(rows);
                }
                assert_eq!(
                    results[0], results[1],
                    "viewport [{start},{end}], width {width}"
                );
            }
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
            ScreenStrategy::Default,
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
            ScreenStrategy::Default,
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
