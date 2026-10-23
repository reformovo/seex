"""Verify the public Run-only Python surface."""

from __future__ import annotations


def test_import_seex() -> None:
    import seex

    assert hasattr(seex, "__all__")
    assert seex.Run.__name__ == "Run"
    assert seex.Settings.__name__ == "Settings"
    assert seex.Diagnostics.__name__ == "Diagnostics"
    assert set(seex.__all__) == {
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
    }
    for removed in (
        "Api",
        "RunRecord",
        "Project",
        "MetricSeries",
        "MetricPoint",
        "MetricSummary",
        "ArrowTable",
        "ComparisonResult",
        "RankingResult",
    ):
        assert not hasattr(seex, removed)
