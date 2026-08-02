//! Public writer client facade.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{CatalogBackend, S3Options};
use crate::engine::client::NativeClient;
use crate::engine::reporting::MetricReporterDiagnostics;
use crate::error::{Error, Result};
use crate::storage::config::{S3ConnectionOverrides, resolve_init_config};

const DEFAULT_METRIC_QUEUE_CAPACITY: usize = 65_536;

/// Builder for a native writer [`Client`].
pub struct ClientBuilder {
    root_path: PathBuf,
    catalog_backend: Option<CatalogBackend>,
    catalog_path: Option<PathBuf>,
    data_path: Option<PathBuf>,
    metric_queue_capacity: usize,
    s3: S3Options,
}

impl ClientBuilder {
    pub fn new(root_path: impl Into<PathBuf>) -> Self {
        Self {
            root_path: root_path.into(),
            catalog_backend: None,
            catalog_path: None,
            data_path: None,
            metric_queue_capacity: DEFAULT_METRIC_QUEUE_CAPACITY,
            s3: S3Options::default(),
        }
    }

    pub fn catalog_backend(mut self, value: CatalogBackend) -> Self {
        self.catalog_backend = Some(value);
        self
    }

    pub fn catalog_path(mut self, value: impl Into<PathBuf>) -> Self {
        self.catalog_path = Some(value.into());
        self
    }

    pub fn data_path(mut self, value: impl Into<PathBuf>) -> Self {
        self.data_path = Some(value.into());
        self
    }

    pub const fn metric_queue_capacity(mut self, value: usize) -> Self {
        self.metric_queue_capacity = value;
        self
    }

    pub fn s3_options(mut self, value: S3Options) -> Self {
        self.s3 = value;
        self
    }

    /// Opens or creates the configured native store.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] for invalid effective configuration or
    /// [`Error::Storage`] when the native store cannot be opened.
    pub fn open(self) -> Result<Client> {
        let capacity =
            i64::try_from(self.metric_queue_capacity).map_err(|_| Error::Configuration)?;
        let resolved = resolve_init_config(
            &self.root_path,
            self.data_path,
            self.catalog_backend.map(CatalogBackend::as_name),
            self.catalog_path,
            capacity,
            S3ConnectionOverrides {
                endpoint: self.s3.endpoint,
                access_key_id: self.s3.access_key_id,
                secret_access_key: self.s3.secret_access_key,
                session_token: self.s3.session_token,
                region: self.s3.region,
                path_style: self.s3.path_style,
                use_ssl: self.s3.use_ssl,
            },
        )
        .map_err(|_| Error::Configuration)?;
        let inner = NativeClient::open_with_catalog_backend_storage_config(
            &self.root_path,
            resolved.catalog_backend,
            resolved.catalog_path,
            resolved.data_path,
            resolved.s3_connection,
            resolved.metric_queue_capacity,
        )
        .map_err(Error::from)?;
        Ok(Client {
            inner: Arc::new(inner),
        })
    }
}

/// Native metric writer and Run factory.
pub struct Client {
    pub(crate) inner: Arc<NativeClient>,
}

impl Client {
    pub fn builder(root_path: impl AsRef<Path>) -> ClientBuilder {
        ClientBuilder::new(root_path.as_ref().to_owned())
    }

    pub fn diagnostics(&self) -> ClientDiagnostics {
        self.inner.diagnostics().into()
    }

    /// Drains queued reports and closes this client without finalizing Runs.
    ///
    /// # Errors
    ///
    /// Returns a writer, drain, or storage error when shutdown cannot finish.
    pub fn shutdown(&self) -> Result<()> {
        self.inner.shutdown(None).map_err(Error::from)
    }
}

/// State of the background metric writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriterState {
    Running,
    Retrying,
    Failed,
    Drained,
    Closed,
}

/// State of the latest terminal-Run flush attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlushState {
    None,
    Running,
    Succeeded,
    Failed,
    TimedOut,
}

/// Read-only runtime diagnostics for one client process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientDiagnostics {
    pub pending_reports: u64,
    pub queue_full_errors: u64,
    pub persisted_reports: u64,
    pub writer_state: WriterState,
    pub last_write_error: Option<String>,
    pub last_flush_run_id: Option<String>,
    pub last_flush_state: FlushState,
    pub last_flush_error: Option<String>,
}

impl From<MetricReporterDiagnostics> for ClientDiagnostics {
    fn from(value: MetricReporterDiagnostics) -> Self {
        Self {
            pending_reports: value.pending_reports,
            queue_full_errors: value.queue_full_errors,
            persisted_reports: value.persisted_reports,
            writer_state: match value.writer_state {
                "running" => WriterState::Running,
                "retrying" => WriterState::Retrying,
                "failed" => WriterState::Failed,
                "closed" => WriterState::Closed,
                _ => WriterState::Drained,
            },
            last_write_error: value.last_write_error,
            last_flush_run_id: value.last_flush_run_id,
            last_flush_state: match value.last_flush_status {
                "running" => FlushState::Running,
                "succeeded" => FlushState::Succeeded,
                "failed" => FlushState::Failed,
                "timed_out" => FlushState::TimedOut,
                _ => FlushState::None,
            },
            last_flush_error: value.last_flush_error,
        }
    }
}
