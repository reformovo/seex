//! Public read-query contracts.

#[cfg(test)]
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::{CatalogBackend, S3Options};
use crate::engine::query::NativeQueryStore;
use crate::error::{Error, Result as SdkResult};
use crate::model::alignment::{
    AlignedMetricPoint, AlignmentAxis, AlignmentQuery, AlignmentReason, AlignmentReduction,
    AlignmentViewport,
};
use crate::model::comparison::{
    EvidenceCompleteness, EvidenceReason, ObjectiveEvidence, ObjectiveMetric,
};
use crate::model::metric::{
    MetricAggregate, MetricKey, MetricPoint, MetricQuery as StorageMetricQuery, ReductionPolicy,
    Step,
};
use crate::model::run::{Run, RunId, RunStatus};
use crate::model::types::{Project, ProjectId};
use crate::storage::bootstrap::{
    NativeReadDatabase, NativeStorageConfig, is_s3_data_path,
    open_existing_native_connection_with_config, open_shared_existing_local_connection_with_config,
};
use crate::storage::config::{S3ConnectionOverrides, resolve_init_config, resolve_storage_config};
use crate::storage::{ProjectConnection, ProjectMetricReader, SeriesDiagnostics, StorageError};

/// Builder for opening one existing native store read-only.
pub struct ReaderBuilder {
    root_path: PathBuf,
    catalog_backend: Option<CatalogBackend>,
    catalog_path: Option<PathBuf>,
    data_path: Option<PathBuf>,
    s3: S3Options,
}

/// Maximum number of real points returned for one chart curve.
pub const MAX_CHART_POINTS: usize = 1_024;
/// Maximum number of Runs returned by one discovery request.
pub const MAX_RUN_PAGE_SIZE: usize = 100;
const MAX_RUN_METADATA_CACHE: usize = 4_096;
const MAX_FINISHED_DIAGNOSTICS_CACHE: usize = 4_096;
const MAX_VOLATILE_DIAGNOSTICS_CACHE: usize = 1_024;

/// Failure while opening a Reader restricted to local data.
#[doc(hidden)]
#[derive(Debug)]
pub enum LocalReaderError {
    UnsupportedS3,
    Sdk(Error),
}

