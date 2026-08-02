//! Public writer client facade.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::{CatalogBackend, S3Options};
use crate::engine::client::NativeClient;
use crate::engine::client::NativeRun;
use crate::engine::reporting::MetricReporterDiagnostics;
use crate::error::{Error, Result};
use crate::model::metric::{MetricKey, Step};
use crate::model::run::{RunId, RunStatus};
use crate::model::types::{Project, ProjectId};
use crate::storage::ProjectCreateGuard;
use crate::storage::config::{S3ConnectionOverrides, resolve_init_config};

const DEFAULT_METRIC_QUEUE_CAPACITY: usize = 65_536;
const MAX_METRICS_PER_LOG: usize = 8_192;

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
            root_path: self.root_path,
        })
    }
}

/// Native metric writer and Run factory.
pub struct Client {
    pub(crate) inner: Arc<NativeClient>,
    root_path: PathBuf,
}

impl Client {
    pub fn builder(root_path: impl AsRef<Path>) -> ClientBuilder {
        ClientBuilder::new(root_path.as_ref().to_owned())
    }

    pub fn diagnostics(&self) -> ClientDiagnostics {
        self.inner.diagnostics().into()
    }

    /// Creates or resumes a Run according to the requested policy.
    ///
    /// # Errors
    ///
    /// Returns a typed option, identity, lifecycle, lock, or storage error.
    pub fn start_run(&self, options: RunOptions) -> Result<RunHandle> {
        let project = self.get_or_create_project(&options.project)?;
        let run_id = options
            .run_id
            .map(RunId::from_string)
            .unwrap_or_else(|| RunId::from_string(uuid::Uuid::new_v4().to_string()));
        if run_id.as_str().is_empty() {
            return Err(Error::InvalidRunOptions { field: "id" });
        }
        let name = options.name.unwrap_or_else(|| run_id.as_str().to_owned());
        let (run, next_step) = match options.resume {
            ResumePolicy::Never => (
                self.inner
                    .create_run(&project.project_id, &name, Some(run_id))
                    .map_err(Error::from)?,
                Step::new(0),
            ),
            ResumePolicy::Allow => match self.inner.get_run(&run_id) {
                Ok(existing) => self.resume_existing(existing, &project.project_id)?,
                Err(crate::engine::EngineError::RunNotFound { .. }) => (
                    self.inner
                        .create_run(&project.project_id, &name, Some(run_id))
                        .map_err(Error::from)?,
                    Step::new(0),
                ),
                Err(error) => return Err(error.into()),
            },
            ResumePolicy::Must => {
                if options.id_was_missing {
                    return Err(Error::InvalidRunOptions { field: "id" });
                }
                let existing = self.inner.get_run(&run_id).map_err(Error::from)?;
                self.resume_existing(existing, &project.project_id)?
            }
        };
        let native = Arc::new(self.inner.run_handle(run));
        native.initialize_cursor(next_step).map_err(Error::from)?;
        Ok(RunHandle {
            native,
            client: Arc::clone(&self.inner),
            lifecycle: Arc::new(Mutex::new(FacadeRunState::Open)),
        })
    }

    fn get_or_create_project(&self, raw_project: &str) -> Result<Project> {
        if raw_project.is_empty() {
            return Err(Error::InvalidRunOptions { field: "project" });
        }
        let project_id = ProjectId::from_string(raw_project);
        let _guard = ProjectCreateGuard::acquire(&self.root_path, &project_id)
            .map_err(|_| Error::Storage)?;
        match self.inner.get_project(&project_id) {
            Ok(project) => Ok(project),
            Err(crate::engine::EngineError::ProjectNotFound { .. }) => self
                .inner
                .create_project(raw_project, Some(project_id))
                .map_err(Error::from),
            Err(error) => Err(error.into()),
        }
    }

    fn resume_existing(
        &self,
        run: crate::Run,
        project_id: &ProjectId,
    ) -> Result<(crate::Run, Step)> {
        if &run.project_id != project_id {
            return Err(Error::RunProjectMismatch {
                run_id: run.run_id.as_str().to_owned(),
            });
        }
        let next_step = match self
            .inner
            .greatest_persisted_step(&run.run_id)
            .map_err(Error::from)?
        {
            Some(step) => Step::new(
                step.value()
                    .checked_add(1)
                    .ok_or(Error::StepOverflow { step: step.value() })?,
            ),
            None => Step::new(0),
        };
        let resumed = self.inner.resume_run(&run.run_id).map_err(Error::from)?;
        Ok((resumed, next_step))
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

/// Policy for resolving an optional Run id.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResumePolicy {
    #[default]
    Never,
    Allow,
    Must,
}

/// Options for creating or resuming one Run.
#[derive(Clone, Debug)]
pub struct RunOptions {
    project: String,
    run_id: Option<String>,
    name: Option<String>,
    resume: ResumePolicy,
    id_was_missing: bool,
}

/// Step and cursor behavior for one metric Mapping.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LogOptions {
    step: Option<i64>,
    commit: Option<bool>,
}

impl LogOptions {
    pub const fn new() -> Self {
        Self {
            step: None,
            commit: None,
        }
    }

    pub const fn step(mut self, value: i64) -> Self {
        self.step = Some(value);
        self
    }

    pub const fn commit(mut self, value: bool) -> Self {
        self.commit = Some(value);
        self
    }
}

