//! Python extension adapter for Seex Core.

#![forbid(unsafe_code)]

mod sdk;

use pyo3::prelude::*;

#[pymodule]
fn _seex(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add_class::<sdk::api::PyApi>()?;
    m.add_class::<sdk::api::PyRunRecord>()?;
    m.add_class::<sdk::client::PyDiagnostics>()?;
    m.add_class::<sdk::client::PyMetricPoint>()?;
    m.add_class::<sdk::client::PyMetricSummary>()?;
    m.add_class::<sdk::client::PyProject>()?;
    m.add_class::<sdk::comparison::PyComparisonResult>()?;
    m.add_class::<sdk::comparison::PyObjectiveEvidence>()?;
    m.add_class::<sdk::comparison::PyObjectiveMetric>()?;
    m.add_class::<sdk::comparison::PyRankingEntry>()?;
    m.add_class::<sdk::comparison::PyRankingResult>()?;
    m.add_class::<sdk::history::PyMetricSeries>()?;
    m.add_class::<sdk::settings::PySettings>()?;
    m.add_class::<sdk::run::PyTargetRun>()?;
    m.add("SeexError", py.get_type::<sdk::client::SeexError>())?;
    macro_rules! add_exception {
        ($name:ident) => {
            m.add(stringify!($name), py.get_type::<sdk::client::$name>())?;
        };
    }
    add_exception!(MetricQueueFullError);
    add_exception!(MetricWriterFailedError);
    add_exception!(MetricDrainTimeoutError);
    add_exception!(MetricFlushError);
    add_exception!(MetricFlushTimeoutError);
    add_exception!(RunClosedError);
    add_exception!(InvalidRunStateError);
    add_exception!(RunAlreadyExistsError);
    add_exception!(RunAlreadyActiveError);
    add_exception!(InvalidConfigurationError);
    add_exception!(StorageError);
    add_exception!(ApiClosedError);
    m.add_function(wrap_pyfunction!(sdk::run::start_run, m)?)?;
    Ok(())
}
