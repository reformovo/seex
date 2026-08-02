//! Public read-query contracts.

#[cfg(test)]
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::{CatalogBackend, S3Options};
use crate::engine::comparison::compare_evidence;
use crate::engine::query::NativeQueryStore;
use crate::engine::ranking::rank_run_evidence;
use crate::error::{Error, Result as SdkResult};
use crate::model::alignment::{
    AlignedMetricPoint, AlignmentAxis, AlignmentQuery, AlignmentReason, AlignmentReduction,
    AlignmentViewport,
};
use crate::model::comparison::{
    ComparisonResult, EvidenceCompleteness, EvidenceReason, ObjectiveEvidence, ObjectiveMetric,
    RankingResult,
};
use crate::model::metric::{
    MetricAggregate, MetricKey, MetricPoint, MetricQuery as StorageMetricQuery, ReductionPolicy,
    Step,
};
use crate::model::run::{Run, RunId, RunStatus};
use crate::model::types::{Project, ProjectId};
use crate::storage::bootstrap::{NativeStorageConfig, open_existing_native_connection_with_config};
use crate::storage::config::{S3ConnectionOverrides, resolve_init_config};
use crate::storage::{
    ParquetSource, ProjectConnection, ProjectMetricReader, SeriesDiagnostics,
    StandaloneMetricReader, StorageError,
};

/// Builder for opening one existing native or standalone store read-only.
pub struct ReaderBuilder {
    source: ReaderSource,
    catalog_backend: Option<CatalogBackend>,
    catalog_path: Option<PathBuf>,
    data_path: Option<PathBuf>,
    s3: S3Options,
}

enum ReaderSource {
    Native(PathBuf),
    Parquet(String),
}

impl ReaderBuilder {
    pub fn new(root_path: impl Into<PathBuf>) -> Self {
        Self {
            source: ReaderSource::Native(root_path.into()),
            catalog_backend: None,
            catalog_path: None,
            data_path: None,
            s3: S3Options::default(),
        }
    }

