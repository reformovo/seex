"""Verify the public Settings object."""

from __future__ import annotations

import pathlib

import pytest
import seex


def test_settings_are_typed_and_redact_credentials(tmp_path: pathlib.Path) -> None:
    settings = seex.Settings(
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
    with pytest.raises(ValueError, match="metric_queue_capacity"):
        seex.Settings(metric_queue_capacity=capacity)


def test_settings_rejects_legacy_duckdb_catalog_backend() -> None:
    with pytest.raises(ValueError, match="catalog_backend must be 'sqlite'"):
        seex.Settings(catalog_backend="duckdb")  # type: ignore[reportArgumentType]


def test_init_rejects_s3_data_path(tmp_path: pathlib.Path) -> None:
    with pytest.raises(seex.InvalidConfigurationError, match="configuration is invalid"):
        seex.init(
            project="project-1",
            dir=tmp_path,
            settings=seex.Settings(data_path="s3://bucket/seex"),
        )
