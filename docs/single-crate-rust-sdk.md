# Single-Crate Rust SDK and W&B-Compatible Python Surface

> Status: Accepted for `0.1.0-beta.1` / `0.1.0b1` by
> [ADR 0015](adr/0015-unified-rust-sdk-performance-preserving-migration.md).
> [ROADMAP.md](ROADMAP.md) is the authoritative implementation order and exit
> criteria; this document specifies the target API and architecture.

## Outcome and Constraints

Publish exactly one crates.io package named `seex` for Rust training workloads,
while the Python SDK and native Desktop consume the same implementation. Python
follows the modern W&B run-scoped workflow where Seex has equivalent semantics:

```python
with seex.init(project="demo", name="baseline") as run:
    run.log({"train/loss": loss})
```

This is not a W&B protocol implementation. Unsupported W&B arguments are not
accepted and ignored.

- Workspace-only packages use `publish = false`; PyO3, GPUI, and renderer types
  do not enter the published SDK.
- DuckLake remains required in native mode; the native storage and Parquet
  compatibility boundaries remain unchanged.
- The first Rust release changes neither catalog nor Parquet schema and adds no
  runtime dependency.
- Metric reporting remains a bounded, non-blocking training hot path.

## Target Workspace

```text
crates/
  seex/                 only package published to crates.io
    src/
      lib.rs            stable facade and common re-exports
      client.rs         Client, ClientBuilder, RunHandle
      reader.rs         Reader, ReaderBuilder
      error.rs          public Error and Result
      config.rs         public connection configuration
      model/            Project, Run, metric, comparison types
      engine/           lifecycle, queries, background reporting
      storage/          DuckDB, DuckLake, Parquet, SQL; private
  seex-python/          private PyO3 cdylib
  seex-app/             private GPUI application
  seex-plot/            private renderer-independent plot primitives
```

Inside `seex`, dependencies remain `model <- storage <- engine <- facade`.
Module visibility, tests, and review gates replace the current Cargo-enforced
boundary; this weaker mechanical isolation is the main cost of one published
package. `ProjectConnection`, `NativeQueryStore`, `StorageError`, and DuckDB
types become implementation details, and Desktop reads through `Reader`.

Rename `seex-chart-core` to `seex-plot` and `ChartError` to `PlotError` without
behavior changes. It continues to own series, axes, viewports, projection,
ticks, path caching, hit testing, brush, selection, and zoom. It stays separate
and unpublished so GPUI cannot enter its windowless geometry and interaction
tests.

## Rust Surface

```rust
let client = seex::Client::builder(".").open()?;
let run = client.start_run(
    seex::RunOptions::new("demo")
        .name("baseline")
        .resume(seex::ResumePolicy::Never),
)?;
run.log([("train/loss", loss)])?;
run.log_with(
    [("eval/accuracy", accuracy)],
    seex::LogOptions::new().step(step).commit(true),
)?;
run.finish()?;
client.shutdown()?;
```

The root exports `Client`, `ClientBuilder`, `Reader`, `ReaderBuilder`,
`RunHandle`, `RunOptions`, `LogOptions`, `ResumePolicy`, `Error`, `Result`, and
common model types.

- `RunHandle` is `Clone + Send + Sync`; clones share admission, step cursor,
  and terminal state. Matching terminal operations are idempotent; conflicting
  ones fail. Repeating the matching terminal operation retries incomplete
  drain or flush work, so the facade needs no separate public flush method.
  Drain and flush errors are explicit, not hidden in `Drop`.
- `Reader` opens an existing store without starting a writer and owns discovery,
  unified axis-aware metric queries, summaries, comparison, and ranking. Query
  callers choose the range and maximum returned point count; Reader does not
  accept pixels or derive a budget from a screen. A local-only constructor lets
  Desktop reject S3 before credential resolution.
- Public errors keep matchable configuration, queue, writer, drain, flush,
  closed-Run, step-regression, query, and sanitized storage variants without
  exposing DuckDB error types.

## Python Surface

