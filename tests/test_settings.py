"""Verify the public Settings object."""

from __future__ import annotations

import pathlib

import pytest
import seex


def test_settings_expose_local_storage_fields(tmp_path: pathlib.Path) -> None:
    settings = seex.Settings(
        catalog_backend="sqlite",
        catalog_path=tmp_path / "catalog.sqlite",
        data_path=tmp_path / "data",
    )

    assert settings.catalog_backend == "sqlite"
    assert settings.catalog_path == tmp_path / "catalog.sqlite"
    assert settings.data_path == tmp_path / "data"
    assert settings.metric_queue_capacity == 65536
    assert not hasattr(settings, "s3_endpoint")


@pytest.mark.parametrize("capacity", [0, 1_048_577])
def test_settings_reject_invalid_queue_capacity(capacity: int) -> None:
    with pytest.raises(ValueError, match="metric_queue_capacity"):
        seex.Settings(metric_queue_capacity=capacity)


def test_settings_rejects_legacy_duckdb_catalog_backend() -> None:
    with pytest.raises(ValueError, match="catalog_backend must be 'sqlite'"):
        seex.Settings(catalog_backend="duckdb")  # type: ignore[reportArgumentType]


def test_init_rejects_remote_data_path(tmp_path: pathlib.Path) -> None:
    with pytest.raises(seex.InvalidConfigurationError, match="configuration is invalid"):
        seex.init(
            project="project-1",
            dir=tmp_path,
            settings=seex.Settings(data_path="s3://bucket/seex"),
        )
