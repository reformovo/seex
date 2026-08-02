import datetime
import os
from collections.abc import Mapping
from typing import Literal, Self

class SeexError(RuntimeError): ...
class MetricQueueFullError(SeexError): ...
class MetricWriterFailedError(SeexError): ...
class MetricDrainTimeoutError(SeexError): ...
class MetricFlushError(SeexError): ...
class MetricFlushTimeoutError(SeexError): ...
class RunClosedError(SeexError): ...
class ClientClosedError(SeexError): ...
class InvalidRunStateError(SeexError): ...
class RunAlreadyExistsError(SeexError): ...
class RunAlreadyActiveError(SeexError): ...
class InvalidConfigurationError(SeexError): ...
class StorageError(SeexError): ...
class ApiClosedError(SeexError): ...

class Settings:
    def __init__(
        self,
        *,
        catalog_backend: Literal["duckdb", "sqlite"] | None = None,
        catalog_path: str | os.PathLike[str] | None = None,
        data_path: str | os.PathLike[str] | None = None,
        metric_queue_capacity: int = 65536,
        s3_endpoint: str | None = None,
        s3_access_key_id: str | None = None,
        s3_secret_access_key: str | None = None,
        s3_session_token: str | None = None,
        s3_region: str | None = None,
        s3_path_style: bool | None = None,
        s3_use_ssl: bool | None = None,
    ) -> None: ...
    catalog_backend: Literal["duckdb", "sqlite"] | None
    catalog_path: str | os.PathLike[str] | None
    data_path: str | os.PathLike[str] | None
    metric_queue_capacity: int
    s3_endpoint: str | None
    s3_access_key_id: str | None
    s3_secret_access_key: str | None
    s3_session_token: str | None
    s3_region: str | None
    s3_path_style: bool | None
    s3_use_ssl: bool | None

class _Run:
    run_id: str
    project_id: str
    name: str
    status: Literal["running", "finished", "failed"]
    def log(
        self,
        data: Mapping[str, int | float],
        *,
        step: int | None = None,
        commit: bool | None = None,
    ) -> None: ...
    def finish(self, exit_code: int | None = None) -> None: ...
    def diagnostics(self) -> Diagnostics: ...
    def __enter__(self) -> Self: ...
    def __exit__(self, exc_type: object, exc_value: object, traceback: object) -> bool: ...

class MetricSeries:
    @property
    def axis(self) -> Literal["step", "relative_time", "timestamp"]: ...
    @property
    def points(self) -> list[MetricPoint]: ...
    @property
    def source_count(self) -> int: ...
    @property
    def downsampled(self) -> bool: ...
    @property
    def completeness(self) -> Literal["complete", "partial", "unavailable", "invalid"]: ...
    @property
    def reasons(self) -> list[str]: ...
    def __arrow_c_stream__(self, requested_schema: object | None = None) -> object: ...

class RunRecord:
    run_id: str
    project_id: str
    name: str
    status: Literal["running", "finished", "failed"]
    created_at: str
    started_at: str
    finished_at: str | None
    def metrics(self) -> list[MetricSummary]: ...
    def metric_summary(self, metric_key: str) -> MetricSummary | None: ...
    def history(
        self,
        metric_key: str,
        *,
        x_axis: Literal["step", "relative_time", "timestamp"] = "step",
        start: int | datetime.timedelta | datetime.datetime | None = None,
        end: int | datetime.timedelta | datetime.datetime | None = None,
        max_points: int | None = None,
    ) -> MetricSeries: ...

class Api:
    def __init__(
        self,
        dir: str | os.PathLike[str] = ".",
        settings: Settings | None = None,
    ) -> None: ...
    def projects(self) -> list[Project]: ...
    def project(self, project_id: str) -> Project | None: ...
    def runs(self, project_id: str) -> list[RunRecord]: ...
    def run(self, path: str) -> RunRecord | None: ...
    def compare_runs(
        self,
        candidate_run_id: str,
        reference_run_id: str,
        *,
        metric_key: str,
        direction: Literal["minimize", "maximize"],
    ) -> ComparisonResult: ...
    def rank_runs(
        self,
        run_ids: list[str],
        *,
        metric_key: str,
        direction: Literal["minimize", "maximize"],
    ) -> RankingResult: ...
    def close(self) -> None: ...
    def __enter__(self) -> Self: ...
    def __exit__(self, exc_type: object, exc_value: object, traceback: object) -> bool: ...