```python
seex.init(
    *,
    project: str | None = None,
    dir: StrPath | None = None,
    id: str | None = None,
    name: str | None = None,
    resume: bool | Literal["allow", "never", "must"] | None = None,
    settings: seex.Settings | None = None,
) -> seex.Run
```

- `project` is a non-empty, stable Project identifier and default display name.
  It is preserved exactly, created when absent, and defaults to
  `"uncategorized"`; Seex does not silently slugify it.
- `id` is a globally unique Run identifier and defaults to a UUID. `name`
  defaults to that identifier.
- `resume=None`, `False`, or `"never"` creates; `True` or `"allow"` resumes a
  running Run or creates when absent; `"must"` requires an explicit existing,
  resumable ID. `"auto"` is unsupported.
- `Settings` owns catalog, data path, queue, and S3 options. Secrets are redacted
  from representations and errors.
- The first surface excludes `entity`, `config`, `tags`, `notes`, `group`,
  `job_type`, online `mode`, media, and artifacts; they need separate product
  and schema decisions.
- Normal context exit finishes the Run. Exceptional exit fails it, re-raises the
  original exception, and attaches any finalization failure as context.
- `run.finish(exit_code=None)` is the explicit terminal operation. `None` or
  zero finishes the Run and a non-zero value fails it. Repeating the same
  outcome retries incomplete finalization; a conflicting outcome fails.
- `run.diagnostics()` is an advanced Seex-only snapshot of the backing writer,
  not part of the W&B compatibility promise.

This is a beta API reset: `seex.init() -> Client` and
`run.log(key, step, value)` are removed in `0.1.0b1` without a compatibility
layer.

`seex.Api(dir=".", settings=None)` is a local read-only API modeled on
`wandb.Api`. It starts no writer and supports `close()` plus a context manager:

- `projects()`/`project()` and `runs()`/`run("project_id/run_id")` discover
  stored resources.
- `RunRecord` provides `metrics()`, `metric_summary()`, and one unified
  `history()` query.
- `Api.compare_runs()` and `Api.rank_runs()` expose current comparison and
  ranking semantics.

The metric-series query is intentionally smaller than W&B's row-oriented
history API:

```python
def history(
    self,
    metric_key: str,
    *,
    x_axis: Literal["step", "relative_time", "timestamp"] = "step",
    start: int | timedelta | datetime | None = None,
    end: int | timedelta | datetime | None = None,
    max_points: int | None = None,
) -> seex.MetricSeries
```

- Ranges are half-open. Step bounds are integers, relative-time bounds are
  `timedelta`, and timestamp bounds are timezone-aware `datetime` values;
  mismatched bound types fail rather than being coerced.
- `max_points=None` returns the full effective series. An integer of at least
  two is a strict caller-selected upper bound. Desktop may derive that number
  from its viewport, but no public Python or Rust SDK method accepts
  `pixel_width` or `points_per_pixel`.
- `MetricSeries` exposes points, source point count, downsampled state,
  completeness, and reasons, and implements the Arrow PyCapsule stream
  protocol. Separate `history_table`, `query_metric_table`, and
  `query_metric_summaries_table` methods are not carried forward.
- Relative time is derived from `Run.started_at`; missing starts, negative or
  decreasing axes, and non-finite values remain explicit evidence rather than
  being repaired. Storage may retain an internal boundary-neighbor option for
  Desktop line continuity, but it is not a second public query API and does not
  weaken the public `max_points` bound.
- W&B permits an arbitrary history metric as `x_axis`. Seex supports only the
  three built-in axes above; arbitrary cross-metric joins and W&B history-row
  reconstruction are unsupported.

The shipped Python `Client` is not retained as a public compatibility facade.
Its methods have these explicit dispositions:

