"""Opt-in MinIO acceptance coverage for S3-backed data paths."""

from __future__ import annotations

import dataclasses
import datetime as dt
import hashlib
import hmac
import os
import pathlib
import urllib.parse
import urllib.request
import uuid
from typing import Literal

import pytest
import seex

_CatalogBackend = Literal["duckdb", "sqlite"]
_CATALOG_BACKENDS: tuple[_CatalogBackend, ...] = ("duckdb", "sqlite")
_REQUIRED_ENV = (
    "SEEX_MINIO_ENDPOINT",
    "SEEX_MINIO_BUCKET",
    "SEEX_MINIO_ACCESS_KEY_ID",
    "SEEX_MINIO_SECRET_ACCESS_KEY",
)
_EMPTY_SHA256 = hashlib.sha256(b"").hexdigest()
_URL_SAFE = "-_.~"


@dataclasses.dataclass(frozen=True)
class MinioConfig:
    endpoint: str
    bucket: str
    access_key_id: str
    secret_access_key: str
    region: str
    use_ssl: bool


@pytest.mark.parametrize("catalog_backend", _CATALOG_BACKENDS)
def test_minio_s3_data_path_round_trips_catalog_backend(
    tmp_path: pathlib.Path,
    catalog_backend: _CatalogBackend,
) -> None:
    config = _require_minio_config()
    root_path = tmp_path / catalog_backend / "seex"
    prefix = f"seex-acceptance/{uuid.uuid4().hex}/{catalog_backend}"
    partition_prefix = prefix + "/main/metric_points/run_id=run-1/metric_key_encoded=train%252Floss/"
    settings = _minio_settings(config, prefix, catalog_backend)
    run = seex.init(
        project="project-1",
        dir=root_path,
        id="run-1",
        name="baseline",
        settings=settings,
    )
    run.log({"train/loss": 0.25, "eval/accuracy": 0.8}, step=0)
    run.log({"train/loss": 0.125}, step=1)
    run.finish()
    diagnostics = run.diagnostics()
    terminal_keys = _list_minio_keys(config, partition_prefix)
    run.finish()
    retry_diagnostics = run.diagnostics()
    retry_keys = _list_minio_keys(config, partition_prefix)

    with seex.Api(root_path, settings) as api:
        assert [item.project_id for item in api.projects()] == ["project-1"]
        assert [item.run_id for item in api.runs("project-1")] == ["run-1"]
        record = api.run("project-1/run-1")
        assert record is not None
        assert record.status == "finished"
        assert [point.step for point in record.history("train/loss", start=0, end=1).points] == [0]
        assert [point.step for point in record.history("train/loss").points] == [0, 1]
        assert [metric.metric_key for metric in record.metrics()] == [
            "eval/accuracy",
            "train/loss",
        ]
        summary = record.metric_summary("train/loss")
        assert summary is not None
        assert (summary.effective_count, summary.last_value_f64) == (2, 0.125)
    assert diagnostics.last_flush_run_id == "run-1"
    assert diagnostics.last_flush_status == "succeeded"
    assert any(key.endswith(".parquet") for key in terminal_keys)
    assert retry_diagnostics.last_flush_run_id == "run-1"
    assert retry_diagnostics.last_flush_status == "succeeded"
    assert any(key.endswith(".parquet") for key in retry_keys)
    with seex.Api(root_path, settings) as reopened:
        reopened_run = reopened.run("project-1/run-1")
        assert reopened_run is not None
        assert reopened_run.status == "finished"
        assert [point.step for point in reopened_run.history("train/loss").points] == [0, 1]


def _minio_settings(
    config: MinioConfig,
    prefix: str,
    catalog_backend: _CatalogBackend,
) -> seex.Settings:
    return seex.Settings(
        data_path=f"s3://{config.bucket}/{prefix.strip('/')}",
        catalog_backend=catalog_backend,
        s3_endpoint=config.endpoint,
        s3_access_key_id=config.access_key_id,
        s3_secret_access_key=config.secret_access_key,
        s3_region=config.region,
        s3_path_style=True,
        s3_use_ssl=config.use_ssl,
    )