class ArrowTable:
    """An Arrow PyCapsule-compatible, dependency-free query result."""
    @property
    def row_count(self) -> int: ...
    @property
    def source_row_count(self) -> int: ...
    @property
    def downsampled(self) -> bool: ...
    @property
    def column_names(self) -> list[str]: ...
    def __arrow_c_stream__(self, requested_schema: object | None = None) -> object: ...

class Diagnostics:
    @property
    def pending_reports(self) -> int: ...
    @property
    def queue_full_errors(self) -> int: ...
    @property
    def persisted_reports(self) -> int: ...
    @property
    def writer_state(self) -> str: ...
    @property
    def last_write_error(self) -> str | None: ...
    @property
    def last_flush_run_id(self) -> str | None: ...
    @property
    def last_flush_status(self) -> str: ...
    @property
    def last_flush_error(self) -> str | None: ...

class AlignedMetricPoint:
    @property
    def run_id(self) -> str: ...
    @property
    def metric_key(self) -> str: ...
    @property
    def step(self) -> int: ...
    @property
    def timestamp(self) -> str: ...
    @property
    def value_f64(self) -> float: ...
    @property
    def ingested_at(self) -> str: ...
    @property
    def axis_value(self) -> int: ...

class AlignedMetricResult:
    @property
    def points(self) -> list[AlignedMetricPoint]: ...
    @property
    def source_row_count(self) -> int: ...
    @property
    def downsampled(self) -> bool: ...
    @property
    def completeness(
        self,
    ) -> Literal["complete", "partial", "unavailable", "invalid"]: ...
    @property
    def reasons(self) -> list[str]: ...

class ObjectiveMetric:
    @property
    def metric_key(self) -> str: ...
    @property
    def direction(self) -> Literal["minimize", "maximize"]: ...

class ObjectiveEvidence:
    @property
    def run_id(self) -> str: ...
    @property
    def run_status(self) -> Literal["running", "finished", "failed"]: ...
    @property
    def last_step(self) -> int | None: ...
    @property
    def last_value_f64(self) -> float | None: ...
    @property
    def completeness(
        self,
    ) -> Literal["complete", "partial", "unavailable", "invalid"]: ...
    @property
    def reasons(self) -> list[str]: ...

class ComparisonResult:
    @property
    def objective(self) -> ObjectiveMetric: ...
    @property
    def candidate(self) -> ObjectiveEvidence: ...
    @property
    def reference(self) -> ObjectiveEvidence: ...
    @property
    def completeness(
        self,
    ) -> Literal["complete", "partial", "unavailable", "invalid"]: ...
    @property
    def raw_delta(self) -> float | None: ...
    @property
    def relative_delta(self) -> float | None: ...
    @property
    def normalized_improvement(self) -> float | None: ...
    @property
    def outcome(self) -> Literal["improved", "regressed", "equal"] | None: ...
    @property
    def preference(
        self,
    ) -> Literal["candidate", "reference", "no_preference", "inconclusive"]: ...

class _MetricComparisonResult:
    @property
    def metric_key(self) -> str: ...
    @property
    def candidate(self) -> ObjectiveEvidence: ...
    @property
    def reference(self) -> ObjectiveEvidence: ...
    @property
    def completeness(
        self,
    ) -> Literal["complete", "partial", "unavailable", "invalid"]: ...
    @property
    def raw_delta(self) -> float | None: ...
    @property
    def relative_delta(self) -> float | None: ...

class _ComparisonReport:
    @property
    def primary(self) -> ComparisonResult: ...
    @property
    def secondary(self) -> list[_MetricComparisonResult]: ...

class RankingEntry:
    @property
    def evidence(self) -> ObjectiveEvidence: ...
    @property
    def rank(self) -> int | None: ...

class RankingResult:
    @property
    def objective(self) -> ObjectiveMetric: ...
    @property
    def entries(self) -> list[RankingEntry]: ...

class MetricPoint:
    run_id: str
    metric_key: str
    step: int
    timestamp: str
    value_f64: float
    ingested_at: str

class MetricSummary:
    run_id: str
    metric_key: str
    effective_count: int
    last_step: int
    last_value_f64: float
    min_value_f64: float
    max_value_f64: float

class Project:
    project_id: str
    name: str
    created_at: str

class Run:
    run_id: str
    project_id: str
    name: str
    status: str
    created_at: str
    started_at: str
    finished_at: str | None
    def log(self, key: str, step: int, value: float, /) -> None: ...

