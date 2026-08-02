use std::sync::Arc;

use arrow::array::{ArrayRef, Float64Array, Int64Array, StringArray, TimestampMillisecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::error::ArrowError;
use arrow::ffi_stream::FFI_ArrowArrayStream;
use arrow::record_batch::{RecordBatch, RecordBatchIterator};
use pyo3::prelude::*;
use pyo3::types::PyCapsule;
use seex::MetricSeries;

fn metric_series_batch(series: &MetricSeries) -> Result<RecordBatch, ArrowError> {
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
            series
                .samples()
                .iter()
                .map(|sample| sample.point.run_id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            series
                .samples()
                .iter()
                .map(|sample| sample.point.metric_key.as_str()),
        )),
        Arc::new(Int64Array::from_iter_values(
            series
                .samples()
                .iter()
                .map(|sample| sample.point.step.value()),
        )),
        Arc::new(
            TimestampMillisecondArray::from_iter_values(
                series
                    .samples()
                    .iter()
                    .map(|sample| sample.point.timestamp.timestamp_millis()),
            )
            .with_timezone("UTC"),
        ),
        Arc::new(Float64Array::from_iter_values(
            series.samples().iter().map(|sample| sample.point.value_f64),
        )),
        Arc::new(
            TimestampMillisecondArray::from_iter_values(
                series
                    .samples()
                    .iter()
                    .map(|sample| sample.point.ingested_at.timestamp_millis()),
            )
            .with_timezone("UTC"),
        ),
    ];
    RecordBatch::try_new(schema, columns)
}

pub fn metric_series_stream<'py>(
    py: Python<'py>,
    series: &MetricSeries,
) -> PyResult<Bound<'py, PyCapsule>> {
    let batch = metric_series_batch(series)
        .map_err(|error| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(error.to_string()))?;
    let schema = batch.schema();
    let reader = RecordBatchIterator::new(vec![Ok::<_, ArrowError>(batch)].into_iter(), schema);
    PyCapsule::new_with_value(
        py,
        FFI_ArrowArrayStream::new(Box::new(reader)),
        c"arrow_array_stream",
    )
}

#[cfg(test)]
mod tests {
    use arrow::array::TimestampMillisecondArray;
    use chrono::{TimeZone, Utc};
    use seex::{
        EvidenceCompleteness, MetricAxis, MetricCoordinate, MetricKey, MetricPoint, MetricSample,
        RunId, Step,
    };

    use super::*;

    fn metric_series(points: Vec<MetricPoint>, source_count: u64) -> MetricSeries {
        let samples = points
            .into_iter()
            .map(|point| MetricSample {
                coordinate: MetricCoordinate::Step(point.step),
                point,
            })
            .collect();
        MetricSeries::from_samples(
            MetricAxis::Step,
            samples,
            source_count,
            EvidenceCompleteness::Complete,
            Vec::new(),
        )
        .expect("test series should be valid")
    }

    #[test]
    fn metric_series_uses_the_six_column_public_schema() -> Result<(), ArrowError> {
        let batch = metric_series_batch(&metric_series(Vec::new(), 0))?;
        let actual: Vec<(&str, DataType)> = batch
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
    fn metric_series_stores_utc_millisecond_timestamps() -> Result<(), ArrowError> {
        let timestamp = Utc.timestamp_millis_opt(1_750_000_000_123).unwrap();
        let point = MetricPoint {
            run_id: RunId::from_string("run-1"),
            metric_key: MetricKey::from_string("train/loss"),
            step: Step::new(7),
            timestamp,
            value_f64: 0.25,
            ingested_at: timestamp,
        };
        let series = metric_series(vec![point], 2);
        let batch = metric_series_batch(&series)?;
        let timestamps = batch
            .column(3)
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .expect("timestamp column must use millisecond Arrow storage");

        assert_eq!(timestamps.value(0), 1_750_000_000_123);
        assert_eq!(series.source_count(), 2);
        assert!(series.downsampled());
        Ok(())
    }
}
