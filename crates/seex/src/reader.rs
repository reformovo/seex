//! Public read-query contracts.

use std::fmt;
use std::path::{Path, PathBuf};

use seex_model::comparison::{EvidenceCompleteness, EvidenceReason};
use seex_model::metric::{MetricAggregate, MetricPoint, Step};
use seex_model::run::Run;
use seex_model::types::{Project, ProjectId};
use seex_storage::bootstrap::{NativeStorageConfig, open_existing_native_connection_with_config};
use seex_storage::config::{S3ConnectionOverrides, resolve_init_config};
use seex_storage::{ProjectConnection, ProjectMetricReader};

use crate::error::{Error, Result as SdkResult};

/// Builder for opening one existing native project store read-only.
pub struct ReaderBuilder {
    root_path: PathBuf,
}

impl ReaderBuilder {
    pub fn new(root_path: impl Into<PathBuf>) -> Self {
        Self {
            root_path: root_path.into(),
        }
    }

    /// Opens the configured store without starting a writer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Configuration`] for invalid effective configuration or
    /// [`Error::Storage`] when the existing native store cannot be opened.
    pub fn open(self) -> SdkResult<Reader> {
        let resolved = resolve_init_config(
            &self.root_path,
            None,
            None,
            None,
            1,
            S3ConnectionOverrides::default(),
        )
        .map_err(|_| Error::Configuration)?;
        let config = NativeStorageConfig::with_backend_and_s3_config(
            resolved.catalog_backend,
            &self.root_path,
            resolved.catalog_path,
            resolved.data_path,
            resolved.s3_connection,
        );
        let connection =
            open_existing_native_connection_with_config(config).map_err(|_| Error::Storage)?;
        Ok(Reader {
            connection: ProjectConnection::new(connection),
        })
    }
}

/// Read-only discovery and metric-query entry point.
pub struct Reader {
    connection: ProjectConnection,
}

impl Reader {
    pub fn builder(root_path: impl AsRef<Path>) -> ReaderBuilder {
        ReaderBuilder::new(root_path.as_ref().to_owned())
    }

    /// Lists Projects in stable catalog order.
    pub fn projects(&self) -> SdkResult<Vec<Project>> {
        self.connection.list_projects().map_err(|_| Error::Storage)
    }

    /// Gets one Project when it exists.
    pub fn project(&self, project_id: &ProjectId) -> SdkResult<Option<Project>> {
        self.connection
            .get_project(project_id)
            .map_err(|_| Error::Storage)
    }

    /// Lists Runs for one Project.
    pub fn runs(&self, project_id: &ProjectId) -> SdkResult<Vec<Run>> {
        self.connection
            .list_runs(project_id, None, None, 0)
            .map_err(|_| Error::Storage)
    }

    /// Lists persisted Metric summaries for one Run.
    pub fn metrics(&self, run: &Run) -> SdkResult<Vec<MetricAggregate>> {
        ProjectMetricReader::new(&self.connection)
            .list_metrics(&run.run_id, run.status)
            .map_err(|_| Error::Storage)
    }
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
    RelativeTime {
        start: RelativeTime,
        end: RelativeTime,
    },
    Timestamps {
        start: Timestamp,
        end: Timestamp,
    },
}

impl MetricRange {
    pub const fn axis(&self) -> MetricAxis {
        match self {
            Self::All(axis) => *axis,
            Self::Steps { .. } => MetricAxis::Step,
            Self::RelativeTime { .. } => MetricAxis::RelativeTime,
            Self::Timestamps { .. } => MetricAxis::Timestamp,
        }
    }

    const fn is_empty(&self) -> bool {
        match self {
            Self::All(_) => false,
            Self::Steps { start, end } => start.value() >= end.value(),
            Self::RelativeTime { start, end } => start.as_millis() >= end.as_millis(),
            Self::Timestamps { start, end } => start.as_millis() >= end.as_millis(),
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
                run_id: seex_model::run::RunId::from_string("run-1"),
                metric_key: seex_model::metric::MetricKey::from_string("loss"),
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
        use seex_core::engine::client::NativeClient;
        use seex_model::run::RunId;

        let root = tempfile::tempdir()?;
        let client = NativeClient::open_with_storage_config(root.path(), None, None, 1024)?;
        let project_id = ProjectId::from_string("project-1");
        let project = client.create_project("reader", Some(project_id.clone()))?;
        let run = client.create_run(
            &project.project_id,
            "baseline",
            Some(RunId::from_string("run-1")),
        )?;
        client
            .run_handle(run.clone())
            .log_metric_at_step("loss", 0, 1.0)?;
        client.finish_run(&run.run_id)?;
        client.shutdown(None)?;
        std::fs::write(
            root.path().join(".seex/config.toml"),
            "schema_version = 1\ncatalog_path = '.seex/catalog.ducklake'\n\
             data_path = '.seex/data'\n",
        )?;

        let reader = Reader::builder(root.path()).open()?;
        let projects = reader.projects()?;
        let runs = reader.runs(&project_id)?;
        let metrics = reader.metrics(&runs[0])?;

        assert_eq!(projects, [project]);
        assert_eq!(reader.project(&project_id)?, projects.first().cloned());
        assert_eq!(runs[0].run_id.as_str(), "run-1");
        assert_eq!(metrics[0].metric_key.as_str(), "loss");
        Ok(())
    }
}
