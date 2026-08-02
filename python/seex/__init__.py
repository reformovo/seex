"""Seex Python API."""

from __future__ import annotations

import os
from typing import Literal

from seex import _seex

Api = _seex.Api
ApiClosedError = _seex.ApiClosedError
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
Run = _seex._Run
RunAlreadyActiveError = _seex.RunAlreadyActiveError
RunAlreadyExistsError = _seex.RunAlreadyExistsError
RunClosedError = _seex.RunClosedError
RunRecord = _seex.RunRecord
Settings = _seex.Settings
StorageError = _seex.StorageError


def init(
    *,
    project: str | None = None,
    dir: str | os.PathLike[str] | None = None,
    id: str | None = None,
    name: str | None = None,
    resume: bool | Literal["never", "allow", "must"] | None = None,
    settings: Settings | None = None,
) -> Run:
    return _seex._start_run(
        project=project,
        dir=dir,
        id=id,
        name=name,
        resume=resume,
        settings=settings,
    )


__all__ = [
    "Api",
    "ApiClosedError",
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
