"""Seex Python API."""

from __future__ import annotations

import os
from typing import Literal

from seex import _seex

Diagnostics = _seex.Diagnostics
InvalidConfigurationError = _seex.InvalidConfigurationError
InvalidRunStateError = _seex.InvalidRunStateError
MetricDrainTimeoutError = _seex.MetricDrainTimeoutError
MetricFlushError = _seex.MetricFlushError
MetricFlushTimeoutError = _seex.MetricFlushTimeoutError
MetricQueueFullError = _seex.MetricQueueFullError
MetricWriterFailedError = _seex.MetricWriterFailedError
SeexError = _seex.SeexError
Run = _seex.Run
RunAlreadyActiveError = _seex.RunAlreadyActiveError
RunAlreadyExistsError = _seex.RunAlreadyExistsError
RunClosedError = _seex.RunClosedError
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
    return _seex.init(
        project=project,
        dir=dir,
        id=id,
        name=name,
        resume=resume,
        settings=settings,
    )


__all__ = [
    "Diagnostics",
    "InvalidConfigurationError",
    "InvalidRunStateError",
    "MetricDrainTimeoutError",
    "MetricFlushError",
    "MetricFlushTimeoutError",
    "MetricQueueFullError",
    "MetricWriterFailedError",
    "Run",
    "RunAlreadyActiveError",
    "RunAlreadyExistsError",
    "RunClosedError",
    "SeexError",
    "Settings",
    "StorageError",
    "init",
]
