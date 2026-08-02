use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use seex::{CatalogBackend, ClientBuilder, ReaderBuilder, S3Options};

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

impl PySettings {
    pub fn client_builder(&self, root: PathBuf) -> ClientBuilder {
        let mut builder = ClientBuilder::new(root)
            .metric_queue_capacity(self.metric_queue_capacity)
            .s3_options(self.s3_options());
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

    pub fn reader_builder(&self, root: PathBuf) -> ReaderBuilder {
        let mut builder = ReaderBuilder::new(root).s3_options(self.s3_options());
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
            Some("duckdb") => Some(CatalogBackend::DuckDb),
            Some("sqlite") => Some(CatalogBackend::Sqlite),
            _ => None,
        }
    }

    fn s3_options(&self) -> S3Options {
        let mut options = S3Options::new();
        if let Some(value) = &self.s3_endpoint {
            options = options.endpoint(value);
        }
        if let Some(value) = &self.s3_access_key_id {
            options = options.access_key_id(value);
        }
        if let Some(value) = &self.s3_secret_access_key {
            options = options.secret_access_key(value);
        }
        if let Some(value) = &self.s3_session_token {
            options = options.session_token(value);
        }
        if let Some(value) = &self.s3_region {
            options = options.region(value);
        }
        if let Some(value) = self.s3_path_style {
            options = options.path_style(value);
        }
        if let Some(value) = self.s3_use_ssl {
            options = options.use_ssl(value);
        }
        options
    }
}

fn redacted(value: &Option<String>) -> &'static str {
    if value.is_some() {
        "'[REDACTED]'"
    } else {
        "None"
    }
}