impl RunOptions {
    pub fn new(project: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            run_id: None,
            name: None,
            resume: ResumePolicy::Never,
            id_was_missing: true,
        }
    }

    pub fn id(mut self, value: impl Into<String>) -> Self {
        self.run_id = Some(value.into());
        self.id_was_missing = false;
        self
    }

    pub fn name(mut self, value: impl Into<String>) -> Self {
        self.name = Some(value.into());
        self
    }

    pub const fn resume(mut self, value: ResumePolicy) -> Self {
        self.resume = value;
        self
    }
}

/// Writable handle for one running Run.
#[derive(Clone)]
pub struct RunHandle {
    native: Arc<NativeRun>,
    client: Arc<NativeClient>,
    lifecycle: Arc<Mutex<FacadeRunState>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FinalizationStage {
    Lifecycle,
    Flush,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FacadeRunState {
    Open,
    Finalizing {
        outcome: RunStatus,
        stage: FinalizationStage,
    },
    Complete {
        outcome: RunStatus,
    },
}

impl RunHandle {
    pub fn log<I, K>(&self, data: I) -> Result<()>
    where
        I: IntoIterator<Item = (K, f64)>,
        K: Into<String>,
    {
        self.log_with(data, LogOptions::new())
    }

    /// Atomically admits one numeric metric Mapping.
    ///
    /// # Errors
    ///
    /// Returns a validation, queue, writer, closed-Run, or cursor error. No
    /// metric or cursor update is admitted on error.
    pub fn log_with<I, K>(&self, data: I, options: LogOptions) -> Result<()>
    where
        I: IntoIterator<Item = (K, f64)>,
        K: Into<String>,
    {
        let mut keys = HashSet::new();
        let mut metrics = Vec::new();
        for (key, value) in data {
            let key = key.into();
            if !keys.insert(key.clone()) {
                return Err(Error::InvalidMetricMapping);
            }
            metrics.push((MetricKey::from_string(key), value));
            if metrics.len() > MAX_METRICS_PER_LOG {
                return Err(Error::MetricMappingTooLarge {
                    count: metrics.len(),
                    maximum: MAX_METRICS_PER_LOG,
                });
            }
        }
        if metrics.is_empty() {
            return Err(Error::InvalidMetricMapping);
        }
        let lifecycle = self.lifecycle.lock().map_err(|_| Error::Storage)?;
        if !matches!(*lifecycle, FacadeRunState::Open) {
            return Err(Error::RunClosed {
                run_id: self.run_id().as_str().to_owned(),
            });
        }
        self.native
            .log_metrics_with_cursor(metrics, options.step.map(Step::new), options.commit)
            .map_err(Error::from)
    }

    pub fn finish(&self) -> Result<()> {
        self.finalize(RunStatus::Finished)
    }

    pub fn fail(&self) -> Result<()> {
        self.finalize(RunStatus::Failed)
    }

    pub fn diagnostics(&self) -> ClientDiagnostics {
        self.client.diagnostics().into()
    }

    fn finalize(&self, requested: RunStatus) -> Result<()> {
        let mut lifecycle = self.lifecycle.lock().map_err(|_| Error::Storage)?;
        let stage = match *lifecycle {
            FacadeRunState::Open => {
                *lifecycle = FacadeRunState::Finalizing {
                    outcome: requested,
                    stage: FinalizationStage::Lifecycle,
                };
                FinalizationStage::Lifecycle
            }
            FacadeRunState::Finalizing { outcome, .. } | FacadeRunState::Complete { outcome }
                if outcome != requested =>
            {
                return Err(Error::TerminalOutcomeConflict {
                    selected: outcome,
                    requested,
                });
            }
            FacadeRunState::Finalizing { stage, .. } => stage,
            FacadeRunState::Complete { .. } => return Ok(()),
        };
        let result = match stage {
            FinalizationStage::Lifecycle => match requested {
                RunStatus::Finished => self.client.finish_run(&self.native.run_id).map(|_| ()),
                RunStatus::Failed => self.client.fail_run(&self.native.run_id).map(|_| ()),
                RunStatus::Running => unreachable!("finalization outcome must be terminal"),
            },
            FinalizationStage::Flush => self.client.flush_run_data(&self.native.run_id, None),
        };
        match result {
            Ok(()) => {
                *lifecycle = FacadeRunState::Complete { outcome: requested };
                Ok(())
            }
            Err(error @ crate::engine::EngineError::MetricFlush { .. })
            | Err(error @ crate::engine::EngineError::MetricFlushTimeout) => {
                *lifecycle = FacadeRunState::Finalizing {
                    outcome: requested,
                    stage: FinalizationStage::Flush,
                };
                Err(error.into())
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn run_id(&self) -> &RunId {
        &self.native.run_id
    }

    pub fn project_id(&self) -> &ProjectId {
        &self.native.project_id
    }

    pub fn name(&self) -> &str {
        &self.native.name
    }

    pub fn status(&self) -> RunStatus {
        self.lifecycle
            .lock()
            .ok()
            .and_then(|state| match *state {
                FacadeRunState::Finalizing {
                    outcome,
                    stage: FinalizationStage::Flush,
                }
                | FacadeRunState::Complete { outcome } => Some(outcome),
                _ => None,
            })
            .unwrap_or(self.native.status)
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