    /// Selects a standalone Parquet file, glob, or object-store URI.
    pub fn parquet(source: impl Into<String>) -> Self {
        Self {
            source: ReaderSource::Parquet(source.into()),
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
        let root_path = match self.source {
            ReaderSource::Native(root_path) => root_path,
            ReaderSource::Parquet(source) => {
                let standalone = StandaloneMetricReader::open(
                    ParquetSource::new(source).map_err(|_| Error::Storage)?,
                )
                .map_err(|_| Error::Storage)?;
                return Ok(Reader {
                    connection: None,
                    standalone: Some(standalone),
                    run_metadata: RefCell::new(HashMap::new()),
                    diagnostics: RefCell::new(DiagnosticsCache::default()),
                    #[cfg(test)]
                    diagnostics_loads: Cell::new(0),
                });
            }
        };
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
        let connection =
            open_existing_native_connection_with_config(config).map_err(public_storage_error)?;
        Ok(Reader {
            connection: Some(ProjectConnection::new(connection)),
            standalone: None,
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
    standalone: Option<StandaloneMetricReader>,
    run_metadata: RefCell<HashMap<RunId, RunMetadata>>,
    diagnostics: RefCell<DiagnosticsCache>,
    #[cfg(test)]
    diagnostics_loads: Cell<usize>,
}

#[derive(Clone)]
struct RunMetadata {
    project_id: Option<ProjectId>,
    started_at_millis: i64,
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

#[derive(Default)]
struct DiagnosticsCache {
    generation: Option<u64>,
    finished: HashMap<DiagnosticsKey, SeriesDiagnostics>,
    volatile: HashMap<DiagnosticsKey, SeriesDiagnostics>,
}

impl Reader {
    pub fn builder(root_path: impl AsRef<Path>) -> ReaderBuilder {
        ReaderBuilder::new(root_path.as_ref().to_owned())
    }

    /// Builds a Reader over a standalone Parquet source.
    pub fn parquet(source: impl Into<String>) -> ReaderBuilder {
        ReaderBuilder::parquet(source)
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

    /// Lists Runs for one Project.
    pub fn runs(&self, project_id: &ProjectId) -> SdkResult<Vec<Run>> {
        let runs = self
            .native()?
            .list_runs(project_id, None, None, 0)
            .map_err(|_| Error::Storage)?;
        self.remember_runs(&runs);
        Ok(runs)
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
        ProjectMetricReader::new(self.native()?)
            .list_metrics(&run.run_id, run.status)
            .map_err(|_| Error::Storage)
    }

    /// Gets one persisted Metric summary when the Run contains that Metric.
    pub fn metric_summary(
        &self,
        run: &Run,
        metric_key: &MetricKey,
    ) -> SdkResult<Option<MetricAggregate>> {
        self.remember_runs(std::slice::from_ref(run));
        ProjectMetricReader::new(self.native()?)
            .query_metric_summaries(std::slice::from_ref(&run.run_id), metric_key)
            .map(|summaries| summaries.into_iter().next())
            .map_err(|_| Error::Storage)
    }

    /// Compares two Runs using their last effective objective values.
    pub fn compare_runs(
        &self,
        candidate_run_id: &RunId,
        reference_run_id: &RunId,
        objective: &ObjectiveMetric,
    ) -> SdkResult<ComparisonResult> {
        if candidate_run_id == reference_run_id {
            return Err(Error::DuplicateRunIdentity {
                run_id: candidate_run_id.as_str().to_owned(),
            });
        }
        let mut evidence = self
            .ranking_evidence(
                &[candidate_run_id.clone(), reference_run_id.clone()],
                objective,
            )?
            .into_iter();
        let candidate = evidence.next().ok_or(Error::Storage)?.1;
        let reference = evidence.next().ok_or(Error::Storage)?.1;
        Ok(compare_evidence(objective, candidate, reference))
    }

    /// Ranks Runs by one objective while retaining incomplete evidence.
    pub fn rank_runs(
        &self,
        run_ids: &[RunId],
        objective: &ObjectiveMetric,
    ) -> SdkResult<RankingResult> {
        let mut seen = HashSet::with_capacity(run_ids.len());
        for run_id in run_ids {
            if !seen.insert(run_id) {
                return Err(Error::DuplicateRunIdentity {
                    run_id: run_id.as_str().to_owned(),
                });
            }
        }
        Ok(rank_run_evidence(
            objective,
            self.ranking_evidence(run_ids, objective)?,
        ))
    }

    fn ranking_evidence(
        &self,
        run_ids: &[RunId],
        objective: &ObjectiveMetric,
    ) -> SdkResult<Vec<(Run, ObjectiveEvidence)>> {
        let connection = self.native()?;
        let runs = connection.get_runs(run_ids).map_err(|error| match error {
            StorageError::RunNotFound { run_id } => Error::RunNotFound { run_id },
            _ => Error::Storage,
        })?;
        self.remember_runs(&runs);
        let evidence = NativeQueryStore::new(connection)
            .objective_evidence_for_runs(&runs, objective)
            .map_err(Error::from)?;
        Ok(runs.into_iter().zip(evidence).collect())
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
    pub fn interrupt_handle(&self) -> Option<crate::storage::ReadInterrupt> {
        self.connection
            .as_ref()
            .map(ProjectConnection::interrupt_handle)
    }

    /// Desktop-only query retaining one real sample outside each range edge.
    ///
    /// The stable [`Reader::query_metric`] contract never returns these
    /// neighbors. `max_points` bounds in-range samples; this adapter may return
    /// at most two additional samples.
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

    fn query_metric_impl(
        &self,
        run_id: &RunId,
        metric_key: &MetricKey,
        query: &MetricQuery,
        retain_neighbors: bool,
        force_full_step: bool,
    ) -> SdkResult<MetricSeries> {
        let metadata = self.metadata(run_id)?;
        let (run_start, run_status) = (metadata.started_at_millis, metadata.status);
        let diagnostics = self.series_diagnostics(run_id, metric_key, &metadata);
        let standalone = self.standalone.is_some();
        let (axis, storage_axis, bounds) = match query.range() {
            MetricRange::All(axis) => (
                *axis,
                match axis {
                    MetricAxis::Step => AlignmentAxis::Step,
                    MetricAxis::RelativeTime | MetricAxis::Timestamp => AlignmentAxis::ElapsedTime,
                },
                None,
            ),
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
            MetricRange::RelativeTime { start, end } => (
                MetricAxis::RelativeTime,
                AlignmentAxis::ElapsedTime,
                Some(AxisBounds::new(
                    Some(start.as_millis()),
                    Some(end.as_millis()),
                )),
            ),
            MetricRange::RelativeTimeFrom { start } => (
                MetricAxis::RelativeTime,
                AlignmentAxis::ElapsedTime,
                Some(AxisBounds::new(Some(start.as_millis()), None)),
            ),
            MetricRange::RelativeTimeUntil { end } => (
                MetricAxis::RelativeTime,
                AlignmentAxis::ElapsedTime,
                Some(AxisBounds::new(None, Some(end.as_millis()))),
            ),
            MetricRange::Timestamps { start, end } => (
                MetricAxis::Timestamp,
                AlignmentAxis::ElapsedTime,
                Some(if standalone {
                    AxisBounds::new(Some(start.as_millis()), Some(end.as_millis()))
                } else {
                    AxisBounds::new(
                        Some(
                            start
                                .as_millis()
                                .checked_sub(run_start)
                                .ok_or(Error::UnsupportedQuery)?,
                        ),
                        Some(
                            end.as_millis()
                                .checked_sub(run_start)
                                .ok_or(Error::UnsupportedQuery)?,
                        ),
                    )
                }),
            ),
            MetricRange::TimestampsFrom { start } => (
                MetricAxis::Timestamp,
                AlignmentAxis::ElapsedTime,
                Some(AxisBounds::new(
                    Some(if standalone {
                        start.as_millis()
                    } else {
                        start
                            .as_millis()
                            .checked_sub(run_start)
                            .ok_or(Error::UnsupportedQuery)?
                    }),
                    None,
                )),
            ),
            MetricRange::TimestampsUntil { end } => (
                MetricAxis::Timestamp,
                AlignmentAxis::ElapsedTime,
                Some(AxisBounds::new(
                    None,
                    Some(if standalone {
                        end.as_millis()
                    } else {
                        end.as_millis()
                            .checked_sub(run_start)
                            .ok_or(Error::UnsupportedQuery)?
                    }),
                )),
            ),
        };
        if axis == MetricAxis::Step && !retain_neighbors {
            return self.query_step_metric(run_id, metric_key, query, &metadata, diagnostics);
        }
        let viewport = bounds.unwrap_or(AxisBounds::new(None, None)).viewport()?;
        let reduction = query
            .max_points()
            .map_or(Ok(AlignmentReduction::Full), |limit| {
                AlignmentReduction::screen_budget(u32::try_from(limit).unwrap_or(u32::MAX), 1)
            })
            .map_err(|_| Error::UnsupportedQuery)?;
        let storage_query = AlignmentQuery {
            run_id: run_id.clone(),
            metric_key: metric_key.clone(),
            axis: storage_axis,
            viewport,
            reduction,
        };
        let narrow_step = use_narrow_step_plan(axis, bounds, diagnostics, force_full_step);
        let result = match (&self.connection, &self.standalone, narrow_step) {
            (Some(connection), None, true) => {
                ProjectMetricReader::new(connection).query_narrow_step_metric(&storage_query)
            }
            (None, Some(reader), true) => reader.query_narrow_step_metric(&storage_query),
            (Some(connection), None, false) => {
                ProjectMetricReader::new(connection).query_aligned_metric(&storage_query)
            }
            (None, Some(reader), false) if axis == MetricAxis::Timestamp => {
                reader.query_timestamp_metric(&storage_query)
            }
            (None, Some(reader), false) => reader.query_aligned_metric(&storage_query),
            _ => return Err(Error::Storage),
        }
        .map_err(public_storage_error)?;
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
        if !retain_neighbors && let Some(max_points) = query.max_points() {
            samples = enforce_point_bound(samples, max_points);
        }
        let source_count = if retain_neighbors {
            result.source_row_count
        } else {
            result.source_row_count.saturating_sub(neighbor_count)
        };
        let (completeness, reasons) = qualify_series(result.reasons, diagnostics, run_status);
        MetricSeries::from_samples(axis, samples, source_count, completeness, reasons)
            .map_err(|_| Error::Storage)
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
        let reduction = query
            .max_points()
            .map_or(Ok(ReductionPolicy::Full), |limit| {
                ReductionPolicy::screen_budget(u32::try_from(limit).unwrap_or(u32::MAX), 1)
            })
            .map_err(|_| Error::UnsupportedQuery)?;
        let storage_query =
            StorageMetricQuery::new(run_id.clone(), metric_key.clone(), start, end, reduction)
                .map_err(|_| Error::UnsupportedQuery)?;
        let result = match (&self.connection, &self.standalone) {
            (Some(connection), None) => {
                ProjectMetricReader::new(connection).query_metric(&storage_query)
            }
            (None, Some(reader)) => reader.query_metric(&storage_query),
            _ => return Err(Error::Storage),
        }
        .map_err(public_storage_error)?;
        let mut samples = result
            .points
            .into_iter()
            .map(|point| MetricSample {
                coordinate: MetricCoordinate::Step(point.step),
                point,
            })
            .collect::<Vec<_>>();
        if let Some(max_points) = query.max_points() {
            samples = enforce_point_bound(samples, max_points);
        }
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
        let loaded = match (&self.connection, &self.standalone) {
            (Some(connection), None) => ProjectMetricReader::new(connection)
                .series_diagnostics(run_id, metric_key)
                .ok(),
            (None, Some(reader)) => reader.series_diagnostics(run_id, metric_key).ok(),
            _ => None,
        }?;
        let mut cache = self.diagnostics.borrow_mut();
        if finished {
            cache.finished.insert(key, loaded);
        } else {
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
            None => RunMetadata {
                project_id: None,
                started_at_millis: 0,
                status: RunStatus::Finished,
            },
        };
        self.run_metadata
            .borrow_mut()
            .insert(run_id.clone(), metadata.clone());
        Ok(metadata)
    }

    fn remember_runs(&self, runs: &[Run]) {
        self.run_metadata.borrow_mut().extend(
            runs.iter()
                .map(|run| (run.run_id.clone(), RunMetadata::from(run))),
        );
    }
}

impl From<&Run> for RunMetadata {
    fn from(run: &Run) -> Self {
        Self {
            project_id: Some(run.project_id.clone()),
            started_at_millis: run.started_at.timestamp_millis(),
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

fn bounded_desktop_points(
    points: Vec<AlignedMetricPoint>,
    bounds: Option<AxisBounds>,
    max_points: Option<usize>,
) -> Vec<AlignedMetricPoint> {
    let Some(bounds) = bounds else {
        return match max_points {
            Some(limit) => enforce_point_bound(points, limit),
            None => points,
        };
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
    if let Some(limit) = max_points {
        inside = enforce_point_bound(inside, limit);
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
    max_points: Option<usize>,
}

impl MetricQuery {
    /// Creates a typed query without deriving limits from display geometry.
    ///
    /// # Errors
    ///
    /// Returns [`MetricQueryError`] for an empty range or a point limit below
    /// two.
    pub const fn new(
        range: MetricRange,
        max_points: Option<usize>,
    ) -> Result<Self, MetricQueryError> {
        if range.is_empty() {
            return Err(MetricQueryError::EmptyRange);
        }
        if let Some(max_points) = max_points
            && max_points < 2
        {
            return Err(MetricQueryError::MaxPointsTooSmall { max_points });
        }
        Ok(Self { range, max_points })
    }

    pub const fn range(&self) -> &MetricRange {
        &self.range
    }

    pub const fn axis(&self) -> MetricAxis {
        self.range.axis()
    }

    pub const fn max_points(&self) -> Option<usize> {
        self.max_points
    }
}

/// Invalid public Reader query options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricQueryError {
    EmptyRange,
    MaxPointsTooSmall { max_points: usize },
}

impl fmt::Display for MetricQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRange => formatter.write_str("metric range must be non-empty"),
            Self::MaxPointsTooSmall { max_points } => {
                write!(formatter, "max_points must be at least 2, got {max_points}")
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
            MetricRange::RelativeTime {
                start: RelativeTime::from_millis(10),
                end: RelativeTime::from_millis(20),
            },
            Some(500),
        )
        .expect("typed range should be valid");

        assert_eq!(query.axis(), MetricAxis::RelativeTime);
        assert_eq!(query.max_points(), Some(500));
        assert_eq!(
            MetricQuery::new(
                MetricRange::Steps {
                    start: Step::new(4),
                    end: Step::new(4),
                },
                None,
            ),
            Err(MetricQueryError::EmptyRange)
        );
    }

    #[test]
    fn public_point_limit_is_strictly_validated_without_pixels() {
        assert_eq!(
            MetricQuery::new(MetricRange::All(MetricAxis::Timestamp), Some(1)),
            Err(MetricQueryError::MaxPointsTooSmall { max_points: 1 })
        );
        assert!(MetricQuery::new(MetricRange::All(MetricAxis::Step), Some(2)).is_ok());
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
            Some((2, 8)),
            Some(diagnostics),
            false
        ));
        assert!(!use_narrow_step_plan(
            MetricAxis::Step,
            Some((0, 10)),
            Some(diagnostics),
            false
        ));
        assert!(!use_narrow_step_plan(
            MetricAxis::Step,
            Some((2, 8)),
            Some(diagnostics),
            true
        ));
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
        use crate::storage::bootstrap::open_native_connection;

        let root = tempfile::tempdir()?;
        let connection = ProjectConnection::new(open_native_connection(root.path())?);
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
            "schema_version = 1\ncatalog_path = '.seex/catalog.ducklake'\n\
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
                Some(2),
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
                Some(2),
            )?,
        )?;
        let relative = reader.query_metric(
            &runs[0].run_id,
            &MetricKey::from_string("loss"),
            &MetricQuery::new(
                MetricRange::RelativeTime {
                    start: RelativeTime::from_millis(0),
                    end: RelativeTime::from_millis(60_000),
                },
                Some(3),
            )?,
        )?;
        let started_at = runs[0].started_at.timestamp_millis();
        let timestamps = reader.query_metric(
            &runs[0].run_id,
            &MetricKey::from_string("loss"),
            &MetricQuery::new(
                MetricRange::Timestamps {
                    start: Timestamp::from_millis(started_at),
                    end: Timestamp::from_millis(started_at + 60_000),
                },
                Some(3),
            )?,
        )?;
        let parquet = root
            .path()
            .join(".seex/data/main/metric_points/**/*.parquet")
            .to_string_lossy()
            .into_owned();
        let standalone = Reader::parquet(parquet).open()?;
        let standalone_steps = standalone.query_metric(
            &run.run_id,
            &MetricKey::from_string("loss"),
            &MetricQuery::new(
                MetricRange::Steps {
                    start: Step::new(1),
                    end: Step::new(4),
                },
                Some(2),
            )?,
        )?;
        let standalone_relative = standalone.query_metric(
            &run.run_id,
            &MetricKey::from_string("loss"),
            &MetricQuery::new(MetricRange::All(MetricAxis::RelativeTime), Some(3))?,
        )?;
        let standalone_timestamps = standalone.query_metric(
            &run.run_id,
            &MetricKey::from_string("loss"),
            &MetricQuery::new(
                MetricRange::Timestamps {
                    start: Timestamp::from_millis(started_at),
                    end: Timestamp::from_millis(started_at + 60_000),
                },
                Some(3),
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
        assert_eq!(desktop_series.samples().len(), 4);
        assert_eq!(desktop_series.samples()[0].point.step, Step::new(0));
        assert_eq!(desktop_series.samples()[3].point.step, Step::new(4));
        assert_eq!(relative.axis(), MetricAxis::RelativeTime);
        assert!(relative.samples().len() <= 3);
        assert!(relative.samples().iter().all(|sample| matches!(
            sample.coordinate,
            MetricCoordinate::RelativeTime(value) if (0..60_000).contains(&value.as_millis())
        )));
        assert_eq!(timestamps.axis(), MetricAxis::Timestamp);
        assert!(timestamps.samples().len() <= 3);
        assert!(timestamps.samples().iter().all(|sample| matches!(
            sample.coordinate,
            MetricCoordinate::Timestamp(value)
                if (started_at..started_at + 60_000).contains(&value.as_millis())
        )));
        assert_eq!(standalone.projects(), Err(Error::UnsupportedQuery));
        assert_eq!(standalone_steps, series);
        assert!(standalone_relative.samples().is_empty());
        assert!(
            standalone_relative
                .reasons()
                .contains(&EvidenceReason::MissingRunStart)
        );
        assert_eq!(
            standalone_relative.completeness(),
            EvidenceCompleteness::Invalid
        );
        assert_eq!(standalone_timestamps, timestamps);
        assert_eq!(reader.diagnostics_loads.get(), 1);
        reader.refresh_diagnostics(1);
        let cache_query = MetricQuery::new(
            MetricRange::Steps {
                start: Step::new(1),
                end: Step::new(4),
            },
            Some(2),
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
        reader.refresh_diagnostics(2);
        for _ in 0..2 {
            reader.query_metric_for_desktop(
                &run.run_id,
                &MetricKey::from_string("loss"),
                &cache_query,
            )?;
        }
        assert_eq!(reader.diagnostics_loads.get(), 2);
        reader.refresh_diagnostics(3);
        reader.query_metric_for_desktop(
            &run.run_id,
            &MetricKey::from_string("loss"),
            &cache_query,
        )?;
        assert_eq!(reader.diagnostics_loads.get(), 3);
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
