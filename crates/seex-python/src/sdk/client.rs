use std::path::PathBuf;
use std::time::Duration;

use pyo3::create_exception;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyAny;

use crate::engine::client::{NativeClient, NativeRun};
use crate::engine::reporting::MetricReporterDiagnostics;
use crate::model::metric::{MetricAggregate, MetricKey, MetricPoint, Step};
use crate::model::run::{RunId, RunStatus};
use crate::model::types::{Project, ProjectId};
use crate::sdk::alignment::{PyAlignedMetricResult, alignment_query};
use crate::sdk::arrow::PyArrowTable;
use crate::sdk::comparison::{
    PyComparisonReport, PyComparisonResult, PyObjectiveEvidence, PyRankingResult, objective,
};
use crate::sdk::compat::query_step_metric;
use seex_core::config::{InitConfigError, S3ConnectionOverrides, resolve_init_config};

create_exception!(
    seex._seex,
    SeexError,
    PyRuntimeError,
    "Base class for Seex SDK errors."
);

macro_rules! sdk_exception {
    ($name:ident, $message:literal) => {
        create_exception!(seex._seex, $name, SeexError, $message);
    };
}

sdk_exception!(MetricQueueFullError, "The metric queue is full.");
sdk_exception!(MetricWriterFailedError, "The metric writer failed.");
sdk_exception!(MetricDrainTimeoutError, "Metric drain timed out.");
sdk_exception!(MetricFlushError, "Metric flush failed.");
sdk_exception!(MetricFlushTimeoutError, "Metric flush timed out.");
sdk_exception!(RunClosedError, "The run is closed for metric reporting.");
sdk_exception!(ClientClosedError, "The client is closed.");
sdk_exception!(
    InvalidRunStateError,
    "The run state does not allow this operation."
);
sdk_exception!(
    RunAlreadyExistsError,
    "A run with the requested run_id already exists."
);
sdk_exception!(
    RunAlreadyActiveError,
    "The requested run already has an active writer."
);
sdk_exception!(InvalidConfigurationError, "Seex configuration is invalid.");
sdk_exception!(StorageError, "A storage operation failed.");

#[pyclass(name = "Client", module = "seex._seex", unsendable)]
pub struct PyClient {
    _inner: NativeClient,
}

impl PyClient {
    fn new(inner: NativeClient) -> Self {
        Self { _inner: inner }
    }
}

#[pymethods]
impl PyClient {
    #[pyo3(signature = (name, project_id=None))]
    pub fn create_project(&self, name: &str, project_id: Option<String>) -> PyResult<PyProject> {
        let project_id = project_id.map(ProjectId::from_string);
        self._inner
            .create_project(name, project_id)
            .map(PyProject::from)
            .map_err(runtime_error)
    }

    pub fn get_project(&self, project_id: &str) -> PyResult<PyProject> {
        let project_id = ProjectId::from_string(project_id);
        self._inner
            .get_project(&project_id)
            .map(PyProject::from)
            .map_err(runtime_error)
    }

    pub fn list_projects(&self) -> PyResult<Vec<PyProject>> {
        self._inner
            .list_projects()
            .map(|projects| projects.into_iter().map(PyProject::from).collect())
            .map_err(runtime_error)
    }

    #[pyo3(signature = (project_id, name, run_id=None))]
    pub fn create_run(
        &self,
        project_id: &str,
        name: &str,
        run_id: Option<String>,
    ) -> PyResult<PyRun> {
        let project_id = ProjectId::from_string(project_id);
        let run_id = run_id.map(RunId::from_string);
        self._inner
            .create_run(&project_id, name, run_id)
            .map(|run| PyRun::from(self._inner.run_handle(run)))
            .map_err(runtime_error)
    }

    pub fn get_run(&self, run_id: &str) -> PyResult<PyRun> {
        let run_id = RunId::from_string(run_id);
        self._inner
            .get_run(&run_id)
            .map(|run| PyRun::from(self._inner.run_handle(run)))
            .map_err(runtime_error)
    }

    pub fn resume_run(&self, run_id: &str) -> PyResult<PyRun> {
        let run_id = RunId::from_string(run_id);
        self._inner
            .resume_run(&run_id)
            .map(|run| PyRun::from(self._inner.run_handle(run)))
            .map_err(runtime_error)
    }

