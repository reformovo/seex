//! Arrow result consumption shared by native LTTB query paths.
//! The SQL shape is five point columns after non-finite values are filtered out,
//! so the materialized row count is the reducible point count.
use super::{
    StorageError,
    lttb::{OrderedLttb, Point},
};
use duckdb::{
    Statement, ToSql,
    arrow::{
        array::{Array, Float64Array, Int64Array},
        record_batch::RecordBatch,
    },
};

#[derive(Clone, Debug)]
pub(crate) struct Observation {
    pub step: i64,
    pub timestamp_millis: i64,
    pub value_f64: f64,
    pub ingested_at_millis: i64,
    pub axis_value: i64,
}
impl Point for Observation {
    fn axis(&self) -> i64 {
        self.axis_value
    }
    fn value(&self) -> f64 {
        self.value_f64
    }
}

pub(crate) struct Selection {
    pub points: Vec<Observation>,
    pub source_count: u64,
}

pub(crate) struct SelectionTimings {
    pub execute: std::time::Duration,
    pub consume: std::time::Duration,
}

fn invalid(message: impl ToString) -> StorageError {
    StorageError::Storage {
        operation: "native LTTB result selection",
        name: "Arrow query result".into(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_string()),
    }
}
fn integer(batch: &RecordBatch, index: usize) -> Result<&Int64Array, StorageError> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| invalid("expected BIGINT point column"))
}

pub(crate) fn select(
    statement: &mut Statement<'_>,
    values: &[Box<dyn ToSql>],
    budget: usize,
) -> Result<Selection, StorageError> {
    select_timed(statement, values, budget).map(|(selection, _)| selection)
}

pub(crate) fn select_timed(
    statement: &mut Statement<'_>,
    values: &[Box<dyn ToSql>],
    budget: usize,
) -> Result<(Selection, SelectionTimings), StorageError> {
    let started = std::time::Instant::now();
    statement.execute(duckdb::params_from_iter(values.iter().map(|v| v.as_ref())))?;
    let expected_rows = statement.row_count();
    let execute = started.elapsed();
    super::bootstrap::record_write_timing("native_execute", execute);
    let started = std::time::Instant::now();
    let mut consumed = 0;
    // Same materialized result API as query_arrow, but exposes the exact result
    // row count before selection. A failed fetch cannot masquerade as complete EOF.
    let mut reducer = OrderedLttb::new(expected_rows, budget);
    while let Some(chunk) = statement.step() {
        let batch = RecordBatch::from(&chunk);
        if batch.num_rows() == 0 {
            continue;
        }
        if batch.num_columns() != 5 {
            return Err(invalid("expected five result columns"));
        }
        let cols = [
            integer(&batch, 0)?,
            integer(&batch, 1)?,
            integer(&batch, 3)?,
            integer(&batch, 4)?,
        ];
        let ys = batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| invalid("expected DOUBLE values"))?;
        for index in 0..batch.num_rows() {
            if cols.iter().any(|column| column.is_null(index)) || ys.is_null(index) {
                return Err(invalid("NULL observation field"));
            }
            let point = Observation {
                step: cols[0].value(index),
                timestamp_millis: cols[1].value(index),
                value_f64: ys.value(index),
                ingested_at_millis: cols[2].value(index),
                axis_value: cols[3].value(index),
            };
            reducer
                .push(point)
                .map_err(|error| invalid(format!("{error:?}")))?;
        }
        consumed += batch.num_rows();
    }
    if consumed != expected_rows {
        return Err(invalid("incomplete Arrow result"));
    }
    let points = reducer.finish().map_err(|e| invalid(format!("{e:?}")))?;
    let consume = started.elapsed();
    super::bootstrap::record_write_timing("native_consume", consume);
    Ok((
        Selection {
            points,
            source_count: expected_rows as u64,
        },
        SelectionTimings { execute, consume },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_batches_match_reference_and_preserve_payloads()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = duckdb::Connection::open_in_memory()?;
        let sql = "SELECT n//2, 10000-n, (n%97)::DOUBLE, 20000+n, n//2
                   FROM range(10000) t(n) WHERE n NOT IN (3,5) ORDER BY n";
        let mut probe = connection.prepare(sql)?;
        probe.execute([])?;
        let mut chunks = 0;
        while probe.step().is_some() {
            chunks += 1;
        }
        assert!(chunks > 1, "fixture must cross Arrow batch boundaries");
        let mut statement = connection.prepare(sql)?;
        let selected = select(&mut statement, &[], 64)?;
        let original = (0_i64..10000)
            .filter(|n| *n != 3 && *n != 5)
            .collect::<Vec<_>>();
        let axes = original.iter().map(|n| n / 2).collect::<Vec<_>>();
        let values = original.iter().map(|n| (n % 97) as f64).collect::<Vec<_>>();
        let expected = crate::storage::lttb::select_indices(&axes, &values, 64)
            .map_err(|error| format!("reference selection failed: {error:?}"))?;
        assert_eq!(selected.source_count, 9998);
        assert_eq!(selected.points.len(), expected.len());
        for (point, index) in selected.points.iter().zip(expected) {
            let n = original[index];
            assert_eq!(
                (
                    point.step,
                    point.axis_value,
                    point.timestamp_millis,
                    point.ingested_at_millis
                ),
                (n / 2, n / 2, 10000 - n, 20000 + n)
            );
            assert_eq!(point.value_f64.to_bits(), ((n % 97) as f64).to_bits());
        }
        Ok(())
    }

    #[test]
    fn non_finite_values_are_rejected_when_the_sql_filter_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = duckdb::Connection::open_in_memory()?;
        let sql = "SELECT 1::BIGINT, 1::BIGINT, 'NaN'::DOUBLE, 1::BIGINT, 1::BIGINT";
        let result = select(&mut connection.prepare(sql)?, &[], 4);
        let Err(error) = result else {
            return Err("non-finite input must not be silently reduced".into());
        };
        assert!(format!("{error:?}").contains("NonFiniteValue"));
        Ok(())
    }

    #[test]
    fn rejects_malformed_results_and_accepts_empty_results() -> Result<(), StorageError> {
        let connection = duckdb::Connection::open_in_memory()?;
        for sql in [
            "SELECT 1",
            "SELECT 'bad',1::BIGINT,1.0::DOUBLE,1::BIGINT,1::BIGINT",
            "SELECT 1::BIGINT,1::BIGINT,NULL::DOUBLE,1::BIGINT,1::BIGINT",
            "SELECT NULL::BIGINT,NULL::BIGINT,1.0::DOUBLE,1::BIGINT,1::BIGINT",
            "SELECT 1::BIGINT,1::BIGINT,1.0::DOUBLE,1::BIGINT,1::BIGINT,2::UBIGINT,2::UBIGINT",
        ] {
            assert!(select(&mut connection.prepare(sql)?, &[], 4).is_err());
        }
        let empty = select(
            &mut connection.prepare(
                "SELECT 1::BIGINT,1::BIGINT,1.0::DOUBLE,1::BIGINT,1::BIGINT WHERE false",
            )?,
            &[],
            4,
        )?;
        assert_eq!(empty.source_count, 0);
        assert!(empty.points.is_empty());
        Ok(())
    }
}
