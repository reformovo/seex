use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use seex::{MetricKey, ProjectId, Reader, Run, RunId, RunStatus};

use crate::sdk::client::{ApiClosedError, PyMetricSummary, PyProject};
use crate::sdk::comparison::{PyComparisonResult, PyRankingResult, objective};
use crate::sdk::history::{PyMetricSeries, metric_query};
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

    #[pyo3(signature = (candidate_run_id, reference_run_id, *, metric_key, direction))]
    fn compare_runs(
        &self,
        candidate_run_id: &str,
        reference_run_id: &str,
        metric_key: &str,
        direction: &str,
    ) -> PyResult<PyComparisonResult> {
        let objective = objective(metric_key, direction)?;
        self.with_reader(|reader| {
            reader
                .compare_runs(
                    &RunId::from_string(candidate_run_id),
                    &RunId::from_string(reference_run_id),
                    &objective,
                )
                .map(PyComparisonResult::from)
                .map_err(sdk_error)
        })
    }

    #[pyo3(signature = (run_ids, *, metric_key, direction))]
    fn rank_runs(
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
        self.with_reader(|reader| {
            reader
                .rank_runs(&run_ids, &objective)
                .map(PyRankingResult::from)
                .map_err(sdk_error)
        })
    }

    fn close(&self) {
        self.reader.borrow_mut().take();
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __exit__(
        &self,
        _exc_type: &Bound<'_, PyAny>,
        _exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> bool {
        self.close();
        false
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
    pub(crate) reader: SharedReader,
    pub(crate) run: Run,
}

impl PyRunRecord {
    fn new(reader: SharedReader, run: Run) -> Self {
        Self { reader, run }
    }

    fn ensure_open(&self) -> PyResult<()> {
        if self.reader.borrow().is_none() {
            return Err(ApiClosedError::new_err("The read-only API is closed."));
        }
        Ok(())
    }

    fn with_reader<T>(&self, read: impl FnOnce(&Reader) -> PyResult<T>) -> PyResult<T> {
        self.ensure_open()?;
        let reader = self.reader.borrow();
        read(
            reader
                .as_ref()
                .ok_or_else(|| ApiClosedError::new_err("The read-only API is closed."))?,
        )
    }
}

#[pymethods]
impl PyRunRecord {
    #[getter]
    fn run_id(&self) -> PyResult<&str> {
        self.ensure_open()?;
        Ok(self.run.run_id.as_str())
    }

    #[getter]
    fn project_id(&self) -> PyResult<&str> {
        self.ensure_open()?;
        Ok(self.run.project_id.as_str())
    }

    #[getter]
    fn name(&self) -> PyResult<&str> {
        self.ensure_open()?;
        Ok(&self.run.name)
    }

    #[getter]
    fn status(&self) -> PyResult<&'static str> {
        self.ensure_open()?;
        Ok(match self.run.status {
            RunStatus::Running => "running",
            RunStatus::Finished => "finished",
            RunStatus::Failed => "failed",
        })
    }

    #[getter]
    fn created_at(&self) -> PyResult<String> {
        self.ensure_open()?;
        Ok(self.run.created_at.to_rfc3339())
    }

    #[getter]
    fn started_at(&self) -> PyResult<String> {
        self.ensure_open()?;
        Ok(self.run.started_at.to_rfc3339())
    }

    #[getter]
    fn finished_at(&self) -> PyResult<Option<String>> {
        self.ensure_open()?;
        Ok(self.run.finished_at.map(|value| value.to_rfc3339()))
    }

    fn metrics(&self) -> PyResult<Vec<PyMetricSummary>> {
        self.with_reader(|reader| {
            reader
                .metrics(&self.run)
                .map(|metrics| metrics.into_iter().map(PyMetricSummary::from).collect())
                .map_err(sdk_error)
        })
    }

    fn metric_summary(&self, metric_key: &str) -> PyResult<Option<PyMetricSummary>> {
        self.with_reader(|reader| {
            reader
                .metric_summary(&self.run, &MetricKey::from_string(metric_key))
                .map(|summary| summary.map(PyMetricSummary::from))
                .map_err(sdk_error)
        })
    }

    #[pyo3(signature = (metric_key, *, x_axis="step", start=None, end=None, max_points=None))]
    fn history(
        &self,
        metric_key: &str,
        x_axis: &str,
        start: Option<&Bound<'_, PyAny>>,
        end: Option<&Bound<'_, PyAny>>,
        max_points: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyMetricSeries> {
        let query = metric_query(x_axis, start, end, max_points)?;
        self.with_reader(|reader| {
            reader
                .query_metric(
                    &self.run.run_id,
                    &MetricKey::from_string(metric_key),
                    &query,
                )
                .map(|series| PyMetricSeries { series })
                .map_err(sdk_error)
        })
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
