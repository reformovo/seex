"""Dependency-free read-only command-line interface for Seex stores."""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import math
import os
import pathlib
import shutil
import sys
from collections.abc import Generator, Sequence

from seex import _seex

_JSON_SCHEMA_VERSION = 2
_APP_BINARY = "seex-app"
_LTTB_AUTO_INSTALL_ENV = "SEEX_LTTB_AUTO_INSTALL"
_LTTB_ERROR_PREFIX = "DuckDB LTTB extension is unavailable:"
_OPERATION_ERROR_CODES = {
    "ClientClosedError": "client_closed",
    "InvalidConfigurationError": "invalid_configuration",
    "InvalidRunStateError": "invalid_run_state",
    "MetricDrainTimeoutError": "metric_drain_timeout",
    "MetricFlushError": "metric_flush_failed",
    "MetricFlushTimeoutError": "metric_flush_timeout",
    "MetricQueueFullError": "metric_queue_full",
    "MetricWriterFailedError": "metric_writer_failed",
    "RunAlreadyActiveError": "run_already_active",
    "RunAlreadyExistsError": "run_already_exists",
    "RunClosedError": "run_closed",
    "StorageError": "storage_error",
}


class _CliRequestError(Exception):
    """A semantic CLI request error with a stable machine code."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def _non_negative_int(value: str) -> int:
    try:
        parsed = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected a non-negative integer") from error
    if parsed < 0:
        raise argparse.ArgumentTypeError("expected a non-negative integer")
    return parsed


@contextlib.contextmanager
def _enable_lttb_auto_install() -> Generator[None, None, None]:
    """Enables LTTB downloads only for one CLI metric query."""
    previous = os.environ.get(_LTTB_AUTO_INSTALL_ENV)
    os.environ[_LTTB_AUTO_INSTALL_ENV] = "1"
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop(_LTTB_AUTO_INSTALL_ENV, None)
        else:
            os.environ[_LTTB_AUTO_INSTALL_ENV] = previous


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="seex")
    parser.add_argument("--path", type=pathlib.Path, default=pathlib.Path("."))
    parser.add_argument("--format", choices=("table", "json"), default="table")
    parser.add_argument("--catalog-backend", choices=("duckdb", "sqlite"))
    parser.add_argument("--catalog-path")
    parser.add_argument("--data-path")
    resources = parser.add_subparsers(dest="resource", required=True)

    app = resources.add_parser("app")
    app.add_argument("project_path", nargs="?", type=pathlib.Path)

    projects = resources.add_parser("projects")
    projects.add_subparsers(dest="action", required=True).add_parser("list")

    runs = resources.add_parser("runs")
    runs_list = runs.add_subparsers(dest="action", required=True).add_parser("list")
    runs_list.add_argument("project_id")
    runs_list.add_argument("--status", choices=("running", "finished", "failed"))
    runs_list.add_argument("--limit", type=_non_negative_int)
    runs_list.add_argument("--offset", type=_non_negative_int, default=0)

    metrics = resources.add_parser("metrics")
    metric_actions = metrics.add_subparsers(dest="action", required=True)
    metrics_list = metric_actions.add_parser("list")
    metrics_list.add_argument("run_id")
    metrics_query = metric_actions.add_parser("query")
    metrics_query.add_argument("run_id")
    metrics_query.add_argument("metric_key")
    metrics_query.add_argument("--start-step", type=int)
    metrics_query.add_argument("--end-step", type=int)
    point_limit = metrics_query.add_mutually_exclusive_group()
    point_limit.add_argument("--max-points", type=_non_negative_int, default=200)
    point_limit.add_argument("--all", action="store_true")
    metrics_compare = metric_actions.add_parser("compare")
    metrics_compare.add_argument("metric_key")
    metrics_compare.add_argument("run_ids", nargs="+")
    metrics_compare.add_argument("--baseline", required=True)
    metrics_compare.add_argument("--direction", choices=("minimize", "maximize"), required=True)
    metrics_compare.add_argument("--secondary", action="append", default=[])

    autoresearch = resources.add_parser("autoresearch")
    autoresearch_actions = autoresearch.add_subparsers(dest="action", required=True)
    autoresearch_compare = autoresearch_actions.add_parser("compare")
    autoresearch_compare.add_argument("candidate_run_id")
    autoresearch_compare.add_argument("--metric", required=True)
    autoresearch_compare.add_argument("--direction", choices=("minimize", "maximize"), required=True)
    reference = autoresearch_compare.add_mutually_exclusive_group(required=True)
    reference.add_argument("--against")
    reference.add_argument("--comparator", action="append")
    autoresearch_compare.add_argument("--secondary", action="append", default=[])
    leaderboard = autoresearch_actions.add_parser("leaderboard")
    leaderboard.add_argument("project_id")
    leaderboard.add_argument("--metric", required=True)
    leaderboard.add_argument("--direction", choices=("minimize", "maximize"), required=True)
    leaderboard.add_argument("--run", dest="run_ids", action="append", default=[])
    page_size = leaderboard.add_mutually_exclusive_group()
    page_size.add_argument("--limit", type=_non_negative_int, default=50)
    page_size.add_argument("--all", action="store_true")
    leaderboard.add_argument("--offset", type=_non_negative_int, default=0)
    best = autoresearch_actions.add_parser("best")
    best.add_argument("project_id")
    best.add_argument("--metric", required=True)
    best.add_argument("--direction", choices=("minimize", "maximize"), required=True)
    best.add_argument("--run", dest="run_ids", action="append", default=[])
    return parser


def _parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    error_output = io.StringIO()
    parser = _build_parser()
    try:
        with contextlib.redirect_stderr(error_output):
            args = parser.parse_args(argv)
            _validate_args(parser, args)
            return args
    except SystemExit as error:
        if error.code == 2 and _json_requested(argv):
            print(
                _render_error("cli_usage_error", _usage_message(error_output.getvalue())),
                file=sys.stderr,
            )
        else:
            print(error_output.getvalue(), end="", file=sys.stderr)
        raise


def _validate_args(parser: argparse.ArgumentParser, args: argparse.Namespace) -> None:
    if args.resource == "app":
        return
    if args.resource == "autoresearch" and args.action in ("leaderboard", "best"):
        if len(set(args.run_ids)) != len(args.run_ids):
            parser.error("autoresearch Run IDs must be unique")
        if args.action == "leaderboard" and args.all and args.offset != 0:
            parser.error("--all cannot be combined with a non-zero --offset")
        return
    if args.action != "compare":
        return
    if args.resource == "metrics":
        _validate_metrics_compare(parser, args)
        return
    references = [args.against] if args.against is not None else args.comparator
    if len(set(references)) != len(references):
        parser.error("autoresearch comparator Run IDs must be unique")
    if args.candidate_run_id in references:
        parser.error("autoresearch candidate must not be a comparator")


def _validate_metrics_compare(parser: argparse.ArgumentParser, args: argparse.Namespace) -> None:
    if len(set(args.run_ids)) != len(args.run_ids):
        parser.error("metrics compare Run IDs must be unique")
    if args.baseline not in args.run_ids:
        parser.error("--baseline must be contained in the requested Run IDs")


def _json_requested(argv: Sequence[str] | None) -> bool:
    arguments = tuple(sys.argv[1:] if argv is None else argv)
    for index, value in enumerate(arguments):
        if value == "--format=json":
            return True
        if value == "--format" and arguments[index + 1 : index + 2] == ("json",):
            return True
    return False


def _usage_message(output: str) -> str:
    marker = ": error: "
    return output.rsplit(marker, maxsplit=1)[-1].strip()


def _render_error(
    code: str,
    message: str,
    guidance: Sequence[dict[str, str]] | None = None,
) -> str:
    error: dict[str, object] = {"code": code, "message": message}
    if guidance is not None:
        error["guidance"] = list(guidance)
    document = {
        "schema_version": _JSON_SCHEMA_VERSION,
        "error": error,
    }
    return _dump_json(document)


def _operation_error_details(
    error: _seex.SeexError,
) -> tuple[str, list[dict[str, str]] | None]:
    if isinstance(error, _seex.StorageError) and str(error).startswith(_LTTB_ERROR_PREFIX):
        return (
            "lttb_extension_unavailable",
            [
                {"action": "query_all", "argument": "--all"},
                {
                    "action": "load_local_extension",
                    "environment_variable": "SEEX_LTTB_EXTENSION_PATH",
                },
            ],
        )
    return (
        _OPERATION_ERROR_CODES.get(type(error).__name__, "operation_failed"),
        None,
    )


def _dump_json(document: object) -> str:
    return json.dumps(
        _json_value(document),
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    )


def _json_scalar(value: object) -> object:
    """Returns a standard-JSON representation for a scalar value."""
    if not isinstance(value, float) or math.isfinite(value):
        return value
    if math.isnan(value):
        return "NaN"
    return "Infinity" if value > 0 else "-Infinity"


def _json_value(value: object) -> object:
    if isinstance(value, dict):
        return {key: _json_value(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_json_value(item) for item in value]
    return _json_scalar(value)


def _render_table(headers: Sequence[str], rows: Sequence[Sequence[object]]) -> str:
    text_rows = [[str(value) for value in row] for row in rows]
    widths = [max(len(header), *(len(row[index]) for row in text_rows)) for index, header in enumerate(headers)]

    def render(row: Sequence[str]) -> str:
        return "  ".join(value.ljust(widths[index]) for index, value in enumerate(row)).rstrip()

    divider = ["-" * width for width in widths]
    return "\n".join((render(headers), render(divider), *(render(row) for row in text_rows)))


def _render(
    headers: Sequence[str],
    rows: Sequence[Sequence[object]],
    output_format: str,
    *,
    kind: str,
    page: dict[str, object] | None = None,
    meta: dict[str, object] | None = None,
) -> str:
    if output_format == "table":
        return _render_table(headers, rows)
    keys = [header.lower() for header in headers]
    data = [{key: _json_scalar(value) for key, value in zip(keys, row, strict=True)} for row in rows]
    document = {
        "schema_version": _JSON_SCHEMA_VERSION,
        "kind": kind,
        "data": data,
        "page": page,
        "meta": {} if meta is None else meta,
    }
    return _dump_json(document)


def _run(client: _seex.Client, args: argparse.Namespace) -> str:
    if args.resource == "projects":
        projects = client.list_projects()
        return _render(
            ("PROJECT_ID", "NAME", "CREATED_AT"),
            [(item.project_id, item.name, item.created_at) for item in projects],
            args.format,
            kind="projects",
        )
    if args.resource == "runs":
        query_limit = None if args.limit is None else args.limit + 1
        runs = client.list_runs(
            args.project_id,
            status=args.status,
            limit=query_limit,
            offset=args.offset,
        )
        has_more = args.limit is not None and len(runs) > args.limit
        if has_more:
            runs = runs[: args.limit]
        return _render(
            ("RUN_ID", "PROJECT_ID", "NAME", "STATUS", "CREATED_AT"),
            [(item.run_id, item.project_id, item.name, item.status, item.created_at) for item in runs],
            args.format,
            kind="runs",
            page={
                "offset": args.offset,
                "limit": args.limit,
                "returned": len(runs),
                "has_more": has_more,
            },
        )
    if args.resource == "autoresearch":
        if args.action == "leaderboard":
            return _run_autoresearch_leaderboard(client, args)
        if args.action == "best":
            return _run_autoresearch_best(client, args)
        return _run_autoresearch_compare(client, args)
    if args.action == "list":
        metrics = client.list_metrics(args.run_id)
        return _render_summaries(
            metrics,
            include_metric_key=True,
            output_format=args.format,
            kind="metrics",
        )
    if args.action == "query":
        max_points = None if args.all else args.max_points
        meta = None
        with _enable_lttb_auto_install():
            if args.format == "json":
                points, source_row_count, downsampled = client._query_metric_with_metadata(
                    args.run_id,
                    args.metric_key,
                    start_step=args.start_step,
                    end_step=args.end_step,
                    max_points=max_points,
                )
                meta = {
                    "source_row_count": source_row_count,
                    "returned_row_count": len(points),
                    "downsampled": downsampled,
                }
            else:
                points = client.query_metric(
                    args.run_id,
                    args.metric_key,
                    start_step=args.start_step,
                    end_step=args.end_step,
                    max_points=max_points,
                )
        return _render(
            ("STEP", "VALUE", "TIMESTAMP"),
            [(item.step, item.value_f64, item.timestamp) for item in points],
            args.format,
            kind="metric_points",
            meta=meta,
        )
    candidate_run_ids = [run_id for run_id in args.run_ids if run_id != args.baseline]
    reports = client._comparison_reports(
        candidate_run_ids,
        args.baseline,
        metric_key=args.metric_key,
        direction=args.direction,
        secondary_metric_keys=args.secondary,
    )
    return _render_comparison_reports(reports, args.format, reference_role="baseline")


def _ranking_run_ids(client: _seex.Client, project_id: str, requested_run_ids: Sequence[str]) -> list[str]:
    if not requested_run_ids:
        return [run.run_id for run in client.list_runs(project_id)]
    client.get_project(project_id)
    run_ids: list[str] = []
    for run_id in requested_run_ids:
        run = client.get_run(run_id)
        if run.project_id != project_id:
            raise _CliRequestError(
                "run_project_mismatch",
                f"run {run_id} does not belong to project {project_id}",
            )
        run_ids.append(run_id)
    return run_ids


def _ranking_entry_document(entry: _seex.RankingEntry) -> dict[str, object]:
    return {"rank": entry.rank, "evidence": _evidence_document(entry.evidence)}


def _ranking_meta(project_id: str, result: _seex.RankingResult) -> dict[str, object]:
    return {
        "project_id": project_id,
        "objective": {
            "metric_key": result.objective.metric_key,
            "direction": result.objective.direction,
        },
        "total_entries": len(result.entries),
        "eligible_entries": sum(entry.rank is not None for entry in result.entries),
    }


def _ranking_rows(
    entries: Sequence[_seex.RankingEntry],
) -> list[tuple[object, ...]]:
    return [
        (
            entry.rank if entry.rank is not None else "-",
            entry.evidence.run_id,
            entry.evidence.run_status,
            entry.evidence.last_step,
            entry.evidence.last_value_f64,
            entry.evidence.completeness,
            ",".join(entry.evidence.reasons) or "-",
        )
        for entry in entries
    ]


def _run_autoresearch_leaderboard(client: _seex.Client, args: argparse.Namespace) -> str:
    run_ids = _ranking_run_ids(client, args.project_id, args.run_ids)
    result = client.rank_runs(run_ids, metric_key=args.metric, direction=args.direction)
    limit = None if args.all else args.limit
    stop = None if limit is None else args.offset + limit
    entries = result.entries[args.offset : stop]
    if args.format == "table":
        return _render_table(
            (
                "RANK",
                "RUN_ID",
                "STATUS",
                "LAST_STEP",
                "LAST_VALUE",
                "EVIDENCE",
                "REASONS",
            ),
            _ranking_rows(entries),
        )
    return _dump_json(
        {
            "schema_version": _JSON_SCHEMA_VERSION,
            "kind": "autoresearch_leaderboard",
            "data": [_ranking_entry_document(entry) for entry in entries],
            "page": {
                "offset": args.offset,
                "limit": limit,
                "returned": len(entries),
                "has_more": args.offset + len(entries) < len(result.entries),
            },
            "meta": _ranking_meta(args.project_id, result),
        }
    )


def _run_autoresearch_best(client: _seex.Client, args: argparse.Namespace) -> str:
    run_ids = _ranking_run_ids(client, args.project_id, args.run_ids)
    result = client.rank_runs(run_ids, metric_key=args.metric, direction=args.direction)
    best = next((entry for entry in result.entries if entry.rank == 1), None)
    if args.format == "table":
        if best is None:
            return "No eligible Run."
        return _render_table(
            (
                "RANK",
                "RUN_ID",
                "STATUS",
                "LAST_STEP",
                "LAST_VALUE",
                "EVIDENCE",
                "REASONS",
            ),
            _ranking_rows([best]),
        )
    return _dump_json(
        {
            "schema_version": _JSON_SCHEMA_VERSION,
            "kind": "autoresearch_best",
            "data": {"best": None if best is None else _ranking_entry_document(best)},
            "page": None,
            "meta": _ranking_meta(args.project_id, result),
        }
    )


def _run_autoresearch_compare(client: _seex.Client, args: argparse.Namespace) -> str:
    incumbent = args.against
    if incumbent is None:
        client.get_run(args.candidate_run_id)
        incumbent = client._best_eligible_run(
            args.comparator,
            metric_key=args.metric,
            direction=args.direction,
        )
        if incumbent is None:
            primary = client._objective_evidence(args.candidate_run_id, args.metric)
            secondary = [
                (
                    metric_key,
                    client._objective_evidence(args.candidate_run_id, metric_key),
                )
                for metric_key in args.secondary
            ]
            return _render_insufficient_comparison(
                primary,
                secondary,
                args.metric,
                args.direction,
                args.format,
            )
    reports = client._comparison_reports(
        [args.candidate_run_id],
        incumbent,
        metric_key=args.metric,
        direction=args.direction,
        secondary_metric_keys=args.secondary,
    )
    return _render_comparison_reports(reports, args.format, reference_role="incumbent")


def _unresolved_metric_document(
    metric_key: str,
    candidate: _seex.ObjectiveEvidence,
) -> dict[str, object]:
    return {
        "metric_key": metric_key,
        "candidate": _evidence_document(candidate),
        "reference": None,
        "completeness": "unavailable",
        "raw_delta": None,
        "relative_delta": None,
    }


def _render_insufficient_comparison(
    primary: _seex.ObjectiveEvidence,
    secondary: Sequence[tuple[str, _seex.ObjectiveEvidence]],
    metric_key: str,
    direction: str,
    output_format: str,
) -> str:
    reason = "no_eligible_incumbent"
    if output_format == "table":
        rows: list[tuple[object, ...]] = [
            (
                "primary",
                primary.run_id,
                "-",
                metric_key,
                primary.last_value_f64,
                primary.completeness,
                ",".join(primary.reasons) or "-",
                "unavailable",
                reason,
                "inconclusive",
            )
        ]
        rows.extend(
            (
                "secondary",
                evidence.run_id,
                "-",
                key,
                evidence.last_value_f64,
                evidence.completeness,
                ",".join(evidence.reasons) or "-",
                "unavailable",
                reason,
                "inconclusive",
            )
            for key, evidence in secondary
        )
        return _render_table(
            (
                "KIND",
                "CANDIDATE",
                "INCUMBENT",
                "METRIC",
                "CANDIDATE_LAST",
                "EVIDENCE",
                "EVIDENCE_REASONS",
                "COMPLETENESS",
                "REPORT_REASONS",
                "PREFERENCE",
            ),
            rows,
        )
    primary_document = _unresolved_metric_document(metric_key, primary)
    primary_document.update(
        {
            "direction": direction,
            "normalized_improvement": None,
            "outcome": None,
            "preference": "inconclusive",
        }
    )
    return _dump_json(
        {
            "schema_version": _JSON_SCHEMA_VERSION,
            "kind": "comparison_reports",
            "data": [
                {
                    "candidate_run_id": primary.run_id,
                    "primary": primary_document,
                    "secondary": [_unresolved_metric_document(key, evidence) for key, evidence in secondary],
                    "completeness": "unavailable",
                    "reasons": [reason],
                    "preference": "inconclusive",
                }
            ],
            "page": None,
            "meta": {"reference_role": "incumbent"},
        }
    )


def _evidence_document(evidence: _seex.ObjectiveEvidence) -> dict[str, object]:
    return {
        "run_id": evidence.run_id,
        "run_status": evidence.run_status,
        "last_step": evidence.last_step,
        "last_value": evidence.last_value_f64,
        "completeness": evidence.completeness,
        "reasons": evidence.reasons,
    }


def _primary_document(result: _seex.ComparisonResult) -> dict[str, object]:
    return {
        "metric_key": result.objective.metric_key,
        "direction": result.objective.direction,
        "candidate": _evidence_document(result.candidate),
        "reference": _evidence_document(result.reference),
        "completeness": result.completeness,
        "raw_delta": result.raw_delta,
        "relative_delta": result.relative_delta,
        "normalized_improvement": result.normalized_improvement,
        "outcome": result.outcome,
        "preference": result.preference,
    }


def _secondary_document(
    result: _seex._MetricComparisonResult,
) -> dict[str, object]:
    return {
        "metric_key": result.metric_key,
        "candidate": _evidence_document(result.candidate),
        "reference": _evidence_document(result.reference),
        "completeness": result.completeness,
        "raw_delta": result.raw_delta,
        "relative_delta": result.relative_delta,
    }


def _comparison_rows(
    report: _seex._ComparisonReport,
) -> list[tuple[object, ...]]:
    primary = report.primary
    primary_reasons = ",".join((*primary.candidate.reasons, *primary.reference.reasons))
    rows: list[tuple[object, ...]] = [
        (
            "primary",
            primary.candidate.run_id,
            primary.reference.run_id,
            primary.objective.metric_key,
            primary.candidate.last_value_f64,
            primary.reference.last_value_f64,
            primary.raw_delta,
            primary.relative_delta,
            primary.normalized_improvement,
            primary.completeness,
            primary_reasons or "-",
            primary.outcome,
            primary.preference,
        )
    ]
    rows.extend(
        (
            "secondary",
            item.candidate.run_id,
            item.reference.run_id,
            item.metric_key,
            item.candidate.last_value_f64,
            item.reference.last_value_f64,
            item.raw_delta,
            item.relative_delta,
            "-",
            item.completeness,
            ",".join((*item.candidate.reasons, *item.reference.reasons)) or "-",
            "-",
            "-",
        )
        for item in report.secondary
    )
    return rows


def _render_comparison_reports(
    reports: Sequence[_seex._ComparisonReport],
    output_format: str,
    *,
    reference_role: str,
) -> str:
    if output_format == "table":
        headers = (
            "KIND",
            "CANDIDATE",
            reference_role.upper(),
            "METRIC",
            "CANDIDATE_LAST",
            "REFERENCE_LAST",
            "RAW_DELTA",
            "RELATIVE_DELTA",
            "IMPROVEMENT",
            "COMPLETENESS",
            "REASONS",
            "OUTCOME",
            "PREFERENCE",
        )
        rows = [row for report in reports for row in _comparison_rows(report)]
        return _render_table(headers, rows)
    return _dump_json(
        {
            "schema_version": _JSON_SCHEMA_VERSION,
            "kind": "comparison_reports",
            "data": [
                {
                    "primary": _primary_document(report.primary),
                    "secondary": [_secondary_document(item) for item in report.secondary],
                }
                for report in reports
            ],
            "page": None,
            "meta": {"reference_role": reference_role},
        }
    )


def _render_summaries(
    summaries: Sequence[_seex.MetricSummary],
    *,
    include_metric_key: bool,
    output_format: str,
    kind: str,
) -> str:
    headers = ["RUN_ID"]
    if include_metric_key:
        headers.append("METRIC_KEY")
    headers.extend(("COUNT", "LAST_STEP", "LAST_VALUE", "MIN", "MAX"))
    rows: list[list[object]] = []
    for item in summaries:
        row: list[object] = [item.run_id]
        if include_metric_key:
            row.append(item.metric_key)
        row.extend(
            (
                item.effective_count,
                item.last_step,
                item.last_value_f64,
                item.min_value_f64,
                item.max_value_f64,
            )
        )
        rows.append(row)
    return _render(headers, rows, output_format, kind=kind)


def _resolve_cli_path(project_path: pathlib.Path, value: str | None) -> pathlib.Path | str | None:
    if value is None or "://" in value:
        return value
    path = pathlib.Path(value)
    return path if path.is_absolute() else project_path / path


def _sanitize_operation_message(
    message: str,
    project_path: pathlib.Path,
    args: argparse.Namespace,
) -> str:
    values = (
        project_path,
        _resolve_cli_path(project_path, args.catalog_path),
        _resolve_cli_path(project_path, args.data_path),
        os.environ.get("SEEX_LTTB_EXTENSION_PATH"),
    )
    local_paths: set[pathlib.Path] = set()
    for value in values:
        if value is None or "://" in str(value):
            continue
        path = pathlib.Path(value)
        local_paths.add(path if path.is_absolute() else project_path / path)
    for path in sorted(local_paths, key=lambda item: len(str(item)), reverse=True):
        message = message.replace(str(path), path.name or "storage path")
    return message


def _launch_app(project_path: pathlib.Path | None) -> int:
    binary = shutil.which(_APP_BINARY)
    if binary is None:
        print(
            "seex-app was not found on PATH; install the Seex desktop app and ensure its binary is available on PATH",
            file=sys.stderr,
        )
        return 1
    arguments = [binary]
    if project_path is not None:
        arguments.append(os.fspath(project_path))
    try:
        os.execv(binary, arguments)
    except OSError as error:
        print(f"failed to launch seex-app: {error}", file=sys.stderr)
        return 1


def main(argv: Sequence[str] | None = None) -> int:
    """Runs the Seex CLI and returns its process exit status."""
    args = _parse_args(argv)
    if args.resource == "app":
        return _launch_app(args.project_path)
    project_path = args.path.absolute()
    try:
        with _seex.init(
            project_path,
            data_path=_resolve_cli_path(project_path, args.data_path),
            catalog_backend=args.catalog_backend,
            catalog_path=_resolve_cli_path(project_path, args.catalog_path),
            _must_exist=True,
        ) as client:
            print(_run(client, args))
    except _CliRequestError as error:
        message = str(error)
        if args.format == "json":
            message = _render_error(error.code, message)
        print(message, file=sys.stderr)
        return 1
    except _seex.SeexError as error:
        message = _sanitize_operation_message(str(error), project_path, args)
        code, guidance = _operation_error_details(error)
        if args.format == "json":
            message = _render_error(code, message, guidance)
        print(message, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
