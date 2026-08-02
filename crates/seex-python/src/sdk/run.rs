use std::cell::Cell;
use std::path::PathBuf;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyMapping, PyTuple};
use seex::{Client, LogOptions, ResumePolicy, RunHandle, RunOptions, RunStatus};

use crate::sdk::client::PyDiagnostics;
use crate::sdk::client::{
    ClientClosedError, InvalidConfigurationError, InvalidRunStateError, MetricDrainTimeoutError,
    MetricFlushError, MetricFlushTimeoutError, MetricQueueFullError, MetricWriterFailedError,
    RunAlreadyActiveError, RunAlreadyExistsError, RunClosedError, SeexError, StorageError,
};
use crate::sdk::settings::PySettings;

#[pyclass(name = "_Run", module = "seex._seex", unsendable)]
pub struct PyTargetRun {
    pub(crate) client: Client,
    pub(crate) handle: RunHandle,
    client_closed: Cell<bool>,
}

#[pymethods]
impl PyTargetRun {
    #[getter]
    fn run_id(&self) -> &str {
        self.handle.run_id().as_str()
    }

    #[getter]
    fn project_id(&self) -> &str {
        self.handle.project_id().as_str()
    }

    #[getter]
    fn name(&self) -> &str {
        self.handle.name()
    }

    #[getter]
    fn status(&self) -> &'static str {
        match self.handle.status() {
            RunStatus::Running => "running",
            RunStatus::Finished => "finished",
            RunStatus::Failed => "failed",
        }
    }

    #[pyo3(signature = (data, *, step=None, commit=None))]
    fn log(
        &self,
        data: &Bound<'_, PyAny>,
        step: Option<i64>,
        commit: Option<bool>,
    ) -> PyResult<()> {
        let metrics = numeric_mapping(data)?;
        let mut options = LogOptions::new();
        if let Some(step) = step {
            options = options.step(step);
        }
        if let Some(commit) = commit {
            options = options.commit(commit);
        }
        self.handle.log_with(metrics, options).map_err(sdk_error)
    }

    fn diagnostics(&self) -> PyDiagnostics {
        self.handle.diagnostics().into()
    }

    #[pyo3(signature = (exit_code=None))]
    fn finish(&self, exit_code: Option<i64>) -> PyResult<()> {
        self.finalize(exit_code).map_err(sdk_error)
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __exit__(
        &self,
        exc_type: &Bound<'_, PyAny>,
        exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        if exc_type.is_none() {
            self.finalize(None).map_err(sdk_error)?;
        } else if let Err(error) = self.finalize(Some(1)) {
            attach_exception_context(exc_value, sdk_error(error));
        }
        Ok(false)
    }
}

impl PyTargetRun {
    fn finalize(&self, exit_code: Option<i64>) -> seex::Result<()> {
        if exit_code.is_none_or(|value| value == 0) {
            self.handle.finish()?;
        } else {
            self.handle.fail()?;
        }
        if !self.client_closed.get() {
            self.client.shutdown()?;
            self.client_closed.set(true);
        }
        Ok(())
    }
}

#[pyfunction(name = "_start_run")]
#[pyo3(signature = (*, project=None, dir=None, id=None, name=None, resume=None, settings=None))]
pub fn start_run(
    project: Option<String>,
    dir: Option<PathBuf>,
    id: Option<String>,
    name: Option<String>,
    resume: Option<&Bound<'_, PyAny>>,
    settings: Option<PyRef<'_, PySettings>>,
) -> PyResult<PyTargetRun> {
    let root = dir.unwrap_or_else(|| PathBuf::from("."));
    let builder = match settings {
        Some(value) => value.client_builder(root),
        None => Client::builder(root),
    };
    let client = builder.open().map_err(sdk_error)?;
    let mut options = RunOptions::new(project.unwrap_or_else(|| String::from("uncategorized")))
        .resume(parse_resume(resume)?);
    if let Some(id) = id {
        options = options.id(id);
    }
    if let Some(name) = name {
        options = options.name(name);
    }
    let handle = client.start_run(options).map_err(sdk_error)?;
    Ok(PyTargetRun {
        client,
        handle,
        client_closed: Cell::new(false),
    })
}