    #[pyo3(signature = (project_id, *, status=None, limit=None, offset=0))]
    pub fn list_runs(
        &self,
        project_id: &str,
        status: Option<&str>,
        limit: Option<usize>,
        offset: usize,
    ) -> PyResult<Vec<PyRun>> {
        let project_id = ProjectId::from_string(project_id);
        let status = parse_run_status(status)?;
        self._inner
            .list_runs_filtered(&project_id, status, limit, offset)
            .map(|runs| {
                runs.into_iter()
                    .map(|run| PyRun::from(self._inner.run_handle(run)))
                    .collect()
            })
            .map_err(runtime_error)
    }

    #[pyo3(signature = (project_id=None))]
    pub fn list_orphan_runs(&self, project_id: Option<String>) -> PyResult<Vec<PyRun>> {
        let project_id = project_id.map(ProjectId::from_string);
        self._inner
            .list_orphan_runs(project_id.as_ref())
            .map(|runs| {
                runs.into_iter()
                    .map(|run| PyRun::from(self._inner.run_handle(run)))
                    .collect()
            })
            .map_err(runtime_error)
    }

    #[pyo3(signature = (run_id, timeout=None))]
    pub fn finish_run(&self, run_id: &str, timeout: Option<f64>) -> PyResult<PyRun> {
        let run_id = RunId::from_string(run_id);
        let timeout = timeout
            .map(|seconds| duration_from_seconds("finish_run timeout", seconds))
            .transpose()?;
        self._inner
            .finish_run_with_timeout(&run_id, timeout)
            .map(|run| PyRun::from(self._inner.run_handle(run)))
            .map_err(runtime_error)
    }

    #[pyo3(signature = (run_id, timeout=None))]
    pub fn fail_run(&self, run_id: &str, timeout: Option<f64>) -> PyResult<PyRun> {
        let run_id = RunId::from_string(run_id);
        let timeout = timeout
            .map(|seconds| duration_from_seconds("fail_run timeout", seconds))
            .transpose()?;
        self._inner
            .fail_run_with_timeout(&run_id, timeout)
            .map(|run| PyRun::from(self._inner.run_handle(run)))
            .map_err(runtime_error)
    }

    #[pyo3(signature = (run_id, timeout=None))]
    pub fn flush_run_data(&self, run_id: &str, timeout: Option<f64>) -> PyResult<()> {
        let run_id = RunId::from_string(run_id);
        let timeout = timeout
            .map(|seconds| duration_from_seconds("flush timeout", seconds))
            .transpose()?;
        self._inner
            .flush_run_data(&run_id, timeout)
            .map_err(runtime_error)
    }

    #[pyo3(signature = (timeout=None))]
    pub fn shutdown(&self, timeout: Option<f64>) -> PyResult<()> {
        let timeout = timeout
            .map(|seconds| duration_from_seconds("shutdown timeout", seconds))
            .transpose()?;
        self._inner.shutdown(timeout).map_err(runtime_error)
    }

    pub fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    pub fn __exit__(
        &self,
        exc_type: &Bound<'_, PyAny>,
        exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        let user_exception_active = !exc_type.is_none();
        match self._inner.shutdown(None) {
            Ok(()) => Ok(false),
            Err(error) if user_exception_active => {
                attach_exception_context(exc_value, runtime_error(error));
                Ok(false)
            }
            Err(error) => Err(runtime_error(error)),
        }
    }

    pub fn diagnostics(&self) -> PyDiagnostics {
        PyDiagnostics::from(self._inner.diagnostics())
    }

