use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyMapping, PyTuple};
use seex::{Client, LogOptions, ResumePolicy, RunHandle, RunOptions, RunStatus};

use crate::sdk::client::PyDiagnostics;
use crate::sdk::client::{
    InvalidConfigurationError, InvalidRunStateError, MetricDrainTimeoutError, MetricFlushError,
    MetricFlushTimeoutError, MetricQueueFullError, MetricWriterFailedError, RunAlreadyActiveError,
    RunAlreadyExistsError, RunClosedError, SeexError, StorageError,
};
use crate::sdk::settings::PySettings;

#[pyclass(name = "Run", module = "seex._seex", unsendable)]
pub struct PyTargetRun {
    client: RefCell<Option<Client>>,
    handle: RefCell<Option<RunHandle>>,
    run_id: String,
    project_id: String,
    name: String,
    terminal: Cell<Option<RunStatus>>,
    final_diagnostics: RefCell<Option<seex::ClientDiagnostics>>,
}

#[pymethods]
impl PyTargetRun {
    #[getter]
    fn run_id(&self) -> &str {
        &self.run_id
    }

    #[getter]
    fn project_id(&self) -> &str {
        &self.project_id
    }

    #[getter]
    fn name(&self) -> &str {
        &self.name
    }

    #[getter]
    fn status(&self) -> &'static str {
        let status = self.terminal.get().unwrap_or_else(|| {
            self.handle
                .borrow()
                .as_ref()
                .map_or(RunStatus::Running, RunHandle::status)
        });
        match status {
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
        self.handle
            .borrow()
            .as_ref()
            .ok_or_else(|| RunClosedError::new_err("Run is closed"))?
            .log_with(metrics, options)
            .map_err(sdk_error)
    }

    fn diagnostics(&self) -> PyResult<PyDiagnostics> {
        if let Some(handle) = self.handle.borrow().as_ref() {
            return Ok(handle.diagnostics().into());
        }
        self.final_diagnostics
            .borrow()
            .clone()
            .map(PyDiagnostics::from)
            .ok_or_else(|| SeexError::new_err("Run diagnostics are unavailable"))
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
        let requested = if exit_code.is_none_or(|value| value == 0) {
            RunStatus::Finished
        } else {
            RunStatus::Failed
        };
        if let Some(selected) = self.terminal.get() {
            return if selected == requested {
                Ok(())
            } else {
                Err(seex::Error::TerminalOutcomeConflict {
                    selected,
                    requested,
                })
            };
        }
        {
            let handle = self.handle.borrow();
            let handle = handle.as_ref().ok_or(seex::Error::Storage)?;
            match requested {
                RunStatus::Finished => handle.finish()?,
                RunStatus::Failed => handle.fail()?,
                RunStatus::Running => unreachable!("requested outcome is terminal"),
            }
        }
        self.client
            .borrow()
            .as_ref()
            .ok_or(seex::Error::Storage)?
            .shutdown()?;
        let diagnostics = self
            .handle
            .borrow()
            .as_ref()
            .ok_or(seex::Error::Storage)?
            .diagnostics();
        self.final_diagnostics.replace(Some(diagnostics));
        self.terminal.set(Some(requested));
        let handle = self.handle.borrow_mut().take();
        let client = self.client.borrow_mut().take();
        drop(handle);
        drop(client);
        Ok(())
    }
}

#[pyfunction(name = "init")]
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
    let run_id = handle.run_id().as_str().to_owned();
    let project_id = handle.project_id().as_str().to_owned();
    let name = handle.name().to_owned();
    Ok(PyTargetRun {
        client: RefCell::new(Some(client)),
        handle: RefCell::new(Some(handle)),
        run_id,
        project_id,
        name,
        terminal: Cell::new(None),
        final_diagnostics: RefCell::new(None),
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
        seex::Error::InvalidRunOptions { .. }
        | seex::Error::InvalidMetricMapping
        | seex::Error::MetricMappingTooLarge { .. }
        | seex::Error::DuplicateRunIdentity { .. } => PyValueError::new_err(message),
        seex::Error::InvalidRunState { .. }
        | seex::Error::RunProjectMismatch { .. }
        | seex::Error::StepRegression { .. }
        | seex::Error::StepOverflow { .. }
        | seex::Error::TerminalOutcomeConflict { .. } => InvalidRunStateError::new_err(message),
        seex::Error::RunNotFound { .. }
        | seex::Error::UnsupportedQuery
        | seex::Error::CatalogNotFound { .. }
        | seex::Error::LttbExtensionUnavailable { .. }
        | seex::Error::Storage => StorageError::new_err(message),
        _ => SeexError::new_err(message),
    }
}
