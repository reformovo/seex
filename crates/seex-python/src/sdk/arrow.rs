use std::sync::Arc;

use arrow::array::{
    ArrayRef, Float64Array, Int64Array, StringArray, TimestampMillisecondArray, UInt64Array,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::error::ArrowError;
use arrow::ffi_stream::FFI_ArrowArrayStream;
use arrow::record_batch::{RecordBatch, RecordBatchIterator};
use pyo3::prelude::*;
use pyo3::types::PyCapsule;

use crate::model::metric::{MetricAggregate, MetricPoint};

#[pyclass(name = "ArrowTable", module = "seex._seex")]
pub struct PyArrowTable {
    batch: RecordBatch,
    source_row_count: u64,
    downsampled: bool,
}

impl PyArrowTable {
    pub fn from_metric_points(
        points: &[MetricPoint],
        source_row_count: u64,
        downsampled: bool,
    ) -> Result<Self, ArrowError> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("metric_key", DataType::Utf8, false),
            Field::new("step", DataType::Int64, false),
            Field::new(
                "timestamp",
                DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
                false,
            ),
            Field::new("value_f64", DataType::Float64, false),
            Field::new(
                "ingested_at",
                DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
                false,
            ),
        ]));
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                points.iter().map(|point| point.run_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                points.iter().map(|point| point.metric_key.as_str()),
            )),
            Arc::new(Int64Array::from_iter_values(
                points.iter().map(|point| point.step.value()),
            )),
            Arc::new(
                TimestampMillisecondArray::from_iter_values(
                    points
                        .iter()
                        .map(|point| point.timestamp.timestamp_millis()),
                )
                .with_timezone("UTC"),
            ),
            Arc::new(Float64Array::from_iter_values(
                points.iter().map(|point| point.value_f64),
            )),
            Arc::new(
                TimestampMillisecondArray::from_iter_values(
                    points
                        .iter()
                        .map(|point| point.ingested_at.timestamp_millis()),
                )
                .with_timezone("UTC"),
            ),
        ];
        Ok(Self {
            batch: RecordBatch::try_new(schema, columns)?,
            source_row_count,
            downsampled,
        })
    }

    pub fn from_metric_summaries(summaries: &[MetricAggregate]) -> Result<Self, ArrowError> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("run_id", DataType::Utf8, false),
            Field::new("metric_key", DataType::Utf8, false),
            Field::new("effective_count", DataType::UInt64, false),
            Field::new("last_step", DataType::Int64, false),
            Field::new("last_value_f64", DataType::Float64, false),
            Field::new("min_value_f64", DataType::Float64, false),
            Field::new("max_value_f64", DataType::Float64, false),
        ]));
        let columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from_iter_values(
                summaries.iter().map(|summary| summary.run_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                summaries.iter().map(|summary| summary.metric_key.as_str()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                summaries.iter().map(|summary| summary.effective_count),
            )),
            Arc::new(Int64Array::from_iter_values(
                summaries.iter().map(|summary| summary.last_step.value()),
            )),
            Arc::new(Float64Array::from_iter_values(
                summaries.iter().map(|summary| summary.last_value_f64),
            )),
            Arc::new(Float64Array::from_iter_values(
                summaries.iter().map(|summary| summary.min_value_f64),
            )),
            Arc::new(Float64Array::from_iter_values(
                summaries.iter().map(|summary| summary.max_value_f64),
            )),
        ];
        Ok(Self {
            batch: RecordBatch::try_new(schema, columns)?,
            source_row_count: summaries.len() as u64,
            downsampled: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use arrow::array::TimestampMillisecondArray;
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::model::metric::{MetricKey, Step};
    use crate::model::run::RunId;

    #[test]
    fn metric_point_table_uses_public_schema_without_partition_key() -> Result<(), ArrowError> {
        let table = PyArrowTable::from_metric_points(&[], 0, false)?;
        let fields = table.batch.schema_ref().fields();
        let actual: Vec<(&str, DataType)> = fields
            .iter()
            .map(|field| (field.name().as_str(), field.data_type().clone()))
            .collect();

        assert_eq!(
            actual,
            vec![
                ("run_id", DataType::Utf8),
                ("metric_key", DataType::Utf8),
                ("step", DataType::Int64),
                (
                    "timestamp",
                    DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
                ),
                ("value_f64", DataType::Float64),
                (
                    "ingested_at",
                    DataType::Timestamp(TimeUnit::Millisecond, Some("UTC".into())),
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn metric_summary_table_matches_public_query_model() -> Result<(), ArrowError> {
        let table = PyArrowTable::from_metric_summaries(&[])?;
        let actual: Vec<(&str, DataType)> = table
            .batch
            .schema_ref()
            .fields()
            .iter()
            .map(|field| (field.name().as_str(), field.data_type().clone()))
            .collect();

        assert_eq!(
            actual,
            vec![
                ("run_id", DataType::Utf8),
                ("metric_key", DataType::Utf8),
                ("effective_count", DataType::UInt64),
                ("last_step", DataType::Int64),
                ("last_value_f64", DataType::Float64),
                ("min_value_f64", DataType::Float64),
                ("max_value_f64", DataType::Float64),
            ]
        );
        Ok(())
    }

    #[test]
    fn metric_point_table_stores_utc_millisecond_timestamps() -> Result<(), ArrowError> {
        let timestamp = Utc.timestamp_millis_opt(1_750_000_000_123).unwrap();
        let point = MetricPoint {
            run_id: RunId::from_string("run-1"),
            metric_key: MetricKey::from_string("train/loss"),
            step: Step::new(7),
            timestamp,
            value_f64: 0.25,
            ingested_at: timestamp,
        };
        let table = PyArrowTable::from_metric_points(&[point], 1, false)?;
        let timestamps = table.batch.column(3);
        let timestamps = timestamps
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .expect("timestamp column must use millisecond Arrow storage");

        assert_eq!(timestamps.value(0), 1_750_000_000_123);
        Ok(())
    }
}

#[pymethods]
impl PyArrowTable {
    #[getter]
    fn row_count(&self) -> usize {
        self.batch.num_rows()
    }

    #[getter]
    const fn source_row_count(&self) -> u64 {
        self.source_row_count
    }

    #[getter]
    const fn downsampled(&self) -> bool {
        self.downsampled
    }

    #[getter]
    fn column_names(&self) -> Vec<&str> {
        self.batch
            .schema_ref()
            .fields()
            .iter()
            .map(|field| field.name().as_str())
            .collect()
    }

    #[pyo3(signature = (requested_schema=None))]
    fn __arrow_c_stream__<'py>(
        &self,
        py: Python<'py>,
        requested_schema: Option<&Bound<'py, PyCapsule>>,
    ) -> PyResult<Bound<'py, PyCapsule>> {
        let _ = requested_schema;
        let schema = self.batch.schema();
        let batches = vec![Ok::<_, ArrowError>(self.batch.clone())];
        let reader = RecordBatchIterator::new(batches.into_iter(), schema);
        PyCapsule::new_with_value(
            py,
            FFI_ArrowArrayStream::new(Box::new(reader)),
            c"arrow_array_stream",
        )
    }
}