    #[pyo3(signature = (run_id, metric_key, *, axis, start, end, pixel_width=None, points_per_pixel=None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "the approved Python API keeps viewport and screen-budget inputs explicit"
    )]
    pub fn query_aligned_metric(
        &self,
        run_id: &str,
        metric_key: &str,
        axis: &str,
        start: i64,
        end: i64,
        pixel_width: Option<u32>,
        points_per_pixel: Option<u16>,
    ) -> PyResult<PyAlignedMetricResult> {
        let query = alignment_query(
            run_id,
            metric_key,
            axis,
            start,
            end,
            pixel_width,
            points_per_pixel,
        )?;
        self._inner
            .query_aligned_metric(&query)
            .map(PyAlignedMetricResult::from)
            .map_err(runtime_error)
    }

    #[pyo3(signature = (candidate_run_id, reference_run_id, *, metric_key, direction))]
    pub fn compare_runs(
        &self,
        candidate_run_id: &str,
        reference_run_id: &str,
        metric_key: &str,
        direction: &str,
    ) -> PyResult<PyComparisonResult> {
        let objective = objective(metric_key, direction)?;
        self._inner
            .compare_runs(
                &RunId::from_string(candidate_run_id),
                &RunId::from_string(reference_run_id),
                &objective,
            )
            .map(PyComparisonResult::from)
            .map_err(runtime_error)
    }

    pub fn _objective_evidence(
        &self,
        run_id: &str,
        metric_key: &str,
    ) -> PyResult<PyObjectiveEvidence> {
        let objective = objective(metric_key, "maximize")?;
        self._inner
            .objective_evidence(&RunId::from_string(run_id), &objective)
            .map(PyObjectiveEvidence::from)
            .map_err(runtime_error)
    }

    #[pyo3(signature = (
        candidate_run_ids,
        reference_run_id,
        *,
        metric_key,
        direction,
        secondary_metric_keys
    ))]
    pub fn _comparison_reports(
        &self,
        candidate_run_ids: Vec<String>,
        reference_run_id: &str,
        metric_key: &str,
        direction: &str,
        secondary_metric_keys: Vec<String>,
    ) -> PyResult<Vec<PyComparisonReport>> {
        let objective = objective(metric_key, direction)?;
        let candidate_run_ids = candidate_run_ids
            .into_iter()
            .map(RunId::from_string)
            .collect::<Vec<_>>();
        let secondary_metric_keys = secondary_metric_keys
            .into_iter()
            .map(MetricKey::from_string)
            .collect::<Vec<_>>();
        self._inner
            .comparison_reports(
                &candidate_run_ids,
                &RunId::from_string(reference_run_id),
                &objective,
                &secondary_metric_keys,
            )
            .map(|reports| reports.into_iter().map(PyComparisonReport::from).collect())
            .map_err(runtime_error)
    }

    #[pyo3(signature = (run_ids, *, metric_key, direction))]
    pub fn rank_runs(
        &self,
        run_ids: Vec<String>,
        metric_key: &str,
        direction: &str,
    ) -> PyResult<PyRankingResult> {
        let objective = objective(metric_key, direction)?;
        let run_ids = run_ids
            .into_iter()
            .map(RunId::from_string)
            .collect::<Vec<_>>();
        self._inner
            .rank_runs(&run_ids, &objective)
            .map(PyRankingResult::from)
            .map_err(runtime_error)
    }

    #[pyo3(signature = (run_ids, *, metric_key, direction))]
    pub fn _best_eligible_run(
        &self,
        run_ids: Vec<String>,
        metric_key: &str,
        direction: &str,
    ) -> PyResult<Option<String>> {
        let objective = objective(metric_key, direction)?;
        let run_ids = run_ids
            .into_iter()
            .map(RunId::from_string)
            .collect::<Vec<_>>();
        self._inner
            .best_eligible_run(&run_ids, &objective)
            .map(|run_id| run_id.map(|value| value.as_str().to_owned()))
            .map_err(runtime_error)
    }

    #[pyo3(signature = (run_id, metric_key, start_step=None, end_step=None, max_points=None))]
    pub fn query_metric(
        &self,
        run_id: &str,
        metric_key: &str,
        start_step: Option<i64>,
        end_step: Option<i64>,
        max_points: Option<usize>,
    ) -> PyResult<Vec<PyMetricPoint>> {
        let run_id = RunId::from_string(run_id);
        let metric_key = MetricKey::from_string(metric_key);
        query_step_metric(
            &self._inner,
            &run_id,
            &metric_key,
            start_step.map(Step::new),
            end_step.map(Step::new),
            max_points,
        )
        .map(|series| {
            series
                .samples()
                .iter()
                .map(|sample| PyMetricPoint::from(sample.point.clone()))
                .collect()
        })
    }

    #[pyo3(signature = (run_id, metric_key, start_step=None, end_step=None, max_points=None))]
    pub fn _query_metric_with_metadata(
        &self,
        run_id: &str,
        metric_key: &str,
        start_step: Option<i64>,
        end_step: Option<i64>,
        max_points: Option<usize>,
    ) -> PyResult<(Vec<PyMetricPoint>, u64, bool)> {
        let run_id = RunId::from_string(run_id);
        let metric_key = MetricKey::from_string(metric_key);
        let series = query_step_metric(
            &self._inner,
            &run_id,
            &metric_key,
            start_step.map(Step::new),
            end_step.map(Step::new),
            max_points,
        )?;
        let points = series
            .samples()
            .iter()
            .map(|sample| PyMetricPoint::from(sample.point.clone()))
            .collect();
        Ok((points, series.source_count(), series.downsampled()))
    }

    pub fn query_metric_summaries(
        &self,
        run_ids: Vec<String>,
        metric_key: &str,
    ) -> PyResult<Vec<PyMetricSummary>> {
        let run_ids: Vec<RunId> = run_ids.into_iter().map(RunId::from_string).collect();
        let metric_key = MetricKey::from_string(metric_key);
        self._inner
            .query_metric_summaries(&run_ids, &metric_key)
            .map(|summaries| summaries.into_iter().map(PyMetricSummary::from).collect())
            .map_err(runtime_error)
    }

    #[pyo3(signature = (run_id, metric_key, start_step=None, end_step=None, max_points=None))]
    pub fn query_metric_table(
        &self,
        run_id: &str,
        metric_key: &str,
        start_step: Option<i64>,
        end_step: Option<i64>,
        max_points: Option<usize>,
    ) -> PyResult<PyArrowTable> {
        let run_id = RunId::from_string(run_id);
        let metric_key = MetricKey::from_string(metric_key);
        let result = self
            ._inner
            .query_metric_with_metadata(
                &run_id,
                &metric_key,
                start_step.map(Step::new),
                end_step.map(Step::new),
                max_points,
            )
            .map_err(runtime_error)?;
        let downsampled = (result.points.len() as u64) < result.source_row_count;
        PyArrowTable::from_metric_points(&result.points, result.source_row_count, downsampled)
            .map_err(|error| {
                PyRuntimeError::new_err(format!("failed to build Arrow table: {error}"))
            })
    }

    pub fn query_metric_summaries_table(
        &self,
        run_ids: Vec<String>,
        metric_key: &str,
    ) -> PyResult<PyArrowTable> {
        let run_ids: Vec<RunId> = run_ids.into_iter().map(RunId::from_string).collect();
        let metric_key = MetricKey::from_string(metric_key);
        self._inner
            .query_metric_summaries(&run_ids, &metric_key)
            .map_err(runtime_error)
            .and_then(|summaries| {
                PyArrowTable::from_metric_summaries(&summaries).map_err(|error| {
                    PyRuntimeError::new_err(format!("failed to build Arrow table: {error}"))
                })
            })
    }

    pub fn list_metrics(&self, run_id: &str) -> PyResult<Vec<PyMetricSummary>> {
        let run_id = RunId::from_string(run_id);
        self._inner
            .list_metrics(&run_id)
            .map(|metrics| metrics.into_iter().map(PyMetricSummary::from).collect())
            .map_err(runtime_error)
    }
}

