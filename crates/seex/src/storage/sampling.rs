//! Arrow result consumption shared by native LTTB query paths.
//! Five point columns are followed, when present, by snapshot-local total,
//! finite-inside and neighbor counts. Only the selected observations survive.
use super::{
    StorageError,
    lttb::{OrderedLttb, Point},
};
use duckdb::{
    Statement, ToSql,
    arrow::{
        array::{Array, Float64Array, Int64Array, UInt64Array},
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
fn count(batch: &RecordBatch, index: usize) -> Result<u64, StorageError> {
    let array = batch
        .column(index)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or_else(|| invalid("expected UBIGINT snapshot count"))?;
    if array.null_count() != 0 {
        return Err(invalid("NULL snapshot count"));
    }
    Ok(array.value(0))
}

pub(crate) fn select(
    statement: &mut Statement<'_>,
    values: &[Box<dyn ToSql>],
    budget: usize,
    viewport: Option<(i64, i64)>,
) -> Result<Selection, StorageError> {
    let started = std::time::Instant::now();
    statement.execute(duckdb::params_from_iter(values.iter().map(|v| v.as_ref())))?;
    let expected_rows = statement.row_count();
    super::bootstrap::record_write_timing("native_execute", started.elapsed());
    let started = std::time::Instant::now();
    let mut consumed = 0;
    let mut stats = None;
    let mut reducer = None;
    let mut neighbors = Vec::new();
    // Same materialized result API as query_arrow, but exposes the exact result
    // row count before selection. A failed fetch cannot masquerade as complete EOF.
    while let Some(chunk) = statement.step() {
        let batch = RecordBatch::from(&chunk);
        if batch.num_rows() == 0 {
            continue;
        }
        if !matches!(batch.num_columns(), 5 | 8) {
            return Err(invalid("expected five or eight result columns"));
        }
        let snapshot = if batch.num_columns() == 5 {
            (expected_rows as u64, expected_rows as u64, 0)
        } else {
            (count(&batch, 5)?, count(&batch, 6)?, count(&batch, 7)?)
        };
        if let Some(previous) = stats {
            if previous != snapshot {
                return Err(invalid("inconsistent snapshot counts"));
            }
        } else {
            if snapshot
                .1
                .checked_add(snapshot.2)
                .is_none_or(|n| n > snapshot.0)
                || snapshot.2 > budget as u64
                || (snapshot.0 != expected_rows as u64 && !(snapshot.0 == 0 && expected_rows == 1))
            {
                return Err(invalid("invalid snapshot counts"));
            }
            let finite =
                usize::try_from(snapshot.1).map_err(|_| invalid("input count overflow"))?;
            reducer = Some(OrderedLttb::new(finite, budget - snapshot.2 as usize));
            stats = Some(snapshot);
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
            if cols[0].is_null(index) {
                continue;
            } // empty result's metadata row
            if cols.iter().any(|c| c.is_null(index)) || ys.is_null(index) {
                return Err(invalid("NULL observation field"));
            }
            let point = Observation {
                step: cols[0].value(index),
                timestamp_millis: cols[1].value(index),
                value_f64: ys.value(index),
                ingested_at_millis: cols[2].value(index),
                axis_value: cols[3].value(index),
            };
            if viewport
                .is_some_and(|(start, end)| point.axis_value < start || point.axis_value > end)
            {
                neighbors.push(point);
            } else if batch.num_columns() == 5 || point.value_f64.is_finite() {
                reducer
                    .as_mut()
                    .expect("snapshot initialized")
                    .push(point)
                    .map_err(|error| invalid(format!("{error:?}")))?;
            }
        }
        consumed += batch.num_rows();
    }
    if consumed != expected_rows {
        return Err(invalid("incomplete Arrow result"));
    }
    let source_count = stats.map_or(0, |s| s.0);
    if neighbors.len() != stats.map_or(0, |s| s.2) as usize {
        return Err(invalid("neighbor count mismatch"));
    }
    let mut points = match reducer {
        Some(reducer) => reducer.finish().map_err(|e| invalid(format!("{e:?}")))?,
        None => Vec::new(),
    };
    points.extend(neighbors);
    points.sort_by_key(|p| p.step);
    super::bootstrap::record_write_timing("native_consume", started.elapsed());
    Ok(Selection {
        points,
        source_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_batches_match_reference_and_preserve_payloads()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = duckdb::Connection::open_in_memory()?;
        let sql = "SELECT n//2, 10000-n,
                   CASE WHEN n=3 THEN 'NaN'::DOUBLE WHEN n=5 THEN 'Infinity'::DOUBLE
                        ELSE (n%97)::DOUBLE END,
                   20000+n, n//2, 10000::UBIGINT, 9998::UBIGINT, 0::UBIGINT
                   FROM range(10000) t(n) ORDER BY n";
        let mut probe = connection.prepare(sql)?;
        probe.execute([])?;
        let mut chunks = 0;
        while probe.step().is_some() {
            chunks += 1;
        }
        assert!(chunks > 1, "fixture must cross Arrow batch boundaries");
        let mut statement = connection.prepare(sql)?;
        let selected = select(&mut statement, &[], 64, None)?;
        let original = (0_i64..10000)
            .filter(|n| *n != 3 && *n != 5)
            .collect::<Vec<_>>();
        let axes = original.iter().map(|n| n / 2).collect::<Vec<_>>();
        let values = original.iter().map(|n| (n % 97) as f64).collect::<Vec<_>>();
        let expected = crate::storage::lttb::select_indices(&axes, &values, 64)
            .map_err(|error| format!("reference selection failed: {error:?}"))?;
        assert_eq!(selected.source_count, 10000);
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
    fn finite_inside_and_nonfinite_neighbor_counts_remain_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = duckdb::Connection::open_in_memory()?;
        let sql = "SELECT n, n+1000,
                   CASE WHEN n=1 THEN 'Infinity'::DOUBLE WHEN n=2 THEN 'NaN'::DOUBLE ELSE n::DOUBLE END,
                   n+2000, n, 5::UBIGINT, 2::UBIGINT, 2::UBIGINT FROM range(1,6) t(n) ORDER BY n";
        let selected = select(&mut connection.prepare(sql)?, &[], 4, Some((2, 4)))?;
        assert_eq!(selected.source_count, 5);
        assert_eq!(
            selected.points.iter().map(|p| p.step).collect::<Vec<_>>(),
            vec![1, 3, 4, 5]
        );
        assert_eq!(selected.points[0].value_f64, f64::INFINITY);
        Ok(())
    }

    #[test]
    fn rejects_malformed_results_but_accepts_empty_metadata() -> Result<(), StorageError> {
        let connection = duckdb::Connection::open_in_memory()?;
        for sql in [
            "SELECT 1",
            "SELECT 'bad',1::BIGINT,1.0::DOUBLE,1::BIGINT,1::BIGINT",
            "SELECT 1::BIGINT,1::BIGINT,NULL::DOUBLE,1::BIGINT,1::BIGINT",
            "SELECT 1::BIGINT,1::BIGINT,1.0::DOUBLE,1::BIGINT,1::BIGINT,2::UBIGINT,2::UBIGINT,0::UBIGINT",
        ] {
            assert!(select(&mut connection.prepare(sql)?, &[], 4, None).is_err());
        }
        let empty = select(
            &mut connection.prepare(
                "SELECT NULL::BIGINT,NULL::BIGINT,NULL::DOUBLE,NULL::BIGINT,NULL::BIGINT,
                    0::UBIGINT,0::UBIGINT,0::UBIGINT",
            )?,
            &[],
            4,
            None,
        )?;
        assert_eq!(empty.source_count, 0);
        assert!(empty.points.is_empty());
        let empty = select(
            &mut connection.prepare(
                "SELECT 1::BIGINT,1::BIGINT,1.0::DOUBLE,1::BIGINT,1::BIGINT WHERE false",
            )?,
            &[],
            4,
            None,
        )?;
        assert_eq!(empty.source_count, 0);
        assert!(empty.points.is_empty());
        Ok(())
    }
}