| Shipped API | Target disposition |
| --- | --- |
| `create_project`, `create_run`, and `resume_run` | Replace with Project get-or-create and Run create/resume inside `seex.init`. |
| `finish_run` and `fail_run` | Replace with Run context management and `Run.finish(exit_code)`. |
| `get_project`, `list_projects`, `get_run`, and `list_runs` | Move resource discovery to `Api`. |
| `query_metric` and `query_aligned_metric` | Consolidate into `RunRecord.history` and Rust `Reader::query_metric`; keep specialized alignment only as an internal implementation detail. |
| `query_metric_table` and `query_metric_summaries_table` | Remove; result objects provide Arrow PyCapsule streaming. |
| `query_metric_summaries` | Keep internal for `metric_summary`, comparison, and ranking. |
| `list_metrics`, `compare_runs`, and `rank_runs` | Move to `RunRecord.metrics` or `Api` according to resource scope. |
| `list_orphan_runs` | Move to CLI/admin recovery rather than the ordinary SDK. |
| `flush_run_data` | Remove from the target facade; retry the matching terminal operation after a finalization error. |
| `diagnostics` | Keep as advanced `Run.diagnostics()` and Rust client diagnostics, outside the W&B compatibility promise. |

The CLI consumes this public API instead of private PyO3 underscore methods.

## W&B API Compatibility Matrix

This comparison is a design aid, not a claim of W&B protocol, backend, storage,
or ecosystem compatibility. **Shipped** means the API exists in the current
Seex Python package; **target** means it is accepted for this beta but not yet
implemented;
**partial** means the workflow is recognizable but its accepted inputs,
effects, or lifecycle differ; **unsupported** means Seex rejects or does not
expose the capability; and **Seex-only** means W&B has no direct equivalent.
Rust entries are native counterparts over the shared Seex engine, not
compatibility with a W&B Rust SDK.

### Initialization and Run lifecycle

| Capability | W&B Python | Seex shipped today | Seex target Python / Rust | Compatibility and difference |
| --- | --- | --- | --- | --- |
| Initialize a training Run | `wandb.init(...)` returns a Run or `None` | `seex.init(path, storage options...) -> Client` | Python: `seex.init(project=..., ...) -> Run`; Rust: `Client::builder(...).open()` then `client.start_run(RunOptions)` | **Partial.** The target Python entry point matches the run-scoped workflow, but it opens Seex native storage rather than a W&B session. Changing `init()` from `Client` to `Run` is a breaking change. |
| Context manager | `with wandb.init() as run` finishes the Run on exit | `with seex.init() as client` shuts down a `Client`; a `Run` is not a context manager | Python Run context finishes normally and fails on an exception; Rust uses explicit terminal methods | **Partial.** The target Python object and normal-exit behavior align; exceptional exit preserves the original exception and may attach finalization failure as its context. |
| Finish or fail a Run | `run.finish(exit_code=...)` / `wandb.finish(...)` | `client.finish_run(run_id)` and `client.fail_run(run_id)` | Python: `run.finish(exit_code=None)`; Rust: `run.finish()` | **Partial.** Current Seex termination is client-scoped; the target is Run-scoped. Matching retries incomplete finalization, and Seex exposes typed drain and flush failures. |
| Resume by Run ID | `id=...` with `resume` set to `"allow"`, `"must"`, `"never"`, `"auto"`, or a Boolean compatibility form | `client.resume_run(run_id)` is a separate operation | Python: `id=...`, with `None`, Boolean, `"allow"`, `"never"`, or `"must"`; Rust: `RunOptions::resume(ResumePolicy)` | **Partial.** Seex does not support `"auto"`; `"must"` requires an explicit existing resumable ID. |
| Client/process shutdown | SDK-managed process lifecycle plus `wandb.finish()` for the active Run | `client.shutdown(timeout)` drains writers and closes the client | No target Python global shutdown API is specified; Rust: `client.shutdown()` | **Partial.** Explicit client shutdown remains a native Rust/storage concern rather than a W&B-compatible Python global. |

### Run identity and metadata

