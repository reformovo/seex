"""Seex Python API."""

from __future__ import annotations

import os
from typing import Literal

from seex import _seex

AlignedMetricPoint = _seex.AlignedMetricPoint
AlignedMetricResult = _seex.AlignedMetricResult
Api = _seex.Api
ApiClosedError = _seex.ApiClosedError
ArrowTable = _seex.ArrowTable
Client = _seex.Client
ClientClosedError = _seex.ClientClosedError
ComparisonResult = _seex.ComparisonResult
Diagnostics = _seex.Diagnostics
InvalidConfigurationError = _seex.InvalidConfigurationError
InvalidRunStateError = _seex.InvalidRunStateError
MetricDrainTimeoutError = _seex.MetricDrainTimeoutError
MetricFlushError = _seex.MetricFlushError
MetricFlushTimeoutError = _seex.MetricFlushTimeoutError
MetricPoint = _seex.MetricPoint
MetricQueueFullError = _seex.MetricQueueFullError
MetricSummary = _seex.MetricSummary
MetricSeries = _seex.MetricSeries
MetricWriterFailedError = _seex.MetricWriterFailedError
ObjectiveEvidence = _seex.ObjectiveEvidence
ObjectiveMetric = _seex.ObjectiveMetric
SeexError = _seex.SeexError
Project = _seex.Project
RankingEntry = _seex.RankingEntry
RankingResult = _seex.RankingResult
Run = _seex.Run
RunAlreadyActiveError = _seex.RunAlreadyActiveError
RunAlreadyExistsError = _seex.RunAlreadyExistsError
RunClosedError = _seex.RunClosedError
RunRecord = _seex.RunRecord
Settings = _seex.Settings
StorageError = _seex.StorageError


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
    return _seex.init(
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
    "Api",
    "ApiClosedError",
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
    "MetricSeries",
    "MetricSummary",
    "MetricWriterFailedError",
    "ObjectiveEvidence",
    "ObjectiveMetric",
    "Project",
    "RankingEntry",
    "RankingResult",
    "Run",
    "RunAlreadyActiveError",
    "RunAlreadyExistsError",
    "RunClosedError",
    "RunRecord",
    "SeexError",
    "Settings",
    "StorageError",
    "init",
]