#[pyclass(name = "Project", module = "seex._seex")]
pub struct PyProject {
    #[pyo3(get)]
    project_id: String,
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    created_at: String,
}

impl From<Project> for PyProject {
    fn from(project: Project) -> Self {
        Self {
            project_id: project.project_id.as_str().to_owned(),
            name: project.name,
            created_at: project.created_at.to_rfc3339(),
        }
    }
}

#[pyclass(name = "Run", module = "seex._seex")]
pub struct PyRun {
    inner: NativeRun,
}

#[pymethods]
impl PyRun {
    #[getter]
    pub fn run_id(&self) -> String {
        self.inner.run_id.as_str().to_owned()
    }

    #[getter]
    pub fn project_id(&self) -> String {
        self.inner.project_id.as_str().to_owned()
    }

    #[getter]
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    #[getter]
    pub fn status(&self) -> String {
        status_as_string(self.inner.status)
    }

    #[getter]
    pub fn created_at(&self) -> String {
        self.inner.created_at.to_rfc3339()
    }

    #[getter]
    pub fn started_at(&self) -> String {
        self.inner.started_at.to_rfc3339()
    }

    #[getter]
    pub fn finished_at(&self) -> Option<String> {
        self.inner
            .finished_at
            .map(|timestamp| timestamp.to_rfc3339())
    }

    pub fn log(&self, key: &str, step: i64, value: f64) -> PyResult<()> {
        self.inner
            .log_metric_at_step(key, step, value)
            .map_err(runtime_error)
    }
}

impl From<NativeRun> for PyRun {
    fn from(run: NativeRun) -> Self {
        Self { inner: run }
    }
}