| Capability | W&B Python | Seex shipped today | Seex target Python / Rust | Compatibility and difference |
| --- | --- | --- | --- | --- |
| Project | `project=` selects a project under an entity | Explicit `create_project`, `get_project`, and `list_projects`; `create_run` requires `project_id` | Python `project=` defaults to `"uncategorized"`; Rust `RunOptions::new(project)` | **Partial.** Seex uses a non-empty stable Project identifier and does not silently slugify it; it has no entity namespace. |
| Run ID and display name | `id=` identifies a Run; `name=` is its display name | `create_run(project_id, name, run_id=None)` | Python `id=` defaults to a UUID and `name=` defaults to that ID; equivalent Rust options | **Partial.** The concepts align, but Seex IDs address its native catalog rather than W&B cloud paths. |
| Entity/team namespace | `entity=` selects a user or team namespace | No equivalent | No equivalent | **Unsupported.** Seex has Projects but no cloud entity/workspace hierarchy. |
| Config, tags, notes, group, job type | `config=`, `tags=`, `notes=`, `group=`, and `job_type=` plus mutable Run metadata | No equivalent catalog fields or public methods | Explicitly excluded from the first surface | **Unsupported.** Adding them requires separate product and schema decisions. |
| Execution mode | `mode=` selects `"online"`, `"offline"`, `"disabled"`, or `"shared"`, with related Settings | Native storage is configured directly; there is no W&B mode | No `mode=`; `seex.Settings` owns catalog, data path, queue, and S3 options | **Partial.** Seex is local/native even when its Parquet data path is S3-compatible; this is not W&B offline mode or later cloud sync. |
| Settings object | `wandb.Settings` configures W&B SDK behavior | Keyword storage options on `seex.init` and `.seex/config.toml` | `seex.Settings` for native storage and queue configuration | **Partial.** The class name and role are similar, but fields, precedence, and effects are intentionally Seex-specific. |

### Metric logging

| Capability | W&B Python | Seex shipped today | Seex target Python / Rust | Compatibility and difference |
| --- | --- | --- | --- | --- |
| Log metrics | `run.log(data: dict[str, Any], step=None, commit=None)` | `run.log(key: str, step: int, value: float)` | Python: `run.log(data, step=None, commit=None)`; Rust: `run.log(...)` and `run.log_with(..., LogOptions)` | **Partial.** The target Python call shape aligns. Replacing the shipped three-positional-argument form is a breaking change. |
| Accepted values | Scalars, nested values, media, histograms, tables, and other W&B data types subject to SDK rules | One numeric metric per call | A non-empty Mapping of at most 8,192 integer or floating-point metrics; Booleans and non-numeric values fail | **Partial.** Scalar numeric logging overlaps; rich W&B value types remain unsupported. |
| Step and commit | Explicit or implicit step; `commit` controls accumulation into a W&B history row | Step is required; no commit cursor | Implicit step starts at zero; explicit-step `commit` defaults false, implicit-step `commit` defaults true; committed steps cannot regress | **Partial.** `commit=false` admits Seex metric points immediately and does not stage or construct a W&B history row. |
| Multi-metric atomicity | A Mapping contributes values to W&B history-row semantics | Each call contains one point | One Mapping shares an observation timestamp and is admitted atomically; failure writes no subset or cursor advance | **Partial.** Seex atomicity is queue admission for metric points, not W&B row transaction compatibility. |
| Asynchronous reporting | SDK buffers and transmits or stores data according to W&B mode and service state | Bounded non-blocking native queue with diagnostics and typed queue/writer/drain/flush errors | Same bounded engine contract; Python exposes advanced `run.diagnostics()` and Rust exposes client diagnostics | **Seex-specific behavior.** A successful `log` means admitted, not durably persisted; queue capacity counts metric points. |
| Summary and metric definitions | Automatic/manual `run.summary` plus `wandb.define_metric(...)` summary and step policies | Terminal metric summaries are query results only | Read-only summaries are planned; no mutable summary or `define_metric` API | **Unsupported** for W&B-compatible mutation. Seex derives effective numeric summaries and uses its own comparison semantics. |

### Read and query APIs

