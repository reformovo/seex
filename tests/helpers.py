"""Shared pytest helpers for Python SDK behavior tests."""

from __future__ import annotations

import time
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import seex

V2_DIAGNOSTIC_FIELDS = (
    "pending_reports",
    "queue_full_errors",
    "persisted_reports",
    "writer_state",
    "last_write_error",
    "last_flush_run_id",
    "last_flush_status",
    "last_flush_error",
)

V2_REMOVED_DIAGNOSTIC_FIELDS = (
    "accepted_reports",
    "queued_reports",
    "dropped_reports",
    "failed_reports",
    "enqueued_reports",
    "writer_drained",
)


def wait_for_metric_points(
    client: seex.Client,
    run_id: str,
    metric_key: str,
    expected_count: int,
    *,
    timeout_seconds: float = 5.0,
    sleep_seconds: float = 0.05,
) -> list[seex.MetricPoint]:
    deadline = time.monotonic() + timeout_seconds
    points: list[seex.MetricPoint] = []
    while time.monotonic() <= deadline:
        points = client.query_metric(run_id, metric_key)
        if len(points) >= expected_count:
            return points
        time.sleep(sleep_seconds)

    actual_count = len(points)
    diagnostics = client.diagnostics()
    raise AssertionError(
        "Timed out waiting for metric points: "
        f"expected_count={expected_count}, "
        f"actual_count={actual_count}, "
        f"run_id={run_id!r}, "
        f"metric_key={metric_key!r}, "
        f"diagnostics={_format_diagnostics(diagnostics)}"
    )


def _format_diagnostics(diagnostics: seex.Diagnostics) -> str:
    return (
        "{"
        f"pending_reports={diagnostics.pending_reports}, "
        f"queue_full_errors={diagnostics.queue_full_errors}, "
        f"persisted_reports={diagnostics.persisted_reports}, "
        f"writer_state={diagnostics.writer_state!r}, "
        f"last_write_error={diagnostics.last_write_error!r}, "
        f"last_flush_run_id={diagnostics.last_flush_run_id!r}, "
        f"last_flush_status={diagnostics.last_flush_status!r}, "
        f"last_flush_error={diagnostics.last_flush_error!r}"
        "}"
    )