fn attach_exception_context(exc_value: &Bound<'_, PyAny>, finalization_error: PyErr) {
    if !exc_value.is_none() {
        let _ = exc_value.setattr("__context__", finalization_error.value(exc_value.py()));
    }
}

fn numeric_mapping(data: &Bound<'_, PyAny>) -> PyResult<Vec<(String, f64)>> {
    let mapping = data
        .cast::<PyMapping>()
        .map_err(|_| PyTypeError::new_err("data must be a Mapping"))?;
    let mut metrics = Vec::with_capacity(mapping.len()?);
    for item in mapping.items()?.try_iter()? {
        let item = item?.cast_into::<PyTuple>()?;
        let key = item
            .get_item(0)?
            .extract::<String>()
            .map_err(|_| PyTypeError::new_err("metric Mapping keys must be non-empty strings"))?;
        if key.is_empty() {
            return Err(PyValueError::new_err(
                "metric Mapping keys must be non-empty strings",
            ));
        }
        let value = item.get_item(1)?;
        if value.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err(
                "metric Mapping values must be integers or floats, not bool",
            ));
        }
        let value = value.extract::<f64>().map_err(|_| {
            PyTypeError::new_err("metric Mapping values must be integers or floats")
        })?;
        metrics.push((key, value));
    }
    Ok(metrics)
}

fn parse_resume(value: Option<&Bound<'_, PyAny>>) -> PyResult<ResumePolicy> {
    let Some(value) = value else {
        return Ok(ResumePolicy::Never);
    };
    if value.is_instance_of::<PyBool>() {
        return value.extract::<bool>().map(|resume| {
            if resume {
                ResumePolicy::Allow
            } else {
                ResumePolicy::Never
            }
        });
    }
    let raw = value.extract::<&str>().map_err(|_| {
        PyTypeError::new_err("resume must be bool, 'allow', 'never', 'must', or None")
    })?;
    match raw {
        "allow" => Ok(ResumePolicy::Allow),
        "never" => Ok(ResumePolicy::Never),
        "must" => Ok(ResumePolicy::Must),
        _ => Err(PyValueError::new_err(
            "resume must be 'allow', 'never', or 'must'",
        )),
    }
}

pub(crate) fn sdk_error(error: seex::Error) -> PyErr {
    let message = error.to_string();
    match error {
        seex::Error::Configuration => InvalidConfigurationError::new_err(message),
        seex::Error::RunAlreadyExists { .. } => RunAlreadyExistsError::new_err(message),
        seex::Error::RunAlreadyActive { .. } => RunAlreadyActiveError::new_err(message),
        seex::Error::MetricQueueFull => MetricQueueFullError::new_err(message),
        seex::Error::RunClosed { .. } => RunClosedError::new_err(message),
        seex::Error::MetricWriterFailed => MetricWriterFailedError::new_err(message),
        seex::Error::MetricDrainTimeout => MetricDrainTimeoutError::new_err(message),
        seex::Error::MetricFlushFailed => MetricFlushError::new_err(message),
        seex::Error::MetricFlushTimeout => MetricFlushTimeoutError::new_err(message),
        seex::Error::ClientClosed => ClientClosedError::new_err(message),
        seex::Error::InvalidRunOptions { .. }
        | seex::Error::InvalidMetricMapping
        | seex::Error::MetricMappingTooLarge { .. }
        | seex::Error::DuplicateRunIdentity { .. } => PyValueError::new_err(message),
        seex::Error::InvalidRunState { .. }
        | seex::Error::RunProjectMismatch { .. }
        | seex::Error::StepRegression { .. }
        | seex::Error::StepOverflow { .. }
        | seex::Error::TerminalOutcomeConflict { .. } => InvalidRunStateError::new_err(message),
        seex::Error::RunNotFound { .. } | seex::Error::UnsupportedQuery | seex::Error::Storage => {
            StorageError::new_err(message)
        }
        _ => SeexError::new_err(message),
    }
}