| Capability | W&B Python | Seex shipped today | Seex target Python / Rust | Compatibility and difference |
| --- | --- | --- | --- | --- |
| Open a read API | `wandb.Api(...)` connects to the W&B public API | Reads are methods on the writer-capable `Client` | Python: local read-only `seex.Api(dir=".", settings=None)`; Rust: `Reader::builder(...).open()` | **Partial.** The role aligns, but Seex starts no writer, reads native stores, and performs no W&B network/API calls. |
| Discover Projects | `api.projects(entity=...)` and `api.project(name, entity=...)` | `client.list_projects()` and `client.get_project(project_id)` | `api.projects()` and `api.project()`; equivalent `Reader` operations | **Partial.** Seex has stable local catalog order and no entity path. |
| Discover Runs | `api.runs(path, filters, order, ...)` and `api.run("entity/project/run")` | `client.list_runs(project_id, status, limit, offset)` and `client.get_run(run_id)` | `api.runs()` and `api.run("project_id/run_id")`; equivalent `Reader` operations | **Partial.** Seex paths omit entity and do not promise W&B filter expressions or ordering syntax. |
| Sampled history | Public `Run.history(samples=500, keys=None, x_axis="_step", ...)` returns sampled history, commonly as a pandas DataFrame | `query_metric(...)` returns typed points for one metric and optional downsampling | `RunRecord.history(metric_key, x_axis=..., max_points=...) -> MetricSeries`; Rust uses the same query contract through `Reader` | **Partial.** The caller controls the point bound and axis, but Seex returns one local numeric series and does not promise a DataFrame, system metrics, arbitrary metric x-axes, or row reconstruction. |
| Full history scan | Public `Run.scan_history(...)` iterates unsampled history rows | Repeated per-metric queries only | No separate `scan_history` method is proposed | **Unsupported** as a W&B-compatible row iterator; Seex exposes metric-series queries instead. |
| Axis-aware metric series | W&B `history(x_axis=...)` samples a history row model and permits a metric key as the axis | `list_metrics()` plus separate `query_metric(...)` and `query_aligned_metric(...)` methods | `RunRecord.metrics()` plus unified `history()` over step, relative time, or timestamp | **Partial.** Seex removes the separate aligned API. It preserves completeness diagnostics, while callers—not SDK screen logic—choose a strict `max_points`. |
| Comparison and ranking | Can be assembled with queries, reports, or other W&B products; no matching core `Api` methods | `compare_runs(...)` and `rank_runs(...)` on `Client` | `Api.compare_runs()` / `rank_runs()` and Rust `Reader` equivalents | **Seex-only.** These use Seex objective, completeness, comparison, and competition-ranking contracts. |

### Capabilities intentionally not mirrored

| Direction | Capability family | Status and boundary |
| --- | --- | --- |
| W&B only | Mutable Run config and summary, tags/notes/groups/jobs, and `define_metric` | **Unsupported.** They require metadata and schema contracts that this release does not introduce. |
| W&B only | Media, `wandb.Table`, plots, histograms, and rich typed values | **Unsupported.** The first Seex logging surface accepts only numeric metric points. |
| W&B only | Artifacts, files, registries, lineage, and `use_artifact` / `log_artifact` | **Unsupported.** Model checkpoints and artifact durability are outside the metric Parquet boundary. |
| W&B only | Sweeps, agents, launch jobs, alerts, reports, automations, integrations, and model `watch` / `unwatch` | **Unsupported.** Seex does not emulate the W&B cloud orchestration or collaboration ecosystem. |
| Seex only | DuckLake catalog plus local or S3-compatible Parquet data paths | **Shipped storage behavior; proposed facade.** The Parquet schema remains the product compatibility boundary. |
| Seex only | Writer diagnostics and explicit drain/flush outcomes | **Shipped** on `Client`; target Python exposes advanced Run diagnostics and retries finalization through the matching terminal operation. Orphan discovery moves to CLI/admin recovery. |
| Seex only | Arrow PyCapsule metric results | **Shipped** through separate table methods; proposed `MetricSeries` implements the protocol directly so duplicate table query methods disappear. |