def _list_minio_keys(config: MinioConfig, prefix: str) -> list[str]:
    query = urllib.parse.urlencode(
        {"list-type": "2", "prefix": prefix},
        quote_via=urllib.parse.quote,
        safe=_URL_SAFE,
    )
    scheme = "https" if config.use_ssl else "http"
    request = urllib.request.Request(
        f"{scheme}://{config.endpoint}/{urllib.parse.quote(config.bucket, safe=_URL_SAFE)}?{query}",
        headers=_signed_list_headers(config, query),
        method="GET",
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        document = response.read()

    raw_keys = (part.split(b"</Key>", 1)[0] for part in document.split(b"<Key>")[1:])
    return [key.decode("utf-8", "strict") for key in raw_keys]


def _signed_list_headers(config: MinioConfig, query: str) -> dict[str, str]:
    now = dt.datetime.now(dt.UTC)
    date_stamp = now.strftime("%Y%m%d")
    amz_date = now.strftime("%Y%m%dT%H%M%SZ")
    region = config.region or "us-east-1"
    headers = {
        "host": config.endpoint,
        "x-amz-content-sha256": _EMPTY_SHA256,
        "x-amz-date": amz_date,
    }
    signed_headers = ";".join(sorted(headers))
    canonical_headers = "".join(f"{name}:{headers[name]}\n" for name in sorted(headers))
    bucket_path = urllib.parse.quote(config.bucket, safe=_URL_SAFE)
    canonical_request = "\n".join(
        (
            "GET",
            f"/{bucket_path}",
            query,
            canonical_headers,
            signed_headers,
            _EMPTY_SHA256,
        )
    )
    scope = f"{date_stamp}/{region}/s3/aws4_request"
    string_to_sign = "\n".join(
        (
            "AWS4-HMAC-SHA256",
            amz_date,
            scope,
            hashlib.sha256(canonical_request.encode()).hexdigest(),
        )
    )
    signature = hmac.new(
        _signing_key(config.secret_access_key, date_stamp, region),
        string_to_sign.encode(),
        hashlib.sha256,
    ).hexdigest()
    headers["Authorization"] = (
        "AWS4-HMAC-SHA256 "
        f"Credential={config.access_key_id}/{scope}, "
        f"SignedHeaders={signed_headers}, "
        f"Signature={signature}"
    )
    return headers


def _signing_key(secret_access_key: str, date_stamp: str, region: str) -> bytes:
    date_key = _sign(f"AWS4{secret_access_key}".encode(), date_stamp)
    region_key = _sign(date_key, region)
    service_key = _sign(region_key, "s3")
    return _sign(service_key, "aws4_request")


def _sign(key: bytes, message: str) -> bytes:
    return hmac.new(key, message.encode(), hashlib.sha256).digest()


def _require_minio_config() -> MinioConfig:
    missing = [name for name in _REQUIRED_ENV if not os.environ.get(name)]
    if missing:
        pytest.skip("set MinIO acceptance environment variables: " + ", ".join(missing))

    return MinioConfig(
        endpoint=os.environ["SEEX_MINIO_ENDPOINT"],
        bucket=os.environ["SEEX_MINIO_BUCKET"],
        access_key_id=os.environ["SEEX_MINIO_ACCESS_KEY_ID"],
        secret_access_key=os.environ["SEEX_MINIO_SECRET_ACCESS_KEY"],
        region=os.environ.get("SEEX_MINIO_REGION", "us-east-1"),
        use_ssl=_parse_bool(os.environ.get("SEEX_MINIO_USE_SSL", "false")),
    )


def _parse_bool(value: str) -> bool:
    normalized = value.strip().lower()
    if normalized in {"1", "true", "yes", "on"}:
        return True
    if normalized in {"0", "false", "no", "off"}:
        return False
    raise AssertionError("SEEX_MINIO_USE_SSL must be a boolean")
