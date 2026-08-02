use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use seex::{ProjectId, Reader, Run, RunId, RunStatus};

use crate::sdk::client::{ApiClosedError, PyProject};
use crate::sdk::run::sdk_error;
use crate::sdk::settings::PySettings;

type SharedReader = Rc<RefCell<Option<Reader>>>;

#[pyclass(name = "Api", module = "seex._seex", unsendable)]
pub struct PyApi {
    reader: SharedReader,
}

#[pymethods]
impl PyApi {
    #[new]
    #[pyo3(signature = (dir=PathBuf::from("."), settings=None))]
    fn new(dir: PathBuf, settings: Option<PyRef<'_, PySettings>>) -> PyResult<Self> {
        let builder = match settings {
            Some(value) => value.reader_builder(dir),
            None => Reader::builder(dir),
        };
        let reader = builder.open().map_err(sdk_error)?;
        Ok(Self {
            reader: Rc::new(RefCell::new(Some(reader))),
        })
    }

    fn projects(&self) -> PyResult<Vec<PyProject>> {
        self.with_reader(|reader| {
            reader
                .projects()
                .map(|projects| projects.into_iter().map(PyProject::from).collect())
                .map_err(sdk_error)
        })
    }

    fn project(&self, project_id: &str) -> PyResult<Option<PyProject>> {
        self.with_reader(|reader| {
            reader
                .project(&ProjectId::from_string(project_id))
                .map(|project| project.map(PyProject::from))
                .map_err(sdk_error)
        })
    }

    fn runs(&self, project_id: &str) -> PyResult<Vec<PyRunRecord>> {
        self.with_reader(|reader| {
            reader
                .runs(&ProjectId::from_string(project_id))
                .map(|runs| {
                    runs.into_iter()
                        .map(|run| PyRunRecord::new(Rc::clone(&self.reader), run))
                        .collect()
                })
                .map_err(sdk_error)
        })
    }

    fn run(&self, path: &str) -> PyResult<Option<PyRunRecord>> {
        let (project_id, run_id) = parse_run_path(path)?;
        self.with_reader(|reader| {
            reader
                .run(
                    &ProjectId::from_string(project_id),
                    &RunId::from_string(run_id),
                )
                .map(|run| run.map(|run| PyRunRecord::new(Rc::clone(&self.reader), run)))
                .map_err(sdk_error)
        })
    }
}

impl PyApi {
    fn with_reader<T>(&self, read: impl FnOnce(&Reader) -> PyResult<T>) -> PyResult<T> {
        let reader = self.reader.borrow();
        read(
            reader
                .as_ref()
                .ok_or_else(|| ApiClosedError::new_err("The read-only API is closed."))?,
        )
    }
}

#[pyclass(name = "RunRecord", module = "seex._seex", unsendable)]
pub struct PyRunRecord {
    pub(crate) _reader: SharedReader,
    pub(crate) run: Run,
}

impl PyRunRecord {
    fn new(reader: SharedReader, run: Run) -> Self {
        Self {
            _reader: reader,
            run,
        }
    }
}

#[pymethods]
impl PyRunRecord {
    #[getter]
    fn run_id(&self) -> &str {
        self.run.run_id.as_str()
    }

    #[getter]
    fn project_id(&self) -> &str {
        self.run.project_id.as_str()
    }

    #[getter]
    fn name(&self) -> &str {
        &self.run.name
    }

    #[getter]
    fn status(&self) -> &'static str {
        match self.run.status {
            RunStatus::Running => "running",
            RunStatus::Finished => "finished",
            RunStatus::Failed => "failed",
        }
    }

    #[getter]
    fn created_at(&self) -> String {
        self.run.created_at.to_rfc3339()
    }

    #[getter]
    fn started_at(&self) -> String {
        self.run.started_at.to_rfc3339()
    }

    #[getter]
    fn finished_at(&self) -> Option<String> {
        self.run.finished_at.map(|value| value.to_rfc3339())
    }
}

fn parse_run_path(path: &str) -> PyResult<(&str, &str)> {
    let Some((project_id, run_id)) = path.split_once('/') else {
        return Err(PyValueError::new_err(
            "Run path must be 'project_id/run_id'",
        ));
    };
    if project_id.is_empty() || run_id.is_empty() || run_id.contains('/') {
        return Err(PyValueError::new_err(
            "Run path must be 'project_id/run_id'",
        ));
    }
    Ok((project_id, run_id))
}
