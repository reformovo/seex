# Seex Roadmap

> This file contains only current and future work. Completed 0.1.x phases and
> their measured results are summarized as release history in
> [`release-notes/0.1.0b0.md`](release-notes/0.1.0b0.md#completed-01x-roadmap).
> Shipped release details and completed roadmap history live together in
> `release-notes/`.

Pre-1.0 releases do not promise store, API, or machine-output compatibility.
The next coordinated milestone is Cargo `0.1.0-beta.1`, Python `0.1.0b1`, and
tag `v0.1.0-beta.1`, all built from the same source.

## 0.1.0 Beta / Unified SDK and Bounded Native Queries

This milestone implements the accepted
[single-crate SDK](single-crate-rust-sdk.md), the accepted
[Viewer configuration and workbench design](drafts/viewer-configuration-and-workbench-state.md),
and the outstanding Viewer query, RSS, and Metal work.
[ADR 0015](adr/0015-unified-rust-sdk-performance-preserving-migration.md)
defines the SDK architecture; this roadmap defines implementation order and
exit criteria.

The order is fixed:

```text
U0 gates -> U1 configuration + Reader -> U2 storage/query
         -> U3 reporting/Run SDK -> U4 Python API
         -> U5 crate/package convergence
         -> U6 Viewer configuration/workbench -> U7 release gates
```

No later phase may remove a boundary or baseline needed by an earlier phase.
U1 may begin only after U0 freezes the original baseline. Physical source moves
begin only after Desktop uses Reader.

### Execution Contract

- [ ] Before every candidate, record its type, primary metric, protected
  metrics, fixture, commands, and no more than five files. Target no more than
  200 changed lines; mechanical source moves and this roadmap archive are the
  declared exceptions.
- [ ] Keep mechanical migration and performance optimization in separate
  candidates. Use Git renames for source moves and leave the workspace buildable
  after every accepted candidate.
- [ ] Accept a migration candidate only when correctness and hard floors pass
  and every reliable protected metric regresses by no more than 3%. It need not
  improve performance.
- [ ] Accept an optimization candidate only when correctness and hard floors
  pass; at least 6 of 7 alternating baseline/candidate pairs improve; the
  primary median improves by at least 5%; protected medians regress by no more
  than 3%; and each deciding metric has relative MAD no greater than 2%.
- [ ] Treat an improvement below 5%, an unreliable metric, or inconsistent
  pairs as no change. Reject correctness, schema, API parity, and hard-gate
  failures immediately.
- [ ] Revert only the rejected candidate's declared hunks. Do not use
  `git reset` or `git checkout`, and do not touch user or accepted changes.
  Profile a no-change or regression with DuckDB plans, RSS, `heap`/`vmmap`,
  allocation stacks, or Metal evidence before opening another candidate.
- [ ] Preserve the catalog and Parquet schemas, DuckLake as a required native
  dependency, four-way Viewer read concurrency, and the 100 ms trailing
  debounce. Add no runtime dependency without separate approval.
- [ ] The direct `toml_edit` runtime dependency is approved only for the U1
  syntax-preserving configuration candidate. Review that dependency change
  separately from document codecs, Source management, and SDK migration.

### U0: Unified Performance Gates and Migration Baseline

U0 generalizes the existing Viewer performance work into one migration gate.
It changes no production behavior and requires no performance improvement.

#### Gate format and comparator

- [x] Define version 2 performance JSON with `reporting`, `query`, and `viewer`
  domains. Record environment, commit and dirty-worktree identity, fixture,
  commands, units, batch size, raw samples, MAD, p50, p95, max, RSS phases, and
  original/rolling comparison results.
- [x] Implement the typed collector and comparator with the standard libraries
  already available to the repository. Keep raw traces and per-iteration trace
  artifacts outside the repository; commit only stable statistics and human
  conclusions.
- [x] Calibrate each timing batch until one sample lasts at least 10 ms. Mark a
  metric with relative MAD above 2% as non-deciding; it cannot accept a
  migration or optimization candidate.
- [x] Encode migration and optimization policies from the Execution Contract,
  including hard floors, Pass, No-change, and Regression outcomes.
- [x] Keep resource counters behind test-support/release test configuration and
  prove an ordinary release build contains no counter state or production log.

#### Workloads

- [x] Cover reporting at the Rust engine and Python/PyO3 boundaries: explicit
  step, implicit single metric, multi-metric Mapping, queue admission,
  drain/persistence, finalization, and peak RSS.
- [x] Cover Reader queries on DuckDB and SQLite with 10 Runs and 1,000,000
  points per series: full and narrow ranges, Step/relative-time/timestamp axes,
  neighbors, duplicates, spikes, last-write-wins, completeness, and reasons.
- [x] Use identical read-only fixtures, independent release binaries, and
  alternating execution order for baseline and candidate query samples.
- [x] Cover Viewer with 10 Runs and at least six visible Metrics: single and
  dual View, sparse/dense windows, 1x/2x/3x, 30 zoom cycles, stale generation,
  query concurrency, snapshot/path counters, CPU, RSS, and Metal trace metadata.
  The deciding 280 Hz trace uses the reset 10-Run/six-Metric workbench and
  user-performed zoom; the failed automated interaction is not baseline evidence.
- [x] Run automated RSS workloads in a fresh process. Record warm, peak, final,
  phase trend, and retained stale-snapshot counts from an external sampler.

#### Baseline freeze and exit

- [x] Freeze the pre-migration worktree as the permanent original baseline and
  first rolling baseline. Preserve the current accepted logical-budget,
  compact-snapshot, bounded-geometry, and shared-DuckDB improvements. The
  corrected U0 code baseline is `e1f248fbb66eed7c49d36ef393e263f6c72df721`.
- [x] Record exact machine, OS, Rust toolchain, display/scale, fixture identity,
  sample count, commands, and known environmental blockers in the performance
  validation document.
- [x] Verify gate instrumentation does not change ordinary release API,
  behavior, binary dependencies, or reliable CPU results by more than 3%.
- [x] Exit U0 only when all three domains can compare a candidate against both
  baselines and reproduce correctness and resource results.

### U1: Configuration Foundation, Public `seex` Facade, and Reader First

U1 establishes the configuration and identity contract before Reader migration.
Configuration, document codecs, the facade, and Reader migration remain
separate candidates. The configuration work does not change public Rust or
Python APIs; the facade and Reader add only the already-planned Rust surface,
and the shipped Python API remains unchanged until U4.

#### U1.1: Configuration and document foundation

- [ ] Define schema version 1 for global `~/.seex/config.toml` and project
  `<root>/.seex/config.toml`. Resolve explicit SDK arguments, project config,
  global config, and built-in defaults in that order; merge S3 tables field by
  field and ignore inherited credentials when effective `data_path` is local.
- [ ] Keep field-level ownership explicit: the SDK reads storage and S3 keys;
  Desktop reads and edits Sources. Share contract fixtures without exposing
  Desktop configuration types through the public SDK.
- [ ] Introduce stable Source aliases and represent Viewer Project and Run
  references by alias rather than path. Validate alias conflicts and Source
  Project allowlists, and load Runs only for allowed Projects.
- [ ] Implement schema version 1 TOML workbench encoding, decoding, and
  validation. Reject the legacy `seex-workbench 1` format without migration.
- [ ] Use the approved direct `toml_edit` dependency for Desktop
  read-modify-write. Preserve comments, unknown fields, native storage fields,
  and secrets; require owner-only permissions for global secrets; write through
  a same-directory temporary file and reject a stale read fingerprint.
- [ ] Prove SDK `init`, `log`, `finish`, and `shutdown` may read effective
  configuration but never rewrite either config or workbench file.

#### U1.2: Facade and Reader migration

- [ ] Create unpublished `crates/seex` as a facade over the current
  `seex-model`, `seex-storage`, and `seex-core` crates. Keep every workspace
  target buildable.
- [ ] Define public `Reader`/`ReaderBuilder`, `MetricAxis`, typed half-open
  ranges, strict caller-selected `max_points`, and `MetricSeries`.
- [ ] Keep pixels out of public Rust and Python queries. Desktop alone converts
  a closed viewport into crate-private options for one real neighbor on each
  side, without weakening the public point bound.
- [ ] Make `MetricSeries` retain real samples, source count, downsampled state,
  completeness, and reasons, and expose an Arrow PyCapsule stream directly.
- [ ] Keep `ProjectConnection`, `ProjectMetricReader`, `NativeQueryStore`,
  storage errors, DuckDB types, and local-only source policy private.
- [ ] Migrate Desktop discovery and curve reads to Reader over configured
  Source aliases and Project allowlists. Preserve four-way scheduling,
  generation reconciliation, hover/locked-cursor real-sample semantics, and
  source-specific failures.
- [ ] Route PyO3 through a temporary compatibility adapter without changing the
  shipped Python surface; defer the breaking public API switch to U4.
- [ ] Cover configuration layering, path bases, schema and alias conflicts,
  allowlists, TOML round trips, source preservation, concurrent edits, and
  byte-for-byte SDK non-mutation, plus Reader/native/standalone parity, all
  axes and range types, strict bounds, Desktop neighbors, missing metadata,
  and Arrow output.
- [ ] Exit U1 only when configuration and Reader correctness pass and query,
  reporting, Viewer CPU, and RSS protected metrics are reliable and regress by
  no more than 3% from the rolling baseline.

### U2: Storage Migration and Query Peak Reduction

Profiling identifies DuckDB window/sort materialization and allocator churn as
the remaining peak-memory path. Full-span and narrow queries therefore use
different execution plans; one universal SQL plan is not a goal.

#### U2.1: Mechanical storage move

- [ ] Move model, storage, and query implementation into private
  `seex::{model, storage}` modules with Git renames. Leave the old unpublished
  crates as re-exports until all consumers migrate.
- [ ] Run the migration gate without SQL, reduction, allocation, schema, or
  behavior changes. Reject reliable protected-metric regression above 3%.

#### U2.2: Split query plans

- [ ] Preserve the incumbent Overview/full-span SQL and its materialized
  ordered plan. Protect its reliable latency and RSS metrics from regression
  above 3%.
- [ ] Add a narrow Step plan that filters the viewport before expensive
  materialization and bucket windows while retaining every same-step
  replacement until last-write-wins.
- [ ] Return one real effective neighbor on each side and reduce only the
  cropped effective rows. Preserve duplicates, spikes, strict point budgets,
  evidence reasons, and DuckDB/SQLite parity.
- [ ] Initially keep relative-time and timestamp queries on the incumbent plan;
  a replacement may change its timestamp, so time filtering cannot precede
  last-write-wins without a separate proof.

#### U2.3: Diagnostics and cancellation

- [ ] Separate whole-series negative/decreasing/completeness diagnostics from
  viewport selection. Finished Runs cache by source/project/run/metric;
  Running Runs invalidate on refresh or storage-generation change.
- [ ] Propagate diagnostics failure as incomplete evidence with an explicit
  reason. Never manufacture complete evidence or silently repair an axis.
- [ ] Give every cloned DuckDB connection an interrupt handle and request token.
  A superseded generation interrupts only its current request; interruption is
  stale cancellation and never becomes an error snapshot.
- [ ] Check supersession before query execution and between Runs. Stale results
  must not enter a merged snapshot or retain their query working set.

#### U2.4: Evidence-driven fallbacks

- [ ] If narrow Step SQL improves real-workload RSS by less than 5%, profile it
  and test a separate narrow-only bounded reducer: DuckDB performs early
  filtering, last-write-wins, and one Step order; Rust retains only
  first/last/min/max candidates for each bucket.
- [ ] If the bounded reducer still misses the RSS target, profile Parquet
  physical ordering and row-group sizing as the next independent candidate.
  Preserve the Parquet schema and partition contract.
- [ ] Do not retry generic `HASH_GROUP_BY` extrema, memory limits, DuckDB thread
  caps, allocator relief scheduling, or removal of the beneficial full-span
  ordered window without new contradictory profile evidence.

#### U2 exit gates

- [ ] Pass full/narrow correctness, DuckDB/SQLite parity, last-write-wins,
  neighbors, diagnostics, and nearest-real-sample hover tests.
- [ ] Keep every reliable full-query protected metric within 3% of the rolling
  baseline. Improve the narrow primary metric by at least 5% with 6 of 7 pairs.
- [ ] Reduce peak RSS for the frozen single- and dual-View real workload by at
  least 25% from the U0 original baseline.
- [ ] Prove superseded queries merge no snapshot and retain no working set.

### U3: Engine Migration and Rust Run SDK

Mechanical engine movement and reporting behavior changes are separate
candidates.

- [ ] Move lifecycle, queue, writer, diagnostics, comparison, and ranking into
  private `seex::engine`; keep `seex-core` as an unpublished re-export until
  consumers migrate. Pass the migration gate within 3%.
- [ ] Implement public `Client`/`ClientBuilder`, `RunHandle`, `RunOptions`,
  `LogOptions`, `ResumePolicy`, and matchable `Error`/`Result`.
- [ ] Make `RunHandle: Clone + Send + Sync`; clones share one admission lock,
  step cursor, queue, and terminal state.
- [ ] Admit a non-empty Mapping atomically with at most 8,192 numeric metrics.
  Failure admits no subset and does not advance the cursor; queue capacity and
  diagnostics count points consistently.
- [ ] Default explicit-step `commit` to false and implicit-step `commit` to
  true. Start a new Run at step zero, resume from the greatest persisted step,
  and reject committed-step regression.
- [ ] Make matching terminal operations retry incomplete drain/flush work.
  Return a typed error for a conflicting terminal outcome and never hide a
  finalization error in `Drop`.
- [ ] Cover Project get-or-create races, resume modes, cloned-handle races,
  atomic queue failure, cursor/commit cases, finalization barriers, and
  persistence without lost reports or partial Mappings.
- [ ] Require explicit single-metric admission of at least 100,000 calls/s.
  Across five runs, implicit single-metric median throughput must be at least
  90% of explicit throughput; multi-metric throughput is counted per point.
- [ ] Exit U3 only when reporting p50/p95, drain latency, persistence, and peak
  RSS meet hard floors and regress by no more than 3% during migration.

### U4: Python Run, Api, CLI, and Arrow Surface

U4 is the planned beta API reset. It does not retain the shipped public Client
as a compatibility layer.

- [ ] Implement typed `seex.init(...) -> seex.Run` with the accepted
  project/id/name/resume/settings semantics and secret-redacted configuration.
- [ ] Implement Mapping `Run.log`, context management, `finish(exit_code)`, and
  advanced diagnostics over the Rust Run SDK. Preserve an original context
  exception and attach finalization failure as context.
- [ ] Add read-only `seex.Api`, `RunRecord.history()`, metrics, summary,
  comparison, and ranking over Reader. Start no writer for read-only use.
- [ ] Limit `history()` axes to step, relative time, and timestamp. Require
  matching typed bounds and a caller-selected strict `max_points`; reject
  coercion and arbitrary metric x-axis joins.
- [ ] Make Python `MetricSeries` implement Arrow PyCapsule streaming directly.
  Remove separate public table-query methods after every consumer migrates.
- [ ] Move the CLI to public Rust/Python facades and remove calls to private
  underscore PyO3 APIs. Preserve deterministic versioned JSON contracts.
- [ ] Update Python type stubs and cover init/resume, Mapping validation,
  context outcomes, Api discovery, range errors, evidence, Arrow, Reader parity,
  CLI JSON, and packaging smoke tests.
- [ ] Pass Python formatting/lint, Pyright, pytest, Rust/PyO3 parity, wheel and
  sdist smoke tests, and the reporting/query migration gates.

### U5: Single-Crate Convergence and Packaging

- [ ] After every consumer migrates, delete the temporary `seex-model`,
  `seex-storage`, and `seex-core` crates and re-exports.
- [ ] Mechanically rename `seex-chart-core` to unpublished `seex-plot` without
  behavior or performance changes. Keep it free of SDK, storage, PyO3, and GPUI
  dependencies.
- [ ] Make `seex` the only publishable workspace crate. Set `publish = false`
  for `seex-python`, `seex-app`, and `seex-plot`.
- [ ] Verify `cargo package -p seex` contains no path dependency or PyO3, GPUI,
  Desktop, or plot source; unpack and build/test it in an independent directory.
- [ ] Align Cargo `0.1.0-beta.1`, Python `0.1.0b1`, and tag
  `v0.1.0-beta.1` to one source without changing catalog/Parquet schemas or
  adding a runtime dependency.
- [ ] Run warning-free Rust formatting, Clippy, check, tests, doc tests, docs,
  package verification, Python gates, and all three performance domains.

### U6: Viewer Configuration and Workbench Experience

U6 completes the accepted Viewer configuration and workbench design after
crate convergence and before final release qualification. Keep Source
management, autosave, and export/import as separate candidates.

#### U6.1: Source management and scope

- [ ] Implement Source directory preflight, editable alias and Project
  multi-selection confirmation, configuration writeback, Manage Projects, and
  Reload Sources. Reject an invalid reload without replacing the last valid
  live Sources.
- [ ] Select project scope only from the Source root used to launch or
  explicitly open Viewer. Importing another Source does not change scope;
  configuration merges global and project documents, while workbenches never
  merge and no project scope uses `~/.seex/workbench.toml`.
- [ ] Keep Archive Project as reversible workbench state. Make Remove Project
  a confirmed unimport that removes the allowlist entry and its workbench
  references; retire persisted `removed_projects`.

#### U6.2: Semantic autosave

- [ ] Persist Views and the active View; selected, baseline, pinned, and
  archived Runs and Projects; Metrics, selected Metric, row heights, axis and
  viewport; major component visibility and dimensions; and expanded Projects.
- [ ] Exclude Source paths and allowlists, hover/focus and menu state, filter
  text, scroll positions, query results, pending tasks, cursors, and transient
  errors from workbench state.
- [ ] Have `WorkbenchSession` produce immutable semantic snapshots and
  coalesce changes. Serialize and atomically replace the file on a GPUI
  background task, then update entities on the foreground; render callbacks
  consume plain snapshots and never perform I/O or re-enter an Entity.
- [ ] Report invalid or unsupported documents without overwriting them. Retain
  unavailable Source, Project, and Run references across save and restart so
  they recover when mappings or data return.

#### U6.3: Export, import, and exit gates

- [ ] Export only `workbench.toml`, never config, Source paths, allowlists,
  native data, secrets, or transient state.
- [ ] Before import, autosave the current workbench and preflight every alias,
  Project, rewrite, and allowlist addition. After confirmation, complete all
  file writes before switching live configuration and workbench state; a
  failure keeps the previous live state.
- [ ] Cover every acceptance scenario in the accepted design, including
  owner-only secret files, invalid external edit fallback, alias remapping,
  missing-reference recovery, archive/unimport behavior, and failed import
  without a live-state switch.
- [ ] Pass pure Rust tests for configuration, permissions, aliases, codecs,
  legacy rejection, and source preservation; pass GPUI tests for Source
  confirmation, reload, autosave, archive/unimport, recovery, export, and
  import. Run `cargo check`, `cargo test`, and the protected Viewer gates.

### U7: Final Resource, Metal, and Release Qualification

U7 begins after U0–U6 pass. CI/release workflow changes remain a separate
review slice and require explicit approval under the repository boundary.

- [ ] Re-run complete Reader, reporting, Python, package, Viewer configuration,
  workbench, correctness, CPU, RSS, and stale-generation gates against original
  and rolling baselines.
- [ ] With 10 Runs and at least six visible Metrics, verify 2x Detail returned
  points are approximately halved and compact snapshot storage is at least 60%
  below the original baseline.
- [ ] Reduce real single- and dual-View peak RSS by at least 25% from the U0
  original baseline. Record automated warm, peak, final, and phase trend.
- [ ] After warm-up, complete 30 zoom-in/out cycles without monotonic RSS
  growth. Final RSS must remain within `max(5%, 32 MiB)` of warm steady state,
  and stale reads must retain no snapshot or query working set.
- [ ] On the active 280 Hz display, trace the converged brush/ruler/chart pan,
  wheel/pinch and keyboard zoom, hover/locked cursors, track scroll, View switch,
  and inspector path. No Viewer-caused presentation may span two refresh
  periods; at 280 Hz the two-period boundary is approximately 7.14 ms.
- [ ] Record RSS, allocation high-water marks, Metal instance-buffer growth,
  exact commands, environment, fixtures, raw/rolling deltas, and conclusion.
- [ ] Only after every resource gate passes, add a separately reviewed macOS
  ARM64 CI job that verifies the Xcode Metal Toolchain and builds the unsigned
  Viewer without changing the Python wheel matrix.
- [ ] On matching tags, produce `seex-app-macos-aarch64`, SHA-256 checksum, and
  attestations, while proving Desktop artifacts cannot publish to PyPI.
- [ ] Publish crates.io before PyPI. If the `seex` crate name cannot be claimed,
  stop and revisit the naming decision rather than silently choosing a fallback.

## Later Backlog

### Local Coordination

- [ ] Define and validate multi-client SQLite run-writer coordination before
  expanding the current single-writer native contract.

### Credentials and Remote Training

- [ ] Add environment-variable or AWS credential-chain discovery for S3
  credentials when explicit config-file credentials are insufficient.
- [ ] Revisit the [remote control-service boundary](drafts/remote-training-architecture-notes.md)
  when local training is complete and a real rented-GPU workflow exists, per
  [ADR 0012](adr/0012-defer-remote-training.md). Produce a remote-training ADR
  before adding remote writers or shared catalog coordination.
- [ ] Consider PostgreSQL catalog support only when service scale or
  availability requires it.

### Analysis and Agent Workflows

- [ ] Evaluate the [research driver](drafts/autoresearch-control-loop-notes.md)
  without moving source or Git mutation into Seex engine.
- [ ] Design config/tag filtering, data export, Web UI, MCP, and agent-facing
  surfaces as independently reviewable phases after the beta is qualified.
- [ ] Revisit cumulative-token and normalized-budget comparison axes,
  repetition/significance policy, and persisted research context as separate
  product and schema decisions.

## 1.0 / Stable Contract

1.0 freezes surfaces proven by pre-1.0 releases. It is the first compatibility
commitment; pre-1.0 stores and APIs remain unsupported unless explicitly named.

- [ ] Accept an ADR defining compatibility for the typed Rust/Python APIs,
  versioned CLI JSON, catalog application schema, and Parquet schema.
- [ ] Add an explicit store schema/version marker without changing the metric
  point Parquet compatibility boundary.
- [ ] Document additive changes, deprecation, breaking changes, and the support
  window for stable stores and machine-readable output.
- [ ] Define migration policy only for stores created after the stable 1.0
  boundary; pre-Seex stores remain unsupported.