#[pyclass(name = "Diagnostics", module = "seex._seex")]
pub struct PyDiagnostics {
    #[pyo3(get)]
    pending_reports: u64,
    #[pyo3(get)]
    queue_full_errors: u64,
    #[pyo3(get)]
    persisted_reports: u64,
    #[pyo3(get)]
    writer_state: &'static str,
    #[pyo3(get)]
    last_write_error: Option<String>,
    #[pyo3(get)]
    last_flush_run_id: Option<String>,
    #[pyo3(get)]
    last_flush_status: &'static str,
    #[pyo3(get)]
    last_flush_error: Option<String>,
}

impl From<MetricReporterDiagnostics> for PyDiagnostics {
    fn from(diagnostics: MetricReporterDiagnostics) -> Self {
        Self {
            pending_reports: diagnostics.pending_reports,
            queue_full_errors: diagnostics.queue_full_errors,
            persisted_reports: diagnostics.persisted_reports,
            writer_state: diagnostics.writer_state,
            last_write_error: diagnostics.last_write_error,
            last_flush_run_id: diagnostics.last_flush_run_id,
            last_flush_status: diagnostics.last_flush_status,
            last_flush_error: diagnostics.last_flush_error,
        }
    }
}

#[pyclass(name = "MetricPoint", module = "seex._seex")]
pub struct PyMetricPoint {
    #[pyo3(get)]
    run_id: String,
    #[pyo3(get)]
    metric_key: String,
    #[pyo3(get)]
    step: i64,
    #[pyo3(get)]
    timestamp: String,
    #[pyo3(get)]
    value_f64: f64,
    #[pyo3(get)]
    ingested_at: String,
}

impl From<MetricPoint> for PyMetricPoint {
    fn from(point: MetricPoint) -> Self {
        Self {
            run_id: point.run_id.as_str().to_owned(),
            metric_key: point.metric_key.as_str().to_owned(),
            step: point.step.value(),
            timestamp: point.timestamp.to_rfc3339(),
            value_f64: point.value_f64,
            ingested_at: point.ingested_at.to_rfc3339(),
        }
    }
}

#[pyclass(name = "MetricSummary", module = "seex._seex")]
pub struct PyMetricSummary {
    #[pyo3(get)]
    run_id: String,
    #[pyo3(get)]
    metric_key: String,
    #[pyo3(get)]
    effective_count: u64,
    #[pyo3(get)]
    last_step: i64,
    #[pyo3(get)]
    last_value_f64: f64,
    #[pyo3(get)]
    min_value_f64: f64,
    #[pyo3(get)]
    max_value_f64: f64,
}

impl From<MetricAggregate> for PyMetricSummary {
    fn from(summary: MetricAggregate) -> Self {
        Self {
            run_id: summary.run_id.as_str().to_owned(),
            metric_key: summary.metric_key.as_str().to_owned(),
            effective_count: summary.effective_count,
            last_step: summary.last_step.value(),
            last_value_f64: summary.last_value_f64,
            min_value_f64: summary.min_value_f64,
            max_value_f64: summary.max_value_f64,
        }
    }
}

#[pyfunction]
#[pyo3(
    signature = (
        path=PathBuf::from("."),
        *,
        data_path=None,
        catalog_backend=None,
        catalog_path=None,
        metric_queue_capacity=65536,
        s3_endpoint=None,
        s3_access_key_id=None,
        s3_secret_access_key=None,
        s3_session_token=None,
        s3_region=None,
        s3_path_style=None,
        s3_use_ssl=None,
        _must_exist=false,
    )
)]
#[expect(
    clippy::too_many_arguments,
    reason = "PyO3 exposes init configuration as keyword-only Python API arguments."
)]
pub fn init(
    path: PathBuf,
    data_path: Option<PathBuf>,
    catalog_backend: Option<&str>,
    catalog_path: Option<PathBuf>,
    metric_queue_capacity: i64,
    s3_endpoint: Option<String>,
    s3_access_key_id: Option<String>,
    s3_secret_access_key: Option<String>,
    s3_session_token: Option<String>,
    s3_region: Option<String>,
    s3_path_style: Option<bool>,
    s3_use_ssl: Option<bool>,
    _must_exist: bool,
) -> PyResult<PyClient> {
    let init_config = resolve_init_config(
        &path,
        data_path,
        catalog_backend,
        catalog_path,
        metric_queue_capacity,
        S3ConnectionOverrides {
            endpoint: s3_endpoint,
            access_key_id: s3_access_key_id,
            secret_access_key: s3_secret_access_key,
            session_token: s3_session_token,
            region: s3_region,
            path_style: s3_path_style,
            use_ssl: s3_use_ssl,
        },
    )
    .map_err(invalid_configuration_error)?;
    let client = if _must_exist {
        NativeClient::open_existing_with_catalog_backend_storage_config(
            path,
            init_config.catalog_backend,
            init_config.catalog_path,
            init_config.data_path,
            init_config.s3_connection,
            init_config.metric_queue_capacity,
        )
    } else {
        NativeClient::open_with_catalog_backend_storage_config(
            path,
            init_config.catalog_backend,
            init_config.catalog_path,
            init_config.data_path,
            init_config.s3_connection,
            init_config.metric_queue_capacity,
        )
    };
    client.map(PyClient::new).map_err(runtime_error)
}