impl fmt::Display for LocalReaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedS3 => formatter.write_str("S3 data paths are unsupported"),
            Self::Sdk(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for LocalReaderError {}

impl From<Error> for LocalReaderError {
    fn from(error: Error) -> Self {
        Self::Sdk(error)
    }
}

/// Cloneable cancellation capability for one Reader connection.
#[doc(hidden)]
#[derive(Clone)]
pub struct ReaderInterrupt(crate::storage::ReadInterrupt);

impl ReaderInterrupt {
    pub fn interrupt(&self) {
        self.0.interrupt();
    }
}

#[cfg(feature = "test-query-probe")]
static QUERY_PROBE: std::sync::Mutex<Option<QueryProbe>> = std::sync::Mutex::new(None);
#[cfg(feature = "test-query-probe")]
struct QueryProbe {
    run_id: RunId,
    metric_key: MetricKey,
    started: std::sync::mpsc::Sender<(ProjectConnection, ReaderInterrupt)>,
    result: std::sync::mpsc::Sender<bool>,
    cleanup: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Opt-in integration probe; never available in a default SDK build.
#[cfg(feature = "test-query-probe")]
#[doc(hidden)]
pub struct ExecutingQueryProbe {
    started: std::sync::mpsc::Receiver<(ProjectConnection, ReaderInterrupt)>,
    result: std::sync::mpsc::Receiver<bool>,
    interrupt: Option<ReaderInterrupt>,
    cleanup: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
#[cfg(feature = "test-query-probe")]
impl ExecutingQueryProbe {
    pub fn wait_until_running(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = std::time::Instant::now() + timeout;
        let (observer, interrupt) = self.started.recv_timeout(timeout)?;
        self.interrupt = Some(interrupt);
        loop {
            let value: Option<i64> = observer.query_row(
                "SELECT last_value FROM duckdb_sequences() WHERE sequence_name = 'seex_worker_probe'",
                [], |row| row.get(0))?;
            if value.is_some_and(|v| v > 0) {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err("SQL probe never advanced".into());
            }
            std::thread::yield_now();
        }
    }
    pub fn wait_interrupted(
        &self,
        timeout: std::time::Duration,
    ) -> Result<bool, std::sync::mpsc::RecvTimeoutError> {
        self.result.recv_timeout(timeout)
    }
}
#[cfg(feature = "test-query-probe")]
impl Drop for ExecutingQueryProbe {
    fn drop(&mut self) {
        self.cleanup
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(handle) = &self.interrupt {
            handle.interrupt();
        }
        let mut slot = QUERY_PROBE.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|p| std::sync::Arc::ptr_eq(&p.cleanup, &self.cleanup))
        {
            slot.take();
        }
    }
}

/// One Run's summary and objective evidence for the Desktop inspector.
#[doc(hidden)]
pub struct DesktopMetricEvidence {
    pub run: Run,
    pub summary: Option<MetricAggregate>,
    pub evidence: ObjectiveEvidence,
}

impl ReaderBuilder {
    pub fn new(root_path: impl Into<PathBuf>) -> Self {
        Self {
            root_path: root_path.into(),
            catalog_backend: None,
            catalog_path: None,
            data_path: None,
            s3: S3Options::default(),
        }
    }

    /// Selects the native catalog backend explicitly.
    pub fn catalog_backend(mut self, value: CatalogBackend) -> Self {
        self.catalog_backend = Some(value);
        self
    }

    /// Selects the local native catalog path explicitly.
    pub fn catalog_path(mut self, value: impl Into<PathBuf>) -> Self {
        self.catalog_path = Some(value.into());
        self
    }

    /// Selects the native Parquet data path explicitly.
    pub fn data_path(mut self, value: impl Into<PathBuf>) -> Self {
        self.data_path = Some(value.into());
        self
    }

    /// Supplies connection-local S3 overrides for an S3 data path.
    pub fn s3_options(mut self, value: S3Options) -> Self {
        self.s3 = value;
        self
    }

    /// Opens the configured store without starting a writer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] for invalid effective configuration or
    /// [`Error::Storage`] when the existing native store cannot be opened.
    pub fn open(self) -> SdkResult<Reader> {
        self.open_impl(false).map_err(|error| match error {
            LocalReaderError::UnsupportedS3 => Error::UnsupportedQuery,
            LocalReaderError::Sdk(error) => error,
        })
    }

    /// Opens the configured native store only when its data path is local.
    #[doc(hidden)]
    pub fn open_local(self) -> std::result::Result<Reader, LocalReaderError> {
        self.open_impl(true)
    }

    fn open_impl(self, local_only: bool) -> std::result::Result<Reader, LocalReaderError> {
        let root_path = self.root_path;
        if local_only {
            let storage = resolve_storage_config(
                &root_path,
                self.data_path.clone(),
                self.catalog_backend.map(CatalogBackend::as_name),
                self.catalog_path.clone(),
            )
            .map_err(|_| Error::Configuration)?;
            if storage.data_path.as_deref().is_some_and(is_s3_data_path) {
                return Err(LocalReaderError::UnsupportedS3);
            }
        }
        let resolved = resolve_init_config(
            &root_path,
            self.data_path,
            self.catalog_backend.map(CatalogBackend::as_name),
            self.catalog_path,
            1,
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
        let config = NativeStorageConfig::with_backend_and_s3_config(
            resolved.catalog_backend,
            &root_path,
            resolved.catalog_path,
            resolved.data_path,
            resolved.s3_connection,
        );
        let retry_config = config.clone();
        let (connection, native_database) = if local_only {
            let (connection, database) = open_shared_existing_local_connection_with_config(config)
                .map_err(public_storage_error)?;
            (connection, Some(database))
        } else {
            (
                open_existing_native_connection_with_config(config)
                    .map_err(public_storage_error)?,
                None,
            )
        };
        Ok(Reader {
            connection: Some(ProjectConnection::new(connection)),
            _native_database: native_database,
            retry_config,
            run_metadata: RefCell::new(HashMap::new()),
            diagnostics: RefCell::new(DiagnosticsCache::default()),
            #[cfg(test)]
            diagnostics_loads: Cell::new(0),
        })
    }
}

/// Read-only discovery and metric-query entry point.
pub struct Reader {
    connection: Option<ProjectConnection>,
    _native_database: Option<NativeReadDatabase>,
    retry_config: NativeStorageConfig,
    run_metadata: RefCell<HashMap<RunId, RunMetadata>>,
    diagnostics: RefCell<DiagnosticsCache>,
    #[cfg(test)]
    diagnostics_loads: Cell<usize>,
}

#[derive(Clone)]
struct RunMetadata {
    project_id: Option<ProjectId>,
    status: RunStatus,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct DiagnosticsKey {
    project_id: Option<ProjectId>,
    run_id: RunId,
    metric_key: MetricKey,
}

#[derive(Clone, Copy)]
struct AxisBounds {
    start: Option<i64>,
    end: Option<i64>,
}

impl AxisBounds {
    const fn new(start: Option<i64>, end: Option<i64>) -> Self {
        Self { start, end }
    }

    fn viewport(self) -> SdkResult<AlignmentViewport> {
        let start = self.start.unwrap_or(i64::MIN);
        let end = self
            .end
            .map(|value| value.checked_sub(1).ok_or(Error::UnsupportedQuery))
            .transpose()?
            .unwrap_or(i64::MAX);
        AlignmentViewport::new(start, end).map_err(|_| Error::UnsupportedQuery)
    }

    fn contains(self, value: i64) -> bool {
        self.start.is_none_or(|start| start <= value) && self.end.is_none_or(|end| value < end)
    }
}

#[derive(Clone, Default)]
struct DiagnosticsCache {
    generation: Option<u64>,
    finished: HashMap<DiagnosticsKey, SeriesDiagnostics>,
    volatile: HashMap<DiagnosticsKey, SeriesDiagnostics>,
}

impl Reader {
    /// Arm a one-shot probe for exactly one Run/key in serialized integration tests.
    #[cfg(feature = "test-query-probe")]
    #[doc(hidden)]
    pub fn arm_executing_query_probe(run_id: RunId, metric_key: MetricKey) -> ExecutingQueryProbe {
        let (started_tx, started) = std::sync::mpsc::channel();
        let (result_tx, result) = std::sync::mpsc::channel();
        let mut slot = QUERY_PROBE.lock().unwrap();
        assert!(slot.is_none(), "query probe already armed");
        let cleanup = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *slot = Some(QueryProbe {
            run_id: run_id.clone(),
            metric_key: metric_key.clone(),
            started: started_tx,
            result: result_tx,
            cleanup: cleanup.clone(),
        });
        ExecutingQueryProbe {
            started,
            result,
            interrupt: None,
            cleanup,
        }
    }

    #[cfg(feature = "test-query-probe")]
    fn execute_armed_probe(&self, run_id: &RunId, metric_key: &MetricKey) -> SdkResult<()> {
        let probe = {
            let mut slot = QUERY_PROBE.lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(|p| &p.run_id == run_id && &p.metric_key == metric_key)
            {
                slot.take()
            } else {
                None
            }
        };
        let Some(probe) = probe else {
            return Ok(());
        };
        let connection = self.native()?;
        connection
            .execute_batch("CREATE SEQUENCE memory.main.seex_worker_probe;")
            .map_err(|_| Error::Storage)?;
        probe
            .started
            .send((
                connection.try_clone().map_err(|_| Error::Storage)?,
                self.interrupt_handle().ok_or(Error::Storage)?,
            ))
            .map_err(|_| Error::Storage)?;
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished = done.clone();
        let cleanup = probe.cleanup.clone();
        let handle = self.interrupt_handle().ok_or(Error::Storage)?;
        let watchdog = std::thread::spawn(move || {
            while !finished.load(std::sync::atomic::Ordering::Acquire) {
                if cleanup.load(std::sync::atomic::Ordering::Acquire) {
                    handle.interrupt();
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        let result = connection.query_row(
            "SELECT sum(sin(i::DOUBLE) + nextval('memory.main.seex_worker_probe')::DOUBLE)
             FROM range(1000000000) AS data(i)",
            [],
            |row| row.get::<_, f64>(0),
        );
        done.store(true, std::sync::atomic::Ordering::Release);
        watchdog.join().expect("probe cleanup watchdog panicked");
        let interrupted = result
            .as_ref()
            .err()
            .is_some_and(|e| e.to_string().to_uppercase().contains("INTERRUPT"));
        let _ = probe.result.send(interrupted);
        result.map(|_| ()).map_err(|_| Error::Storage)
    }
    pub fn builder(root_path: impl AsRef<Path>) -> ReaderBuilder {
        ReaderBuilder::new(root_path.as_ref().to_owned())
    }

    fn native(&self) -> SdkResult<&ProjectConnection> {
        self.connection.as_ref().ok_or(Error::UnsupportedQuery)
    }

    /// Lists Projects in stable catalog order.
    pub fn projects(&self) -> SdkResult<Vec<Project>> {
        self.native()?.list_projects().map_err(|_| Error::Storage)
    }

    /// Gets one Project when it exists.
    pub fn project(&self, project_id: &ProjectId) -> SdkResult<Option<Project>> {
        self.native()?
            .get_project(project_id)
            .map_err(|_| Error::Storage)
    }

    /// Lists the first bounded page of Runs for one Project, newest first.
    pub fn runs(&self, project_id: &ProjectId) -> SdkResult<Vec<Run>> {
        self.runs_page(project_id, None, MAX_RUN_PAGE_SIZE)
    }

    /// Lists Runs after the last Run of a prior page in stable descending order.
    pub fn runs_page(
        &self,
        project_id: &ProjectId,
        after: Option<&Run>,
        limit: usize,
    ) -> SdkResult<Vec<Run>> {
        if limit == 0
            || limit > MAX_RUN_PAGE_SIZE
            || after.is_some_and(|run| &run.project_id != project_id)
        {
            return Err(Error::UnsupportedQuery);
        }
        let runs = self
            .native()?
            .list_runs_page(project_id, after, limit)
            .map_err(|_| Error::Storage)?;
        self.remember_runs(&runs);
        Ok(runs)
    }

    /// Finds a metric key in the indexed global dictionary.
    pub fn metric_key_exists(&self, metric_key: &MetricKey) -> SdkResult<bool> {
        self.native()?
            .metric_key_exists(metric_key)
            .map_err(|_| Error::Storage)
    }

    /// Lists an indexed prefix page from the global metric key dictionary.
    pub fn metric_keys_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> SdkResult<Vec<MetricKey>> {
        if limit == 0 || limit > MAX_RUN_PAGE_SIZE {
            return Err(Error::UnsupportedQuery);
        }
        self.native()?
            .metric_keys_page(prefix, after, limit)
            .map_err(|_| Error::Storage)
    }

    /// Lists an indexed page of actual metric keys for one Run.
    pub fn series_keys_page(
        &self,
        run: &Run,
        after: Option<&str>,
        limit: usize,
    ) -> SdkResult<Vec<MetricKey>> {
        if limit == 0 || limit > MAX_RUN_PAGE_SIZE {
            return Err(Error::UnsupportedQuery);
        }
        self.native()?
            .series_keys_page(&run.run_id, after, limit)
            .map_err(|_| Error::Storage)
    }

    /// Gets one Run only when it belongs to the requested Project.
    pub fn run(&self, project_id: &ProjectId, run_id: &RunId) -> SdkResult<Option<Run>> {
        let run = match self.native()?.get_run(run_id) {
            Ok(run) => run,
            Err(StorageError::RunNotFound { .. }) => return Ok(None),
            Err(_) => return Err(Error::Storage),
        };
        if &run.project_id != project_id {
            return Ok(None);
        }
        self.remember_runs(std::slice::from_ref(&run));
        Ok(Some(run))
    }

    /// Loads Desktop-selected Runs in request order.
    #[doc(hidden)]
    pub fn runs_for_desktop(&self, run_ids: &[RunId]) -> SdkResult<Vec<Run>> {
        let runs = self
            .native()?
            .get_runs(run_ids)
            .map_err(|_| Error::Storage)?;
        self.remember_runs(&runs);
        Ok(runs)
    }

    /// Lists persisted Metric summaries for one Run.
    pub fn metrics(&self, run: &Run) -> SdkResult<Vec<MetricAggregate>> {
        self.remember_runs(std::slice::from_ref(run));
        let result = ProjectMetricReader::new(self.native()?).list_metrics(&run.run_id, run.status);
        match result {
            Ok(metrics) => Ok(metrics),
            Err(_) => ProjectMetricReader::new(self.fresh_query_reader()?.native()?)
                .list_metrics(&run.run_id, run.status)
                .map_err(|_| Error::Storage),
        }
    }

    /// Gets one persisted Metric summary when the Run contains that Metric.
    pub fn metric_summary(
        &self,
        run: &Run,
        metric_key: &MetricKey,
    ) -> SdkResult<Option<MetricAggregate>> {
        self.remember_runs(std::slice::from_ref(run));
        let query = |reader: &Reader| {
            ProjectMetricReader::new(reader.native()?)
                .query_metric_summaries(std::slice::from_ref(&run.run_id), metric_key)
                .map_err(|_| Error::Storage)
        };
        match query(self) {
            Ok(summaries) => Ok(summaries.into_iter().next()),
            Err(_) => Ok(query(&self.fresh_query_reader()?)?.into_iter().next()),
        }
    }

    /// Queries one axis with a strict caller-selected point bound.
    pub fn query_metric(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        query: &MetricQuery,
    ) -> SdkResult<MetricSeries> {
        self.diagnostics.borrow_mut().volatile.clear();
        self.query_metric_impl(run_id, metric_key, query, false, false)
    }

    /// Selects the current Desktop storage generation for volatile diagnostics.
    #[doc(hidden)]
    pub fn refresh_diagnostics(&self, generation: u64) {
        let mut cache = self.diagnostics.borrow_mut();
        if cache.generation != Some(generation) {
            cache.generation = Some(generation);
            cache.volatile.clear();
        }
    }

    /// Returns cancellation capability for this Reader's native connection.
    #[doc(hidden)]
    pub fn interrupt_handle(&self) -> Option<ReaderInterrupt> {
        self.connection
            .as_ref()
            .map(ProjectConnection::interrupt_handle)
            .map(ReaderInterrupt)
    }

    /// Loads Run summaries and objective evidence for one Desktop inspector.
    #[doc(hidden)]
    pub fn inspect_metric_for_desktop(
        &self,
        run_ids: &[RunId],
        metric_key: &MetricKey,
        is_superseded: &mut dyn FnMut() -> bool,
    ) -> SdkResult<Option<Vec<DesktopMetricEvidence>>> {
        if is_superseded() {
            return Ok(None);
        }
        let connection = self.native()?;
        let runs = connection.get_runs(run_ids).map_err(public_storage_error)?;
        if is_superseded() {
            return Ok(None);
        }
        let store = NativeQueryStore::new(connection);
        let mut summaries = store
            .query_metric_summaries(run_ids, metric_key)
            .map_err(Error::from)?
            .into_iter()
            .map(|summary| (summary.run_id.clone(), summary))
            .collect::<HashMap<_, _>>();
        if is_superseded() {
            return Ok(None);
        }
        let objective = ObjectiveMetric {
            metric_key: metric_key.clone(),
            direction: crate::model::comparison::ObjectiveDirection::Minimize,
        };
        let evidence = store
            .objective_evidence_for_runs(&runs, &objective)
            .map_err(Error::from)?;
        if is_superseded() {
            return Ok(None);
        }
        Ok(Some(
            runs.into_iter()
                .zip(evidence)
                .map(|(run, evidence)| DesktopMetricEvidence {
                    summary: summaries.remove(&run.run_id),
                    run,
                    evidence,
                })
                .collect(),
        ))
    }

    /// Desktop-only query retaining one real sample outside each range edge.
    ///
    /// The stable [`Reader::query_metric`] contract never returns these
    /// neighbors. This adapter may return at most two neighbors while keeping
    /// the complete result within the query's `max_points` bound.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] for unsupported queries or storage failures.
    #[doc(hidden)]
    pub fn query_metric_for_desktop(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        query: &MetricQuery,
    ) -> SdkResult<MetricSeries> {
        self.query_metric_impl(run_id, metric_key, query, true, false)
    }

    /// Desktop Overview query retaining neighbors on the incumbent plan.
    #[doc(hidden)]
    pub fn query_metric_overview_for_desktop(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        query: &MetricQuery,
    ) -> SdkResult<MetricSeries> {
        self.query_metric_impl(run_id, metric_key, query, true, true)
    }

    /// Request-scoped connection; Storage failures discard and retry the full panel once.
    #[doc(hidden)]
    pub fn query_metrics_for_desktop(
        &self,
        run_ids: &[RunId],
        metric_key: &MetricKey,
        query: &MetricQuery,
        overview: bool,
        is_superseded: &mut dyn FnMut() -> bool,
    ) -> SdkResult<Option<Vec<MetricSeries>>> {
        self.query_metrics_for_desktop_with_interrupt(
            run_ids,
            metric_key,
            query,
            overview,
            is_superseded,
            &mut |_| true,
            &mut |_| {},
        )
    }

    /// Desktop-only registration of each request-scoped connection before SQL.
    /// Returning false rejects an already-cancelled attempt without executing it.
    #[doc(hidden)]
    pub fn query_metrics_for_desktop_with_interrupt(
        &self,
        run_ids: &[RunId],
        metric_key: &MetricKey,
        query: &MetricQuery,
        overview: bool,
        is_superseded: &mut dyn FnMut() -> bool,
        register: &mut dyn FnMut(ReaderInterrupt) -> bool,
        trace: &mut dyn FnMut(&'static str),
    ) -> SdkResult<Option<Vec<MetricSeries>>> {
        if run_ids.len() > 16 {
            return Err(Error::UnsupportedQuery);
        }
        if run_ids.is_empty() {
            return Ok(Some(Vec::new()));
        }
        if is_superseded() {
            return Ok(None);
        }
        let trace = std::cell::RefCell::new(trace);
        let cancelled = std::cell::RefCell::new(is_superseded);
        self.with_fresh_query_reader_checked(
            &mut || !(cancelled.borrow_mut())(),
            register,
            &mut |phase| (trace.borrow_mut())(phase),
            |reader| {
                let mut results = Vec::with_capacity(run_ids.len());
                for run_id in run_ids {
                    if (cancelled.borrow_mut())() {
                        return Ok(None);
                    }
                    let result = reader.query_metric_once_traced(
                        run_id,
                        metric_key,
                        query,
                        true,
                        overview,
                        &mut |phase| {
                            (trace.borrow_mut())(phase);
                        },
                    );
                    // An interrupt maps to Storage too; cancellation must not retry SQL.
                    if (cancelled.borrow_mut())() {
                        return Ok(None);
                    }
                    results.push(result?);
                }
                if (cancelled.borrow_mut())() {
                    return Ok(None);
                }
                Ok(Some(results))
            },
        )
    }

    fn query_metric_impl(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        query: &MetricQuery,
        retain_neighbors: bool,
        force_full_step: bool,
    ) -> SdkResult<MetricSeries> {
        self.with_fresh_query_reader(|reader| {
            reader
                .query_metric_once(run_id, metric_key, query, retain_neighbors, force_full_step)
                .map(Some)
        })?
        .ok_or(Error::Storage)
    }

    fn with_fresh_query_reader<T>(
        &self,
        operation: impl FnMut(&Self) -> SdkResult<Option<T>>,
    ) -> SdkResult<Option<T>> {
        self.with_fresh_query_reader_registered(&mut |_| true, &mut |_| {}, operation)
    }

    fn with_fresh_query_reader_registered<T>(
        &self,
        register: &mut dyn FnMut(ReaderInterrupt) -> bool,
        trace: &mut dyn FnMut(&'static str),
        operation: impl FnMut(&Self) -> SdkResult<Option<T>>,
    ) -> SdkResult<Option<T>> {
        self.with_fresh_query_reader_checked(&mut || true, register, trace, operation)
    }

    fn with_fresh_query_reader_checked<T>(
        &self,
        before_open: &mut dyn FnMut() -> bool,
        register: &mut dyn FnMut(ReaderInterrupt) -> bool,
        trace: &mut dyn FnMut(&'static str),
        mut operation: impl FnMut(&Self) -> SdkResult<Option<T>>,
    ) -> SdkResult<Option<T>> {
        for attempt in 0..2 {
            if !before_open() {
                trace("fresh_open_skipped");
                return Ok(None);
            }
            trace("fresh_open_start");
            let reader = match self.fresh_query_reader() {
                Err(Error::Storage) if attempt == 0 => continue,
                result => result?,
            };
            if attempt != 0 {
                reader.diagnostics.borrow_mut().volatile.clear();
            }
            trace("fresh_open_finish");
            if !register(reader.interrupt_handle().ok_or(Error::Storage)?) {
                trace("fresh_register_rejected");
                return Ok(None);
            }
            trace("query_start");
            match operation(&reader) {
                Err(Error::Storage) if attempt == 0 => continue,
                Ok(Some(value)) => {
                    self.remember_query_diagnostics(&reader);
                    return Ok(Some(value));
                }
                result => return result,
            }
        }
        Err(Error::Storage)
    }

    fn remember_query_diagnostics(&self, query_reader: &Self) {
        self.diagnostics
            .replace(query_reader.diagnostics.borrow().clone());
        #[cfg(test)]
        self.diagnostics_loads
            .set(query_reader.diagnostics_loads.get());
    }

    fn fresh_query_reader(&self) -> SdkResult<Self> {
        let started = std::time::Instant::now();
        let connection = open_existing_native_connection_with_config(self.retry_config.clone())
            .map_err(public_storage_error)?;
        crate::storage::bootstrap::record_write_timing("reader_connection_open", started.elapsed());
        Ok(Self {
            connection: Some(ProjectConnection::new(connection)),
            _native_database: None,
            retry_config: self.retry_config.clone(),
            run_metadata: RefCell::new(HashMap::new()),
            diagnostics: RefCell::new(self.diagnostics.borrow().clone()),
            #[cfg(test)]
            diagnostics_loads: Cell::new(self.diagnostics_loads.get()),
        })
    }

    fn query_metric_once(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        query: &MetricQuery,
        retain_neighbors: bool,
        force_full_step: bool,
    ) -> SdkResult<MetricSeries> {
        self.query_metric_once_traced(
            run_id,
            metric_key,
            query,
            retain_neighbors,
            force_full_step,
            &mut |_| {},
        )
    }

    fn query_metric_once_traced(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        query: &MetricQuery,
        retain_neighbors: bool,
        force_full_step: bool,
        trace: &mut dyn FnMut(&'static str),
    ) -> SdkResult<MetricSeries> {
        #[cfg(feature = "test-query-probe")]
        self.execute_armed_probe(run_id, metric_key)?;
        trace("metadata_start");
        let metadata = self.metadata(run_id)?;
        trace("metadata_finish");

        let run_status = metadata.status;
        trace("diagnostics_start");
        let diagnostics = self.series_diagnostics(run_id, metric_key, &metadata);
        trace("diagnostics_finish");
        let (axis, storage_axis, bounds) = match query.range() {
            MetricRange::All(axis) => (*axis, AlignmentAxis::Step, None),
            MetricRange::Steps { start, end } => (
                MetricAxis::Step,
                AlignmentAxis::Step,
                Some(AxisBounds::new(Some(start.value()), Some(end.value()))),
            ),
            MetricRange::StepsFrom { start } => (
                MetricAxis::Step,
                AlignmentAxis::Step,
                Some(AxisBounds::new(Some(start.value()), None)),
            ),
            MetricRange::StepsUntil { end } => (
                MetricAxis::Step,
                AlignmentAxis::Step,
                Some(AxisBounds::new(None, Some(end.value()))),
            ),
            // MetricQuery::new rejects these axes. Keep the match exhaustive so
            // the internal range types can be retired independently later.
            MetricRange::RelativeTime { .. }
            | MetricRange::RelativeTimeFrom { .. }
            | MetricRange::RelativeTimeUntil { .. }
            | MetricRange::Timestamps { .. }
            | MetricRange::TimestampsFrom { .. }
            | MetricRange::TimestampsUntil { .. } => return Err(Error::UnsupportedQuery),
        };
        if axis == MetricAxis::Step && !retain_neighbors {
            return self.query_step_metric(run_id, metric_key, query, &metadata, diagnostics);
        }
        let viewport = bounds.unwrap_or(AxisBounds::new(None, None)).viewport()?;
        let reduction = AlignmentReduction::screen_budget(
            u32::try_from(query.max_points()).unwrap_or(u32::MAX),
            1,
        )
        .map_err(|_| Error::UnsupportedQuery)?;
        let storage_query = AlignmentQuery {
            run_id: run_id.clone(),
            metric_key: metric_key.clone(),
            axis: storage_axis,
            viewport,
            reduction,
        };
        let narrow_step = use_narrow_step_plan(axis, bounds, diagnostics, force_full_step);
        let bounded_full_count = self
            .connection
            .is_some()
            .then(|| use_bounded_full_plan(axis, bounds, diagnostics, narrow_step))
            .filter(|bounded| *bounded)
            .and(diagnostics.map(|diagnostics| diagnostics.effective_count));
        let connection = self.connection.as_ref().ok_or(Error::Storage)?;
        trace("storage_start");
        let result = match (narrow_step, bounded_full_count) {
            (true, None) => {
                ProjectMetricReader::new(connection).query_narrow_step_metric(&storage_query)
            }
            (false, Some(source_count)) => ProjectMetricReader::new(connection)
                .query_bounded_full_aligned_metric(&storage_query, source_count),
            (false, None) => {
                ProjectMetricReader::new(connection).query_aligned_metric(&storage_query)
            }
            (true, Some(_)) => return Err(Error::Storage),
        }
        .map_err(public_storage_error)?;
        trace("storage_finish");
        let neighbor_count = result
            .points
            .iter()
            .filter(|point| !in_half_open_range(point.axis_value, bounds))
            .count() as u64;
        let points = if retain_neighbors {
            bounded_desktop_points(result.points, bounds, query.max_points())
        } else {
            result
                .points
                .into_iter()
                .filter(|point| in_half_open_range(point.axis_value, bounds))
                .collect()
        };
        trace("points_start");
        let mut samples = points
            .into_iter()
            .map(|point| {
                let coordinate = match axis {
                    MetricAxis::Step => MetricCoordinate::Step(Step::new(point.axis_value)),
                    MetricAxis::RelativeTime => {
                        MetricCoordinate::RelativeTime(RelativeTime::from_millis(point.axis_value))
                    }
                    MetricAxis::Timestamp => MetricCoordinate::Timestamp(Timestamp::from_millis(
                        point.point.timestamp.timestamp_millis(),
                    )),
                };
                MetricSample {
                    coordinate,
                    point: point.point,
                }
            })
            .collect::<Vec<_>>();
        if !retain_neighbors {
            samples = enforce_point_bound(samples, query.max_points());
        }
        let source_count = if retain_neighbors {
            result.source_row_count
        } else {
            result.source_row_count.saturating_sub(neighbor_count)
        };
        let (completeness, reasons) = qualify_series(result.reasons, diagnostics, run_status);
        let series = MetricSeries::from_samples(axis, samples, source_count, completeness, reasons)
            .map_err(|_| Error::Storage)?;
        trace("points_finish");
        Ok(series)
    }

    fn query_step_metric(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        query: &MetricQuery,
        metadata: &RunMetadata,
        diagnostics: Option<SeriesDiagnostics>,
    ) -> SdkResult<MetricSeries> {
        let (start, end) = match query.range() {
            MetricRange::All(MetricAxis::Step) => (None, None),
            MetricRange::Steps { start, end } => (Some(*start), Some(*end)),
            MetricRange::StepsFrom { start } => (Some(*start), None),
            MetricRange::StepsUntil { end } => (None, Some(*end)),
            _ => return Err(Error::UnsupportedQuery),
        };
        let reduction = ReductionPolicy::screen_budget(
            u32::try_from(query.max_points()).unwrap_or(u32::MAX),
            1,
        )
        .map_err(|_| Error::UnsupportedQuery)?;
        let storage_query =
            StorageMetricQuery::new(run_id.clone(), metric_key.clone(), start, end, reduction)
                .map_err(|_| Error::UnsupportedQuery)?;
        let connection = self.connection.as_ref().ok_or(Error::Storage)?;
        let started = std::time::Instant::now();
        let result = ProjectMetricReader::new(connection)
            .query_metric(&storage_query)
            .map_err(public_storage_error)?;
        crate::storage::bootstrap::record_write_timing("reader_points_query", started.elapsed());
        let mut samples = result
            .points
            .into_iter()
            .map(|point| MetricSample {
                coordinate: MetricCoordinate::Step(point.step),
                point,
            })
            .collect::<Vec<_>>();
        samples = enforce_point_bound(samples, query.max_points());
        let (completeness, reasons) = qualify_series(Vec::new(), diagnostics, metadata.status);
        MetricSeries::from_samples(
            MetricAxis::Step,
            samples,
            result.source_row_count,
            completeness,
            reasons,
        )
        .map_err(|_| Error::Storage)
    }

    fn series_diagnostics(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        metadata: &RunMetadata,
    ) -> Option<SeriesDiagnostics> {
        let key = DiagnosticsKey {
            project_id: metadata.project_id.clone(),
            run_id: run_id.clone(),
            metric_key: metric_key.clone(),
        };
        let finished = metadata.status == RunStatus::Finished;
        let cached = self.diagnostics.borrow();
        let diagnostics = if finished {
            cached.finished.get(&key)
        } else {
            cached.volatile.get(&key)
        };
        if let Some(diagnostics) = diagnostics {
            return Some(*diagnostics);
        }
        drop(cached);
        #[cfg(test)]
        self.diagnostics_loads.set(self.diagnostics_loads.get() + 1);
        let loaded = ProjectMetricReader::new(self.connection.as_ref()?)
            .series_diagnostics(run_id, metric_key)
            .ok()?;
        let mut cache = self.diagnostics.borrow_mut();
        if finished {
            if cache.finished.len() >= MAX_FINISHED_DIAGNOSTICS_CACHE {
                cache.finished.clear();
            }
            cache.finished.insert(key, loaded);
        } else {
            if cache.volatile.len() >= MAX_VOLATILE_DIAGNOSTICS_CACHE {
                cache.volatile.clear();
            }
            cache.volatile.insert(key, loaded);
        }
        Some(loaded)
    }

    fn metadata(&self, run_id: &RunId) -> SdkResult<RunMetadata> {
        if let Some(metadata) = self.run_metadata.borrow().get(run_id).cloned() {
            return Ok(metadata);
        }
        let metadata = match &self.connection {
            Some(connection) => {
                let run = connection.get_run(run_id).map_err(|_| Error::Storage)?;
                RunMetadata::from(&run)
            }
            None => return Err(Error::Storage),
        };
        let mut cached = self.run_metadata.borrow_mut();
        if cached.len() >= MAX_RUN_METADATA_CACHE {
            cached.clear();
        }
        cached.insert(run_id.clone(), metadata.clone());
        Ok(metadata)
    }

    fn remember_runs(&self, runs: &[Run]) {
        let runs = runs
            .get(runs.len().saturating_sub(MAX_RUN_METADATA_CACHE)..)
            .unwrap_or(runs);
        let mut cached = self.run_metadata.borrow_mut();
        if cached.len().saturating_add(runs.len()) > MAX_RUN_METADATA_CACHE {
            cached.clear();
        }
        cached.extend(
            runs.iter()
                .map(|run| (run.run_id.clone(), RunMetadata::from(run))),
        );
    }
}

impl From<&Run> for RunMetadata {
    fn from(run: &Run) -> Self {
        Self {
            project_id: Some(run.project_id.clone()),
            status: run.status,
        }
    }
}

fn use_narrow_step_plan(
    axis: MetricAxis,
    bounds: Option<AxisBounds>,
    diagnostics: Option<SeriesDiagnostics>,
    force_full: bool,
) -> bool {
    let (Some(bounds), Some(diagnostics)) = (bounds, diagnostics) else {
        return false;
    };
    if force_full || axis != MetricAxis::Step || diagnostics.effective_count == 0 {
        return false;
    }
    match (diagnostics.min_step, diagnostics.max_step) {
        (Some(min_step), Some(max_step)) => {
            !(bounds.start.is_none_or(|start| start <= min_step)
                && bounds.end.is_none_or(|end| max_step < end))
        }
        _ => false,
    }
}

fn use_bounded_full_plan(
    axis: MetricAxis,
    bounds: Option<AxisBounds>,
    diagnostics: Option<SeriesDiagnostics>,
    narrow_step: bool,
) -> bool {
    let Some(diagnostics) = diagnostics else {
        return false;
    };
    if narrow_step || !bounded_reduction_is_safe(axis, diagnostics) {
        return false;
    }
    match (axis, bounds, diagnostics.min_step, diagnostics.max_step) {
        (_, None, _, _) => true,
        (MetricAxis::Step, Some(bounds), Some(minimum), Some(maximum)) => {
            bounds.start.is_none_or(|start| start <= minimum)
                && bounds.end.is_none_or(|end| maximum < end)
        }
        _ => false,
    }
}

fn bounded_reduction_is_safe(axis: MetricAxis, diagnostics: SeriesDiagnostics) -> bool {
    !diagnostics.has_negative_step
        && !diagnostics.has_non_finite_value
        // Timestamp order qualifies evidence, but cannot change the Step reduction axis.
        && (axis == MetricAxis::Step || !diagnostics.has_decreasing_timestamp)
}

fn public_storage_error(error: StorageError) -> Error {
    match error {
        StorageError::CatalogNotFound { name } => Error::CatalogNotFound { name },
        StorageError::LttbExtensionUnavailable { message } => {
            Error::LttbExtensionUnavailable { message }
        }
        StorageError::RunNotFound { run_id } => Error::RunNotFound { run_id },
        _ => Error::Storage,
    }
}

fn in_half_open_range(value: i64, bounds: Option<AxisBounds>) -> bool {
    bounds.is_none_or(|bounds| bounds.contains(value))
}

#[cfg(test)]
pub(crate) fn bound_quality_samples(
    points: Vec<AlignedMetricPoint>,
    start: i64,
    end: i64,
    budget: usize,
) -> Vec<AlignedMetricPoint> {
    bounded_desktop_points(
        points,
        Some(AxisBounds::new(Some(start), Some(end))),
        budget,
    )
}

fn bounded_desktop_points(
    points: Vec<AlignedMetricPoint>,
    bounds: Option<AxisBounds>,
    max_points: usize,
) -> Vec<AlignedMetricPoint> {
    let Some(bounds) = bounds else {
        return enforce_point_bound(points, max_points);
    };
    let mut left = None;
    let mut right = None;
    let mut inside = Vec::new();
    for point in points {
        if bounds.start.is_some_and(|start| point.axis_value < start) {
            left = Some(point);
        } else if bounds.end.is_some_and(|end| point.axis_value >= end) {
            right.get_or_insert(point);
        } else {
            inside.push(point);
        }
    }
    let neighbor_count = usize::from(left.is_some()) + usize::from(right.is_some());
    let inside_limit = max_points.saturating_sub(neighbor_count);
    if inside_limit < 2 {
        inside.truncate(inside_limit);
    } else {
        inside = enforce_point_bound(inside, inside_limit);
    }
    left.into_iter().chain(inside).chain(right).collect()
}

fn enforce_point_bound<T: Clone>(samples: Vec<T>, max_points: usize) -> Vec<T> {
    if samples.len() <= max_points {
        return samples;
    }
    let last = samples.len() - 1;
    (0..max_points)
        .map(|index| samples[index * last / (max_points - 1)].clone())
        .collect()
}

fn qualify_series(
    alignment_reasons: Vec<AlignmentReason>,
    diagnostics: Option<SeriesDiagnostics>,
    run_status: RunStatus,
) -> (EvidenceCompleteness, Vec<EvidenceReason>) {
    let mut reasons = alignment_reasons
        .into_iter()
        .map(|reason| match reason {
            AlignmentReason::MissingRunStart => EvidenceReason::MissingRunStart,
            AlignmentReason::NegativeAxis => EvidenceReason::NegativeAxis,
            AlignmentReason::DecreasingAxis => EvidenceReason::DecreasingAxis,
        })
        .collect::<Vec<_>>();
    let missing_metric = diagnostics.is_some_and(|value| value.effective_count == 0);
    match diagnostics {
        Some(value) => {
            if value.has_negative_step {
                reasons.push(EvidenceReason::NegativeAxis);
            }
            if value.has_decreasing_timestamp {
                reasons.push(EvidenceReason::DecreasingAxis);
            }
            if value.has_non_finite_value {
                reasons.push(EvidenceReason::NonFiniteValue);
            }
            if missing_metric && reasons.is_empty() {
                reasons.push(EvidenceReason::MissingMetric);
            }
        }
        None => reasons.push(EvidenceReason::DiagnosticsUnavailable),
    }
    let invalid = reasons.iter().any(|reason| {
        matches!(
            reason,
            EvidenceReason::NegativeAxis
                | EvidenceReason::DecreasingAxis
                | EvidenceReason::NonFiniteValue
        )
    });
    let missing_run_start = reasons.contains(&EvidenceReason::MissingRunStart);
    let mut completeness = if invalid {
        EvidenceCompleteness::Invalid
    } else if missing_metric || missing_run_start {
        EvidenceCompleteness::Unavailable
    } else {
        EvidenceCompleteness::Complete
    };
    if reasons.contains(&EvidenceReason::DiagnosticsUnavailable) {
        completeness = completeness.max(EvidenceCompleteness::Partial);
    }
    let lifecycle_reason = match run_status {
        RunStatus::Running => Some(EvidenceReason::RunRunning),
        RunStatus::Failed => Some(EvidenceReason::RunFailed),
        RunStatus::Finished => None,
    };
    if let Some(reason) = lifecycle_reason {
        completeness = completeness.max(EvidenceCompleteness::Partial);
        reasons.push(reason);
    }
    (completeness, reasons)
}

/// Horizontal coordinate requested for a metric series.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricAxis {
    Step,
    RelativeTime,
    Timestamp,
}

/// Milliseconds relative to a Run's start time.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RelativeTime(i64);

impl RelativeTime {
    pub const fn from_millis(value: i64) -> Self {
        Self(value)
    }

    pub const fn as_millis(self) -> i64 {
        self.0
    }
}

/// Milliseconds from the Unix epoch in UTC.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(i64);

impl Timestamp {
    pub const fn from_millis(value: i64) -> Self {
        Self(value)
    }

    pub const fn as_millis(self) -> i64 {
        self.0
    }
}

/// A full axis or a typed half-open metric range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetricRange {
    All(MetricAxis),
    Steps {
        start: Step,
        end: Step,
    },
    StepsFrom {
        start: Step,
    },
    StepsUntil {
        end: Step,
    },
    RelativeTime {
        start: RelativeTime,
        end: RelativeTime,
    },
    RelativeTimeFrom {
        start: RelativeTime,
    },
    RelativeTimeUntil {
        end: RelativeTime,
    },
    Timestamps {
        start: Timestamp,
        end: Timestamp,
    },
    TimestampsFrom {
        start: Timestamp,
    },
    TimestampsUntil {
        end: Timestamp,
    },
}

impl MetricRange {
    pub const fn axis(&self) -> MetricAxis {
        match self {
            Self::All(axis) => *axis,
            Self::Steps { .. } | Self::StepsFrom { .. } | Self::StepsUntil { .. } => {
                MetricAxis::Step
            }
            Self::RelativeTime { .. }
            | Self::RelativeTimeFrom { .. }
            | Self::RelativeTimeUntil { .. } => MetricAxis::RelativeTime,
            Self::Timestamps { .. }
            | Self::TimestampsFrom { .. }
            | Self::TimestampsUntil { .. } => MetricAxis::Timestamp,
        }
    }

    const fn is_empty(&self) -> bool {
        match self {
            Self::All(_) => false,
            Self::Steps { start, end } => start.value() >= end.value(),
            Self::StepsUntil { end } => end.value() == i64::MIN,
            Self::RelativeTime { start, end } => start.as_millis() >= end.as_millis(),
            Self::RelativeTimeUntil { end } => end.as_millis() == i64::MIN,
            Self::Timestamps { start, end } => start.as_millis() >= end.as_millis(),
            Self::TimestampsUntil { end } => end.as_millis() == i64::MIN,
            Self::StepsFrom { .. }
            | Self::RelativeTimeFrom { .. }
            | Self::TimestampsFrom { .. } => false,
        }
    }
}

/// Validated options for one public metric-series query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricQuery {
    range: MetricRange,
    max_points: usize,
}

impl MetricQuery {
    /// Creates a typed query without deriving limits from display geometry.
    ///
    /// # Errors
    ///
    /// Returns [`MetricQueryError`] for an empty range, unsupported axis, or
    /// point limit outside the chart contract.
    pub const fn new(range: MetricRange, max_points: usize) -> Result<Self, MetricQueryError> {
        if range.is_empty() {
            return Err(MetricQueryError::EmptyRange);
        }
        let axis = range.axis();
        if !matches!(axis, MetricAxis::Step) {
            return Err(MetricQueryError::UnsupportedAxis { axis });
        }
        if max_points < 2 {
            return Err(MetricQueryError::MaxPointsTooSmall { max_points });
        }
        if max_points > MAX_CHART_POINTS {
            return Err(MetricQueryError::MaxPointsTooLarge { max_points });
        }
        Ok(Self { range, max_points })
    }

    pub const fn range(&self) -> &MetricRange {
        &self.range
    }

    pub const fn axis(&self) -> MetricAxis {
        self.range.axis()
    }

    pub const fn max_points(&self) -> usize {
        self.max_points
    }
}

/// Invalid public Reader query options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricQueryError {
    EmptyRange,
    UnsupportedAxis { axis: MetricAxis },
    MaxPointsTooSmall { max_points: usize },
    MaxPointsTooLarge { max_points: usize },
}

impl fmt::Display for MetricQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRange => formatter.write_str("metric range must be non-empty"),
            Self::UnsupportedAxis { axis } => {
                write!(formatter, "metric axis {axis:?} is not supported; use Step")
            }
            Self::MaxPointsTooSmall { max_points } => {
                write!(formatter, "max_points must be at least 2, got {max_points}")
            }
            Self::MaxPointsTooLarge { max_points } => {
                write!(
                    formatter,
                    "max_points must be at most {MAX_CHART_POINTS}, got {max_points}"
                )
            }
        }
    }
}

impl std::error::Error for MetricQueryError {}

/// Typed horizontal coordinate retained with one real metric sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricCoordinate {
    Step(Step),
    RelativeTime(RelativeTime),
    Timestamp(Timestamp),
}

impl MetricCoordinate {
    pub const fn axis(self) -> MetricAxis {
        match self {
            Self::Step(_) => MetricAxis::Step,
            Self::RelativeTime(_) => MetricAxis::RelativeTime,
            Self::Timestamp(_) => MetricAxis::Timestamp,
        }
    }
}

/// One persisted real sample and its selected horizontal coordinate.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricSample {
    pub point: MetricPoint,
    pub coordinate: MetricCoordinate,
}

/// One qualified, optionally reduced metric series returned by Reader.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricSeries {
    axis: MetricAxis,
    samples: Vec<MetricSample>,
    source_count: u64,
    completeness: EvidenceCompleteness,
    reasons: Vec<EvidenceReason>,
}

impl MetricSeries {
    /// Builds a series from real samples while checking its metadata invariants.
    ///
    /// # Errors
    ///
    /// Returns [`MetricSeriesError`] when a sample uses a different axis or the
    /// source count is smaller than the returned sample count.
    pub fn from_samples(
        axis: MetricAxis,
        samples: Vec<MetricSample>,
        source_count: u64,
        completeness: EvidenceCompleteness,
        reasons: Vec<EvidenceReason>,
    ) -> Result<Self, MetricSeriesError> {
        if samples
            .iter()
            .any(|sample| sample.coordinate.axis() != axis)
        {
            return Err(MetricSeriesError::MixedAxes);
        }
        if source_count < samples.len() as u64 {
            return Err(MetricSeriesError::InvalidSourceCount);
        }
        Ok(Self {
            axis,
            samples,
            source_count,
            completeness,
            reasons,
        })
    }

    pub const fn axis(&self) -> MetricAxis {
        self.axis
    }

    pub fn samples(&self) -> &[MetricSample] {
        &self.samples
    }

    pub const fn source_count(&self) -> u64 {
        self.source_count
    }

    pub fn downsampled(&self) -> bool {
        self.source_count > self.samples.len() as u64
    }

    pub const fn completeness(&self) -> EvidenceCompleteness {
        self.completeness
    }

    pub fn reasons(&self) -> &[EvidenceReason] {
        &self.reasons
    }
}

/// Invalid metadata supplied while constructing a metric series.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricSeriesError {
    MixedAxes,
    InvalidSourceCount,
}

impl fmt::Display for MetricSeriesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MixedAxes => formatter.write_str("MetricSeries samples must use one axis"),
            Self::InvalidSourceCount => {
                formatter.write_str("MetricSeries source_count is smaller than its samples")
            }
        }
    }
}

