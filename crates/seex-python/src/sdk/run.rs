use std::path::PathBuf;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool};
use seex::{Client, ResumePolicy, RunHandle, RunOptions, RunStatus};

use crate::sdk::client::{
    ClientClosedError, InvalidConfigurationError, InvalidRunStateError, MetricDrainTimeoutError,
    MetricFlushError, MetricFlushTimeoutError, MetricQueueFullError, MetricWriterFailedError,
    RunAlreadyActiveError, RunAlreadyExistsError, RunClosedError, SeexError, StorageError,
};
use crate::sdk::settings::PySettings;

#[pyclass(name = "_Run", module = "seex._seex", unsendable)]
pub struct PyTargetRun {
    pub(crate) _client: Client,
    pub(crate) handle: RunHandle,
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
        _client: client,
        handle,
    })
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