fn invalid_configuration_error(error: InitConfigError) -> PyErr {
    InvalidConfigurationError::new_err(error.to_string())
}

fn duration_from_seconds(name: &str, seconds: f64) -> PyResult<Duration> {
    if seconds < 0.0 || !seconds.is_finite() {
        return Err(InvalidConfigurationError::new_err(format!(
            "{name} must be a finite non-negative number"
        )));
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn attach_exception_context(exc_value: &Bound<'_, PyAny>, teardown_error: PyErr) {
    if exc_value.is_none() {
        return;
    }
    let py = exc_value.py();
    let teardown_value = teardown_error.value(py);
    let _ = exc_value.setattr("__context__", teardown_value);
}

pub(crate) fn runtime_error(error: crate::engine::EngineError) -> PyErr {
    let message = error.to_string();
    match error {
        crate::engine::EngineError::RunAlreadyExists { .. } => {
            RunAlreadyExistsError::new_err(message)
        }
        crate::engine::EngineError::RunAlreadyActive { .. } => {
            RunAlreadyActiveError::new_err(message)
        }
        crate::engine::EngineError::RunClosed { .. } => RunClosedError::new_err(message),
        crate::engine::EngineError::InvalidRunTransition { .. } => {
            InvalidRunStateError::new_err(message)
        }
        crate::engine::EngineError::Io(_)
        | crate::engine::EngineError::ProjectAlreadyExists { .. }
        | crate::engine::EngineError::ProjectNotFound { .. }
        | crate::engine::EngineError::RunNotFound { .. }
        | crate::engine::EngineError::CatalogNotFound { .. }
        | crate::engine::EngineError::Storage { .. }
        | crate::engine::EngineError::StorageFailure(_)
        | crate::engine::EngineError::StorageLayer { .. } => StorageError::new_err(message),
        crate::engine::EngineError::MetricQueryMaxPointsTooSmall { .. }
        | crate::engine::EngineError::MetricQueryMaxPointsTooLarge { .. } => {
            SeexError::new_err(message)
        }
        crate::engine::EngineError::MetricQueueFull => MetricQueueFullError::new_err(message),
        crate::engine::EngineError::MetricWriterFailed { .. } => {
            MetricWriterFailedError::new_err(message)
        }
        crate::engine::EngineError::MetricDrainTimeout => MetricDrainTimeoutError::new_err(message),
        crate::engine::EngineError::MetricFlush { .. } => MetricFlushError::new_err(message),
        crate::engine::EngineError::MetricFlushTimeout => MetricFlushTimeoutError::new_err(message),
        crate::engine::EngineError::ClientClosed => ClientClosedError::new_err(message),
        _ => SeexError::new_err(message),
    }
}

fn status_as_string(status: RunStatus) -> String {
    match status {
        RunStatus::Running => "running",
        RunStatus::Finished => "finished",
        RunStatus::Failed => "failed",
    }
    .to_owned()
}

fn parse_run_status(status: Option<&str>) -> PyResult<Option<RunStatus>> {
    match status {
        None => Ok(None),
        Some("running") => Ok(Some(RunStatus::Running)),
        Some("finished") => Ok(Some(RunStatus::Finished)),
        Some("failed") => Ok(Some(RunStatus::Failed)),
        Some(status) => Err(PyValueError::new_err(format!(
            "status must be one of 'running', 'finished', or 'failed', got {status:?}"
        ))),
    }
}
