"""Verify shared pytest helper behavior."""

from __future__ import annotations

import dataclasses
from typing import TYPE_CHECKING, cast

import pytest

from tests import helpers

if TYPE_CHECKING:
    import seex
    from seex import _seex


@dataclasses.dataclass(frozen=True)
class _FakeDiagnostics:
    pending_reports: int = 4
    queue_full_errors: int = 2
    persisted_reports: int = 3
    writer_state: str = "running"
    last_write_error: str | None = "write failed"
    last_flush_run_id: str | None = "run-1"
    last_flush_status: str = "failed"
    last_flush_error: str | None = "flush failed"


class _FakeClient:
    def query_metric(
        self,
        run_id: str,
        metric_key: str,
    ) -> list[seex.MetricPoint]:
        del run_id, metric_key
        return []

    def diagnostics(self) -> _FakeDiagnostics:
        return _FakeDiagnostics()


def test_wait_for_metric_points_timeout_includes_context() -> None:
    client = cast("_seex.Client", _FakeClient())

    with pytest.raises(AssertionError) as error:
        helpers.wait_for_metric_points(
            client,
            "run-1",
            "train/loss",
            expected_count=2,
            timeout_seconds=0.0,
            sleep_seconds=0.0,
        )

    message = str(error.value)
    assert "expected_count=2" in message
    assert "actual_count=0" in message
    assert "run_id='run-1'" in message
    assert "metric_key='train/loss'" in message
    assert "pending_reports=4" in message
    assert "queue_full_errors=2" in message
    assert "writer_state='running'" in message
