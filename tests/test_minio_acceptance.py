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

from tests import helpers

_CatalogBackend = Literal["duckdb", "sqlite"]
_CATALOG_BACKENDS: tuple[_CatalogBackend, ...] = ("duckdb", "sqlite")
_REQUIRED_ENV = (
    "PULSEON_MINIO_ENDPOINT",
    "PULSEON_MINIO_BUCKET",
    "PULSEON_MINIO_ACCESS_KEY_ID",
    "PULSEON_MINIO_SECRET_ACCESS_KEY",
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
    root_path = tmp_path / catalog_backend / "pulseon"
    prefix = f"pulseon-acceptance/{uuid.uuid4().hex}/{catalog_backend}"
    partition_prefix = prefix + "/main/metric_points/run_id=run-1/metric_key_encoded=train%252Floss/"
    client = _open_minio_client(root_path, config, prefix, catalog_backend)
    project = client.create_project("minio acceptance", project_id="project-1")
    run = client.create_run(project.project_id, "baseline", run_id="run-1")
    run.log("train/loss", 0, 0.25)
    run.log("train/loss", 1, 0.125)
    run.log("eval/accuracy", 0, 0.8)

    active_points = helpers.wait_for_metric_points(client, run.run_id, "train/loss", expected_count=2)
    helpers.wait_for_metric_points(
        client,
        run.run_id,
        "eval/accuracy",
        expected_count=1,
    )
    discovered_projects = client.list_projects()
    discovered_runs = client.list_runs(project.project_id, status="running", limit=1, offset=0)
    active_metrics = client.list_metrics(run.run_id)
    ranged_points = client.query_metric(run.run_id, "train/loss", start_step=0, end_step=1)
    finished = client.finish_run(run.run_id)
    terminal_points = client.query_metric(run.run_id, "train/loss")
    summaries = client.query_metric_summaries([run.run_id], "train/loss")
    metrics = client.list_metrics(run.run_id)
    diagnostics = client.diagnostics()
    terminal_keys = _list_minio_keys(config, partition_prefix)
    client.flush_run_data(run.run_id)
    retry_diagnostics = client.diagnostics()
    retry_keys = _list_minio_keys(config, partition_prefix)
    client.shutdown()

    reopened = _open_minio_client(root_path, config, prefix, catalog_backend)
    reopened_run = reopened.get_run(finished.run_id)
    reopened_points = reopened.query_metric(finished.run_id, "train/loss")
    reopened.shutdown()

    assert [point.value_f64 for point in active_points] == [0.25, 0.125]
    assert [item.project_id for item in discovered_projects] == ["project-1"]
    assert [item.run_id for item in discovered_runs] == ["run-1"]
    assert [metric.metric_key for metric in active_metrics] == [
        "eval/accuracy",
        "train/loss",
    ]
    assert [point.step for point in ranged_points] == [0]
    assert finished.status == "finished"
    assert [point.step for point in terminal_points] == [0, 1]
    assert [summary.effective_count for summary in summaries] == [2]
    assert [summary.last_value_f64 for summary in summaries] == [0.125]
    assert [metric.metric_key for metric in metrics] == [
        "eval/accuracy",
        "train/loss",
    ]
    assert diagnostics.last_flush_run_id == "run-1"
    assert diagnostics.last_flush_status == "succeeded"
    assert any(key.endswith(".parquet") for key in terminal_keys)
    assert retry_diagnostics.last_flush_run_id == "run-1"
    assert retry_diagnostics.last_flush_status == "succeeded"
    assert any(key.endswith(".parquet") for key in retry_keys)
    assert reopened_run.status == "finished"
    assert [point.step for point in reopened_points] == [0, 1]


def _open_minio_client(
    root_path: pathlib.Path,
    config: MinioConfig,
    prefix: str,
    catalog_backend: _CatalogBackend,
):
    import pulseon

    return pulseon.init(
        root_path,
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
        endpoint=os.environ["PULSEON_MINIO_ENDPOINT"],
        bucket=os.environ["PULSEON_MINIO_BUCKET"],
        access_key_id=os.environ["PULSEON_MINIO_ACCESS_KEY_ID"],
        secret_access_key=os.environ["PULSEON_MINIO_SECRET_ACCESS_KEY"],
        region=os.environ.get("PULSEON_MINIO_REGION", "us-east-1"),
        use_ssl=_parse_bool(os.environ.get("PULSEON_MINIO_USE_SSL", "false")),
    )


def _parse_bool(value: str) -> bool:
    normalized = value.strip().lower()
    if normalized in {"1", "true", "yes", "on"}:
        return True
    if normalized in {"0", "false", "no", "off"}:
        return False
    raise AssertionError("PULSEON_MINIO_USE_SSL must be a boolean")
