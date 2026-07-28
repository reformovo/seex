"""PulseOn Python API."""

from __future__ import annotations

import os
from typing import Literal

from pulseon import _pulseon

AlignedMetricPoint = _pulseon.AlignedMetricPoint
AlignedMetricResult = _pulseon.AlignedMetricResult
ArrowTable = _pulseon.ArrowTable
Client = _pulseon.Client
ClientClosedError = _pulseon.ClientClosedError
ComparisonResult = _pulseon.ComparisonResult
Diagnostics = _pulseon.Diagnostics
InvalidConfigurationError = _pulseon.InvalidConfigurationError
InvalidRunStateError = _pulseon.InvalidRunStateError
MetricDrainTimeoutError = _pulseon.MetricDrainTimeoutError
MetricFlushError = _pulseon.MetricFlushError
MetricFlushTimeoutError = _pulseon.MetricFlushTimeoutError
MetricPoint = _pulseon.MetricPoint
MetricQueueFullError = _pulseon.MetricQueueFullError
MetricSummary = _pulseon.MetricSummary
MetricWriterFailedError = _pulseon.MetricWriterFailedError
ObjectiveEvidence = _pulseon.ObjectiveEvidence
ObjectiveMetric = _pulseon.ObjectiveMetric
PulseOnError = _pulseon.PulseOnError
Project = _pulseon.Project
RankingEntry = _pulseon.RankingEntry
RankingResult = _pulseon.RankingResult
Run = _pulseon.Run
RunAlreadyActiveError = _pulseon.RunAlreadyActiveError
RunAlreadyExistsError = _pulseon.RunAlreadyExistsError
RunClosedError = _pulseon.RunClosedError
StorageError = _pulseon.StorageError


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
) -> Client:
    return _pulseon.init(
        path,
        data_path=data_path,
        catalog_backend=catalog_backend,
        catalog_path=catalog_path,
        metric_queue_capacity=metric_queue_capacity,
        s3_endpoint=s3_endpoint,
        s3_access_key_id=s3_access_key_id,
        s3_secret_access_key=s3_secret_access_key,
        s3_session_token=s3_session_token,
        s3_region=s3_region,
        s3_path_style=s3_path_style,
        s3_use_ssl=s3_use_ssl,
    )


__all__ = [
    "AlignedMetricPoint",
    "AlignedMetricResult",
    "ArrowTable",
    "Client",
    "ClientClosedError",
    "ComparisonResult",
    "Diagnostics",
    "InvalidConfigurationError",
    "InvalidRunStateError",
    "MetricDrainTimeoutError",
    "MetricFlushError",
    "MetricFlushTimeoutError",
    "MetricPoint",
    "MetricQueueFullError",
    "MetricSummary",
    "MetricWriterFailedError",
    "ObjectiveEvidence",
    "ObjectiveMetric",
    "Project",
    "PulseOnError",
    "RankingEntry",
    "RankingResult",
    "Run",
    "RunAlreadyActiveError",
    "RunAlreadyExistsError",
    "RunClosedError",
    "StorageError",
    "init",
]