Snapshot: W&B Python SDK
[`0.28.1`](https://pypi.org/project/wandb/0.28.1/) and the official W&B
references for [`wandb.init`](https://docs.wandb.ai/ref/python/init/),
the training [`Run`](https://docs.wandb.ai/ref/python/experiments/run/),
the public [`Api`](https://docs.wandb.ai/ref/python/public-api/api/), and the
public read-only [`Run`](https://docs.wandb.ai/ref/python/public-api/run/),
checked on 2026-07-30. Re-check the SDK version and these references whenever
this design's compatibility claims change.

## W&B-Compatible Log Semantics

Python exposes `run.log(data, step=None, commit=None)`; Rust exposes `log` and
`log_with` over the same engine contract.

- `data` is a non-empty Mapping from metric keys to integer or floating-point
  values. Booleans and non-numeric objects fail; existing non-finite evidence
  behavior is preserved.
- A new Run's implicit step starts at zero. Resume reads the greatest persisted
  step once during startup and continues from its successor.
- Implicit-step `commit` defaults true; explicit-step `commit` defaults false.
  False keeps the Run cursor, true advances it, and steps cannot move backward.
- `commit=false` still admits metrics immediately. Seex preserves same-step
  series semantics but does not stage a W&B history row.
- One Mapping shares an observation timestamp and is admitted atomically.
  Failure writes no subset and does not advance the cursor.
- Queue capacity and pending/persisted diagnostics count metric points;
  queue-full diagnostics count failed calls. One call contains at most 8,192
  metrics and cannot exceed total capacity.

The cursor reuses the Run admission lock. Unlike the removed auto-step design,
it performs no hot-path storage query and does not assign per-metric cursors.

## Migration Sequence

1. Freeze the unified reporting, query, and Viewer original and rolling
   baselines.
2. Add an unpublished `crates/seex` facade and Reader over the current crates;
   migrate Desktop reads without changing the public Python API.
3. Move model and storage into `seex`, keeping temporary unpublished re-export
   packages. Keep mechanical moves separate from narrow-query optimization.
4. Move the engine, add atomic metric batches, the Run cursor, and Rust
   `RunHandle`.
5. Add the Python Run, Settings, and Api surfaces. Consolidate metric reads
   behind the axis-aware query and remove public aligned/table variants only
   after Desktop, CLI, and PyO3 use the replacement.
6. Remove temporary packages, rename `seex-chart-core` to private `seex-plot`,
   finish packaging, and pass the shared release gates.

Every step leaves the workspace buildable and tested. Source moves use Git
renames so review focuses on boundary changes. Migration candidates preserve
reliable metrics within 3%; optimization candidates additionally satisfy the
paired A/B improvement contract in ADR 0015 and ROADMAP U0.

## Verification and Release Gates

- Run Rust format, workspace Clippy with warnings denied, tests, doc tests, and
  warning-free `cargo doc -p seex --no-deps`.
- Verify `seex-plot` has no GPUI, storage, or SDK dependency.
- Run `cargo package -p seex` and build the package independently; it contains
  no path dependency, PyO3, Desktop, or Plot source.
- Run Python format/lint, Pyright, pytest, wheel/sdist smoke, CLI JSON, catalog
  parity, and MinIO/S3 acceptance.
- Cover Project get-or-create races, resume policies, context outcomes, cloned
  RunHandle races, cursor/commit behavior, atomic queue failure, finalization
  barriers, and Reader/Desktop parity.
- Cover step, relative-time, and timestamp ranges; strict caller-selected point
  limits; full-series queries; invalid bound types; missing or non-monotonic
  time evidence; Arrow streaming from `MetricSeries`; and Desktop boundary
  continuity without pixels in the public SDK contract.
- Benchmark explicit step, implicit single-metric, and multi-metric Mapping.
  Implicit logging stays above 100,000 calls/s and its five-run median is at
  least 90% of explicit-step throughput on the same host; persistence loses no
  report and admits no partial Mapping.

Cargo `0.1.0-beta.1`, PyPI `0.1.0b1`, and tag `v0.1.0-beta.1` identify the same
source; release publishes crates.io before PyPI. Initial crates.io publication
uses a one-time token in the protected `release` environment, then switches to
trusted OIDC publishing and removes the token. If `seex` cannot be claimed,
stop and revisit naming rather than silently using a fallback.