impl std::error::Error for MetricSeriesError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_pages_reject_unbounded_limits() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config = NativeStorageConfig::with_backend_and_s3_config(
            crate::storage::bootstrap::CatalogBackend::Sqlite,
            root.path(),
            None,
            None,
            None,
        );
        drop(crate::storage::bootstrap::open_native_connection_with_config(config)?);
        let reader = Reader::builder(root.path())
            .catalog_backend(CatalogBackend::Sqlite)
            .open()?;
        let project = ProjectId::from_string("project");
        assert!(matches!(
            reader.runs_page(&project, None, 0),
            Err(Error::UnsupportedQuery)
        ));
        assert!(matches!(
            reader.runs_page(&project, None, MAX_RUN_PAGE_SIZE + 1),
            Err(Error::UnsupportedQuery)
        ));
        Ok(())
    }

    #[test]
    fn registered_fresh_handle_interrupts_executing_sql_and_next_attempt_succeeds()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir()?;
        let client = crate::Client::builder(root.path()).open()?;
        let run = client.start_run(crate::RunOptions::new("project").id("run"))?;
        run.finish()?;
        client.shutdown()?;
        let reader = Reader::builder(root.path()).open()?;
        let (registered_tx, registered_rx) = mpsc::channel();
        let (observer_tx, observer_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let query = std::thread::spawn(move || {
            reader.with_fresh_query_reader_registered(&mut |handle| {
                registered_tx.send(handle).unwrap();
                true
            }, &mut |_| {}, |fresh: &Reader| {
                let connection = fresh.native()?;
                connection.execute_batch("CREATE SEQUENCE memory.main.seex_interrupt_progress;")
                    .map_err(|_| Error::Storage)?;
                observer_tx.send(connection.try_clone().unwrap()).unwrap();
                let result = connection.query_row(
                    "SELECT sum(sin(i::DOUBLE) + nextval('memory.main.seex_interrupt_progress')::DOUBLE)
                     FROM range(1000000000) AS data(i)", [], |row| row.get::<_, f64>(0));
                let reused: i32 = connection.query_row("SELECT 7", [], |row| row.get(0)).unwrap();
                result_tx.send((result, reused)).unwrap();
                Ok(Some(())) // Do not retry the deliberately interrupted probe SQL.
            })
        });
        struct InterruptOnDrop(ReaderInterrupt);
        impl Drop for InterruptOnDrop {
            fn drop(&mut self) {
                self.0.interrupt();
            }
        }
        // Ensure a failed progress assertion also cancels the long probe query.
        let cleanup = InterruptOnDrop(registered_rx.recv_timeout(Duration::from_secs(10))?);
        let handle = &cleanup.0;
        let observer = observer_rx.recv_timeout(Duration::from_secs(10))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let progress: Option<i64> = observer.query_row(
                "SELECT last_value FROM duckdb_sequences() WHERE sequence_name = 'seex_interrupt_progress'",
                [], |row| row.get(0))?;
            if progress.is_some_and(|value| value > 0) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "query never advanced its sequence"
            );
            std::thread::yield_now();
        }
        handle.interrupt();
        let (result, reused) = result_rx.recv_timeout(Duration::from_secs(10))?;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .to_uppercase()
                .contains("INTERRUPT")
        );
        assert_eq!(reused, 7);
        assert_eq!(query.join().expect("query thread panicked")?, Some(()));
        // A dropped old connection's handle must not interrupt a later fresh one.
        let next = Reader::builder(root.path()).open()?;
        let value = next.with_fresh_query_reader_registered(
            &mut |_| true,
            &mut |_| {},
            |fresh: &Reader| {
                handle.interrupt();
                let value: i32 = fresh
                    .native()?
                    .query_row("SELECT 9", [], |row| row.get(0))
                    .map_err(|_| Error::Storage)?;
                Ok(Some(value))
            },
        )?;
        assert_eq!(value, Some(9));
        Ok(())
    }

    #[test]
    fn cancelled_retry_skips_open_and_already_cancelled_request_never_opens()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = crate::Client::builder(root.path()).open()?;
        let run = client.start_run(crate::RunOptions::new("probe").id("run"))?;
        run.finish()?;
        client.shutdown()?;
        let reader = Reader::builder(root.path()).open()?;
        let mut phases = Vec::new();
        let cancelled = Cell::new(false);
        let value = reader.with_fresh_query_reader_checked::<()>(
            &mut || !cancelled.get(),
            &mut |_| true,
            &mut |p| phases.push(p),
            |_| {
                cancelled.set(true);
                Err(Error::Storage)
            },
        )?;
        assert_eq!(value, None);
        assert_eq!(
            phases,
            [
                "fresh_open_start",
                "fresh_open_finish",
                "query_start",
                "fresh_open_skipped"
            ]
        );
        phases.clear();
        let value = reader.with_fresh_query_reader_checked::<()>(
            &mut || false,
            &mut |_| panic!("must not register"),
            &mut |p| phases.push(p),
            |_| panic!("must not execute"),
        )?;
        assert_eq!(value, None);
        assert_eq!(phases, ["fresh_open_skipped"]);
        let calls = Cell::new(0);
        phases.clear();
        let query = MetricQuery::new(MetricRange::All(MetricAxis::Step), 8)?;
        let value = reader.query_metrics_for_desktop_with_interrupt(
            &[run.run_id().clone()],
            &MetricKey::from_string("loss"),
            &query,
            false,
            &mut || {
                calls.set(calls.get() + 1);
                calls.get() == 2
            },
            &mut |_| panic!("cancelled before open must not register"),
            &mut |p| phases.push(p),
        )?;
        assert!(value.is_none());
        assert_eq!(phases, ["fresh_open_skipped"]);
        Ok(())
    }

    #[test]
    fn fresh_attempt_registers_before_sql_and_replaces_handle_on_retry()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = crate::Client::builder(root.path()).open()?;
        let run = client.start_run(crate::RunOptions::new("project").id("run"))?;
        run.finish()?;
        client.shutdown()?;
        let reader = Reader::builder(root.path()).open()?;
        let session = reader.interrupt_handle().unwrap();
        let handles = RefCell::new(Vec::<ReaderInterrupt>::new());
        let result = reader.with_fresh_query_reader_registered(
            &mut |handle| {
                assert!(!session.0.same_connection(&handle.0));
                assert!(
                    handles
                        .borrow()
                        .iter()
                        .all(|old| !old.0.same_connection(&handle.0))
                );
                handles.borrow_mut().push(handle);
                true
            },
            &mut |_| {},
            |fresh: &Reader| {
                let handle = fresh.interrupt_handle().unwrap();
                assert!(
                    handles
                        .borrow()
                        .last()
                        .unwrap()
                        .0
                        .same_connection(&handle.0)
                );
                if handles.borrow().len() == 1 {
                    return Err(Error::Storage);
                }
                let value = fresh
                    .native()?
                    .query_row("SELECT 7", [], |row| row.get::<_, i32>(0))
                    .map_err(|_| Error::Storage)?;
                Ok(Some(value))
            },
        )?;
        assert_eq!(result, Some(7));
        assert_eq!(handles.borrow().len(), 2);
        let rejected =
            reader.with_fresh_query_reader_registered::<()>(&mut |_| false, &mut |_| {}, |_| {
                panic!("cancelled registration must not execute SQL")
            })?;
        assert_eq!(rejected, None);
        let calls = Cell::new(0);
        let cancelled_retry = reader.with_fresh_query_reader_registered::<()>(
            &mut |_| {
                calls.set(calls.get() + 1);
                calls.get() == 1
            },
            &mut |_| {},
            |_| {
                assert_eq!(calls.get(), 1);
                Err(Error::Storage)
            },
        )?;
        assert_eq!(cancelled_retry, None);
        assert_eq!(calls.get(), 2);
        Ok(())
    }

    #[test]
    fn fresh_query_interrupt_handle_is_not_the_session_handle()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = crate::Client::builder(root.path()).open()?;
        let run = client.start_run(crate::RunOptions::new("project").id("run"))?;
        run.log_with([("loss", 1.)], crate::LogOptions::new().step(0))?;
        run.finish()?;
        client.shutdown()?;
        let reader = Reader::builder(root.path()).open()?;
        let session = reader
            .interrupt_handle()
            .expect("session connection exists");
        assert!(
            session
                .0
                .same_connection(&reader.interrupt_handle().unwrap().0)
        );
        let mut attempts = Vec::new();
        let result = reader.with_fresh_query_reader(|fresh| {
            let handle = fresh.interrupt_handle().expect("query connection exists");
            assert!(
                handle
                    .0
                    .same_connection(&fresh.interrupt_handle().unwrap().0)
            );
            // Characterize the gap: the worker's session handle is not this handle.
            assert!(!session.0.same_connection(&handle.0));
            assert!(
                attempts
                    .iter()
                    .all(|old: &ReaderInterrupt| !old.0.same_connection(&handle.0))
            );
            attempts.push(handle);
            if attempts.len() == 1 {
                Err(Error::Storage)
            } else {
                Ok(Some(()))
            }
        })?;
        assert_eq!(result, Some(()));
        assert_eq!(attempts.len(), 2);
        Ok(())
    }

    #[test]
    fn fresh_query_retries_the_whole_operation_and_publishes_only_success()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let client = crate::Client::builder(root.path()).open()?;
        for id in ["a", "b"] {
            let run = client.start_run(crate::RunOptions::new("panel").id(id))?;
            run.log_with([("loss", 1.0)], crate::LogOptions::new().step(0))?;
            run.finish()?;
        }
        let reader = Reader::builder(root.path()).open()?;
        let key = MetricKey::from_string("loss");
        let query = MetricQuery::new(MetricRange::All(MetricAxis::Step), 8)?;
        let mut calls = Vec::new();
        let mut attempts = 0;
        let result = reader
            .with_fresh_query_reader(|fresh| {
                attempts += 1;
                assert_eq!(
                    fresh.diagnostics_loads.get(),
                    0,
                    "failed-attempt cache must not escape"
                );
                let mut output = Vec::new();
                for id in ["a", "b"] {
                    calls.push(id);
                    if attempts == 1 && id == "b" {
                        return Err(Error::Storage);
                    }
                    output.push(fresh.query_metric_once(
                        &RunId::from_string(id),
                        &key,
                        &query,
                        true,
                        true,
                    )?);
                }
                Ok(Some(output))
            })?
            .unwrap();
        assert_eq!(calls, vec!["a", "b", "a", "b"]);
        assert_eq!(result.len(), 2);
        assert_eq!(reader.diagnostics_loads.get(), 2);
        let cached = reader.diagnostics.borrow().finished.len();
        let mut calls = 0;
        let error = reader.with_fresh_query_reader::<()>(|fresh| {
            calls += 1;
            fresh.diagnostics.borrow_mut().finished.clear();
            Err(Error::Storage)
        });
        assert_eq!(error, Err(Error::Storage));
        assert_eq!(calls, 2);
        assert_eq!(reader.diagnostics.borrow().finished.len(), cached);
        let cancelled = reader.with_fresh_query_reader::<()>(|fresh| {
            fresh.diagnostics.borrow_mut().finished.clear();
            Ok(None)
        })?;
        assert!(cancelled.is_none());
        assert_eq!(reader.diagnostics.borrow().finished.len(), cached);
        let mut calls = 0;
        assert_eq!(
            reader.with_fresh_query_reader::<()>(|_| {
                calls += 1;
                Err(Error::UnsupportedQuery)
            }),
            Err(Error::UnsupportedQuery)
        );
        assert_eq!(calls, 1);
        Ok(())
    }

    #[test]
    fn run_metadata_cache_does_not_grow_with_pages() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let config = NativeStorageConfig::with_backend_and_s3_config(
            crate::storage::bootstrap::CatalogBackend::Sqlite,
            root.path(),
            None,
            None,
            None,
        );
        drop(crate::storage::bootstrap::open_native_connection_with_config(config)?);
        let reader = Reader::builder(root.path())
            .catalog_backend(CatalogBackend::Sqlite)
            .open()?;
        let now = chrono::Utc::now();
        let runs = (0..MAX_RUN_METADATA_CACHE + 100)
            .map(|index| Run {
                run_id: RunId::from_string(format!("run-{index}")),
                project_id: ProjectId::from_string("project"),
                name: String::new(),
                status: RunStatus::Running,
                created_at: now,
                started_at: now,
                finished_at: None,
            })
            .collect::<Vec<_>>();
        reader.remember_runs(&runs);
        assert_eq!(reader.run_metadata.borrow().len(), MAX_RUN_METADATA_CACHE);
        Ok(())
    }

    fn sample(coordinate: MetricCoordinate) -> MetricSample {
        let timestamp = "2026-01-01T00:00:00Z"
            .parse()
            .expect("test timestamp should parse");
        MetricSample {
            point: MetricPoint {
                run_id: crate::model::run::RunId::from_string("run-1"),
                metric_key: crate::model::metric::MetricKey::from_string("loss"),
                step: Step::new(7),
                timestamp,
                value_f64: 0.5,
                ingested_at: timestamp,
            },
            coordinate,
        }
    }

    #[test]
    fn typed_ranges_select_their_axis_and_reject_empty_bounds() {
        let query = MetricQuery::new(
            MetricRange::Steps {
                start: Step::new(10),
                end: Step::new(20),
            },
            500,
        )
        .expect("typed range should be valid");

        assert_eq!(query.axis(), MetricAxis::Step);
        assert_eq!(query.max_points(), 500);
        assert_eq!(
            MetricQuery::new(
                MetricRange::Steps {
                    start: Step::new(4),
                    end: Step::new(4),
                },
                2,
            ),
            Err(MetricQueryError::EmptyRange)
        );
    }

    #[test]
    fn public_point_limit_is_strictly_validated_without_pixels() {
        assert_eq!(
            MetricQuery::new(MetricRange::All(MetricAxis::Step), 1),
            Err(MetricQueryError::MaxPointsTooSmall { max_points: 1 })
        );
        assert_eq!(
            MetricQuery::new(MetricRange::All(MetricAxis::Timestamp), 2),
            Err(MetricQueryError::UnsupportedAxis {
                axis: MetricAxis::Timestamp
            })
        );
        assert_eq!(
            MetricQuery::new(MetricRange::All(MetricAxis::Step), MAX_CHART_POINTS + 1),
            Err(MetricQueryError::MaxPointsTooLarge {
                max_points: MAX_CHART_POINTS + 1
            })
        );
        assert!(MetricQuery::new(MetricRange::All(MetricAxis::Step), 2).is_ok());
    }

    #[test]
    fn narrow_step_plan_requires_an_incomplete_cached_boundary() {
        let diagnostics = SeriesDiagnostics {
            effective_count: 10,
            min_step: Some(0),
            max_step: Some(9),
            has_negative_step: false,
            has_decreasing_timestamp: false,
            has_non_finite_value: false,
        };

        assert!(use_narrow_step_plan(
            MetricAxis::Step,
            Some(AxisBounds::new(Some(2), Some(8))),
            Some(diagnostics),
            false
        ));
        assert!(!use_narrow_step_plan(
            MetricAxis::Step,
            Some(AxisBounds::new(Some(0), Some(10))),
            Some(diagnostics),
            false
        ));
        assert!(!use_narrow_step_plan(
            MetricAxis::Step,
            Some(AxisBounds::new(Some(2), Some(8))),
            Some(diagnostics),
            true
        ));
    }

    #[test]
    fn bounded_full_plan_requires_complete_valid_diagnostics() {
        let diagnostics = SeriesDiagnostics {
            effective_count: 10,
            min_step: Some(0),
            max_step: Some(9),
            has_negative_step: false,
            has_decreasing_timestamp: false,
            has_non_finite_value: false,
        };

        assert!(use_bounded_full_plan(
            MetricAxis::Step,
            Some(AxisBounds::new(Some(0), Some(10))),
            Some(diagnostics),
            false
        ));
        assert!(!use_bounded_full_plan(
            MetricAxis::Step,
            Some(AxisBounds::new(Some(2), Some(8))),
            Some(diagnostics),
            true
        ));
        assert!(!use_bounded_full_plan(
            MetricAxis::RelativeTime,
            Some(AxisBounds::new(Some(0), Some(10))),
            Some(diagnostics),
            false
        ));
        let invalid = SeriesDiagnostics {
            has_decreasing_timestamp: true,
            ..diagnostics
        };
        assert!(!bounded_reduction_is_safe(
            MetricAxis::RelativeTime,
            invalid
        ));
        assert!(bounded_reduction_is_safe(MetricAxis::Step, invalid));
        assert!(!bounded_reduction_is_safe(
            MetricAxis::Step,
            SeriesDiagnostics {
                has_negative_step: true,
                ..invalid
            }
        ));
        assert!(!bounded_reduction_is_safe(
            MetricAxis::Step,
            SeriesDiagnostics {
                has_non_finite_value: true,
                ..invalid
            }
        ));
        let (completeness, reasons) =
            qualify_series(Vec::new(), Some(invalid), RunStatus::Finished);
        assert_eq!(completeness, EvidenceCompleteness::Invalid);
        assert_eq!(reasons, vec![EvidenceReason::DecreasingAxis]);
    }

    #[test]
    fn metric_series_retains_real_samples_and_qualified_metadata() -> Result<(), MetricSeriesError>
    {
        let series = MetricSeries::from_samples(
            MetricAxis::Step,
            vec![sample(MetricCoordinate::Step(Step::new(7)))],
            10,
            EvidenceCompleteness::Partial,
            vec![EvidenceReason::RunRunning],
        )?;

        assert_eq!(series.samples()[0].point.step, Step::new(7));
        assert_eq!(series.source_count(), 10);
        assert!(series.downsampled());
        assert_eq!(series.completeness(), EvidenceCompleteness::Partial);
        assert_eq!(series.reasons(), [EvidenceReason::RunRunning]);
        Ok(())
    }

    #[test]
    fn metric_series_rejects_mixed_axes_and_impossible_source_counts() {
        let relative = sample(MetricCoordinate::RelativeTime(RelativeTime::from_millis(7)));
        assert_eq!(
            MetricSeries::from_samples(
                MetricAxis::Step,
                vec![relative],
                1,
                EvidenceCompleteness::Complete,
                Vec::new(),
            ),
            Err(MetricSeriesError::MixedAxes)
        );
        assert_eq!(
            MetricSeries::from_samples(
                MetricAxis::Step,
                vec![sample(MetricCoordinate::Step(Step::new(7)))],
                0,
                EvidenceCompleteness::Complete,
                Vec::new(),
            ),
            Err(MetricSeriesError::InvalidSourceCount)
        );
    }

    #[test]
    fn reader_opens_without_a_writer_and_discovers_catalog_resources()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::model::run::RunId;
        use crate::storage::MetricWrite;
        use crate::storage::bootstrap::{
            CatalogBackend, NativeStorageConfig, open_native_connection_with_config,
        };

        let root = tempfile::tempdir()?;
        let connection = ProjectConnection::new(open_native_connection_with_config(
            NativeStorageConfig::with_backend_and_s3_config(
                CatalogBackend::Sqlite,
                root.path(),
                None,
                None,
                None,
            ),
        )?);
        let project_id = ProjectId::from_string("project-1");
        let created_at = crate::storage::time::current_timestamp("created_at")?;
        let project = Project {
            project_id: project_id.clone(),
            name: String::from("reader"),
            created_at,
        };
        connection.create_project(&project)?;
        let run =
            connection.create_run(&project.project_id, "baseline", RunId::from_string("run-1"))?;
        let started_at = run.started_at.timestamp_millis();
        let mut rows = (0..5)
            .map(|step| MetricWrite {
                run_id: run.run_id.as_str().to_owned(),
                metric_key: String::from("loss"),
                step,
                timestamp_millis: started_at + step,
                value_f64: step as f64,
                ingested_at_millis: started_at + step,
            })
            .collect::<Vec<_>>();
        rows.extend([
            MetricWrite {
                run_id: run.run_id.as_str().to_owned(),
                metric_key: String::from("loss"),
                step: -2,
                timestamp_millis: started_at - 100,
                value_f64: 1.0,
                ingested_at_millis: started_at - 2,
            },
            MetricWrite {
                run_id: run.run_id.as_str().to_owned(),
                metric_key: String::from("loss"),
                step: -1,
                timestamp_millis: started_at - 200,
                value_f64: f64::NAN,
                ingested_at_millis: started_at - 1,
            },
        ]);
        connection.append_metric_batch(&rows)?;
        connection.rebuild_metric_aggregates_for_run(&run.run_id)?;
        connection.mark_run_terminal(&run.run_id, RunStatus::Finished, created_at)?;
        connection.flush_metric_points()?;
        drop(connection);
        std::fs::write(
            root.path().join(".seex/config.toml"),
            "schema_version = 1\ncatalog_backend = 'sqlite'\ncatalog_path = '.seex/catalog.sqlite'\n\
             data_path = '.seex/data'\n",
        )?;

        let reader = Reader::builder(root.path()).open()?;
        let projects = reader.projects()?;
        let runs = reader.runs(&project_id)?;
        let metrics = reader.metrics(&runs[0])?;
        let series = reader.query_metric(
            &runs[0].run_id,
            &MetricKey::from_string("loss"),
            &MetricQuery::new(
                MetricRange::Steps {
                    start: Step::new(1),
                    end: Step::new(4),
                },
                2,
            )?,
        )?;
        let desktop_series = reader.query_metric_for_desktop(
            &runs[0].run_id,
            &MetricKey::from_string("loss"),
            &MetricQuery::new(
                MetricRange::Steps {
                    start: Step::new(1),
                    end: Step::new(4),
                },
                2,
            )?,
        )?;

        assert_eq!(projects, [project]);
        assert_eq!(reader.project(&project_id)?, projects.first().cloned());
        assert_eq!(runs[0].run_id.as_str(), "run-1");
        assert_eq!(metrics[0].metric_key.as_str(), "loss");
        assert_eq!(series.source_count(), 3);
        assert_eq!(series.samples().len(), 2);
        assert!(series.samples().iter().all(|sample| {
            let step = sample.point.step.value();
            (1..4).contains(&step)
        }));
        assert_eq!(series.completeness(), EvidenceCompleteness::Invalid);
        assert_eq!(
            series.reasons(),
            [
                EvidenceReason::NegativeAxis,
                EvidenceReason::DecreasingAxis,
                EvidenceReason::NonFiniteValue,
            ]
        );
        assert_eq!(desktop_series.samples().len(), 2);
        assert_eq!(desktop_series.samples()[0].point.step, Step::new(0));
        assert_eq!(desktop_series.samples()[1].point.step, Step::new(4));
        assert_eq!(reader.diagnostics_loads.get(), 1);
        reader.refresh_diagnostics(1);
        let cache_query = MetricQuery::new(
            MetricRange::Steps {
                start: Step::new(1),
                end: Step::new(4),
            },
            2,
        )?;
        reader.query_metric_for_desktop(
            &run.run_id,
            &MetricKey::from_string("loss"),
            &cache_query,
        )?;
        assert_eq!(reader.diagnostics_loads.get(), 1);

        reader
            .run_metadata
            .borrow_mut()
            .get_mut(&run.run_id)
            .expect("test Run metadata should be cached")
            .status = RunStatus::Running;
        // A stale caller-side status cannot change the fresh query snapshot.
        reader.refresh_diagnostics(2);
        for _ in 0..2 {
            reader.query_metric_for_desktop(
                &run.run_id,
                &MetricKey::from_string("loss"),
                &cache_query,
            )?;
        }
        assert_eq!(reader.diagnostics_loads.get(), 1);
        reader.refresh_diagnostics(3);
        reader.query_metric_for_desktop(
            &run.run_id,
            &MetricKey::from_string("loss"),
            &cache_query,
        )?;
        assert_eq!(reader.diagnostics_loads.get(), 1);
        Ok(())
    }

    #[test]
    fn diagnostic_failure_preserves_points_with_explicit_partial_evidence()
    -> Result<(), MetricSeriesError> {
        let sample = sample(MetricCoordinate::Step(Step::new(7)));
        let (completeness, reasons) = qualify_series(Vec::new(), None, RunStatus::Finished);
        let series =
            MetricSeries::from_samples(MetricAxis::Step, vec![sample], 1, completeness, reasons)?;

        assert_eq!(series.samples().len(), 1);
        assert_eq!(series.completeness(), EvidenceCompleteness::Partial);
        assert_eq!(series.reasons(), [EvidenceReason::DiagnosticsUnavailable]);
        Ok(())
    }
}
