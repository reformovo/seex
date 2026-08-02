"""Verify the target U4 Settings object before the public API cutover."""

from __future__ import annotations

import pathlib

import pytest


def test_settings_are_typed_and_redact_credentials(tmp_path: pathlib.Path) -> None:
    from seex import _seex

    settings = _seex.Settings(
        catalog_backend="sqlite",
        data_path=tmp_path / "data",
        s3_access_key_id="credential-a",
        s3_secret_access_key="credential-b",
        s3_session_token="credential-c",
    )

    rendered = repr(settings)
    assert settings.catalog_backend == "sqlite"
    assert settings.data_path == tmp_path / "data"
    assert "credential-a" not in rendered
    assert "credential-b" not in rendered
    assert "credential-c" not in rendered
    assert rendered.count("[REDACTED]") == 3


@pytest.mark.parametrize("capacity", [0, 1_048_577])
def test_settings_reject_invalid_queue_capacity(capacity: int) -> None:
    from seex import _seex

    with pytest.raises(ValueError, match="metric_queue_capacity"):
        _seex.Settings(metric_queue_capacity=capacity)