class Client:
    def create_project(self, name: str, project_id: str | None = None) -> Project: ...
    def get_project(self, project_id: str) -> Project: ...
    def list_projects(self) -> list[Project]:
        """Lists projects in stable catalog order."""
    def create_run(self, project_id: str, name: str, run_id: str | None = None) -> Run: ...
    def get_run(self, run_id: str) -> Run: ...
    def resume_run(self, run_id: str) -> Run: ...
    def list_runs(
        self,
        project_id: str,
        *,
        status: Literal["running", "finished", "failed"] | None = None,
        limit: int | None = None,
        offset: int = 0,
    ) -> list[Run]:
        """Lists project runs in stable created order with pagination."""
    def list_orphan_runs(self, project_id: str | None = None) -> list[Run]: ...
    def finish_run(self, run_id: str, timeout: float | None = None) -> Run: ...
    def fail_run(self, run_id: str, timeout: float | None = None) -> Run: ...
    def flush_run_data(self, run_id: str, timeout: float | None = None) -> None: ...
    def shutdown(self, timeout: float | None = None) -> None: ...
    def __enter__(self) -> Self: ...
    def __exit__(self, exc_type: object, exc_value: object, traceback: object) -> bool: ...
    def diagnostics(self) -> Diagnostics: ...
    def query_aligned_metric(
        self,
        run_id: str,
        metric_key: str,
        *,
        axis: Literal["step", "elapsed_time"],
        start: int,
        end: int,
        pixel_width: int | None = None,
        points_per_pixel: int | None = None,
    ) -> AlignedMetricResult:
        """Queries aligned metric evidence in a closed viewport."""
    def compare_runs(
        self,
        candidate_run_id: str,
        reference_run_id: str,
        *,
        metric_key: str,
        direction: Literal["minimize", "maximize"],
    ) -> ComparisonResult: ...
    def _objective_evidence(self, run_id: str, metric_key: str) -> ObjectiveEvidence: ...
    def _comparison_reports(
        self,
        candidate_run_ids: list[str],
        reference_run_id: str,
        *,
        metric_key: str,
        direction: Literal["minimize", "maximize"],
        secondary_metric_keys: list[str],
    ) -> list[_ComparisonReport]: ...
    def rank_runs(
        self,
        run_ids: list[str],
        *,
        metric_key: str,
        direction: Literal["minimize", "maximize"],
    ) -> RankingResult: ...
    def _best_eligible_run(
        self,
        run_ids: list[str],
        *,
        metric_key: str,
        direction: Literal["minimize", "maximize"],
    ) -> str | None: ...
    def query_metric(
        self,
        run_id: str,
        metric_key: str,
        start_step: int | None = None,
        end_step: int | None = None,
        max_points: int | None = None,
    ) -> list[MetricPoint]:
        """Queries persisted effective points in [start_step, end_step).

        A finite max_points value must be at least 2.
        """
    def _query_metric_with_metadata(
        self,
        run_id: str,
        metric_key: str,
        start_step: int | None = None,
        end_step: int | None = None,
        max_points: int | None = None,
    ) -> tuple[list[MetricPoint], int, bool]: ...
    def query_metric_summaries(self, run_ids: list[str], metric_key: str) -> list[MetricSummary]:
        """Compares persisted metric summaries in requested run order."""
    def query_metric_table(
        self,
        run_id: str,
        metric_key: str,
        start_step: int | None = None,
        end_step: int | None = None,
        max_points: int | None = None,
    ) -> ArrowTable:
        """Queries an Arrow table for the range [start_step, end_step)."""
    def query_metric_summaries_table(self, run_ids: list[str], metric_key: str) -> ArrowTable:
        """Returns metric summaries through the Arrow PyCapsule protocol."""
    def list_metrics(self, run_id: str) -> list[MetricSummary]:
        """Lists metrics derived from persisted effective points."""

def init(
    path: str | os.PathLike[str] = ".",
    *,
    data_path: str | os.PathLike[str] | None = None,
    catalog_backend: Literal["duckdb", "sqlite"] | None = None,
    catalog_path: str | os.PathLike[str] | None = None,
    metric_queue_capacity: int = 65536,
    s3_endpoint: str | None = None,
    s3_access_key_id: str | None = None,
    s3_secret_access_key: str | None = None,
    s3_session_token: str | None = None,
    s3_region: str | None = None,
    s3_path_style: bool | None = None,
    s3_use_ssl: bool | None = None,
    _must_exist: bool = False,
) -> Client:
    """Opens a project client using explicit options or project config.

    Relative paths read from project config resolve against the project root.
    """

def _start_run(
    *,
    project: str | None = None,
    dir: str | os.PathLike[str] | None = None,
    id: str | None = None,
    name: str | None = None,
    resume: bool | Literal["allow", "never", "must"] | None = None,
    settings: Settings | None = None,
) -> _Run: ...
