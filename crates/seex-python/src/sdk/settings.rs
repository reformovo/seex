use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[pyclass(name = "Settings", module = "seex._seex", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PySettings {
    #[pyo3(get)]
    pub catalog_backend: Option<String>,
    #[pyo3(get)]
    pub catalog_path: Option<PathBuf>,
    #[pyo3(get)]
    pub data_path: Option<PathBuf>,
    #[pyo3(get)]
    pub metric_queue_capacity: usize,
    #[pyo3(get)]
    pub s3_endpoint: Option<String>,
    #[pyo3(get)]
    pub s3_access_key_id: Option<String>,
    #[pyo3(get)]
    pub s3_secret_access_key: Option<String>,
    #[pyo3(get)]
    pub s3_session_token: Option<String>,
    #[pyo3(get)]
    pub s3_region: Option<String>,
    #[pyo3(get)]
    pub s3_path_style: Option<bool>,
    #[pyo3(get)]
    pub s3_use_ssl: Option<bool>,
}

#[pymethods]
impl PySettings {
    #[new]
    #[pyo3(signature = (*, catalog_backend=None, catalog_path=None, data_path=None,
        metric_queue_capacity=65536, s3_endpoint=None, s3_access_key_id=None,
        s3_secret_access_key=None, s3_session_token=None, s3_region=None,
        s3_path_style=None, s3_use_ssl=None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "Settings mirrors native storage fields"
    )]
    fn new(
        catalog_backend: Option<String>,
        catalog_path: Option<PathBuf>,
        data_path: Option<PathBuf>,
        metric_queue_capacity: usize,
        s3_endpoint: Option<String>,
        s3_access_key_id: Option<String>,
        s3_secret_access_key: Option<String>,
        s3_session_token: Option<String>,
        s3_region: Option<String>,
        s3_path_style: Option<bool>,
        s3_use_ssl: Option<bool>,
    ) -> PyResult<Self> {
        if catalog_backend
            .as_deref()
            .is_some_and(|value| !matches!(value, "duckdb" | "sqlite"))
        {
            return Err(PyValueError::new_err(
                "catalog_backend must be 'duckdb' or 'sqlite'",
            ));
        }
        if !(1..=1_048_576).contains(&metric_queue_capacity) {
            return Err(PyValueError::new_err(
                "metric_queue_capacity must be between 1 and 1048576",
            ));
        }
        Ok(Self {
            catalog_backend,
            catalog_path,
            data_path,
            metric_queue_capacity,
            s3_endpoint,
            s3_access_key_id,
            s3_secret_access_key,
            s3_session_token,
            s3_region,
            s3_path_style,
            s3_use_ssl,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "Settings(catalog_backend={:?}, catalog_path={:?}, data_path={:?}, \
             metric_queue_capacity={}, s3_endpoint={:?}, s3_access_key_id={}, \
             s3_secret_access_key={}, s3_session_token={}, s3_region={:?}, \
             s3_path_style={:?}, s3_use_ssl={:?})",
            self.catalog_backend,
            self.catalog_path,
            self.data_path,
            self.metric_queue_capacity,
            self.s3_endpoint,
            redacted(&self.s3_access_key_id),
            redacted(&self.s3_secret_access_key),
            redacted(&self.s3_session_token),
            self.s3_region,
            self.s3_path_style,
            self.s3_use_ssl,
        )
    }
}

fn redacted(value: &Option<String>) -> &'static str {
    if value.is_some() {
        "'[REDACTED]'"
    } else {
        "None"
    }
}
