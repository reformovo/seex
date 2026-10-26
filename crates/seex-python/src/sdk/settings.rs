use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use seex::{CatalogBackend, ClientBuilder};

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
}

#[pymethods]
impl PySettings {
    #[new]
    #[pyo3(signature = (*, catalog_backend=None, catalog_path=None, data_path=None,
        metric_queue_capacity=65536))]
    fn new(
        catalog_backend: Option<String>,
        catalog_path: Option<PathBuf>,
        data_path: Option<PathBuf>,
        metric_queue_capacity: usize,
    ) -> PyResult<Self> {
        if catalog_backend
            .as_deref()
            .is_some_and(|value| value != "sqlite")
        {
            return Err(PyValueError::new_err("catalog_backend must be 'sqlite'"));
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
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "Settings(catalog_backend={:?}, catalog_path={:?}, data_path={:?}, \
             metric_queue_capacity={})",
            self.catalog_backend, self.catalog_path, self.data_path, self.metric_queue_capacity,
        )
    }
}

impl PySettings {
    pub fn client_builder(&self, root: PathBuf) -> ClientBuilder {
        let mut builder =
            ClientBuilder::new(root).metric_queue_capacity(self.metric_queue_capacity);
        if let Some(backend) = self.native_catalog_backend() {
            builder = builder.catalog_backend(backend);
        }
        if let Some(path) = &self.catalog_path {
            builder = builder.catalog_path(path);
        }
        if let Some(path) = &self.data_path {
            builder = builder.data_path(path);
        }
        builder
    }

    fn native_catalog_backend(&self) -> Option<CatalogBackend> {
        match self.catalog_backend.as_deref() {
            Some("sqlite") => Some(CatalogBackend::Sqlite),
            _ => None,
        }
    }
}
