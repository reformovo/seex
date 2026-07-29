# Single-Crate Rust SDK and W&B-Compatible Python Surface

> Status: exploratory design, not an accepted architecture decision or roadmap
> commitment. Promotion requires an ADR, a roadmap phase, and explicit scope
> expansion for the workspace migration.

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

## Proposed Workspace

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
  ones fail. Drain and flush errors are explicit, not hidden in `Drop`.
- `Reader` opens an existing store without starting a writer and owns discovery,
  metric/aligned queries, summaries, comparison, and ranking. A local-only
  constructor lets Desktop reject S3 before credential resolution.
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

This is a beta API reset: `seex.init() -> Client` and
`run.log(key, step, value)` are removed in `0.1.0b1` without a compatibility
layer.

`seex.Api(dir=".", settings=None)` is a local read-only API modeled on
`wandb.Api`. It starts no writer and supports `close()` plus a context manager:

- `projects()`/`project()` and `runs()`/`run("project_id/run_id")` discover
  stored resources.
- `RunRecord` provides `metrics()`, `history()`, `history_table()`, and
  `aligned_history()`.
- `Api.compare_runs()` and `Api.rank_runs()` expose current comparison and
  ranking semantics.

The CLI consumes this public API instead of private PyO3 underscore methods.

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

1. Promote the decision through an ADR, Context updates, crate-boundary docs,
   and a committed roadmap phase.
2. Move model, storage, and core into `crates/seex`; leave temporary unpublished
   re-export packages so consumers remain buildable.
3. Rename `seex-chart-core` to `seex-plot` as a behavior-preserving change.
4. Add the Rust facade and Reader, migrate Desktop and PyO3, then remove the
   temporary packages and internal re-exports.
5. Add atomic metric batches, the Run cursor, Rust RunHandle, and the new Python
   Run, Settings, and Api surfaces.
6. Complete packaging, documentation, release automation, and the shared
   `0.1.0-beta.1` / `0.1.0b1` release.

Every step leaves the workspace buildable and tested. Source moves use Git
renames so review focuses on boundary changes.

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
- Benchmark explicit step, implicit single-metric, and multi-metric Mapping.
  Implicit logging stays above 100,000 calls/s and its five-run median is at
  least 90% of explicit-step throughput on the same host; persistence loses no
  report and admits no partial Mapping.

Cargo `0.1.0-beta.1`, PyPI `0.1.0b1`, and tag `v0.1.0-beta.1` identify the same
source; release publishes crates.io before PyPI. Initial crates.io publication
uses a one-time token in the protected `release` environment, then switches to
trusted OIDC publishing and removes the token. If `seex` cannot be claimed,
stop and revisit naming rather than silently using a fallback.
